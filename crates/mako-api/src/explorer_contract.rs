use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CollectionId, ExplorerAccessMode, ExplorerOperation, TenantScope};

pub const EXPLORER_MAX_GRANT_SECONDS: u64 = 300;
pub const EXPLORER_MAX_PAGE_DOCUMENTS: u32 = 200;
pub const EXPLORER_MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024;
pub const EXPLORER_MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;
pub const EXPLORER_MAX_PREDICATES: u32 = 16;
pub const EXPLORER_MAX_SORT_FIELDS: u32 = 4;
pub const EXPLORER_MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
pub const EXPLORER_MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
pub const EXPLORER_MAX_ACTIVE_JOBS_PER_TENANT: u32 = 4;
pub const EXPLORER_MAX_JOB_SECONDS: u64 = 60 * 60;
pub const EXPLORER_ARTIFACT_RETENTION_SECONDS: u64 = 24 * 60 * 60;
pub const EXPLORER_ARTIFACT_GRANT_SECONDS: u64 = 5 * 60;
pub const EXPLORER_MAX_SYNC_WINDOW_SECONDS: u64 = 7 * 24 * 60 * 60;
pub const EXPLORER_MAX_RESTORE_REQUESTS_PER_PROJECT: u32 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerGrantRequest {
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub mode: ExplorerAccessMode,
    pub operations: Vec<ExplorerOperation>,
    pub application_user_id: Option<String>,
    pub reason: Option<String>,
    pub duration_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerGrantView {
    pub grant_id: String,
    pub capability: String,
    pub mode: ExplorerAccessMode,
    pub operations: Vec<ExplorerOperation>,
    pub application_user_id: Option<String>,
    pub issued_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
    pub authorization_epoch: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerGrantRevocation {
    pub grant_id: String,
    pub revoked_at_unix_seconds: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerPredicateOperator {
    Equal,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerPredicate {
    pub field: String,
    pub operator: ExplorerPredicateOperator,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerSortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerSort {
    pub field: String,
    pub direction: ExplorerSortDirection,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerPageRequest {
    pub limit: u32,
    pub cursor: Option<String>,
    pub include_retained_tombstones: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerDocumentView {
    pub document_id: String,
    pub revision: String,
    pub schema_version: u64,
    pub deleted: bool,
    pub content: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerDocumentPage {
    pub items: Vec<ExplorerDocumentView>,
    pub next_cursor: Option<String>,
    pub snapshot: String,
    pub exhausted: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerQueryRequest {
    pub predicates: Vec<ExplorerPredicate>,
    pub sort: Vec<ExplorerSort>,
    pub limit: u32,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerRequiredIndex {
    pub fields: Vec<ExplorerSort>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerQueryPlan {
    pub supported: bool,
    pub index_name: Option<String>,
    pub effective_order: Vec<ExplorerSort>,
    pub effective_limit: u32,
    pub query_fingerprint: String,
    pub required_index: Option<ExplorerRequiredIndex>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerMutationKind {
    Create,
    Update,
    Delete,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerMutationRequest {
    pub kind: ExplorerMutationKind,
    pub document_id: String,
    pub expected_revision: Option<String>,
    pub schema_version: u64,
    pub idempotency_key: String,
    pub content: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerSimulationResult {
    pub allowed: bool,
    pub schema_valid: bool,
    pub diagnostics: Vec<String>,
    pub would_conflict: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerMutationResult {
    pub committed: bool,
    pub document: Option<ExplorerDocumentView>,
    pub conflict: Option<ExplorerDocumentView>,
    pub audit_reference: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerRevisionView {
    pub revision: String,
    pub schema_version: u64,
    pub commit_position: u64,
    pub committed_at_unix_seconds: Option<u64>,
    pub deleted: bool,
    pub retained_until_unix_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataJobKind {
    Import,
    Export,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataJobState {
    AwaitingUpload,
    DryRun,
    AwaitingConfirmation,
    Queued,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportConflictStrategy {
    CreateOnly,
    UpdateExisting,
    Upsert,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobCreateRequest {
    pub kind: DataJobKind,
    pub collection_id: CollectionId,
    pub conflict_strategy: Option<ImportConflictStrategy>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobDryRunRequest {
    pub upload_digest: String,
    pub schema_version: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobConfirmationRequest {
    pub expected_manifest_digest: String,
    pub acknowledge_partial_import_cancellation: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobProgress {
    pub processed: u64,
    pub committed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub exported: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobManifest {
    pub format_version: u32,
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub schema_version: u64,
    pub snapshot: Option<String>,
    pub row_count: u64,
    pub byte_count: u64,
    pub digest: String,
    pub finalized_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataJobView {
    pub job_id: String,
    pub kind: DataJobKind,
    pub state: DataJobState,
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub creator_id: String,
    pub conflict_strategy: Option<ImportConflictStrategy>,
    pub progress: DataJobProgress,
    pub errors: Vec<String>,
    pub manifest: Option<DataJobManifest>,
    pub created_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ArtifactGrantView {
    pub job_id: String,
    pub method: String,
    pub url: String,
    pub digest: Option<String>,
    pub expires_at_unix_seconds: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Current,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceSummarySection {
    pub status: ProviderStatus,
    pub observed_at_unix_seconds: u64,
    pub fresh_until_unix_seconds: u64,
    pub retained_since_unix_seconds: Option<u64>,
    pub payload: Option<Value>,
    pub remediation_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub tenant: TenantScope,
    pub sections: BTreeMap<String, WorkspaceSummarySection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceDestination {
    pub id: String,
    pub label: String,
    pub path: String,
    pub permitted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConnectMetadata {
    pub tenant: TenantScope,
    pub public_endpoint: String,
    pub public_key_id: String,
    pub public_key: String,
    pub collections: Vec<ConnectCollection>,
    pub rxdb_client_range: String,
    pub template_version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConnectCollection {
    pub collection_id: CollectionId,
    pub active_schema_version: u64,
}

/// Public-only inputs for a bounded connection check. This intentionally
/// accepts credential metadata, never a credential secret or application-user token.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConnectionCheckRequest {
    pub public_key_id: Option<String>,
    pub collection_id: Option<CollectionId>,
    pub schema_version: Option<u64>,
    pub rxdb_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionCheckState {
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConnectionCheckStep {
    pub id: String,
    pub state: ConnectionCheckState,
    pub remediation_code: Option<String>,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConnectionCheckResult {
    pub checked_at_unix_seconds: u64,
    pub steps: Vec<ConnectionCheckStep>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SyncSummary {
    pub tenant: TenantScope,
    pub collection_id: Option<CollectionId>,
    pub window_start_unix_seconds: u64,
    pub window_end_unix_seconds: u64,
    pub observed_at_unix_seconds: u64,
    pub retained_since_unix_seconds: u64,
    pub pull_count: u64,
    pub push_count: u64,
    pub live_streams: u64,
    pub lag_p95_milliseconds: u64,
    pub conflicts: u64,
    pub policy_denials: u64,
    pub throttled: u64,
    pub checkpoint_expired: u64,
    pub stream_gaps: u64,
    pub resyncs: u64,
    pub schema_mismatches: u64,
    pub client_version_classes: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperBackupView {
    pub backup_id: String,
    pub tenant: TenantScope,
    pub recovery_point_unix_seconds: u64,
    pub verified_at_unix_seconds: u64,
    pub retained_until_unix_seconds: u64,
    pub last_restore_drill_unix_seconds: Option<u64>,
    pub recovery_objective_status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperRestoreRequest {
    pub backup_id: String,
    pub target_environment_name: String,
    pub reason: String,
    pub step_up_token: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperRestoreState {
    Requested,
    Restoring,
    VerifyingIsolation,
    VerifyingStorage,
    VerifyingServices,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperRestoreView {
    pub request_id: String,
    pub backup_id: String,
    pub target: TenantScope,
    pub state: DeveloperRestoreState,
    pub accessible: bool,
    pub overwrite_permitted: bool,
    pub promotion_permitted: bool,
    pub requested_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExplorerContractError {
    InvalidLimit,
    TooManyPredicates,
    TooManySortFields,
    InvalidDuration,
}

impl ExplorerPageRequest {
    pub fn validate(&self) -> Result<(), ExplorerContractError> {
        if self.limit == 0 || self.limit > EXPLORER_MAX_PAGE_DOCUMENTS {
            return Err(ExplorerContractError::InvalidLimit);
        }
        Ok(())
    }
}

impl ExplorerQueryRequest {
    pub fn validate(&self) -> Result<(), ExplorerContractError> {
        if self.limit == 0 || self.limit > EXPLORER_MAX_PAGE_DOCUMENTS {
            return Err(ExplorerContractError::InvalidLimit);
        }
        if self.predicates.len() > EXPLORER_MAX_PREDICATES as usize {
            return Err(ExplorerContractError::TooManyPredicates);
        }
        if self.sort.len() > EXPLORER_MAX_SORT_FIELDS as usize {
            return Err(ExplorerContractError::TooManySortFields);
        }
        Ok(())
    }
}

impl ExplorerGrantRequest {
    pub fn validate(&self) -> Result<(), ExplorerContractError> {
        if self.duration_seconds == 0 || self.duration_seconds > EXPLORER_MAX_GRANT_SECONDS {
            return Err(ExplorerContractError::InvalidDuration);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvironmentId, ProjectId};

    #[test]
    fn shared_limits_fail_closed() {
        let page = ExplorerPageRequest {
            limit: EXPLORER_MAX_PAGE_DOCUMENTS + 1,
            cursor: None,
            include_retained_tombstones: false,
        };
        assert_eq!(page.validate(), Err(ExplorerContractError::InvalidLimit));
    }

    #[test]
    fn public_views_do_not_define_reusable_service_credentials() {
        let view = ConnectMetadata {
            tenant: TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            ),
            public_endpoint: "https://example.test".to_owned(),
            public_key_id: "pub_abcdefgh".to_owned(),
            public_key: "public-value".to_owned(),
            collections: Vec::new(),
            rxdb_client_range: "^1.0.0".to_owned(),
            template_version: 1,
        };
        let encoded = serde_json::to_string(&view).expect("serialize");
        assert!(!encoded.contains("serviceCredential"));
        assert!(!encoded.contains("secret"));
    }
}
