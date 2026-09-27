//! Authenticated, loopback-only service-to-service protocol for Mako Cloud.

#![forbid(unsafe_code)]

mod auth;
mod client;
mod contract;
mod guard;
mod journal;
#[cfg(test)]
mod tests;

pub use auth::{
    DeploymentKey, InternalAuthError, InternalRequestAuthenticator, SignedInternalRequest,
    VerifiedInternalRequest,
};
pub use client::{
    ControlToDataClient, ControlToEdgeClient, EdgeToControlClient, EdgeToDataClient,
    InternalClientError, InternalHttpClient, InternalHttpClientConfig, InternalResponse,
    application_mail_scope,
};
pub use contract::{
    ApplicationMailAcknowledgeRequest, ApplicationMailAcknowledgeResponse,
    ApplicationMailDrainRequest, ApplicationMailDrainResponse, ApplicationMailIntent,
    ChangeFeedEntry, ChangeFeedEvent, DataJobExportPageInput, DataJobExportPageOutput,
    DataJobImportBatchInput, DataJobImportBatchOutput, DataJobRowError, DeleteBucketObjectInput,
    FunctionScheduleInvokeRequest, FunctionScheduleInvokeResponse, FunctionSecretResolutionRequest,
    FunctionSecretResolutionResponse, INTERNAL_PROTOCOL_VERSION, IdentityAdminCommand,
    IdentityAdminOperation, IdentityAdminPermission, IdentityVerificationOperation,
    IdentityVerificationRequest, IdentityVerificationResponse, IndexFieldInput, InspectBucketInput,
    InspectIndexInput, InstallAllowedOriginsInput, InstallAuthProvidersInput, InstallBucketInput,
    InstallCollectionInput, InstallCustomDomainsInput, InstallIndexInput, InstallPolicyInput,
    InstallQuotaPolicyInput, InternalCaller, InternalRoute, ListBucketObjectsInput,
    MAX_INTERNAL_BODY_BYTES, MAXIMUM_ALLOWED_ORIGINS, MAXIMUM_ORIGIN_BYTES,
    OPERATOR_ADMIN_ENVIRONMENT_ID, OPERATOR_ADMIN_PROJECT_ID, OperatorEntitlementApplyResponse,
    OperatorEntitlementCommand, OperatorEntitlementOperation, OperatorEntitlementPlanResponse,
    ReadChangeFeedInput, ReadChangeFeedOutput, RemoveBucketInput, RemoveIndexInput,
    ResolvedFunctionSecret, is_allowed_origin, is_exact_origin,
};
pub use guard::{
    GuardDecision, InternalReplayGuard, NoopReplayGuard, RocksInternalReplayGuard,
    RocksInternalReplayGuardError,
};
pub use journal::{
    EncryptedResponseJournal, PreparedResponseJournal, ResponseJournalError, ResponseJournalLookup,
    ResponseJournalStoreOutcome,
};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "internal-rpc";
