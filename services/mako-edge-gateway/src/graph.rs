use std::{
    error::Error,
    fmt,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::{NonZeroU64, NonZeroUsize},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use futures::executor::block_on;
use mako_api::{EnvironmentId, ProjectId, TenantScope};
use mako_audit::{
    ActorIdentity, AuditCategory, AuditEvent, AuditOutcome, AuditStore, AuditStoreConfig,
    CorrelationId, CursorSigningKey, RequestId, ResourceReference, SafeAttributes, SignalContext,
    SignalScope, TelemetryRedactor, TraceId,
};
use mako_config::{DeploymentEnvironment, ServiceConfig, ServiceKind};
use mako_edge_gateway::{
    FunctionAccessTokenVerifier, FunctionInvocationActor, FunctionInvocationAuditEvent,
    FunctionInvocationAuditOutcome, FunctionInvocationAuditSink, FunctionRouteError,
    FunctionRouteResolver, GatewayFunctionInvocationAdmission, RegionalDeploymentHealth,
    ResolvedFunctionRoute,
};
use mako_gateway::{
    AccessTokenVerificationError, GatewayQuotaEngine, GatewayQuotaEngineConfig, GatewayQuotaLimit,
    GatewayQuotaPolicy, GatewayQuotaResource, GatewayQuotaWindow, VerifiedAccessIdentity,
};
use mako_identity::{AccessAuthorizationEpochs, AppUserId, SessionId};
use mako_internal_rpc::{
    DeploymentKey, EdgeToControlClient, EdgeToDataClient, FunctionSecretResolutionRequest,
    FunctionSecretResolutionResponse, IdentityVerificationOperation, IdentityVerificationRequest,
    IdentityVerificationResponse, InternalCaller, InternalHttpClient, InternalHttpClientConfig,
};
use mako_service_runtime::{ReadinessProbe, ReadinessSnapshot};
use mako_storage::{
    Durability, KvAdapter, ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity,
    RocksDbAdapter, RocksDbConfig, StorageError, check_storage_readiness,
};
use rand_core::{OsRng, RngCore};

use crate::runtime::LoopbackRuntimeInvoker;

const DATA_PLANE_ENDPOINT: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
const CONTROL_PLANE_ENDPOINT: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8081);
const RUNTIME_ENDPOINT: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9000);
const AUDIT_RETENTION_MILLISECONDS: u64 = 90 * 24 * 60 * 60 * 1_000;

enum StorageOwner {
    Local(RocksDbAdapter),
    Production(Box<ProductionRocksDb>),
}

impl StorageOwner {
    fn open(config: &ServiceConfig) -> Result<Self, StorageError> {
        if config.environment == DeploymentEnvironment::Production {
            let identity = ProductionVolumeIdentity::new(
                config.service.name(),
                format!("{}-{}", config.service.name(), config.region),
            )?;
            return block_on(ProductionRocksDb::open(ProductionRocksDbConfig {
                database_path: config.rocksdb.path.clone(),
                identity,
                maximum_batch_operations: config.rocksdb.maximum_batch_operations,
                maximum_scan_items: config.rocksdb.maximum_scan_items,
                transaction_lock_timeout: config.rocksdb.transaction_lock_timeout,
                transaction_expiration: config.rocksdb.transaction_expiration,
                disk_warning_free_bytes: config.rocksdb.disk_warning_free_bytes,
                disk_critical_free_bytes: config.rocksdb.disk_critical_free_bytes,
            }))
            .map(Box::new)
            .map(Self::Production);
        }
        let mut rocks = RocksDbConfig::new(&config.rocksdb.path);
        rocks.minimum_durability = Durability::Sync;
        rocks.maximum_batch_operations = config.rocksdb.maximum_batch_operations;
        rocks.maximum_scan_items = config.rocksdb.maximum_scan_items;
        RocksDbAdapter::open(rocks).map(Self::Local)
    }

