//! Bounded, metadata-only read models and guarded workflows for the operator workspace.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    num::NonZeroUsize,
    sync::Arc,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::{ProjectId, TenantScope};
use mako_identity::NormalizedEmail;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter,
    ScanDirection, ScanRequest, StorageError, WriteBatch,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{
    ControlKeyspace, ControlKeyspaceError, DeveloperRegistrationError, DeveloperRegistrationStore,
    LifecycleState, OperatorAuditAction, OperatorAuditEvent, OperatorAuditOutcome,
    OperatorAuditSink, OperatorPermission, OperatorPrincipal, OrganizationStore,
    OrganizationStoreError, ProjectRecord, ProjectStore, ProjectStoreError,
};

const MAX_PAGE: usize = 100;
const MAX_SCAN_MULTIPLIER: usize = 8;
const MAX_TIME_WINDOW_SECONDS: u64 = 31 * 24 * 60 * 60;
const CURSOR_TTL_SECONDS: u64 = 15 * 60;
const STEP_UP_SECONDS: u64 = 5 * 60;
const MAX_RECOVERY_JOB_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Current,
    Stale,
    Unavailable,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SafeDiagnosticLink {
    pub label: String,
    pub url: String,
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorReadSection {
    pub id: String,
    pub freshness: Freshness,
    pub observed_at_unix_seconds: Option<u64>,
    pub provider: String,
    pub message: Option<String>,
    pub metrics: BTreeMap<String, Value>,
    pub links: Vec<SafeDiagnosticLink>,
}

impl OperatorReadSection {
    #[must_use]
    pub fn unavailable(id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            freshness: Freshness::Unavailable,
            observed_at_unix_seconds: None,
            provider: provider.into(),
            message: Some("The source is unavailable; no healthy state is implied.".to_owned()),
            metrics: BTreeMap::new(),
            links: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), OperatorControlCenterError> {
        if !safe_text(&self.id, 64)
            || !safe_text(&self.provider, 128)
            || self
                .message
                .as_ref()
                .is_some_and(|message| !safe_text(message, 1_024))
            || self.metrics.len() > 64
            || self.links.len() > 16
        {
            return Err(OperatorControlCenterError::UnsafeResponse);
        }
        for (name, value) in &self.metrics {
            if !safe_metric_name(name) || !safe_value(value, 0) {
                return Err(OperatorControlCenterError::UnsafeResponse);
            }
        }
        for link in &self.links {
            if !safe_text(&link.label, 128)
                || !matches!(
                    link.kind.as_str(),
                    "dashboard" | "logs" | "traces" | "runbook"
                )
                || !safe_diagnostic_url(&link.url)
            {
                return Err(OperatorControlCenterError::UnsafeResponse);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryKind {
    Operations,
    Alerts,
    Backups,
    Sync,
    Fleet,
    #[serde(rename = "rocksdb")]
    RocksDb,
    Security,
}

impl InventoryKind {
    #[must_use]
    pub const fn permission(self) -> OperatorPermission {
        match self {
            Self::Operations => OperatorPermission::OperationsRead,
            Self::Alerts => OperatorPermission::IncidentRead,
            Self::Backups => OperatorPermission::BackupRead,
            Self::Sync | Self::Fleet | Self::RocksDb => OperatorPermission::FleetRead,
            Self::Security => OperatorPermission::SecurityRead,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operations => "operations",
            Self::Alerts => "alerts",
            Self::Backups => "backups",
            Self::Sync => "sync",
            Self::Fleet => "fleet",
            Self::RocksDb => "rocksdb",
            Self::Security => "security",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperatorProviderError {
    Unavailable,
    Deadline,
    InvalidResponse,
}

impl fmt::Display for OperatorProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "operator provider unavailable",
            Self::Deadline => "operator provider deadline exceeded",
            Self::InvalidResponse => "operator provider returned an unsafe response",
        })
    }
}

impl Error for OperatorProviderError {}

#[async_trait]
pub trait OperatorProvider: Send + Sync {
    async fn overview(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError>;

    async fn tenant(
        &self,
        tenant: &TenantScope,
        kind: InventoryKind,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError>;

    async fn global(
        &self,
        kind: InventoryKind,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError>;
}

/// Narrow telemetry-summary view of an operator provider.
#[async_trait]
pub trait OperatorTelemetrySummaryProvider: Send + Sync {
    async fn telemetry_summary(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError>;
}

/// Narrow current-alert view of an operator provider.
#[async_trait]
pub trait OperatorCurrentAlertProvider: Send + Sync {
    async fn current_alert_sections(
        &self,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError>;
}

/// Narrow fleet-health view of an operator provider.
#[async_trait]
pub trait OperatorFleetHealthProvider: Send + Sync {
    async fn fleet_health_sections(
        &self,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError>;
}

/// Narrow backup-evidence view of an operator provider.
#[async_trait]
pub trait OperatorBackupEvidenceProvider: Send + Sync {
    async fn backup_evidence_sections(
        &self,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError>;
}

/// Narrow release-state view of an operator provider.
#[async_trait]
pub trait OperatorReleaseStateProvider: Send + Sync {
    async fn release_state_sections(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError>;
}

/// Narrow approved-diagnostic-link view of an operator provider.
#[async_trait]
pub trait OperatorDiagnosticLinkProvider: Send + Sync {
    async fn approved_diagnostic_links(
        &self,
        kind: InventoryKind,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<SafeDiagnosticLink>, OperatorProviderError>;
}

#[async_trait]
impl<T> OperatorTelemetrySummaryProvider for T
where
    T: OperatorProvider + Send + Sync + ?Sized,
{
    async fn telemetry_summary(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
        self.overview(now_unix_seconds).await
    }
}

macro_rules! global_provider_view {
    ($trait_name:ident, $method:ident, $kind:expr) => {
        #[async_trait]
        impl<T> $trait_name for T
        where
            T: OperatorProvider + Send + Sync + ?Sized,
        {
            async fn $method(
                &self,
                from_unix_seconds: u64,
                until_unix_seconds: u64,
            ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
                self.global($kind, from_unix_seconds, until_unix_seconds)
                    .await
            }
        }
    };
}

global_provider_view!(
    OperatorCurrentAlertProvider,
    current_alert_sections,
    InventoryKind::Alerts
);
global_provider_view!(
    OperatorFleetHealthProvider,
    fleet_health_sections,
    InventoryKind::Fleet
);
global_provider_view!(
    OperatorBackupEvidenceProvider,
    backup_evidence_sections,
    InventoryKind::Backups
);

#[async_trait]
impl<T> OperatorReleaseStateProvider for T
where
    T: OperatorProvider + Send + Sync + ?Sized,
{
    async fn release_state_sections(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
        Ok(self
            .overview(now_unix_seconds)
            .await?
            .into_iter()
            .filter(|section| section.id == "release-state")
            .collect())
    }
}

#[async_trait]
impl<T> OperatorDiagnosticLinkProvider for T
where
    T: OperatorProvider + Send + Sync + ?Sized,
{
    async fn approved_diagnostic_links(
        &self,
        kind: InventoryKind,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<SafeDiagnosticLink>, OperatorProviderError> {
        Ok(self
            .global(kind, from_unix_seconds, until_unix_seconds)
            .await?
            .into_iter()
            .flat_map(|section| section.links)
            .collect())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorCursorPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub observed_at_unix_seconds: u64,
    pub examined_records: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TenantSearch {
    pub query: Option<String>,
    pub lifecycle: Option<LifecycleState>,
    pub health: Option<Freshness>,
    pub region: Option<String>,
    pub plan: Option<String>,
    pub quota_state: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IncidentSearch {
    pub state: Option<IncidentState>,
    pub severity: Option<String>,
    pub project_id: Option<ProjectId>,
    pub from_unix_seconds: Option<u64>,
    pub until_unix_seconds: Option<u64>,
}

impl IncidentSearch {
    fn validate(&self, now_unix_seconds: u64) -> Result<(), OperatorControlCenterError> {
        if self.severity.as_ref().is_some_and(|severity| {
            !matches!(severity.as_str(), "critical" | "high" | "medium" | "low")
        }) {
            return Err(OperatorControlCenterError::InvalidInput);
        }
        if self.from_unix_seconds.is_some() || self.until_unix_seconds.is_some() {
            validate_window(
                self.from_unix_seconds.unwrap_or(0),
                self.until_unix_seconds.unwrap_or(now_unix_seconds),
                now_unix_seconds,
            )?;
        }
        Ok(())
    }

    fn digest(&self) -> Result<String, OperatorControlCenterError> {
        Ok(blake3::hash(&serde_json::to_vec(self)?)
            .to_hex()
            .to_string())
    }
}

impl TenantSearch {
    fn validate(&self) -> Result<(), OperatorControlCenterError> {
        for value in [
            self.query.as_deref(),
            self.region.as_deref(),
            self.plan.as_deref(),
            self.quota_state.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !safe_text(value, 200) {
                return Err(OperatorControlCenterError::InvalidInput);
            }
        }
        Ok(())
    }

    fn digest(&self) -> Result<String, OperatorControlCenterError> {
        Ok(blake3::hash(&serde_json::to_vec(self)?)
            .to_hex()
            .to_string())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TenantSummary {
    pub project_id: ProjectId,
    pub project_name: String,
    pub organization_id: String,
    pub organization_name: Option<String>,
    pub lifecycle: LifecycleState,
    pub region: String,
    pub environment_count: usize,
    pub health: Freshness,
    pub plan: String,
    pub updated_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Tenant360 {
    pub project: TenantSummary,
    pub environments: Vec<crate::EnvironmentRecord>,
    pub sections: BTreeMap<String, OperatorReadSection>,
    pub partial: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorOverview {
    pub sections: Vec<OperatorReadSection>,
    pub active_tenants: usize,
    pub attention_tenants: usize,
    pub observed_at_unix_seconds: u64,
    pub partial: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AlertSummary {
    pub fingerprint: String,
    pub severity: String,
    pub affected_scope: String,
    pub first_observed_at_unix_seconds: u64,
    pub last_observed_at_unix_seconds: u64,
    pub freshness: Freshness,
    pub runbook: Option<SafeDiagnosticLink>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BackupSummary {
    pub backup_id: String,
    pub project_id: ProjectId,
    pub age_seconds: u64,
    pub size_bytes: u64,
    pub integrity_verified: bool,
    pub remotely_verified: bool,
    pub protected_target: String,
    pub last_restore_drill_at_unix_seconds: Option<u64>,
    pub objective_met: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FleetSummary {
    pub instance_id: String,
    pub service: String,
    pub region: String,
    pub version: String,
    pub readiness: Freshness,
    pub restart_count: u64,
    pub certificate_expires_at_unix_seconds: Option<u64>,
    pub configuration_drift: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorSecuritySummary {
    pub active_sessions: usize,
    pub revoked_sessions: usize,
    pub entitlements: usize,
    pub recent_failures: usize,
    pub throttled_sources: usize,
    pub support_grants: usize,
    pub freshness: Freshness,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentState {
    Open,
    Acknowledged,
    Resolved,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IncidentEvent {
    pub sequence: u64,
    pub action: String,
    pub actor_id: String,
    pub note: Option<String>,
    pub at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IncidentRecord {
    pub id: String,
    pub fingerprint: String,
    pub title: String,
    pub severity: String,
    pub state: IncidentState,
    pub project_id: Option<ProjectId>,
    pub assignee: Option<String>,
    pub version: u64,
    pub opened_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
    pub timeline: Vec<IncidentEvent>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GuardedMutation {
    pub operation_key: String,
    pub reviewed_version: u64,
    pub reason: String,
    pub case_reference: Option<String>,
    pub confirmation: String,
    pub action_binding: String,
    pub password_verified_at_unix_seconds: u64,
}

impl GuardedMutation {
    pub fn action_binding(action: &str, target: &str, reviewed_version: u64) -> String {
        let digest = Sha256::digest(
            format!("operator-action-v1\0{action}\0{target}\0{reviewed_version}").as_bytes(),
        );
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn validate(
        &self,
        action: &str,
        target: &str,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorControlCenterError> {
        if !safe_text(&self.operation_key, 128)
            || !safe_text(&self.reason, 1_024)
            || self.reason.len() < 8
            || self
                .case_reference
                .as_ref()
                .is_some_and(|value| !safe_text(value, 200))
            || self.confirmation != target
            || self.action_binding != Self::action_binding(action, target, self.reviewed_version)
            || self.password_verified_at_unix_seconds > now_unix_seconds
            || now_unix_seconds - self.password_verified_at_unix_seconds > STEP_UP_SECONDS
        {
            return Err(OperatorControlCenterError::StepUpRequired);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryJobState {
    Requested,
    Preparing,
    Restoring,
    Verifying,
    PromotionReady,
    Promoted,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RecoveryRequest {
    pub id: String,
    pub project_id: ProjectId,
    pub backup_id: String,
    pub target: String,
    pub backup_verified: bool,
    pub impact_preview: String,
    pub guard: GuardedMutation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RecoveryJobRecord {
    pub id: String,
    pub project_id: ProjectId,
    pub backup_id: String,
    pub target: String,
    pub state: RecoveryJobState,
    pub version: u64,
    pub verification_succeeded: bool,
    #[serde(default = "default_true")]
    pub promotion_permitted: bool,
    pub created_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
    pub last_error_class: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryExecutorError {
    Unavailable,
    EvidenceRejected,
    RestoreFailed,
    VerificationFailed,
    PromotionFailed,
}

impl fmt::Display for RecoveryExecutorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "recovery executor is unavailable",
            Self::EvidenceRejected => "recovery evidence was rejected",
            Self::RestoreFailed => "approved restore procedure failed",
            Self::VerificationFailed => "post-restore verification failed",
            Self::PromotionFailed => "approved promotion procedure failed",
        })
    }
}

impl Error for RecoveryExecutorError {}

/// Typed wrapper around approved backup procedures. It intentionally accepts no command or path.
#[async_trait]
pub trait RecoveryExecutor: Send + Sync {
    async fn prepare(&self, job: &RecoveryJobRecord) -> Result<(), RecoveryExecutorError>;
    async fn restore(&self, job: &RecoveryJobRecord) -> Result<(), RecoveryExecutorError>;
    async fn verify(&self, job: &RecoveryJobRecord) -> Result<(), RecoveryExecutorError>;
    async fn promote(&self, job: &RecoveryJobRecord) -> Result<(), RecoveryExecutorError>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityExportState {
    Pending,
    Ready,
    Expired,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActivityExportRecord {
    pub id: String,
    pub requested_by: String,
    pub filter_digest: String,
    pub filter: Value,
    pub state: ActivityExportState,
    pub record_count: usize,
    pub checksum: Option<String>,
    pub created_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectionEvidence {
    pub name: String,
    pub source_count: usize,
    pub projected_count: usize,
    pub checksum: String,
    pub cursor: Option<String>,
    pub complete: bool,
    pub updated_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TenantSearchIndexRecord {
    schema_version: u8,
    project_id: ProjectId,
    organization_id: String,
    normalized_project_name: String,
    region: String,
    lifecycle: LifecycleState,
    source_updated_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Default)]
pub struct OperatorControlCenterConfig {
    pub recovery_creation_enabled: bool,
    pub recovery_promotion_enabled: bool,
    pub allowed_diagnostic_origins: BTreeSet<String>,
}

#[derive(Clone)]
pub struct OperatorControlCenterService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    developers: DeveloperRegistrationStore,
    provider: Arc<dyn OperatorProvider>,
    audit: Arc<dyn OperatorAuditSink>,
    cursor_key: [u8; 32],
    config: OperatorControlCenterConfig,
}

impl fmt::Debug for OperatorControlCenterService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorControlCenterService")
            .field("durability", &self.durability)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl OperatorControlCenterService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        developers: DeveloperRegistrationStore,
        provider: Arc<dyn OperatorProvider>,
        audit: Arc<dyn OperatorAuditSink>,
        cursor_key: [u8; 32],
        config: OperatorControlCenterConfig,
    ) -> Result<Self, OperatorControlCenterError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(OperatorControlCenterError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            developers,
            provider,
            audit,
            cursor_key,
            config,
        })
    }

    pub async fn overview(
        &self,
        actor: &OperatorPrincipal,
        now_unix_seconds: u64,
    ) -> Result<OperatorOverview, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::OverviewRead,
            "overview",
            now_unix_seconds,
        )?;
        let (mut sections, partial) = match self.provider.overview(now_unix_seconds).await {
            Ok(sections) => (sections, false),
            Err(_) => (
                vec![OperatorReadSection::unavailable(
                    "overview",
                    "operator-summary",
                )],
                true,
            ),
        };
        validate_sections(&mut sections, &self.config)?;
        let projects = self.scan_projects(None, MAX_PAGE + 1).await?;
        let active_tenants = projects
            .iter()
            .filter(|(_, project)| project.lifecycle() == LifecycleState::Active)
            .count();
        let attention_tenants = projects
            .iter()
            .filter(|(_, project)| {
                matches!(
                    project.lifecycle(),
                    LifecycleState::Failed | LifecycleState::Suspended | LifecycleState::Deleting
                )
            })
            .count();
        sections.push(OperatorReadSection {
            id: "tenant-lifecycle".to_owned(),
            freshness: if projects.len() > MAX_PAGE {
                Freshness::Stale
            } else {
                Freshness::Current
            },
            observed_at_unix_seconds: Some(now_unix_seconds),
            provider: "control-plane".to_owned(),
            message: (projects.len() > MAX_PAGE).then(|| {
                "Tenant lifecycle counts reached the bounded overview limit; use the tenant directory for complete pagination.".to_owned()
            }),
            metrics: BTreeMap::from([
                ("activeCount".to_owned(), Value::from(active_tenants)),
                ("attentionCount".to_owned(), Value::from(attention_tenants)),
                ("examinedCount".to_owned(), Value::from(projects.len())),
            ]),
            links: Vec::new(),
        });
        validate_sections(&mut sections, &self.config)?;
        self.allowed_global(actor, "overview", now_unix_seconds);
        Ok(OperatorOverview {
            sections,
            active_tenants,
            attention_tenants,
            observed_at_unix_seconds: now_unix_seconds,
            partial,
        })
    }

    pub async fn tenants(
        &self,
        actor: &OperatorPrincipal,
        search: &TenantSearch,
        cursor: Option<&str>,
        limit: usize,
        now_unix_seconds: u64,
    ) -> Result<OperatorCursorPage<TenantSummary>, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::TenantRead,
            "tenant_directory",
            now_unix_seconds,
        )?;
        search.validate()?;
        validate_limit(limit)?;
        let digest = search.digest()?;
        let start = cursor
            .map(|value| self.decode_cursor(value, "tenants", &digest, now_unix_seconds))
            .transpose()?;
        let scan_limit = (limit.saturating_mul(MAX_SCAN_MULTIPLIER) + 1).min(MAX_PAGE * 8 + 1);
        let records = self.scan_projects(start, scan_limit).await?;
        let mut items = Vec::with_capacity(limit);
        let query = search
            .query
            .as_ref()
            .map(|value| value.to_ascii_lowercase());
        let email_organizations = if let Some(query) = query.as_deref() {
            match NormalizedEmail::parse(query) {
                Ok(email) => match self
                    .developers
                    .get_authentication_identity_by_email(&email)
                    .await?
                {
                    Some(identity) => Some(
                        self.organizations
                            .list_organizations_for(
                                identity.id(),
                                NonZeroUsize::new(MAX_PAGE).expect("positive"),
                            )
                            .await?
                            .into_iter()
                            .map(|organization| organization.id().as_str().to_owned())
                            .collect::<BTreeSet<_>>(),
                    ),
                    None => Some(BTreeSet::new()),
                },
                Err(_) => None,
            }
        } else {
            None
        };
        let mut last_examined = None;
        for (key, project) in &records {
            last_examined = Some(key.clone());
            let organization = self
                .organizations
                .get_organization(project.organization_id())
                .await?;
            let environments = self
                .projects
                .list_environments(project.id(), NonZeroUsize::new(MAX_PAGE).expect("positive"))
                .await?;
            if !tenant_matches(
                project,
                organization.as_ref(),
                &environments,
                search,
                query.as_deref(),
                email_organizations.as_ref(),
            ) {
                continue;
            }
            items.push(tenant_summary(
                project,
                organization.as_ref(),
                environments.len(),
            ));
            if items.len() == limit {
                break;
            }
        }
        let has_more = records.len() == scan_limit || items.len() == limit;
        let next_cursor = if has_more {
            last_examined
                .as_deref()
                .map(|key| self.encode_cursor("tenants", key, &digest, now_unix_seconds))
                .transpose()?
        } else {
            None
        };
        self.allowed_global(actor, "tenant_directory", now_unix_seconds);
        Ok(OperatorCursorPage {
            items,
            next_cursor,
            observed_at_unix_seconds: now_unix_seconds,
            examined_records: records.len(),
        })
    }

    pub async fn tenant_360(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        now_unix_seconds: u64,
    ) -> Result<Tenant360, OperatorControlCenterError> {
        self.require_project(
            actor,
            OperatorPermission::TenantRead,
            project_id,
            "tenant_360",
            now_unix_seconds,
        )?;
        let project = self.project(project_id).await?;
        let organization = self
            .organizations
            .get_organization(project.organization_id())
            .await?;
        let environments = self
            .projects
            .list_environments(project_id, NonZeroUsize::new(MAX_PAGE).expect("positive"))
            .await?;
        let summary = self.tenant_summary(&project, organization.as_ref()).await?;
        let mut sections = BTreeMap::new();
        let mut partial = false;
        sections.insert(
            "lifecycle".to_owned(),
            OperatorReadSection {
                id: "lifecycle".to_owned(),
                freshness: lifecycle_freshness(project.lifecycle()),
                observed_at_unix_seconds: Some(project.updated_at_unix_seconds()),
                provider: "control-plane".to_owned(),
                message: None,
                metrics: BTreeMap::from([
                    (
                        "environmentCount".to_owned(),
                        Value::from(environments.len()),
                    ),
                    (
                        "active".to_owned(),
                        Value::from(project.lifecycle() == LifecycleState::Active),
                    ),
                ]),
                links: Vec::new(),
            },
        );
        for (section_name, kind) in [
            ("provisioning", InventoryKind::Operations),
            ("usage", InventoryKind::Operations),
            ("quota", InventoryKind::Operations),
            ("sync", InventoryKind::Sync),
            ("authentication", InventoryKind::Security),
            ("functions", InventoryKind::Fleet),
            ("backups", InventoryKind::Backups),
            ("safe-errors", InventoryKind::Operations),
        ] {
            let mut values = Vec::new();
            for environment in &environments {
                let tenant = TenantScope::new(project_id.clone(), environment.id().clone());
                match self
                    .provider
                    .tenant(
                        &tenant,
                        kind,
                        now_unix_seconds.saturating_sub(24 * 60 * 60),
                        now_unix_seconds,
                    )
                    .await
                {
                    Ok(mut received) => values.append(&mut received),
                    Err(_) => {
                        partial = true;
                        values.push(OperatorReadSection::unavailable(
                            format!("{}:{}", kind.as_str(), environment.id().as_str()),
                            kind.as_str(),
                        ));
                    }
                }
            }
            if values.is_empty() {
                values.push(OperatorReadSection {
                    id: kind.as_str().to_owned(),
                    freshness: Freshness::Unknown,
                    observed_at_unix_seconds: Some(now_unix_seconds),
                    provider: "control-plane".to_owned(),
                    message: Some("No environments currently contribute data.".to_owned()),
                    metrics: BTreeMap::new(),
                    links: Vec::new(),
                });
            }
            validate_sections(&mut values, &self.config)?;
            let mut collapsed = collapse_sections(kind, values);
            collapsed.id = section_name.to_owned();
            sections.insert(section_name.to_owned(), collapsed);
        }
        let activity = self
            .scan_records::<ActivityRecord>(ControlKeyspace::operator_activity_range()?, MAX_PAGE)
            .await?;
        let project_activity = activity
            .iter()
            .filter(|(_, event)| event.project_id.as_ref() == Some(project_id))
            .count();
        sections.insert(
            "operator-activity".to_owned(),
            OperatorReadSection {
                id: "operator-activity".to_owned(),
                freshness: Freshness::Current,
                observed_at_unix_seconds: Some(now_unix_seconds),
                provider: "activity-projection".to_owned(),
                message: None,
                metrics: BTreeMap::from([(
                    "recentCount".to_owned(),
                    Value::from(project_activity),
                )]),
                links: Vec::new(),
            },
        );
        let support = self
            .scan_records::<Value>(
                ControlKeyspace::operator_all_support_sessions_range()?,
                MAX_PAGE,
            )
            .await?;
        let support_count = support
            .iter()
            .filter(|(_, record)| {
                record.get("projectId").and_then(Value::as_str) == Some(project_id.as_str())
            })
            .count();
        sections.insert(
            "support-history".to_owned(),
            OperatorReadSection {
                id: "support-history".to_owned(),
                freshness: Freshness::Current,
                observed_at_unix_seconds: Some(now_unix_seconds),
                provider: "control-plane".to_owned(),
                message: None,
                metrics: BTreeMap::from([("recordCount".to_owned(), Value::from(support_count))]),
                links: Vec::new(),
            },
        );
        self.allowed_project(actor, project_id, "tenant_360", now_unix_seconds);
        Ok(Tenant360 {
            project: summary,
            environments,
            sections,
            partial,
        })
    }

    pub async fn inventory(
        &self,
        actor: &OperatorPrincipal,
        kind: InventoryKind,
        project_id: Option<&ProjectId>,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorControlCenterError> {
        validate_window(from_unix_seconds, until_unix_seconds, now_unix_seconds)?;
        if let Some(project_id) = project_id {
            self.require_project(
                actor,
                kind.permission(),
                project_id,
                kind.as_str(),
                now_unix_seconds,
            )?;
            self.project(project_id).await?;
        } else {
            self.require_global(actor, kind.permission(), kind.as_str(), now_unix_seconds)?;
        }
        let mut sections = if let Some(project_id) = project_id {
            let environments = self
                .projects
                .list_environments(project_id, NonZeroUsize::new(MAX_PAGE).expect("positive"))
                .await?;
            let mut scoped = Vec::new();
            for environment in environments {
                let tenant = TenantScope::new(project_id.clone(), environment.id().clone());
                match self
                    .provider
                    .tenant(&tenant, kind, from_unix_seconds, until_unix_seconds)
                    .await
                {
                    Ok(mut received) => scoped.append(&mut received),
                    Err(_) => scoped.push(OperatorReadSection::unavailable(
                        format!("{}:{}", kind.as_str(), environment.id().as_str()),
                        kind.as_str(),
                    )),
                }
            }
            if scoped.is_empty() {
                vec![OperatorReadSection {
                    id: kind.as_str().to_owned(),
                    freshness: Freshness::Unknown,
                    observed_at_unix_seconds: Some(now_unix_seconds),
                    provider: "control-plane".to_owned(),
                    message: Some("No environments currently contribute data.".to_owned()),
                    metrics: BTreeMap::new(),
                    links: Vec::new(),
                }]
            } else {
                scoped
            }
        } else {
            self.provider
                .global(kind, from_unix_seconds, until_unix_seconds)
                .await
                .unwrap_or_else(|_| {
                    vec![OperatorReadSection::unavailable(
                        kind.as_str(),
                        kind.as_str(),
                    )]
                })
        };
        validate_sections(&mut sections, &self.config)?;
        if let Some(project_id) = project_id {
            self.allowed_project(actor, project_id, kind.as_str(), now_unix_seconds);
        } else {
            self.allowed_global(actor, kind.as_str(), now_unix_seconds);
        }
        Ok(sections)
    }

    pub async fn list_incidents(
        &self,
        actor: &OperatorPrincipal,
        search: &IncidentSearch,
        cursor: Option<&str>,
        limit: usize,
        now_unix_seconds: u64,
    ) -> Result<OperatorCursorPage<IncidentRecord>, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::IncidentRead,
            "incidents",
            now_unix_seconds,
        )?;
        search.validate(now_unix_seconds)?;
        validate_limit(limit)?;
        let digest = search.digest()?;
        let start = cursor
            .map(|value| self.decode_cursor(value, "incidents", &digest, now_unix_seconds))
            .transpose()?;
        let range = with_start(ControlKeyspace::operator_incidents_range()?, start)?;
        let records = self
            .scan_records::<IncidentRecord>(
                range,
                (limit.saturating_mul(MAX_SCAN_MULTIPLIER) + 1).min(MAX_PAGE * 8 + 1),
            )
            .await?;
        let mut items = Vec::with_capacity(limit);
        let mut last_examined = None;
        for (key, record) in &records {
            last_examined = Some(key);
            if search.state.is_some_and(|state| state != record.state)
                || search
                    .severity
                    .as_ref()
                    .is_some_and(|severity| severity != &record.severity)
                || search
                    .project_id
                    .as_ref()
                    .is_some_and(|project_id| record.project_id.as_ref() != Some(project_id))
                || search
                    .from_unix_seconds
                    .is_some_and(|from| record.updated_at_unix_seconds < from)
                || search
                    .until_unix_seconds
                    .is_some_and(|until| record.updated_at_unix_seconds > until)
            {
                continue;
            }
            items.push(record.clone());
            if items.len() == limit {
                break;
            }
        }
        let has_more = records.len() > limit || items.len() == limit;
        let next_cursor = has_more
            .then_some(last_examined)
            .flatten()
            .map(|key| self.encode_cursor("incidents", key, &digest, now_unix_seconds))
            .transpose()?;
        self.allowed_global(actor, "incidents", now_unix_seconds);
        Ok(OperatorCursorPage {
            items,
            next_cursor,
            observed_at_unix_seconds: now_unix_seconds,
            examined_records: records.len(),
        })
    }

    pub async fn incident(
        &self,
        actor: &OperatorPrincipal,
        id: &str,
        now_unix_seconds: u64,
    ) -> Result<IncidentRecord, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::IncidentRead,
            "incidents",
            now_unix_seconds,
        )?;
        validate_identifier(id, "inc_").map_err(|_| OperatorControlCenterError::NotFound)?;
        let record: IncidentRecord = self
            .read_required(&ControlKeyspace::operator_incident_key(id)?)
            .await?;
        self.allowed_global(actor, "incidents", now_unix_seconds);
        Ok(record)
    }

    pub async fn current_alerts(
        &self,
        actor: &OperatorPrincipal,
        project_id: Option<&ProjectId>,
        cursor: Option<&str>,
        limit: usize,
        now_unix_seconds: u64,
    ) -> Result<OperatorCursorPage<AlertSummary>, OperatorControlCenterError> {
        validate_limit(limit)?;
        let scope = project_id.map_or("global", ProjectId::as_str);
        let filter_digest = blake3::hash(scope.as_bytes()).to_hex().to_string();
        let start = cursor
            .map(|value| self.decode_cursor(value, "alerts", &filter_digest, now_unix_seconds))
            .transpose()?
            .map(|value| {
                String::from_utf8(value).map_err(|_| OperatorControlCenterError::InvalidCursor)
            })
            .transpose()?;
        let sections = self
            .inventory(
                actor,
                InventoryKind::Alerts,
                project_id,
                now_unix_seconds.saturating_sub(24 * 60 * 60),
                now_unix_seconds,
                now_unix_seconds,
            )
            .await?;
        let mut alerts = sections
            .into_iter()
            .filter(|section| section.freshness != Freshness::Current)
            .map(|section| {
                let observed = section.observed_at_unix_seconds.unwrap_or(now_unix_seconds);
                let fingerprint = format!(
                    "alr_{}",
                    &blake3::hash(
                        format!("{}:{}:{scope}", section.provider, section.id).as_bytes()
                    )
                    .to_hex()[..32]
                );
                AlertSummary {
                    fingerprint,
                    severity: match section.freshness {
                        Freshness::Unavailable => "critical",
                        Freshness::Stale => "high",
                        Freshness::Unknown => "medium",
                        Freshness::Current => "low",
                    }
                    .to_owned(),
                    affected_scope: scope.to_owned(),
                    first_observed_at_unix_seconds: observed,
                    last_observed_at_unix_seconds: observed,
                    freshness: section.freshness,
                    runbook: section
                        .links
                        .into_iter()
                        .find(|link| link.kind == "runbook"),
                }
            })
            .collect::<Vec<_>>();
        alerts.sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
        if let Some(start) = start {
            alerts.retain(|alert| alert.fingerprint > start);
        }
        let examined_records = alerts.len().min(limit + 1);
        let has_more = alerts.len() > limit;
        alerts.truncate(limit);
        let next_cursor = has_more
            .then(|| alerts.last().map(|alert| alert.fingerprint.as_bytes()))
            .flatten()
            .map(|key| self.encode_cursor("alerts", key, &filter_digest, now_unix_seconds))
            .transpose()?;
        Ok(OperatorCursorPage {
            items: alerts,
            next_cursor,
            observed_at_unix_seconds: now_unix_seconds,
            examined_records,
        })
    }

    pub async fn current_alert(
        &self,
        actor: &OperatorPrincipal,
        fingerprint: &str,
        project_id: Option<&ProjectId>,
        now_unix_seconds: u64,
    ) -> Result<AlertSummary, OperatorControlCenterError> {
        if !fingerprint.starts_with("alr_") || !safe_text(fingerprint, 100) {
            return Err(OperatorControlCenterError::NotFound);
        }
        self.current_alerts(actor, project_id, None, MAX_PAGE, now_unix_seconds)
            .await?
            .items
            .into_iter()
            .find(|alert| alert.fingerprint == fingerprint)
            .ok_or(OperatorControlCenterError::NotFound)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_incident(
        &self,
        actor: &OperatorPrincipal,
        id: String,
        fingerprint: String,
        title: String,
        severity: String,
        project_id: Option<ProjectId>,
        guard: &GuardedMutation,
        now_unix_seconds: u64,
    ) -> Result<IncidentRecord, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::IncidentManage,
            "incident_create",
            now_unix_seconds,
        )?;
        validate_identifier(&id, "inc_")?;
        if !safe_text(&fingerprint, 128)
            || !safe_text(&title, 200)
            || !matches!(severity.as_str(), "critical" | "high" | "medium" | "low")
        {
            return Err(OperatorControlCenterError::InvalidInput);
        }
        if let Err(error) = guard.validate("incident_create", &id, now_unix_seconds) {
            self.denied_project(
                actor,
                project_id.as_ref(),
                "incident_create",
                now_unix_seconds,
            );
            return Err(error);
        }
        if let Some(project_id) = &project_id {
            self.project(project_id).await?;
        }
        let record = IncidentRecord {
            id: id.clone(),
            fingerprint,
            title,
            severity,
            state: IncidentState::Open,
            project_id,
            assignee: None,
            version: 1,
            opened_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            timeline: vec![IncidentEvent {
                sequence: 1,
                action: "opened".to_owned(),
                actor_id: actor.id().as_str().to_owned(),
                note: Some(guard.reason.clone()),
                at_unix_seconds: now_unix_seconds,
            }],
        };
        self.create_record(ControlKeyspace::operator_incident_key(&id)?, &record)
            .await?;
        self.append_activity(
            actor,
            "incident_create",
            &id,
            record.project_id.as_ref(),
            "allowed",
            guard,
            now_unix_seconds,
        )
        .await?;
        Ok(record)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_incident(
        &self,
        actor: &OperatorPrincipal,
        id: &str,
        action: &str,
        assignee: Option<String>,
        note: Option<String>,
        guard: &GuardedMutation,
        now_unix_seconds: u64,
    ) -> Result<IncidentRecord, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::IncidentManage,
            "incident_update",
            now_unix_seconds,
        )?;
        let key = ControlKeyspace::operator_incident_key(id)?;
        let previous: IncidentRecord = self.read_required(&key).await?;
        if previous.version != guard.reviewed_version {
            self.denied_project(
                actor,
                previous.project_id.as_ref(),
                "incident_update",
                now_unix_seconds,
            );
            return Err(OperatorControlCenterError::StaleVersion);
        }
        if let Err(error) = guard.validate(action, id, now_unix_seconds) {
            self.denied_project(
                actor,
                previous.project_id.as_ref(),
                "incident_update",
                now_unix_seconds,
            );
            return Err(error);
        }
        let mut next = previous.clone();
        match action {
            "acknowledge" if previous.state == IncidentState::Open => {
                next.state = IncidentState::Acknowledged;
            }
            "assign" => {
                let value = assignee.ok_or(OperatorControlCenterError::InvalidInput)?;
                if !safe_text(&value, 200) {
                    return Err(OperatorControlCenterError::InvalidInput);
                }
                next.assignee = Some(value);
            }
            "annotate" => {
                if note.as_ref().is_none_or(|value| !safe_text(value, 1_024)) {
                    return Err(OperatorControlCenterError::InvalidInput);
                }
            }
            "resolve" if previous.state != IncidentState::Resolved => {
                next.state = IncidentState::Resolved;
            }
            _ => return Err(OperatorControlCenterError::InvalidState),
        }
        next.version = next.version.saturating_add(1);
        next.updated_at_unix_seconds = now_unix_seconds;
        next.timeline.push(IncidentEvent {
            sequence: next.version,
            action: action.to_owned(),
            actor_id: actor.id().as_str().to_owned(),
            note: note.or_else(|| Some(guard.reason.clone())),
            at_unix_seconds: now_unix_seconds,
        });
        if next.timeline.len() > MAX_PAGE {
            return Err(OperatorControlCenterError::BoundExceeded);
        }
        self.replace_record(key, &previous, &next).await?;
        self.append_activity(
            actor,
            action,
            id,
            next.project_id.as_ref(),
            "allowed",
            guard,
            now_unix_seconds,
        )
        .await?;
        Ok(next)
    }

    pub async fn create_recovery_job(
        &self,
        actor: &OperatorPrincipal,
        request: RecoveryRequest,
        now_unix_seconds: u64,
    ) -> Result<RecoveryJobRecord, OperatorControlCenterError> {
        self.require_project(
            actor,
            OperatorPermission::RecoveryManage,
            &request.project_id,
            "recovery_create",
            now_unix_seconds,
        )?;
        if !self.config.recovery_creation_enabled {
            return Err(OperatorControlCenterError::FeatureDisabled);
        }
        validate_identifier(&request.id, "rcv_")?;
        if !request.backup_verified
            || !safe_text(&request.backup_id, 128)
            || !safe_text(&request.target, 256)
            || !safe_text(&request.impact_preview, 2_048)
        {
            return Err(OperatorControlCenterError::InvalidInput);
        }
        if let Err(error) = request
            .guard
            .validate("recovery_create", &request.id, now_unix_seconds)
        {
            self.denied_project(
                actor,
                Some(&request.project_id),
                "recovery_create",
                now_unix_seconds,
            );
            return Err(error);
        }
        self.project(&request.project_id).await?;
        let record = RecoveryJobRecord {
            id: request.id.clone(),
            project_id: request.project_id,
            backup_id: request.backup_id,
            target: request.target,
            state: RecoveryJobState::Requested,
            version: 1,
            verification_succeeded: false,
            promotion_permitted: true,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds: now_unix_seconds + MAX_RECOVERY_JOB_SECONDS,
            last_error_class: None,
        };
        self.create_record(
            ControlKeyspace::operator_recovery_job_key(&record.id)?,
            &record,
        )
        .await?;
        self.append_activity(
            actor,
            "recovery_create",
            &record.id,
            Some(&record.project_id),
            "allowed",
            &request.guard,
            now_unix_seconds,
        )
        .await?;
        Ok(record)
    }

    pub async fn list_recovery_jobs(
        &self,
        actor: &OperatorPrincipal,
        project_id: Option<&ProjectId>,
        cursor: Option<&str>,
        limit: usize,
        now_unix_seconds: u64,
    ) -> Result<OperatorCursorPage<RecoveryJobRecord>, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::BackupRead,
            "recovery_jobs",
            now_unix_seconds,
        )?;
        validate_limit(limit)?;
        if let Some(project_id) = project_id {
            self.project(project_id).await?;
        }
        let digest = project_id.map_or("global", ProjectId::as_str);
        let start = cursor
            .map(|value| self.decode_cursor(value, "recovery_jobs", digest, now_unix_seconds))
            .transpose()?;
        let range = with_start(ControlKeyspace::operator_recovery_jobs_range()?, start)?;
        let records = self
            .scan_records::<RecoveryJobRecord>(
                range,
                (limit.saturating_mul(MAX_SCAN_MULTIPLIER) + 1).min(MAX_PAGE * 8 + 1),
            )
            .await?;
        let mut items = Vec::with_capacity(limit);
        let mut last_examined = None;
        for (key, record) in &records {
            last_examined = Some(key);
            if project_id.is_some_and(|project_id| &record.project_id != project_id) {
                continue;
            }
            items.push(record.clone());
            if items.len() == limit {
                break;
            }
        }
        let has_more = records.len() > limit || items.len() == limit;
        let next_cursor = has_more
            .then_some(last_examined)
            .flatten()
            .map(|key| self.encode_cursor("recovery_jobs", key, digest, now_unix_seconds))
            .transpose()?;
        self.allowed_global(actor, "recovery_jobs", now_unix_seconds);
        Ok(OperatorCursorPage {
            items,
            next_cursor,
            observed_at_unix_seconds: now_unix_seconds,
            examined_records: records.len(),
        })
    }

    pub async fn recovery_job(
        &self,
        actor: &OperatorPrincipal,
        id: &str,
        now_unix_seconds: u64,
    ) -> Result<RecoveryJobRecord, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::BackupRead,
            "recovery_jobs",
            now_unix_seconds,
        )?;
        validate_identifier(id, "rcv_").map_err(|_| OperatorControlCenterError::NotFound)?;
        let record: RecoveryJobRecord = self
            .read_required(&ControlKeyspace::operator_recovery_job_key(id)?)
            .await?;
        self.allowed_project(actor, &record.project_id, "recovery_jobs", now_unix_seconds);
        Ok(record)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn advance_recovery_job(
        &self,
        actor: &OperatorPrincipal,
        id: &str,
        next_state: RecoveryJobState,
        verification_succeeded: bool,
        error_class: Option<String>,
        guard: &GuardedMutation,
        now_unix_seconds: u64,
    ) -> Result<RecoveryJobRecord, OperatorControlCenterError> {
        let key = ControlKeyspace::operator_recovery_job_key(id)?;
        let previous: RecoveryJobRecord = self.read_required(&key).await?;
        self.require_project(
            actor,
            OperatorPermission::RecoveryManage,
            &previous.project_id,
            "recovery_advance",
            now_unix_seconds,
        )?;
        if previous.version != guard.reviewed_version
            || previous.expires_at_unix_seconds <= now_unix_seconds
        {
            self.denied_project(
                actor,
                Some(&previous.project_id),
                "recovery_advance",
                now_unix_seconds,
            );
            return Err(OperatorControlCenterError::StaleVersion);
        }
        if let Err(error) = guard.validate("recovery_advance", id, now_unix_seconds) {
            self.denied_project(
                actor,
                Some(&previous.project_id),
                "recovery_advance",
                now_unix_seconds,
            );
            return Err(error);
        }
        if next_state == RecoveryJobState::Promoted && !self.config.recovery_promotion_enabled {
            return Err(OperatorControlCenterError::FeatureDisabled);
        }
        if !valid_recovery_transition(previous.state, next_state)
            || (next_state == RecoveryJobState::PromotionReady && !verification_succeeded)
            || (next_state == RecoveryJobState::Promoted && !previous.verification_succeeded)
            || error_class
                .as_ref()
                .is_some_and(|value| !safe_text(value, 128))
        {
            return Err(OperatorControlCenterError::InvalidState);
        }
        let mut next = previous.clone();
        next.state = next_state;
        next.version = next.version.saturating_add(1);
        next.updated_at_unix_seconds = now_unix_seconds;
        next.verification_succeeded |= verification_succeeded;
        next.last_error_class = error_class;
        self.replace_record(key, &previous, &next).await?;
        self.append_activity(
            actor,
            "recovery_advance",
            id,
            Some(&next.project_id),
            "allowed",
            guard,
            now_unix_seconds,
        )
        .await?;
        Ok(next)
    }

    pub async fn execute_recovery_step(
        &self,
        actor: &OperatorPrincipal,
        id: &str,
        executor: &dyn RecoveryExecutor,
        guard: &GuardedMutation,
        now_unix_seconds: u64,
    ) -> Result<RecoveryJobRecord, OperatorControlCenterError> {
        let key = ControlKeyspace::operator_recovery_job_key(id)?;
        let current: RecoveryJobRecord = self.read_required(&key).await?;
        if current.version != guard.reviewed_version
            || current.expires_at_unix_seconds <= now_unix_seconds
        {
            self.denied_project(
                actor,
                Some(&current.project_id),
                "recovery_execute",
                now_unix_seconds,
            );
            return Err(OperatorControlCenterError::StaleVersion);
        }
        if current.state == RecoveryJobState::PromotionReady
            && (!self.config.recovery_promotion_enabled || !current.promotion_permitted)
        {
            return Err(OperatorControlCenterError::FeatureDisabled);
        }
        guard.validate("recovery_execute", id, now_unix_seconds)?;
        let mut transition_guard = guard.clone();
        transition_guard.action_binding =
            GuardedMutation::action_binding("recovery_advance", id, guard.reviewed_version);
        let (next, verification, result) = match current.state {
            RecoveryJobState::Requested => (
                RecoveryJobState::Preparing,
                false,
                executor.prepare(&current).await,
            ),
            RecoveryJobState::Preparing => (
                RecoveryJobState::Restoring,
                false,
                executor.restore(&current).await,
            ),
            RecoveryJobState::Restoring => (RecoveryJobState::Verifying, false, Ok(())),
            RecoveryJobState::Verifying => (
                RecoveryJobState::PromotionReady,
                true,
                executor.verify(&current).await,
            ),
            RecoveryJobState::PromotionReady => (
                RecoveryJobState::Promoted,
                current.verification_succeeded,
                executor.promote(&current).await,
            ),
            _ => return Err(OperatorControlCenterError::InvalidState),
        };
        match result {
            Ok(()) => {
                self.advance_recovery_job(
                    actor,
                    id,
                    next,
                    verification,
                    None,
                    &transition_guard,
                    now_unix_seconds,
                )
                .await
            }
            Err(error) => {
                let class = match error {
                    RecoveryExecutorError::Unavailable => "executor_unavailable",
                    RecoveryExecutorError::EvidenceRejected => "evidence_rejected",
                    RecoveryExecutorError::RestoreFailed => "restore_failed",
                    RecoveryExecutorError::VerificationFailed => "verification_failed",
                    RecoveryExecutorError::PromotionFailed => "promotion_failed",
                };
                self.advance_recovery_job(
                    actor,
                    id,
                    RecoveryJobState::Failed,
                    false,
                    Some(class.to_owned()),
                    &transition_guard,
                    now_unix_seconds,
                )
                .await
            }
        }
    }

    pub async fn create_activity_export(
        &self,
        actor: &OperatorPrincipal,
        id: String,
        filter: &Value,
        guard: &GuardedMutation,
        now_unix_seconds: u64,
    ) -> Result<ActivityExportRecord, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::ActivityExport,
            "activity_export",
            now_unix_seconds,
        )?;
        validate_identifier(&id, "exp_")?;
        if !safe_value(filter, 0) {
            return Err(OperatorControlCenterError::UnsafeResponse);
        }
        guard.validate("activity_export", &id, now_unix_seconds)?;
        let filter_digest = blake3::hash(&serde_json::to_vec(filter)?)
            .to_hex()
            .to_string();
        let record = ActivityExportRecord {
            id: id.clone(),
            requested_by: actor.id().as_str().to_owned(),
            filter_digest,
            filter: filter.clone(),
            state: ActivityExportState::Pending,
            record_count: 0,
            checksum: None,
            created_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds: now_unix_seconds + 60 * 60,
        };
        self.create_record(ControlKeyspace::operator_activity_export_key(&id)?, &record)
            .await?;
        self.append_activity(
            actor,
            "activity_export",
            &id,
            None,
            "allowed",
            guard,
            now_unix_seconds,
        )
        .await?;
        Ok(record)
    }

    pub async fn activity_export(
        &self,
        actor: &OperatorPrincipal,
        id: &str,
        now_unix_seconds: u64,
    ) -> Result<ActivityExportRecord, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::ActivityExport,
            "activity_export",
            now_unix_seconds,
        )?;
        validate_identifier(id, "exp_").map_err(|_| OperatorControlCenterError::NotFound)?;
        let mut record: ActivityExportRecord = self
            .read_required(&ControlKeyspace::operator_activity_export_key(id)?)
            .await?;
        if record.expires_at_unix_seconds <= now_unix_seconds
            && record.state != ActivityExportState::Expired
        {
            record.state = ActivityExportState::Expired;
        }
        Ok(record)
    }

    pub async fn process_activity_export(
        &self,
        actor: &OperatorPrincipal,
        id: &str,
        now_unix_seconds: u64,
    ) -> Result<ActivityExportRecord, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::ActivityExport,
            "activity_export",
            now_unix_seconds,
        )?;
        let key = ControlKeyspace::operator_activity_export_key(id)?;
        let previous: ActivityExportRecord = self.read_required(&key).await?;
        if previous.expires_at_unix_seconds <= now_unix_seconds {
            return Err(OperatorControlCenterError::StaleVersion);
        }
        if previous.state == ActivityExportState::Ready {
            return Ok(previous);
        }
        if previous.state != ActivityExportState::Pending {
            return Err(OperatorControlCenterError::InvalidState);
        }
        let records = self
            .scan_records::<ActivityRecord>(ControlKeyspace::operator_activity_range()?, MAX_PAGE)
            .await?;
        if records.iter().any(|(_, record)| !record.integrity_valid()) {
            return Err(OperatorControlCenterError::UnsafeResponse);
        }
        let query = previous
            .filter
            .get("query")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase);
        let selected = records
            .iter()
            .map(|(_, record)| record)
            .filter(|record| {
                query.as_ref().is_none_or(|query| {
                    [
                        record.actor_id.as_str(),
                        record.action.as_str(),
                        record.target.as_str(),
                        record.outcome.as_str(),
                    ]
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(query))
                })
            })
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&selected)?;
        let mut next = previous.clone();
        next.state = ActivityExportState::Ready;
        next.record_count = selected.len();
        next.checksum = Some(blake3::hash(&bytes).to_hex().to_string());
        self.replace_record(key, &previous, &next).await?;
        Ok(next)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn activity(
        &self,
        actor: &OperatorPrincipal,
        cursor: Option<&str>,
        limit: usize,
        query: Option<&str>,
        from_unix_seconds: Option<u64>,
        until_unix_seconds: Option<u64>,
        now_unix_seconds: u64,
    ) -> Result<OperatorCursorPage<ActivityRecord>, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::ActivityRead,
            "activity",
            now_unix_seconds,
        )?;
        validate_limit(limit)?;
        if let Some(query) = query
            && !safe_text(query, 200)
        {
            return Err(OperatorControlCenterError::InvalidInput);
        }
        let from =
            from_unix_seconds.unwrap_or_else(|| now_unix_seconds.saturating_sub(24 * 60 * 60));
        let until = until_unix_seconds.unwrap_or(now_unix_seconds);
        validate_window(from, until, now_unix_seconds)?;
        let filter_digest =
            blake3::hash(format!("{}:{from}:{until}", query.unwrap_or_default()).as_bytes())
                .to_hex()
                .to_string();
        let start = cursor
            .map(|value| self.decode_cursor(value, "activity", &filter_digest, now_unix_seconds))
            .transpose()?;
        let range = with_start(ControlKeyspace::operator_activity_range()?, start)?;
        let records = self
            .scan_records::<ActivityRecord>(range, limit.saturating_mul(MAX_SCAN_MULTIPLIER) + 1)
            .await?;
        if records.iter().any(|(_, record)| !record.integrity_valid()) {
            return Err(OperatorControlCenterError::UnsafeResponse);
        }
        let needle = query.map(str::to_ascii_lowercase);
        let mut items = Vec::with_capacity(limit);
        let mut last_examined = None;
        for (key, record) in &records {
            last_examined = Some(key.clone());
            if record.at_unix_seconds < from
                || record.at_unix_seconds > until
                || needle.as_ref().is_some_and(|needle| {
                    ![
                        record.actor_id.as_str(),
                        record.action.as_str(),
                        record.target.as_str(),
                        record.outcome.as_str(),
                        record.case_reference.as_deref().unwrap_or_default(),
                        record.project_id.as_ref().map_or("", ProjectId::as_str),
                    ]
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(needle))
                })
            {
                continue;
            }
            items.push(record.clone());
            if items.len() == limit {
                break;
            }
        }
        let has_more = records.len() > limit || items.len() == limit;
        let next_cursor = has_more
            .then_some(last_examined)
            .flatten()
            .map(|key| self.encode_cursor("activity", &key, &filter_digest, now_unix_seconds))
            .transpose()?;
        self.allowed_global(actor, "activity", now_unix_seconds);
        Ok(OperatorCursorPage {
            items,
            next_cursor,
            observed_at_unix_seconds: now_unix_seconds,
            examined_records: records.len(),
        })
    }

    pub async fn rebuild_projection(
        &self,
        actor: &OperatorPrincipal,
        name: &str,
        cursor: Option<&str>,
        now_unix_seconds: u64,
    ) -> Result<ProjectionEvidence, OperatorControlCenterError> {
        self.require_global(
            actor,
            OperatorPermission::SecurityManage,
            "projection_rebuild",
            now_unix_seconds,
        )?;
        if !matches!(name, "tenant_search" | "activity") {
            return Err(OperatorControlCenterError::InvalidInput);
        }
        let key = ControlKeyspace::operator_projection_key(name)?;
        let previous: Option<ProjectionEvidence> = self.read_optional(&key).await?;
        if let Some(previous) = &previous {
            if previous.complete {
                return Ok(previous.clone());
            }
            if previous.cursor.as_deref() != cursor {
                return Err(OperatorControlCenterError::StaleVersion);
            }
        }
        let start = previous
            .as_ref()
            .and_then(|evidence| evidence.cursor.as_deref())
            .map(|value| {
                URL_SAFE_NO_PAD
                    .decode(value)
                    .map_err(|_| OperatorControlCenterError::InvalidCursor)
            })
            .transpose()?;
        let prior_count = previous.as_ref().map_or(0, |value| value.projected_count);
        let prior_checksum = previous.as_ref().map_or_else(
            || blake3::hash(b"").to_hex().to_string(),
            |value| value.checksum.clone(),
        );
        let (count, checksum, next_cursor, complete, writes) = match name {
            "tenant_search" => {
                let records = self.scan_projects(start, MAX_PAGE + 1).await?;
                let mut hasher = blake3::Hasher::new();
                hasher.update(prior_checksum.as_bytes());
                let page = records.iter().take(MAX_PAGE).collect::<Vec<_>>();
                let mut writes = Vec::with_capacity(page.len());
                for (_, project) in &page {
                    hasher.update(&serde_json::to_vec(project)?);
                    let index = TenantSearchIndexRecord {
                        schema_version: 1,
                        project_id: project.id().clone(),
                        organization_id: project.organization_id().as_str().to_owned(),
                        normalized_project_name: project.name().to_ascii_lowercase(),
                        region: project.region().to_owned(),
                        lifecycle: project.lifecycle(),
                        source_updated_at_unix_seconds: project.updated_at_unix_seconds(),
                    };
                    writes.push((
                        ControlKeyspace::operator_tenant_search_key(project.id())?,
                        serde_json::to_vec(&index)?,
                    ));
                }
                let complete = records.len() <= MAX_PAGE;
                let next = (!complete)
                    .then(|| page.last().map(|(key, _)| URL_SAFE_NO_PAD.encode(key)))
                    .flatten();
                (
                    prior_count + page.len(),
                    hasher.finalize().to_hex().to_string(),
                    next,
                    complete,
                    writes,
                )
            }
            "activity" => {
                let range = with_start(ControlKeyspace::operator_activity_range()?, start)?;
                let records = self
                    .scan_records::<ActivityRecord>(range, MAX_PAGE + 1)
                    .await?;
                if records.iter().any(|(_, record)| !record.integrity_valid()) {
                    return Err(OperatorControlCenterError::UnsafeResponse);
                }
                let mut hasher = blake3::Hasher::new();
                hasher.update(prior_checksum.as_bytes());
                let page = records.iter().take(MAX_PAGE).collect::<Vec<_>>();
                for (_, record) in &page {
                    hasher.update(&serde_json::to_vec(record)?);
                }
                let complete = records.len() <= MAX_PAGE;
                let next = (!complete)
                    .then(|| page.last().map(|(key, _)| URL_SAFE_NO_PAD.encode(key)))
                    .flatten();
                (
                    prior_count + page.len(),
                    hasher.finalize().to_hex().to_string(),
                    next,
                    complete,
                    Vec::new(),
                )
            }
            _ => unreachable!(),
        };
        let evidence = ProjectionEvidence {
            name: name.to_owned(),
            source_count: count,
            projected_count: count,
            checksum,
            cursor: next_cursor,
            complete,
            updated_at_unix_seconds: now_unix_seconds,
        };
        let mut batch = WriteBatch::with_capacity(writes.len() + 1);
        for (write_key, value) in writes {
            batch.put(&write_key, value);
        }
        batch.put(&key, serde_json::to_vec(&evidence)?);
        let conditions = match previous {
            Some(previous) => vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(&previous)?,
            }],
            None => vec![KeyCondition::Missing { key }],
        };
        self.apply(AtomicWrite {
            conditions,
            batch,
            durability: self.durability,
        })
        .await?;
        Ok(evidence)
    }

    async fn tenant_summary(
        &self,
        project: &ProjectRecord,
        organization: Option<&crate::OrganizationRecord>,
    ) -> Result<TenantSummary, OperatorControlCenterError> {
        let environments = self
            .projects
            .list_environments(project.id(), NonZeroUsize::new(MAX_PAGE).expect("positive"))
            .await?;
        Ok(tenant_summary(project, organization, environments.len()))
    }

    async fn project(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectRecord, OperatorControlCenterError> {
        self.projects
            .get_project(project_id)
            .await?
            .ok_or(OperatorControlCenterError::NotFound)
    }

    async fn scan_projects(
        &self,
        start: Option<Vec<u8>>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, ProjectRecord)>, OperatorControlCenterError> {
        let range = with_start(ControlKeyspace::projects_range()?, start)?;
        self.scan_records(range, limit).await
    }

    async fn scan_records<T: DeserializeOwned>(
        &self,
        range: KeyRange,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, T)>, OperatorControlCenterError> {
        let limit = NonZeroUsize::new(limit).ok_or(OperatorControlCenterError::InvalidInput)?;
        let records = self
            .adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await?;
        records
            .into_iter()
            .map(|record| Ok((record.key, serde_json::from_slice(&record.value)?)))
            .collect()
    }

    async fn read_optional<T: DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<Option<T>, OperatorControlCenterError> {
        self.adapter
            .get(key)
            .await?
            .map(|value| serde_json::from_slice(&value).map_err(Into::into))
            .transpose()
    }

    async fn read_required<T: DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<T, OperatorControlCenterError> {
        self.read_optional(key)
            .await?
            .ok_or(OperatorControlCenterError::NotFound)
    }

    async fn create_record<T: Serialize>(
        &self,
        key: Vec<u8>,
        record: &T,
    ) -> Result<(), OperatorControlCenterError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(record)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn replace_record<T: Serialize>(
        &self,
        key: Vec<u8>,
        previous: &T,
        next: &T,
    ) -> Result<(), OperatorControlCenterError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), OperatorControlCenterError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(OperatorControlCenterError::Conflict),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn append_activity(
        &self,
        actor: &OperatorPrincipal,
        action: &str,
        target: &str,
        project_id: Option<&ProjectId>,
        outcome: &str,
        guard: &GuardedMutation,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorControlCenterError> {
        let id = format!(
            "evt_{}",
            &blake3::hash(format!("{}:{}:{}", guard.operation_key, action, target).as_bytes())
                .to_hex()[..24]
        );
        let mut record = ActivityRecord {
            id: id.clone(),
            source_event_id: guard.operation_key.clone(),
            actor_id: actor.id().as_str().to_owned(),
            action: action.to_owned(),
            target: target.to_owned(),
            project_id: project_id.cloned(),
            case_reference: guard.case_reference.clone(),
            outcome: outcome.to_owned(),
            at_unix_seconds: now_unix_seconds,
            integrity: String::new(),
        };
        record.integrity = record.computed_integrity();
        let key = ControlKeyspace::operator_activity_key(now_unix_seconds * 1_000, &id)?;
        match self.create_record(key, &record).await {
            Ok(()) | Err(OperatorControlCenterError::Conflict) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn require_global(
        &self,
        actor: &OperatorPrincipal,
        permission: OperatorPermission,
        action: &str,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorControlCenterError> {
        if allows_compat(actor, permission) {
            return Ok(());
        }
        self.audit.record(OperatorAuditEvent {
            operator_id: actor.id().clone(),
            action: audit_action(action),
            project_id: global_project_id(),
            target: format!("denied:{action}"),
            reason: "permission denied".to_owned(),
            outcome: OperatorAuditOutcome::Denied,
            at_unix_seconds: now_unix_seconds,
        });
        Err(OperatorControlCenterError::Forbidden)
    }

    fn require_project(
        &self,
        actor: &OperatorPrincipal,
        permission: OperatorPermission,
        project_id: &ProjectId,
        action: &str,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorControlCenterError> {
        if allows_compat(actor, permission) {
            return Ok(());
        }
        self.audit.record(OperatorAuditEvent {
            operator_id: actor.id().clone(),
            action: audit_action(action),
            project_id: project_id.clone(),
            target: format!("denied:{action}"),
            reason: "permission denied".to_owned(),
            outcome: OperatorAuditOutcome::Denied,
            at_unix_seconds: now_unix_seconds,
        });
        Err(OperatorControlCenterError::Forbidden)
    }

    fn allowed_global(&self, actor: &OperatorPrincipal, action: &str, now_unix_seconds: u64) {
        self.allowed_project(actor, &global_project_id(), action, now_unix_seconds);
    }

    fn denied_project(
        &self,
        actor: &OperatorPrincipal,
        project_id: Option<&ProjectId>,
        action: &str,
        now_unix_seconds: u64,
    ) {
        self.audit.record(OperatorAuditEvent {
            operator_id: actor.id().clone(),
            action: audit_action(action),
            project_id: project_id.cloned().unwrap_or_else(global_project_id),
            target: format!("denied:{action}"),
            reason: "state, scope, or step-up validation denied".to_owned(),
            outcome: OperatorAuditOutcome::Denied,
            at_unix_seconds: now_unix_seconds,
        });
    }

    fn allowed_project(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        action: &str,
        now_unix_seconds: u64,
    ) {
        self.audit.record(OperatorAuditEvent {
            operator_id: actor.id().clone(),
            action: audit_action(action),
            project_id: project_id.clone(),
            target: action.to_owned(),
            reason: "operator control center read".to_owned(),
            outcome: OperatorAuditOutcome::Allowed,
            at_unix_seconds: now_unix_seconds,
        });
    }

    fn encode_cursor(
        &self,
        kind: &str,
        key: &[u8],
        filter_digest: &str,
        now_unix_seconds: u64,
    ) -> Result<String, OperatorControlCenterError> {
        let payload = CursorPayload {
            version: 1,
            kind: kind.to_owned(),
            key: URL_SAFE_NO_PAD.encode(key),
            filter_digest: filter_digest.to_owned(),
            expires_at_unix_seconds: now_unix_seconds + CURSOR_TTL_SECONDS,
        };
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?);
        let signature = blake3::keyed_hash(&self.cursor_key, encoded.as_bytes()).to_hex();
        Ok(format!("{encoded}.{signature}"))
    }

    fn decode_cursor(
        &self,
        value: &str,
        kind: &str,
        filter_digest: &str,
        now_unix_seconds: u64,
    ) -> Result<Vec<u8>, OperatorControlCenterError> {
        if value.len() > 4_096 || value.chars().any(char::is_control) {
            return Err(OperatorControlCenterError::InvalidCursor);
        }
        let (encoded, signature) = value
            .split_once('.')
            .ok_or(OperatorControlCenterError::InvalidCursor)?;
        let expected = blake3::keyed_hash(&self.cursor_key, encoded.as_bytes()).to_hex();
        if signature.as_bytes() != expected.as_bytes() {
            return Err(OperatorControlCenterError::InvalidCursor);
        }
        let payload: CursorPayload = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| OperatorControlCenterError::InvalidCursor)?,
        )?;
        if payload.version != 1
            || payload.kind != kind
            || payload.filter_digest != filter_digest
            || payload.expires_at_unix_seconds <= now_unix_seconds
        {
            return Err(OperatorControlCenterError::InvalidCursor);
        }
        URL_SAFE_NO_PAD
            .decode(payload.key)
            .map_err(|_| OperatorControlCenterError::InvalidCursor)
    }
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CursorPayload {
    version: u8,
    kind: String,
    key: String,
    filter_digest: String,
    expires_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActivityRecord {
    pub id: String,
    pub source_event_id: String,
    pub actor_id: String,
    pub action: String,
    pub target: String,
    pub project_id: Option<ProjectId>,
    pub case_reference: Option<String>,
    pub outcome: String,
    pub at_unix_seconds: u64,
    pub integrity: String,
}

impl ActivityRecord {
    #[must_use]
    pub fn computed_integrity(&self) -> String {
        let canonical = serde_json::json!({
            "sourceEventId": self.source_event_id,
            "actorId": self.actor_id,
            "action": self.action,
            "target": self.target,
            "projectId": self.project_id,
            "caseReference": self.case_reference,
            "outcome": self.outcome,
            "atUnixSeconds": self.at_unix_seconds,
        });
        blake3::hash(&serde_json::to_vec(&canonical).expect("activity canonical value serializes"))
            .to_hex()
            .to_string()
    }

    #[must_use]
    pub fn integrity_valid(&self) -> bool {
        self.integrity == self.computed_integrity()
    }
}

fn allows_compat(actor: &OperatorPrincipal, permission: OperatorPermission) -> bool {
    actor.allows(permission)
        || (matches!(
            permission,
            OperatorPermission::OverviewRead
                | OperatorPermission::OperationsRead
                | OperatorPermission::IncidentRead
                | OperatorPermission::BackupRead
                | OperatorPermission::FleetRead
                | OperatorPermission::SecurityRead
                | OperatorPermission::ActivityRead
        ) && actor.allows(OperatorPermission::TenantRead))
}

fn global_project_id() -> ProjectId {
    ProjectId::parse("prj_global00").expect("static global project id is valid")
}

fn audit_action(action: &str) -> OperatorAuditAction {
    if action == "overview" {
        OperatorAuditAction::OverviewRead
    } else if action.starts_with("incident")
        || matches!(action, "acknowledge" | "assign" | "annotate" | "resolve")
    {
        OperatorAuditAction::IncidentManage
    } else if action.starts_with("recovery") {
        OperatorAuditAction::RecoveryManage
    } else if action.starts_with("security") {
        OperatorAuditAction::SecurityManage
    } else if action == "activity_export" {
        OperatorAuditAction::ActivityExport
    } else if action == "activity" {
        OperatorAuditAction::ActivityRead
    } else if action == "projection_rebuild" {
        OperatorAuditAction::ProjectionRebuild
    } else if action == "tenant_directory" || action == "tenant_360" {
        OperatorAuditAction::TenantRead
    } else {
        OperatorAuditAction::InventoryRead
    }
}

fn with_start(
    mut range: KeyRange,
    start: Option<Vec<u8>>,
) -> Result<KeyRange, OperatorControlCenterError> {
    if let Some(mut start) = start {
        if !range.contains(&start) {
            return Err(OperatorControlCenterError::InvalidCursor);
        }
        start.push(0);
        if start >= range.end_exclusive {
            return Err(OperatorControlCenterError::InvalidCursor);
        }
        range.start_inclusive = start;
    }
    Ok(range)
}

fn tenant_matches(
    project: &ProjectRecord,
    organization: Option<&crate::OrganizationRecord>,
    environments: &[crate::EnvironmentRecord],
    search: &TenantSearch,
    query: Option<&str>,
    email_organizations: Option<&BTreeSet<String>>,
) -> bool {
    if search
        .lifecycle
        .is_some_and(|value| value != project.lifecycle())
        || search
            .region
            .as_ref()
            .is_some_and(|value| value != project.region())
        || search
            .health
            .is_some_and(|value| value != lifecycle_freshness(project.lifecycle()))
        || search.plan.as_ref().is_some_and(|value| value != "preview")
        || search
            .quota_state
            .as_ref()
            .is_some_and(|value| value != "unknown")
    {
        return false;
    }
    query.is_none_or(|query| {
        email_organizations
            .is_some_and(|organizations| organizations.contains(project.organization_id().as_str()))
            || [
                project.id().as_str(),
                project.name(),
                project.organization_id().as_str(),
                project.region(),
                organization.map_or("", crate::OrganizationRecord::name),
            ]
            .iter()
            .any(|value| value.to_ascii_lowercase().contains(query))
            || environments.iter().any(|environment| {
                [environment.id().as_str(), environment.name()]
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(query))
            })
    })
}

fn tenant_summary(
    project: &ProjectRecord,
    organization: Option<&crate::OrganizationRecord>,
    environment_count: usize,
) -> TenantSummary {
    TenantSummary {
        project_id: project.id().clone(),
        project_name: project.name().to_owned(),
        organization_id: project.organization_id().as_str().to_owned(),
        organization_name: organization.map(|value| value.name().to_owned()),
        lifecycle: project.lifecycle(),
        region: project.region().to_owned(),
        environment_count,
        health: lifecycle_freshness(project.lifecycle()),
        plan: "preview".to_owned(),
        updated_at_unix_seconds: project.updated_at_unix_seconds(),
    }
}

fn lifecycle_freshness(lifecycle: LifecycleState) -> Freshness {
    match lifecycle {
        LifecycleState::Active => Freshness::Current,
        LifecycleState::Failed | LifecycleState::Deleting => Freshness::Unavailable,
        LifecycleState::Suspended | LifecycleState::DeletionGrace => Freshness::Stale,
        LifecycleState::Provisioning | LifecycleState::Deleted => Freshness::Unknown,
    }
}

fn validate_limit(limit: usize) -> Result<(), OperatorControlCenterError> {
    if (1..=MAX_PAGE).contains(&limit) {
        Ok(())
    } else {
        Err(OperatorControlCenterError::BoundExceeded)
    }
}

fn validate_window(
    from_unix_seconds: u64,
    until_unix_seconds: u64,
    now_unix_seconds: u64,
) -> Result<(), OperatorControlCenterError> {
    if from_unix_seconds > until_unix_seconds
        || until_unix_seconds > now_unix_seconds
        || until_unix_seconds - from_unix_seconds > MAX_TIME_WINDOW_SECONDS
    {
        Err(OperatorControlCenterError::BoundExceeded)
    } else {
        Ok(())
    }
}

fn validate_identifier(value: &str, prefix: &str) -> Result<(), OperatorControlCenterError> {
    if value.strip_prefix(prefix).is_some_and(|suffix| {
        (8..=96).contains(&suffix.len())
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    }) {
        Ok(())
    } else {
        Err(OperatorControlCenterError::InvalidInput)
    }
}

fn validate_sections(
    sections: &mut [OperatorReadSection],
    config: &OperatorControlCenterConfig,
) -> Result<(), OperatorControlCenterError> {
    if sections.len() > 64 {
        return Err(OperatorControlCenterError::BoundExceeded);
    }
    for section in sections {
        section.validate()?;
        section.links.retain(|link| {
            let Ok(url) = url_origin(&link.url) else {
                return false;
            };
            config.allowed_diagnostic_origins.contains(&url)
        });
    }
    Ok(())
}

fn collapse_sections(
    kind: InventoryKind,
    sections: Vec<OperatorReadSection>,
) -> OperatorReadSection {
    let freshness = if sections
        .iter()
        .any(|section| section.freshness == Freshness::Unavailable)
    {
        Freshness::Unavailable
    } else if sections
        .iter()
        .any(|section| section.freshness == Freshness::Stale)
    {
        Freshness::Stale
    } else if sections
        .iter()
        .all(|section| section.freshness == Freshness::Current)
    {
        Freshness::Current
    } else {
        Freshness::Unknown
    };
    let mut metrics = BTreeMap::new();
    metrics.insert("sectionCount".to_owned(), Value::from(sections.len()));
    metrics.insert(
        "unavailableCount".to_owned(),
        Value::from(
            sections
                .iter()
                .filter(|section| section.freshness == Freshness::Unavailable)
                .count(),
        ),
    );
    OperatorReadSection {
        id: kind.as_str().to_owned(),
        freshness,
        observed_at_unix_seconds: sections
            .iter()
            .filter_map(|section| section.observed_at_unix_seconds)
            .min(),
        provider: "aggregate".to_owned(),
        message: (freshness != Freshness::Current)
            .then(|| "One or more scoped sources are stale, unknown, or unavailable.".to_owned()),
        metrics,
        links: sections
            .into_iter()
            .flat_map(|section| section.links)
            .take(16)
            .collect(),
    }
}

fn safe_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn safe_metric_name(value: &str) -> bool {
    safe_text(value, 64)
        && ![
            "secret", "token", "password", "document", "selector", "rawEmail", "key", "value",
        ]
        .iter()
        .any(|blocked| {
            value
                .to_ascii_lowercase()
                .contains(&blocked.to_ascii_lowercase())
        })
}

fn safe_value(value: &Value, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(value) => safe_text(value, 1_024),
        Value::Array(values) => {
            values.len() <= 100 && values.iter().all(|value| safe_value(value, depth + 1))
        }
        Value::Object(values) => {
            values.len() <= 64
                && values
                    .iter()
                    .all(|(name, value)| safe_metric_name(name) && safe_value(value, depth + 1))
        }
    }
}

fn safe_diagnostic_url(value: &str) -> bool {
    value.starts_with("https://")
        && value.len() <= 2_048
        && !value.contains('@')
        && !value.contains("..")
        && !value.chars().any(char::is_control)
}

fn url_origin(value: &str) -> Result<String, OperatorControlCenterError> {
    if !safe_diagnostic_url(value) {
        return Err(OperatorControlCenterError::UnsafeResponse);
    }
    let after_scheme = value
        .strip_prefix("https://")
        .ok_or(OperatorControlCenterError::UnsafeResponse)?;
    let authority = after_scheme.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains(['?', '#', '@']) {
        return Err(OperatorControlCenterError::UnsafeResponse);
    }
    Ok(format!("https://{authority}"))
}

fn valid_recovery_transition(previous: RecoveryJobState, next: RecoveryJobState) -> bool {
    matches!(
        (previous, next),
        (RecoveryJobState::Requested, RecoveryJobState::Preparing)
            | (RecoveryJobState::Preparing, RecoveryJobState::Restoring)
            | (RecoveryJobState::Restoring, RecoveryJobState::Verifying)
            | (
                RecoveryJobState::Verifying,
                RecoveryJobState::PromotionReady
            )
            | (RecoveryJobState::PromotionReady, RecoveryJobState::Promoted)
            | (_, RecoveryJobState::Failed)
            | (
                RecoveryJobState::Requested
                    | RecoveryJobState::Preparing
                    | RecoveryJobState::Restoring
                    | RecoveryJobState::Verifying
                    | RecoveryJobState::PromotionReady,
                RecoveryJobState::Cancelled
            )
    )
}

#[derive(Debug)]
pub enum OperatorControlCenterError {
    UnsupportedDurability,
    InvalidInput,
    InvalidCursor,
    InvalidState,
    BoundExceeded,
    UnsafeResponse,
    StepUpRequired,
    StaleVersion,
    FeatureDisabled,
    Forbidden,
    NotFound,
    Conflict,
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Developer(DeveloperRegistrationError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for OperatorControlCenterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedDurability => "operator control-center durability is unsupported",
            Self::InvalidInput => "operator control-center request is invalid",
            Self::InvalidCursor => "operator control-center cursor is invalid or expired",
            Self::InvalidState => "operator control-center resource state is invalid",
            Self::BoundExceeded => "operator control-center request exceeds a safety bound",
            Self::UnsafeResponse => "operator control-center provider response is unsafe",
            Self::StepUpRequired => {
                "operator password verification is stale or action-bound proof is invalid"
            }
            Self::StaleVersion => "operator resource changed since it was reviewed",
            Self::FeatureDisabled => "operator workflow is disabled by a feature gate",
            Self::Forbidden => "operator control-center action is forbidden",
            Self::NotFound => "operator control-center resource was not found",
            Self::Conflict => "operator control-center resource changed concurrently",
            Self::Project(_) => "operator project read failed",
            Self::Organization(_) => "operator organization read failed",
            Self::Developer(_) => "operator developer lookup failed",
            Self::Keyspace(_) => "operator control-center key is invalid",
            Self::Storage(_) => "operator control-center storage failed",
            Self::Json(_) => "operator control-center record is invalid",
        })
    }
}

impl Error for OperatorControlCenterError {}

macro_rules! error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for OperatorControlCenterError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(ProjectStoreError, Project);
error_from!(OrganizationStoreError, Organization);
error_from!(DeveloperRegistrationError, Developer);
error_from!(ControlKeyspaceError, Keyspace);
error_from!(StorageError, Storage);
error_from!(serde_json::Error, Json);

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicU8, Ordering},
    };

    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{OperatorId, OrganizationId};

    #[derive(Default)]
    struct Audit(Mutex<Vec<OperatorAuditEvent>>);

    impl OperatorAuditSink for Audit {
        fn record(&self, event: OperatorAuditEvent) {
            self.0.lock().expect("audit lock").push(event);
        }
    }

    #[derive(Default)]
    struct FakeProvider {
        fail: bool,
    }

    #[async_trait]
    impl OperatorProvider for FakeProvider {
        async fn overview(
            &self,
            now: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            if self.fail {
                return Err(OperatorProviderError::Unavailable);
            }
            Ok(vec![section("service-readiness", now)])
        }

        async fn tenant(
            &self,
            _tenant: &TenantScope,
            kind: InventoryKind,
            _from: u64,
            until: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            if self.fail {
                return Err(OperatorProviderError::Unavailable);
            }
            Ok(vec![section(kind.as_str(), until)])
        }

        async fn global(
            &self,
            kind: InventoryKind,
            _from: u64,
            until: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            self.tenant(
                &TenantScope::new(
                    ProjectId::parse("prj_example00").expect("project"),
                    mako_api::EnvironmentId::parse("env_example00").expect("environment"),
                ),
                kind,
                0,
                until,
            )
            .await
        }
    }

    fn section(id: &str, now: u64) -> OperatorReadSection {
        OperatorReadSection {
            id: id.to_owned(),
            freshness: Freshness::Current,
            observed_at_unix_seconds: Some(now),
            provider: "test".to_owned(),
            message: None,
            metrics: BTreeMap::from([("count".to_owned(), Value::from(1))]),
            links: Vec::new(),
        }
    }

    fn service(
        provider: FakeProvider,
    ) -> (OperatorControlCenterService, MemoryAdapter, Arc<Audit>) {
        service_with_provider(Arc::new(provider))
    }

    fn service_with_provider(
        provider: Arc<dyn OperatorProvider>,
    ) -> (OperatorControlCenterService, MemoryAdapter, Arc<Audit>) {
        let adapter = MemoryAdapter::default();
        let shared: Arc<dyn KvAdapter> = Arc::new(adapter.clone());
        let audit = Arc::new(Audit::default());
        let service = OperatorControlCenterService::new(
            Arc::clone(&shared),
            Durability::Memory,
            ProjectStore::new(Arc::clone(&shared), Durability::Memory).expect("projects"),
            OrganizationStore::new(Arc::clone(&shared), Durability::Memory).expect("organizations"),
            DeveloperRegistrationStore::new(
                Arc::clone(&shared),
                Durability::Memory,
                crate::DeveloperLookupKey::derive(b"operator-control-center-test-key"),
            )
            .expect("developers"),
            provider,
            audit.clone(),
            [7; 32],
            OperatorControlCenterConfig::default(),
        )
        .expect("service");
        (service, adapter, audit)
    }

    fn actor(permissions: impl IntoIterator<Item = OperatorPermission>) -> OperatorPrincipal {
        OperatorPrincipal::for_test(
            OperatorId::parse("opr_example00").expect("operator"),
            permissions,
        )
    }

    #[test]
    fn permissions_are_least_privilege() {
        let observer = crate::OperatorRole::Observer.permissions();
        assert!(observer.contains(&OperatorPermission::OverviewRead));
        assert!(!observer.contains(&OperatorPermission::IncidentManage));
        assert!(!observer.contains(&OperatorPermission::RecoveryManage));
        assert!(!observer.contains(&OperatorPermission::ActivityExport));
    }

    #[test]
    fn redaction_rejects_secrets_document_fields_and_unsafe_urls() {
        let mut unsafe_section = section("health", 10);
        unsafe_section
            .metrics
            .insert("accessToken".to_owned(), Value::String("secret".to_owned()));
        assert!(matches!(
            unsafe_section.validate(),
            Err(OperatorControlCenterError::UnsafeResponse)
        ));
        unsafe_section.metrics.clear();
        unsafe_section.links.push(SafeDiagnosticLink {
            label: "logs".to_owned(),
            kind: "logs".to_owned(),
            url: "https://user:password@example.test/logs".to_owned(),
        });
        assert!(matches!(
            unsafe_section.validate(),
            Err(OperatorControlCenterError::UnsafeResponse)
        ));
    }

    #[test]
    fn cursor_is_scope_bound_expiring_and_tamper_evident() {
        let (service, _, _) = service(FakeProvider::default());
        let cursor = service
            .encode_cursor("tenants", b"safe-key", "filter-a", 100)
            .expect("cursor");
        assert_eq!(
            service
                .decode_cursor(&cursor, "tenants", "filter-a", 101)
                .expect("decode"),
            b"safe-key"
        );
        assert!(
            service
                .decode_cursor(&cursor, "incidents", "filter-a", 101)
                .is_err()
        );
        assert!(
            service
                .decode_cursor(&cursor, "tenants", "filter-b", 101)
                .is_err()
        );
        assert!(
            service
                .decode_cursor(&cursor, "tenants", "filter-a", 1_001)
                .is_err()
        );
        let mut tampered = cursor;
        tampered.push('x');
        assert!(
            service
                .decode_cursor(&tampered, "tenants", "filter-a", 101)
                .is_err()
        );
    }

    #[test]
    fn guarded_mutation_binds_action_target_version_and_freshness() {
        let guard = GuardedMutation {
            operation_key: "operation-0001".to_owned(),
            reviewed_version: 4,
            reason: "Customer-impacting incident response".to_owned(),
            case_reference: None,
            confirmation: "inc_example00".to_owned(),
            action_binding: GuardedMutation::action_binding("resolve", "inc_example00", 4),
            password_verified_at_unix_seconds: 900,
        };
        assert!(guard.validate("resolve", "inc_example00", 1_000).is_ok());
        assert!(matches!(
            guard.validate("assign", "inc_example00", 1_000),
            Err(OperatorControlCenterError::StepUpRequired)
        ));
        assert!(matches!(
            guard.validate("resolve", "inc_example00", 1_300),
            Err(OperatorControlCenterError::StepUpRequired)
        ));
    }

    #[test]
    fn provider_failure_is_explicitly_unavailable_not_healthy() {
        let (service, _, _) = service(FakeProvider { fail: true });
        let view = futures::executor::block_on(
            service.overview(&actor([OperatorPermission::TenantRead]), 1_000),
        )
        .expect("overview");
        assert!(view.partial);
        assert_eq!(view.sections[0].freshness, Freshness::Unavailable);
    }

    #[test]
    fn tenant_inventory_is_bounded_even_for_non_matching_search() {
        let (service, _, _) = service(FakeProvider::default());
        let organization = crate::OrganizationRecord::new(
            OrganizationId::parse("org_example00").expect("organization"),
            "Example",
            1,
        )
        .expect("organization");
        let owner = crate::MembershipRecord::new(
            organization.id().clone(),
            crate::DeveloperIdentityId::parse("dev_example00").expect("developer"),
            crate::OrganizationRole::Owner,
            1,
        );
        futures::executor::block_on(
            service
                .organizations
                .create_organization(&organization, &owner),
        )
        .expect("create organization");
        for index in 0..1_000 {
            let project = ProjectRecord::new(
                ProjectId::parse(format!("prj_{index:08}")).expect("project"),
                organization.id().clone(),
                format!("Tenant {index}"),
                "us-east-1",
                1,
            )
            .expect("project");
            futures::executor::block_on(service.projects.create_project(&project))
                .expect("create project");
        }
        let page = futures::executor::block_on(service.tenants(
            &actor([OperatorPermission::TenantRead]),
            &TenantSearch {
                query: Some("does-not-exist".to_owned()),
                ..TenantSearch::default()
            },
            None,
            25,
            100,
        ))
        .expect("page");
        assert!(page.items.is_empty());
        assert!(page.examined_records <= 201);
        assert!(page.next_cursor.is_some());
    }

    #[test]
    fn tenant_search_normalizes_exact_email_and_matches_environment_identifiers() {
        let (service, _, _) = service(FakeProvider::default());
        let developer_id = crate::DeveloperIdentityId::parse("dev_example00").expect("developer");
        let identity = crate::AuthenticationIdentityRecord::new_verified(
            developer_id.clone(),
            NormalizedEmail::parse("Owner@Example.Test").expect("email"),
            "Owner",
            "$argon2id$test-password-hash-that-is-long-enough",
            1,
        )
        .expect("identity");
        futures::executor::block_on(service.developers.create_authentication_identity(&identity))
            .expect("create identity");
        let organization = crate::OrganizationRecord::new(
            OrganizationId::parse("org_example00").expect("organization"),
            "Example",
            1,
        )
        .expect("organization");
        let owner = crate::MembershipRecord::new(
            organization.id().clone(),
            developer_id,
            crate::OrganizationRole::Owner,
            1,
        );
        futures::executor::block_on(
            service
                .organizations
                .create_organization(&organization, &owner),
        )
        .expect("create organization");
        let project = ProjectRecord::new(
            ProjectId::parse("prj_example00").expect("project"),
            organization.id().clone(),
            "Tenant",
            "us-east-1",
            1,
        )
        .expect("project");
        futures::executor::block_on(service.projects.create_project(&project))
            .expect("create project");
        futures::executor::block_on(
            service.projects.create_environment(
                &crate::EnvironmentRecord::new(
                    mako_api::EnvironmentId::parse("env_example00").expect("environment"),
                    project.id().clone(),
                    "Production Primary",
                    1,
                )
                .expect("environment"),
            ),
        )
        .expect("create environment");

        for query in ["OWNER@example.test", "production primary", "env_example00"] {
            let page = futures::executor::block_on(service.tenants(
                &actor([OperatorPermission::TenantRead]),
                &TenantSearch {
                    query: Some(query.to_owned()),
                    ..TenantSearch::default()
                },
                None,
                25,
                100,
            ))
            .expect("tenant search");
            assert_eq!(page.items.len(), 1, "query {query}");
            assert_eq!(page.items[0].project_id, *project.id());
        }
    }

    #[derive(Default)]
    struct ScopeProvider {
        tenants: Mutex<Vec<TenantScope>>,
        global_calls: Mutex<usize>,
    }

    #[async_trait]
    impl OperatorProvider for ScopeProvider {
        async fn overview(
            &self,
            now: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            Ok(vec![section("overview", now)])
        }

        async fn tenant(
            &self,
            tenant: &TenantScope,
            kind: InventoryKind,
            _from: u64,
            until: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            self.tenants
                .lock()
                .expect("tenant calls")
                .push(tenant.clone());
            Ok(vec![section(kind.as_str(), until)])
        }

        async fn global(
            &self,
            kind: InventoryKind,
            _from: u64,
            until: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            *self.global_calls.lock().expect("global calls") += 1;
            Ok(vec![section(kind.as_str(), until)])
        }
    }

    #[test]
    fn project_inventory_calls_only_exact_tenant_provider_scopes() {
        let provider = Arc::new(ScopeProvider::default());
        let (service, _, _) = service_with_provider(provider.clone());
        let organization = crate::OrganizationRecord::new(
            OrganizationId::parse("org_example00").expect("organization"),
            "Example",
            1,
        )
        .expect("organization");
        let owner = crate::MembershipRecord::new(
            organization.id().clone(),
            crate::DeveloperIdentityId::parse("dev_example00").expect("developer"),
            crate::OrganizationRole::Owner,
            1,
        );
        futures::executor::block_on(
            service
                .organizations
                .create_organization(&organization, &owner),
        )
        .expect("create organization");
        let project = ProjectRecord::new(
            ProjectId::parse("prj_example00").expect("project"),
            organization.id().clone(),
            "Tenant",
            "us-east-1",
            1,
        )
        .expect("project");
        futures::executor::block_on(service.projects.create_project(&project))
            .expect("create project");
        for id in ["env_example00", "env_example01"] {
            futures::executor::block_on(
                service.projects.create_environment(
                    &crate::EnvironmentRecord::new(
                        mako_api::EnvironmentId::parse(id).expect("environment"),
                        project.id().clone(),
                        id,
                        1,
                    )
                    .expect("environment"),
                ),
            )
            .expect("create environment");
        }

        let sections = futures::executor::block_on(service.inventory(
            &actor([OperatorPermission::FleetRead]),
            InventoryKind::Sync,
            Some(project.id()),
            1,
            100,
            100,
        ))
        .expect("inventory");
        assert_eq!(sections.len(), 2);
        assert_eq!(*provider.global_calls.lock().expect("global calls"), 0);
        let scopes = provider.tenants.lock().expect("tenant calls");
        assert_eq!(scopes.len(), 2);
        assert!(
            scopes
                .iter()
                .all(|scope| scope.project_id() == project.id())
        );
    }

    #[test]
    fn tenant_pagination_remains_stable_when_a_project_is_inserted_between_pages() {
        let (service, _, _) = service(FakeProvider::default());
        let organization = crate::OrganizationRecord::new(
            OrganizationId::parse("org_example00").expect("organization"),
            "Example",
            1,
        )
        .expect("organization");
        let owner = crate::MembershipRecord::new(
            organization.id().clone(),
            crate::DeveloperIdentityId::parse("dev_example00").expect("developer"),
            crate::OrganizationRole::Owner,
            1,
        );
        futures::executor::block_on(
            service
                .organizations
                .create_organization(&organization, &owner),
        )
        .expect("create organization");
        for index in (0..30).step_by(2) {
            let project = ProjectRecord::new(
                ProjectId::parse(format!("prj_{index:08}")).expect("project"),
                organization.id().clone(),
                format!("Tenant {index}"),
                "us-east-1",
                1,
            )
            .expect("project");
            futures::executor::block_on(service.projects.create_project(&project))
                .expect("create project");
        }
        let operator = actor([OperatorPermission::TenantRead]);
        let first = futures::executor::block_on(service.tenants(
            &operator,
            &TenantSearch::default(),
            None,
            5,
            100,
        ))
        .expect("first page");
        let inserted = ProjectRecord::new(
            ProjectId::parse("prj_00000009").expect("project"),
            organization.id().clone(),
            "Inserted tenant",
            "us-east-1",
            2,
        )
        .expect("project");
        futures::executor::block_on(service.projects.create_project(&inserted))
            .expect("insert project");
        let second = futures::executor::block_on(service.tenants(
            &operator,
            &TenantSearch::default(),
            first.next_cursor.as_deref(),
            5,
            101,
        ))
        .expect("second page");
        let first_ids = first
            .items
            .iter()
            .map(|item| &item.project_id)
            .collect::<BTreeSet<_>>();
        assert!(
            second
                .items
                .iter()
                .all(|item| !first_ids.contains(&item.project_id))
        );
        assert!(ProjectId::parse("../../another-tenant").is_err());
    }

    #[test]
    fn denied_read_is_safe_and_durably_audited() {
        let (service, _, audit) = service(FakeProvider::default());
        let error = futures::executor::block_on(service.overview(&actor([]), 100))
            .expect_err("permission is required");
        assert!(matches!(error, OperatorControlCenterError::Forbidden));
        let events = audit.0.lock().expect("audit events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome, OperatorAuditOutcome::Denied);
        assert_eq!(events[0].target, "denied:overview");
    }

    #[test]
    fn tenant_search_backfill_resumes_and_records_count_checksum_evidence() {
        let (service, _, _) = service(FakeProvider::default());
        let organization = crate::OrganizationRecord::new(
            OrganizationId::parse("org_example00").expect("organization"),
            "Example",
            1,
        )
        .expect("organization");
        let owner = crate::MembershipRecord::new(
            organization.id().clone(),
            crate::DeveloperIdentityId::parse("dev_example00").expect("developer"),
            crate::OrganizationRole::Owner,
            1,
        );
        futures::executor::block_on(
            service
                .organizations
                .create_organization(&organization, &owner),
        )
        .expect("create organization");
        for index in 0..225 {
            let project = ProjectRecord::new(
                ProjectId::parse(format!("prj_{index:08}")).expect("project"),
                organization.id().clone(),
                format!("Tenant {index}"),
                "us-east-1",
                1,
            )
            .expect("project");
            futures::executor::block_on(service.projects.create_project(&project))
                .expect("create project");
        }
        let operator = actor([OperatorPermission::SecurityManage]);
        let mut cursor = None;
        let final_evidence = loop {
            let evidence = futures::executor::block_on(service.rebuild_projection(
                &operator,
                "tenant_search",
                cursor.as_deref(),
                100,
            ))
            .expect("backfill page");
            if evidence.complete {
                break evidence;
            }
            assert!(evidence.projected_count <= 200);
            cursor = evidence.cursor;
        };
        assert_eq!(final_evidence.source_count, 225);
        assert_eq!(final_evidence.projected_count, 225);
        assert_eq!(final_evidence.checksum.len(), 64);
        let records = futures::executor::block_on(service.scan_records::<TenantSearchIndexRecord>(
            ControlKeyspace::operator_tenant_search_range().expect("range"),
            226,
        ))
        .expect("indexes");
        assert_eq!(records.len(), 225);
        let replay = futures::executor::block_on(service.rebuild_projection(
            &operator,
            "tenant_search",
            None,
            101,
        ))
        .expect("idempotent replay");
        assert_eq!(replay, final_evidence);
    }

    fn guard(action: &str, target: &str, version: u64, operation: &str) -> GuardedMutation {
        GuardedMutation {
            operation_key: operation.to_owned(),
            reviewed_version: version,
            reason: "Reviewed incident response action".to_owned(),
            case_reference: Some("case-example".to_owned()),
            confirmation: target.to_owned(),
            action_binding: GuardedMutation::action_binding(action, target, version),
            password_verified_at_unix_seconds: 900,
        }
    }

    #[test]
    fn incident_updates_reject_stale_tabs_and_preserve_ordered_history() {
        let (service, _, audit) = service(FakeProvider::default());
        let operator = actor([
            OperatorPermission::IncidentManage,
            OperatorPermission::IncidentRead,
        ]);
        let id = "inc_example00";
        let created = futures::executor::block_on(service.create_incident(
            &operator,
            id.to_owned(),
            "alr_example00".to_owned(),
            "Example incident".to_owned(),
            "high".to_owned(),
            None,
            &guard("incident_create", id, 0, "operation-create"),
            1_000,
        ))
        .expect("create incident");
        assert_eq!(created.version, 1);
        let acknowledged = futures::executor::block_on(service.update_incident(
            &operator,
            id,
            "acknowledge",
            None,
            None,
            &guard("acknowledge", id, 1, "operation-ack"),
            1_001,
        ))
        .expect("acknowledge");
        assert_eq!(acknowledged.state, IncidentState::Acknowledged);
        let stale = futures::executor::block_on(service.update_incident(
            &operator,
            id,
            "resolve",
            None,
            None,
            &guard("resolve", id, 1, "operation-stale"),
            1_002,
        ))
        .expect_err("stale version");
        assert!(matches!(stale, OperatorControlCenterError::StaleVersion));
        assert!(
            audit
                .0
                .lock()
                .expect("audit events")
                .iter()
                .any(|event| event.outcome == OperatorAuditOutcome::Denied)
        );
        let assigned = futures::executor::block_on(service.update_incident(
            &operator,
            id,
            "assign",
            Some("on-call".to_owned()),
            None,
            &guard("assign", id, 2, "operation-assign"),
            1_003,
        ))
        .expect("assign");
        let annotated = futures::executor::block_on(service.update_incident(
            &operator,
            id,
            "annotate",
            None,
            Some("Scoped source checked".to_owned()),
            &guard("annotate", id, 3, "operation-note"),
            1_004,
        ))
        .expect("annotate");
        let resolved = futures::executor::block_on(service.update_incident(
            &operator,
            id,
            "resolve",
            None,
            None,
            &guard("resolve", id, 4, "operation-resolve"),
            1_005,
        ))
        .expect("resolve");
        assert_eq!(assigned.timeline.len(), 3);
        assert_eq!(annotated.timeline.len(), 4);
        assert_eq!(resolved.timeline.len(), 5);
        assert_eq!(resolved.state, IncidentState::Resolved);
        assert_eq!(
            resolved
                .timeline
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5],
        );
    }

    #[derive(Default)]
    struct MutableAlertProvider(AtomicU8);

    #[async_trait]
    impl OperatorProvider for MutableAlertProvider {
        async fn overview(
            &self,
            now: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            Ok(vec![section("overview", now)])
        }

        async fn tenant(
            &self,
            _tenant: &TenantScope,
            kind: InventoryKind,
            _from: u64,
            until: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            self.global(kind, 0, until).await
        }

        async fn global(
            &self,
            kind: InventoryKind,
            _from: u64,
            until: u64,
        ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
            match self.0.load(Ordering::Relaxed) {
                2 => Err(OperatorProviderError::Unavailable),
                state => {
                    let mut value = section(kind.as_str(), until);
                    if state == 1 {
                        value.freshness = Freshness::Stale;
                    }
                    Ok(vec![value])
                }
            }
        }
    }

    #[test]
    fn alert_fingerprints_are_stable_and_disappearance_does_not_resolve_incidents() {
        let provider = Arc::new(MutableAlertProvider::default());
        provider.0.store(1, Ordering::Relaxed);
        let (service, _, _) = service_with_provider(provider.clone());
        let operator = actor([
            OperatorPermission::IncidentRead,
            OperatorPermission::IncidentManage,
        ]);
        let first =
            futures::executor::block_on(service.current_alerts(&operator, None, None, 25, 1_000))
                .expect("alerts");
        let repeated =
            futures::executor::block_on(service.current_alerts(&operator, None, None, 25, 1_001))
                .expect("alerts repeated");
        assert_eq!(first.items[0].fingerprint, repeated.items[0].fingerprint);
        let id = "inc_example00";
        futures::executor::block_on(service.create_incident(
            &operator,
            id.to_owned(),
            first.items[0].fingerprint.clone(),
            "Provider alert".to_owned(),
            "high".to_owned(),
            None,
            &guard("incident_create", id, 0, "operation-provider-incident"),
            1_001,
        ))
        .expect("incident");
        provider.0.store(0, Ordering::Relaxed);
        assert!(
            futures::executor::block_on(service.current_alerts(&operator, None, None, 25, 1_002,))
                .expect("resolved provider")
                .items
                .is_empty()
        );
        assert_eq!(
            futures::executor::block_on(service.incident(&operator, id, 1_002))
                .expect("incident remains")
                .state,
            IncidentState::Open,
        );
        provider.0.store(2, Ordering::Relaxed);
        let unavailable =
            futures::executor::block_on(service.current_alerts(&operator, None, None, 25, 1_003))
                .expect("unavailable alert");
        assert_eq!(unavailable.items[0].freshness, Freshness::Unavailable);
    }

    #[derive(Default)]
    struct RecoveryFake {
        calls: Mutex<Vec<String>>,
        fail_restore: bool,
        fail_verify: bool,
    }

    #[async_trait]
    impl RecoveryExecutor for RecoveryFake {
        async fn prepare(&self, _job: &RecoveryJobRecord) -> Result<(), RecoveryExecutorError> {
            self.calls.lock().expect("calls").push("prepare".to_owned());
            Ok(())
        }

        async fn restore(&self, _job: &RecoveryJobRecord) -> Result<(), RecoveryExecutorError> {
            self.calls.lock().expect("calls").push("restore".to_owned());
            if self.fail_restore {
                Err(RecoveryExecutorError::RestoreFailed)
            } else {
                Ok(())
            }
        }

        async fn verify(&self, _job: &RecoveryJobRecord) -> Result<(), RecoveryExecutorError> {
            self.calls.lock().expect("calls").push("verify".to_owned());
            if self.fail_verify {
                Err(RecoveryExecutorError::VerificationFailed)
            } else {
                Ok(())
            }
        }

        async fn promote(&self, _job: &RecoveryJobRecord) -> Result<(), RecoveryExecutorError> {
            self.calls.lock().expect("calls").push("promote".to_owned());
            Ok(())
        }
    }

    fn recovery_service() -> OperatorControlCenterService {
        let (mut service, _, _) = service(FakeProvider::default());
        service.config.recovery_creation_enabled = true;
        service.config.recovery_promotion_enabled = true;
        let project = ProjectRecord::new(
            ProjectId::parse("prj_example00").expect("project"),
            OrganizationId::parse("org_example00").expect("organization"),
            "Tenant",
            "us-east-1",
            1,
        )
        .expect("project");
        futures::executor::block_on(service.projects.create_project(&project))
            .expect("create project");
        service
    }

    fn recovery_request(verified: bool) -> RecoveryRequest {
        RecoveryRequest {
            id: "rcv_example00".to_owned(),
            project_id: ProjectId::parse("prj_example00").expect("project"),
            backup_id: "backup_example00".to_owned(),
            target: "isolated-recovery-environment".to_owned(),
            backup_verified: verified,
            impact_preview: "Creates an isolated restore target without overwriting production."
                .to_owned(),
            guard: guard(
                "recovery_create",
                "rcv_example00",
                0,
                "operation-recovery-create",
            ),
        }
    }

    #[test]
    fn recovery_executor_rejects_bad_evidence_replay_and_failed_verification() {
        let service = recovery_service();
        let operator = actor([
            OperatorPermission::RecoveryManage,
            OperatorPermission::BackupRead,
        ]);
        assert!(matches!(
            futures::executor::block_on(service.create_recovery_job(
                &operator,
                recovery_request(false),
                1_000,
            )),
            Err(OperatorControlCenterError::InvalidInput)
        ));
        let created = futures::executor::block_on(service.create_recovery_job(
            &operator,
            recovery_request(true),
            1_000,
        ))
        .expect("create recovery");
        let executor = RecoveryFake {
            fail_verify: true,
            ..RecoveryFake::default()
        };
        let preparing = futures::executor::block_on(service.execute_recovery_step(
            &operator,
            &created.id,
            &executor,
            &guard("recovery_execute", &created.id, 1, "operation-prepare"),
            1_001,
        ))
        .expect("prepare");
        assert_eq!(preparing.state, RecoveryJobState::Preparing);
        assert!(matches!(
            futures::executor::block_on(service.execute_recovery_step(
                &operator,
                &created.id,
                &executor,
                &guard("recovery_execute", &created.id, 1, "operation-prepare"),
                1_001,
            )),
            Err(OperatorControlCenterError::StaleVersion)
        ));
        let restoring = futures::executor::block_on(service.execute_recovery_step(
            &operator,
            &created.id,
            &executor,
            &guard("recovery_execute", &created.id, 2, "operation-restore"),
            1_002,
        ))
        .expect("restore");
        let verifying = futures::executor::block_on(service.execute_recovery_step(
            &operator,
            &created.id,
            &executor,
            &guard("recovery_execute", &created.id, 3, "operation-verifying"),
            1_003,
        ))
        .expect("verifying");
        assert_eq!(restoring.state, RecoveryJobState::Restoring);
        assert_eq!(verifying.state, RecoveryJobState::Verifying);
        let failed = futures::executor::block_on(service.execute_recovery_step(
            &operator,
            &created.id,
            &executor,
            &guard("recovery_execute", &created.id, 4, "operation-verify"),
            1_004,
        ))
        .expect("failure is durable");
        assert_eq!(failed.state, RecoveryJobState::Failed);
        assert!(!failed.verification_succeeded);
        assert_eq!(failed.backup_id, "backup_example00");
        assert_eq!(failed.target, "isolated-recovery-environment");
        assert_eq!(
            executor.calls.lock().expect("calls").as_slice(),
            ["prepare", "restore", "verify"],
        );
    }

    #[test]
    fn interrupted_restore_is_failed_durably_without_losing_recovery_target() {
        let service = recovery_service();
        let operator = actor([
            OperatorPermission::RecoveryManage,
            OperatorPermission::BackupRead,
        ]);
        let created = futures::executor::block_on(service.create_recovery_job(
            &operator,
            recovery_request(true),
            1_000,
        ))
        .expect("create recovery");
        let executor = RecoveryFake {
            fail_restore: true,
            ..RecoveryFake::default()
        };
        let prepared = futures::executor::block_on(service.execute_recovery_step(
            &operator,
            &created.id,
            &executor,
            &guard("recovery_execute", &created.id, 1, "operation-prepare"),
            1_001,
        ))
        .expect("prepare");
        let failed = futures::executor::block_on(service.execute_recovery_step(
            &operator,
            &created.id,
            &executor,
            &guard("recovery_execute", &created.id, 2, "operation-restore"),
            1_002,
        ))
        .expect("durable failure");
        assert_eq!(prepared.state, RecoveryJobState::Preparing);
        assert_eq!(failed.state, RecoveryJobState::Failed);
        assert_eq!(failed.last_error_class.as_deref(), Some("restore_failed"));
        assert_eq!(failed.target, "isolated-recovery-environment");
        assert_eq!(failed.backup_id, "backup_example00");
    }

    #[test]
    fn recovery_promotion_requires_successful_verification_and_its_separate_gate() {
        let mut service = recovery_service();
        let operator = actor([
            OperatorPermission::RecoveryManage,
            OperatorPermission::BackupRead,
        ]);
        let created = futures::executor::block_on(service.create_recovery_job(
            &operator,
            recovery_request(true),
            1_000,
        ))
        .expect("create recovery");
        let executor = RecoveryFake::default();
        let mut job = created;
        for (version, operation, now) in [
            (1, "operation-prepare", 1_001),
            (2, "operation-restore", 1_002),
            (3, "operation-verifying", 1_003),
            (4, "operation-verify", 1_004),
        ] {
            job = futures::executor::block_on(service.execute_recovery_step(
                &operator,
                &job.id,
                &executor,
                &guard("recovery_execute", &job.id, version, operation),
                now,
            ))
            .expect("approved step");
        }
        assert_eq!(job.state, RecoveryJobState::PromotionReady);
        assert!(job.verification_succeeded);
        service.config.recovery_promotion_enabled = false;
        assert!(matches!(
            futures::executor::block_on(service.execute_recovery_step(
                &operator,
                &job.id,
                &executor,
                &guard("recovery_execute", &job.id, 5, "operation-promote-blocked"),
                1_005,
            )),
            Err(OperatorControlCenterError::FeatureDisabled)
        ));
        assert_eq!(
            executor
                .calls
                .lock()
                .expect("calls")
                .last()
                .map(String::as_str),
            Some("verify")
        );
    }

    #[test]
    fn narrow_provider_interfaces_are_deterministic_and_inventory_specific() {
        let provider = FakeProvider::default();
        let alerts = futures::executor::block_on(provider.current_alert_sections(10, 20))
            .expect("alert sections");
        let fleet = futures::executor::block_on(provider.fleet_health_sections(10, 20))
            .expect("fleet sections");
        let backups = futures::executor::block_on(provider.backup_evidence_sections(10, 20))
            .expect("backup sections");
        assert_eq!(alerts[0].id, "alerts");
        assert_eq!(fleet[0].id, "fleet");
        assert_eq!(backups[0].id, "backups");
        assert_eq!(alerts[0].observed_at_unix_seconds, Some(20));
    }

    #[test]
    fn activity_projection_deduplicates_validates_integrity_and_exports_expire() {
        let (service, adapter, _) = service(FakeProvider::default());
        let operator = actor([
            OperatorPermission::ActivityRead,
            OperatorPermission::ActivityExport,
        ]);
        let activity_guard = guard("test_activity", "target-example", 0, "operation-activity");
        futures::executor::block_on(service.append_activity(
            &operator,
            "test_activity",
            "target-example",
            None,
            "allowed",
            &activity_guard,
            1_000,
        ))
        .expect("first projection");
        futures::executor::block_on(service.append_activity(
            &operator,
            "test_activity",
            "target-example",
            None,
            "allowed",
            &activity_guard,
            1_000,
        ))
        .expect("duplicate projection is idempotent");

        let activity = futures::executor::block_on(service.activity(
            &operator,
            None,
            25,
            None,
            Some(900),
            Some(1_100),
            1_100,
        ))
        .expect("activity");
        assert_eq!(activity.items.len(), 1);
        assert!(activity.items[0].integrity_valid());

        let export_guard = guard(
            "activity_export",
            "exp_example00",
            0,
            "operation-activity-export",
        );
        let pending = futures::executor::block_on(service.create_activity_export(
            &operator,
            "exp_example00".to_owned(),
            &serde_json::json!({"query": "test_activity"}),
            &export_guard,
            1_100,
        ))
        .expect("pending export");
        assert_eq!(pending.state, ActivityExportState::Pending);
        let ready = futures::executor::block_on(service.process_activity_export(
            &operator,
            "exp_example00",
            1_101,
        ))
        .expect("processed export");
        assert_eq!(ready.state, ActivityExportState::Ready);
        assert_eq!(ready.record_count, 1);
        assert_eq!(ready.checksum.as_deref().map(str::len), Some(64));
        assert_eq!(
            futures::executor::block_on(service.activity_export(
                &operator,
                "exp_example00",
                ready.expires_at_unix_seconds,
            ))
            .expect("expired export")
            .state,
            ActivityExportState::Expired,
        );

        let activity_key = ControlKeyspace::operator_activity_key(1_000_000, &activity.items[0].id)
            .expect("activity key");
        let mut corrupt = activity.items[0].clone();
        corrupt.target = "tampered-target".to_owned();
        let mut batch = WriteBatch::new();
        batch.put(
            &activity_key,
            serde_json::to_vec(&corrupt).expect("corrupt record"),
        );
        futures::executor::block_on(adapter.write(batch, Durability::Memory))
            .expect("write corrupt projection");
        assert!(matches!(
            futures::executor::block_on(service.activity(
                &operator,
                None,
                25,
                None,
                Some(900),
                Some(1_100),
                1_100,
            )),
            Err(OperatorControlCenterError::UnsafeResponse)
        ));
    }
}
