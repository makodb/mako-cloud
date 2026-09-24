use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    future::Future,
    net::SocketAddr,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::executor::block_on;
use mako_api::{
    EnvironmentId, ExplorerCapabilityKey, ExplorerCapabilityKeyRing, ProjectId, TenantScope,
};
use mako_audit::{AuditStore, AuditStoreConfig, CursorSigningKey, TelemetryRedactor};
use mako_auth_providers::ProviderSecretKey;
use mako_config::{DeploymentEnvironment, ServiceConfig, ServiceKind};
use mako_control_plane::{
    AllowedOriginsService, ApplicationMailConfig, ApplicationMailStore, ApplicationMailWorker,
    ApplicationUserAccess, AutomationTokenService, CollectionAdminService, ControlAuditSink,
    ControlPlaneAuthenticator, CredentialAdminService, DataJobService, DataPlaneApplicationUsers,
    DataPlaneExplorerEpochs, DeveloperLookupKey, DeveloperMailCipher, DeveloperMailEncryptionKey,
    DeveloperMailOutboxWorker, DeveloperMailTransport, DeveloperRegistrationConfig,
    DeveloperRegistrationService, DeveloperRegistrationStore, DeveloperRestoreService,
    DeveloperWorkspaceSecurity, EmailTemplateService, EnvironmentRecord, ExplorerGrantService,
    FunctionAdminService, FunctionDeploymentBackend, FunctionSecretEncryptionKey, LifecycleState,
    ManagementAuthorizer, ObservabilityBackend, ObservabilityService, OperatorAuditSink,
    OperatorAuthenticationAuditSink, OperatorAuthenticationConfig, OperatorAuthenticationKey,
    OperatorAuthenticationService, OperatorAuthenticationStore, OperatorAuthenticator,
    OperatorService, OrganizationId, OrganizationRecord, OrganizationService, OrganizationStore,
    PolicyAdminService, ProductionObservabilityBackend, ProductionObservabilityConfig,
    ProjectEnvironmentService, ProjectRecord, ProjectStore, RuntimeDeploymentClient,
    RuntimeDeploymentClientConfig, RuntimeSupervisorCredential, TelemetryQueryCredential,
    WebhookService, WebhookStore, WebhookTransport, WebhookWorker, WebhookWorkerConfig,
};
use mako_control_plane::{
    CustomDomainService, CustomDomainStore, CustomDomainVerifier, FunctionScheduleInvoker,
    FunctionScheduleService, FunctionScheduleStore, FunctionScheduleWorker,
    FunctionScheduleWorkerConfig, UdpTxtResolver,
};
use mako_identity::KeyEncryptionKey;
use mako_internal_rpc::{
    ControlToDataClient, ControlToEdgeClient, DeploymentKey, IdentityAdminCommand,
    IdentityAdminOperation, IdentityAdminPermission, InternalCaller, InternalClientError,
    InternalHttpClient, InternalHttpClientConfig, InternalRequestAuthenticator,
    RocksInternalReplayGuard,
};
use mako_object_store::{ObjectStore, S3Credentials, S3ObjectStore, S3ObjectStoreConfig};
use mako_policy::PolicyCompiler;
use mako_provisioning::{
    KvProvisioningBackend, Provisioner, ProvisioningBackendConfig, ProvisioningOperation,
    ProvisioningResource, ProvisioningState, ProvisioningStore, ProvisioningWorkflow,
    ProvisioningWorkflowError, StrandedAction, StrandedPolicy, stranded_actions,
};
use mako_service_runtime::{ReadinessProbe, ReadinessSnapshot};
use mako_storage::{
    Durability, KvAdapter, SqliteAdapter, SqliteConfig, StorageError, StorageReadiness,
    check_storage_readiness,
};

use crate::{
    audit::{PersistentControlAudit, SharedControlAudit},
    developer_metrics::DeveloperMetrics,
    function_resolution::FunctionResolutionService,
    identity::{
        DeploymentDeveloperSessionIssuer, DeploymentHostedSessionAuthenticator,
        deployment_authenticators,
    },
    smtp::{ProductionSmtpConfig, ProductionSmtpTransport, SmtpCredentials, SmtpTlsMode},
};

const AUDIT_RETENTION_MILLISECONDS: u64 = 90 * 24 * 60 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlPlaneStorageMode {
    Sqlite,
}

enum StorageOwner {
    Sqlite(SqliteAdapter),
}

impl StorageOwner {
    fn open(config: &ServiceConfig) -> Result<Self, StorageError> {
        let settings = config.control_sqlite.as_ref().ok_or_else(|| {
            StorageError::invalid(
                "control_storage",
                "control-plane SQLite configuration is required",
            )
        })?;
        let mut sqlite =
            SqliteConfig::new(&settings.database_path, settings.database_identity.clone());
        sqlite.lock_path = settings.lock_path.clone();
        sqlite.create_if_missing = config.environment != DeploymentEnvironment::Production;
        sqlite.minimum_durability = Durability::Sync;
        sqlite.maximum_batch_operations = settings.maximum_batch_operations;
        sqlite.maximum_scan_items = settings.maximum_scan_items;
        sqlite.busy_timeout = settings.busy_timeout;
        sqlite.transaction_expiration = settings.transaction_expiration;
        sqlite.shutdown_timeout = settings.shutdown_timeout;
        sqlite.wal_autocheckpoint_pages = settings.wal_autocheckpoint_pages;
        sqlite.maximum_wal_bytes = settings.maximum_wal_bytes;
        sqlite.disk_warning_free_bytes = settings.disk_warning_free_bytes;
        sqlite.disk_critical_free_bytes = settings.disk_critical_free_bytes;
        SqliteAdapter::open(sqlite).map(Self::Sqlite)
    }

    fn adapter(&self) -> SqliteAdapter {
        match self {
            Self::Sqlite(adapter) => adapter.clone(),
        }
    }

    fn mode(&self) -> ControlPlaneStorageMode {
        ControlPlaneStorageMode::Sqlite
    }

    fn initial_readiness(&self) -> StorageReadiness {
        match self {
            Self::Sqlite(adapter) => block_on(check_storage_readiness(adapter, Durability::Sync)),
        }
    }

    fn health_signals(&self) -> Result<mako_storage::SqliteHealthSignals, StorageError> {
        match self {
            Self::Sqlite(adapter) => adapter.health_signals(),
        }
    }

    async fn shutdown(self) -> Result<(), StorageError> {
        match self {
            Self::Sqlite(adapter) => adapter.shutdown(),
        }
    }
}

struct ControlPlaneComponents {
    public_origin: String,
    telemetry_emitter: Arc<mako_telemetry_client::TelemetryEmitter>,
    function_log_collector: crate::function_logs::FunctionLogCollector,
    audit: SharedControlAudit,
    developer_authenticator: ControlPlaneAuthenticator,
    developer_session_issuer: DeploymentDeveloperSessionIssuer,
    hosted_session_authenticator: DeploymentHostedSessionAuthenticator,
    developer_registration: DeveloperRegistrationStore,
    developer_registration_service: DeveloperRegistrationService,
    developer_mail_worker: Option<DeveloperMailOutboxWorker>,
    application_mail_worker: Option<ApplicationMailWorker>,
    email_templates: EmailTemplateService,
    allowed_origins: AllowedOriginsService,
    webhooks: WebhookService,
    webhook_worker: WebhookWorker,
    function_schedules: FunctionScheduleService,
    function_schedule_worker: FunctionScheduleWorker,
    custom_domains: CustomDomainService,
    custom_domain_verifier: CustomDomainVerifier,
    developer_metrics: Arc<DeveloperMetrics>,
    operator_authenticator: OperatorAuthenticator,
    operator_password_authentication: OperatorAuthenticationService,
    operator_break_glass_bearer_enabled: bool,
    organizations: OrganizationStore,
    projects: ProjectStore,
    organization_service: OrganizationService,
    project_service: ProjectEnvironmentService,
    management_authorizer: ManagementAuthorizer,
    automation_tokens: AutomationTokenService,
    collections: CollectionAdminService,
    policies: PolicyAdminService,
    application_users: ApplicationUserAccess,
    explorer_grants: ExplorerGrantService,
    data_jobs: DataJobService,
    developer_workspace_security: DeveloperWorkspaceSecurity,
    developer_restores: DeveloperRestoreService,
    developer_backups: Arc<crate::operator_provider::ProductionOperatorProvider>,
    provisioning: Provisioner,
    provisioning_backend: KvProvisioningBackend,
    operator_service: OperatorService,
    operator_control_center: mako_control_plane::OperatorControlCenterService,
    data_plane_identity_admin: ControlToDataClient,
    credentials: CredentialAdminService,
    functions: FunctionAdminService,
    object_store: Arc<S3ObjectStore>,
    runtime: Arc<RuntimeDeploymentClient>,
    observability_backend: Arc<ProductionObservabilityBackend>,
    observability: ObservabilityService,
    function_resolution: FunctionResolutionService,
    /// Seals application sign-in client secrets for the data plane, which
    /// derives the same key from the same internal secret.
    provider_secret_key: ProviderSecretKey,
    internal_deployment_key: DeploymentKey,
    enforce_production_dependencies: bool,
}

/// Complete persistent dependency graph for management and operator traffic.
/// It deliberately exposes no control-owned application credential authority.
pub struct ControlPlaneGraph {
    storage: StorageOwner,
    adapter: Arc<dyn KvAdapter>,
    components: ControlPlaneComponents,
    /// When the provisioning worker last looked for records stranded in
    /// `provisioning` without a workflow; zero until it has.
    last_stranded_sweep_unix_seconds: AtomicU64,
}

/// How long a project or environment may sit in `provisioning` with no
/// workflow before the worker treats it as stranded and makes one. Long
/// enough that a creation request still writing its workflow is never raced.
const STRANDED_GRACE_SECONDS: u64 = 120;
/// How often the worker sweeps for stranded records. Each sweep reads every
/// organization, project, and environment, so it runs well under the pace of
/// the ten-second workflow pass.
const STRANDED_SWEEP_INTERVAL_SECONDS: u64 = 60;
/// One page of a record listing during the stranded-record sweep, and the
/// most records of one kind a sweep reads before leaving the rest to the
/// next: a pass stays bounded however large the deployment grows.
const SWEEP_PAGE: NonZeroUsize = NonZeroUsize::new(100).expect("page is positive");
const SWEEP_RECORD_CAP: usize = 10_000;
/// The actor the worker names when it installs an environment's plan limits.
/// Not a developer: the install is the platform's own step, taken for every
/// environment regardless of who asked for it.
const PROVISIONING_WORKER_ACTOR_ID: &str = "control-plane-provisioning-worker";

impl ControlPlaneGraph {
    pub fn open(config: &ServiceConfig) -> Result<Self, ControlPlaneGraphError> {
        Self::open_with_data_plane_endpoint(config, config.data_plane_address)
    }

    fn open_with_data_plane_endpoint(
        config: &ServiceConfig,
        data_plane_endpoint: SocketAddr,
    ) -> Result<Self, ControlPlaneGraphError> {
        Self::open_with_dependencies(config, data_plane_endpoint, None, None, None)
    }

    fn production_smtp_transport(
        registration: &mako_config::DeveloperRegistrationSettings,
    ) -> Result<Option<Arc<dyn DeveloperMailTransport>>, ControlPlaneGraphError> {
        registration
            .smtp
            .as_ref()
            .map(|smtp| {
                ProductionSmtpTransport::new(ProductionSmtpConfig {
                    relay_hostname: smtp.relay_hostname.clone(),
                    port: smtp.port,
                    tls_mode: match smtp.tls_mode {
                        mako_config::SmtpTlsMode::Wrapper => SmtpTlsMode::Wrapper,
                        mako_config::SmtpTlsMode::StartTls => SmtpTlsMode::StartTls,
                        mako_config::SmtpTlsMode::Plaintext => SmtpTlsMode::Plaintext,
                    },
                    credentials: match (&smtp.username, &smtp.password) {
                        (Some(username), Some(password)) => Some(SmtpCredentials {
                            username: username.clone(),
                            password: password.expose_secret().to_owned(),
                        }),
                        _ => None,
                    },
                    sender: smtp.sender.clone(),
                    timeout: smtp.timeout,
                })
                .map(|transport| Arc::new(transport) as Arc<dyn DeveloperMailTransport>)
            })
            .transpose()
            .map_err(|_| ControlPlaneGraphError::Composition("SMTP relay"))
    }

