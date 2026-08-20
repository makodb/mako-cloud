use std::{collections::BTreeSet, error::Error, fmt, str::FromStr};

use mako_service_runtime::HttpMethod;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const INTERNAL_PROTOCOL_VERSION: &str = "v1";
pub const MAX_INTERNAL_BODY_BYTES: usize = 256 * 1024;

pub const VERSION_HEADER: &str = "x-mako-internal-version";
pub const CALLER_HEADER: &str = "x-mako-internal-caller";
pub const PROJECT_HEADER: &str = "x-mako-project-id";
pub const ENVIRONMENT_HEADER: &str = "x-mako-environment-id";
pub const TIMESTAMP_HEADER: &str = "x-mako-internal-timestamp";
pub const NONCE_HEADER: &str = "x-mako-internal-nonce";
pub const IDEMPOTENCY_HEADER: &str = "x-mako-idempotency-key";
pub const DIGEST_HEADER: &str = "x-mako-content-digest";
pub const SIGNATURE_HEADER: &str = "x-mako-internal-signature";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InternalCaller {
    ControlPlane,
    EdgeGateway,
    OperatorAdmin,
}

impl InternalCaller {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ControlPlane => "control-plane",
            Self::EdgeGateway => "edge-gateway",
            Self::OperatorAdmin => "operator-admin",
        }
    }
}

impl fmt::Display for InternalCaller {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for InternalCaller {
    type Err = InternalContractError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "control-plane" => Ok(Self::ControlPlane),
            "edge-gateway" => Ok(Self::EdgeGateway),
            "operator-admin" => Ok(Self::OperatorAdmin),
            _ => Err(InternalContractError),
        }
    }
}

/// The complete private route allowlist. Callers cannot select a target or
/// method independently of the operation they are authorized to perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InternalRoute {
    IdentityAdmin,
    IdentityVerify,
    FunctionSecretsResolve,
    OperatorEntitlementPlan,
    OperatorEntitlementApply,
}

impl InternalRoute {
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::IdentityAdmin => "/_internal/v1/data/identity/admin",
            Self::IdentityVerify => "/_internal/v1/data/identity/verify",
            Self::FunctionSecretsResolve => "/_internal/v1/control/functions/resolve",
            Self::OperatorEntitlementPlan => "/_internal/v1/control/operator-entitlements/plan",
            Self::OperatorEntitlementApply => "/_internal/v1/control/operator-entitlements/apply",
        }
    }

    #[must_use]
    pub const fn method(self) -> HttpMethod {
        HttpMethod::Post
    }

    #[must_use]
    pub const fn caller(self) -> InternalCaller {
        match self {
            Self::IdentityAdmin => InternalCaller::ControlPlane,
            Self::IdentityVerify | Self::FunctionSecretsResolve => InternalCaller::EdgeGateway,
            Self::OperatorEntitlementPlan | Self::OperatorEntitlementApply => {
                InternalCaller::OperatorAdmin
            }
        }
    }

    #[must_use]
    pub fn from_method_path(method: HttpMethod, path: &str) -> Option<Self> {
        [
            Self::IdentityAdmin,
            Self::IdentityVerify,
            Self::FunctionSecretsResolve,
            Self::OperatorEntitlementPlan,
            Self::OperatorEntitlementApply,
        ]
        .into_iter()
        .find(|route| route.method() == method && route.path() == path)
    }
}

pub const OPERATOR_ADMIN_PROJECT_ID: &str = "prj_operator_admin";
pub const OPERATOR_ADMIN_ENVIRONMENT_ID: &str = "env_operator_admin";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorEntitlementOperation {
    InitialBootstrap,
    Grant,
    Replace,
    Revoke,
    RepairBootstrapDeveloperAdmission,
}

/// Protected operator-administration input. Its custom `Debug` implementation
/// prevents the target and private reason from leaking through diagnostics.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorEntitlementCommand {
    pub operation: OperatorEntitlementOperation,
    pub target_email: String,
    pub permissions: BTreeSet<String>,
    pub private_reason: String,
    pub environment_binding: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub activate_waitlisted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_bootstrap_idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_confirmation: Option<String>,
}