    fn adapter(&self) -> RocksDbAdapter {
        match self {
            Self::Local(adapter) => adapter.clone(),
            Self::Production(storage) => storage.adapter().clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct PrivateRouteResolver {
    client: EdgeToControlClient,
    region: String,
}

#[async_trait]
impl FunctionRouteResolver for PrivateRouteResolver {
    async fn resolve(
        &self,
        project_ref: &str,
        function_name: &str,
    ) -> Result<Option<ResolvedFunctionRoute>, FunctionRouteError> {
        let tenant = tenant_from_project_ref(project_ref).ok_or(FunctionRouteError::Unavailable)?;
        let request_id = internal_request_id("route", project_ref, function_name);
        let response: FunctionSecretResolutionResponse = match self.client.resolve(
            &tenant,
            &request_id,
            &FunctionSecretResolutionRequest {
                project_ref: project_ref.to_owned(),
                function_name: function_name.to_owned(),
                region: self.region.clone(),
            },
        ) {
            Ok(response) => response,
            Err(mako_internal_rpc::InternalClientError::Remote { status: 404, .. }) => {
                return Ok(None);
            }
            Err(_) => return Err(FunctionRouteError::Unavailable),
        };
        let organization_id = response.organization_id.clone();
        if response.function_name != function_name
            || response.version == 0
            || response.secrets.iter().any(|secret| {
                secret.name.is_empty()
                    || secret.version == 0
                    || secret.value.len() > 64 * 1024
                    || secret.value.chars().any(char::is_control)
            })
        {
            return Err(FunctionRouteError::Unavailable);
        }
        let regional_deployments = response
            .selected_regions
            .iter()
            .map(|region| RegionalDeploymentHealth {
                region: region.clone(),
                healthy: region == &self.region,
                valid_until_unix_seconds: now_unix_seconds().saturating_add(10),
            })
            .collect();
        Ok(Some(ResolvedFunctionRoute {
            tenant,
            organization_id,
            function_name: response.function_name,
            active_version: response.version,
            selected_regions: response.selected_regions,
            regional_deployments,
            verify_jwt: response.verify_jwt,
            request_limit_bytes: response.request_limit_bytes,
            response_limit_bytes: response.response_limit_bytes,
        }))
    }
}

#[derive(Clone)]
pub(crate) struct PrivateTokenVerifier {
    client: EdgeToDataClient,
}

impl FunctionAccessTokenVerifier for PrivateTokenVerifier {
    fn verify(
        &self,
        token: &str,
        tenant: &TenantScope,
        _now_unix_seconds: u64,
    ) -> Result<VerifiedAccessIdentity, AccessTokenVerificationError> {
        let request_id = internal_request_id(
            "token",
            tenant.project_id().as_str(),
            tenant.environment_id().as_str(),
        );
        let response: IdentityVerificationResponse = self
            .client
            .verify(
                tenant,
                &request_id,
                &IdentityVerificationRequest {
                    operation: IdentityVerificationOperation::AccessToken,
                    presented_credential: token.to_owned(),
                    collection_id: None,
                    requested_operation: None,
                },
            )
            .map_err(|_| AccessTokenVerificationError::Malformed)?;
        let IdentityVerificationResponse::AccessToken {
            user_id,
            role,
            session_id,
            environment_authorization_epoch,
            user_authorization_epoch,
            trusted_claims,
        } = response
        else {
            return Err(AccessTokenVerificationError::Malformed);
        };
        VerifiedAccessIdentity::from_authority(
            tenant.clone(),
            AppUserId::parse(user_id).map_err(|_| AccessTokenVerificationError::Malformed)?,
            role,
            SessionId::parse(session_id).map_err(|_| AccessTokenVerificationError::Malformed)?,
            AccessAuthorizationEpochs {
                environment: environment_authorization_epoch,
                user: user_authorization_epoch,
            },
            trusted_claims,
        )
    }
}

pub(crate) struct PersistentFunctionAudit {
    store: AuditStore,
    redactor: TelemetryRedactor,
    healthy: AtomicBool,
}

impl PersistentFunctionAudit {
    fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
}

impl FunctionInvocationAuditSink for PersistentFunctionAudit {
    fn record(&self, event: FunctionInvocationAuditEvent) {
        let context = &event.context;
        let actor = match &context.actor {
            FunctionInvocationActor::ApplicationUser {
                user_id,
                session_id,
            } => ActorIdentity::ApplicationUser {
                actor_id: user_id.clone(),
                session_id: session_id.clone(),
            },
            FunctionInvocationActor::PublicWebhook => ActorIdentity::Anonymous,
        };
        let outcome = match event.outcome {
            FunctionInvocationAuditOutcome::Admitted => AuditOutcome::Allowed,
            FunctionInvocationAuditOutcome::Throttled
            | FunctionInvocationAuditOutcome::QuotaRejected => AuditOutcome::Denied,
            FunctionInvocationAuditOutcome::RuntimeRejected => AuditOutcome::Failed,
        };
        let digest = blake3::hash(format!("{}:{:?}", context.request_id, event.outcome).as_bytes())
            .to_hex()
            .to_string();
        let built = (|| {
            Some(AuditEvent {
                context: SignalContext::new(
                    SignalScope::Tenant {
                        tenant: context.tenant.clone(),
                        organization_id: Some(context.organization_id.clone()),
                    },
                    actor,
                    ResourceReference::new("function", context.function_name.clone()).ok()?,
                    RequestId::parse(context.request_id.clone()).ok()?,
                    CorrelationId::parse(context.request_id.clone()).ok()?,
                    TraceId::parse(context.trace_id.clone()).ok(),
                )
                .ok()?,
                event_id: format!("evt_{}", &digest[..32]),
                occurred_at_unix_milliseconds: now_unix_seconds().max(1).saturating_mul(1_000),
                action: "function_invoke".to_owned(),
                outcome,
                reason_code: match event.outcome {
                    FunctionInvocationAuditOutcome::Admitted => "admitted",
                    FunctionInvocationAuditOutcome::Throttled => "throttled",
                    FunctionInvocationAuditOutcome::QuotaRejected => "quota_rejected",
                    FunctionInvocationAuditOutcome::RuntimeRejected => "runtime_rejected",
                }
                .to_owned(),
                details: SafeAttributes::default(),
            })
        })();
        let Some(audit) = built else {
            eprintln!("function audit rejected a locally constructed event");
            self.healthy.store(false, Ordering::Release);
            return;
        };
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    block_on(self.store.append(
                        &context.tenant,
                        AuditCategory::Function,
                        audit,
                        &self.redactor,
                    ))
                })
                .join()
        });
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                eprintln!("function audit persistence failed: {error:?}");
                self.healthy.store(false, Ordering::Release);
            }
            Err(_) => {
                eprintln!("function audit persistence worker panicked");
                self.healthy.store(false, Ordering::Release);
            }
        }
    }
}

