use std::{
    collections::HashMap,
    error::Error,
    fmt,
    num::{NonZeroU64, NonZeroUsize},
    sync::{Arc, RwLock},
    time::Duration,
};

use futures::executor::block_on;
use mako_api::{CollectionScope, ExplorerCapabilityKey, ExplorerCapabilityKeyRing, TenantScope};
use mako_audit::{
    AuditStore, AuditStoreConfig, AuditStoreError, CursorSigningKey, RedactionError,
    TelemetryRedactor,
};
use mako_config::{DeploymentEnvironment, ServiceConfig, ServiceKind};
use mako_documents::DocumentEngine;
use mako_gateway::{
    AccessTokenVerificationConfig, AccessTokenVerificationError, AuthorizedReplicationRequest,
    GatewayAccessTokenVerifier, GatewayAuthorizationEpochResolver, GatewayQuotaEngine,
    GatewayQuotaEngineConfig, GatewayQuotaLimit, GatewayQuotaPolicy, GatewayQuotaResource,
    GatewayQuotaWindow, GatewayReplicationQuotaEnforcer, GatewaySessionResolver,
    PersistentQuotaPolicySource, ReplicationGateway, ReplicationGatewayError,
    ReplicationGatewayRequest, VerifiedAccessIdentity,
};
use mako_identity::{
    AccessAuthorizationEpochs, AccessToken, AccessTokenConfig, AccessTokenError, AccessTokenInput,
    AccessTokenIssuer, AppUserId, AppUserRecord, ApplicationSessionGrant, ApplicationSessionStore,
    ApplicationSessionStoreError, Argon2idParameters, FixedWindowRateLimitConfig, IdentityStore,
    IdentityStoreError, KeyEncryptionKey, NormalizedEmail, PasswordPolicy, PasswordService,
    PersistentSignInThrottle, ProjectSigningKeyRecord, ProjectSigningKeyRing,
    ProjectSigningKeyStore, ProjectSigningKeyStoreError, RateLimitError, RefreshCredential,
    RefreshSessionOutcome, SessionId,
};
use mako_internal_rpc::{
    DeploymentKey, EncryptedResponseJournal, InternalAuthError, InternalCaller,
    InternalRequestAuthenticator, ResponseJournalError, RocksInternalReplayGuard,
    RocksInternalReplayGuardError,
};
use mako_policy::{
    AuthorizationEpochError, AuthorizationEpochStore, ExplorerAuthorizationError,
    ExplorerAuthorizationStore, PolicyContextError, PolicyStore, PolicyStoreError, SubjectId,
};
use mako_service_runtime::{CrossOriginPolicy, HttpRequestHead, ReadinessProbe, ReadinessSnapshot};
use mako_storage::{
    Durability, KvAdapter, ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity,
    RocksDbAdapter, RocksDbConfig, ScanDirection, ScanRequest, StorageError, StorageReadiness,
    TenantKeyspace, WriteBatch, check_storage_readiness,
};
use mako_sync::ReplicationTokenKey;

use crate::explorer_metrics::{ExplorerMetrics, ExplorerMetricsSnapshot};

const ACCESS_TOKEN_AUDIENCE: &str = "mako-data";
const ACCESS_TOKEN_LIFETIME_SECONDS: u64 = 300;
const ACCESS_TOKEN_CLOCK_SKEW_SECONDS: u64 = 30;
const AUDIT_RETENTION_MILLISECONDS: u64 = 90 * 24 * 60 * 60 * 1_000;
const ONE_MINUTE_MILLISECONDS: u64 = 60_000;
const ONE_DAY_MILLISECONDS: u64 = 24 * 60 * 60 * 1_000;

/// Confirms which local RocksDB startup path owns the graph. No memory variant
/// exists, so production code cannot silently select one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageMode {
    LocalRocksDb,
    ProductionRocksDb,
}

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
        rocks.transaction_lock_timeout = config.rocksdb.transaction_lock_timeout;
        rocks.transaction_expiration = config.rocksdb.transaction_expiration;
        RocksDbAdapter::open(rocks).map(Self::Local)
    }

    fn adapter(&self) -> RocksDbAdapter {
        match self {
            Self::Local(adapter) => adapter.clone(),
            Self::Production(storage) => storage.adapter().clone(),
        }
    }

    fn mode(&self) -> StorageMode {
        match self {
            Self::Local(_) => StorageMode::LocalRocksDb,
            Self::Production(_) => StorageMode::ProductionRocksDb,
        }
    }

    fn initial_readiness(&self) -> StorageReadiness {
        match self {
            Self::Local(adapter) => block_on(check_storage_readiness(adapter, Durability::Sync)),
            Self::Production(storage) => storage.readiness().clone(),
        }
    }

    async fn shutdown(self) -> Result<(), StorageError> {
        match self {
            Self::Local(adapter) => {
                adapter.health().await?;
                drop(adapter);
                Ok(())
            }
            Self::Production(storage) => storage.graceful_shutdown().await,
        }
    }
}

/// Application objects never share a bucket with function bundles or artifacts.
const APPLICATION_OBJECT_BUCKET: &str = "mako-application-objects-v1";

struct DataPlaneComponents {
    documents: DocumentEngine,
    object_store: Arc<mako_object_store::S3ObjectStore>,
    object_key_root: mako_file_storage::ObjectKeyRoot,
    enforce_object_store: bool,
    /// Where the outside world reaches this node: provider callbacks come here.
    public_url: String,
    provider_secret_key: mako_auth_providers::ProviderSecretKey,
    flow_state: mako_auth_providers::FlowStateVerifier,
    provider_client: mako_auth_providers::ProviderClient,
    telemetry: Arc<mako_telemetry_client::TelemetryEmitter>,
    storage_sampler: Arc<crate::telemetry::StorageSampler>,
    quota_checkpoints: Arc<crate::telemetry::QuotaCheckpointer>,
    quotas: Arc<GatewayQuotaEngine>,
    quota_policy: GatewayQuotaPolicy,
    quota_policies: Arc<PersistentQuotaPolicySource>,
    custom_domains: CustomDomainRegistry,
    allowed_origins: AllowedOriginRegistry,
    audit: AuditStore,
    redactor: TelemetryRedactor,
    password_service: PasswordService,
    sign_in_throttle: PersistentSignInThrottle,
    replication_gateway: ReplicationGateway,
    gateway_state: PersistentGatewayState,
    signing_encryption_key: KeyEncryptionKey,
    session_protection_key: KeyEncryptionKey,
    internal_deployment_key: DeploymentKey,
    access_token_config: AccessTokenConfig,
    verification_config: AccessTokenVerificationConfig,
    replication_token_key: ReplicationTokenKey,
    explorer_capability_keys: ExplorerCapabilityKeyRing,
    explorer_cursor_key: [u8; 32],
    explorer_metrics: ExplorerMetrics,
}

const CUSTOM_DOMAINS_DOMAIN: &[u8] = b"data/custom-domains/v1";
/// Environments with custom domains on one node, within the adapter's own
/// scan bound; a saturated scan at startup fails closed.
const MAXIMUM_CUSTOM_DOMAIN_ENTRIES: usize = 10_000;
/// The most hostnames one environment may be served on.
pub const MAXIMUM_CUSTOM_DOMAINS_PER_ENVIRONMENT: usize = 64;

/// The verified custom hostnames the control plane installed, per
/// environment: persisted in this node's keyspace and mirrored in memory
/// so every application request can be checked without a read.
pub struct CustomDomainRegistry {
    adapter: Arc<dyn KvAdapter>,
    hosts: RwLock<HashMap<String, TenantScope>>,
}

impl fmt::Debug for CustomDomainRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CustomDomainRegistry")
            .finish_non_exhaustive()
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CustomDomainEntry {
    project_id: String,
    environment_id: String,
    hostnames: Vec<String>,
}

impl CustomDomainRegistry {
    fn key(tenant: &TenantScope) -> Result<Vec<u8>, StorageError> {
        TenantKeyspace::system_key(
            CUSTOM_DOMAINS_DOMAIN,
            format!("{}/{}", tenant.project_id(), tenant.environment_id()),
        )
        .map_err(|error| StorageError::invalid("custom_domain_key", error.to_string()))
    }

