use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use async_trait::async_trait;
use mako_api::{EnvironmentId, ProjectId};
use mako_provisioning::{
    OperatorRepairAction, Provisioner, ProvisioningResource, ProvisioningWorkflow,
    ProvisioningWorkflowError, ProvisioningWorkflowId,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::{
    ControlKeyspace, ControlKeyspaceError, ControlModelError, DeveloperIdentityId,
    EnvironmentRecord, ProjectRecord, ProjectStore, ProjectStoreError, QuotaResource,
};

const MAX_SUPPORT_SESSION_SECONDS: u64 = 8 * 60 * 60;

macro_rules! operator_identifier {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, OperatorError> {
                let value = value.into();
                let valid = value.strip_prefix($prefix).is_some_and(|suffix| {
                    (8..=96).contains(&suffix.len())
                        && suffix
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                });
                if !valid {
                    return Err(OperatorError::InvalidIdentifier);
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
            }
        }
    };
}

operator_identifier!(OperatorId, "opr_");
operator_identifier!(QuotaOverrideId, "qov_");
operator_identifier!(AbuseResponseId, "abr_");
operator_identifier!(SupportSessionId, "sup_");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorPermission {
    TenantRead,
    OverviewRead,
    OperationsRead,
    IncidentRead,
    IncidentManage,
    BackupRead,
    RecoveryManage,
    FleetRead,
    SecurityRead,
    SecurityManage,
    ActivityRead,
    ActivityExport,
    ProvisioningRepair,
    QuotaOverride,
    AbuseResponse,
    SupportAccess,
    WaitlistReview,
}

/// Least-privilege permission bundles used by operator bootstrap and administration UIs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRole {
    Observer,
    Responder,
    SecurityAdministrator,
    RecoveryAdministrator,
    Administrator,
}