pub struct EdgeGatewayGraph {
    _storage: StorageOwner,
    adapter: Arc<dyn KvAdapter>,
    pub(crate) routes: PrivateRouteResolver,
    pub(crate) tokens: PrivateTokenVerifier,
    pub(crate) admission: GatewayFunctionInvocationAdmission,
    pub(crate) audit: Arc<PersistentFunctionAudit>,
    pub(crate) runtime: LoopbackRuntimeInvoker,
    data_client: EdgeToDataClient,
    control_client: EdgeToControlClient,
    region: String,
}

impl EdgeGatewayGraph {
    pub fn open(config: &ServiceConfig) -> Result<Self, EdgeGatewayGraphError> {
        if config.service != ServiceKind::EdgeGateway {
            return Err(EdgeGatewayGraphError::WrongService);
        }
        let secret = config
            .internal_auth_secret
            .as_ref()
            .ok_or(EdgeGatewayGraphError::MissingKeyMaterial)?;
        let deployment_key = DeploymentKey::derive(secret.expose_secret())?;
        let storage = StorageOwner::open(config)?;
        let adapter: Arc<dyn KvAdapter> = Arc::new(storage.adapter());
        if !block_on(check_storage_readiness(adapter.as_ref(), Durability::Sync)).is_ready() {
            return Err(EdgeGatewayGraphError::StorageNotReady);
        }
        let data_client = EdgeToDataClient::new(InternalHttpClient::new(
            InternalHttpClientConfig::loopback(DATA_PLANE_ENDPOINT),
            deployment_key.clone(),
            InternalCaller::EdgeGateway,
        )?)?;
        let control_client = EdgeToControlClient::new(InternalHttpClient::new(
            InternalHttpClientConfig::loopback(CONTROL_PLANE_ENDPOINT),
            deployment_key,
            InternalCaller::EdgeGateway,
        )?)?;
        let engine = Arc::new(GatewayQuotaEngine::new(
            Arc::clone(&adapter),
            GatewayQuotaEngineConfig {
                durability: Durability::Sync,
                maximum_conflict_retries: NonZeroUsize::new(128).expect("nonzero"),
                maximum_charges_per_request: NonZeroUsize::new(8).expect("nonzero"),
            },
        ));
        let policy = Arc::new(GatewayQuotaPolicy::new([
            (
                GatewayQuotaResource::FunctionInvocations,
                rate_limit(600, 60_000),
            ),
            (
                GatewayQuotaResource::PublicFunctionInvocations,
                rate_limit(120, 60_000),
            ),
            (
                GatewayQuotaResource::FunctionRequestBytes,
                rate_limit(64 * 1024 * 1024, 60_000),
            ),
        ])?);
        let audit_store = AuditStore::new(
            Arc::clone(&adapter),
            AuditStoreConfig {
                durability: Durability::Sync,
                retention_milliseconds: AUDIT_RETENTION_MILLISECONDS,
                maximum_page_records: NonZeroUsize::new(100).expect("nonzero"),
                maximum_export_records: NonZeroUsize::new(10_000).expect("nonzero"),
                maximum_examined_records: NonZeroUsize::new(50_000).expect("nonzero"),
                cursor_signing_key: CursorSigningKey::new(blake3::derive_key(
                    "mako/edge-gateway/audit-cursor-signing/v1",
                    secret.expose_secret().as_bytes(),
                )),
            },
        )?;
        let audit = Arc::new(PersistentFunctionAudit {
            store: audit_store,
            redactor: TelemetryRedactor::new([secret.expose_secret()])?,
            healthy: AtomicBool::new(true),
        });
        Ok(Self {
            _storage: storage,
            adapter,
            routes: PrivateRouteResolver {
                client: control_client.clone(),
                region: config.region.clone(),
            },
            tokens: PrivateTokenVerifier {
                client: data_client.clone(),
            },
            admission: GatewayFunctionInvocationAdmission::new(engine, policy),
            audit,
            runtime: LoopbackRuntimeInvoker::new(RUNTIME_ENDPOINT),
            data_client,
            control_client,
            region: config.region.clone(),
        })
    }