    fn load(adapter: Arc<dyn KvAdapter>) -> Result<Self, DataPlaneGraphError> {
        let range = TenantKeyspace::system_domain_range(CUSTOM_DOMAINS_DOMAIN)
            .map_err(|_| DataPlaneGraphError::StorageNotReady)?;
        let entries = block_on(adapter.scan(ScanRequest::new(
            range,
            ScanDirection::Forward,
            NonZeroUsize::new(MAXIMUM_CUSTOM_DOMAIN_ENTRIES).expect("nonzero"),
        )))?;
        if entries.len() >= MAXIMUM_CUSTOM_DOMAIN_ENTRIES {
            return Err(DataPlaneGraphError::StorageNotReady);
        }
        let mut hosts = HashMap::new();
        for entry in entries {
            let entry: CustomDomainEntry = serde_json::from_slice(&entry.value)
                .map_err(|_| DataPlaneGraphError::StorageNotReady)?;
            let tenant = TenantScope::require(Some(&entry.project_id), Some(&entry.environment_id))
                .map_err(|_| DataPlaneGraphError::StorageNotReady)?;
            for hostname in entry.hostnames {
                hosts.insert(hostname, tenant.clone());
            }
        }
        Ok(Self {
            adapter,
            hosts: RwLock::new(hosts),
        })
    }

    /// Whether `hostname` is a verified custom domain of exactly `tenant`.
    #[must_use]
    pub fn permits(&self, tenant: &TenantScope, hostname: &str) -> bool {
        self.hosts
            .read()
            .ok()
            .is_some_and(|hosts| hosts.get(hostname) == Some(tenant))
    }

    /// The hostnames installed for an environment, sorted.
    #[must_use]
    pub fn hostnames(&self, tenant: &TenantScope) -> Vec<String> {
        let mut hostnames = self.hosts.read().map_or_else(
            |_| Vec::new(),
            |hosts| {
                hosts
                    .iter()
                    .filter(|(_, owner)| *owner == tenant)
                    .map(|(hostname, _)| hostname.clone())
                    .collect::<Vec<_>>()
            },
        );
        hostnames.sort();
        hostnames
    }

    /// Replaces an environment's list: persisted first, then mirrored, so a
    /// restart sees what a request saw.
    pub async fn install(
        &self,
        tenant: &TenantScope,
        hostnames: Vec<String>,
    ) -> Result<(), StorageError> {
        let key = Self::key(tenant)?;
        let mut batch = WriteBatch::with_capacity(1);
        if hostnames.is_empty() {
            batch.delete(key);
        } else {
            let encoded = serde_json::to_vec(&CustomDomainEntry {
                project_id: tenant.project_id().as_str().to_owned(),
                environment_id: tenant.environment_id().as_str().to_owned(),
                hostnames: hostnames.clone(),
            })
            .map_err(|_| {
                StorageError::invalid(
                    "custom_domain_install",
                    "custom domain entry could not be encoded",
                )
            })?;
            batch.put(key, encoded);
        }
        self.adapter.write(batch, Durability::Sync).await?;
        if let Ok(mut hosts) = self.hosts.write() {
            hosts.retain(|_, owner| owner != tenant);
            for hostname in hostnames {
                hosts.insert(hostname, tenant.clone());
            }
        }
        Ok(())
    }
}

const ALLOWED_ORIGINS_DOMAIN: &[u8] = b"data/allowed-origins/v1";
/// Environments with an allowlist on one node, within the adapter's own
/// scan bound; a saturated scan at startup fails closed.
const MAXIMUM_ALLOWED_ORIGIN_ENTRIES: usize = 10_000;

/// The browser origins each environment allows cross-origin, as the control
/// plane installed them: persisted in this node's keyspace and mirrored in
/// memory so every request can be answered without a read.
///
/// The list belongs to the environment, not to a hostname, so it decides a
/// request on the platform's own hostname exactly as it decides one on a
/// custom domain the environment is served on.
pub struct AllowedOriginRegistry {
    adapter: Arc<dyn KvAdapter>,
    origins: RwLock<HashMap<TenantScope, Vec<String>>>,
}

impl fmt::Debug for AllowedOriginRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AllowedOriginRegistry")
            .finish_non_exhaustive()
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AllowedOriginEntry {
    project_id: String,
    environment_id: String,
    origins: Vec<String>,
}

impl AllowedOriginRegistry {
    fn key(tenant: &TenantScope) -> Result<Vec<u8>, StorageError> {
        TenantKeyspace::system_key(
            ALLOWED_ORIGINS_DOMAIN,
            format!("{}/{}", tenant.project_id(), tenant.environment_id()),
        )
        .map_err(|error| StorageError::invalid("allowed_origin_key", error.to_string()))
    }

    fn load(adapter: Arc<dyn KvAdapter>) -> Result<Self, DataPlaneGraphError> {
        let range = TenantKeyspace::system_domain_range(ALLOWED_ORIGINS_DOMAIN)
            .map_err(|_| DataPlaneGraphError::StorageNotReady)?;
        let entries = block_on(adapter.scan(ScanRequest::new(
            range,
            ScanDirection::Forward,
            NonZeroUsize::new(MAXIMUM_ALLOWED_ORIGIN_ENTRIES).expect("nonzero"),
        )))?;
        if entries.len() >= MAXIMUM_ALLOWED_ORIGIN_ENTRIES {
            return Err(DataPlaneGraphError::StorageNotReady);
        }
        let mut origins = HashMap::new();
        for entry in entries {
            let entry: AllowedOriginEntry = serde_json::from_slice(&entry.value)
                .map_err(|_| DataPlaneGraphError::StorageNotReady)?;
            let tenant = TenantScope::require(Some(&entry.project_id), Some(&entry.environment_id))
                .map_err(|_| DataPlaneGraphError::StorageNotReady)?;
            origins.insert(tenant, entry.origins);
        }
        Ok(Self {
            adapter,
            origins: RwLock::new(origins),
        })
    }

    /// Whether `origin` may call `tenant`'s application API cross-origin: an
    /// exact match against the installed list, and never a match for an
    /// environment with none.
    #[must_use]
    pub fn allows(&self, tenant: &TenantScope, origin: &str) -> bool {
        self.origins.read().ok().is_some_and(|origins| {
            origins
                .get(tenant)
                .is_some_and(|allowed| allowed.iter().any(|listed| listed == origin))
        })
    }

    /// The list installed for an environment, in the order it was given.
    #[must_use]
    pub fn installed(&self, tenant: &TenantScope) -> Vec<String> {
        self.origins.read().map_or_else(
            |_| Vec::new(),
            |origins| origins.get(tenant).cloned().unwrap_or_default(),
        )
    }

    /// Replaces an environment's list: persisted first, then mirrored, so a
    /// restart sees what a request saw.
    pub async fn install(
        &self,
        tenant: &TenantScope,
        origins: Vec<String>,
    ) -> Result<(), StorageError> {
        let key = Self::key(tenant)?;
        let mut batch = WriteBatch::with_capacity(1);
        if origins.is_empty() {
            batch.delete(key);
        } else {
            let encoded = serde_json::to_vec(&AllowedOriginEntry {
                project_id: tenant.project_id().as_str().to_owned(),
                environment_id: tenant.environment_id().as_str().to_owned(),
                origins: origins.clone(),
            })
            .map_err(|_| {
                StorageError::invalid(
                    "allowed_origin_install",
                    "allowed origin entry could not be encoded",
                )
            })?;
            batch.put(key, encoded);
        }
        self.adapter.write(batch, Durability::Sync).await?;
        if let Ok(mut installed) = self.origins.write() {
            if origins.is_empty() {
                installed.remove(tenant);
            } else {
                installed.insert(tenant.clone(), origins);
            }
        }
        Ok(())
    }
}

/// The data plane's answer to "may this origin call this environment": the
/// allowlist the control plane installed for the environment the path
/// names. Only an application route has one -- the developer workspace and
/// the service-credential routes are never answered cross-origin -- and a
/// path whose tenant cannot be read answers nothing.
pub struct EnvironmentAllowedOrigins(Arc<DataPlaneGraph>);

impl EnvironmentAllowedOrigins {
    #[must_use]
    pub const fn new(graph: Arc<DataPlaneGraph>) -> Self {
        Self(graph)
    }
}

impl fmt::Debug for EnvironmentAllowedOrigins {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnvironmentAllowedOrigins")
            .finish_non_exhaustive()
    }
}

impl CrossOriginPolicy for EnvironmentAllowedOrigins {
    fn allows_origin(&self, request: &HttpRequestHead, origin: &str) -> bool {
        application_tenant(request.path())
            .is_some_and(|tenant| self.0.allowed_origins().allows(&tenant, origin))
    }
}

/// The environment an application route addresses:
/// `/v1/projects/{projectId}/environments/{environmentId}/{surface}/...`
/// where the surface is one a browser application calls with its own
/// session or public key. Every other path -- the service-credential
/// routes, the developer workspace's explorer, the private internal
/// protocol, anything unrecognized -- has no environment for this purpose,
/// and so is never answered cross-origin.
fn application_tenant(path: &str) -> Option<TenantScope> {
    let mut segments = path.strip_prefix("/v1/projects/")?.split('/');
    let project_id = segments.next()?;
    if segments.next()? != "environments" {
        return None;
    }
    let environment_id = segments.next()?;
    if !matches!(segments.next()?, "auth" | "collections" | "storage") {
        return None;
    }
    TenantScope::require(Some(project_id), Some(environment_id)).ok()
}