impl OperatorRole {
    #[must_use]
    pub fn permissions(self) -> BTreeSet<OperatorPermission> {
        use OperatorPermission as Permission;
        let observer = [
            Permission::TenantRead,
            Permission::OverviewRead,
            Permission::OperationsRead,
            Permission::IncidentRead,
            Permission::BackupRead,
            Permission::FleetRead,
            Permission::SecurityRead,
            Permission::ActivityRead,
        ];
        match self {
            Self::Observer => observer.into_iter().collect(),
            Self::Responder => observer
                .into_iter()
                .chain([
                    Permission::IncidentManage,
                    Permission::ProvisioningRepair,
                    Permission::QuotaOverride,
                    Permission::AbuseResponse,
                    Permission::SupportAccess,
                ])
                .collect(),
            Self::SecurityAdministrator => observer
                .into_iter()
                .chain([Permission::SecurityManage, Permission::ActivityExport])
                .collect(),
            Self::RecoveryAdministrator => observer
                .into_iter()
                .chain([Permission::RecoveryManage])
                .collect(),
            Self::Administrator => observer
                .into_iter()
                .chain([
                    Permission::IncidentManage,
                    Permission::RecoveryManage,
                    Permission::SecurityManage,
                    Permission::ActivityExport,
                    Permission::ProvisioningRepair,
                    Permission::QuotaOverride,
                    Permission::AbuseResponse,
                    Permission::SupportAccess,
                    Permission::WaitlistReview,
                ])
                .collect(),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct OperatorSessionToken(String);

impl OperatorSessionToken {
    fn parse_authorization(value: Option<&str>) -> Result<Self, OperatorAuthenticationError> {
        let value = value.ok_or(OperatorAuthenticationError::MissingAuthorization)?;
        let token = value
            .strip_prefix("Bearer ")
            .ok_or(OperatorAuthenticationError::MalformedAuthorization)?;
        if token.len() < 16
            || token.len() > 16 * 1024
            || token.chars().any(char::is_whitespace)
            || token.chars().any(char::is_control)
        {
            return Err(OperatorAuthenticationError::MalformedAuthorization);
        }
        Ok(Self(token.to_owned()))
    }

    #[must_use]
    pub fn expose_to_provider(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OperatorSessionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperatorSessionToken([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorSessionClaims {
    pub operator_id: OperatorId,
    pub issuer: String,
    pub audiences: Vec<String>,
    pub permissions: BTreeSet<OperatorPermission>,
    pub expires_at_unix_seconds: u64,
}

#[async_trait]
pub trait OperatorIdentityProvider: Send + Sync {
    async fn verify_session(
        &self,
        token: &OperatorSessionToken,
        now_unix_seconds: u64,
    ) -> Result<OperatorSessionClaims, OperatorIdentityProviderError>;
}

#[derive(Clone)]
pub struct OperatorAuthenticator {
    provider: Arc<dyn OperatorIdentityProvider>,
    expected_issuer: String,
    expected_audience: String,
    enabled: bool,
}

impl fmt::Debug for OperatorAuthenticator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorAuthenticator")
            .field("expected_issuer", &self.expected_issuer)
            .field("expected_audience", &self.expected_audience)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl OperatorAuthenticator {
    pub fn new(
        provider: Arc<dyn OperatorIdentityProvider>,
        expected_issuer: impl Into<String>,
        expected_audience: impl Into<String>,
    ) -> Result<Self, OperatorAuthenticationError> {
        let expected_issuer = expected_issuer.into();
        let expected_audience = expected_audience.into();
        if !valid_text(&expected_issuer, 2_048)
            || !valid_text(&expected_audience, 256)
            || expected_issuer.len() < 8
            || expected_audience.len() < 3
        {
            return Err(OperatorAuthenticationError::InvalidConfiguration);
        }
        Ok(Self {
            provider,
            expected_issuer,
            expected_audience,
            enabled: true,
        })
    }

    #[must_use]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub async fn authenticate(
        &self,
        authorization: Option<&str>,
        now_unix_seconds: u64,
    ) -> Result<OperatorPrincipal, OperatorAuthenticationError> {
        if !self.enabled {
            return Err(OperatorAuthenticationError::Disabled);
        }
        let token = OperatorSessionToken::parse_authorization(authorization)?;
        let claims = self
            .provider
            .verify_session(&token, now_unix_seconds)
            .await?;
        if claims.issuer != self.expected_issuer
            || !claims
                .audiences
                .iter()
                .any(|audience| audience == &self.expected_audience)
            || claims.expires_at_unix_seconds <= now_unix_seconds
            || claims.permissions.is_empty()
        {
            return Err(OperatorAuthenticationError::InvalidClaims);
        }
        Ok(OperatorPrincipal {
            id: claims.operator_id,
            authentication_identity_id: None,
            permissions: claims.permissions,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorPrincipal {
    id: OperatorId,
    authentication_identity_id: Option<DeveloperIdentityId>,
    permissions: BTreeSet<OperatorPermission>,
}

impl OperatorPrincipal {
    pub(crate) fn authenticated(
        id: OperatorId,
        authentication_identity_id: DeveloperIdentityId,
        permissions: impl IntoIterator<Item = OperatorPermission>,
    ) -> Self {
        Self {
            id,
            authentication_identity_id: Some(authentication_identity_id),
            permissions: permissions.into_iter().collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        id: OperatorId,
        permissions: impl IntoIterator<Item = OperatorPermission>,
    ) -> Self {
        Self {
            id,
            authentication_identity_id: None,
            permissions: permissions.into_iter().collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_identity(
        id: OperatorId,
        authentication_identity_id: DeveloperIdentityId,
        permissions: impl IntoIterator<Item = OperatorPermission>,
    ) -> Self {
        Self::authenticated(id, authentication_identity_id, permissions)
    }

    #[must_use]
    pub fn id(&self) -> &OperatorId {
        &self.id
    }

    #[must_use]
    pub fn authentication_identity_id(&self) -> Option<&DeveloperIdentityId> {
        self.authentication_identity_id.as_ref()
    }

    #[must_use]
    pub fn allows(&self, permission: OperatorPermission) -> bool {
        self.permissions.contains(&permission)
    }

    #[must_use]
    pub fn permissions(&self) -> &BTreeSet<OperatorPermission> {
        &self.permissions
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorProjectView {
    pub project: ProjectRecord,
    pub environments: Vec<EnvironmentRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct QuotaOverrideRecord {
    id: QuotaOverrideId,
    project_id: ProjectId,
    resource: QuotaResource,
    limit: u64,
    reason: String,
    operator_id: OperatorId,
    expires_at_unix_seconds: Option<u64>,
    created_at_unix_seconds: u64,
    #[serde(default = "default_operator_record_state")]
    state: OperatorRecordState,
    #[serde(default = "default_version")]
    version: u64,
    #[serde(default)]
    history: Vec<OperatorRecordEvent>,
    #[serde(default)]
    revoked_at_unix_seconds: Option<u64>,
}

impl QuotaOverrideRecord {
    #[must_use]
    pub fn id(&self) -> &QuotaOverrideId {
        &self.id
    }

    #[must_use]
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRecordState {
    #[default]
    Active,
    Revoked,
    Restored,
}

fn default_operator_record_state() -> OperatorRecordState {
    OperatorRecordState::Active
}

const fn default_version() -> u64 {
    1
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorRecordEvent {
    pub sequence: u64,
    pub action: String,
    pub operator_id: OperatorId,
    pub reason: String,
    pub at_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewQuotaOverride {
    pub id: QuotaOverrideId,
    pub project_id: ProjectId,
    pub resource: QuotaResource,
    pub limit: u64,
    pub reason: String,
    pub expires_at_unix_seconds: Option<u64>,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "environmentId")]
pub enum AbuseTarget {
    Project,
    Environment(EnvironmentId),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AbuseResponseRecord {
    id: AbuseResponseId,
    project_id: ProjectId,
    target: AbuseTarget,
    reason: String,
    operator_id: OperatorId,
    created_at_unix_seconds: u64,
    #[serde(default = "default_operator_record_state")]
    state: OperatorRecordState,
    #[serde(default = "default_version")]
    version: u64,
    #[serde(default)]
    history: Vec<OperatorRecordEvent>,
    #[serde(default)]
    restored_at_unix_seconds: Option<u64>,
}

impl AbuseResponseRecord {
    #[must_use]
    pub fn id(&self) -> &AbuseResponseId {
        &self.id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewAbuseResponse {
    pub id: AbuseResponseId,
    pub project_id: ProjectId,
    pub target: AbuseTarget,
    pub reason: String,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportPermission {
    ProjectMetadataRead,
    ApplicationUserRead,
    LogsRead,
    DocumentRead,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportSessionState {
    Active,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SupportSessionRecord {
    id: SupportSessionId,
    operator_id: OperatorId,
    project_id: ProjectId,
    environment_id: Option<EnvironmentId>,
    permissions: BTreeSet<SupportPermission>,
    reason: String,
    state: SupportSessionState,
    expires_at_unix_seconds: u64,
    created_at_unix_seconds: u64,
    revoked_at_unix_seconds: Option<u64>,
    #[serde(default = "default_version")]
    version: u64,
    #[serde(default)]
    history: Vec<OperatorRecordEvent>,
}

impl SupportSessionRecord {
    #[must_use]
    pub fn id(&self) -> &SupportSessionId {
        &self.id
    }

    #[must_use]
    pub const fn state(&self) -> SupportSessionState {
        self.state
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewSupportSession {
    pub id: SupportSessionId,
    pub project_id: ProjectId,
    pub environment_id: Option<EnvironmentId>,
    pub permissions: BTreeSet<SupportPermission>,
    pub reason: String,
    pub expires_at_unix_seconds: u64,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSupportAccess {
    session_id: SupportSessionId,
    operator_id: OperatorId,
    project_id: ProjectId,
    environment_id: Option<EnvironmentId>,
    permission: SupportPermission,
    reason: String,
    expires_at_unix_seconds: u64,
}

impl VerifiedSupportAccess {
    #[must_use]
    pub fn session_id(&self) -> &SupportSessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn permission(&self) -> SupportPermission {
        self.permission
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorAuditAction {
    TenantRead,
    OverviewRead,
    InventoryRead,
    IncidentManage,
    RecoveryManage,
    SecurityManage,
    ActivityRead,
    ActivityExport,
    ProjectionRebuild,
    ProvisioningRepair,
    QuotaOverride,
    AbuseResponse,
    SupportSessionCreate,
    SupportSessionUse,
    SupportSessionRevoke,
}

impl OperatorAuditAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TenantRead => "operator_tenant_read",
            Self::OverviewRead => "operator_overview_read",
            Self::InventoryRead => "operator_inventory_read",
            Self::IncidentManage => "operator_incident_manage",
            Self::RecoveryManage => "operator_recovery_manage",
            Self::SecurityManage => "operator_security_manage",
            Self::ActivityRead => "operator_activity_read",
            Self::ActivityExport => "operator_activity_export",
            Self::ProjectionRebuild => "operator_projection_rebuild",
            Self::ProvisioningRepair => "operator_provisioning_repair",
            Self::QuotaOverride => "operator_quota_override",
            Self::AbuseResponse => "operator_abuse_response",
            Self::SupportSessionCreate => "operator_support_session_create",
            Self::SupportSessionUse => "operator_support_session_use",
            Self::SupportSessionRevoke => "operator_support_session_revoke",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorAuditOutcome {
    Allowed,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorAuditEvent {
    pub operator_id: OperatorId,
    pub action: OperatorAuditAction,
    pub project_id: ProjectId,
    pub target: String,
    pub reason: String,
    pub outcome: OperatorAuditOutcome,
    pub at_unix_seconds: u64,
}

pub trait OperatorAuditSink: Send + Sync {
    fn record(&self, event: OperatorAuditEvent);
}

#[derive(Clone)]
pub struct OperatorService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    provisioning: Provisioner,
    audit: Arc<dyn OperatorAuditSink>,
}

impl fmt::Debug for OperatorService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl OperatorService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        provisioning: Provisioner,
        audit: Arc<dyn OperatorAuditSink>,
    ) -> Result<Self, OperatorError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(OperatorError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            provisioning,
            audit,
        })
    }

    pub async fn inspect_project(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        environment_limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<OperatorProjectView, OperatorError> {
        self.require(
            actor,
            OperatorPermission::TenantRead,
            OperatorAuditAction::TenantRead,
            project_id,
            "operator tenant lookup",
            now_unix_seconds,
        )?;
        let project = self.project(project_id).await?;
        let environments = self
            .projects
            .list_environments(project_id, environment_limit)
            .await?;
        self.allowed(
            actor,
            OperatorAuditAction::TenantRead,
            project_id,
            project_id.as_str(),
            "operator tenant lookup",
            now_unix_seconds,
        );
        Ok(OperatorProjectView {
            project,
            environments,
        })
    }

    pub async fn repair_provisioning(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        workflow_id: &ProvisioningWorkflowId,
        reason: &str,
        action: OperatorRepairAction,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, OperatorError> {
        self.repair_provisioning_guarded(
            actor,
            project_id,
            workflow_id,
            reason,
            action,
            None,
            None,
            now_unix_seconds,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn repair_provisioning_guarded(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        workflow_id: &ProvisioningWorkflowId,
        reason: &str,
        action: OperatorRepairAction,
        reviewed_at_unix_seconds: Option<u64>,
        operation_key: Option<String>,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, OperatorError> {
        validate_reason(reason)?;
        self.require(
            actor,
            OperatorPermission::ProvisioningRepair,
            OperatorAuditAction::ProvisioningRepair,
            project_id,
            reason,
            now_unix_seconds,
        )?;
        self.project(project_id).await?;
        let workflow = self.provisioning.inspect(workflow_id).await?;
        if !resource_belongs_to_project(workflow.resource(), project_id) {
            self.denied(
                actor,
                OperatorAuditAction::ProvisioningRepair,
                project_id,
                workflow_id.as_str(),
                "provisioning workflow scope mismatch",
                now_unix_seconds,
            );
            return Err(OperatorError::ScopeMismatch);
        }
        if reviewed_at_unix_seconds
            .is_some_and(|reviewed| reviewed != workflow.updated_at_unix_seconds())
        {
            self.denied(
                actor,
                OperatorAuditAction::ProvisioningRepair,
                project_id,
                workflow_id.as_str(),
                "reviewed provisioning state is stale",
                now_unix_seconds,
            );
            return Err(OperatorError::Conflict);
        }
        let repaired = self
            .provisioning
            .operator_repair_guarded(
                workflow_id,
                actor.id().as_str(),
                reason,
                action,
                operation_key,
                now_unix_seconds,
            )
            .await?;
        self.allowed(
            actor,
            OperatorAuditAction::ProvisioningRepair,
            project_id,
            workflow_id.as_str(),
            reason,
            now_unix_seconds,
        );
        Ok(repaired)
    }

    pub async fn create_quota_override(
        &self,
        actor: &OperatorPrincipal,
        input: NewQuotaOverride,
    ) -> Result<QuotaOverrideRecord, OperatorError> {
        validate_reason(&input.reason)?;
        if input.limit == 0
            || input
                .expires_at_unix_seconds
                .is_some_and(|expires| expires <= input.now_unix_seconds)
        {
            return Err(OperatorError::InvalidInput);
        }
        self.require(
            actor,
            OperatorPermission::QuotaOverride,
            OperatorAuditAction::QuotaOverride,
            &input.project_id,
            &input.reason,
            input.now_unix_seconds,
        )?;
        self.project(&input.project_id).await?;
        let record = QuotaOverrideRecord {
            id: input.id,
            project_id: input.project_id,
            resource: input.resource,
            limit: input.limit,
            reason: input.reason.clone(),
            operator_id: actor.id().clone(),
            expires_at_unix_seconds: input.expires_at_unix_seconds,
            created_at_unix_seconds: input.now_unix_seconds,
            state: OperatorRecordState::Active,
            version: 1,
            history: vec![OperatorRecordEvent {
                sequence: 1,
                action: "created".to_owned(),
                operator_id: actor.id().clone(),
                reason: input.reason.clone(),
                at_unix_seconds: input.now_unix_seconds,
            }],
            revoked_at_unix_seconds: None,
        };
        let key =
            ControlKeyspace::operator_quota_override_key(&record.project_id, record.id.as_str())?;
        self.create(key, serde_json::to_vec(&record)?).await?;
        self.allowed(
            actor,
            OperatorAuditAction::QuotaOverride,
            &record.project_id,
            record.id.as_str(),
            &record.reason,
            input.now_unix_seconds,
        );
        Ok(record)
    }

    pub async fn respond_to_abuse(
        &self,
        actor: &OperatorPrincipal,
        input: NewAbuseResponse,
    ) -> Result<AbuseResponseRecord, OperatorError> {
        validate_reason(&input.reason)?;
        self.require(
            actor,
            OperatorPermission::AbuseResponse,
            OperatorAuditAction::AbuseResponse,
            &input.project_id,
            &input.reason,
            input.now_unix_seconds,
        )?;
        let record = AbuseResponseRecord {
            id: input.id,
            project_id: input.project_id,
            target: input.target,
            reason: input.reason.clone(),
            operator_id: actor.id().clone(),
            created_at_unix_seconds: input.now_unix_seconds,
            state: OperatorRecordState::Active,
            version: 1,
            history: vec![OperatorRecordEvent {
                sequence: 1,
                action: "applied".to_owned(),
                operator_id: actor.id().clone(),
                reason: input.reason.clone(),
                at_unix_seconds: input.now_unix_seconds,
            }],
            restored_at_unix_seconds: None,
        };
        let key =
            ControlKeyspace::operator_abuse_response_key(&record.project_id, record.id.as_str())?;
        let (resource_key, previous, next) = match &record.target {
            AbuseTarget::Project => {
                let previous = self.project(&record.project_id).await?;
                let mut next = previous.clone();
                next.suspend(input.now_unix_seconds)?;
                (
                    ControlKeyspace::project_key(&record.project_id)?,
                    serde_json::to_vec(&previous)?,
                    serde_json::to_vec(&next)?,
                )
            }
            AbuseTarget::Environment(environment_id) => {
                self.project(&record.project_id).await?;
                let previous = self
                    .projects
                    .get_environment(&record.project_id, environment_id)
                    .await?
                    .ok_or(OperatorError::NotFound)?;
                let mut next = previous.clone();
                next.suspend(input.now_unix_seconds)?;
                (
                    ControlKeyspace::environment_key(&record.project_id, environment_id)?,
                    serde_json::to_vec(&previous)?,
                    serde_json::to_vec(&next)?,
                )
            }
        };
        let mut batch = WriteBatch::new();
        batch.put(&resource_key, next);
        batch.put(&key, serde_json::to_vec(&record)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::Missing { key },
                KeyCondition::ValueEquals {
                    key: resource_key,
                    value: previous,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await?;
        self.allowed(
            actor,
            OperatorAuditAction::AbuseResponse,
            &record.project_id,
            record.id.as_str(),
            &record.reason,
            input.now_unix_seconds,
        );
        Ok(record)
    }

    pub async fn create_support_session(
        &self,
        actor: &OperatorPrincipal,
        input: NewSupportSession,
    ) -> Result<SupportSessionRecord, OperatorError> {
        validate_reason(&input.reason)?;
        if input.permissions.is_empty()
            || input.expires_at_unix_seconds <= input.now_unix_seconds
            || input.expires_at_unix_seconds - input.now_unix_seconds > MAX_SUPPORT_SESSION_SECONDS
        {
            return Err(OperatorError::InvalidInput);
        }
        self.require(
            actor,
            OperatorPermission::SupportAccess,
            OperatorAuditAction::SupportSessionCreate,
            &input.project_id,
            &input.reason,
            input.now_unix_seconds,
        )?;
        self.project(&input.project_id).await?;
        if let Some(environment_id) = &input.environment_id {
            self.projects
                .get_environment(&input.project_id, environment_id)
                .await?
                .ok_or(OperatorError::NotFound)?;
        }
        let record = SupportSessionRecord {
            id: input.id,
            operator_id: actor.id().clone(),
            project_id: input.project_id,
            environment_id: input.environment_id,
            permissions: input.permissions,
            reason: input.reason.clone(),
            state: SupportSessionState::Active,
            expires_at_unix_seconds: input.expires_at_unix_seconds,
            created_at_unix_seconds: input.now_unix_seconds,
            revoked_at_unix_seconds: None,
            version: 1,
            history: vec![OperatorRecordEvent {
                sequence: 1,
                action: "created".to_owned(),
                operator_id: actor.id().clone(),
                reason: input.reason.clone(),
                at_unix_seconds: input.now_unix_seconds,
            }],
        };
        let key = support_key(&record)?;
        self.create(key, serde_json::to_vec(&record)?).await?;
        self.allowed(
            actor,
            OperatorAuditAction::SupportSessionCreate,
            &record.project_id,
            record.id.as_str(),
            &record.reason,
            input.now_unix_seconds,
        );
        Ok(record)
    }

    pub async fn verify_support_access(
        &self,
        session_id: &SupportSessionId,
        project_id: &ProjectId,
        environment_id: Option<&EnvironmentId>,
        permission: SupportPermission,
        now_unix_seconds: u64,
    ) -> Result<VerifiedSupportAccess, OperatorError> {
        let record = self.support_session(project_id, session_id).await?;
        if record.state != SupportSessionState::Active
            || record.expires_at_unix_seconds <= now_unix_seconds
            || record.environment_id.as_ref() != environment_id
            || !record.permissions.contains(&permission)
        {
            self.audit.record(OperatorAuditEvent {
                operator_id: record.operator_id,
                action: OperatorAuditAction::SupportSessionUse,
                project_id: record.project_id,
                target: session_id.as_str().to_owned(),
                reason: record.reason,
                outcome: OperatorAuditOutcome::Denied,
                at_unix_seconds: now_unix_seconds,
            });
            return Err(OperatorError::Forbidden);
        }
        let access = VerifiedSupportAccess {
            session_id: record.id,
            operator_id: record.operator_id,
            project_id: record.project_id,
            environment_id: record.environment_id,
            permission,
            reason: record.reason,
            expires_at_unix_seconds: record.expires_at_unix_seconds,
        };
        self.audit.record(OperatorAuditEvent {
            operator_id: access.operator_id.clone(),
            action: OperatorAuditAction::SupportSessionUse,
            project_id: access.project_id.clone(),
            target: access.session_id.as_str().to_owned(),
            reason: access.reason.clone(),
            outcome: OperatorAuditOutcome::Allowed,
            at_unix_seconds: now_unix_seconds,
        });
        Ok(access)
    }

    pub async fn revoke_support_session(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        session_id: &SupportSessionId,
        reason: &str,
        now_unix_seconds: u64,
    ) -> Result<SupportSessionRecord, OperatorError> {
        validate_reason(reason)?;
        self.require(
            actor,
            OperatorPermission::SupportAccess,
            OperatorAuditAction::SupportSessionRevoke,
            project_id,
            reason,
            now_unix_seconds,
        )?;
        let previous = self.support_session(project_id, session_id).await?;
        if previous.state != SupportSessionState::Active {
            return Err(OperatorError::InvalidState);
        }
        let mut next = previous.clone();
        next.state = SupportSessionState::Revoked;
        next.revoked_at_unix_seconds = Some(now_unix_seconds);
        next.version = next.version.saturating_add(1);
        next.history.push(OperatorRecordEvent {
            sequence: next.version,
            action: "revoked".to_owned(),
            operator_id: actor.id().clone(),
            reason: reason.to_owned(),
            at_unix_seconds: now_unix_seconds,
        });
        self.replace(
            support_key(&previous)?,
            serde_json::to_vec(&previous)?,
            serde_json::to_vec(&next)?,
        )
        .await?;
        self.allowed(
            actor,
            OperatorAuditAction::SupportSessionRevoke,
            project_id,
            session_id.as_str(),
            reason,
            now_unix_seconds,
        );
        Ok(next)
    }

    pub async fn list_quota_overrides(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<QuotaOverrideRecord>, OperatorError> {
        self.require_read(
            actor,
            project_id,
            "quota override inventory",
            now_unix_seconds,
        )?;
        self.project(project_id).await?;
        self.scan_scoped(
            ControlKeyspace::operator_quota_overrides_range(project_id)?,
            limit,
            |record: &QuotaOverrideRecord| record.project_id == *project_id,
        )
        .await
    }

    pub async fn list_provisioning_workflows(
        &self,
        actor: &OperatorPrincipal,
        project_id: Option<&ProjectId>,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<ProvisioningWorkflow>, OperatorError> {
        let audit_project = project_id.cloned().unwrap_or_else(global_project_id);
        self.require_read(
            actor,
            &audit_project,
            "provisioning workflow inventory",
            now_unix_seconds,
        )?;
        if let Some(project_id) = project_id {
            self.project(project_id).await?;
        }
        let workflows = self.provisioning.list(limit).await?;
        Ok(workflows
            .into_iter()
            .filter(|workflow| {
                project_id.is_none_or(|project_id| {
                    resource_belongs_to_project(workflow.resource(), project_id)
                })
            })
            .filter(|workflow| {
                matches!(
                    workflow.state(),
                    mako_provisioning::ProvisioningState::Queued
                        | mako_provisioning::ProvisioningState::Running
                        | mako_provisioning::ProvisioningState::Compensating
                        | mako_provisioning::ProvisioningState::Failed
                        | mako_provisioning::ProvisioningState::RepairRequired
                )
            })
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn replace_quota_override(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        override_id: &QuotaOverrideId,
        reviewed_version: u64,
        limit: u64,
        expires_at_unix_seconds: Option<u64>,
        reason: &str,
        now_unix_seconds: u64,
    ) -> Result<QuotaOverrideRecord, OperatorError> {
        validate_reason(reason)?;
        if limit == 0 || expires_at_unix_seconds.is_some_and(|expires| expires <= now_unix_seconds)
        {
            return Err(OperatorError::InvalidInput);
        }
        self.require(
            actor,
            OperatorPermission::QuotaOverride,
            OperatorAuditAction::QuotaOverride,
            project_id,
            reason,
            now_unix_seconds,
        )?;
        let key = ControlKeyspace::operator_quota_override_key(project_id, override_id.as_str())?;
        let previous: QuotaOverrideRecord = self.read_required(&key).await?;
        if previous.project_id != *project_id
            || previous.id != *override_id
            || previous.state != OperatorRecordState::Active
            || previous.version != reviewed_version
        {
            return Err(OperatorError::Conflict);
        }
        let mut next = previous.clone();
        next.limit = limit;
        next.expires_at_unix_seconds = expires_at_unix_seconds;
        next.reason = reason.to_owned();
        next.version = next.version.saturating_add(1);
        next.history.push(OperatorRecordEvent {
            sequence: next.version,
            action: "replaced".to_owned(),
            operator_id: actor.id().clone(),
            reason: reason.to_owned(),
            at_unix_seconds: now_unix_seconds,
        });
        self.replace(
            key,
            serde_json::to_vec(&previous)?,
            serde_json::to_vec(&next)?,
        )
        .await?;
        self.allowed(
            actor,
            OperatorAuditAction::QuotaOverride,
            project_id,
            override_id.as_str(),
            reason,
            now_unix_seconds,
        );
        Ok(next)
    }

    pub async fn revoke_quota_override(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        override_id: &QuotaOverrideId,
        reviewed_version: u64,
        reason: &str,
        now_unix_seconds: u64,
    ) -> Result<QuotaOverrideRecord, OperatorError> {
        validate_reason(reason)?;
        self.require(
            actor,
            OperatorPermission::QuotaOverride,
            OperatorAuditAction::QuotaOverride,
            project_id,
            reason,
            now_unix_seconds,
        )?;
        let key = ControlKeyspace::operator_quota_override_key(project_id, override_id.as_str())?;
        let previous: QuotaOverrideRecord = self.read_required(&key).await?;
        if previous.project_id != *project_id
            || previous.id != *override_id
            || previous.state != OperatorRecordState::Active
            || previous.version != reviewed_version
        {
            return Err(OperatorError::Conflict);
        }
        let mut next = previous.clone();
        next.state = OperatorRecordState::Revoked;
        next.revoked_at_unix_seconds = Some(now_unix_seconds);
        next.version = next.version.saturating_add(1);
        next.history.push(OperatorRecordEvent {
            sequence: next.version,
            action: "revoked".to_owned(),
            operator_id: actor.id().clone(),
            reason: reason.to_owned(),
            at_unix_seconds: now_unix_seconds,
        });
        self.replace(
            key,
            serde_json::to_vec(&previous)?,
            serde_json::to_vec(&next)?,
        )
        .await?;
        self.allowed(
            actor,
            OperatorAuditAction::QuotaOverride,
            project_id,
            override_id.as_str(),
            reason,
            now_unix_seconds,
        );
        Ok(next)
    }

    pub async fn list_abuse_responses(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<AbuseResponseRecord>, OperatorError> {
        self.require_read(
            actor,
            project_id,
            "abuse response inventory",
            now_unix_seconds,
        )?;
        self.project(project_id).await?;
        self.scan_scoped(
            ControlKeyspace::operator_abuse_responses_range(project_id)?,
            limit,
            |record: &AbuseResponseRecord| record.project_id == *project_id,
        )
        .await
    }

    pub async fn restore_abuse_response(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        response_id: &AbuseResponseId,
        reviewed_version: u64,
        reason: &str,
        now_unix_seconds: u64,
    ) -> Result<AbuseResponseRecord, OperatorError> {
        validate_reason(reason)?;
        self.require(
            actor,
            OperatorPermission::AbuseResponse,
            OperatorAuditAction::AbuseResponse,
            project_id,
            reason,
            now_unix_seconds,
        )?;
        let record_key =
            ControlKeyspace::operator_abuse_response_key(project_id, response_id.as_str())?;
        let previous: AbuseResponseRecord = self.read_required(&record_key).await?;
        if previous.project_id != *project_id
            || previous.id != *response_id
            || previous.state != OperatorRecordState::Active
            || previous.version != reviewed_version
        {
            return Err(OperatorError::Conflict);
        }
        let (resource_key, resource_previous, resource_next) = match &previous.target {
            AbuseTarget::Project => {
                let current = self.project(project_id).await?;
                let mut restored = current.clone();
                restored.restore(now_unix_seconds)?;
                (
                    ControlKeyspace::project_key(project_id)?,
                    serde_json::to_vec(&current)?,
                    serde_json::to_vec(&restored)?,
                )
            }
            AbuseTarget::Environment(environment_id) => {
                let current = self
                    .projects
                    .get_environment(project_id, environment_id)
                    .await?
                    .ok_or(OperatorError::NotFound)?;
                let mut restored = current.clone();
                restored.restore(now_unix_seconds)?;
                (
                    ControlKeyspace::environment_key(project_id, environment_id)?,
                    serde_json::to_vec(&current)?,
                    serde_json::to_vec(&restored)?,
                )
            }
        };
        let mut next = previous.clone();
        next.state = OperatorRecordState::Restored;
        next.restored_at_unix_seconds = Some(now_unix_seconds);
        next.version = next.version.saturating_add(1);
        next.history.push(OperatorRecordEvent {
            sequence: next.version,
            action: "restored".to_owned(),
            operator_id: actor.id().clone(),
            reason: reason.to_owned(),
            at_unix_seconds: now_unix_seconds,
        });
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&resource_key, resource_next);
        batch.put(&record_key, serde_json::to_vec(&next)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: resource_key,
                    value: resource_previous,
                },
                KeyCondition::ValueEquals {
                    key: record_key,
                    value: serde_json::to_vec(&previous)?,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await?;
        self.allowed(
            actor,
            OperatorAuditAction::AbuseResponse,
            project_id,
            response_id.as_str(),
            reason,
            now_unix_seconds,
        );
        Ok(next)
    }

    pub async fn list_support_sessions(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<SupportSessionRecord>, OperatorError> {
        self.require_read(
            actor,
            project_id,
            "support session inventory",
            now_unix_seconds,
        )?;
        self.project(project_id).await?;
        self.scan_scoped(
            ControlKeyspace::operator_support_sessions_range(project_id)?,
            limit,
            |record: &SupportSessionRecord| record.project_id == *project_id,
        )
        .await
    }

    pub async fn list_active_support_sessions_for_operator(
        &self,
        actor: &OperatorPrincipal,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<SupportSessionRecord>, OperatorError> {
        if !actor.allows(OperatorPermission::SupportAccess) {
            return Ok(Vec::new());
        }
        let records = self
            .scan_scoped(
                ControlKeyspace::operator_all_support_sessions_range()?,
                limit,
                |record: &SupportSessionRecord| {
                    record.operator_id == *actor.id()
                        && record.state == SupportSessionState::Active
                        && record.revoked_at_unix_seconds.is_none()
                        && record.expires_at_unix_seconds > now_unix_seconds
                },
            )
            .await?;
        for record in &records {
            self.allowed(
                actor,
                OperatorAuditAction::SupportSessionUse,
                &record.project_id,
                record.id.as_str(),
                "active support-mode status",
                now_unix_seconds,
            );
        }
        Ok(records)
    }

    async fn project(&self, project_id: &ProjectId) -> Result<ProjectRecord, OperatorError> {
        self.projects
            .get_project(project_id)
            .await?
            .ok_or(OperatorError::NotFound)
    }

    async fn read_required<T: serde::de::DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<T, OperatorError> {
        self.adapter
            .get(key)
            .await?
            .ok_or(OperatorError::NotFound)
            .and_then(|value| serde_json::from_slice(&value).map_err(Into::into))
    }

    async fn scan_scoped<T: serde::de::DeserializeOwned>(
        &self,
        range: mako_storage::KeyRange,
        limit: NonZeroUsize,
        scoped: impl Fn(&T) -> bool,
    ) -> Result<Vec<T>, OperatorError> {
        if limit.get() > 100 {
            return Err(OperatorError::InvalidInput);
        }
        let values = self
            .adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await?;
        let records = values
            .into_iter()
            .map(|value| serde_json::from_slice::<T>(&value.value))
            .collect::<Result<Vec<_>, _>>()?;
        if records.iter().any(|record| !scoped(record)) {
            return Err(OperatorError::CorruptRecord);
        }
        Ok(records)
    }

    fn require_read(
        &self,
        actor: &OperatorPrincipal,
        project_id: &ProjectId,
        reason: &str,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorError> {
        if actor.allows(OperatorPermission::TenantRead)
            || actor.allows(OperatorPermission::OperationsRead)
        {
            return Ok(());
        }
        self.audit.record(OperatorAuditEvent {
            operator_id: actor.id().clone(),
            action: OperatorAuditAction::TenantRead,
            project_id: project_id.clone(),
            target: "authorization".to_owned(),
            reason: reason.to_owned(),
            outcome: OperatorAuditOutcome::Denied,
            at_unix_seconds: now_unix_seconds,
        });
        Err(OperatorError::Forbidden)
    }

    async fn support_session(
        &self,
        project_id: &ProjectId,
        session_id: &SupportSessionId,
    ) -> Result<SupportSessionRecord, OperatorError> {
        let key = ControlKeyspace::operator_support_session_key(project_id, session_id.as_str())?;
        let record: SupportSessionRecord = self
            .adapter
            .get(&key)
            .await?
            .ok_or(OperatorError::NotFound)
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(Into::into))?;
        if record.project_id != *project_id || record.id != *session_id {
            return Err(OperatorError::CorruptRecord);
        }
        Ok(record)
    }

    fn require(
        &self,
        actor: &OperatorPrincipal,
        permission: OperatorPermission,
        action: OperatorAuditAction,
        project_id: &ProjectId,
        reason: &str,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorError> {
        if !actor.allows(permission) {
            self.audit.record(OperatorAuditEvent {
                operator_id: actor.id().clone(),
                action,
                project_id: project_id.clone(),
                target: "authorization".to_owned(),
                reason: reason.to_owned(),
                outcome: OperatorAuditOutcome::Denied,
                at_unix_seconds: now_unix_seconds,
            });
            return Err(OperatorError::Forbidden);
        }
        Ok(())
    }

    fn allowed(
        &self,
        actor: &OperatorPrincipal,
        action: OperatorAuditAction,
        project_id: &ProjectId,
        target: &str,
        reason: &str,
        now_unix_seconds: u64,
    ) {
        self.audit.record(OperatorAuditEvent {
            operator_id: actor.id().clone(),
            action,
            project_id: project_id.clone(),
            target: target.to_owned(),
            reason: reason.to_owned(),
            outcome: OperatorAuditOutcome::Allowed,
            at_unix_seconds: now_unix_seconds,
        });
    }

    fn denied(
        &self,
        actor: &OperatorPrincipal,
        action: OperatorAuditAction,
        project_id: &ProjectId,
        target: &str,
        reason: &str,
        now_unix_seconds: u64,
    ) {
        self.audit.record(OperatorAuditEvent {
            operator_id: actor.id().clone(),
            action,
            project_id: project_id.clone(),
            target: target.to_owned(),
            reason: reason.to_owned(),
            outcome: OperatorAuditOutcome::Denied,
            at_unix_seconds: now_unix_seconds,
        });
    }

    async fn create(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), OperatorError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, value);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn replace(
        &self,
        key: Vec<u8>,
        previous: Vec<u8>,
        next: Vec<u8>,
    ) -> Result<(), OperatorError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, next);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: previous,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), OperatorError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(OperatorError::Conflict),
        }
    }
}

fn resource_belongs_to_project(resource: &ProvisioningResource, project_id: &ProjectId) -> bool {
    match resource {
        ProvisioningResource::Project(resource_project_id) => resource_project_id == project_id,
        ProvisioningResource::Environment(tenant) => tenant.project_id() == project_id,
    }
}

fn support_key(record: &SupportSessionRecord) -> Result<Vec<u8>, OperatorError> {
    ControlKeyspace::operator_support_session_key(&record.project_id, record.id.as_str())
        .map_err(Into::into)
}

fn global_project_id() -> ProjectId {
    ProjectId::parse("prj_global00").expect("static global project id is valid")
}

fn validate_reason(reason: &str) -> Result<(), OperatorError> {
    if reason.len() < 8
        || reason.len() > 1_024
        || reason.trim() != reason
        || reason.chars().any(char::is_control)
    {
        return Err(OperatorError::InvalidInput);
    }
    Ok(())
}

fn valid_text(value: &str, max: usize) -> bool {
    value.trim() == value
        && !value.is_empty()
        && value.len() <= max
        && !value.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorIdentityProviderError {
    Unavailable,
    InvalidToken,
}

impl fmt::Display for OperatorIdentityProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "operator identity provider is unavailable",
            Self::InvalidToken => "operator session is invalid",
        })
    }
}

impl Error for OperatorIdentityProviderError {}

#[derive(Debug)]
pub enum OperatorAuthenticationError {
    Disabled,
    MissingAuthorization,
    MalformedAuthorization,
    InvalidConfiguration,
    InvalidClaims,
    Provider(OperatorIdentityProviderError),
}

impl fmt::Display for OperatorAuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Disabled => "operator bearer authentication is disabled",
            Self::MissingAuthorization => "operator authorization is required",
            Self::MalformedAuthorization => "operator authorization is malformed",
            Self::InvalidConfiguration => "operator authentication configuration is invalid",
            Self::InvalidClaims => "operator session claims are invalid",
            Self::Provider(_) => "operator identity verification failed",
        })
    }
}

impl Error for OperatorAuthenticationError {}

impl From<OperatorIdentityProviderError> for OperatorAuthenticationError {
    fn from(error: OperatorIdentityProviderError) -> Self {
        Self::Provider(error)
    }
}

#[derive(Debug)]
pub enum OperatorError {
    UnsupportedDurability,
    InvalidIdentifier,
    InvalidInput,
    InvalidState,
    NotFound,
    Forbidden,
    ScopeMismatch,
    Conflict,
    CorruptRecord,
    Project(ProjectStoreError),
    Provisioning(ProvisioningWorkflowError),
    Model(ControlModelError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for OperatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedDurability => "operator durability is unsupported",
            Self::InvalidIdentifier => "operator identifier is invalid",
            Self::InvalidInput => "operator request is invalid",
            Self::InvalidState => "operator resource state is invalid",
            Self::NotFound => "operator resource was not found",
            Self::Forbidden => "operator action is forbidden",
            Self::ScopeMismatch => "operator resource scope does not match",
            Self::Conflict => "operator resource changed concurrently",
            Self::CorruptRecord => "operator record is corrupt",
            Self::Project(_) => "operator project operation failed",
            Self::Provisioning(_) => "operator provisioning operation failed",
            Self::Model(_) => "operator lifecycle change failed",
            Self::Keyspace(_) => "operator key is invalid",
            Self::Storage(_) => "operator storage operation failed",
            Self::Json(_) => "operator record is invalid",
        })
    }
}

impl Error for OperatorError {}

macro_rules! operator_error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for OperatorError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

operator_error_from!(ProjectStoreError, Project);
operator_error_from!(ProvisioningWorkflowError, Provisioning);
operator_error_from!(ControlModelError, Model);
operator_error_from!(ControlKeyspaceError, Keyspace);
operator_error_from!(StorageError, Storage);
operator_error_from!(serde_json::Error, Json);

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{LifecycleState, OrganizationId};

    #[derive(Default)]
    struct Audit(Mutex<Vec<OperatorAuditEvent>>);

    impl OperatorAuditSink for Audit {
        fn record(&self, event: OperatorAuditEvent) {
            self.0.lock().expect("audit").push(event);
        }
    }

    struct StaticOperatorIdentityProvider {
        claims: OperatorSessionClaims,
    }

    #[async_trait]
    impl OperatorIdentityProvider for StaticOperatorIdentityProvider {
        async fn verify_session(
            &self,
            _token: &OperatorSessionToken,
            _now_unix_seconds: u64,
        ) -> Result<OperatorSessionClaims, OperatorIdentityProviderError> {
            Ok(self.claims.clone())
        }
    }

    #[test]
    fn break_glass_authenticator_is_explicitly_enabled_and_fails_closed() {
        let provider: Arc<dyn OperatorIdentityProvider> =
            Arc::new(StaticOperatorIdentityProvider {
                claims: OperatorSessionClaims {
                    operator_id: OperatorId::parse("opr_breakglass01").expect("operator"),
                    issuer: "https://cloud.example.test/control-identity".to_owned(),
                    audiences: vec!["mako-operator".to_owned()],
                    permissions: BTreeSet::from([OperatorPermission::TenantRead]),
                    expires_at_unix_seconds: 200,
                },
            });
        let disabled = OperatorAuthenticator::new(
            Arc::clone(&provider),
            "https://cloud.example.test/control-identity",
            "mako-operator",
        )
        .expect("authenticator")
        .with_enabled(false);
        assert!(matches!(
            futures::executor::block_on(
                disabled.authenticate(Some("Bearer incident-token-01"), 100)
            ),
            Err(OperatorAuthenticationError::Disabled)
        ));

        let enabled = OperatorAuthenticator::new(
            Arc::clone(&provider),
            "https://cloud.example.test/control-identity",
            "mako-operator",
        )
        .expect("authenticator");
        assert!(
            futures::executor::block_on(
                enabled.authenticate(Some("Bearer incident-token-01"), 100)
            )
            .is_ok()
        );
        assert!(matches!(
            futures::executor::block_on(
                enabled.authenticate(Some("Bearer incident-token-01"), 200)
            ),
            Err(OperatorAuthenticationError::InvalidClaims)
        ));

        let wrong_audience = OperatorAuthenticator::new(
            provider,
            "https://cloud.example.test/control-identity",
            "another-operator-audience",
        )
        .expect("authenticator");
        assert!(matches!(
            futures::executor::block_on(
                wrong_audience.authenticate(Some("Bearer incident-token-01"), 100)
            ),
            Err(OperatorAuthenticationError::InvalidClaims)
        ));
    }

    #[test]
    fn operator_permissions_scope_repairs_abuse_and_expiring_support() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let projects =
                ProjectStore::new(adapter.clone(), Durability::Memory).expect("projects");
            let provisioning = Provisioner::new(
                mako_provisioning::ProvisioningStore::new(adapter.clone(), Durability::Memory)
                    .expect("provisioning"),
            );
            let project_id = ProjectId::parse("prj_example00").expect("project");
            let mut project = ProjectRecord::new(
                project_id.clone(),
                OrganizationId::parse("org_example00").expect("organization"),
                "Mako",
                "local",
                1,
            )
            .expect("project");
            project
                .transition(LifecycleState::Active, 2, None)
                .expect("active");
            projects.create_project(&project).await.expect("project");
            let audit = Arc::new(Audit::default());
            let service = OperatorService::new(
                adapter,
                Durability::Memory,
                projects.clone(),
                provisioning,
                audit.clone(),
            )
            .expect("service");
            let actor = OperatorPrincipal::for_test(
                OperatorId::parse("opr_security00").expect("operator"),
                [
                    OperatorPermission::TenantRead,
                    OperatorPermission::QuotaOverride,
                    OperatorPermission::AbuseResponse,
                    OperatorPermission::SupportAccess,
                ],
            );
            service
                .inspect_project(
                    &actor,
                    &project_id,
                    NonZeroUsize::new(10).expect("limit"),
                    3,
                )
                .await
                .expect("inspect");
            let quota = service
                .create_quota_override(
                    &actor,
                    NewQuotaOverride {
                        id: QuotaOverrideId::parse("qov_example00").expect("override"),
                        project_id: project_id.clone(),
                        resource: QuotaResource::StorageBytes,
                        limit: 10_000,
                        reason: "temporary incident allowance".to_owned(),
                        expires_at_unix_seconds: Some(3_600),
                        now_unix_seconds: 4,
                    },
                )
                .await
                .expect("quota");
            assert_eq!(quota.limit(), 10_000);
            let session = service
                .create_support_session(
                    &actor,
                    NewSupportSession {
                        id: SupportSessionId::parse("sup_example00").expect("session"),
                        project_id: project_id.clone(),
                        environment_id: None,
                        permissions: [SupportPermission::ProjectMetadataRead]
                            .into_iter()
                            .collect(),
                        reason: "investigate customer incident".to_owned(),
                        expires_at_unix_seconds: 100,
                        now_unix_seconds: 5,
                    },
                )
                .await
                .expect("session");
            service
                .verify_support_access(
                    session.id(),
                    &project_id,
                    None,
                    SupportPermission::ProjectMetadataRead,
                    6,
                )
                .await
                .expect("support");
            assert!(
                service
                    .verify_support_access(
                        session.id(),
                        &project_id,
                        None,
                        SupportPermission::ProjectMetadataRead,
                        100,
                    )
                    .await
                    .is_err()
            );
            service
                .respond_to_abuse(
                    &actor,
                    NewAbuseResponse {
                        id: AbuseResponseId::parse("abr_example00").expect("response"),
                        project_id: project_id.clone(),
                        target: AbuseTarget::Project,
                        reason: "confirmed abusive traffic".to_owned(),
                        now_unix_seconds: 7,
                    },
                )
                .await
                .expect("abuse response");
            assert_eq!(
                projects
                    .get_project(&project_id)
                    .await
                    .expect("project")
                    .expect("exists")
                    .lifecycle(),
                LifecycleState::Suspended
            );
            assert!(audit.0.lock().expect("audit").len() >= 5);
        });
    }

    #[test]
    fn provisioning_repair_rejects_cross_tenant_and_stale_review_with_denied_audit() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let projects =
                ProjectStore::new(adapter.clone(), Durability::Memory).expect("projects");
            let provisioning = Provisioner::new(
                mako_provisioning::ProvisioningStore::new(adapter.clone(), Durability::Memory)
                    .expect("provisioning"),
            );
            let expected_project = ProjectId::parse("prj_expected0").expect("project");
            let other_project = ProjectId::parse("prj_other000").expect("project");
            for project_id in [&expected_project, &other_project] {
                projects
                    .create_project(
                        &ProjectRecord::new(
                            project_id.clone(),
                            OrganizationId::parse("org_example00").expect("organization"),
                            project_id.as_str(),
                            "local",
                            1,
                        )
                        .expect("project"),
                    )
                    .await
                    .expect("create project");
            }
            let workflow_id = ProvisioningWorkflowId::parse("wf_scope00000").expect("workflow");
            provisioning
                .enqueue(
                    workflow_id.clone(),
                    ProvisioningResource::Project(expected_project.clone()),
                    mako_provisioning::ProvisioningOperation::Create,
                    10,
                )
                .await
                .expect("workflow");
            let audit = Arc::new(Audit::default());
            let service = OperatorService::new(
                adapter,
                Durability::Memory,
                projects,
                provisioning,
                audit.clone(),
            )
            .expect("service");
            let actor = OperatorPrincipal::for_test(
                OperatorId::parse("opr_repair000").expect("operator"),
                [OperatorPermission::ProvisioningRepair],
            );
            assert!(matches!(
                service
                    .repair_provisioning_guarded(
                        &actor,
                        &other_project,
                        &workflow_id,
                        "reviewed cross tenant repair",
                        OperatorRepairAction::Requeue,
                        Some(10),
                        Some("repair-operation-0001".to_owned()),
                        20,
                    )
                    .await,
                Err(OperatorError::ScopeMismatch)
            ));
            assert!(matches!(
                service
                    .repair_provisioning_guarded(
                        &actor,
                        &expected_project,
                        &workflow_id,
                        "reviewed stale tenant repair",
                        OperatorRepairAction::Requeue,
                        Some(9),
                        Some("repair-operation-0002".to_owned()),
                        21,
                    )
                    .await,
                Err(OperatorError::Conflict)
            ));
            let events = audit.0.lock().expect("audit");
            assert_eq!(events.len(), 2);
            assert!(
                events
                    .iter()
                    .all(|event| event.outcome == OperatorAuditOutcome::Denied)
            );
        });
    }
}