    pub(crate) fn region(&self) -> &str {
        &self.region
    }
}

impl ReadinessProbe for EdgeGatewayGraph {
    fn snapshot(&self) -> ReadinessSnapshot {
        let storage = block_on(check_storage_readiness(
            self.adapter.as_ref(),
            Durability::Sync,
        ))
        .is_ready();
        let data = self.data_client.dependency_ready().unwrap_or(false);
        let control = self.control_client.dependency_ready().unwrap_or(false);
        let runtime = self.runtime.dependency_ready();
        let audit = self.audit.healthy();
        if storage && data && control && runtime && audit {
            ReadinessSnapshot::ready("storage_identity_control_runtime_audit_ready")
        } else {
            ReadinessSnapshot::not_ready(format!(
                "dependencies_not_ready:storage={storage},data={data},control={control},runtime={runtime},audit={audit}"
            ))
        }
    }
}

fn tenant_from_project_ref(value: &str) -> Option<TenantScope> {
    let (project, environment_suffix) = value.rsplit_once("--env_")?;
    Some(TenantScope::new(
        ProjectId::parse(project.to_owned()).ok()?,
        EnvironmentId::parse(format!("env_{environment_suffix}")).ok()?,
    ))
}

fn internal_request_id(domain: &str, left: &str, right: &str) -> String {
    let mut random = [0_u8; 32];
    OsRng.fill_bytes(&mut random);
    let digest = blake3::keyed_hash(
        &random,
        format!("{domain}:{left}:{right}:{}", now_unix_seconds()).as_bytes(),
    )
    .to_hex()
    .to_string();
    format!("req_{}", &digest[..32])
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |duration| duration.as_secs().max(1))
}