/// Complete storage-backed dependency graph for the production data plane.
/// Every tenant-scoped authority created by this graph shares the one owned
/// adapter and requires synchronous durability.
pub struct DataPlaneGraph {
    storage: StorageOwner,
    adapter: Arc<dyn KvAdapter>,
    components: DataPlaneComponents,
}

impl DataPlaneGraph {
    pub fn open(config: &ServiceConfig) -> Result<Self, DataPlaneGraphError> {
        if config.service != ServiceKind::DataPlane {
            return Err(DataPlaneGraphError::WrongService);
        }
        let secret = config
            .internal_auth_secret
            .as_ref()
            .ok_or(DataPlaneGraphError::MissingKeyMaterial)?;
        if secret.expose_secret().len() < 32 || secret.expose_secret().chars().any(char::is_control)
        {
            return Err(DataPlaneGraphError::InvalidKeyMaterial);
        }
        let issuer = config.public_url.as_ref().map_or_else(
            || format!("http://{}", config.bind_address),
            |url| url.as_str().trim_end_matches('/').to_owned(),
        );
        let signing_encryption_key = KeyEncryptionKey::from_bytes(blake3::derive_key(
            "mako/data-plane/application-signing-key-encryption/v1",
            secret.expose_secret().as_bytes(),
        ));
        let session_protection_key = KeyEncryptionKey::from_bytes(blake3::derive_key(
            "mako/data-plane/refresh-credential-protection/v1",
            secret.expose_secret().as_bytes(),
        ));
        let internal_deployment_key = DeploymentKey::derive(secret.expose_secret())?;
        let replication_token_key = ReplicationTokenKey::from_bytes(blake3::derive_key(
            "mako/data-plane/replication-token-signing/v1",
            secret.expose_secret().as_bytes(),
        ));
        let explorer_capability_keys = ExplorerCapabilityKeyRing::new(
            ExplorerCapabilityKey::new(
                "xcap-v1",
                &blake3::derive_key(
                    "mako/explorer-capability-signing/v1",
                    secret.expose_secret().as_bytes(),
                ),
            )
            .map_err(|_| DataPlaneGraphError::InvalidKeyMaterial)?,
            Vec::new(),
        )
        .map_err(|_| DataPlaneGraphError::InvalidKeyMaterial)?;
        let object_key_root = mako_file_storage::ObjectKeyRoot::from_bytes(blake3::derive_key(
            "mako/data-plane/object-encryption/v1",
            secret.expose_secret().as_bytes(),
        ));
        let object_access = config
            .object_store_access_key
            .as_ref()
            .ok_or(DataPlaneGraphError::MissingKeyMaterial)?;
        let object_secret = config
            .object_store_secret_key
            .as_ref()
            .ok_or(DataPlaneGraphError::MissingKeyMaterial)?;
        let object_store = Arc::new(
            mako_object_store::S3ObjectStore::new(
                mako_object_store::S3ObjectStoreConfig::loopback(
                    config.object_store_endpoint.clone(),
                    config.region.clone(),
                )
                .with_bucket(APPLICATION_OBJECT_BUCKET),
                mako_object_store::S3Credentials::new(
                    object_access.expose_secret(),
                    object_secret.expose_secret(),
                )
                .map_err(|_| DataPlaneGraphError::InvalidKeyMaterial)?,
            )
            .map_err(|_| DataPlaneGraphError::InvalidKeyMaterial)?,
        );
        let enforce_object_store =
            config.environment == mako_config::DeploymentEnvironment::Production;
        let public_url = issuer.clone();
        let provider_secret_key =
            mako_auth_providers::ProviderSecretKey::derive(secret.expose_secret().as_bytes());
        let flow_state = mako_auth_providers::FlowStateVerifier::new(
            mako_auth_providers::FlowStateKey::derive(secret.expose_secret().as_bytes()),
        );
        let provider_client =
            mako_auth_providers::ProviderClient::new(mako_auth_providers::ProviderClientConfig {
                allow_plain_http_loopback: !enforce_object_store,
                ..mako_auth_providers::ProviderClientConfig::default()
            });
        let explorer_cursor_key = blake3::derive_key(
            "mako/data-plane/explorer-cursor-signing/v1",
            secret.expose_secret().as_bytes(),
        );
        let audit_cursor_key = CursorSigningKey::new(blake3::derive_key(
            "mako/data-plane/audit-cursor-signing/v1",
            secret.expose_secret().as_bytes(),
        ));
        let storage = StorageOwner::open(config)?;
        let initial_readiness = storage.initial_readiness();
        if !initial_readiness.is_ready() {
            return Err(DataPlaneGraphError::StorageNotReady);
        }
        let adapter: Arc<dyn KvAdapter> = Arc::new(storage.adapter());
        let documents = DocumentEngine::new(Arc::clone(&adapter));
        let quotas = Arc::new(GatewayQuotaEngine::new(
            Arc::clone(&adapter),
            GatewayQuotaEngineConfig {
                durability: Durability::Sync,
                maximum_conflict_retries: nonzero(128),
                maximum_charges_per_request: nonzero(8),
            },
        ));
        let quota_policy = GatewayQuotaPolicy::new([
            (
                GatewayQuotaResource::AuthenticationRequests,
                rate_limit(600, ONE_MINUTE_MILLISECONDS),
            ),
            (
                GatewayQuotaResource::DocumentRequests,
                rate_limit(10_000, ONE_MINUTE_MILLISECONDS),
            ),
            (
                GatewayQuotaResource::DocumentBytes,
                rate_limit(50 * 1024 * 1024, ONE_MINUTE_MILLISECONDS),
            ),
            (
                GatewayQuotaResource::ReplicationRequests,
                hard_and_rate_limit(100_000, ONE_DAY_MILLISECONDS, 120, ONE_MINUTE_MILLISECONDS),
            ),
            (
                GatewayQuotaResource::ReplicationBytes,
                hard_and_rate_limit(
                    10 * 1024 * 1024 * 1024,
                    ONE_DAY_MILLISECONDS,
                    64 * 1024 * 1024,
                    ONE_MINUTE_MILLISECONDS,
                ),
            ),
        ])?;
        let audit = AuditStore::new(
            Arc::clone(&adapter),
            AuditStoreConfig {
                durability: Durability::Sync,
                retention_milliseconds: AUDIT_RETENTION_MILLISECONDS,
                maximum_page_records: nonzero(100),
                maximum_export_records: nonzero(10_000),
                maximum_examined_records: nonzero(50_000),
                cursor_signing_key: audit_cursor_key,
            },
        )?;
        let redactor = TelemetryRedactor::new([secret.expose_secret()])?;
        let password_service =
            PasswordService::new(PasswordPolicy::default(), Argon2idParameters::default());
        let sign_in_throttle = PersistentSignInThrottle::new(
            Arc::clone(&adapter),
            FixedWindowRateLimitConfig::new(10, 15 * 60)?,
            Durability::Sync,
        )?;
        let access_token_config = AccessTokenConfig::new(
            issuer.clone(),
            ACCESS_TOKEN_AUDIENCE,
            ACCESS_TOKEN_LIFETIME_SECONDS,
        )?;
        let verification_config = AccessTokenVerificationConfig::new(
            issuer,
            ACCESS_TOKEN_AUDIENCE,
            ACCESS_TOKEN_CLOCK_SKEW_SECONDS,
        )?;
        let gateway_state = PersistentGatewayState {
            adapter: Arc::clone(&adapter),
        };

        let custom_domains = CustomDomainRegistry::load(Arc::clone(&adapter))?;
        let allowed_origins = AllowedOriginRegistry::load(Arc::clone(&adapter))?;
        // Built before the adapter is moved into the graph.
        let quota_policies = Arc::new(PersistentQuotaPolicySource::new(
            Arc::clone(&adapter),
            quota_policy.clone(),
            // Short, because an operator raising a tenant's limit during an
            // incident should take effect promptly.
            Duration::from_secs(10),
        ));
        Ok(Self {
            storage,
            adapter,
            components: DataPlaneComponents {
                object_store,
                object_key_root,
                enforce_object_store,
                public_url,
                provider_secret_key,
                flow_state,
                provider_client,
                // The telemetry store has an ingest endpoint that nothing has
                // ever called, which is why every signal the management API
                // serves from it answers empty. This is the emitting side.
                telemetry: Arc::new(mako_telemetry_client::TelemetryEmitter::new(
                    config.telemetry_query_address,
                    secret.expose_secret(),
                    "mako.data-plane",
                )),
                storage_sampler: Arc::new(crate::telemetry::StorageSampler::new(
                    config.region.clone(),
                )),
                quota_checkpoints: Arc::new(crate::telemetry::QuotaCheckpointer::new()),
                documents,
                quotas,
                quota_policies,
                quota_policy,
                custom_domains,
                allowed_origins,
                audit,
                redactor,
                password_service,
                sign_in_throttle,
                replication_gateway: ReplicationGateway,
                gateway_state,
                signing_encryption_key,
                session_protection_key,
                internal_deployment_key,
                access_token_config,
                verification_config,
                replication_token_key,
                explorer_capability_keys,
                explorer_cursor_key,
                explorer_metrics: ExplorerMetrics::default(),
            },
        })
    }