    /// `mail_transport` replaces the configured SMTP relay. Hosted registration
    /// only opens when mail is genuinely reachable, so a test that drives it
    /// has to supply a transport rather than name a relay it cannot contact.
    /// `schedule_invoker` likewise replaces the edge gateway for the
    /// scheduler.
    #[allow(clippy::field_reassign_with_default)]
    fn open_with_dependencies(
        config: &ServiceConfig,
        data_plane_endpoint: SocketAddr,
        mail_transport: Option<Arc<dyn DeveloperMailTransport>>,
        webhook_transport: Option<Arc<dyn WebhookTransport>>,
        schedule_invoker: Option<Arc<dyn FunctionScheduleInvoker>>,
    ) -> Result<Self, ControlPlaneGraphError> {
        if config.service != ServiceKind::ControlPlane {
            return Err(ControlPlaneGraphError::WrongService);
        }
        let secret = config
            .internal_auth_secret
            .as_ref()
            .ok_or(ControlPlaneGraphError::MissingKeyMaterial)?;
        if secret.expose_secret().len() < 32 || secret.expose_secret().chars().any(char::is_control)
        {
            return Err(ControlPlaneGraphError::InvalidKeyMaterial);
        }
        let public_origin = config.public_url.as_ref().map_or_else(
            || format!("http://{}", config.bind_address),
            ToString::to_string,
        );
        let issuer = format!("{}/control-identity", public_origin.trim_end_matches('/'));
        let storage = StorageOwner::open(config)?;
        if !storage.initial_readiness().is_ready() {
            return Err(ControlPlaneGraphError::StorageNotReady);
        }
        let adapter: Arc<dyn KvAdapter> = Arc::new(storage.adapter());
        let developer_registration = DeveloperRegistrationStore::new(
            Arc::clone(&adapter),
            Durability::Sync,
            DeveloperLookupKey::derive(secret.expose_secret().as_bytes()),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("developer registration store"))?;
        block_on(developer_registration.migrate_legacy_developers())
            .map_err(|_| ControlPlaneGraphError::Composition("developer identity migration"))?;
        let registration = &config.developer_registration;
        let mail_key_material = registration
            .mail_encryption_secret
            .as_ref()
            .map_or_else(|| secret.expose_secret(), |value| value.expose_secret());
        let mail_key = DeveloperMailEncryptionKey::derive(mail_key_material.as_bytes());
        let smtp_transport: Option<Arc<dyn DeveloperMailTransport>> = match mail_transport {
            Some(transport) => Some(transport),
            None => Self::production_smtp_transport(registration)?,
        };
        let mail_ready = smtp_transport
            .as_ref()
            .is_some_and(|transport| block_on(transport.readiness()).is_ok());
        let mut developer_registration_config = DeveloperRegistrationConfig::default();
        developer_registration_config.enabled = registration.enabled;
        developer_registration_config.mail_ready = mail_ready;
        developer_registration_config.public_origin =
            public_origin.trim_end_matches('/').to_owned();
        developer_registration_config.issuer = issuer.clone();
        developer_registration_config.verification_lifetime_seconds =
            registration.verification_lifetime.as_secs();
        developer_registration_config.recovery_lifetime_seconds =
            registration.recovery_lifetime.as_secs();
        developer_registration_config.access_lifetime_seconds =
            registration.access_lifetime.as_secs();
        developer_registration_config.refresh_lifetime_seconds =
            registration.refresh_lifetime.as_secs();
        developer_registration_config.decision_retention_seconds =
            registration.decision_retention.as_secs();
        developer_registration_config.rate_window_seconds = registration.rate_window.as_secs();
        developer_registration_config.global_requests_per_window =
            registration.global_requests_per_window;
        developer_registration_config.source_requests_per_window =
            registration.source_requests_per_window;
        developer_registration_config.email_requests_per_window =
            registration.email_requests_per_window;
        developer_registration_config.token_attempts_per_window =
            registration.token_attempts_per_window;
        developer_registration_config.maximum_parallel_password_work =
            registration.maximum_parallel_password_work;
        developer_registration_config.maximum_pending_outbox = registration.maximum_pending_outbox;
        developer_registration_config.maximum_outbox_batch = registration.maximum_outbox_batch;
        developer_registration_config.outbox_lease_seconds = registration.outbox_lease.as_secs();
        developer_registration_config.outbox_maximum_attempts =
            registration.outbox_maximum_attempts;
        developer_registration_config.outbox_maximum_backoff_seconds =
            registration.outbox_maximum_backoff.as_secs();
        developer_registration_config.delivered_mail_retention_seconds =
            registration.delivered_mail_retention.as_secs();
        let developer_registration_service = DeveloperRegistrationService::new(
            developer_registration.clone(),
            developer_registration_config.clone(),
            mail_key.clone(),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("developer registration service"))?;
        let operator_authentication_store = OperatorAuthenticationStore::new(
            Arc::clone(&adapter),
            Durability::Sync,
            OperatorAuthenticationKey::derive(secret.expose_secret().as_bytes()),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("operator authentication store"))?;
        block_on(operator_authentication_store.migrate()).map_err(|_| {
            ControlPlaneGraphError::Composition("operator authentication migration")
        })?;
        let developer_metrics = Arc::new(DeveloperMetrics::default());
        let application_mail_config =
            ApplicationMailConfig::from_registration(&developer_registration_config);
        let developer_mail_worker = smtp_transport
            .clone()
            .map(|transport| {
                DeveloperMailOutboxWorker::new(
                    developer_registration.clone(),
                    DeveloperMailCipher::new(mail_key.clone()),
                    transport,
                    developer_registration_config,
                )
            })
            .transpose()
            .map_err(|_| ControlPlaneGraphError::Composition("developer mail worker"))?;
        let (developer_authenticator, operator_authenticator) = deployment_authenticators(
            secret.expose_secret(),
            &issuer,
            Some(developer_registration.clone()),
        )
        .map_err(|_| ControlPlaneGraphError::Identity)?;
        let operator_authenticator = operator_authenticator
            .with_enabled(config.operator_authentication.break_glass_bearer_enabled);
        let developer_session_issuer =
            DeploymentDeveloperSessionIssuer::derive(secret.expose_secret());
        let hosted_session_authenticator = DeploymentHostedSessionAuthenticator::new(
            secret.expose_secret(),
            issuer.clone(),
            developer_registration.clone(),
        );

        let audit_store = AuditStore::new(
            Arc::clone(&adapter),
            AuditStoreConfig {
                durability: Durability::Sync,
                retention_milliseconds: AUDIT_RETENTION_MILLISECONDS,
                maximum_page_records: nonzero(100),
                maximum_export_records: nonzero(10_000),
                maximum_examined_records: nonzero(50_000),
                cursor_signing_key: CursorSigningKey::new(blake3::derive_key(
                    "mako/control-plane/audit-cursor-signing/v1",
                    secret.expose_secret().as_bytes(),
                )),
            },
        )
        .map_err(|_| ControlPlaneGraphError::Composition("audit store"))?;
        let redactor = TelemetryRedactor::new([secret.expose_secret()])
            .map_err(|_| ControlPlaneGraphError::Composition("audit redaction"))?;
        let audit = Arc::new(PersistentControlAudit::new(
            audit_store.clone(),
            redactor,
            Arc::clone(&adapter),
        ));
        let control_audit: Arc<dyn ControlAuditSink> = audit.clone();
        let operator_audit: Arc<dyn OperatorAuditSink> = audit.clone();
        let operator_authentication_audit: Arc<dyn OperatorAuthenticationAuditSink> = audit.clone();
        let operator_settings = &config.operator_authentication;
        let operator_password_authentication = OperatorAuthenticationService::new(
            operator_authentication_store,
            developer_registration_service.clone(),
            OperatorAuthenticationConfig {
                enabled: operator_settings.enabled,
                session_lifetime_seconds: operator_settings.session_lifetime.as_secs(),
                mutation_freshness_seconds: operator_settings.mutation_freshness.as_secs(),
                attempt_window_seconds: operator_settings.attempt_window.as_secs(),
                source_attempts_per_window: operator_settings.source_attempts_per_window,
                identity_attempts_per_window: operator_settings.identity_attempts_per_window,
                base_backoff_seconds: operator_settings.base_backoff.as_secs(),
                maximum_backoff_seconds: operator_settings.maximum_backoff.as_secs(),
                cookie_name: operator_settings.cookie_name.clone(),
            },
            operator_authentication_audit,
        )
        .map_err(|_| ControlPlaneGraphError::Composition("operator password authentication"))?;

        let organizations = OrganizationStore::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| ControlPlaneGraphError::Composition("organization store"))?;
        let projects = ProjectStore::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| ControlPlaneGraphError::Composition("project store"))?;
        let organization_service =
            OrganizationService::new(organizations.clone(), Arc::clone(&control_audit));
        let project_service = ProjectEnvironmentService::new(
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
        );
        let management_authorizer =
            ManagementAuthorizer::new(organizations.clone(), projects.clone());
        let automation_tokens = AutomationTokenService::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| ControlPlaneGraphError::Composition("automation tokens"))?;
        let collections = CollectionAdminService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("collection management"))?;
        let policies = PolicyAdminService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
            PolicyCompiler::default(),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("policy management"))?;
        let application_users = ApplicationUserAccess::new(
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
        );
        let explorer_capability_key = ExplorerCapabilityKey::new(
            "xcap-v1",
            &blake3::derive_key(
                "mako/explorer-capability-signing/v1",
                secret.expose_secret().as_bytes(),
            ),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("explorer capability key"))?;
        let explorer_grants = ExplorerGrantService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            developer_registration.clone(),
            projects.clone(),
            organizations.clone(),
            collections.clone(),
            ExplorerCapabilityKeyRing::new(explorer_capability_key, Vec::new())
                .map_err(|_| ControlPlaneGraphError::Composition("explorer capability key ring"))?,
            Arc::clone(&control_audit),
        );

        let provisioning_store = ProvisioningStore::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| ControlPlaneGraphError::Composition("provisioning store"))?;
        let provisioning = Provisioner::new(provisioning_store);
        let provisioning_config = ProvisioningBackendConfig::new(
            &public_origin,
            &public_origin,
            &public_origin,
            default_quotas(),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("provisioning configuration"))?;
        let provisioning_backend =
            KvProvisioningBackend::new(Arc::clone(&adapter), Durability::Sync, provisioning_config)
                .map_err(|_| ControlPlaneGraphError::Composition("provisioning backend"))?;
        let operator_service = OperatorService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            provisioning.clone(),
            Arc::clone(&operator_audit),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("operator service"))?;

        let deployment_key = DeploymentKey::derive(secret.expose_secret())
            .map_err(|_| ControlPlaneGraphError::Composition("internal authentication"))?;
        let internal_client = InternalHttpClient::new(
            InternalHttpClientConfig::loopback(data_plane_endpoint),
            deployment_key.clone(),
            InternalCaller::ControlPlane,
        )
        .map_err(|_| ControlPlaneGraphError::Composition("data-plane client"))?;
        let data_plane_identity_admin = ControlToDataClient::new(internal_client)
            .map_err(|_| ControlPlaneGraphError::Composition("data-plane identity client"))?;
        // Application users live in the data plane; a policy preview must find
        // the user it impersonates there, not in control storage.
        let explorer_grants = explorer_grants
            .with_application_users(Arc::new(DataPlaneApplicationUsers::new(Arc::new(
                data_plane_identity_admin.clone(),
            ))))
            .with_epochs(Arc::new(DataPlaneExplorerEpochs::new(Arc::new(
                data_plane_identity_admin.clone(),
            ))));
        let function_encryption_key = FunctionSecretEncryptionKey::from_bytes(blake3::derive_key(
            "mako/control-plane/function-secret-encryption/v1",
            secret.expose_secret().as_bytes(),
        ));
        let credentials = CredentialAdminService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
            KeyEncryptionKey::from_bytes(blake3::derive_key(
                "mako/control-plane/unused-local-signing-key-encryption/v1",
                secret.expose_secret().as_bytes(),
            )),
            function_encryption_key.clone(),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("function secret service"))?;
        let email_templates = EmailTemplateService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("email templates"))?;
        let allowed_origins = AllowedOriginsService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("allowed origins"))?;
        // Application mail shares the developer transport and the developer
        // mail key; without a relay there is nothing to drain intents into.
        let application_mail_worker = smtp_transport
            .map(|transport| {
                ApplicationMailWorker::new(
                    ApplicationMailStore::new(Arc::clone(&adapter), Durability::Sync)?,
                    DeveloperMailCipher::new(mail_key.clone()),
                    transport,
                    Arc::new(data_plane_identity_admin.clone()),
                    email_templates.clone(),
                    application_mail_config,
                )
            })
            .transpose()
            .map_err(|_| ControlPlaneGraphError::Composition("application mail worker"))?;
        // Webhooks: signing secrets rest under the developer-mail key; the
        // change feed is the data plane; deliveries go out over the bounded
        // HTTPS client, which speaks plain HTTP to loopback only outside
        // staging and production so local stubs can receive them.
        let allow_plain_http_loopback = !matches!(
            config.environment,
            DeploymentEnvironment::Staging | DeploymentEnvironment::Production
        );
        let webhook_store = WebhookStore::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| ControlPlaneGraphError::Composition("webhook store"))?;
        let webhooks = WebhookService::new(
            webhook_store.clone(),
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
            DeveloperMailCipher::new(mail_key.clone()),
            Arc::new(data_plane_identity_admin.clone()),
            allow_plain_http_loopback,
        );
        let webhook_transport: Arc<dyn WebhookTransport> = match webhook_transport {
            Some(transport) => transport,
            None => Arc::new(mako_control_plane::HttpWebhookTransport::new(
                allow_plain_http_loopback,
            )),
        };
        let webhook_worker = WebhookWorker::new(
            webhook_store,
            DeveloperMailCipher::new(mail_key),
            webhook_transport,
            Arc::new(data_plane_identity_admin.clone()),
            WebhookWorkerConfig::default(),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("webhook worker"))?;
        // Scheduled functions: invocations go through the edge gateway on
        // the internal hop, with an I/O timeout that outlasts a function's
        // wall-clock limit rather than the short loopback default.
        let schedule_invoker: Arc<dyn FunctionScheduleInvoker> = match schedule_invoker {
            Some(invoker) => invoker,
            None => {
                let edge_client = InternalHttpClient::new(
                    InternalHttpClientConfig {
                        endpoint: config.edge_gateway_address,
                        connect_timeout: Duration::from_secs(2),
                        io_timeout: Duration::from_secs(
                            mako_control_plane::FUNCTION_SCHEDULE_INVOCATION_TIMEOUT_SECONDS + 5,
                        ),
                        maximum_response_bytes: 64 * 1024,
                    },
                    deployment_key.clone(),
                    InternalCaller::ControlPlane,
                )
                .map_err(|_| ControlPlaneGraphError::Composition("edge-gateway client"))?;
                Arc::new(
                    ControlToEdgeClient::new(edge_client)
                        .map_err(|_| ControlPlaneGraphError::Composition("edge-gateway client"))?,
                )
            }
        };
        let schedule_store = FunctionScheduleStore::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| ControlPlaneGraphError::Composition("function schedule store"))?;
        let function_schedules = FunctionScheduleService::new(
            schedule_store.clone(),
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
        );
        let function_schedule_worker = FunctionScheduleWorker::new(
            schedule_store,
            schedule_invoker,
            FunctionScheduleWorkerConfig::default(),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("function schedule worker"))?;
        // Custom domains: the verifier asks the host's resolver for the
        // proof record and publishes each environment's verified list to
        // the data plane over the identity-administration route quota
        // policies travel on.
        let custom_domain_store = CustomDomainStore::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| ControlPlaneGraphError::Composition("custom domain store"))?;
        let custom_domain_verifier = CustomDomainVerifier::new(
            custom_domain_store,
            Arc::new(UdpTxtResolver::new(config.dns_resolver)),
            Arc::new(data_plane_identity_admin.clone()),
        );
        let custom_domains = CustomDomainService::new(
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
            custom_domain_verifier.clone(),
            config.public_hostname.clone(),
        );
        let object_access = config
            .object_store_access_key
            .as_ref()
            .ok_or(ControlPlaneGraphError::MissingKeyMaterial)?;
        let object_secret = config
            .object_store_secret_key
            .as_ref()
            .ok_or(ControlPlaneGraphError::MissingKeyMaterial)?;
        let object_store = Arc::new(
            S3ObjectStore::new(
                S3ObjectStoreConfig::loopback(
                    config.object_store_endpoint.clone(),
                    config.region.clone(),
                ),
                S3Credentials::new(object_access.expose_secret(), object_secret.expose_secret())
                    .map_err(|_| ControlPlaneGraphError::Composition("object-store credentials"))?,
            )
            .map_err(|_| ControlPlaneGraphError::Composition("object store"))?,
        );
        let data_jobs = DataJobService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            collections.clone(),
            object_store.clone(),
            blake3::derive_key(
                "mako/control-plane/data-job-artifact-grant/v1",
                secret.expose_secret().as_bytes(),
            ),
            &public_origin,
            data_plane_identity_admin.clone(),
            Arc::clone(&control_audit),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("data jobs"))?;
        let runtime = Arc::new(
            RuntimeDeploymentClient::new(
                RuntimeDeploymentClientConfig::loopback(
                    config.runtime_supervisor_address,
                    config.region.clone(),
                ),
                RuntimeSupervisorCredential::new(secret.expose_secret())
                    .map_err(|_| ControlPlaneGraphError::Composition("runtime credential"))?,
            )
            .map_err(|_| ControlPlaneGraphError::Composition("runtime client"))?,
        );
        let objects: Arc<dyn ObjectStore> = object_store.clone();
        let deployment_backend: Arc<dyn FunctionDeploymentBackend> = runtime.clone();
        // The emitting side of the telemetry pipeline for this plane: what
        // carries collected function logs into the retained store.
        let telemetry_emitter = Arc::new(mako_telemetry_client::TelemetryEmitter::new(
            config.telemetry_query_address,
            secret.expose_secret(),
            "mako.control-plane",
        ));
        let functions = FunctionAdminService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
            credentials.clone(),
            objects,
            deployment_backend,
        )
        .map_err(|_| ControlPlaneGraphError::Composition("function administration"))?;
        let function_log_collector = crate::function_logs::FunctionLogCollector::new(
            Arc::clone(&adapter),
            organizations.clone(),
            projects.clone(),
            Arc::new(functions.clone()),
            Arc::clone(&telemetry_emitter),
            Arc::new(
                TelemetryRedactor::new([secret.expose_secret()])
                    .map_err(|_| ControlPlaneGraphError::Composition("log scrubbing"))?,
            ),
        );
        let observability_backend = Arc::new(
            ProductionObservabilityBackend::new(
                ProductionObservabilityConfig::loopback(
                    config.telemetry_query_address,
                    config.region.clone(),
                ),
                audit_store.clone(),
                Arc::clone(&adapter),
                Arc::new(
                    TelemetryRedactor::new([secret.expose_secret()]).map_err(|_| {
                        ControlPlaneGraphError::Composition("observability redaction")
                    })?,
                ),
                TelemetryQueryCredential::new(secret.expose_secret())
                    .map_err(|_| ControlPlaneGraphError::Composition("telemetry credential"))?,
            )
            .map_err(|_| ControlPlaneGraphError::Composition("observability backend"))?,
        );
        let backend: Arc<dyn ObservabilityBackend> = observability_backend.clone();
        let observability = ObservabilityService::new(
            projects.clone(),
            organizations.clone(),
            Arc::clone(&control_audit),
            backend,
        );
        let operator_control_center_config = operator_control_center_config();
        let developer_backups =
            Arc::new(crate::operator_provider::ProductionOperatorProvider::new(
                observability_backend.clone(),
                operator_control_center_config
                    .allowed_diagnostic_origins
                    .clone(),
                operator_backup_evidence(),
                storage.adapter(),
            ));
        let operator_provider: Arc<dyn mako_control_plane::OperatorProvider> =
            developer_backups.clone();
        let operator_control_center = mako_control_plane::OperatorControlCenterService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            developer_registration.clone(),
            operator_provider,
            operator_audit,
            blake3::derive_key(
                "mako/operator-control-center/cursor-signing/v1",
                secret.expose_secret().as_bytes(),
            ),
            operator_control_center_config,
        )
        .map_err(|_| ControlPlaneGraphError::Composition("operator control center"))?;
        let function_resolution =
            FunctionResolutionService::new(Arc::clone(&adapter), function_encryption_key)?;
        let provider_secret_key = ProviderSecretKey::derive(secret.expose_secret().as_bytes());
        let developer_workspace_security = DeveloperWorkspaceSecurity::new(blake3::derive_key(
            "mako/developer-workspace/step-up/v1",
            secret.expose_secret().as_bytes(),
        ));
        let developer_restores = DeveloperRestoreService::new(
            Arc::clone(&adapter),
            Durability::Sync,
            projects.clone(),
            organizations.clone(),
            developer_workspace_security.clone(),
        )
        .map_err(|_| ControlPlaneGraphError::Composition("developer restore service"))?;

        Ok(Self {
            storage,
            adapter,
            last_stranded_sweep_unix_seconds: AtomicU64::new(0),
            components: ControlPlaneComponents {
                public_origin: public_origin.trim_end_matches('/').to_owned(),
                telemetry_emitter,
                function_log_collector,
                audit,
                developer_authenticator,
                developer_session_issuer,
                hosted_session_authenticator,
                developer_registration,
                developer_registration_service,
                developer_mail_worker,
                application_mail_worker,
                email_templates,
                allowed_origins,
                webhooks,
                webhook_worker,
                function_schedules,
                function_schedule_worker,
                custom_domains,
                custom_domain_verifier,
                developer_metrics,
                operator_authenticator,
                operator_password_authentication,
                operator_break_glass_bearer_enabled: operator_settings.break_glass_bearer_enabled,
                organizations,
                projects,
                organization_service,
                project_service,
                management_authorizer,
                automation_tokens,
                collections,
                policies,
                application_users,
                explorer_grants,
                data_jobs,
                developer_workspace_security,
                developer_restores,
                developer_backups,
                provisioning,
                provisioning_backend,
                operator_service,
                operator_control_center,
                data_plane_identity_admin,
                credentials,
                functions,
                object_store,
                runtime,
                observability_backend,
                observability,
                function_resolution,
                provider_secret_key,
                internal_deployment_key: deployment_key,
                enforce_production_dependencies: config.environment
                    == DeploymentEnvironment::Production,
            },
        })
    }

    #[must_use]
    pub fn storage_mode(&self) -> ControlPlaneStorageMode {
        self.storage.mode()
    }

    #[must_use]
    pub fn developer_authenticator(&self) -> &ControlPlaneAuthenticator {
        &self.components.developer_authenticator
    }

    #[must_use]
    pub fn developer_registration_store(&self) -> &DeveloperRegistrationStore {
        &self.components.developer_registration
    }

    #[must_use]
    pub fn developer_registration_service(&self) -> &DeveloperRegistrationService {
        &self.components.developer_registration_service
    }

    pub fn developer_mail_worker(&self) -> Option<&DeveloperMailOutboxWorker> {
        self.components.developer_mail_worker.as_ref()
    }

    #[must_use]
    pub fn application_mail_worker(&self) -> Option<&ApplicationMailWorker> {
        self.components.application_mail_worker.as_ref()
    }

    #[must_use]
    pub fn email_template_service(&self) -> &EmailTemplateService {
        &self.components.email_templates
    }

    #[must_use]
    pub fn allowed_origins_service(&self) -> &AllowedOriginsService {
        &self.components.allowed_origins
    }

    #[must_use]
    pub fn webhook_service(&self) -> &WebhookService {
        &self.components.webhooks
    }

    #[must_use]
    pub fn webhook_worker(&self) -> &WebhookWorker {
        &self.components.webhook_worker
    }

    pub fn observe_webhooks(&self, report: &mako_control_plane::WebhookWorkerReport) {
        self.components.developer_metrics.observe_webhooks(report);
    }

    pub fn observe_webhook_worker_failure(&self) {
        self.components
            .developer_metrics
            .observe_webhook_worker_failure();
    }

    #[must_use]
    pub fn function_schedule_service(&self) -> &FunctionScheduleService {
        &self.components.function_schedules
    }

    #[must_use]
    pub fn function_schedule_worker(&self) -> &FunctionScheduleWorker {
        &self.components.function_schedule_worker
    }

    pub fn observe_function_schedules(
        &self,
        report: &mako_control_plane::FunctionScheduleWorkerReport,
    ) {
        self.components
            .developer_metrics
            .observe_function_schedules(report);
    }

    pub fn observe_function_schedule_worker_failure(&self) {
        self.components
            .developer_metrics
            .observe_function_schedule_worker_failure();
    }

    #[must_use]
    pub fn custom_domain_service(&self) -> &CustomDomainService {
        &self.components.custom_domains
    }

    #[must_use]
    pub fn custom_domain_verifier(&self) -> &CustomDomainVerifier {
        &self.components.custom_domain_verifier
    }

    pub fn observe_custom_domains(&self, report: &mako_control_plane::CustomDomainVerifierReport) {
        self.components
            .developer_metrics
            .observe_custom_domains(report);
    }

    pub fn observe_custom_domain_worker_failure(&self) {
        self.components
            .developer_metrics
            .observe_custom_domain_worker_failure();
    }

    pub(crate) fn developer_metrics(&self) -> &Arc<DeveloperMetrics> {
        &self.components.developer_metrics
    }

    pub(crate) fn control_storage_health_signals(
        &self,
    ) -> Result<mako_storage::SqliteHealthSignals, StorageError> {
        self.storage.health_signals()
    }

    pub fn observe_developer_mail(&self, report: &mako_control_plane::DeveloperOutboxWorkerReport) {
        self.components.developer_metrics.observe_mail(report);
    }

    pub fn observe_developer_mail_worker_failure(&self) {
        self.components
            .developer_metrics
            .observe_mail_worker_failure();
    }

    pub fn observe_application_mail(
        &self,
        report: &mako_control_plane::ApplicationMailWorkerReport,
    ) {
        self.components
            .developer_metrics
            .observe_application_mail(report);
    }

    pub fn observe_application_mail_worker_failure(&self) {
        self.components
            .developer_metrics
            .observe_application_mail_worker_failure();
    }

    pub async fn cleanup_operator_authentication(
        &self,
        now_unix_seconds: u64,
    ) -> Result<mako_control_plane::OperatorAuthenticationCleanupReport, ControlPlaneGraphError>
    {
        self.components
            .operator_password_authentication
            .store()
            .cleanup_expired(
                now_unix_seconds,
                NonZeroUsize::new(256).expect("operator cleanup limit is non-zero"),
            )
            .await
            .map_err(|_| ControlPlaneGraphError::Composition("operator authentication cleanup"))
    }

    pub(crate) fn developer_session_issuer(&self) -> &DeploymentDeveloperSessionIssuer {
        &self.components.developer_session_issuer
    }

    pub(crate) fn hosted_session_authenticator(&self) -> &DeploymentHostedSessionAuthenticator {
        &self.components.hosted_session_authenticator
    }

    #[must_use]
    pub fn operator_authenticator(&self) -> &OperatorAuthenticator {
        &self.components.operator_authenticator
    }

    #[must_use]
    pub fn operator_password_authentication(&self) -> &OperatorAuthenticationService {
        &self.components.operator_password_authentication
    }

    #[must_use]
    pub const fn operator_break_glass_bearer_enabled(&self) -> bool {
        self.components.operator_break_glass_bearer_enabled
    }

    #[must_use]
    pub fn telemetry_emitter(&self) -> &Arc<mako_telemetry_client::TelemetryEmitter> {
        &self.components.telemetry_emitter
    }

    /// One pass of the function-log collector: read every deployed
    /// function's supervisor buffer and emit what is new, scrubbed. Driven
    /// from a worker thread, never from a request.
    pub async fn collect_function_logs(&self, now_unix_milliseconds: u64) -> usize {
        self.components
            .function_log_collector
            .collect_once(now_unix_milliseconds)
            .await
    }

    pub fn organization_store(&self) -> &OrganizationStore {
        &self.components.organizations
    }

    #[must_use]
    pub fn project_store(&self) -> &ProjectStore {
        &self.components.projects
    }

    #[must_use]
    pub fn organization_service(&self) -> &OrganizationService {
        &self.components.organization_service
    }

    #[must_use]
    pub fn project_service(&self) -> &ProjectEnvironmentService {
        &self.components.project_service
    }

    #[must_use]
    pub fn management_authorizer(&self) -> &ManagementAuthorizer {
        &self.components.management_authorizer
    }

    #[must_use]
    pub fn automation_token_service(&self) -> &AutomationTokenService {
        &self.components.automation_tokens
    }

    #[must_use]
    pub fn collection_service(&self) -> &CollectionAdminService {
        &self.components.collections
    }

    #[must_use]
    pub fn policy_service(&self) -> &PolicyAdminService {
        &self.components.policies
    }

    #[must_use]
    pub fn application_user_access(&self) -> &ApplicationUserAccess {
        &self.components.application_users
    }

    #[must_use]
    pub fn explorer_grant_service(&self) -> &ExplorerGrantService {
        &self.components.explorer_grants
    }

    #[must_use]
    pub fn data_job_service(&self) -> &DataJobService {
        &self.components.data_jobs
    }

    #[must_use]
    pub fn developer_workspace_security(&self) -> &DeveloperWorkspaceSecurity {
        &self.components.developer_workspace_security
    }

    #[must_use]
    pub fn developer_restore_service(&self) -> &DeveloperRestoreService {
        &self.components.developer_restores
    }

    pub fn developer_backups(
        &self,
        tenant: &mako_api::TenantScope,
        now_unix_seconds: u64,
    ) -> Vec<mako_api::DeveloperBackupView> {
        self.components
            .developer_backups
            .developer_backups(tenant, now_unix_seconds)
    }

    #[must_use]
    pub fn provisioner(&self) -> &Provisioner {
        &self.components.provisioning
    }

    /// Advance every provisioning workflow that is still queued or running.
    ///
    /// Project and environment creation enqueue a workflow and report an
    /// asynchronous state. Without something executing them the resources stay
    /// in `provisioning` forever and never expose a usable data plane, so this
    /// runs one pass and reports how many workflows it advanced. A workflow that
    /// fails does not stop the pass: it keeps its own diagnostics and is retried
    /// on the next one.
    ///
    /// Once a minute the pass also looks for records that are stranded: still
    /// `provisioning` after the grace period with no workflow at all -- the
    /// creation request died between writing the record and enqueueing -- or
    /// with a workflow that failed for a reason worth retrying. It makes or
    /// retries the workflow, then runs it in the same pass, so a stranded
    /// resource heals without an operator.
    pub async fn run_pending_provisioning(&self, now_unix_seconds: u64) -> (usize, usize) {
        let Ok(mut workflows) = self.components.provisioning.list_all().await else {
            return (0, 0);
        };
        let mut advanced = 0;
        let mut failed = 0;
        if self.stranded_sweep_is_due(now_unix_seconds) {
            let (queued, sweep_failures) = self
                .reconcile_stranded_records(&workflows, now_unix_seconds)
                .await;
            failed += sweep_failures;
            if queued > 0 {
                match self.components.provisioning.list_all().await {
                    Ok(refreshed) => workflows = refreshed,
                    Err(_) => return (advanced, failed + 1),
                }
            }
        }
        for workflow in workflows {
            // A workflow that already finished still leaves its project or
            // environment in Provisioning if nothing transitioned the record,
            // so completed workflows are reconciled rather than skipped.
            let completed = match workflow.state() {
                ProvisioningState::Queued | ProvisioningState::Running => {
                    match self
                        .components
                        .provisioning
                        .run(
                            workflow.id(),
                            &self.components.provisioning_backend,
                            now_unix_seconds,
                        )
                        .await
                    {
                        Ok(completed) => completed,
                        Err(_) => {
                            failed += 1;
                            continue;
                        }
                    }
                }
                ProvisioningState::Active => workflow,
                _ => continue,
            };
            if completed.state() != ProvisioningState::Active {
                continue;
            }
            match self
                .activate_provisioned_resource(completed.resource(), now_unix_seconds)
                .await
            {
                Ok(true) => advanced += 1,
                Ok(false) => {}
                Err(()) => failed += 1,
            }
        }
        (advanced, failed)
    }

    /// Whether a minute has passed since the last stranded-record sweep,
    /// claiming this pass as the sweep when it has.
    fn stranded_sweep_is_due(&self, now_unix_seconds: u64) -> bool {
        let last = self
            .last_stranded_sweep_unix_seconds
            .load(Ordering::Acquire);
        if last != 0 && now_unix_seconds < last.saturating_add(STRANDED_SWEEP_INTERVAL_SECONDS) {
            return false;
        }
        self.last_stranded_sweep_unix_seconds
            .compare_exchange(last, now_unix_seconds, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Find every project and environment still in `provisioning`, and give
    /// each one that has no live workflow the workflow it is missing.
    ///
    /// Returns how many workflows were made or retried, and how many steps
    /// failed. Every listing is read to its end a page at a time: an active
    /// record never leaves a listing, so a first-page-only sweep would stop
    /// seeing new records once a hundred had accumulated.
    async fn reconcile_stranded_records(
        &self,
        workflows: &[ProvisioningWorkflow],
        now_unix_seconds: u64,
    ) -> (usize, usize) {
        let organizations = &self.components.organizations;
        let projects = &self.components.projects;
        let Ok(every_organization) = read_all_pages(
            |after: Option<OrganizationId>| async move {
                organizations
                    .all_organizations_after(after.as_ref(), SWEEP_PAGE)
                    .await
                    .map_err(|_| ())
            },
            |organization: &OrganizationRecord| organization.id().clone(),
        )
        .await
        else {
            return (0, 1);
        };
        let mut failed = 0;
        let mut records = Vec::new();
        for organization in every_organization {
            let organization_id = organization.id();
            let Ok(every_project) = read_all_pages(
                |after: Option<ProjectId>| async move {
                    projects
                        .list_projects_after(organization_id, after.as_ref(), SWEEP_PAGE)
                        .await
                        .map_err(|_| ())
                },
                |project: &ProjectRecord| project.id().clone(),
            )
            .await
            else {
                failed += 1;
                continue;
            };
            for project in every_project {
                let project_id = project.id();
                if project.lifecycle() == LifecycleState::Provisioning {
                    records.push((
                        ProvisioningResource::Project(project_id.clone()),
                        project.created_at_unix_seconds(),
                    ));
                }
                let Ok(every_environment) = read_all_pages(
                    |after: Option<EnvironmentId>| async move {
                        projects
                            .list_environments_after(project_id, after.as_ref(), SWEEP_PAGE)
                            .await
                            .map_err(|_| ())
                    },
                    |environment: &EnvironmentRecord| environment.id().clone(),
                )
                .await
                else {
                    failed += 1;
                    continue;
                };
                for environment in every_environment {
                    if environment.lifecycle() == LifecycleState::Provisioning {
                        records.push((
                            ProvisioningResource::Environment(TenantScope::new(
                                project_id.clone(),
                                environment.id().clone(),
                            )),
                            environment.created_at_unix_seconds(),
                        ));
                    }
                }
            }
        }
        let policy = StrandedPolicy {
            grace_seconds: STRANDED_GRACE_SECONDS,
            ..StrandedPolicy::default()
        };
        let mut queued = 0;
        for action in stranded_actions(&records, workflows, now_unix_seconds, &policy) {
            let outcome = match action {
                StrandedAction::Enqueue { id, resource } => self
                    .components
                    .provisioning
                    .enqueue(
                        id,
                        resource,
                        ProvisioningOperation::Create,
                        now_unix_seconds,
                    )
                    .await
                    .map(|_| ()),
                StrandedAction::Retry(id) => self
                    .components
                    .provisioning
                    .retry(&id, now_unix_seconds)
                    .await
                    .map(|_| ()),
            };
            match outcome {
                Ok(()) => queued += 1,
                // Another pass got there first; its workflow is the one to run.
                Err(
                    ProvisioningWorkflowError::Conflict | ProvisioningWorkflowError::NotRetryable,
                ) => {}
                Err(_) => failed += 1,
            }
        }
        (queued, failed)
    }

    /// Move the project or environment a completed workflow provisioned out of
    /// its provisioning lifecycle state.
    ///
    /// An environment is given its organization's plan limits first, so it is
    /// never active without them. When the data plane cannot take them the
    /// environment stays `provisioning` and the next pass tries again; the
    /// workflow is already complete, so nothing else is redone.
    async fn activate_provisioned_resource(
        &self,
        resource: &ProvisioningResource,
        now_unix_seconds: u64,
    ) -> Result<bool, ()> {
        match resource {
            ProvisioningResource::Project(project_id) => {
                let Ok(Some(current)) = self.components.projects.get_project(project_id).await
                else {
                    return Err(());
                };
                if current.lifecycle() != LifecycleState::Provisioning {
                    return Ok(false);
                }
                let mut next = current.clone();
                next.transition(LifecycleState::Active, now_unix_seconds, None)
                    .map_err(|_| ())?;
                self.components
                    .projects
                    .replace_project(&current, &next)
                    .await
                    .map(|()| true)
                    .map_err(|_| ())
            }
            ProvisioningResource::Environment(tenant) => {
                let Ok(Some(current)) = self
                    .components
                    .projects
                    .get_environment(tenant.project_id(), tenant.environment_id())
                    .await
                else {
                    return Err(());
                };
                if current.lifecycle() != LifecycleState::Provisioning {
                    return Ok(false);
                }
                if let Err(error) = self.install_plan_limits(tenant, now_unix_seconds).await {
                    eprintln!(
                        "provisioning activation deferred: class=plan_limits environment={} reason={error}",
                        tenant.environment_id().as_str()
                    );
                    return Err(());
                }
                let mut next = current.clone();
                next.transition(LifecycleState::Active, now_unix_seconds, None)
                    .map_err(|_| ())?;
                self.components
                    .projects
                    .replace_environment(&current, &next)
                    .await
                    .map(|()| true)
                    .map_err(|_| ())
            }
        }
    }

    /// Resolve what the environment's organization is entitled to and install
    /// the limits that follow into the data plane that serves it.
    ///
    /// The plan and any exception made to it are control-plane state; the
    /// limits they imply are what the gateway enforces. Translating here keeps
    /// that in one place, so the data plane holds what it was given rather
    /// than a second copy of the rules. Each attempt travels under its own
    /// idempotency key: the policy is recomputed every time, so a retry after
    /// a plan change installs the new limits rather than replaying the old.
    pub async fn install_plan_limits(
        &self,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<(), PlanLimitInstallError> {
        let project = self
            .components
            .projects
            .get_project(tenant.project_id())
            .await
            .map_err(|_| PlanLimitInstallError::ControlStorage)?
            .ok_or(PlanLimitInstallError::ProjectMissing)?;
        let organization = self
            .components
            .organizations
            .get_organization(project.organization_id())
            .await
            .map_err(|_| PlanLimitInstallError::ControlStorage)?
            .ok_or(PlanLimitInstallError::OrganizationMissing)?;
        let exceptions = self
            .components
            .operator_service
            .plan_exceptions(project.organization_id(), now_unix_seconds)
            .await
            .map_err(|_| PlanLimitInstallError::ControlStorage)?;
        let policy = plan_policy(organization.plan_id(), &exceptions, now_unix_seconds)?;
        let command = quota_policy_command(PROVISIONING_WORKER_ACTOR_ID, &policy);
        let attempt = format!(
            "plan-limits_{}_{now_unix_seconds}",
            tenant.environment_id().as_str()
        );
        let _: serde_json::Value = self
            .components
            .data_plane_identity_admin
            .administer(
                tenant,
                &format!("req_{attempt}"),
                &format!("idem_{attempt}"),
                &command,
            )
            .map_err(PlanLimitInstallError::DataPlane)?;
        Ok(())
    }

    #[must_use]
    pub fn provisioning_backend(&self) -> &KvProvisioningBackend {
        &self.components.provisioning_backend
    }

    #[must_use]
    pub fn operator_service(&self) -> &OperatorService {
        &self.components.operator_service
    }

    pub fn operator_control_center(&self) -> &mako_control_plane::OperatorControlCenterService {
        &self.components.operator_control_center
    }

    #[must_use]
    pub fn data_plane_identity_admin(&self) -> &ControlToDataClient {
        &self.components.data_plane_identity_admin
    }

    #[must_use]
    pub fn credential_service(&self) -> &CredentialAdminService {
        &self.components.credentials
    }

    #[must_use]
    pub fn function_service(&self) -> &FunctionAdminService {
        &self.components.functions
    }

    #[must_use]
    pub fn observability_service(&self) -> &ObservabilityService {
        &self.components.observability
    }

    #[must_use]
    pub fn function_resolution(&self) -> &FunctionResolutionService {
        &self.components.function_resolution
    }

    /// The key application sign-in client secrets are sealed under before
    /// they travel to the data plane; the data plane opens them with the key
    /// it derives from the same internal secret.
    #[must_use]
    pub fn provider_secret_key(&self) -> &ProviderSecretKey {
        &self.components.provider_secret_key
    }

    #[must_use]
    pub fn internal_authenticator(&self) -> InternalRequestAuthenticator {
        InternalRequestAuthenticator::new(
            self.components.internal_deployment_key.clone(),
            InternalCaller::EdgeGateway,
        )
    }

    #[must_use]
    pub fn operator_admin_authenticator(&self) -> InternalRequestAuthenticator {
        InternalRequestAuthenticator::new(
            self.components.internal_deployment_key.clone(),
            InternalCaller::OperatorAdmin,
        )
    }

    #[must_use]
    pub fn public_origin(&self) -> &str {
        &self.components.public_origin
    }

    pub fn internal_replay_guard(
        &self,
        tenant: &mako_api::TenantScope,
    ) -> Result<RocksInternalReplayGuard, ControlPlaneGraphError> {
        RocksInternalReplayGuard::new(Arc::clone(&self.adapter), tenant, tenant)
            .map_err(|_| ControlPlaneGraphError::Composition("internal replay guard"))
    }

    pub fn readiness(&self) -> ControlPlaneReadiness {
        let enforce = self.components.enforce_production_dependencies;
        ControlPlaneReadiness {
            storage: block_on(check_storage_readiness(
                self.adapter.as_ref(),
                Durability::Sync,
            )),
            developer_authentication: true,
            operator_authentication: true,
            rbac: true,
            audit: self.components.audit.healthy(),
            provisioning: true,
            management: true,
            data_plane_identity_admin: self
                .components
                .data_plane_identity_admin
                .dependency_ready()
                .unwrap_or(false),
            object_store: !enforce
                || self.components.object_store.dependency_ready()
                || self.components.object_store.ensure_bucket().is_ok(),
            runtime_supervisor: !enforce || self.components.runtime.dependency_ready(),
            telemetry_query: !enforce
                || block_on(self.components.observability_backend.dependency_ready()),
            function_resolution: true,
        }
    }

    pub async fn shutdown(self) -> Result<(), ControlPlaneGraphError> {
        let Self {
            storage,
            adapter,
            components,
            last_stranded_sweep_unix_seconds: _,
        } = self;
        drop(components);
        drop(adapter);
        storage.shutdown().await.map_err(Into::into)
    }
}

/// Why an environment's plan limits could not be installed. Every variant is
/// retried by the next worker pass; the environment stays `provisioning`
/// until one succeeds.
#[derive(Debug)]
pub enum PlanLimitInstallError {
    /// The project or organization record could not be read.
    ControlStorage,
    ProjectMissing,
    OrganizationMissing,
    /// The organization is on a plan the catalog no longer names -- a
    /// deployment bug, and inventing limits for it would hide that.
    PlanUnknown,
    /// The plan's entitlements do not resolve to limits the gateway can hold.
    PolicyInvalid,
    DataPlane(InternalClientError),
}

impl fmt::Display for PlanLimitInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ControlStorage => f.write_str("control storage is unavailable"),
            Self::ProjectMissing => f.write_str("project record is missing"),
            Self::OrganizationMissing => f.write_str("organization record is missing"),
            Self::PlanUnknown => f.write_str("the organization's plan is not in the catalog"),
            Self::PolicyInvalid => f.write_str("plan limits could not be resolved"),
            Self::DataPlane(error) => write!(f, "data plane refused the limits: {error}"),
        }
    }
}

