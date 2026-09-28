use std::collections::BTreeMap;
use std::{collections::BTreeSet, error::Error, fmt, str::FromStr};

use mako_service_runtime::HttpMethod;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const INTERNAL_PROTOCOL_VERSION: &str = "v1";
pub const MAX_INTERNAL_BODY_BYTES: usize = 256 * 1024;
// A one-MiB document plus canonical metadata must fit in an export page.
// The HTTP client and encrypted response journal must admit the JSON envelope.
pub const DATA_JOB_EXPORT_PAGE_BYTES: usize = 2 * 1024 * 1024;
pub const CONTROL_DATA_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

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
    /// The control plane collects application mail the data plane wrote.
    ApplicationMailDrain,
    ApplicationMailAcknowledge,
    FunctionSecretsResolve,
    /// The control plane's scheduler invokes a function through the edge
    /// gateway, so admission, metering, metrics, logs, and audit apply to a
    /// scheduled run exactly as to any other invocation.
    FunctionScheduleInvoke,
    OperatorEntitlementPlan,
    OperatorEntitlementApply,
}

impl InternalRoute {
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::IdentityAdmin => "/_internal/v1/data/identity/admin",
            Self::IdentityVerify => "/_internal/v1/data/identity/verify",
            Self::ApplicationMailDrain => "/_internal/v1/data/application-mail/drain",
            Self::ApplicationMailAcknowledge => "/_internal/v1/data/application-mail/acknowledge",
            Self::FunctionSecretsResolve => "/_internal/v1/control/functions/resolve",
            Self::FunctionScheduleInvoke => "/_internal/v1/edge/function-schedule-invoke",
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
            Self::IdentityAdmin
            | Self::ApplicationMailDrain
            | Self::ApplicationMailAcknowledge
            | Self::FunctionScheduleInvoke => InternalCaller::ControlPlane,
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
            Self::FunctionScheduleInvoke,
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
    /// The developer's current explorer epoch. The data plane is the authority:
    /// it checks every capability against it and advances it itself, so the
    /// control plane signs grants with this value rather than a local copy.
    ReadExplorerEpoch,
    ImportDataJobBatch,
    ExportDataJobPage,
    /// Install the quota policy a tenant is held to.
    ///
    /// The control plane owns plans and the exceptions made to them, and
    /// resolves both into limits. The data plane enforces what it is given, so
    /// the translation lives in one place rather than on both sides.
    InstallQuotaPolicy,
    /// Install a collection index in the data plane that serves the
    /// environment, and build it. Document queries are refused unless an
    /// active index covers them, and only the data plane holds the documents
    /// an index is built from.
    InstallIndex,
    /// Report the data plane's view of a collection index. The control plane
    /// records the definition it was asked for; the state that decides whether
    /// a query is answerable lives here.
    InspectIndex,
    /// Remove a collection index from the data plane: queries and writes stop
    /// using it, its entries and uniqueness claims are deleted, and its
    /// definition is forgotten. Deleting an index used to be recorded only in
    /// the control plane, so the data plane went on enforcing it -- a deleted
    /// unique index kept refusing duplicates.
    RemoveIndex,
    /// Install collection metadata in the data plane that serves the
    /// environment. The control plane owns the collection record, but document
    /// traffic cannot resolve a collection until its metadata exists in the
    /// data plane's own store.
    InstallCollection,
    /// Validate every stored document of a collection against the schema in
    /// the metadata given (an `InstallCollectionInput`), installing nothing.
    /// Only the data plane holds the documents, so a schema migration asks it
    /// before the control plane activates the migration's target.
    CheckStoredDocuments,
    /// Install and activate a document policy in the data plane that serves the
    /// environment. Document authorization is default-deny, so a policy that
    /// exists only in the control store leaves every read filtered out and every
    /// write denied.
    InstallPolicy,
    /// Bucket lifecycle and object administration for application file storage.
    InstallBucket,
    RemoveBucket,
    ListBuckets,
    /// External sign-in providers and magic-link settings for an environment.
    InstallAuthProviders,
    InspectAuthProviders,
    InspectBucket,
    ListBucketObjects,
    DeleteBucketObject,
    /// Read a page of a collection's committed change log as positions and
    /// revisions only -- never document fields. The control plane's webhook
    /// worker consumes it; a slow webhook endpoint must never touch the
    /// data plane's write path.
    ReadChangeFeed,
    /// Install the complete list of verified custom domains an environment
    /// is served on. The control plane verifies domains and publishes the
    /// list on every change; the data plane refuses a request that arrives
    /// on any other custom domain for that environment.
    InstallCustomDomains,
    /// Install the browser origins allowed to call an environment's
    /// application API cross-origin. The developer sets one list per
    /// environment; the data plane answers a cross-origin request only for
    /// an origin on it, on the platform hostname and on every custom domain
    /// the environment is served on alike.
    InstallAllowedOrigins,
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
    ReadBuckets,
    ManageBuckets,
    ReadChangeFeed,
}