    #[must_use]
    pub fn storage_mode(&self) -> StorageMode {
        self.storage.mode()
    }

    #[must_use]
    pub fn document_engine(&self) -> &DocumentEngine {
        &self.components.documents
    }

    #[must_use]
    pub fn telemetry(&self) -> &Arc<mako_telemetry_client::TelemetryEmitter> {
        &self.components.telemetry
    }

    #[must_use]
    pub fn storage_adapter(&self) -> &Arc<dyn KvAdapter> {
        &self.adapter
    }

    #[must_use]
    pub fn storage_sampler(&self) -> &Arc<crate::telemetry::StorageSampler> {
        &self.components.storage_sampler
    }

    #[must_use]
    pub fn quota_checkpoints(&self) -> &Arc<crate::telemetry::QuotaCheckpointer> {
        &self.components.quota_checkpoints
    }

    /// Summarize the quota counters of every settled minute a tenant was
    /// admitted in, into the telemetry buffer. Driven from the telemetry
    /// worker, never from a request thread.
    pub async fn checkpoint_quota_counters(&self, now_unix_milliseconds: u64) -> usize {
        self.components
            .quota_checkpoints
            .checkpoint_due(
                &self.components.quotas,
                self.components.quota_policies.as_ref(),
                &self.components.telemetry,
                now_unix_milliseconds,
            )
            .await
    }

    #[must_use]
    pub fn quota_engine(&self) -> &Arc<GatewayQuotaEngine> {
        &self.components.quotas
    }

    #[must_use]
    pub fn quota_policies(&self) -> &Arc<PersistentQuotaPolicySource> {
        &self.components.quota_policies
    }

    #[must_use]
    pub fn quota_policy(&self) -> &GatewayQuotaPolicy {
        &self.components.quota_policy
    }

    /// The verified custom hostnames each environment is served on.
    #[must_use]
    pub fn custom_domains(&self) -> &CustomDomainRegistry {
        &self.components.custom_domains
    }

    #[must_use]
    pub fn allowed_origins(&self) -> &AllowedOriginRegistry {
        &self.components.allowed_origins
    }

    #[must_use]
    pub fn audit_store(&self) -> &AuditStore {
        &self.components.audit
    }

    #[must_use]
    pub fn telemetry_redactor(&self) -> &TelemetryRedactor {
        &self.components.redactor
    }

    #[must_use]
    pub fn password_service(&self) -> &PasswordService {
        &self.components.password_service
    }

    #[must_use]
    pub fn sign_in_throttle(&self) -> &PersistentSignInThrottle {
        &self.components.sign_in_throttle
    }

    #[must_use]
    pub const fn replication_gateway(&self) -> &ReplicationGateway {
        &self.components.replication_gateway
    }

    #[must_use]
    pub const fn replication_token_key(&self) -> &ReplicationTokenKey {
        &self.components.replication_token_key
    }

    #[must_use]
    pub fn explorer_capability_keys(&self) -> &ExplorerCapabilityKeyRing {
        &self.components.explorer_capability_keys
    }

    #[must_use]
    pub const fn explorer_cursor_key(&self) -> &[u8; 32] {
        &self.components.explorer_cursor_key
    }

    pub(crate) fn explorer_metrics(&self) -> &ExplorerMetrics {
        &self.components.explorer_metrics
    }

    #[must_use]
    pub fn explorer_metrics_snapshot(&self) -> ExplorerMetricsSnapshot {
        self.components.explorer_metrics.snapshot()
    }

    pub(crate) async fn authorize_replication(
        &self,
        trusted_tenant: &TenantScope,
        request: ReplicationGatewayRequest,
    ) -> Result<AuthorizedReplicationRequest, ReplicationAuthorizationError> {
        let credentials = self
            .identity_store(trusted_tenant, trusted_tenant)
            .map_err(|_| ReplicationAuthorizationError::AuthorityUnavailable)?;
        let jwks = self
            .signing_key_store(trusted_tenant, trusted_tenant)
            .map_err(|_| ReplicationAuthorizationError::AuthorityUnavailable)?
            .jwks()
            .await
            .map_err(|_| ReplicationAuthorizationError::AuthorityUnavailable)?;
        let verifier = GatewayAccessTokenVerifier::new(
            &jwks,
            &self.components.gateway_state,
            &self.components.gateway_state,
            self.components.verification_config.clone(),
        );
        let quotas = GatewayReplicationQuotaEnforcer::new(
            Arc::clone(&self.components.quotas),
            Arc::new(self.components.quota_policy.clone()),
        );
        self.components
            .replication_gateway
            .authorize(request, &credentials, &verifier, &quotas)
            .await
            .map_err(ReplicationAuthorizationError::Gateway)
    }

    pub fn identity_store(
        &self,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
    ) -> Result<IdentityStore, DataPlaneIdentityError> {
        IdentityStore::new(
            Arc::clone(&self.adapter),
            requested_tenant,
            trusted_tenant,
            Durability::Sync,
        )
        .map_err(Into::into)
    }

    pub fn signing_key_store(
        &self,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
    ) -> Result<ProjectSigningKeyStore, DataPlaneIdentityError> {
        ProjectSigningKeyStore::new(
            Arc::clone(&self.adapter),
            requested_tenant,
            trusted_tenant,
            Durability::Sync,
        )
        .map_err(Into::into)
    }

    pub fn authorization_epoch_store(
        &self,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
    ) -> Result<AuthorizationEpochStore, DataPlaneIdentityError> {
        AuthorizationEpochStore::new(
            Arc::clone(&self.adapter),
            requested_tenant,
            trusted_tenant,
            Durability::Sync,
        )
        .map_err(Into::into)
    }

    pub fn explorer_authorization_store(
        &self,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
    ) -> Result<ExplorerAuthorizationStore, DataPlaneIdentityError> {
        ExplorerAuthorizationStore::new(
            Arc::clone(&self.adapter),
            requested_tenant,
            trusted_tenant,
            Durability::Sync,
        )
        .map_err(Into::into)
    }

    pub fn public_url(&self) -> &str {
        &self.components.public_url
    }

    pub fn provider_secret_key(&self) -> &mako_auth_providers::ProviderSecretKey {
        &self.components.provider_secret_key
    }

    pub fn flow_state(&self) -> &mako_auth_providers::FlowStateVerifier {
        &self.components.flow_state
    }

    pub fn provider_client(&self) -> &mako_auth_providers::ProviderClient {
        &self.components.provider_client
    }

    /// The node-wide outbox of mail the control plane sends for applications.
    pub fn application_mail(&self) -> crate::application_mail::ApplicationMailOutbox {
        crate::application_mail::ApplicationMailOutbox::new(
            Arc::clone(&self.adapter),
            Durability::Sync,
        )
    }

    /// Buckets and objects of one tenant, over this node's object store.
    pub fn file_storage(
        &self,
        trusted_tenant: &TenantScope,
    ) -> Result<mako_file_storage::FileStorageService, mako_file_storage::FileStorageError> {
        mako_file_storage::FileStorageService::new(
            Arc::clone(&self.adapter),
            Arc::clone(&self.components.object_store) as Arc<dyn mako_object_store::ObjectStore>,
            &self.components.object_key_root,
            trusted_tenant.clone(),
            Durability::Sync,
        )
    }

    pub fn policy_store(
        &self,
        trusted_tenant: &TenantScope,
        scope: CollectionScope,
    ) -> Result<PolicyStore, DataPlaneIdentityError> {
        PolicyStore::new(
            Arc::clone(&self.adapter),
            trusted_tenant,
            scope,
            Durability::Sync,
        )
        .map_err(Into::into)
    }

    #[must_use]
    pub fn internal_authenticator(&self, caller: InternalCaller) -> InternalRequestAuthenticator {
        InternalRequestAuthenticator::new(self.components.internal_deployment_key.clone(), caller)
    }