impl Error for PlanLimitInstallError {}

/// Read a bounded listing to its end, one page at a time, stopping after
/// [`SWEEP_RECORD_CAP`] records so a pass stays bounded however large the
/// deployment grows. `page` is asked for the records after the id it is given.
async fn read_all_pages<T, Id, Page, Fut>(
    mut page: Page,
    id_of: impl Fn(&T) -> Id,
) -> Result<Vec<T>, ()>
where
    Page: FnMut(Option<Id>) -> Fut,
    Fut: Future<Output = Result<Vec<T>, ()>>,
{
    let mut all = Vec::new();
    let mut after = None;
    loop {
        let batch = page(after).await?;
        let last_page = batch.len() < SWEEP_PAGE.get();
        after = batch.last().map(&id_of);
        all.extend(batch);
        if last_page || all.len() >= SWEEP_RECORD_CAP {
            return Ok(all);
        }
    }
}

/// The command that installs the quota policy a tenant is held to, whoever
/// delivers it: the provisioning worker at activation, a developer
/// transferring a project, an operator changing a plan. The permission set
/// carries exactly what the operation needs and nothing else.
pub(crate) fn quota_policy_command(
    actor_id: &str,
    policy: &serde_json::Value,
) -> IdentityAdminCommand {
    IdentityAdminCommand {
        operation: IdentityAdminOperation::InstallQuotaPolicy,
        actor_id: actor_id.to_owned(),
        permissions: BTreeSet::from([IdentityAdminPermission::ManageCollections]),
        input: serde_json::json!({ "policy": policy }),
    }
}