impl fmt::Debug for OperatorEntitlementCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorEntitlementCommand")
            .field("operation", &self.operation)
            .field("permissions", &self.permissions)
            .field("environment_binding", &self.environment_binding)
            .field("activate_waitlisted", &self.activate_waitlisted)
            .field(
                "prior_bootstrap_idempotency_key",
                &self
                    .prior_bootstrap_idempotency_key
                    .as_ref()
                    .map(|_| "[REDACTED]"),
            )
            .field(
                "typed_confirmation",
                &self.typed_confirmation.as_ref().map(|_| "[REDACTED]"),
            )
            .field("private_fields", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorEntitlementPlanResponse {
    pub environment_binding: String,
    pub developer_identity_id: String,
    pub email_digest: String,
    pub permission_digest: String,
    pub request_digest: String,
    pub operation_digest: String,
    pub developer_status_before: Option<String>,
    pub developer_status_after: Option<String>,
    pub operator_status_before: String,
    pub operator_status_after: String,
    /// Retained for clients from the combined-lifecycle bootstrap release.
    pub lifecycle_result: String,
    pub typed_confirmation: String,
    pub replayed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorEntitlementApplyResponse {
    pub environment_binding: String,
    pub developer_identity_id: String,
    pub permission_digest: String,
    pub operation_digest: String,
    pub developer_status_before: Option<String>,
    pub developer_status_after: Option<String>,
    pub operator_status_before: String,
    pub operator_status_after: String,
    /// Retained for clients from the combined-lifecycle bootstrap release.
    pub lifecycle_result: String,
    pub operator_epoch: u64,
    pub replayed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityAdminOperation {
    SearchUsers,
    InspectUser,
    CreateUser,
    InviteUser,
    UpdateUserMetadata,
    DisableUser,
    RestoreUser,
    RevokeSession,
    RevokeAllSessions,
    DeleteUser,
    CreateProjectCredential,
    ListProjectCredentials,
    InspectProjectCredential,
    RotateProjectCredential,
    RetireProjectCredential,
    InitializeSigningKey,
    RotateSigningKey,
    ListSigningKeys,
    IssueExplorerGrant,
    RevokeExplorerGrant,
    AdvanceExplorerEpoch,
    ImportDataJobBatch,
    ExportDataJobPage,
    /// Install a collection index in the data plane that serves the
    /// environment, and build it. Document queries are refused unless an
    /// active index covers them, and only the data plane holds the documents
    /// an index is built from.
    InstallIndex,
    /// Report the data plane's view of a collection index. The control plane
    /// records the definition it was asked for; the state that decides whether
    /// a query is answerable lives here.
    InspectIndex,
    /// Install collection metadata in the data plane that serves the
    /// environment. The control plane owns the collection record, but document
    /// traffic cannot resolve a collection until its metadata exists in the
    /// data plane's own store.
    InstallCollection,
    /// Install and activate a document policy in the data plane that serves the
    /// environment. Document authorization is default-deny, so a policy that
    /// exists only in the control store leaves every read filtered out and every
    /// write denied.
    InstallPolicy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityAdminPermission {
    ReadApplicationUsers,
    ManageApplicationUsers,
    ReadProjectCredentials,
    ManageProjectCredentials,
    ReadSigningKeys,
    ManageSigningKeys,
    ManageExplorerGrants,
    ExecuteDataJobs,
    ManageCollections,
    ManagePolicies,
}

/// Collection metadata propagated to the data plane. `metadata` is the
/// document engine's own collection-metadata encoding, so the data plane
/// validates it with the same codec that reads it back rather than a parallel
/// wire schema that could drift.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallCollectionInput {
    pub collection_id: String,
    pub metadata: Value,
}

/// A document policy propagated to the data plane. `policy` is the policy
/// model's own encoding, so the data plane validates it with the same codec it
/// reads back rather than a parallel wire schema that could drift.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallIndexInput {
    pub collection_id: String,
    pub name: String,
    pub version: u64,
    pub kind: String,
    pub fields: Vec<IndexFieldInput>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IndexFieldInput {
    pub path: String,
    pub direction: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InspectIndexInput {
    pub collection_id: String,
    pub name: String,
    pub version: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallPolicyInput {
    pub collection_id: String,
    pub version: u64,
    pub policy: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobImportBatchInput {
    pub job_id: String,
    pub collection_id: String,
    pub schema_version: u64,
    pub conflict_strategy: mako_api::ImportConflictStrategy,
    pub start_row: u64,
    pub rows: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobRowError {
    pub row: u64,
    pub code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobImportBatchOutput {
    pub processed: u64,
    pub committed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub errors: Vec<DataJobRowError>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobExportPageInput {
    pub job_id: String,
    pub collection_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobExportPageOutput {
    pub rows: Vec<Value>,
    pub next_cursor: Option<String>,
    pub snapshot: String,
    pub schema_version: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IdentityAdminCommand {
    pub operation: IdentityAdminOperation,
    pub actor_id: String,
    pub permissions: BTreeSet<IdentityAdminPermission>,
    pub input: Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityVerificationOperation {
    AccessToken,
    ProjectCredential,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IdentityVerificationRequest {
    pub operation: IdentityVerificationOperation,
    pub presented_credential: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_operation: Option<String>,
}

/// Safe, typed result returned by the data-plane identity authority. Raw
/// credentials are never reflected across the private boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentityVerificationResponse {
    AccessToken {
        user_id: String,
        role: String,
        session_id: String,
        environment_authorization_epoch: u64,
        user_authorization_epoch: u64,
        trusted_claims: Map<String, Value>,
    },
    ProjectCredential {
        credential_id: String,
        service: bool,
        bypasses_document_policies: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionSecretResolutionRequest {
    pub project_ref: String,
    pub function_name: String,
    pub region: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionSecretResolutionResponse {
    pub organization_id: String,
    pub function_name: String,
    pub version: u64,
    pub bundle_digest: String,
    pub entrypoint: String,
    pub runtime_version: String,
    pub verify_jwt: bool,
    pub selected_regions: Vec<String>,
    pub request_limit_bytes: u64,
    pub response_limit_bytes: u64,
    pub secrets: Vec<ResolvedFunctionSecret>,
}

/// Exact immutable secret version delivered only on the authenticated private
/// hop. Debug output is deliberately redacted.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedFunctionSecret {
    pub name: String,
    pub version: u64,
    pub value: String,
}

impl fmt::Debug for ResolvedFunctionSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedFunctionSecret")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternalContractError;

impl fmt::Display for InternalContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("internal RPC contract value is invalid")
    }
}

impl Error for InternalContractError {}