    pub fn internal_replay_guard(
        &self,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
    ) -> Result<RocksInternalReplayGuard, DataPlaneIdentityError> {
        RocksInternalReplayGuard::new(Arc::clone(&self.adapter), requested_tenant, trusted_tenant)
            .map_err(Into::into)
    }

    pub fn internal_response_journal(
        &self,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
    ) -> Result<EncryptedResponseJournal, DataPlaneIdentityError> {
        EncryptedResponseJournal::new(
            Arc::clone(&self.adapter),
            requested_tenant,
            trusted_tenant,
            &self.components.internal_deployment_key,
        )
        .map_err(Into::into)
    }

    pub async fn initialize_signing_key(
        &self,
        trusted_tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<ProjectSigningKeyRecord, DataPlaneIdentityError> {
        self.signing_key_store(trusted_tenant, trusted_tenant)?
            .initialize(&self.components.signing_encryption_key, now_unix_seconds)
            .await
            .map_err(Into::into)
    }

    pub async fn rotate_signing_key(
        &self,
        trusted_tenant: &TenantScope,
        now_unix_seconds: u64,
        overlap_seconds: u64,
    ) -> Result<ProjectSigningKeyRecord, DataPlaneIdentityError> {
        self.signing_key_store(trusted_tenant, trusted_tenant)?
            .rotate(
                &self.components.signing_encryption_key,
                now_unix_seconds,
                overlap_seconds,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn signing_key_ring(
        &self,
        trusted_tenant: &TenantScope,
    ) -> Result<ProjectSigningKeyRing, DataPlaneIdentityError> {
        self.signing_key_store(trusted_tenant, trusted_tenant)?
            .key_ring()
            .await
            .map_err(Into::into)
    }

    pub async fn issue_access_token(
        &self,
        input: AccessTokenInput,
        now_unix_seconds: u64,
    ) -> Result<AccessToken, DataPlaneIdentityError> {
        let ring = self.signing_key_ring(&input.tenant).await?;
        AccessTokenIssuer::new(
            &ring,
            &self.components.signing_encryption_key,
            self.components.access_token_config.clone(),
        )
        .issue(input, now_unix_seconds)
        .map_err(Into::into)
    }

    pub async fn verify_access_token(
        &self,
        token: &str,
        trusted_tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<VerifiedAccessIdentity, DataPlaneIdentityError> {
        let jwks = self
            .signing_key_store(trusted_tenant, trusted_tenant)?
            .jwks()
            .await?;
        GatewayAccessTokenVerifier::new(
            &jwks,
            &self.components.gateway_state,
            &self.components.gateway_state,
            self.components.verification_config.clone(),
        )
        .verify(token, trusted_tenant, now_unix_seconds)
        .map_err(Into::into)
    }

    pub async fn create_application_session(
        &self,
        trusted_tenant: &TenantScope,
        user_id: &AppUserId,
        now_unix_seconds: u64,
    ) -> Result<DataPlaneSessionGrant, DataPlaneIdentityError> {
        let identity = self.identity_store(trusted_tenant, trusted_tenant)?;
        let grant =
            ApplicationSessionStore::new(&identity, &self.components.session_protection_key)
                .create(user_id, now_unix_seconds)
                .await?;
        self.complete_session_grant(grant, now_unix_seconds).await
    }

    pub async fn refresh_application_session(
        &self,
        trusted_tenant: &TenantScope,
        presented_refresh: &str,
        now_unix_seconds: u64,
    ) -> Result<DataPlaneRefreshOutcome, DataPlaneIdentityError> {
        let identity = self.identity_store(trusted_tenant, trusted_tenant)?;
        match ApplicationSessionStore::new(&identity, &self.components.session_protection_key)
            .refresh(presented_refresh, now_unix_seconds)
            .await?
        {
            RefreshSessionOutcome::Rotated(grant) => self
                .complete_session_grant(*grant, now_unix_seconds)
                .await
                .map(Box::new)
                .map(DataPlaneRefreshOutcome::Rotated),
            RefreshSessionOutcome::Invalid => Ok(DataPlaneRefreshOutcome::Invalid),
            RefreshSessionOutcome::ReplayDetected => Ok(DataPlaneRefreshOutcome::ReplayDetected),
        }
    }

    async fn complete_session_grant(
        &self,
        grant: ApplicationSessionGrant,
        now_unix_seconds: u64,
    ) -> Result<DataPlaneSessionGrant, DataPlaneIdentityError> {
        let ring = self.signing_key_ring(grant.user.scope()).await?;
        if ring.records().is_empty() {
            let _ = self
                .initialize_signing_key(grant.user.scope(), now_unix_seconds)
                .await;
            if self
                .signing_key_ring(grant.user.scope())
                .await?
                .records()
                .is_empty()
            {
                return Err(DataPlaneIdentityError::Signing(
                    ProjectSigningKeyStoreError::ConcurrentChange,
                ));
            }
        }
        let subject = SubjectId::parse(grant.user.id().as_str())?;
        let epochs = self
            .authorization_epoch_store(grant.user.scope(), grant.user.scope())?
            .epochs_for(&subject)
            .await?;
        let role = grant
            .user
            .trusted_metadata()
            .values()
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("authenticated")
            .to_owned();
        let access_token = self
            .issue_access_token(
                AccessTokenInput {
                    tenant: grant.user.scope().clone(),
                    user_id: grant.user.id().clone(),
                    role,
                    session_id: grant.session.id().clone(),
                    authorization_epochs: AccessAuthorizationEpochs {
                        environment: epochs.environment().get(),
                        user: epochs.user().get(),
                    },
                    trusted_claims: grant.user.trusted_metadata().values().clone(),
                },
                now_unix_seconds,
            )
            .await?;
        Ok(DataPlaneSessionGrant {
            access_token,
            refresh_credential: grant.refresh_credential,
            user: grant.user,
            email: grant.email,
            session_id: grant.session.id().clone(),
            expires_in_seconds: ACCESS_TOKEN_LIFETIME_SECONDS,
            authorization_epoch: epochs.user().get(),
        })
    }

    pub fn readiness(&self) -> DataPlaneReadiness {
        DataPlaneReadiness {
            storage: block_on(check_storage_readiness(
                self.adapter.as_ref(),
                Durability::Sync,
            )),
            identity: true,
            documents: true,
            policy: true,
            gateway: true,
            quota: true,
            audit: true,
            object_store: !self.components.enforce_object_store
                || self.components.object_store.dependency_ready()
                || self.components.object_store.ensure_bucket().is_ok(),
        }
    }

    pub async fn shutdown(self) -> Result<(), DataPlaneGraphError> {
        let Self {
            storage,
            adapter,
            components,
        } = self;
        drop(components);
        drop(adapter);
        storage.shutdown().await.map_err(Into::into)
    }
}

impl ReadinessProbe for DataPlaneGraph {
    fn snapshot(&self) -> ReadinessSnapshot {
        self.readiness().snapshot()
    }
}

pub struct DataPlaneSessionGrant {
    pub access_token: AccessToken,
    pub refresh_credential: RefreshCredential,
    pub user: AppUserRecord,
    pub email: Option<NormalizedEmail>,
    pub session_id: SessionId,
    pub expires_in_seconds: u64,
    pub authorization_epoch: u64,
}

pub enum DataPlaneRefreshOutcome {
    Rotated(Box<DataPlaneSessionGrant>),
    Invalid,
    ReplayDetected,
}

/// Route readiness is the conjunction of storage and every constructed domain
/// dependency. It exposes only stable component names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataPlaneReadiness {
    pub storage: StorageReadiness,
    pub identity: bool,
    pub documents: bool,
    pub policy: bool,
    pub gateway: bool,
    pub quota: bool,
    pub audit: bool,
    /// The application object store answers, or this deployment does not require it.
    pub object_store: bool,
}

impl DataPlaneReadiness {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.storage.is_ready()
            && self.identity
            && self.documents
            && self.policy
            && self.gateway
            && self.quota
            && self.audit
            && self.object_store
    }

    fn snapshot(&self) -> ReadinessSnapshot {
        let dependencies = [
            ("storage", self.storage.is_ready()),
            ("identity", self.identity),
            ("documents", self.documents),
            ("policy", self.policy),
            ("gateway", self.gateway),
            ("quota", self.quota),
            ("audit", self.audit),
            ("object_store", self.object_store),
        ];
        let failed = dependencies
            .into_iter()
            .filter_map(|(name, ready)| (!ready).then_some(name))
            .collect::<Vec<_>>();
        if failed.is_empty() {
            ReadinessSnapshot::ready("storage_identity_documents_policy_gateway_quota_audit_ready")
        } else {
            ReadinessSnapshot::not_ready(format!("dependencies_not_ready:{}", failed.join(",")))
        }
    }
}

impl fmt::Display for DataPlaneReadiness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.snapshot().detail)
    }
}