/// The limits an organization's plan implies, as the gateway will store them.
pub fn plan_policy(
    plan_id: &str,
    exceptions: &[mako_billing::PlanException],
    now_unix_seconds: u64,
) -> Result<serde_json::Value, PlanLimitInstallError> {
    let plan = mako_billing::plan(plan_id).ok_or(PlanLimitInstallError::PlanUnknown)?;
    let plan = mako_billing::effective_plan(&plan, exceptions, now_unix_seconds);
    let policy = mako_billing::enforcement_policy(&plan.entitlements)
        .map_err(|_| PlanLimitInstallError::PolicyInvalid)?;
    serde_json::to_value(&policy).map_err(|_| PlanLimitInstallError::PolicyInvalid)
}

impl ReadinessProbe for ControlPlaneGraph {
    fn snapshot(&self) -> ReadinessSnapshot {
        self.readiness().snapshot()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlPlaneReadiness {
    pub storage: StorageReadiness,
    pub developer_authentication: bool,
    pub operator_authentication: bool,
    pub rbac: bool,
    pub audit: bool,
    pub provisioning: bool,
    pub management: bool,
    pub data_plane_identity_admin: bool,
    pub object_store: bool,
    pub runtime_supervisor: bool,
    pub telemetry_query: bool,
    pub function_resolution: bool,
}

impl ControlPlaneReadiness {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.storage.is_ready()
            && self.developer_authentication
            && self.operator_authentication
            && self.rbac
            && self.audit
            && self.provisioning
            && self.management
            && self.object_store
            && self.runtime_supervisor
            && self.telemetry_query
            && self.function_resolution
    }

    fn snapshot(&self) -> ReadinessSnapshot {
        let gating_dependencies = [
            ("storage", self.storage.is_ready()),
            ("developer_authentication", self.developer_authentication),
            ("operator_authentication", self.operator_authentication),
            ("rbac", self.rbac),
            ("audit", self.audit),
            ("provisioning", self.provisioning),
            ("management", self.management),
            ("object_store", self.object_store),
            ("runtime_supervisor", self.runtime_supervisor),
            ("telemetry_query", self.telemetry_query),
            ("function_resolution", self.function_resolution),
        ];
        let failed = gating_dependencies
            .into_iter()
            .filter_map(|(name, ready)| (!ready).then_some(name))
            .collect::<Vec<_>>();
        if failed.is_empty() {
            ReadinessSnapshot::ready(if self.data_plane_identity_admin {
                "control_ready:data_plane=available"
            } else {
                "control_ready:data_plane=unavailable"
            })
        } else {
            ReadinessSnapshot::not_ready(format!("dependencies_not_ready:{}", failed.join(",")))
        }
    }
}

impl fmt::Display for ControlPlaneReadiness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.snapshot().detail)
    }
}

