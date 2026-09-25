//! Multi-tenant management and operator control-plane domain.

#![forbid(unsafe_code)]

mod allowed_origins;
mod application_mail;
mod application_user;
mod automation;
mod collection;
mod credentials;
mod cron;
mod custom_domain;
mod data_job;
mod deletion;
mod developer_identity;
mod developer_registration;
mod developer_workflow;
mod dns;
mod email_template;
mod explorer;
mod function;
mod function_bundle;
mod function_schedule;
mod keyspace;
mod management_access;
mod model;
mod observability;
mod operator;
mod operator_authentication;
mod operator_control_center;
mod organization;
mod policy;
mod production_observability;
mod project;
mod retention_job;
mod runtime_backend;
mod webhook;
mod workspace;

pub use allowed_origins::{
    AllowedOriginsError, AllowedOriginsRecord, AllowedOriginsService,
    MAXIMUM_ORIGINS as MAXIMUM_ALLOWED_ORIGINS, validate_allowed_origins,
};
pub use cron::{CronError, CronField, CronSchedule};
pub use custom_domain::{
    CustomDomainCheckOutcome, CustomDomainError, CustomDomainId, CustomDomainPublishError,
    CustomDomainPublisher, CustomDomainRecord, CustomDomainService, CustomDomainState,
    CustomDomainStore, CustomDomainVerifier, CustomDomainVerifierReport,
    ERROR_DNS_UNAVAILABLE as CUSTOM_DOMAIN_ERROR_DNS_UNAVAILABLE,
    ERROR_RECORD_MISMATCH as CUSTOM_DOMAIN_ERROR_RECORD_MISMATCH,
    ERROR_RECORD_MISSING as CUSTOM_DOMAIN_ERROR_RECORD_MISSING,
    FAILURES_BEFORE_REVOCATION as CUSTOM_DOMAIN_FAILURES_BEFORE_REVOCATION,
    MAXIMUM_DOMAINS_PER_PROJECT as MAXIMUM_CUSTOM_DOMAINS_PER_PROJECT, NewCustomDomain,
    VERIFICATION_RECORD_PREFIX as CUSTOM_DOMAIN_VERIFICATION_RECORD_PREFIX,
    VERIFICATION_RECORD_TYPE as CUSTOM_DOMAIN_VERIFICATION_RECORD_TYPE,
    VERIFICATION_VALUE_PREFIX as CUSTOM_DOMAIN_VERIFICATION_VALUE_PREFIX, normalize_hostname,
    validate_hostname, verification_record_name,
};
pub use developer_identity::{
    ControlPlaneAuthenticator, DeveloperAuthenticationError, DeveloperIdentityProvider,
    DeveloperPrincipal, DeveloperSessionClaims, DeveloperSessionToken, IdentityProviderError,
};
pub use developer_registration::{
    AuthenticationIdentityRecord, AuthenticationSecurityStatus, DeveloperAccessAudience,
    DeveloperAccount, DeveloperAuthToken, DeveloperAuthTokenId, DeveloperAuthTokenPurpose,
    DeveloperAuthTokenRecord, DeveloperCleanupReport, DeveloperDecisionKind,
    DeveloperDecisionRecord, DeveloperLookupKey, DeveloperMailKind, DeveloperMailOutboxId,
    DeveloperMailOutboxRecord, DeveloperMailOutboxState, DeveloperMigrationReport,
    DeveloperRateLimitRecord, DeveloperRefreshCredential, DeveloperRefreshSessionId,
    DeveloperRefreshSessionRecord, DeveloperRegistrationError, DeveloperRegistrationHealthSnapshot,
    DeveloperRegistrationStore, DeveloperReviewPage, DeveloperRoleRecord, EncryptedDeveloperMail,
};
pub use developer_workflow::{
    DeveloperAccessClaims, DeveloperGenericOutcome, DeveloperMailCipher,
    DeveloperMailEncryptionKey, DeveloperMailEnvelope, DeveloperMailFailureKind,
    DeveloperMailOutboxWorker, DeveloperMailTransport, DeveloperMailTransportError,
    DeveloperOutboxWorkerReport, DeveloperRecoveryOutcome, DeveloperRegistrationConfig,
    DeveloperRegistrationService, DeveloperSessionGrant, DeveloperVerificationOutcome,
    DeveloperWaitlistStatus, DeveloperWorkflowError,
};
pub use dns::{
    DnsError, QUERY_ATTEMPTS as DNS_QUERY_ATTEMPTS, QUERY_TIMEOUT as DNS_QUERY_TIMEOUT,
    TxtResolver, UdpTxtResolver, decode_txt_response, encode_txt_query,
};
pub use explorer::{
    DataPlaneApplicationUsers, DataPlaneExplorerEpochs, ExplorerApplicationUsers, ExplorerEpochs,
    ExplorerGrantError, ExplorerGrantService,
};
pub use function_schedule::{
    FunctionScheduleError, FunctionScheduleId, FunctionScheduleInvokeError,
    FunctionScheduleInvoker, FunctionScheduleMethod, FunctionScheduleRecord,
    FunctionScheduleRequest, FunctionScheduleRunId, FunctionScheduleRunOutcome,
    FunctionScheduleRunPage, FunctionScheduleRunQuery, FunctionScheduleRunRecord,
    FunctionScheduleRunSummary, FunctionScheduleService, FunctionScheduleState,
    FunctionScheduleStore, FunctionScheduleUpdate, FunctionScheduleWorker,
    FunctionScheduleWorkerConfig, FunctionScheduleWorkerReport,
    INVOCATION_TIMEOUT_SECONDS as FUNCTION_SCHEDULE_INVOCATION_TIMEOUT_SECONDS,
    NewFunctionSchedule,
};
pub use keyspace::{ControlKeyspace, ControlKeyspaceError};
pub use mako_api::QuotaResource;
pub use management_access::{
    AuthorizedManagementActor, ManagementAccessError, ManagementAction, ManagementActor,
    ManagementAuthorizer,
};
pub use model::{
    ControlModelError, DeveloperIdentity, DeveloperIdentityId, DeveloperIdentityStatus,
    EnvironmentRecord, InvitationId, InvitationInput, InvitationRecord, InvitationStatus,
    LifecycleState, MembershipRecord, OrganizationId, OrganizationRecord, OrganizationRole,
    PlanChangeRecord, ProjectDataPermission, ProjectRecord, QuotaSet,
};
pub use observability::{
    EventOutcome, HealthState, ObservabilityBackend, ObservabilityBackendError, ObservabilityError,
    ObservabilityPage, ObservabilityPayload, ObservabilityQuery, ObservabilityRecord,
    ObservabilityService, ObservabilitySignal, RetentionWindow,
};
pub use operator::{
    AbuseResponseId, AbuseResponseRecord, AbuseTarget, CreditId, CreditRecord, NewAbuseResponse,
    NewQuotaOverride, NewSupportSession, OperatorAuditAction, OperatorAuditEvent,
    OperatorAuditOutcome, OperatorAuditSink, OperatorAuthenticationError, OperatorAuthenticator,
    OperatorError, OperatorId, OperatorIdentityProvider, OperatorIdentityProviderError,
    OperatorPermission, OperatorPrincipal, OperatorProjectView, OperatorRecordEvent,
    OperatorRecordState, OperatorRole, OperatorService, OperatorSessionClaims,
    OperatorSessionToken, QuotaOverrideId, QuotaOverrideRecord, SupportPermission,
    SupportSessionId, SupportSessionRecord, SupportSessionState, VerifiedSupportAccess,
};
pub use operator_authentication::{
    AuthenticatedOperatorSession, BootstrapDeveloperAdmissionRepairInput,
    BootstrapDeveloperAdmissionRepairPlan, BootstrapDeveloperAdmissionRepairResult,
    BootstrapDeveloperAdmissionRepairService, OperatorAttemptClass, OperatorAttemptRecord,
    OperatorAuthenticationAuditAction, OperatorAuthenticationAuditEvent,
    OperatorAuthenticationAuditOutcome, OperatorAuthenticationAuditSink,
    OperatorAuthenticationCleanupReport, OperatorAuthenticationConfig,
    OperatorAuthenticationHealthSnapshot, OperatorAuthenticationKey,
    OperatorAuthenticationMigrationReport, OperatorAuthenticationService,
    OperatorAuthenticationStore, OperatorAuthenticationStoreError, OperatorBootstrapInput,
    OperatorBootstrapPlan, OperatorBootstrapResult, OperatorBootstrapService,
    OperatorEntitlementAdminInput, OperatorEntitlementAdminPlan, OperatorEntitlementAdminResult,
    OperatorEntitlementAdminService, OperatorEntitlementChangeKind,
    OperatorEntitlementChangeResult, OperatorEntitlementRecord, OperatorEntitlementService,
    OperatorIdempotencyRecord, OperatorPasswordAuthenticationError, OperatorSessionCredential,
    OperatorSessionGrant, OperatorSessionProfile, OperatorSessionRecord,
};
pub use operator_control_center::{
    ActivityExportRecord, ActivityExportState, ActivityRecord, AlertSummary, BackupSummary,
    FleetSummary, Freshness, GuardedMutation, IncidentEvent, IncidentRecord, IncidentSearch,
    IncidentState, InventoryKind, OperatorBackupEvidenceProvider, OperatorControlCenterConfig,
    OperatorControlCenterError, OperatorControlCenterService, OperatorCurrentAlertProvider,
    OperatorCursorPage, OperatorDiagnosticLinkProvider, OperatorFleetHealthProvider,
    OperatorOverview, OperatorProvider, OperatorProviderError, OperatorReadSection,
    OperatorReleaseStateProvider, OperatorSecuritySummary, OperatorTelemetrySummaryProvider,
    ProjectionEvidence, RecoveryExecutor, RecoveryExecutorError, RecoveryJobRecord,
    RecoveryJobState, RecoveryRequest, SafeDiagnosticLink, Tenant360, TenantSearch, TenantSummary,
};
pub use organization::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, InvitationToken,
    InvoiceRecord, IssuedInvitation, NewInvitation, OrganizationService, OrganizationServiceError,
    OrganizationStore, OrganizationStoreError, is_read_action, team_audit_tenant,
};
pub use policy::{
    ActivePolicyView, NewPolicyDraft, PolicyAdminError, PolicyAdminService, PolicyExampleResult,
    PolicyValidationView,
};
pub use production_observability::{
    ProductionObservabilityBackend, ProductionObservabilityConfig, ProductionObservabilityError,
    TELEMETRY_AUTHORIZATION_HEADER, TELEMETRY_HEALTH_PATH, TELEMETRY_INGEST_PATH,
    TELEMETRY_PROTOCOL_VERSION, TELEMETRY_QUERY_PATH, TELEMETRY_REQUEST_ID_HEADER,
    TELEMETRY_VERSION_HEADER, TelemetryHealth, TelemetryIngestRequest, TelemetryIngestResponse,
    TelemetryQueryCredential, TelemetryQueryRequest, TelemetryQueryResponse,
};
pub use project::{
    NewEnvironment, NewProject, ProjectEnvironmentService, ProjectStore, ProjectStoreError,
};
pub use retention_job::{
    CollectionRetentionReport, CollectionRetentionTarget, RetentionJob, RetentionJobError,
    RetentionJobReport,
};
pub use runtime_backend::{
    RuntimeClientError, RuntimeDeploymentClient, RuntimeDeploymentClientConfig,
    RuntimeSupervisorCredential,
};
pub use webhook::{
    HttpWebhookTransport, NewWebhookEndpoint, WebhookChangeSource, WebhookDeliveryId,
    WebhookDeliveryPage, WebhookDeliveryQuery, WebhookDeliveryRecord, WebhookDeliveryState,
    WebhookEndpointCreated, WebhookEndpointId, WebhookEndpointRecord, WebhookEndpointState,
    WebhookEndpointUpdate, WebhookError, WebhookEvent, WebhookService, WebhookSigningSecret,
    WebhookSourceError, WebhookStore, WebhookSubscription, WebhookTransport, WebhookTransportError,
    WebhookWorker, WebhookWorkerConfig, WebhookWorkerReport, webhook_signature,
};
pub use workspace::{
    DeveloperRestoreService, DeveloperStepUpGrant, DeveloperWorkspaceError,
    DeveloperWorkspaceSecurity,
};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "control-plane";
pub use application_mail::{
    ApplicationMailConfig, ApplicationMailDepth, ApplicationMailError, ApplicationMailOutboxId,
    ApplicationMailOutboxRecord, ApplicationMailSource, ApplicationMailSourceError,
    ApplicationMailStore, ApplicationMailWorker, ApplicationMailWorkerReport, EnqueueOutcome,
};
pub use application_user::{
    ApplicationUserAccess, ApplicationUserAccessError, PolicyMetadataInvalidation,
};
pub use automation::{
    AutomationPermission, AutomationPrincipal, AutomationScope, AutomationTokenId,
    AutomationTokenIssue, AutomationTokenRecord, AutomationTokenSecret, AutomationTokenService,
    AutomationTokenStatus, ManagementResourceScope, TokenServiceError,
};
pub use collection::{
    CollectionAdminError, CollectionAdminService, CollectionCompatibilityReport, IndexBuildStatus,
    NewCollection, NewIndex, NewSchemaMigration, PublishSchema, SchemaMigrationId,
    SchemaMigrationRecord, SchemaMigrationState, SchemaPublicationOutcome,
};
pub use credentials::{
    CredentialAdminError, CredentialAdminService, FunctionSecretEncryptionKey, FunctionSecretIssue,
    FunctionSecretMetadata, FunctionSecretName, FunctionSecretResolver, FunctionSecretState,
    FunctionSecretValue, ResolvedFunctionSecret, SigningKeyView,
};
pub use data_job::{
    ArtifactMethod, DataJobError, DataJobMetricsSnapshot, DataJobService, DataJobWorkerReport,
};
pub use deletion::{
    DeletionAuditAction, DeletionAuditEvent, DeletionAuditSink, DeletionBackend,
    DeletionBackendError, DeletionError, DeletionLifecycleService, DeletionScope, DeletionWorkflow,
    DeletionWorkflowState,
};
pub use email_template::{
    EmailTemplateError, EmailTemplateKind, EmailTemplateRecord, EmailTemplateRender,
    EmailTemplateService, EmailTemplateText, EmailTemplateView, MAXIMUM_SUBJECT_BYTES,
    MAXIMUM_TEXT_BODY_BYTES, ResolvedEmailTemplate, preview_variables, render_template,
};
pub use function::{
    FunctionAdminError, FunctionAdminService, FunctionBackendError, FunctionConfiguration,
    FunctionDeploymentBackend, FunctionDeploymentResult, FunctionDeploymentSecret,
    FunctionDeploymentSpec, FunctionLimits, FunctionLogEntry, FunctionLogPage, FunctionLogQuery,
    FunctionName, FunctionRecord, FunctionSecretVersionReference, FunctionState,
    FunctionTestRequest, FunctionTestResponse, FunctionVersionRecord, FunctionVersionState,
    NewFunction, NewFunctionVersion,
};
pub use function_bundle::{
    FunctionBundleDiagnostic, FunctionBundleDiagnosticSeverity, FunctionBundleFormat,
    FunctionBundleRecord, FunctionBundleUpload, FunctionBundleUploadOutcome, FunctionSourceFile,
};