#[derive(Clone)]
struct PersistentGatewayState {
    adapter: Arc<dyn KvAdapter>,
}

impl GatewaySessionResolver for PersistentGatewayState {
    fn is_active(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> bool {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let store = IdentityStore::new(
                        Arc::clone(&self.adapter),
                        tenant,
                        tenant,
                        Durability::Sync,
                    )
                    .ok()?;
                    block_on(store.session_is_active(user_id, session_id, now_unix_seconds)).ok()
                })
                .join()
        })
        .ok()
        .flatten()
        .unwrap_or(false)
    }
}

impl GatewayAuthorizationEpochResolver for PersistentGatewayState {
    fn current_epochs(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
    ) -> Option<AccessAuthorizationEpochs> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let subject = SubjectId::parse(user_id.as_str()).ok()?;
                    let store = AuthorizationEpochStore::new(
                        Arc::clone(&self.adapter),
                        tenant,
                        tenant,
                        Durability::Sync,
                    )
                    .ok()?;
                    let epochs = block_on(store.epochs_for(&subject)).ok()?;
                    Some(AccessAuthorizationEpochs {
                        environment: epochs.environment().get(),
                        user: epochs.user().get(),
                    })
                })
                .join()
        })
        .ok()
        .flatten()
    }
}

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("data-plane limits are positive constants")
}

fn rate_limit(limit: u64, window_milliseconds: u64) -> GatewayQuotaLimit {
    GatewayQuotaLimit {
        hard: None,
        rate: Some(GatewayQuotaWindow {
            limit: NonZeroU64::new(limit).expect("data-plane quota limits are positive"),
            window_milliseconds: NonZeroU64::new(window_milliseconds)
                .expect("data-plane quota windows are positive"),
        }),
    }
}

fn hard_and_rate_limit(
    hard_limit: u64,
    hard_window_milliseconds: u64,
    rate_limit: u64,
    rate_window_milliseconds: u64,
) -> GatewayQuotaLimit {
    GatewayQuotaLimit {
        hard: Some(GatewayQuotaWindow {
            limit: NonZeroU64::new(hard_limit).expect("data-plane quota limits are positive"),
            window_milliseconds: NonZeroU64::new(hard_window_milliseconds)
                .expect("data-plane quota windows are positive"),
        }),
        rate: Some(GatewayQuotaWindow {
            limit: NonZeroU64::new(rate_limit).expect("data-plane quota limits are positive"),
            window_milliseconds: NonZeroU64::new(rate_window_milliseconds)
                .expect("data-plane quota windows are positive"),
        }),
    }
}

pub(crate) enum ReplicationAuthorizationError {
    AuthorityUnavailable,
    Gateway(ReplicationGatewayError),
}

#[derive(Debug)]
pub enum DataPlaneGraphError {
    WrongService,
    MissingKeyMaterial,
    InvalidKeyMaterial,
    StorageNotReady,
    Storage(StorageError),
    Audit(AuditStoreError),
    Redaction(RedactionError),
    AccessToken(AccessTokenError),
    AccessVerification(AccessTokenVerificationError),
    InternalAuth(InternalAuthError),
    Quota(mako_gateway::GatewayQuotaError),
    RateLimit(RateLimitError),
}

impl fmt::Display for DataPlaneGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongService => "data-plane graph received another service configuration",
            Self::MissingKeyMaterial => "data-plane protected key material is unavailable",
            Self::InvalidKeyMaterial => "data-plane protected key material is invalid",
            Self::StorageNotReady => "data-plane storage dependencies are not ready",
            Self::Storage(_) => "data-plane storage could not be opened",
            Self::Audit(_) => "data-plane audit storage could not be composed",
            Self::Redaction(_) => "data-plane telemetry redaction could not be composed",
            Self::AccessToken(_) => "data-plane token issuance could not be configured",
            Self::AccessVerification(_) => "data-plane token verification could not be configured",
            Self::InternalAuth(_) => "data-plane internal authentication could not be configured",
            Self::Quota(_) => "data-plane quota policy could not be configured",
            Self::RateLimit(_) => "data-plane sign-in throttling could not be configured",
        })
    }
}

impl Error for DataPlaneGraphError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Audit(error) => Some(error),
            Self::Redaction(error) => Some(error),
            Self::AccessToken(error) => Some(error),
            Self::AccessVerification(error) => Some(error),
            Self::InternalAuth(error) => Some(error),
            Self::Quota(error) => Some(error),
            Self::RateLimit(error) => Some(error),
            Self::WrongService
            | Self::MissingKeyMaterial
            | Self::InvalidKeyMaterial
            | Self::StorageNotReady => None,
        }
    }
}

impl From<StorageError> for DataPlaneGraphError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<AuditStoreError> for DataPlaneGraphError {
    fn from(error: AuditStoreError) -> Self {
        Self::Audit(error)
    }
}

impl From<RedactionError> for DataPlaneGraphError {
    fn from(error: RedactionError) -> Self {
        Self::Redaction(error)
    }
}

impl From<AccessTokenError> for DataPlaneGraphError {
    fn from(error: AccessTokenError) -> Self {
        Self::AccessToken(error)
    }
}

impl From<AccessTokenVerificationError> for DataPlaneGraphError {
    fn from(error: AccessTokenVerificationError) -> Self {
        Self::AccessVerification(error)
    }
}

impl From<InternalAuthError> for DataPlaneGraphError {
    fn from(error: InternalAuthError) -> Self {
        Self::InternalAuth(error)
    }
}

impl From<mako_gateway::GatewayQuotaError> for DataPlaneGraphError {
    fn from(error: mako_gateway::GatewayQuotaError) -> Self {
        Self::Quota(error)
    }
}

impl From<RateLimitError> for DataPlaneGraphError {
    fn from(error: RateLimitError) -> Self {
        Self::RateLimit(error)
    }
}

#[derive(Debug)]
pub enum DataPlaneIdentityError {
    Identity(IdentityStoreError),
    Signing(ProjectSigningKeyStoreError),
    Authorization(AuthorizationEpochError),
    Policy(PolicyStoreError),
    TokenIssue(AccessTokenError),
    TokenVerification(AccessTokenVerificationError),
    InternalRpc(RocksInternalReplayGuardError),
    ResponseJournal(ResponseJournalError),
    Session(ApplicationSessionStoreError),
    PolicyContext(PolicyContextError),
    Explorer(ExplorerAuthorizationError),
}

impl fmt::Display for DataPlaneIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Identity(_) => "data-plane identity storage operation failed",
            Self::Signing(_) => "data-plane signing-key storage operation failed",
            Self::Authorization(_) => "data-plane authorization-epoch operation failed",
            Self::Policy(_) => "data-plane policy storage operation failed",
            Self::TokenIssue(_) => "data-plane access-token issuance failed",
            Self::TokenVerification(_) => "data-plane access-token verification failed",
            Self::InternalRpc(_) => "data-plane internal replay guard failed",
            Self::ResponseJournal(_) => "data-plane internal response journal failed",
            Self::Session(_) => "data-plane application session operation failed",
            Self::PolicyContext(_) => "data-plane policy identity context is invalid",
            Self::Explorer(_) => "data-plane explorer authorization operation failed",
        })
    }
}

impl Error for DataPlaneIdentityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(match self {
            Self::Identity(error) => error,
            Self::Signing(error) => error,
            Self::Authorization(error) => error,
            Self::Policy(error) => error,
            Self::TokenIssue(error) => error,
            Self::TokenVerification(error) => error,
            Self::InternalRpc(error) => error,
            Self::ResponseJournal(error) => error,
            Self::Session(error) => error,
            Self::PolicyContext(error) => error,
            Self::Explorer(error) => error,
        })
    }
}

impl From<IdentityStoreError> for DataPlaneIdentityError {
    fn from(error: IdentityStoreError) -> Self {
        Self::Identity(error)
    }
}

impl From<ProjectSigningKeyStoreError> for DataPlaneIdentityError {
    fn from(error: ProjectSigningKeyStoreError) -> Self {
        Self::Signing(error)
    }
}

impl From<AuthorizationEpochError> for DataPlaneIdentityError {
    fn from(error: AuthorizationEpochError) -> Self {
        Self::Authorization(error)
    }
}

impl From<PolicyStoreError> for DataPlaneIdentityError {
    fn from(error: PolicyStoreError) -> Self {
        Self::Policy(error)
    }
}

impl From<ExplorerAuthorizationError> for DataPlaneIdentityError {
    fn from(error: ExplorerAuthorizationError) -> Self {
        Self::Explorer(error)
    }
}