/// The kind of change one change-feed entry describes. Mirrors the public
/// `WebhookEvent` vocabulary so the control plane needs no translation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeFeedEvent {
    Insert,
    Update,
    Delete,
}

impl ChangeFeedEvent {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// A page of a collection's change log after `after_position`, at most
/// `limit` entries (1..=500).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadChangeFeedInput {
    pub collection_id: String,
    pub after_position: u64,
    pub limit: u32,
}

/// One committed change: identifiers and revisions only. Document fields are
/// never on this wire.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ChangeFeedEntry {
    pub document_id: String,
    pub revision: String,
    pub previous_revision: Option<String>,
    pub commit_position: u64,
    pub event: ChangeFeedEvent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadChangeFeedOutput {
    pub changes: Vec<ChangeFeedEntry>,
    /// The position the page covered through; the caller's next
    /// `after_position`.
    pub scanned_through: u64,
    /// The committed high water the page was read against.
    pub high_water: u64,
    /// Nothing more was committed at or below `high_water` when the page was read.
    pub exhausted: bool,
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
/// The environment's sign-in settings as the control plane sealed them: the
/// provider set with client secrets as ciphertext under the shared key, and
/// whether magic links are enabled. The data plane validates before storing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallAuthProvidersInput {
    pub settings: Value,
}

/// One email the data plane wants sent to an application user; the control
/// plane renders it with the environment's template and delivers it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApplicationMailIntent {
    pub id: String,
    pub project_id: String,
    pub environment_id: String,
    /// `verification`, `recovery`, `invitation`, or `magic_link`.
    pub kind: String,
    pub recipient: String,
    /// Template variables, already safe to render: `link`, `expires_at`, ...
    pub variables: BTreeMap<String, String>,
    pub created_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApplicationMailDrainRequest {
    /// How long the drained intents stay invisible to the next drain.
    pub lease_seconds: u64,
    pub limit: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApplicationMailDrainResponse {
    pub intents: Vec<ApplicationMailIntent>,
}

/// Intents the control plane has durably taken; the data plane forgets them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApplicationMailAcknowledgeRequest {
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApplicationMailAcknowledgeResponse {
    pub acknowledged: u64,
}

/// Every verified hostname an environment is served on; an empty list
/// withdraws serving on custom domains for the environment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallCustomDomainsInput {
    pub hostnames: Vec<String>,
}

/// Every browser origin allowed to call one environment's application API
/// cross-origin; an empty list withdraws cross-origin access from it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallAllowedOriginsInput {
    #[serde(default)]
    pub origins: Vec<String>,
}

/// The most origins one environment allows; the public contract's bound.
pub const MAXIMUM_ALLOWED_ORIGINS: usize = 16;
/// The longest origin the public contract accepts.
pub const MAXIMUM_ORIGIN_BYTES: usize = 262;

/// Whether `value` is an origin in the exact form the platform matches a
/// browser's `Origin` header against: `http://` or `https://`, a lowercase
/// host of letters, digits, dots, and hyphens, an optional port, and
/// nothing else -- no path, trailing slash, userinfo, query, or fragment.
/// Which scheme a host may use is [`is_allowed_origin`]'s rule, not this one.
#[must_use]
pub fn is_exact_origin(value: &str) -> bool {
    if value.is_empty() || value.len() > MAXIMUM_ORIGIN_BYTES {
        return false;
    }
    let Some(authority) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if let Some(port) = port
        && (port.is_empty()
            || port.len() > 5
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || port
                .parse::<u32>()
                .is_ok_and(|port| port == 0 || port > 65_535))
    {
        return false;
    }
    !host.is_empty()
        && host.len() <= 253
        && host
            .split('.')
            .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'))
        && host.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
}