fn default_quotas() -> BTreeMap<String, u64> {
    BTreeMap::from([
        ("storage_bytes".to_owned(), 10 * 1024 * 1024 * 1024),
        ("replication_requests_per_day".to_owned(), 100_000),
        (
            "replication_bytes_per_day".to_owned(),
            10 * 1024 * 1024 * 1024,
        ),
        ("function_invocations_per_day".to_owned(), 100_000),
    ])
}

fn operator_backup_evidence() -> Vec<mako_control_plane::BackupSummary> {
    std::env::var("MAKO_OPERATOR_BACKUP_EVIDENCE_JSON")
        .ok()
        .and_then(|value| {
            serde_json::from_str::<Vec<mako_control_plane::BackupSummary>>(&value).ok()
        })
        .filter(|records| records.len() <= 100)
        .unwrap_or_default()
}

fn operator_control_center_config() -> mako_control_plane::OperatorControlCenterConfig {
    let enabled = |name: &str| {
        std::env::var(name)
            .ok()
            .is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1")
    };
    let allowed_diagnostic_origins = std::env::var("MAKO_OPERATOR_DIAGNOSTIC_ORIGINS")
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|origin| {
                    origin.strip_prefix("https://").is_some_and(|authority| {
                        !authority.is_empty()
                            && !authority.contains(['/', '?', '#', '@'])
                            && !authority.contains("..")
                    })
                })
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();
    mako_control_plane::OperatorControlCenterConfig {
        recovery_creation_enabled: enabled("MAKO_OPERATOR_RECOVERY_CREATE_ENABLED"),
        recovery_promotion_enabled: enabled("MAKO_OPERATOR_RECOVERY_PROMOTE_ENABLED"),
        allowed_diagnostic_origins,
    }
}

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("control-plane limits are positive constants")
}

#[derive(Debug)]
pub enum ControlPlaneGraphError {
    WrongService,
    MissingKeyMaterial,
    InvalidKeyMaterial,
    StorageNotReady,
    Storage(StorageError),
    Identity,
    FunctionResolution(crate::FunctionResolutionError),
    Composition(&'static str),
}

impl fmt::Display for ControlPlaneGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongService => {
                formatter.write_str("control-plane graph received another service configuration")
            }
            Self::MissingKeyMaterial => {
                formatter.write_str("control-plane protected key material is unavailable")
            }
            Self::InvalidKeyMaterial => {
                formatter.write_str("control-plane protected key material is invalid")
            }
            Self::StorageNotReady => {
                formatter.write_str("control-plane storage dependencies are not ready")
            }
            Self::Storage(_) => formatter.write_str("control-plane storage could not be opened"),
            Self::Identity => formatter.write_str("control-plane identity could not be composed"),
            Self::FunctionResolution(_) => {
                formatter.write_str("control-plane function resolution could not be composed")
            }
            Self::Composition(component) => {
                write!(formatter, "control-plane {component} could not be composed")
            }
        }
    }
}