impl From<AccessTokenError> for DataPlaneIdentityError {
    fn from(error: AccessTokenError) -> Self {
        Self::TokenIssue(error)
    }
}

impl From<AccessTokenVerificationError> for DataPlaneIdentityError {
    fn from(error: AccessTokenVerificationError) -> Self {
        Self::TokenVerification(error)
    }
}

impl From<RocksInternalReplayGuardError> for DataPlaneIdentityError {
    fn from(error: RocksInternalReplayGuardError) -> Self {
        Self::InternalRpc(error)
    }
}

impl From<ResponseJournalError> for DataPlaneIdentityError {
    fn from(error: ResponseJournalError) -> Self {
        Self::ResponseJournal(error)
    }
}

impl From<ApplicationSessionStoreError> for DataPlaneIdentityError {
    fn from(error: ApplicationSessionStoreError) -> Self {
        Self::Session(error)
    }
}

impl From<PolicyContextError> for DataPlaneIdentityError {
    fn from(error: PolicyContextError) -> Self {
        Self::PolicyContext(error)
    }
}

/// Fixtures shared by the route modules' tests: a data-plane configuration
/// over a temporary directory under the crate's ignored `.local`, and the
/// tenant those tests address.
#[cfg(test)]
pub(crate) mod test_support {
    use std::{fs, path::Path};

    use mako_api::{EnvironmentId, ProjectId, TenantScope};
    use mako_config::{ConfigLoader, ServiceConfig, ServiceKind};
    use tempfile::{Builder, TempDir};

    pub(crate) use mako_config::DeploymentEnvironment;

    pub(crate) fn config_for(root: &Path, environment: DeploymentEnvironment) -> ServiceConfig {
        let rocksdb = root.join("rocksdb");
        let backup = root.join("backup");
        fs::create_dir_all(&rocksdb).expect("database directory");
        fs::create_dir_all(&backup).expect("backup directory");
        let environment_name = match environment {
            DeploymentEnvironment::Local => "local",
            DeploymentEnvironment::Production => "production",
            _ => unreachable!("test uses local or production"),
        };
        ConfigLoader::from_environment([
            ("MAKO_ENVIRONMENT", environment_name),
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
                "env:TEST_DATA_PLANE_ROOT_KEY",
            ),
            (
                "TEST_DATA_PLANE_ROOT_KEY",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
            // Application objects need the object store's credentials, as the
            // control plane does for bundles; the store itself is only reached
            // when an object is written or, in production, at readiness.
            (
                "MAKO_OBJECT_STORE_ACCESS_KEY_REF",
                "env:TEST_OBJECT_STORE_ACCESS_KEY",
            ),
            (
                "TEST_OBJECT_STORE_ACCESS_KEY",
                "test-object-store-access-key",
            ),
            (
                "MAKO_OBJECT_STORE_SECRET_KEY_REF",
                "env:TEST_OBJECT_STORE_SECRET_KEY",
            ),
            (
                "TEST_OBJECT_STORE_SECRET_KEY",
                "test-object-store-secret-key",
            ),
        ])
        .load(ServiceKind::DataPlane)
        .expect("configuration")
    }

    pub(crate) fn local_tempdir(prefix: &str) -> TempDir {
        let root = std::env::current_dir()
            .expect("working directory")
            .join(".local");
        fs::create_dir_all(&root).expect("local test root");
        Builder::new()
            .prefix(prefix)
            .tempdir_in(root)
            .expect("temporary directory")
    }

    pub(crate) fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_example00").expect("project"),
            EnvironmentId::parse("env_example00").expect("environment"),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use mako_api::{EnvironmentId, ProjectId};
    use mako_identity::{
        EmailSignupConfig, SignInRequestMetadata, SignInResponse, SignInService, SignupService,
        TransactionalEmailProvider, VerificationEmail,
    };
    use mako_internal_rpc::{GuardDecision, InternalReplayGuard, InternalRoute};
    use mako_storage::{ProductionVolumeIdentity, provision_production_volume};

    use super::{
        test_support::{config_for, local_tempdir, tenant},
        *,
    };

    /// A request that arrived on a custom hostname is served only for the
    /// environment that hostname is installed for, and the installed list
    /// survives a restart.
    #[test]
    fn custom_domains_gate_application_requests_and_survive_reopening() {
        use mako_service_runtime::{HttpMethod, HttpRequest};

        let directory = local_tempdir("data-plane-custom-domains");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let own = tenant();
        let other = TenantScope::new(
            ProjectId::parse("prj_example00").expect("project"),
            EnvironmentId::parse("env_example01").expect("environment"),
        );
        let request = |header: Option<&str>| {
            HttpRequest::for_test(
                HttpMethod::Get,
                "/v1/projects/prj_example00/environments/env_example00/auth/jwks",
                header.map(|value| ("X-Mako-Custom-Domain".to_owned(), value.to_owned())),
                Vec::new(),
                None,
            )
        };
        {
            let graph = DataPlaneGraph::open(&config).expect("graph");
            assert!(
                crate::auth_http::require_custom_domain(&graph, &request(None), &own).is_ok(),
                "the platform hostname carries no header and is always served"
            );
            let refused = crate::auth_http::require_custom_domain(
                &graph,
                &request(Some("api.example.com")),
                &own,
            )
            .expect_err("nothing installed");
            assert_eq!(refused.envelope().error.code, mako_api::ErrorCode::NotFound);
            block_on(
                graph
                    .custom_domains()
                    .install(&own, vec!["api.example.com".to_owned()]),
            )
            .expect("installed");
            assert!(graph.custom_domains().permits(&own, "api.example.com"));
            assert!(!graph.custom_domains().permits(&other, "api.example.com"));
            // The cross-origin allowlist is the environment's, installed
            // and persisted the same way, and answers for no other tenant.
            block_on(
                graph
                    .allowed_origins()
                    .install(&own, vec!["https://app.example.com".to_owned()]),
            )
            .expect("origins installed");
            assert!(
                graph
                    .allowed_origins()
                    .allows(&own, "https://app.example.com")
            );
            assert!(
                !graph
                    .allowed_origins()
                    .allows(&own, "https://other.example.com"),
                "only a listed origin is allowed"
            );
            assert!(
                !graph
                    .allowed_origins()
                    .allows(&other, "https://app.example.com"),
                "an environment with no list allows nothing"
            );
            assert!(
                crate::auth_http::require_custom_domain(
                    &graph,
                    &request(Some("API.example.com:443")),
                    &own
                )
                .is_ok(),
                "the header is normalized before the check"
            );
            assert!(
                crate::auth_http::require_custom_domain(
                    &graph,
                    &request(Some("api.example.com")),
                    &other
                )
                .is_err(),
                "a hostname serves exactly its own environment"
            );
            assert!(
                crate::auth_http::require_custom_domain(
                    &graph,
                    &request(Some("other.example.com")),
                    &own
                )
                .is_err()
            );
            assert!(
                crate::auth_http::require_custom_domain(&graph, &request(Some("")), &own).is_err()
            );
            block_on(graph.shutdown()).expect("shutdown");
        }
        {
            let graph = DataPlaneGraph::open(&config).expect("reopened graph");
            assert_eq!(
                graph.custom_domains().hostnames(&own),
                vec!["api.example.com".to_owned()],
                "the installed list is read back at startup"
            );
            assert_eq!(
                graph.allowed_origins().installed(&own),
                vec!["https://app.example.com".to_owned()],
                "so is the environment's cross-origin allowlist"
            );
            block_on(graph.allowed_origins().install(&own, Vec::new())).expect("withdrawn");
            assert!(
                !graph
                    .allowed_origins()
                    .allows(&own, "https://app.example.com"),
                "an emptied list ends cross-origin access at once"
            );
            // Replacing the list withdraws what is no longer in it.
            block_on(
                graph
                    .custom_domains()
                    .install(&own, vec!["app.example.com".to_owned()]),
            )
            .expect("replaced");
            assert!(!graph.custom_domains().permits(&own, "api.example.com"));
            assert!(graph.custom_domains().permits(&own, "app.example.com"));
            block_on(graph.custom_domains().install(&own, Vec::new())).expect("withdrawn");
            assert!(graph.custom_domains().hostnames(&own).is_empty());
            block_on(graph.shutdown()).expect("shutdown");
        }
    }

