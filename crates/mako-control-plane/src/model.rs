use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use mako_api::{EnvironmentId, ProjectId, QuotaResource};
use serde::{Deserialize, Deserializer, Serialize, de};

macro_rules! control_id {
    ($name:ident, $prefix:literal, $field:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, ControlModelError> {
                let value = value.into();
                validate_identifier($field, &value, $prefix)?;
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

control_id!(DeveloperIdentityId, "dev_", "developer identity id");
control_id!(OrganizationId, "org_", "organization id");
control_id!(InvitationId, "inv_", "invitation id");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRole {
    Viewer,
    Developer,
    Administrator,
    Owner,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectDataPermission {
    DataRead,
    DataAdmin,
    DocumentHistory,
    Import,
    Export,
    BackupRead,
    RestoreRequest,
}

impl OrganizationRole {
    #[must_use]
    pub fn data_permissions(self) -> BTreeSet<ProjectDataPermission> {
        use ProjectDataPermission::{
            BackupRead, DataAdmin, DataRead, DocumentHistory, Export, Import, RestoreRequest,
        };
        match self {
            Self::Viewer => BTreeSet::from([DataRead, BackupRead]),
            Self::Developer => {
                BTreeSet::from([DataRead, DocumentHistory, Import, Export, BackupRead])
            }
            Self::Administrator | Self::Owner => BTreeSet::from([
                DataRead,
                DataAdmin,
                DocumentHistory,
                Import,
                Export,
                BackupRead,
                RestoreRequest,
            ]),
        }
    }

    #[must_use]
    pub fn allows_data(self, permission: ProjectDataPermission) -> bool {
        self.data_permissions().contains(&permission)
    }

    #[must_use]
    pub const fn can_manage_members(self) -> bool {
        matches!(self, Self::Administrator | Self::Owner)
    }

    #[must_use]
    pub const fn can_mutate_projects(self) -> bool {
        matches!(self, Self::Developer | Self::Administrator | Self::Owner)
    }

    #[must_use]
    pub const fn can_delete_organization(self) -> bool {
        matches!(self, Self::Owner)
    }

    #[must_use]
    pub const fn can_assign(self, role: Self) -> bool {
        match self {
            Self::Owner => true,
            Self::Administrator => !matches!(role, Self::Owner),
            Self::Developer | Self::Viewer => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperIdentityStatus {
    Unverified,
    Waitlisted,
    Active,
    Rejected,
    Disabled,
    Deleted,
}

impl DeveloperIdentityStatus {
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        self as u8 == next as u8
            || matches!(
                (self, next),
                (
                    Self::Unverified,
                    Self::Waitlisted | Self::Rejected | Self::Deleted
                ) | (
                    Self::Waitlisted,
                    Self::Active | Self::Rejected | Self::Deleted
                ) | (Self::Active, Self::Disabled | Self::Deleted)
                    | (Self::Disabled, Self::Active | Self::Deleted)
            )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationStatus {
    Pending,
    Accepted,
    Revoked,
    Expired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Provisioning,
    Active,
    Suspended,
    Failed,
    DeletionGrace,
    Deleting,
    Deleted,
}

impl LifecycleState {
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        if self as u8 == next as u8 {
            return true;
        }
        matches!(
            (self, next),
            (
                Self::Provisioning,
                Self::Active | Self::Failed | Self::Suspended
            ) | (
                Self::Active,
                Self::Suspended | Self::Failed | Self::DeletionGrace
            ) | (Self::Suspended, Self::Active | Self::DeletionGrace)
                | (Self::Failed, Self::Provisioning | Self::DeletionGrace)
                | (Self::DeletionGrace, Self::Active | Self::Deleting)
                | (Self::Deleting, Self::Deleted)
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperIdentity {
    id: DeveloperIdentityId,
    issuer: String,
    subject: String,
    normalized_email: String,
    display_name: String,
    status: DeveloperIdentityStatus,
    created_at_unix_seconds: u64,
    last_authenticated_at_unix_seconds: u64,
}

impl DeveloperIdentity {
    pub fn new(
        id: DeveloperIdentityId,
        issuer: impl Into<String>,
        subject: impl Into<String>,
        normalized_email: impl Into<String>,
        display_name: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, ControlModelError> {
        let record = Self {
            id,
            issuer: issuer.into(),
            subject: subject.into(),
            normalized_email: normalized_email.into(),
            display_name: display_name.into(),
            status: DeveloperIdentityStatus::Active,
            created_at_unix_seconds: now_unix_seconds,
            last_authenticated_at_unix_seconds: now_unix_seconds,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), ControlModelError> {
        validate_text("identity issuer", &self.issuer, 1, 2_048)?;
        validate_text("identity subject", &self.subject, 1, 512)?;
        validate_email(&self.normalized_email)?;
        validate_text("display name", &self.display_name, 1, 200)?;
        if self.last_authenticated_at_unix_seconds < self.created_at_unix_seconds {
            return Err(ControlModelError::InvalidField {
                field: "last authenticated at",
                reason: "cannot precede creation",
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &DeveloperIdentityId {
        &self.id
    }

    #[must_use]
    pub fn normalized_email(&self) -> &str {
        &self.normalized_email
    }

    #[must_use]
    pub const fn status(&self) -> DeveloperIdentityStatus {
        self.status
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OrganizationRecord {
    id: OrganizationId,
    name: String,
    lifecycle: LifecycleState,
    /// Which plan this organization subscribes to.
    ///
    /// Defaulted for records written before plans existed, so every
    /// organization already stored is on the free plan rather than on nothing.
    #[serde(default = "free_plan_id")]
    plan_id: String,
    /// Every plan change, oldest first, so a billing period that spans one
    /// can be rated stretch by stretch. Empty means the plan above has held
    /// since the organization was created -- which is also what records
    /// written before the history existed correctly mean.
    #[serde(default)]
    plan_history: Vec<PlanChangeRecord>,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    deletion_deadline_unix_seconds: Option<u64>,
}

fn free_plan_id() -> String {
    "free".to_owned()
}

/// One recorded plan change: which plan the organization moved to, and when.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlanChangeRecord {
    pub plan_id: String,
    pub at_unix_seconds: u64,
}

/// Plan changes an organization keeps. Rating only ever looks a period or two
/// back, so the tail is enough; the audit log keeps the full story.
const PLAN_HISTORY_LIMIT: usize = 32;

impl OrganizationRecord {
    pub fn new(
        id: OrganizationId,
        name: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, ControlModelError> {
        let name = name.into();
        validate_text("organization name", &name, 1, 200)?;
        Ok(Self {
            id,
            name,
            lifecycle: LifecycleState::Active,
            plan_id: free_plan_id(),
            plan_history: Vec::new(),
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            deletion_deadline_unix_seconds: None,
        })
    }

    #[must_use]
    pub fn id(&self) -> &OrganizationId {
        &self.id
    }

    #[must_use]
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    /// Move this organization to another plan.
    ///
    /// Whether the plan exists is the caller's to check against the catalog;
    /// this only refuses shapes that could not name one.
    pub fn change_plan(
        &mut self,
        plan_id: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<(), ControlModelError> {
        let plan_id = plan_id.into();
        validate_text("plan id", &plan_id, 1, 64)?;
        self.plan_history.push(PlanChangeRecord {
            plan_id: plan_id.clone(),
            at_unix_seconds: now_unix_seconds,
        });
        if self.plan_history.len() > PLAN_HISTORY_LIMIT {
            self.plan_history.remove(0);
        }
        self.plan_id = plan_id;
        self.updated_at_unix_seconds = now_unix_seconds;
        Ok(())
    }

    /// The plans this organization held across a window, oldest first, as
    /// `(plan id, from unix seconds)` stretches. The first entry starts at or
    /// before the window; each later entry starts a new stretch inside it.
    #[must_use]
    pub fn plan_stretches(&self, from_unix_seconds: u64) -> Vec<(String, u64)> {
        // The plan in force at the window's start is the last change at or
        // before it -- or the original plan, which for every organization is
        // the free plan the record was created on.
        let mut initial = free_plan_id();
        let mut stretches = Vec::new();
        for change in &self.plan_history {
            if change.at_unix_seconds <= from_unix_seconds {
                initial = change.plan_id.clone();
            } else {
                stretches.push((change.plan_id.clone(), change.at_unix_seconds));
            }
        }
        stretches.insert(0, (initial, from_unix_seconds));
        stretches
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn lifecycle(&self) -> LifecycleState {
        self.lifecycle
    }

    #[must_use]
    pub const fn deletion_deadline_unix_seconds(&self) -> Option<u64> {
        self.deletion_deadline_unix_seconds
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    pub fn rename(
        &mut self,
        name: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<(), ControlModelError> {
        let name = name.into();
        validate_text("organization name", &name, 1, 200)?;
        if now_unix_seconds < self.updated_at_unix_seconds {
            return Err(ControlModelError::InvalidField {
                field: "organization timestamp",
                reason: "cannot move backwards",
            });
        }
        self.name = name;
        self.updated_at_unix_seconds = now_unix_seconds;
        Ok(())
    }

    pub fn request_deletion(
        &mut self,
        now_unix_seconds: u64,
        deadline_unix_seconds: u64,
    ) -> Result<(), ControlModelError> {
        if !self
            .lifecycle
            .can_transition_to(LifecycleState::DeletionGrace)
            || deadline_unix_seconds <= now_unix_seconds
        {
            return Err(ControlModelError::InvalidField {
                field: "organization deletion",
                reason: "requires an active lifecycle and a future deadline",
            });
        }
        self.lifecycle = LifecycleState::DeletionGrace;
        self.updated_at_unix_seconds = now_unix_seconds;
        self.deletion_deadline_unix_seconds = Some(deadline_unix_seconds);
        Ok(())
    }

    pub fn restore(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        if !self.lifecycle.can_transition_to(LifecycleState::Active) {
            return Err(ControlModelError::InvalidLifecycleTransition {
                from: self.lifecycle,
                to: LifecycleState::Active,
            });
        }
        self.lifecycle = LifecycleState::Active;
        self.updated_at_unix_seconds = now_unix_seconds;
        self.deletion_deadline_unix_seconds = None;
        Ok(())
    }

    pub fn begin_final_deletion(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        if self.lifecycle != LifecycleState::DeletionGrace
            || self
                .deletion_deadline_unix_seconds
                .is_none_or(|deadline| now_unix_seconds < deadline)
        {
            return Err(ControlModelError::InvalidLifecycleTransition {
                from: self.lifecycle,
                to: LifecycleState::Deleting,
            });
        }
        self.lifecycle = LifecycleState::Deleting;
        self.updated_at_unix_seconds = now_unix_seconds;
        Ok(())
    }

    pub fn complete_deletion(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        if self.lifecycle != LifecycleState::Deleting {
            return Err(ControlModelError::InvalidLifecycleTransition {
                from: self.lifecycle,
                to: LifecycleState::Deleted,
            });
        }
        self.lifecycle = LifecycleState::Deleted;
        self.updated_at_unix_seconds = now_unix_seconds;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MembershipRecord {
    organization_id: OrganizationId,
    developer_identity_id: DeveloperIdentityId,
    role: OrganizationRole,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

impl MembershipRecord {
    #[must_use]
    pub fn new(
        organization_id: OrganizationId,
        developer_identity_id: DeveloperIdentityId,
        role: OrganizationRole,
        now_unix_seconds: u64,
    ) -> Self {
        Self {
            organization_id,
            developer_identity_id,
            role,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
        }
    }

    #[must_use]
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    #[must_use]
    pub fn developer_identity_id(&self) -> &DeveloperIdentityId {
        &self.developer_identity_id
    }

    #[must_use]
    pub const fn role(&self) -> OrganizationRole {
        self.role
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    pub fn change_role(&mut self, role: OrganizationRole, now_unix_seconds: u64) {
        self.role = role;
        self.updated_at_unix_seconds = now_unix_seconds;
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InvitationRecord {
    id: InvitationId,
    organization_id: OrganizationId,
    normalized_email: String,
    role: OrganizationRole,
    invited_by: DeveloperIdentityId,
    token_digest: String,
    status: InvitationStatus,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    accepted_by: Option<DeveloperIdentityId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvitationInput {
    pub id: InvitationId,
    pub organization_id: OrganizationId,
    pub normalized_email: String,
    pub role: OrganizationRole,
    pub invited_by: DeveloperIdentityId,
    pub token_digest: String,
    pub created_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

impl InvitationRecord {
    pub fn new(input: InvitationInput) -> Result<Self, ControlModelError> {
        let record = Self {
            id: input.id,
            organization_id: input.organization_id,
            normalized_email: input.normalized_email,
            role: input.role,
            invited_by: input.invited_by,
            token_digest: input.token_digest,
            status: InvitationStatus::Pending,
            created_at_unix_seconds: input.created_at_unix_seconds,
            expires_at_unix_seconds: input.expires_at_unix_seconds,
            accepted_by: None,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), ControlModelError> {
        validate_email(&self.normalized_email)?;
        validate_text("invitation token digest", &self.token_digest, 32, 256)?;
        if self.expires_at_unix_seconds <= self.created_at_unix_seconds {
            return Err(ControlModelError::InvalidField {
                field: "invitation expiry",
                reason: "must follow creation",
            });
        }
        if matches!(self.status, InvitationStatus::Accepted) != self.accepted_by.is_some() {
            return Err(ControlModelError::InvalidField {
                field: "invitation acceptance",
                reason: "accepted status and actor must be recorded together",
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &InvitationId {
        &self.id
    }

    #[must_use]
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    #[must_use]
    pub const fn status(&self) -> InvitationStatus {
        self.status
    }

    #[must_use]
    pub fn normalized_email(&self) -> &str {
        &self.normalized_email
    }

    #[must_use]
    pub const fn role(&self) -> OrganizationRole {
        self.role
    }

    #[must_use]
    pub fn token_digest(&self) -> &str {
        &self.token_digest
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    pub fn accept(
        &mut self,
        accepted_by: DeveloperIdentityId,
        now_unix_seconds: u64,
    ) -> Result<(), ControlModelError> {
        if self.status != InvitationStatus::Pending
            || now_unix_seconds >= self.expires_at_unix_seconds
        {
            return Err(ControlModelError::InvalidField {
                field: "invitation",
                reason: "is not pending or has expired",
            });
        }
        self.status = InvitationStatus::Accepted;
        self.accepted_by = Some(accepted_by);
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectRecord {
    id: ProjectId,
    organization_id: OrganizationId,
    name: String,
    region: String,
    lifecycle: LifecycleState,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    failure_diagnostic: Option<String>,
    deletion_deadline_unix_seconds: Option<u64>,
}

impl ProjectRecord {
    pub fn new(
        id: ProjectId,
        organization_id: OrganizationId,
        name: impl Into<String>,
        region: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, ControlModelError> {
        let name = name.into();
        let region = region.into();
        validate_text("project name", &name, 1, 200)?;
        validate_region(&region)?;
        Ok(Self {
            id,
            organization_id,
            name,
            region,
            lifecycle: LifecycleState::Provisioning,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            failure_diagnostic: None,
            deletion_deadline_unix_seconds: None,
        })
    }

    #[must_use]
    pub fn id(&self) -> &ProjectId {
        &self.id
    }

    #[must_use]
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    #[must_use]
    pub const fn lifecycle(&self) -> LifecycleState {
        self.lifecycle
    }

    #[must_use]
    pub const fn deletion_deadline_unix_seconds(&self) -> Option<u64> {
        self.deletion_deadline_unix_seconds
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    #[must_use]
    pub fn failure_diagnostic(&self) -> Option<&str> {
        self.failure_diagnostic.as_deref()
    }

    pub fn transition(
        &mut self,
        next: LifecycleState,
        now_unix_seconds: u64,
        failure_diagnostic: Option<String>,
    ) -> Result<(), ControlModelError> {
        transition_lifecycle(
            &mut self.lifecycle,
            &mut self.updated_at_unix_seconds,
            &mut self.failure_diagnostic,
            next,
            now_unix_seconds,
            failure_diagnostic,
        )
    }

    pub fn suspend(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        self.transition(LifecycleState::Suspended, now_unix_seconds, None)
    }

    pub fn restore(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        self.transition(LifecycleState::Active, now_unix_seconds, None)?;
        self.deletion_deadline_unix_seconds = None;
        Ok(())
    }

    pub fn request_deletion(
        &mut self,
        now_unix_seconds: u64,
        deadline_unix_seconds: u64,
    ) -> Result<(), ControlModelError> {
        if deadline_unix_seconds <= now_unix_seconds {
            return Err(ControlModelError::InvalidField {
                field: "project deletion deadline",
                reason: "must be in the future",
            });
        }
        self.transition(LifecycleState::DeletionGrace, now_unix_seconds, None)?;
        self.deletion_deadline_unix_seconds = Some(deadline_unix_seconds);
        Ok(())
    }

    pub fn begin_final_deletion(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        if self.lifecycle != LifecycleState::DeletionGrace
            || self
                .deletion_deadline_unix_seconds
                .is_none_or(|deadline| now_unix_seconds < deadline)
        {
            return Err(ControlModelError::InvalidLifecycleTransition {
                from: self.lifecycle,
                to: LifecycleState::Deleting,
            });
        }
        self.transition(LifecycleState::Deleting, now_unix_seconds, None)
    }

    pub fn complete_deletion(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        self.transition(LifecycleState::Deleted, now_unix_seconds, None)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvironmentRecord {
    id: EnvironmentId,
    project_id: ProjectId,
    name: String,
    lifecycle: LifecycleState,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    failure_diagnostic: Option<String>,
    deletion_deadline_unix_seconds: Option<u64>,
}

impl EnvironmentRecord {
    pub fn new(
        id: EnvironmentId,
        project_id: ProjectId,
        name: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, ControlModelError> {
        let name = name.into();
        validate_text("environment name", &name, 1, 100)?;
        Ok(Self {
            id,
            project_id,
            name,
            lifecycle: LifecycleState::Provisioning,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            failure_diagnostic: None,
            deletion_deadline_unix_seconds: None,
        })
    }

    #[must_use]
    pub fn id(&self) -> &EnvironmentId {
        &self.id
    }

    #[must_use]
    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn lifecycle(&self) -> LifecycleState {
        self.lifecycle
    }

    #[must_use]
    pub const fn deletion_deadline_unix_seconds(&self) -> Option<u64> {
        self.deletion_deadline_unix_seconds
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    #[must_use]
    pub fn failure_diagnostic(&self) -> Option<&str> {
        self.failure_diagnostic.as_deref()
    }

    pub fn transition(
        &mut self,
        next: LifecycleState,
        now_unix_seconds: u64,
        failure_diagnostic: Option<String>,
    ) -> Result<(), ControlModelError> {
        transition_lifecycle(
            &mut self.lifecycle,
            &mut self.updated_at_unix_seconds,
            &mut self.failure_diagnostic,
            next,
            now_unix_seconds,
            failure_diagnostic,
        )
    }

    pub fn suspend(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        self.transition(LifecycleState::Suspended, now_unix_seconds, None)
    }

    pub fn restore(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        self.transition(LifecycleState::Active, now_unix_seconds, None)?;
        self.deletion_deadline_unix_seconds = None;
        Ok(())
    }

    pub fn request_deletion(
        &mut self,
        now_unix_seconds: u64,
        deadline_unix_seconds: u64,
    ) -> Result<(), ControlModelError> {
        if deadline_unix_seconds <= now_unix_seconds {
            return Err(ControlModelError::InvalidField {
                field: "environment deletion deadline",
                reason: "must be in the future",
            });
        }
        self.transition(LifecycleState::DeletionGrace, now_unix_seconds, None)?;
        self.deletion_deadline_unix_seconds = Some(deadline_unix_seconds);
        Ok(())
    }

    pub fn begin_final_deletion(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        if self.lifecycle != LifecycleState::DeletionGrace
            || self
                .deletion_deadline_unix_seconds
                .is_none_or(|deadline| now_unix_seconds < deadline)
        {
            return Err(ControlModelError::InvalidLifecycleTransition {
                from: self.lifecycle,
                to: LifecycleState::Deleting,
            });
        }
        self.transition(LifecycleState::Deleting, now_unix_seconds, None)
    }

    pub fn complete_deletion(&mut self, now_unix_seconds: u64) -> Result<(), ControlModelError> {
        self.transition(LifecycleState::Deleted, now_unix_seconds, None)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct QuotaSet {
    project_id: ProjectId,
    limits: BTreeMap<QuotaResource, u64>,
    version: u64,
    updated_at_unix_seconds: u64,
}

impl QuotaSet {
    pub fn new(
        project_id: ProjectId,
        limits: BTreeMap<QuotaResource, u64>,
        updated_at_unix_seconds: u64,
    ) -> Result<Self, ControlModelError> {
        if limits.is_empty() || limits.values().any(|limit| *limit == 0) {
            return Err(ControlModelError::InvalidField {
                field: "quota limits",
                reason: "must contain only positive limits",
            });
        }
        Ok(Self {
            project_id,
            limits,
            version: 1,
            updated_at_unix_seconds,
        })
    }

    #[must_use]
    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    #[must_use]
    pub fn limit(&self, resource: QuotaResource) -> Option<u64> {
        self.limits.get(&resource).copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlModelError {
    InvalidIdentifier {
        field: &'static str,
    },
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    InvalidLifecycleTransition {
        from: LifecycleState,
        to: LifecycleState,
    },
}

impl fmt::Display for ControlModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { field } => write!(formatter, "{field} is invalid"),
            Self::InvalidField { field, reason } => write!(formatter, "{field} {reason}"),
            Self::InvalidLifecycleTransition { from, to } => {
                write!(
                    formatter,
                    "cannot transition lifecycle from {from:?} to {to:?}"
                )
            }
        }
    }
}

impl Error for ControlModelError {}

fn validate_identifier(
    field: &'static str,
    value: &str,
    prefix: &str,
) -> Result<(), ControlModelError> {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return Err(ControlModelError::InvalidIdentifier { field });
    };
    if !(8..=64).contains(&suffix.len())
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ControlModelError::InvalidIdentifier { field });
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), ControlModelError> {
    if !(minimum..=maximum).contains(&value.len())
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(ControlModelError::InvalidField {
            field,
            reason: "has invalid length or characters",
        });
    }
    Ok(())
}

fn validate_email(value: &str) -> Result<(), ControlModelError> {
    validate_text("normalized email", value, 3, 320)?;
    if value.bytes().any(|byte| byte.is_ascii_uppercase())
        || value.split_once('@').is_none_or(|(local, domain)| {
            local.is_empty() || domain.is_empty() || !domain.contains('.')
        })
    {
        return Err(ControlModelError::InvalidField {
            field: "normalized email",
            reason: "must be a normalized address",
        });
    }
    Ok(())
}

fn validate_region(value: &str) -> Result<(), ControlModelError> {
    validate_text("region", value, 2, 64)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ControlModelError::InvalidField {
            field: "region",
            reason: "must use lowercase letters, numbers, and hyphens",
        });
    }
    Ok(())
}

fn transition_lifecycle(
    current: &mut LifecycleState,
    updated_at: &mut u64,
    current_failure: &mut Option<String>,
    next: LifecycleState,
    now_unix_seconds: u64,
    failure_diagnostic: Option<String>,
) -> Result<(), ControlModelError> {
    if !current.can_transition_to(next) {
        return Err(ControlModelError::InvalidLifecycleTransition {
            from: *current,
            to: next,
        });
    }
    if now_unix_seconds < *updated_at {
        return Err(ControlModelError::InvalidField {
            field: "lifecycle timestamp",
            reason: "cannot move backwards",
        });
    }
    if let Some(diagnostic) = &failure_diagnostic {
        validate_text("failure diagnostic", diagnostic, 1, 1_024)?;
    }
    if matches!(next, LifecycleState::Failed) != failure_diagnostic.is_some() {
        return Err(ControlModelError::InvalidField {
            field: "failure diagnostic",
            reason: "is required only for failed state",
        });
    }
    *current = next;
    *updated_at = now_unix_seconds;
    *current_failure = failure_diagnostic;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Organizations stored before plans existed have no planId field. They
    /// must come back on the free plan, not fail to load -- a deployment's
    /// existing customers losing their organizations on upgrade would be far
    /// worse than any plan bug.
    #[test]
    fn an_organization_stored_before_plans_existed_is_on_the_free_plan() {
        let stored = r#"{
            "id": "org_prehistoric0",
            "name": "Stored Before Plans",
            "lifecycle": "active",
            "createdAtUnixSeconds": 1,
            "updatedAtUnixSeconds": 1,
            "deletionDeadlineUnixSeconds": null
        }"#;
        let record: OrganizationRecord = serde_json::from_str(stored).expect("old record loads");
        assert_eq!(record.plan_id(), "free");

        // And once written back, the field is explicit.
        let rewritten = serde_json::to_string(&record).expect("serialize");
        assert!(rewritten.contains("\"planId\":\"free\""));
        // A record without a history has held its plan since creation.
        assert_eq!(record.plan_stretches(5), vec![("free".to_owned(), 5)]);
    }

    /// Rating a period needs to know which plan held when, so a change must
    /// leave a stretch boundary behind, and the plan in force at a window's
    /// start must be the last change at or before it.
    #[test]
    fn plan_changes_leave_stretches_a_billing_period_can_be_split_by() {
        let mut record = OrganizationRecord::new(
            OrganizationId::parse("org_stretches01").expect("id"),
            "Stretches",
            10,
        )
        .expect("record");
        record.change_plan("pro", 100).expect("upgrade");
        record.change_plan("free", 200).expect("downgrade");

        // A window opening mid-way starts on the plan then in force.
        assert_eq!(
            record.plan_stretches(150),
            vec![("pro".to_owned(), 150), ("free".to_owned(), 200)]
        );
        // A window opening before any change starts on the original plan.
        assert_eq!(
            record.plan_stretches(50),
            vec![
                ("free".to_owned(), 50),
                ("pro".to_owned(), 100),
                ("free".to_owned(), 200)
            ]
        );
    }

    #[test]
    fn role_capabilities_preserve_viewer_and_owner_boundaries() {
        assert!(!OrganizationRole::Viewer.can_mutate_projects());
        assert!(OrganizationRole::Developer.can_mutate_projects());
        assert!(!OrganizationRole::Administrator.can_assign(OrganizationRole::Owner));
        assert!(OrganizationRole::Owner.can_delete_organization());
    }

    #[test]
    fn data_permissions_are_least_privilege_by_default() {
        assert!(OrganizationRole::Viewer.allows_data(ProjectDataPermission::DataRead));
        assert!(!OrganizationRole::Viewer.allows_data(ProjectDataPermission::Import));
        assert!(OrganizationRole::Developer.allows_data(ProjectDataPermission::Export));
        assert!(!OrganizationRole::Developer.allows_data(ProjectDataPermission::DataAdmin));
        assert!(OrganizationRole::Administrator.allows_data(ProjectDataPermission::RestoreRequest));
        assert!(OrganizationRole::Owner.allows_data(ProjectDataPermission::DataAdmin));
    }

    #[test]
    fn lifecycle_rejects_skipping_deletion_grace() {
        let mut project = ProjectRecord::new(
            ProjectId::parse("prj_abcdefgh").expect("project id"),
            OrganizationId::parse("org_abcdefgh").expect("organization id"),
            "Example",
            "us-east-1",
            10,
        )
        .expect("project");
        project
            .transition(LifecycleState::Active, 11, None)
            .expect("activation");
        assert!(matches!(
            project.transition(LifecycleState::Deleting, 12, None),
            Err(ControlModelError::InvalidLifecycleTransition { .. })
        ));
    }

    #[test]
    fn deserialization_validates_control_identifiers() {
        assert!(serde_json::from_str::<OrganizationId>(r#""org_bad""#).is_err());
    }

    #[test]
    fn invitation_never_contains_a_raw_secret_field() {
        let invitation = InvitationRecord::new(InvitationInput {
            id: InvitationId::parse("inv_abcdefgh").expect("invitation id"),
            organization_id: OrganizationId::parse("org_abcdefgh").expect("organization id"),
            normalized_email: "developer@example.test".to_owned(),
            role: OrganizationRole::Developer,
            invited_by: DeveloperIdentityId::parse("dev_abcdefgh").expect("developer id"),
            token_digest: "b3:12345678901234567890123456789012".to_owned(),
            created_at_unix_seconds: 10,
            expires_at_unix_seconds: 20,
        })
        .expect("invitation");
        let encoded = serde_json::to_value(invitation).expect("serialize");
        assert!(encoded.get("tokenDigest").is_some());
        assert!(encoded.get("token").is_none());
    }
}