impl Error for ControlPlaneGraphError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::FunctionResolution(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StorageError> for ControlPlaneGraphError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<crate::FunctionResolutionError> for ControlPlaneGraphError {
    fn from(error: crate::FunctionResolutionError) -> Self {
        Self::FunctionResolution(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::{Ipv4Addr, TcpListener},
        path::Path,
        thread,
    };

    use mako_config::ConfigLoader;
    use mako_control_plane::{
        ControlKeyspace, DeveloperAccount, DeveloperIdentityId, DeveloperIdentityStatus,
        OperatorEntitlementRecord, OperatorPermission,
    };
    use mako_identity::{Argon2idParameters, NormalizedEmail, PasswordPolicy, PasswordService};
    use mako_service_runtime::{HttpMethod, HttpRequest};
    use mako_storage::{SqliteAdapter, SqliteConfig, WriteBatch};
    use tempfile::{Builder, TempDir};

    use super::*;

    #[test]
    fn local_graph_composes_all_dependencies_and_probes_data_plane() {
        let directory = local_tempdir("control-plane-local");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("ready listener");
        let endpoint = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("ready request");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 512];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).expect("request bytes");
                assert!(read > 0, "request ended before its headers");
                request.extend_from_slice(&chunk[..read]);
            }
            assert!(String::from_utf8_lossy(&request).starts_with("GET /readyz HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nx-mako-request-id: req_readiness\r\nconnection: close\r\n\r\n",
                )
                .expect("ready response");
        });

        let graph = Arc::new(
            ControlPlaneGraph::open_with_data_plane_endpoint(&config, endpoint)
                .expect("control-plane graph"),
        );
        assert_eq!(graph.storage_mode(), ControlPlaneStorageMode::Sqlite);
        let router = crate::control_plane_router(Arc::clone(&graph)).expect("management routes");
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/_internal/v1/control/functions/resolve"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/_internal/v1/control/operator-entitlements/plan"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/_internal/v1/control/operator-entitlements/apply"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/v1/projects/prj_example00/environments/env_example00/functions"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/operator/projects/prj_example00"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/operator/overview"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/operator/tenants"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/operator/support-sessions/current"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/v1/developer-auth/registrations"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/developer-auth/wait-list-status"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/v1/operator-auth/sessions"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/operator-auth/sessions/current"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Delete,
            "/v1/operator-auth/sessions/current"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/v1/operator-auth/sessions/current/actions/verify-password"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/v1/operator/developer-waitlist/dev_example00/actions/approve"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Post,
            "/v1/projects/prj_example00/environments/env_example00/storage-buckets"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Patch,
            "/v1/projects/prj_example00/environments/env_example00/storage-buckets/avatars"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/projects/prj_example00/environments/env_example00/storage-buckets/avatars/objects"
        ));
        assert!(router.permits(
            mako_service_runtime::HttpMethod::Delete,
            "/v1/projects/prj_example00/environments/env_example00/storage-buckets/avatars/objects/users/42/me.png"
        ));
        assert!(router.permits(mako_service_runtime::HttpMethod::Get, "/metrics"));
        assert!(!router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/v1/developer-auth/private-identities"
        ));
        assert!(!router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/_internal/v1/control/functions/resolve"
        ));
        assert!(!router.permits(
            mako_service_runtime::HttpMethod::Get,
            "/_internal/v1/unknown"
        ));
        drop(router);
        assert!(graph.readiness().is_ready());
        server.join().expect("ready server");
        let _ = graph.developer_authenticator();
        let _ = graph.operator_authenticator();
        let _ = graph.management_authorizer();
        let _ = graph.data_plane_identity_admin();
        let _ = graph.function_resolution();
        let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
        block_on(graph.shutdown()).expect("shutdown");
    }

    #[test]
    fn control_readiness_and_control_state_survive_data_plane_outage() {
        let directory = local_tempdir("control-plane-data-outage");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("unused listener");
        let endpoint = unavailable.local_addr().expect("unused endpoint");
        drop(unavailable);

        let graph = ControlPlaneGraph::open_with_data_plane_endpoint(&config, endpoint)
            .expect("control-plane graph");
        let readiness = graph.readiness();
        assert!(!readiness.data_plane_identity_admin);
        assert!(readiness.is_ready());
        assert_eq!(
            readiness.snapshot().detail,
            "control_ready:data_plane=unavailable"
        );

        let mut batch = WriteBatch::new();
        batch.put(b"\x01control-outage-test", b"available");
        block_on(graph.adapter.write(batch, Durability::Sync)).expect("control mutation");
        assert_eq!(
            block_on(graph.adapter.get(b"\x01control-outage-test")).expect("control read"),
            Some(b"available".to_vec())
        );
        block_on(graph.shutdown()).expect("shutdown");
    }

    /// An environment whose creation request died between writing the record
    /// and enqueueing its workflow -- the beta's `development` environment sat
    /// in `provisioning` for a week that way while a checkpoint backup had the
    /// data plane stopped (finding #49) -- is found by the worker's sweep once
    /// the grace period has passed, given a workflow under a deterministic id,
    /// and run in the same pass. A project heals the same way and activates at
    /// once; an environment also needs its plan limits installed, so with the
    /// data plane away it stays `provisioning`, the pass counts the deferral,
    /// and it is retried rather than activated without limits.
    #[test]
    fn the_worker_gives_stranded_records_a_workflow_and_never_activates_without_limits() {
        use mako_api::{EnvironmentId, ProjectId};
        use mako_control_plane::{
            EnvironmentRecord, MembershipRecord, OrganizationId, OrganizationRecord,
            OrganizationRole, ProjectRecord,
        };
        use mako_provisioning::reconciliation_workflow_id;

        let directory = local_tempdir("control-plane-stranded");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("unused listener");
        let endpoint = unavailable.local_addr().expect("unused endpoint");
        drop(unavailable);
        let graph = ControlPlaneGraph::open_with_data_plane_endpoint(&config, endpoint)
            .expect("control-plane graph");

        let created_at = 1_000;
        let organization_id = OrganizationId::parse("org_stranded00").expect("organization id");
        let owner = DeveloperIdentityId::parse("dev_owner000").expect("owner id");
        let organization = OrganizationRecord::new(organization_id.clone(), "Stranded", created_at)
            .expect("organization");
        let membership = MembershipRecord::new(
            organization_id.clone(),
            owner,
            OrganizationRole::Owner,
            created_at,
        );
        block_on(
            graph
                .organization_store()
                .create_organization(&organization, &membership),
        )
        .expect("organization stored");
        let project_id = ProjectId::parse("prj_stranded00").expect("project id");
        let project = ProjectRecord::new(
            project_id.clone(),
            organization_id,
            "Stranded",
            "us-east-1-beta",
            created_at,
        )
        .expect("project");
        block_on(graph.project_store().create_project(&project)).expect("project stored");
        let environment_id = EnvironmentId::parse("env_stranded00").expect("environment id");
        let environment = EnvironmentRecord::new(
            environment_id.clone(),
            project_id.clone(),
            "development",
            created_at,
        )
        .expect("environment");
        block_on(graph.project_store().create_environment(&environment))
            .expect("environment stored");
        let project_resource = ProvisioningResource::Project(project_id.clone());
        let environment_resource = ProvisioningResource::Environment(TenantScope::new(
            project_id.clone(),
            environment_id.clone(),
        ));

        // Inside the grace period the records are a creation request still in
        // flight, and nothing touches them.
        let (advanced, failed) =
            block_on(graph.run_pending_provisioning(created_at + STRANDED_GRACE_SECONDS - 1));
        assert_eq!((advanced, failed), (0, 0));
        assert!(
            block_on(
                graph
                    .provisioner()
                    .list(NonZeroUsize::new(10).expect("limit"))
            )
            .expect("workflows")
            .is_empty()
        );

        // The next sweep is a minute later: both records are past the grace
        // period, both get a workflow, both workflows run. The project
        // activates; the environment's plan limits cannot be installed
        // because the data plane is away, so it waits and the pass says so.
        let sweep_at = created_at + STRANDED_GRACE_SECONDS + STRANDED_SWEEP_INTERVAL_SECONDS;
        let (advanced, failed) = block_on(graph.run_pending_provisioning(sweep_at));
        assert_eq!((advanced, failed), (1, 1));
        for resource in [&project_resource, &environment_resource] {
            let workflow = block_on(
                graph
                    .provisioner()
                    .inspect(&reconciliation_workflow_id(resource).expect("reconciliation id")),
            )
            .expect("reconciliation workflow");
            assert_eq!(workflow.state(), ProvisioningState::Active);
            assert_eq!(workflow.resource(), resource);
        }
        let project = block_on(graph.project_store().get_project(&project_id))
            .expect("project read")
            .expect("project");
        assert_eq!(project.lifecycle(), LifecycleState::Active);
        let environment = block_on(
            graph
                .project_store()
                .get_environment(&project_id, &environment_id),
        )
        .expect("environment read")
        .expect("environment");
        assert_eq!(environment.lifecycle(), LifecycleState::Provisioning);

        // Every later pass retries the install and nothing else: the workflow
        // is complete, so no step is redone and no second workflow appears.
        let (advanced, failed) = block_on(graph.run_pending_provisioning(sweep_at + 10));
        assert_eq!((advanced, failed), (0, 1));
        assert_eq!(
            block_on(
                graph
                    .provisioner()
                    .list(NonZeroUsize::new(10).expect("limit"))
            )
            .expect("workflows")
            .len(),
            2
        );
        block_on(graph.shutdown()).expect("shutdown");
    }

    /// An object path is the one management path parameter that spans
    /// segments, so its escapes are checked where the route binds it, before
    /// The management API is never answered cross-origin. Cross-origin
    /// access is a property of an application's own API -- the data plane's
    /// and the gateway's application routes -- and no origin an environment
    /// allows for that ever reaches a management, operator, or developer
    /// workspace route: the control plane installs no cross-origin
    /// middleware at all, so there is nothing to misconfigure.
    #[test]
    fn the_management_api_never_answers_cross_origin() {
        let directory = local_tempdir("control-plane-cors");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("unused listener");
        let endpoint = unavailable.local_addr().expect("unused endpoint");
        drop(unavailable);
        let graph = Arc::new(
            ControlPlaneGraph::open_with_data_plane_endpoint(&config, endpoint)
                .expect("control-plane graph"),
        );
        let router = crate::control_plane_router(Arc::clone(&graph)).expect("router");
        let listed = "https://app.example.com";
        for (method, path) in [
            (HttpMethod::Get, "/v1/projects"),
            (
                HttpMethod::Get,
                "/v1/projects/prj_example0001/environments/env_example0001/allowed-origins",
            ),
            (HttpMethod::Get, "/v1/operator/overview"),
            (
                HttpMethod::Options,
                "/v1/projects/prj_example0001/environments/env_example0001/collections",
            ),
        ] {
            let response = router.respond_for_test(request(
                method,
                path,
                Some(listed),
                None,
                b"",
                "127.0.0.9:1000",
            ));
            for header in [
                "access-control-allow-origin",
                "access-control-allow-methods",
                "access-control-expose-headers",
                "vary",
            ] {
                assert_eq!(
                    response.header_for_test(header),
                    None,
                    "{path} answered {header}"
                );
            }
        }
    }

    /// the developer is even authenticated and long before the data plane is
    /// asked to delete anything.
    #[test]
    fn storage_object_paths_are_checked_before_the_data_plane_is_asked() {
        let directory = local_tempdir("control-plane-storage-paths");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("unused listener");
        let endpoint = unavailable.local_addr().expect("unused endpoint");
        drop(unavailable);
        let graph = Arc::new(
            ControlPlaneGraph::open_with_data_plane_endpoint(&config, endpoint)
                .expect("control-plane graph"),
        );
        let router = crate::control_plane_router(Arc::clone(&graph)).expect("router");
        let objects = "/v1/projects/prj_example0001/environments/env_example0001/storage-buckets/avatars/objects";

        // An empty segment (`users//me.png`) never gets this far: the transport
        // refuses it before routing. These reach the handler and are refused there.
        for escaping in [
            "users/../secrets.txt",
            "users/%2e%2e/me.png",
            "users/./me.png",
        ] {
            let refused = dispatch(
                &router,
                request(
                    HttpMethod::Delete,
                    &format!("{objects}/{escaping}"),
                    None,
                    None,
                    b"",
                    "127.0.0.9:1000",
                ),
            )
            .expect_err("an escaping object path is refused");
            assert_eq!(
                refused.envelope().error.code,
                mako_api::ErrorCode::InvalidRequest
            );
            assert_eq!(
                refused.envelope().error.message,
                "storage object path is invalid"
            );
        }

        let unauthenticated = dispatch(
            &router,
            request(
                HttpMethod::Delete,
                &format!("{objects}/users/42/me.png"),
                None,
                None,
                b"",
                "127.0.0.9:1000",
            ),
        )
        .expect_err("a well-formed path still needs a developer");
        assert_eq!(
            unauthenticated.envelope().error.code,
            mako_api::ErrorCode::Unauthenticated
        );

        drop(router);
        let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
        block_on(graph.shutdown()).expect("shutdown");
    }

    /// The operator console is the surface an outage is diagnosed from, so it
    /// must not depend on the database that failed. Nothing else proves the
    /// operator identity path is reachable while the data plane is gone.
    #[test]
    fn operator_authenticates_and_inspects_control_state_while_the_data_plane_is_unavailable() {
        let directory = local_tempdir("control-plane-operator-outage");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        // Bound only long enough to reserve an address, then released, so the
        // control plane is configured for a data plane that is not listening.
        let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("unused listener");
        let endpoint = unavailable.local_addr().expect("unused endpoint");
        drop(unavailable);

        let graph = Arc::new(
            ControlPlaneGraph::open_with_data_plane_endpoint(&config, endpoint)
                .expect("control-plane graph"),
        );
        let readiness = graph.readiness();
        assert!(readiness.is_ready(), "control plane must still serve");
        assert!(!readiness.data_plane_identity_admin);
        seed_password_operator(&graph);
        let router = crate::control_plane_router(Arc::clone(&graph)).expect("router");

        let signed_in = dispatch(
            &router,
            request(
                HttpMethod::Post,
                "/v1/operator-auth/sessions",
                Some("https://api.example.test"),
                None,
                br#"{"email":"operator@example.test","password":"correct horse battery staple"}"#,
                "127.0.0.9:1000",
            ),
        )
        .expect("operator sign-in must not depend on the data plane");
        assert_eq!(signed_in.status_for_test(), 200);
        let cookie = signed_in
            .header_for_test("set-cookie")
            .expect("operator cookie")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_owned();

        // Authenticated, and able to read back control-owned session state.
        assert_eq!(
            dispatch(
                &router,
                request(
                    HttpMethod::Get,
                    "/v1/operator-auth/sessions/current",
                    None,
                    Some(&cookie),
                    b"",
                    "127.0.0.9:1001",
                ),
            )
            .expect("inspect current operator session")
            .status_for_test(),
            200
        );
        assert_eq!(
            block_on(
                graph
                    .operator_password_authentication()
                    .store()
                    .health_snapshot(now_unix_seconds_for_test()),
            )
            .expect("health")
            .active_sessions,
            1
        );

        // The health evidence names the failed dependency rather than hiding it.
        assert_eq!(
            graph.readiness().snapshot().detail,
            "control_ready:data_plane=unavailable"
        );

        drop(router);
        let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
        block_on(graph.shutdown()).expect("shutdown");
    }

    /// The path a stranger takes: register, verify, wait, get reviewed, and
    /// only then reach the product.
    ///
    /// Every step here is a route a browser calls. The service-level workflow
    /// test covers the same lifecycle through `DeveloperRegistrationService`
    /// directly, and the console specs cover the pages — but between them sits
    /// the HTTP surface those pages actually depend on, and nothing drove it.
    /// Rejection in particular had no server-side coverage at all.
    #[test]
    fn hosted_registration_reaches_the_product_through_the_public_and_operator_routes() {
        const ORIGIN: &str = "https://api.example.test";
        const PASSWORD: &str = "a-sufficiently-long-applicant-password";

        let directory = local_tempdir("control-plane-hosted-registration");
        let config = registration_config_for(directory.path());
        let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("unused listener");
        let endpoint = unavailable.local_addr().expect("unused endpoint");
        drop(unavailable);
        let graph = Arc::new(
            ControlPlaneGraph::open_with_dependencies(
                &config,
                endpoint,
                Some(Arc::new(ReadyMailTransport)),
                None,
                None,
            )
            .expect("control-plane graph"),
        );
        let _ = graph.readiness();
        seed_password_operator(&graph);
        let router = crate::control_plane_router(Arc::clone(&graph)).expect("router");

        // --- A visitor creates an account. ---------------------------------

        let register = |email: &str, name: &str, remote: &str| {
            let body =
                format!(r#"{{"email":"{email}","displayName":"{name}","password":"{PASSWORD}"}}"#);
            dispatch(
                &router,
                request(
                    HttpMethod::Post,
                    "/v1/developer-auth/registrations",
                    Some(ORIGIN),
                    None,
                    body.as_bytes(),
                    remote,
                ),
            )
            .expect("registration is accepted")
        };
        assert!(
            (200..300).contains(
                &register("applicant@example.test", "New Applicant", "127.0.1.1:1000")
                    .status_for_test()
            ),
        );
        assert!(
            (200..300).contains(
                &register("rejected@example.test", "Other Applicant", "127.0.1.2:1000")
                    .status_for_test()
            ),
        );

        // --- Verification uses the token the queued mail carries. ----------

        let approved_token = verification_token(&graph, "applicant@example.test");
        let rejected_token = verification_token(&graph, "rejected@example.test");
        for (token, remote) in [
            (&approved_token, "127.0.1.1:1001"),
            (&rejected_token, "127.0.1.2:1001"),
        ] {
            let body = format!(r#"{{"token":"{token}"}}"#);
            let verified = dispatch(
                &router,
                request(
                    HttpMethod::Post,
                    "/v1/developer-auth/verifications",
                    Some(ORIGIN),
                    None,
                    body.as_bytes(),
                    remote,
                ),
            )
            .expect("verification");
            assert!((200..300).contains(&verified.status_for_test()));
        }

        // --- Verified, but wait-listed: not yet a product identity. --------

        let waitlisted = sign_in(
            &router,
            "applicant@example.test",
            PASSWORD,
            "127.0.1.1:1002",
        );
        assert_eq!(waitlisted["audience"], "mako-developer-waitlist");
        let waitlist_token = waitlisted["accessToken"]
            .as_str()
            .expect("wait-list access token")
            .to_owned();

        // Its own route answers, which is the baseline the revocation checks
        // below are measured against.
        assert_eq!(
            dispatch(
                &router,
                request_with_authorization(
                    HttpMethod::Get,
                    "/v1/developer-auth/wait-list-status",
                    &waitlist_token,
                    "127.0.1.1:1007",
                ),
            )
            .expect("a wait-list session may read its own status")
            .status_for_test(),
            200
        );

        // A wait-list session must not reach a product route.
        let refused = dispatch(
            &router,
            request_with_authorization(
                HttpMethod::Get,
                "/v1/teams",
                &waitlist_token,
                "127.0.1.1:1003",
            ),
        )
        .expect_err("a wait-list session must not reach the product");
        // The audience is wrong, so the route never gets as far as authorizing
        // against tenant state; either rejection code proves that.
        assert!(
            matches!(
                refused.envelope().error.code,
                mako_api::ErrorCode::Unauthenticated | mako_api::ErrorCode::PermissionDenied
            ),
            "a wait-list session was rejected as {:?}",
            refused.envelope().error.code
        );

        // --- The operator reviews the queue. -------------------------------

        let cookie = operator_cookie(&router, "127.0.1.9:1000");
        let page = json_body(
            &dispatch(
                &router,
                request(
                    HttpMethod::Get,
                    "/v1/operator/developer-waitlist",
                    None,
                    Some(&cookie),
                    b"",
                    "127.0.1.9:1001",
                ),
            )
            .expect("wait-list page"),
        );
        let applicants = page["applicants"].as_array().expect("applicant page");
        let listed = applicants
            .iter()
            .find(|applicant| applicant["email"] == "applicant@example.test")
            .expect("the applicant is queued for review");
        let identity = listed["developerIdentityId"]
            .as_str()
            .expect("applicant identity")
            .to_owned();
        // The page carries review metadata and nothing that could authenticate
        // as the applicant or disclose a reviewer's private note.
        for forbidden in [
            "password",
            "passwordHash",
            "token",
            "accessToken",
            "refreshToken",
            "sessionCredential",
            "reason",
            "reviewerNote",
        ] {
            assert!(
                listed.get(forbidden).is_none(),
                "the wait-list page exposes {forbidden}"
            );
        }

        // --- Approval commits once and is idempotent. ----------------------

        let approve = |remote: &str| {
            dispatch(
                &router,
                request_with_idempotency(
                    HttpMethod::Post,
                    &format!("/v1/operator/developer-waitlist/{identity}/actions/approve"),
                    ORIGIN,
                    &cookie,
                    br#"{"reason":"qualified for the beta"}"#,
                    "hosted-registration-approve",
                    remote,
                ),
            )
            .expect("approval")
        };
        let first = json_body(&approve("127.0.1.9:1002"));
        assert_eq!(first["status"], "active", "approval did not commit");
        let replayed = json_body(&approve("127.0.1.9:1003"));
        assert_eq!(
            first, replayed,
            "replaying the approval key did not return the committed result"
        );

        // The wait-list session the applicant already held is revoked. Measured
        // on the route that answered it a moment ago, so this cannot pass
        // merely because the audience was wrong all along.
        dispatch(
            &router,
            request_with_authorization(
                HttpMethod::Get,
                "/v1/developer-auth/wait-list-status",
                &waitlist_token,
                "127.0.1.1:1004",
            ),
        )
        .expect_err("the wait-list session must not survive approval");

        // --- A fresh sign-in reaches the product, joined to nothing. -------

        let active = sign_in(
            &router,
            "applicant@example.test",
            PASSWORD,
            "127.0.1.1:1005",
        );
        assert_eq!(active["audience"], "mako-management");
        let access = active["accessToken"].as_str().expect("access token");
        let organizations = json_body(
            &dispatch(
                &router,
                request_with_authorization(HttpMethod::Get, "/v1/teams", access, "127.0.1.1:1006"),
            )
            .expect("the approved developer reaches the product"),
        );
        assert_eq!(
            organizations["items"].as_array().map(Vec::len),
            Some(0),
            "an approved developer was silently joined to an existing tenant"
        );

        // --- Rejection is the other outcome, and discloses nothing. --------

        let rejected_identity = identity_for(&graph, "rejected@example.test");
        let rejected_session =
            sign_in(&router, "rejected@example.test", PASSWORD, "127.0.1.2:1002");
        let rejected_token_value = rejected_session["accessToken"]
            .as_str()
            .expect("wait-list access token")
            .to_owned();
        assert_eq!(
            dispatch(
                &router,
                request_with_authorization(
                    HttpMethod::Get,
                    "/v1/developer-auth/wait-list-status",
                    &rejected_token_value,
                    "127.0.1.2:1005",
                ),
            )
            .expect("the applicant may read its own status before a decision")
            .status_for_test(),
            200
        );
        let decision = json_body(
            &dispatch(
                &router,
                request_with_idempotency(
                    HttpMethod::Post,
                    &format!("/v1/operator/developer-waitlist/{rejected_identity}/actions/reject"),
                    ORIGIN,
                    &cookie,
                    br#"{"reason":"private reviewer note that must not leak"}"#,
                    "hosted-registration-reject",
                    "127.0.1.9:1004",
                ),
            )
            .expect("rejection"),
        );
        assert_eq!(decision["status"], "rejected");

        // Every session is gone and no product permission was granted.
        dispatch(
            &router,
            request_with_authorization(
                HttpMethod::Get,
                "/v1/developer-auth/wait-list-status",
                &rejected_token_value,
                "127.0.1.2:1003",
            ),
        )
        .expect_err("a rejected identity must hold no session");
        dispatch(
            &router,
            request_with_authorization(
                HttpMethod::Get,
                "/v1/teams",
                &rejected_token_value,
                "127.0.1.2:1006",
            ),
        )
        .expect_err("a rejected identity must hold no product permission");
        // Signing in again reports the decision without the reviewer's reason.
        let status = dispatch(
            &router,
            request(
                HttpMethod::Post,
                "/v1/developer-auth/sessions",
                Some(ORIGIN),
                None,
                format!(r#"{{"email":"rejected@example.test","password":"{PASSWORD}"}}"#)
                    .as_bytes(),
                "127.0.1.2:1004",
            ),
        );
        let disclosed = match &status {
            Ok(response) => {
                String::from_utf8_lossy(response.body_for_test().unwrap_or_default()).into_owned()
            }
            Err(error) => serde_json::to_string(error.envelope()).expect("error json"),
        };
        assert!(
            !disclosed.contains("private reviewer note"),
            "the reviewer's private reason was disclosed to the applicant: {disclosed}"
        );

        drop(router);
        let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
        block_on(graph.shutdown()).expect("shutdown");
    }

    /// Reports ready and is never asked to deliver: the queued message is read
    /// out of the outbox rather than sent, because the token is what the test
    /// needs and sending it would require a real relay.
    struct ReadyMailTransport;

    #[async_trait::async_trait]
    impl mako_control_plane::DeveloperMailTransport for ReadyMailTransport {
        async fn readiness(&self) -> Result<(), mako_control_plane::DeveloperMailTransportError> {
            Ok(())
        }

        async fn deliver(
            &self,
            _delivery_id: &str,
            _envelope: &mako_control_plane::DeveloperMailEnvelope,
        ) -> Result<(), mako_control_plane::DeveloperMailTransportError> {
            unreachable!("this test never runs the delivery worker")
        }
    }

    fn json_body(response: &mako_service_runtime::HttpResponse) -> serde_json::Value {
        serde_json::from_slice(response.body_for_test().expect("a fixed response body"))
            .expect("response is json")
    }

    fn sign_in(
        router: &mako_service_runtime::HttpRouter,
        email: &str,
        password: &str,
        remote: &str,
    ) -> serde_json::Value {
        let body = format!(r#"{{"email":"{email}","password":"{password}"}}"#);
        json_body(
            &dispatch(
                router,
                request(
                    HttpMethod::Post,
                    "/v1/developer-auth/sessions",
                    Some("https://api.example.test"),
                    None,
                    body.as_bytes(),
                    remote,
                ),
            )
            .expect("sign in"),
        )
    }

    fn operator_cookie(router: &mako_service_runtime::HttpRouter, remote: &str) -> String {
        dispatch(
            router,
            request(
                HttpMethod::Post,
                "/v1/operator-auth/sessions",
                Some("https://api.example.test"),
                None,
                br#"{"email":"operator@example.test","password":"correct horse battery staple"}"#,
                remote,
            ),
        )
        .expect("operator sign in")
        .header_for_test("set-cookie")
        .expect("operator cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
    }

    /// The verification token is never returned over HTTP — it is mailed. The
    /// queued message is read straight out of the outbox and decrypted with the
    /// configured key, which is what the delivery worker would have done.
    fn verification_token(graph: &ControlPlaneGraph, email: &str) -> String {
        let cipher = DeveloperMailCipher::new(DeveloperMailEncryptionKey::derive(
            MAIL_ENCRYPTION_SECRET.as_bytes(),
        ));
        let identity = identity_for(graph, email);
        let records = block_on(graph.developer_registration_store().pending_outbox(
            now_unix_seconds_for_test(),
            NonZeroUsize::new(100).expect("limit"),
        ))
        .expect("pending outbox");
        let record = records
            .iter()
            .rev()
            .find(|record| {
                record.kind() == mako_control_plane::DeveloperMailKind::VerifyEmail
                    && record.identity_id().as_str() == identity
            })
            .expect("a verification mail was queued");
        let envelope = cipher.decrypt(record).expect("decrypt");
        envelope
            .text_body()
            .split("#token=")
            .nth(1)
            .and_then(|value| value.lines().next())
            .expect("the verification mail carries a token")
            .to_owned()
    }

    fn identity_for(graph: &ControlPlaneGraph, email: &str) -> String {
        let normalized = NormalizedEmail::parse(email).expect("email");
        block_on(
            graph
                .developer_registration_store()
                .get_account_by_email(&normalized),
        )
        .expect("account read")
        .expect("account exists")
        .identity()
        .id()
        .as_str()
        .to_owned()
    }

    fn request_with_authorization(
        method: HttpMethod,
        path: &str,
        token: &str,
        remote: &str,
    ) -> HttpRequest {
        HttpRequest::for_test(
            method,
            path,
            vec![("authorization".to_owned(), format!("Bearer {token}"))],
            b"",
            Some(remote.parse().expect("remote address")),
        )
    }

    fn request_with_idempotency(
        method: HttpMethod,
        path: &str,
        origin: &str,
        cookie: &str,
        body: &[u8],
        idempotency_key: &str,
        remote: &str,
    ) -> HttpRequest {
        HttpRequest::for_test(
            method,
            path,
            vec![
                ("origin".to_owned(), origin.to_owned()),
                ("cookie".to_owned(), cookie.to_owned()),
                ("content-type".to_owned(), "application/json".to_owned()),
                ("idempotency-key".to_owned(), idempotency_key.to_owned()),
            ],
            body,
            Some(remote.parse().expect("remote address")),
        )
    }

    #[test]
    fn operator_password_http_boundary_enforces_origin_cookie_audience_freshness_and_signout() {
        let directory = local_tempdir("control-plane-operator-http");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("ready listener");
        let endpoint = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("ready request");
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).expect("request bytes");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nx-mako-request-id: req_readiness\r\nconnection: close\r\n\r\n",
                )
                .expect("ready response");
        });
        let graph = Arc::new(
            ControlPlaneGraph::open_with_data_plane_endpoint(&config, endpoint)
                .expect("control-plane graph"),
        );
        let _ = graph.readiness();
        seed_password_operator(&graph);
        let router = crate::control_plane_router(Arc::clone(&graph)).expect("router");

        let cross_origin = dispatch(
            &router,
            request(
                HttpMethod::Post,
                "/v1/operator-auth/sessions",
                Some("https://evil.example.test"),
                None,
                br#"{"email":"operator@example.test","password":"correct horse battery staple"}"#,
                "127.0.0.2:1000",
            ),
        )
        .expect_err("cross-origin login must fail");
        assert_eq!(
            cross_origin.envelope().error.code,
            mako_api::ErrorCode::PermissionDenied
        );
        assert_eq!(
            block_on(
                graph
                    .operator_password_authentication()
                    .store()
                    .health_snapshot(now_unix_seconds_for_test()),
            )
            .expect("health")
            .active_sessions,
            0
        );

        let unknown = dispatch(
            &router,
            request(
                HttpMethod::Post,
                "/v1/operator-auth/sessions",
                Some("https://api.example.test"),
                None,
                br#"{"email":"unknown@example.test","password":"incorrect password"}"#,
                "127.0.0.2:1001",
            ),
        )
        .expect_err("unknown identity");
        let incorrect = dispatch(
            &router,
            request(
                HttpMethod::Post,
                "/v1/operator-auth/sessions",
                Some("https://api.example.test"),
                None,
                br#"{"email":"operator@example.test","password":"incorrect password"}"#,
                "127.0.0.3:1002",
            ),
        )
        .expect_err("incorrect password");
        assert_eq!(
            unknown.envelope().error.code,
            incorrect.envelope().error.code
        );
        assert_eq!(
            unknown.envelope().error.message,
            incorrect.envelope().error.message
        );
        let repeated = dispatch(
            &router,
            request(
                HttpMethod::Post,
                "/v1/operator-auth/sessions",
                Some("https://api.example.test"),
                None,
                br#"{"email":"operator@example.test","password":"incorrect again"}"#,
                "127.0.0.3:1003",
            ),
        )
        .expect_err("repeated attempt");
        assert_eq!(
            repeated.envelope().error.code,
            mako_api::ErrorCode::Unauthenticated
        );
        let throttled = dispatch(
            &router,
            request(
                HttpMethod::Post,
                "/v1/operator-auth/sessions",
                Some("https://api.example.test"),
                None,
                br#"{"email":"operator@example.test","password":"incorrect third attempt"}"#,
                "127.0.0.3:1004",
            ),
        )
        .expect_err("attempt budget must throttle");
        assert_eq!(
            throttled.envelope().error.code,
            mako_api::ErrorCode::RateLimited
        );

        let signed_in = dispatch(
            &router,
            request(
                HttpMethod::Post,
                "/v1/operator-auth/sessions",
                Some("https://api.example.test"),
                None,
                br#"{"email":"operator@example.test","password":"correct horse battery staple"}"#,
                "127.0.0.4:1004",
            ),
        )
        .expect("sign in");
        assert_eq!(signed_in.status_for_test(), 200);
        let set_cookie = signed_in
            .header_for_test("set-cookie")
            .expect("operator cookie")
            .to_owned();
        for required in [
            "__Secure-mako_operator=",
            "Secure",
            "HttpOnly",
            "SameSite=Strict",
            "Path=/v1",
            "Max-Age=",
        ] {
            assert!(set_cookie.contains(required), "cookie omits {required}");
        }
        assert!(!set_cookie.contains("Domain="));
        let cookie = set_cookie
            .split(';')
            .next()
            .expect("cookie pair")
            .to_owned();

        assert_eq!(
            dispatch(
                &router,
                request(
                    HttpMethod::Get,
                    "/v1/operator-auth/sessions/current",
                    None,
                    Some(&cookie),
                    b"",
                    "127.0.0.4:1005",
                ),
            )
            .expect("inspect")
            .status_for_test(),
            200
        );
        let denied = dispatch(
            &router,
            request(
                HttpMethod::Get,
                "/v1/operator/projects/prj_example00",
                None,
                Some(&cookie),
                b"",
                "127.0.0.4:1006",
            ),
        )
        .expect_err("missing permission");
        assert_eq!(
            denied.envelope().error.code,
            mako_api::ErrorCode::PermissionDenied
        );

        let mutation_path = "/v1/operator/developer-waitlist/dev_applicant01/actions/approve";
        let csrf = dispatch(
            &router,
            request(
                HttpMethod::Post,
                mutation_path,
                Some("https://evil.example.test"),
                Some(&cookie),
                b"not-json",
                "127.0.0.4:1007",
            ),
        )
        .expect_err("mutation CSRF");
        assert_eq!(
            csrf.envelope().error.code,
            mako_api::ErrorCode::PermissionDenied
        );
        let credential = cookie
            .split_once('=')
            .map(|(_, value)| value)
            .expect("operator credential");
        let authenticated = block_on(
            graph
                .operator_password_authentication()
                .authenticate(Some(credential), now_unix_seconds_for_test()),
        )
        .expect("authenticated operator");
        let stale_at = authenticated
            .profile
            .password_verified_at_unix_seconds
            .checked_add(301)
            .expect("stale timestamp");
        assert!(matches!(
            graph
                .operator_password_authentication()
                .require_mutation_freshness(&authenticated, stale_at),
            Err(mako_control_plane::OperatorPasswordAuthenticationError::StepUpRequired)
        ));
        assert!(
            dispatch(
                &router,
                request(
                    HttpMethod::Post,
                    "/v1/operator-auth/sessions/current/actions/verify-password",
                    Some("https://api.example.test"),
                    Some(&cookie),
                    br#"{"password":"correct horse battery staple"}"#,
                    "127.0.0.4:1009",
                ),
            )
            .is_ok()
        );
        let parsed_after_freshness = dispatch(
            &router,
            request(
                HttpMethod::Post,
                mutation_path,
                Some("https://api.example.test"),
                Some(&cookie),
                b"not-json",
                "127.0.0.4:1010",
            ),
        )
        .expect_err("invalid body after freshness");
        assert_eq!(
            parsed_after_freshness.envelope().error.code,
            mako_api::ErrorCode::InvalidRequest
        );

        let signed_out = dispatch(
            &router,
            request(
                HttpMethod::Delete,
                "/v1/operator-auth/sessions/current",
                Some("https://api.example.test"),
                Some(&cookie),
                b"",
                "127.0.0.4:1011",
            ),
        )
        .expect("sign out");
        assert_eq!(signed_out.status_for_test(), 204);
        assert!(
            signed_out
                .header_for_test("set-cookie")
                .is_some_and(|value| value.contains("Max-Age=0"))
        );
        let after_signout = dispatch(
            &router,
            request(
                HttpMethod::Get,
                "/v1/operator-auth/sessions/current",
                None,
                Some(&cookie),
                b"",
                "127.0.0.4:1012",
            ),
        )
        .expect_err("revoked cookie");
        assert_eq!(
            after_signout.envelope().error.code,
            mako_api::ErrorCode::Unauthenticated
        );

        drop(router);
        server.join().expect("ready server");
        let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
        block_on(graph.shutdown()).expect("shutdown");
    }

    // A break-glass bearer is an incident credential, and it may read but never decide.
    //
    // Two separate rules produce that, and until now neither was asserted anywhere in
    // Rust. Hosted deployments disable bearer acceptance, and an operator API must then
    // reject the credential outright. Enabled during an incident, a mutation is still
    // refused, because every wait-list decision needs a password verification from the
    // preceding five minutes and a bearer session has none to offer. The only wire-level
    // assertions of that second refusal were browser tests against a mocked route, which
    // fabricate the very response they check, so removing the guard broke nothing.
    #[test]
    fn a_break_glass_bearer_may_never_decide_a_wait_list_application() {
        for (break_glass_enabled, expected) in [
            ("false", mako_api::ErrorCode::Unauthenticated),
            ("true", mako_api::ErrorCode::OperatorStepUpRequired),
        ] {
            let directory = local_tempdir("control-plane-break-glass-mutation");
            let config = config_with(
                directory.path(),
                DeploymentEnvironment::Local,
                vec![(
                    "MAKO_OPERATOR_BREAK_GLASS_BEARER_ENABLED",
                    break_glass_enabled,
                )],
            );
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("ready listener");
            let endpoint = listener.local_addr().expect("listener address");
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("ready request");
                let mut request = [0_u8; 512];
                let _ = stream.read(&mut request).expect("request bytes");
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nx-mako-request-id: req_readiness\r\nconnection: close\r\n\r\n",
                    )
                    .expect("ready response");
            });
            let graph = Arc::new(
                ControlPlaneGraph::open_with_data_plane_endpoint(&config, endpoint)
                    .expect("control-plane graph"),
            );
            let _ = graph.readiness();
            seed_password_operator(&graph);
            assert_eq!(
                graph.operator_break_glass_bearer_enabled(),
                break_glass_enabled == "true"
            );
            let router = crate::control_plane_router(Arc::clone(&graph)).expect("router");

            // Same origin, well-formed body, entitled operator seeded: the only thing
            // missing is the password-backed cookie.
            let refused = dispatch(
                &router,
                request(
                    HttpMethod::Post,
                    "/v1/operator/developer-waitlist/dev_applicant01/actions/approve",
                    Some("https://api.example.test"),
                    None,
                    br#"{"reason":"an incident token must not admit anybody"}"#,
                    "127.0.0.2:1000",
                ),
            )
            .expect_err("a cookieless operator mutation must be refused");
            assert_eq!(refused.envelope().error.code, expected);

            drop(router);
            server.join().expect("ready server");
            let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
            block_on(graph.shutdown()).expect("shutdown");
        }
    }

    #[test]
    fn production_refuses_an_unprovisioned_control_volume() {
        let directory = local_tempdir("control-plane-empty-production");
        let config = config_for(directory.path(), DeploymentEnvironment::Production);
        assert!(matches!(
            ControlPlaneGraph::open(&config),
            Err(ControlPlaneGraphError::Storage(_))
        ));
    }

    #[test]
    fn production_opens_only_the_control_plane_owned_volume() {
        let directory = local_tempdir("control-plane-production");
        let config = config_for(directory.path(), DeploymentEnvironment::Production);
        provision_control_sqlite(&config);
        let graph = ControlPlaneGraph::open(&config).expect("production graph");
        assert_eq!(graph.storage_mode(), ControlPlaneStorageMode::Sqlite);
        block_on(graph.shutdown()).expect("shutdown");
    }

    fn config_for(root: &Path, environment: DeploymentEnvironment) -> ServiceConfig {
        config_with(root, environment, Vec::new())
    }

    /// The hosted registration surface is deny-by-default and refuses to open
    /// without protected mail settings, so a test that drives it has to supply
    /// them. No mail is sent here — the outbox is read directly — so the relay
    /// named below is never contacted.
    const MAIL_ENCRYPTION_SECRET: &str = "a-dedicated-test-mail-encryption-secret";

    fn registration_config_for(root: &Path) -> ServiceConfig {
        config_with(
            root,
            DeploymentEnvironment::Local,
            vec![
                ("MAKO_DEVELOPER_REGISTRATION_ENABLED", "true"),
                ("MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME", "smtp.invalid"),
                ("MAKO_DEVELOPER_SMTP_PORT", "587"),
                ("MAKO_DEVELOPER_SMTP_TLS_MODE", "starttls"),
                ("MAKO_DEVELOPER_SMTP_USERNAME", "tests@smtp.invalid"),
                (
                    "MAKO_DEVELOPER_SMTP_SENDER",
                    "Mako Tests <no-reply@smtp.invalid>",
                ),
                (
                    "MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF",
                    "env:TEST_DEVELOPER_MAIL_KEY",
                ),
                ("TEST_DEVELOPER_MAIL_KEY", MAIL_ENCRYPTION_SECRET),
                (
                    "MAKO_DEVELOPER_SMTP_PASSWORD_REF",
                    "env:TEST_DEVELOPER_SMTP_PASSWORD",
                ),
                ("TEST_DEVELOPER_SMTP_PASSWORD", "a-protected-smtp-password"),
            ],
        )
    }

    fn config_with(
        root: &Path,
        environment: DeploymentEnvironment,
        extra: Vec<(&str, &str)>,
    ) -> ServiceConfig {
        let database = root.join("live/control.sqlite3");
        let lock = root.join("lock/control.lock");
        let migration = root.join("migration");
        let backup_staging = root.join("backup-staging");
        let backup_publish = root.join("backup-publish");
        let restore = root.join("restore");
        let reserve = root.join("reserve");
        let environment_name = match environment {
            DeploymentEnvironment::Local => "local",
            DeploymentEnvironment::Production => "production",
            _ => unreachable!("test uses local or production"),
        };
        let mut entries: Vec<(&str, &str)> = vec![
            ("MAKO_ENVIRONMENT", environment_name),
            ("MAKO_REGION", "us-east-1-beta"),
            ("MAKO_PUBLIC_URL", "https://api.example.test"),
            (
                "MAKO_CONTROL_SQLITE_PATH",
                database.to_str().expect("UTF-8 path"),
            ),
            (
                "MAKO_CONTROL_SQLITE_LOCK_PATH",
                lock.to_str().expect("UTF-8 path"),
            ),
            ("MAKO_CONTROL_SQLITE_IDENTITY", "mako-control-test"),
            (
                "MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE",
                migration.to_str().expect("UTF-8 path"),
            ),
            (
                "MAKO_CONTROL_SQLITE_BACKUP_STAGING",
                backup_staging.to_str().expect("UTF-8 path"),
            ),
            (
                "MAKO_CONTROL_SQLITE_BACKUP_PUBLISH",
                backup_publish.to_str().expect("UTF-8 path"),
            ),
            (
                "MAKO_CONTROL_SQLITE_RESTORE_WORKSPACE",
                restore.to_str().expect("UTF-8 path"),
            ),
            (
                "MAKO_CONTROL_SQLITE_RESERVE_PATH",
                reserve.to_str().expect("UTF-8 path"),
            ),
            ("MAKO_CONTROL_SQLITE_DISK_WARNING_FREE_BYTES", "134217728"),
            ("MAKO_CONTROL_SQLITE_DISK_CRITICAL_FREE_BYTES", "67108864"),
            (
                "MAKO_INTERNAL_AUTH_SECRET_REF",
                "env:TEST_CONTROL_PLANE_ROOT_KEY",
            ),
            (
                "TEST_CONTROL_PLANE_ROOT_KEY",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
            (
                "MAKO_OBJECT_STORE_ACCESS_KEY_REF",
                "env:TEST_OBJECT_STORE_ACCESS_KEY",
            ),
            (
                "TEST_OBJECT_STORE_ACCESS_KEY",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
            (
                "MAKO_OBJECT_STORE_SECRET_KEY_REF",
                "env:TEST_OBJECT_STORE_SECRET_KEY",
            ),
            (
                "TEST_OBJECT_STORE_SECRET_KEY",
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            ),
            ("MAKO_OPERATOR_PASSWORD_AUTH_ENABLED", "true"),
            ("MAKO_OPERATOR_MUTATION_FRESHNESS_SECONDS", "300"),
            ("MAKO_OPERATOR_BASE_BACKOFF_SECONDS", "30"),
            ("MAKO_OPERATOR_MAX_BACKOFF_SECONDS", "30"),
            ("MAKO_OPERATOR_SOURCE_RATE_LIMIT", "1"),
        ];
        entries.extend(extra);
        ConfigLoader::from_environment(entries)
            .load(ServiceKind::ControlPlane)
            .expect("configuration")
    }

    fn provision_control_sqlite(config: &ServiceConfig) {
        let settings = config
            .control_sqlite
            .as_ref()
            .expect("control SQLite settings");
        let mut sqlite =
            SqliteConfig::new(&settings.database_path, settings.database_identity.clone());
        sqlite.lock_path = settings.lock_path.clone();
        sqlite.create_if_missing = true;
        sqlite.maximum_batch_operations = settings.maximum_batch_operations;
        sqlite.maximum_scan_items = settings.maximum_scan_items;
        sqlite.busy_timeout = settings.busy_timeout;
        sqlite.transaction_expiration = settings.transaction_expiration;
        sqlite.shutdown_timeout = settings.shutdown_timeout;
        sqlite.wal_autocheckpoint_pages = settings.wal_autocheckpoint_pages;
        sqlite.maximum_wal_bytes = settings.maximum_wal_bytes;
        sqlite.disk_warning_free_bytes = settings.disk_warning_free_bytes;
        sqlite.disk_critical_free_bytes = settings.disk_critical_free_bytes;
        SqliteAdapter::open(sqlite)
            .expect("provision control SQLite")
            .shutdown()
            .expect("close provisioned control SQLite");
    }

    fn seed_password_operator(graph: &ControlPlaneGraph) {
        let now = now_unix_seconds_for_test();
        let passwords =
            PasswordService::new(PasswordPolicy::default(), Argon2idParameters::default());
        let hash = passwords
            .hash("correct horse battery staple")
            .expect("password hash");
        let identity = DeveloperIdentityId::parse("dev_operator01").expect("identity");
        let email = NormalizedEmail::parse("operator@example.test").expect("email");
        let mut account = DeveloperAccount::new_unverified(
            identity.clone(),
            email,
            "Test Operator",
            hash.encoded(),
            now - 10,
        )
        .expect("account");
        account
            .transition(DeveloperIdentityStatus::Waitlisted, now - 9)
            .expect("verify");
        let lookup = graph.components.developer_registration.lookup_key();
        let mut batch = WriteBatch::new();
        batch.put(
            ControlKeyspace::authentication_identity_key(&identity).expect("identity key"),
            serde_json::to_vec(account.identity()).expect("identity json"),
        );
        batch.put(
            ControlKeyspace::developer_role_key(&identity).expect("role key"),
            serde_json::to_vec(account.role()).expect("role json"),
        );
        batch.put(
            ControlKeyspace::developer_email_key(&lookup.email_digest(account.normalized_email()))
                .expect("email key"),
            serde_json::to_vec(&identity).expect("identity json"),
        );
        batch.put(
            ControlKeyspace::developer_status_key(
                account.status(),
                account.created_at_unix_seconds(),
                &identity,
            )
            .expect("status key"),
            serde_json::to_vec(&identity).expect("identity json"),
        );
        block_on(graph.adapter.write(batch, Durability::Sync)).expect("seed account");
        let store = graph.operator_password_authentication().store();
        let entitlement = OperatorEntitlementRecord::new(
            identity.clone(),
            store.authentication_key().stable_operator_id(&identity),
            1,
            [OperatorPermission::WaitlistReview],
            "protected HTTP integration entitlement",
            "request_http_seed",
            now - 7,
        )
        .expect("entitlement");
        block_on(store.create_entitlement(&entitlement)).expect("seed entitlement");
    }

    fn request(
        method: HttpMethod,
        path: &str,
        origin: Option<&str>,
        cookie: Option<&str>,
        body: &[u8],
        remote: &str,
    ) -> HttpRequest {
        let mut headers = Vec::new();
        if let Some(origin) = origin {
            headers.push(("origin".to_owned(), origin.to_owned()));
        }
        if let Some(cookie) = cookie {
            headers.push(("cookie".to_owned(), cookie.to_owned()));
        }
        if !body.is_empty() {
            headers.push(("content-type".to_owned(), "application/json".to_owned()));
        }
        HttpRequest::for_test(
            method,
            path,
            headers,
            body,
            Some(remote.parse().expect("remote address")),
        )
    }

    fn dispatch(
        router: &mako_service_runtime::HttpRouter,
        request: HttpRequest,
    ) -> Result<mako_service_runtime::HttpResponse, mako_service_runtime::HttpApiError> {
        router
            .dispatch_for_test(request)
            .expect("registered test route")
    }

    fn now_unix_seconds_for_test() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("system time")
            .as_secs()
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