    /// The router the service serves carries the cross-origin middleware:
    /// a preflight from an origin the environment lists is answered before
    /// routing, every answer to that origin is labelled, and nothing else
    /// is -- not an unlisted origin, and not a route a browser application
    /// does not call, wherever the request arrived.
    #[test]
    fn the_router_answers_preflights_and_labels_responses_only_for_a_listed_origin() {
        use mako_service_runtime::{HttpMethod, HttpRequest};

        const LISTED: &str = "https://app.example.com";

        let directory = local_tempdir("data-plane-cors");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let graph = Arc::new(DataPlaneGraph::open(&config).expect("graph"));
        let own = tenant();
        block_on(
            graph
                .allowed_origins()
                .install(&own, vec![LISTED.to_owned()]),
        )
        .expect("installed");
        let router = crate::data_plane_router(Arc::clone(&graph)).expect("router");
        let scope = "/v1/projects/prj_example00/environments/env_example00";
        let request = |method, path: String, headers: Vec<(String, String)>| {
            HttpRequest::for_test(method, path, headers, Vec::new(), None)
        };
        let from = |origin: &str| vec![("Origin".to_owned(), origin.to_owned())];
        let signin = format!("{scope}/auth/signin");

        let preflight =
            router.respond_for_test(request(HttpMethod::Options, signin.clone(), from(LISTED)));
        assert_eq!(preflight.status_for_test(), 204);
        assert_eq!(
            preflight.header_for_test("access-control-allow-origin"),
            Some(LISTED)
        );
        assert_eq!(
            preflight.header_for_test("access-control-allow-methods"),
            Some(mako_service_runtime::CORS_ALLOW_METHODS)
        );
        assert_eq!(preflight.header_for_test("vary"), Some("Origin"));

        // An unlisted origin's preflight is not answered here at all: it
        // routes, and the route has no `OPTIONS`.
        let refused = router.respond_for_test(request(
            HttpMethod::Options,
            signin.clone(),
            from("https://other.example"),
        ));
        assert_eq!(refused.status_for_test(), 405);
        assert_eq!(refused.header_for_test("access-control-allow-origin"), None);

        // A real request is routed and its answer -- here the refusal of a
        // body-less sign-in -- comes back labelled.
        let posted =
            router.respond_for_test(request(HttpMethod::Post, signin.clone(), from(LISTED)));
        assert_eq!(
            posted.header_for_test("access-control-allow-origin"),
            Some(LISTED)
        );
        assert_eq!(
            posted.header_for_test("access-control-expose-headers"),
            Some(mako_service_runtime::CORS_EXPOSE_HEADERS)
        );
        // The same request on a custom domain of the same environment is
        // labelled the same way: the list belongs to the environment.
        let mut on_domain = from(LISTED);
        on_domain.push((
            "X-Mako-Custom-Domain".to_owned(),
            "api.example.com".to_owned(),
        ));
        block_on(
            graph
                .custom_domains()
                .install(&own, vec!["api.example.com".to_owned()]),
        )
        .expect("domain installed");
        let on_custom_domain =
            router.respond_for_test(request(HttpMethod::Post, signin, on_domain));
        assert_eq!(
            on_custom_domain.header_for_test("access-control-allow-origin"),
            Some(LISTED)
        );

        // The routes a browser application does not call are never
        // labelled, even for the listed origin: the environment's own
        // service-credential route, and the private internal protocol.
        for path in [
            format!("{scope}/service/collections/notes/documents/query"),
            format!("{scope}/explorer/collections/notes/browse"),
            "/_internal/v1/data/identity/verify".to_owned(),
        ] {
            let response =
                router.respond_for_test(request(HttpMethod::Post, path.clone(), from(LISTED)));
            assert_eq!(
                response.header_for_test("access-control-allow-origin"),
                None,
                "{path} must never be answered cross-origin"
            );
            assert_eq!(response.header_for_test("vary"), None, "{path}");
        }

        drop(router);
        let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
        block_on(graph.shutdown()).expect("shutdown");
    }

    #[test]
    fn local_graph_uses_rocksdb_and_persists_identity_authority() {
        let directory = local_tempdir("data-plane-local");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let graph = Arc::new(DataPlaneGraph::open(&config).expect("graph"));
        assert_eq!(graph.storage_mode(), StorageMode::LocalRocksDb);
        assert!(graph.readiness().is_ready());
        let router = crate::data_plane_router(Arc::clone(&graph)).expect("documented routes");
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/_internal/v1/data/identity/verify"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/v1/projects/prj_example00/environments/env_example00/auth/token"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/projects/prj_example00/environments/env_example00/collections/widgets/replication/stream"
        ));
        assert!(!router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/_internal/v1/data/identity/verify"
        ));
        assert!(!router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/_internal/v1/unknown"
        ));
        drop(router);

        let tenant = tenant();
        graph
            .identity_store(&tenant, &tenant)
            .expect("identity store");
        let first = block_on(graph.initialize_signing_key(&tenant, 10)).expect("signing key");
        let ring = block_on(graph.signing_key_ring(&tenant)).expect("ring");
        assert_eq!(ring.records().len(), 1);
        assert_eq!(ring.records()[0].key_id(), first.key_id());

        let identity = graph
            .identity_store(&tenant, &tenant)
            .expect("identity store");
        let signup = SignupService::new(
            &identity,
            graph.password_service(),
            &NoEmail,
            EmailSignupConfig {
                enabled: true,
                require_verification: false,
                verification_ttl_seconds: 60,
            },
        )
        .expect("signup service");
        block_on(signup.sign_up(&tenant, "route@example.test", "valid-password-123", 11))
            .expect("signup");
        let email = NormalizedEmail::parse("route@example.test").expect("email");
        let user = block_on(identity.user_by_email(&email))
            .expect("user lookup")
            .expect("user");
        let session =
            block_on(graph.create_application_session(&tenant, user.id(), 12)).expect("session");
        let verified = block_on(graph.verify_access_token(
            session.access_token.expose_for_authorization_header(),
            &tenant,
            13,
        ))
        .expect("gateway verification");
        assert_eq!(verified.user_id(), user.id());
        let signin_audit = crate::auth_http::DataPlaneAuthenticationAudit::new(Arc::clone(&graph));
        let signin = SignInService::new(
            &identity,
            graph.password_service(),
            graph.sign_in_throttle(),
            &signin_audit,
        )
        .expect("sign-in service");
        let signin_metadata =
            SignInRequestMetadata::new("req_route_signin", "127.0.0.1").expect("metadata");
        assert!(matches!(
            block_on(signin.sign_in(
                &tenant,
                "route@example.test",
                "valid-password-123",
                &signin_metadata,
                14,
            ))
            .expect("sign in"),
            SignInResponse::Authenticated { .. }
        ));
        drop(signin);
        drop(signin_audit);

        let authenticator = graph.internal_authenticator(InternalCaller::ControlPlane);
        let signed = authenticator
            .sign(
                InternalRoute::IdentityAdmin,
                &tenant,
                "req_graph",
                "idem_graph",
                100,
                b"{}".to_vec(),
            )
            .expect("signed internal request");
        let verified = authenticator
            .verify_signed(InternalRoute::IdentityAdmin, &signed, 100)
            .expect("verified internal request");
        let guard = graph
            .internal_replay_guard(&tenant, &tenant)
            .expect("internal replay guard");
        assert_eq!(
            block_on(guard.claim(&verified, 100)).expect("guard claim"),
            GuardDecision::Fresh
        );

        let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
        block_on(graph.shutdown()).expect("shutdown");
    }

    #[test]
    fn production_refuses_empty_unprovisioned_storage() {
        let directory = local_tempdir("data-plane-empty-production");
        let config = config_for(directory.path(), DeploymentEnvironment::Production);
        let result = DataPlaneGraph::open(&config);
        assert!(matches!(result, Err(DataPlaneGraphError::Storage(_))));
        assert!(
            fs::read_dir(directory.path().join("rocksdb"))
                .expect("database directory")
                .next()
                .is_none()
        );
    }

    #[test]
    fn production_opens_only_its_provisioned_owned_volume() {
        let directory = local_tempdir("data-plane-production");
        let config = config_for(directory.path(), DeploymentEnvironment::Production);
        provision_production_volume(
            &config.rocksdb.path,
            ProductionVolumeIdentity::new("mako-data-plane", "mako-data-plane-us-east-1-beta")
                .expect("identity"),
        )
        .expect("provision");

        let graph = DataPlaneGraph::open(&config).expect("production graph");
        assert_eq!(graph.storage_mode(), StorageMode::ProductionRocksDb);
        // Production refuses to serve without its object store: every other
        // dependency is ready, and the store, unreachable here, is what holds it.
        let readiness = graph.readiness();
        assert!(readiness.storage.is_ready());
        assert!(!readiness.object_store);
        assert!(!readiness.is_ready());
        assert_eq!(readiness.to_string(), "dependencies_not_ready:object_store");
        block_on(graph.shutdown()).expect("shutdown");
    }

    struct NoEmail;

    impl TransactionalEmailProvider for NoEmail {
        fn enqueue_verification(&self, _email: VerificationEmail) {}
    }
}
