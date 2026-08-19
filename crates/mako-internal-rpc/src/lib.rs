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
    ControlToDataClient, EdgeToControlClient, EdgeToDataClient, InternalClientError,
    InternalHttpClient, InternalHttpClientConfig, InternalResponse,
};
pub use contract::{
    DataJobExportPageInput, DataJobExportPageOutput, DataJobImportBatchInput,
    DataJobImportBatchOutput, DataJobRowError, FunctionSecretResolutionRequest,
    FunctionSecretResolutionResponse, INTERNAL_PROTOCOL_VERSION, IdentityAdminCommand,
    IdentityAdminOperation, IdentityAdminPermission, IdentityVerificationOperation,
    IdentityVerificationRequest, IdentityVerificationResponse, InstallCollectionInput,
    InstallPolicyInput, InternalCaller, InternalRoute, MAX_INTERNAL_BODY_BYTES,
    OPERATOR_ADMIN_ENVIRONMENT_ID, OPERATOR_ADMIN_PROJECT_ID, OperatorEntitlementApplyResponse,
    OperatorEntitlementCommand, OperatorEntitlementOperation, OperatorEntitlementPlanResponse,
    ResolvedFunctionSecret,
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
