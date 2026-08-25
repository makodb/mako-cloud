use std::{error::Error, fmt, num::NonZeroU64, num::NonZeroUsize, sync::Arc, time::Duration};

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
use mako_service_runtime::{ReadinessProbe, ReadinessSnapshot};
use mako_storage::{
    Durability, KvAdapter, ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity,
    RocksDbAdapter, RocksDbConfig, StorageError, StorageReadiness, check_storage_readiness,
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

struct DataPlaneComponents {
    documents: DocumentEngine,
    telemetry: Arc<mako_telemetry_client::TelemetryEmitter>,
    storage_sampler: Arc<crate::telemetry::StorageSampler>,
    quotas: Arc<GatewayQuotaEngine>,
    quota_policy: GatewayQuotaPolicy,
    quota_policies: Arc<PersistentQuotaPolicySource>,
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
                // The telemetry store has an ingest endpoint that nothing has
                // ever called, which is why every signal the management API
                // serves from it answers empty. This is the emitting side.
                telemetry: Arc::new(mako_telemetry_client::TelemetryEmitter::new(
                    config.telemetry_query_address,
                    secret.expose_secret(),
                    "mako.data-plane",
                )),
                storage_sampler: Arc::new(crate::telemetry::StorageSampler::new()),
                documents,
                quotas,
                quota_policies,
                quota_policy,
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

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use mako_api::{EnvironmentId, ProjectId};
    use mako_config::ConfigLoader;
    use mako_identity::{
        EmailSignupConfig, SignInRequestMetadata, SignInResponse, SignInService, SignupService,
        TransactionalEmailProvider, VerificationEmail,
    };
    use mako_internal_rpc::{GuardDecision, InternalReplayGuard, InternalRoute};
    use mako_storage::{ProductionVolumeIdentity, provision_production_volume};
    use tempfile::{Builder, TempDir};

    use super::*;

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
        assert!(graph.readiness().is_ready());
        block_on(graph.shutdown()).expect("shutdown");
    }

    fn config_for(root: &Path, environment: DeploymentEnvironment) -> ServiceConfig {
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
        ])
        .load(ServiceKind::DataPlane)
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

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_example00").expect("project"),
            EnvironmentId::parse("env_example00").expect("environment"),
        )
    }

    struct NoEmail;

    impl TransactionalEmailProvider for NoEmail {
        fn enqueue_verification(&self, _email: VerificationEmail) {}
    }
}