fn rate_limit(limit: u64, window_milliseconds: u64) -> GatewayQuotaLimit {
    GatewayQuotaLimit {
        hard: None,
        rate: Some(GatewayQuotaWindow {
            limit: NonZeroU64::new(limit).expect("nonzero quota limit"),
            window_milliseconds: NonZeroU64::new(window_milliseconds)
                .expect("nonzero quota window"),
        }),
    }
}

#[derive(Debug)]
pub enum EdgeGatewayGraphError {
    WrongService,
    MissingKeyMaterial,
    StorageNotReady,
    Storage(StorageError),
    Internal(mako_internal_rpc::InternalClientError),
    InternalAuth(mako_internal_rpc::InternalAuthError),
    Quota(mako_gateway::GatewayQuotaError),
    Audit(mako_audit::AuditStoreError),
    Redaction(mako_audit::RedactionError),
}

impl fmt::Display for EdgeGatewayGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("edge gateway production composition failed")
    }
}

impl Error for EdgeGatewayGraphError {}

impl From<StorageError> for EdgeGatewayGraphError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}
impl From<mako_internal_rpc::InternalClientError> for EdgeGatewayGraphError {
    fn from(value: mako_internal_rpc::InternalClientError) -> Self {
        Self::Internal(value)
    }
}
impl From<mako_internal_rpc::InternalAuthError> for EdgeGatewayGraphError {
    fn from(value: mako_internal_rpc::InternalAuthError) -> Self {
        Self::InternalAuth(value)
    }
}
impl From<mako_gateway::GatewayQuotaError> for EdgeGatewayGraphError {
    fn from(value: mako_gateway::GatewayQuotaError) -> Self {
        Self::Quota(value)
    }
}
impl From<mako_audit::AuditStoreError> for EdgeGatewayGraphError {
    fn from(value: mako_audit::AuditStoreError) -> Self {
        Self::Audit(value)
    }
}
impl From<mako_audit::RedactionError> for EdgeGatewayGraphError {
    fn from(value: mako_audit::RedactionError) -> Self {
        Self::Redaction(value)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use mako_config::ConfigLoader;
    use mako_storage::{ProductionVolumeIdentity, provision_production_volume};
    use tempfile::{Builder, TempDir};

    use super::*;

    #[test]
    fn local_graph_registers_only_the_stable_invocation_contract() {
        let directory = local_tempdir("edge-gateway-local");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let graph = Arc::new(EdgeGatewayGraph::open(&config).expect("edge graph"));
        let router = crate::edge_gateway_router(graph).expect("edge routes");
        assert_eq!(router.route_count(), 7);
        for method in [
            mako_service_runtime::HttpMethod::Get,
            mako_service_runtime::HttpMethod::Head,
            mako_service_runtime::HttpMethod::Post,
            mako_service_runtime::HttpMethod::Put,
            mako_service_runtime::HttpMethod::Patch,
            mako_service_runtime::HttpMethod::Delete,
            mako_service_runtime::HttpMethod::Options,
        ] {
            assert!(router.permits(
                method,
                "/prj_example00--env_example00/functions/v1/hello-world"
            ));
        }
        assert!(!router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/_internal/v1/control/functions/resolve"
        ));
        assert!(!router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/prj_example00--env_example00/functions/v2/hello-world"
        ));
    }

    #[test]
    fn production_requires_and_opens_only_its_owned_volume() {
        let empty = local_tempdir("edge-gateway-empty-production");
        let empty_config = config_for(empty.path(), DeploymentEnvironment::Production);
        assert!(matches!(
            EdgeGatewayGraph::open(&empty_config),
            Err(EdgeGatewayGraphError::Storage(_))
        ));

        let directory = local_tempdir("edge-gateway-production");
        let config = config_for(directory.path(), DeploymentEnvironment::Production);
        provision_production_volume(
            &config.rocksdb.path,
            ProductionVolumeIdentity::new("mako-edge-gateway", "mako-edge-gateway-us-east-1-beta")
                .expect("volume identity"),
        )
        .expect("provision volume");
        EdgeGatewayGraph::open(&config).expect("production edge graph");
    }

    #[test]
    fn persistent_function_audit_accepts_the_hosted_invocation_shape() {
        let directory = local_tempdir("edge-gateway-audit");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let graph = EdgeGatewayGraph::open(&config).expect("edge graph");
        let tenant = TenantScope::new(
            ProjectId::parse("prj_d2b2dcc3f5acbcc2105200bafbde2134".to_owned()).expect("project"),
            EnvironmentId::parse("env_4cfece7ecdaeba1f83064208c1cd84d6".to_owned())
                .expect("environment"),
        );
        graph.audit.record(FunctionInvocationAuditEvent {
            context: mako_edge_gateway::FunctionInvocationAuditContext {
                tenant,
                organization_id: "org_74d122b2a75fbf4f9fe389f62156f0e9".to_owned(),
                function_name: "qualification-stream".to_owned(),
                version: 1,
                region: "us-east-1-beta".to_owned(),
                regional_failover: false,
                request_id: "req_0123456789abcdef01234567".to_owned(),
                trace_id: "trc_4bf92f3577b34da6a3ce929d0e0e4736".to_owned(),
                actor: FunctionInvocationActor::ApplicationUser {
                    user_id: "usr_8c4c7ec8d2f8ce2d37252f8eed4886df92fe1b5ae166d63ad5beda788df02dc0"
                        .to_owned(),
                    session_id: "ses_0123456789abcdef".to_owned(),
                },
            },
            outcome: FunctionInvocationAuditOutcome::Admitted,
        });
        assert!(graph.audit.healthy());
    }

    fn config_for(root: &Path, environment: DeploymentEnvironment) -> ServiceConfig {
        let rocksdb = root.join("rocksdb");
        let backup = root.join("backup");
        fs::create_dir_all(&rocksdb).expect("database directory");
        fs::create_dir_all(&backup).expect("backup directory");
        ConfigLoader::from_environment([
            (
                "MAKO_ENVIRONMENT",
                if environment == DeploymentEnvironment::Production {
                    "production"
                } else {
                    "local"
                },
            ),
            ("MAKO_REGION", "us-east-1-beta"),
            ("MAKO_PUBLIC_URL", "https://api.example.test"),
            ("MAKO_ROCKSDB_PATH", rocksdb.to_str().expect("UTF-8 path")),
            (
                "MAKO_ROCKSDB_BACKUP_DESTINATION",
                backup.to_str().expect("UTF-8 path"),
            ),
            ("MAKO_ROCKSDB_DISK_WARNING_FREE_BYTES", "134217728"),
            ("MAKO_ROCKSDB_DISK_CRITICAL_FREE_BYTES", "67108864"),
            (
                "MAKO_INTERNAL_AUTH_SECRET_REF",
                "env:TEST_EDGE_GATEWAY_ROOT_KEY",
            ),
            (
                "TEST_EDGE_GATEWAY_ROOT_KEY",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
        ])
        .load(ServiceKind::EdgeGateway)
        .expect("configuration")
    }

    fn local_tempdir(prefix: &str) -> TempDir {
        let root = std::env::current_dir()
            .expect("working directory")
            .join(".local");
        fs::create_dir_all(&root).expect("local test root");
        Builder::new()
            .prefix(prefix)
            .tempdir_in(root)
            .expect("temporary directory")
    }
}