/// Whether `value` may be allowed cross-origin at all: an exact origin
/// ([`is_exact_origin`]) that is `https`, or `http` only for a loopback
/// host -- `localhost`, a name under it, or an address in `127.0.0.0/8` --
/// so a development server on the developer's own machine can be listed
/// while a plaintext origin on the internet cannot. Both planes decide with
/// this one function: the control plane when the developer sets the list,
/// the data plane when it is installed.
#[must_use]
pub fn is_allowed_origin(value: &str) -> bool {
    if !is_exact_origin(value) {
        return false;
    }
    let Some(authority) = value.strip_prefix("http://") else {
        return true;
    };
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .strip_prefix("127.")
            .is_some_and(|rest| matches!(rest.split('.').count(), 3))
            && host.split('.').all(|label| label.parse::<u8>().is_ok())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallQuotaPolicyInput {
    /// The resolved gateway policy, already translated from the plan.
    pub policy: Value,
}

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
pub struct RemoveIndexInput {
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

/// A bucket as the control plane configured it, installed into the data plane
/// verbatim; the data plane validates it again before it takes effect.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallBucketInput {
    pub bucket: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RemoveBucketInput {
    pub bucket_id: String,
    /// Delete the bucket's objects too; without it a bucket holding objects is refused.
    #[serde(default)]
    pub delete_objects: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InspectBucketInput {
    pub bucket_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ListBucketObjectsInput {
    pub bucket_id: String,
    #[serde(default)]
    pub prefix: Option<String>,
    pub limit: u64,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeleteBucketObjectInput {
    pub bucket_id: String,
    pub path: String,
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
        /// The address the token carries and whether the environment has
        /// confirmed it -- policy inputs, so the gateway that verifies a
        /// token elsewhere sees exactly what the data plane does. Empty and
        /// false when the token carries no address.
        #[serde(default)]
        email: String,
        #[serde(default)]
        email_verified: bool,
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
    /// The verified custom domains the function's environment is served
    /// on. A request that arrives on a custom domain is served only when
    /// its host is in this list. Absent from older control planes.
    #[serde(default)]
    pub custom_domains: Vec<String>,
    /// The browser origins the function's environment allows cross-origin,
    /// the same list the data plane is given. The gateway answers a
    /// preflight and labels a response only for an origin on it, on the
    /// platform hostname and on a custom domain alike. Absent from older
    /// control planes, which means no cross-origin access.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

/// One scheduled invocation the control plane asks the edge gateway to
/// perform on its behalf. The tenant travels in the signed envelope and is
/// repeated here so the gateway can refuse a body that names another.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionScheduleInvokeRequest {
    pub project_id: String,
    pub environment_id: String,
    pub function_name: String,
    pub schedule_id: String,
    pub run_id: String,
    /// `GET`, `POST`, `PUT`, `PATCH`, or `DELETE`.
    pub method: String,
    /// Path under the function, beginning with `/`, without a query string.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Request headers, names lowercased; never `authorization`.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// The request body, standard base64; empty for a body-less request.
    pub body_base64: String,
}

/// What the function answered: the status, the deployment version that
/// served, the content type, and the first bytes of the body.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionScheduleInvokeResponse {
    pub status: u16,
    pub version: u64,
    /// Only `content-type` crosses; nothing else the function set does.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// At most the first 4 KiB of the response body, standard base64.
    pub body_base64: String,
    pub duration_milliseconds: u64,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_origins_are_scheme_host_and_optional_port_only() {
        for valid in [
            "https://app.example.com",
            "https://app.example.com:8443",
            "http://localhost:5173",
            "http://127.0.0.1:5173",
            "https://a-1.b2.example",
        ] {
            assert!(is_exact_origin(valid), "{valid:?} is an exact origin");
        }
        for invalid in [
            "",
            "app.example.com",
            "ftp://app.example.com",
            "https://App.example.com",
            "https://app.example.com/",
            "https://app.example.com/path",
            "https://app.example.com?x=1",
            "https://app.example.com#top",
            "https://user@app.example.com",
            "https://app.example.com:",
            "https://app.example.com:0",
            "https://app.example.com:65536",
            "https://app.example.com:443:1",
            "https://-app.example.com",
            "https://app..example.com",
            "https://app_1.example.com",
            "https://[::1]:5173",
            "https:// app.example.com",
            &format!("https://{}.example", "a".repeat(260)),
        ] {
            assert!(!is_exact_origin(invalid), "{invalid:?} must be refused");
        }
    }

    /// Which scheme an origin may use is decided once, for both planes: a
    /// plaintext origin is allowed only on the developer's own machine.
    #[test]
    fn only_https_or_a_loopback_http_origin_may_be_allowed() {
        for allowed in [
            "https://app.example.com",
            "https://app.example.com:8443",
            "http://localhost:5173",
            "http://app.localhost",
            "http://127.0.0.1:5173",
            "http://127.10.0.2",
        ] {
            assert!(is_allowed_origin(allowed), "{allowed:?}");
        }
        for refused in [
            "http://app.example.com",
            "http://localhost.example.com",
            "http://127.0.0.1.example.com",
            "http://128.0.0.1",
            "http://127.0.0",
            "http://127.0.0.256",
            "https://app.example.com/",
        ] {
            assert!(!is_allowed_origin(refused), "{refused:?}");
        }
    }

    #[test]
    fn installs_carry_exactly_their_list() {
        let input: InstallCustomDomainsInput =
            serde_json::from_str(r#"{"hostnames":["api.example.com"]}"#).expect("hostnames");
        assert_eq!(input.hostnames, ["api.example.com"]);
        let empty: InstallAllowedOriginsInput = serde_json::from_str("{}").expect("empty");
        assert!(
            empty.origins.is_empty(),
            "an install with no origins withdraws cross-origin access"
        );
        let origins: InstallAllowedOriginsInput =
            serde_json::from_str(r#"{"origins":["https://app.example.com"]}"#).expect("origins");
        assert_eq!(origins.origins, ["https://app.example.com"]);
        assert!(
            serde_json::from_str::<InstallAllowedOriginsInput>(
                r#"{"allowedOrigins":["https://a.example"]}"#
            )
            .is_err(),
            "an unknown field is never silently an empty install"
        );
    }
}
