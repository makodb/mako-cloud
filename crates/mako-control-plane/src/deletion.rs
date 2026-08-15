use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use mako_api::{EnvironmentId, ProjectId};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, StorageError,
    WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlKeyspace, ControlKeyspaceError, ControlModelError, DeveloperPrincipal,
    EnvironmentRecord, OrganizationId, OrganizationRecord, OrganizationStore,
    OrganizationStoreError, ProjectRecord, ProjectStore, ProjectStoreError,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DeletionScope {
    Organization {
        organization_id: OrganizationId,
    },
    Project {
        project_id: ProjectId,
    },
    Environment {
        project_id: ProjectId,
        environment_id: EnvironmentId,
    },
}

impl DeletionScope {
    fn key(&self) -> String {
        match self {
            Self::Organization { organization_id } => {
                format!("organization:{}", organization_id.as_str())
            }
            Self::Project { project_id } => format!("project:{}", project_id.as_str()),
            Self::Environment {
                project_id,
                environment_id,
            } => format!(
                "environment:{}:{}",
                project_id.as_str(),
                environment_id.as_str()
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionWorkflowState {
    RevocationPending,
    Grace,
    RestorationPending,
    Restored,
    Destroying,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeletionWorkflow {
    scope: DeletionScope,
    state: DeletionWorkflowState,
    deadline_unix_seconds: u64,
    access_revoked: bool,
    secrets_destroyed: bool,
    data_destroyed: bool,
    requested_by: String,
    requested_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    completed_at_unix_seconds: Option<u64>,
}

impl DeletionWorkflow {
    #[must_use]
    pub fn scope(&self) -> &DeletionScope {
        &self.scope
    }

    #[must_use]
    pub const fn state(&self) -> DeletionWorkflowState {
        self.state
    }

    #[must_use]
    pub const fn deadline_unix_seconds(&self) -> u64 {
        self.deadline_unix_seconds
    }

    #[must_use]
    pub const fn destruction_complete(&self) -> bool {
        self.secrets_destroyed && self.data_destroyed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeletionAuditAction {
    Requested,
    AccessRevoked,
    Restored,
    SecretsDestroyed,
    DataDestroyed,
    Completed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeletionAuditEvent {
    pub actor_id: String,
    pub scope: DeletionScope,
    pub action: DeletionAuditAction,
    pub at_unix_seconds: u64,
}

pub trait DeletionAuditSink: Send + Sync {
    fn record(&self, event: DeletionAuditEvent);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeletionBackendError {
    Unavailable,
    Failed,
}

impl fmt::Display for DeletionBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "deletion backend is unavailable",
            Self::Failed => "deletion backend operation failed",
        })
    }
}

impl Error for DeletionBackendError {}

/// Every method must be idempotent because a successful external operation can
/// be retried after a control-record compare-and-write conflict.
#[async_trait]
pub trait DeletionBackend: Send + Sync {
    async fn revoke_data_plane(&self, scope: &DeletionScope) -> Result<(), DeletionBackendError>;
    async fn restore_data_plane(&self, scope: &DeletionScope) -> Result<(), DeletionBackendError>;
    async fn destroy_secrets(&self, scope: &DeletionScope) -> Result<(), DeletionBackendError>;
    async fn destroy_data(&self, scope: &DeletionScope) -> Result<(), DeletionBackendError>;
}

#[derive(Clone)]
pub struct DeletionLifecycleService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    organizations: OrganizationStore,
    projects: ProjectStore,
    backend: Arc<dyn DeletionBackend>,
    audit: Arc<dyn DeletionAuditSink>,
}

impl fmt::Debug for DeletionLifecycleService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeletionLifecycleService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl DeletionLifecycleService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        organizations: OrganizationStore,
        projects: ProjectStore,
        backend: Arc<dyn DeletionBackend>,
        audit: Arc<dyn DeletionAuditSink>,
    ) -> Result<Self, DeletionError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(DeletionError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            organizations,
            projects,
            backend,
            audit,
        })
    }

    pub async fn request_deletion(
        &self,
        actor: &DeveloperPrincipal,
        scope: DeletionScope,
        deadline_unix_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<DeletionWorkflow, DeletionError> {
        if deadline_unix_seconds <= now_unix_seconds {
            return Err(DeletionError::InvalidDeadline);
        }
        let previous_resource = self.resource(&scope).await?;
        self.require_owner(actor, previous_resource.organization_id())
            .await?;
        let mut next_resource = previous_resource.clone();
        next_resource.request_deletion(now_unix_seconds, deadline_unix_seconds)?;
        let workflow = DeletionWorkflow {
            scope: scope.clone(),
            state: DeletionWorkflowState::RevocationPending,
            deadline_unix_seconds,
            access_revoked: false,
            secrets_destroyed: false,
            data_destroyed: false,
            requested_by: actor.identity_id().as_str().to_owned(),
            requested_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            completed_at_unix_seconds: None,
        };
        let workflow_key = workflow_key(&scope)?;
        let existing = self.read_workflow(&scope).await?;
        if existing
            .as_ref()
            .is_some_and(|record| record.state != DeletionWorkflowState::Restored)
        {
            return Err(DeletionError::InvalidState);
        }
        let mut conditions = vec![KeyCondition::ValueEquals {
            key: previous_resource.key()?,
            value: previous_resource.encode()?,
        }];
        if let Some(existing) = existing {
            conditions.push(KeyCondition::ValueEquals {
                key: workflow_key.clone(),
                value: serde_json::to_vec(&existing)?,
            });
        } else {
            conditions.push(KeyCondition::Missing {
                key: workflow_key.clone(),
            });
        }
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&previous_resource.key()?, next_resource.encode()?);
        batch.put(&workflow_key, serde_json::to_vec(&workflow)?);
        self.apply(AtomicWrite {
            conditions,
            batch,
            durability: self.durability,
        })
        .await?;
        self.audit.record(DeletionAuditEvent {
            actor_id: actor.identity_id().as_str().to_owned(),
            scope: scope.clone(),
            action: DeletionAuditAction::Requested,
            at_unix_seconds: now_unix_seconds,
        });
        self.complete_revocation(&workflow, now_unix_seconds).await
    }

    pub async fn retry_revocation(
        &self,
        scope: &DeletionScope,
        now_unix_seconds: u64,
    ) -> Result<DeletionWorkflow, DeletionError> {
        let workflow = self.workflow(scope).await?;
        if workflow.state != DeletionWorkflowState::RevocationPending {
            return Err(DeletionError::InvalidState);
        }
        self.complete_revocation(&workflow, now_unix_seconds).await
    }

    async fn complete_revocation(
        &self,
        previous: &DeletionWorkflow,
        now_unix_seconds: u64,
    ) -> Result<DeletionWorkflow, DeletionError> {
        self.backend.revoke_data_plane(&previous.scope).await?;
        let mut next = previous.clone();
        next.state = DeletionWorkflowState::Grace;
        next.access_revoked = true;
        next.updated_at_unix_seconds = now_unix_seconds;
        self.replace_workflow(previous, &next).await?;
        self.audit.record(DeletionAuditEvent {
            actor_id: "system/deletion-worker".to_owned(),
            scope: next.scope.clone(),
            action: DeletionAuditAction::AccessRevoked,
            at_unix_seconds: now_unix_seconds,
        });
        Ok(next)
    }

    pub async fn restore(
        &self,
        actor: &DeveloperPrincipal,
        scope: &DeletionScope,
        now_unix_seconds: u64,
    ) -> Result<DeletionWorkflow, DeletionError> {
        let previous_workflow = self.workflow(scope).await?;
        if previous_workflow.state != DeletionWorkflowState::Grace
            || now_unix_seconds >= previous_workflow.deadline_unix_seconds
        {
            return Err(DeletionError::GraceExpired);
        }
        let previous_resource = self.resource(scope).await?;
        self.require_owner(actor, previous_resource.organization_id())
            .await?;
        let mut next_resource = previous_resource.clone();
        next_resource.restore(now_unix_seconds)?;
        let mut next_workflow = previous_workflow.clone();
        next_workflow.state = DeletionWorkflowState::RestorationPending;
        next_workflow.updated_at_unix_seconds = now_unix_seconds;
        self.replace_resource_and_workflow(
            &previous_resource,
            &next_resource,
            &previous_workflow,
            &next_workflow,
        )
        .await?;
        self.complete_restoration(&next_workflow, now_unix_seconds)
            .await
    }

    pub async fn retry_restoration(
        &self,
        scope: &DeletionScope,
        now_unix_seconds: u64,
    ) -> Result<DeletionWorkflow, DeletionError> {
        let workflow = self.workflow(scope).await?;
        if workflow.state != DeletionWorkflowState::RestorationPending {
            return Err(DeletionError::InvalidState);
        }
        self.complete_restoration(&workflow, now_unix_seconds).await
    }

    async fn complete_restoration(
        &self,
        previous: &DeletionWorkflow,
        now_unix_seconds: u64,
    ) -> Result<DeletionWorkflow, DeletionError> {
        self.backend.restore_data_plane(&previous.scope).await?;
        let mut next = previous.clone();
        next.state = DeletionWorkflowState::Restored;
        next.access_revoked = false;
        next.updated_at_unix_seconds = now_unix_seconds;
        self.replace_workflow(previous, &next).await?;
        self.audit.record(DeletionAuditEvent {
            actor_id: next.requested_by.clone(),
            scope: next.scope.clone(),
            action: DeletionAuditAction::Restored,
            at_unix_seconds: now_unix_seconds,
        });
        Ok(next)
    }

    pub async fn finalize(
        &self,
        scope: &DeletionScope,
        now_unix_seconds: u64,
    ) -> Result<DeletionWorkflow, DeletionError> {
        let previous_workflow = self.workflow(scope).await?;
        if previous_workflow.state == DeletionWorkflowState::Destroying {
            return self
                .continue_destruction(previous_workflow, now_unix_seconds)
                .await;
        }
        if previous_workflow.state != DeletionWorkflowState::Grace {
            return Err(DeletionError::InvalidState);
        }
        if now_unix_seconds < previous_workflow.deadline_unix_seconds {
            return Err(DeletionError::GraceNotExpired);
        }
        let previous_resource = self.resource(scope).await?;
        let mut next_resource = previous_resource.clone();
        next_resource.begin_final_deletion(now_unix_seconds)?;
        let mut next_workflow = previous_workflow.clone();
        next_workflow.state = DeletionWorkflowState::Destroying;
        next_workflow.updated_at_unix_seconds = now_unix_seconds;
        self.replace_resource_and_workflow(
            &previous_resource,
            &next_resource,
            &previous_workflow,
            &next_workflow,
        )
        .await?;
        self.continue_destruction(next_workflow, now_unix_seconds)
            .await
    }

    async fn continue_destruction(
        &self,
        mut workflow: DeletionWorkflow,
        now_unix_seconds: u64,
    ) -> Result<DeletionWorkflow, DeletionError> {
        if !workflow.secrets_destroyed {
            self.backend.destroy_secrets(&workflow.scope).await?;
            let previous = workflow.clone();
            workflow.secrets_destroyed = true;
            workflow.updated_at_unix_seconds = now_unix_seconds;
            self.replace_workflow(&previous, &workflow).await?;
            self.audit.record(DeletionAuditEvent {
                actor_id: "system/deletion-worker".to_owned(),
                scope: workflow.scope.clone(),
                action: DeletionAuditAction::SecretsDestroyed,
                at_unix_seconds: now_unix_seconds,
            });
        }
        if !workflow.data_destroyed {
            self.backend.destroy_data(&workflow.scope).await?;
            let previous = workflow.clone();
            workflow.data_destroyed = true;
            workflow.updated_at_unix_seconds = now_unix_seconds;
            self.replace_workflow(&previous, &workflow).await?;
            self.audit.record(DeletionAuditEvent {
                actor_id: "system/deletion-worker".to_owned(),
                scope: workflow.scope.clone(),
                action: DeletionAuditAction::DataDestroyed,
                at_unix_seconds: now_unix_seconds,
            });
        }
        let previous_resource = self.resource(&workflow.scope).await?;
        let mut next_resource = previous_resource.clone();
        next_resource.complete_deletion(now_unix_seconds)?;
        let previous_workflow = workflow.clone();
        workflow.state = DeletionWorkflowState::Completed;
        workflow.updated_at_unix_seconds = now_unix_seconds;
        workflow.completed_at_unix_seconds = Some(now_unix_seconds);
        self.replace_resource_and_workflow(
            &previous_resource,
            &next_resource,
            &previous_workflow,
            &workflow,
        )
        .await?;
        self.audit.record(DeletionAuditEvent {
            actor_id: "system/deletion-worker".to_owned(),
            scope: workflow.scope.clone(),
            action: DeletionAuditAction::Completed,
            at_unix_seconds: now_unix_seconds,
        });
        Ok(workflow)
    }

    pub async fn workflow(&self, scope: &DeletionScope) -> Result<DeletionWorkflow, DeletionError> {
        self.read_workflow(scope)
            .await?
            .ok_or(DeletionError::NotFound)
    }

    async fn read_workflow(
        &self,
        scope: &DeletionScope,
    ) -> Result<Option<DeletionWorkflow>, DeletionError> {
        let record = self
            .adapter
            .get(&workflow_key(scope)?)
            .await?
            .map(|bytes| serde_json::from_slice::<DeletionWorkflow>(&bytes))
            .transpose()?;
        if record.as_ref().is_some_and(|record| record.scope != *scope) {
            return Err(DeletionError::CorruptRecord);
        }
        Ok(record)
    }

    async fn resource(&self, scope: &DeletionScope) -> Result<ScopedResource, DeletionError> {
        match scope {
            DeletionScope::Organization { organization_id } => self
                .organizations
                .get_organization(organization_id)
                .await?
                .map(ScopedResource::Organization)
                .ok_or(DeletionError::NotFound),
            DeletionScope::Project { project_id } => self
                .projects
                .get_project(project_id)
                .await?
                .map(ScopedResource::Project)
                .ok_or(DeletionError::NotFound),
            DeletionScope::Environment {
                project_id,
                environment_id,
            } => {
                let project = self
                    .projects
                    .get_project(project_id)
                    .await?
                    .ok_or(DeletionError::NotFound)?;
                let environment = self
                    .projects
                    .get_environment(project_id, environment_id)
                    .await?
                    .ok_or(DeletionError::NotFound)?;
                Ok(ScopedResource::Environment {
                    organization_id: project.organization_id().clone(),
                    record: environment,
                })
            }
        }
    }

    async fn require_owner(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
    ) -> Result<(), DeletionError> {
        let membership = self
            .organizations
            .get_membership(organization_id, actor.identity_id())
            .await?;
        if membership
            .as_ref()
            .is_none_or(|membership| !membership.role().can_delete_organization())
        {
            return Err(DeletionError::Forbidden);
        }
        Ok(())
    }

    async fn replace_workflow(
        &self,
        previous: &DeletionWorkflow,
        next: &DeletionWorkflow,
    ) -> Result<(), DeletionError> {
        let key = workflow_key(&previous.scope)?;
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

    async fn replace_resource_and_workflow(
        &self,
        previous_resource: &ScopedResource,
        next_resource: &ScopedResource,
        previous_workflow: &DeletionWorkflow,
        next_workflow: &DeletionWorkflow,
    ) -> Result<(), DeletionError> {
        let resource_key = previous_resource.key()?;
        let workflow_key = workflow_key(&previous_workflow.scope)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&resource_key, next_resource.encode()?);
        batch.put(&workflow_key, serde_json::to_vec(next_workflow)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: resource_key,
                    value: previous_resource.encode()?,
                },
                KeyCondition::ValueEquals {
                    key: workflow_key,
                    value: serde_json::to_vec(previous_workflow)?,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), DeletionError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(DeletionError::Conflict),
        }
    }
}

#[derive(Clone, Debug)]
enum ScopedResource {
    Organization(OrganizationRecord),
    Project(ProjectRecord),
    Environment {
        organization_id: OrganizationId,
        record: EnvironmentRecord,
    },
}

impl ScopedResource {
    fn organization_id(&self) -> &OrganizationId {
        match self {
            Self::Organization(record) => record.id(),
            Self::Project(record) => record.organization_id(),
            Self::Environment {
                organization_id, ..
            } => organization_id,
        }
    }

    fn key(&self) -> Result<Vec<u8>, DeletionError> {
        match self {
            Self::Organization(record) => {
                ControlKeyspace::organization_key(record.id()).map_err(Into::into)
            }
            Self::Project(record) => ControlKeyspace::project_key(record.id()).map_err(Into::into),
            Self::Environment { record, .. } => {
                ControlKeyspace::environment_key(record.project_id(), record.id())
                    .map_err(Into::into)
            }
        }
    }

    fn encode(&self) -> Result<Vec<u8>, DeletionError> {
        match self {
            Self::Organization(record) => serde_json::to_vec(record).map_err(Into::into),
            Self::Project(record) => serde_json::to_vec(record).map_err(Into::into),
            Self::Environment { record, .. } => serde_json::to_vec(record).map_err(Into::into),
        }
    }

    fn request_deletion(&mut self, now: u64, deadline: u64) -> Result<(), DeletionError> {
        match self {
            Self::Organization(record) => record.request_deletion(now, deadline)?,
            Self::Project(record) => record.request_deletion(now, deadline)?,
            Self::Environment { record, .. } => record.request_deletion(now, deadline)?,
        }
        Ok(())
    }

    fn restore(&mut self, now: u64) -> Result<(), DeletionError> {
        match self {
            Self::Organization(record) => record.restore(now)?,
            Self::Project(record) => record.restore(now)?,
            Self::Environment { record, .. } => record.restore(now)?,
        }
        Ok(())
    }

    fn begin_final_deletion(&mut self, now: u64) -> Result<(), DeletionError> {
        match self {
            Self::Organization(record) => record.begin_final_deletion(now)?,
            Self::Project(record) => record.begin_final_deletion(now)?,
            Self::Environment { record, .. } => record.begin_final_deletion(now)?,
        }
        Ok(())
    }

    fn complete_deletion(&mut self, now: u64) -> Result<(), DeletionError> {
        match self {
            Self::Organization(record) => record.complete_deletion(now)?,
            Self::Project(record) => record.complete_deletion(now)?,
            Self::Environment { record, .. } => record.complete_deletion(now)?,
        }
        Ok(())
    }
}

fn workflow_key(scope: &DeletionScope) -> Result<Vec<u8>, DeletionError> {
    ControlKeyspace::deletion_workflow_key(&scope.key()).map_err(Into::into)
}

#[derive(Debug)]
pub enum DeletionError {
    UnsupportedDurability,
    InvalidDeadline,
    GraceNotExpired,
    GraceExpired,
    InvalidState,
    NotFound,
    Forbidden,
    Conflict,
    CorruptRecord,
    Backend(DeletionBackendError),
    Model(ControlModelError),
    Organization(OrganizationStoreError),
    Project(ProjectStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for DeletionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedDurability => "deletion durability is unsupported",
            Self::InvalidDeadline => "deletion deadline is invalid",
            Self::GraceNotExpired => "deletion grace period has not expired",
            Self::GraceExpired => "deletion grace period has expired",
            Self::InvalidState => "deletion workflow state is invalid",
            Self::NotFound => "deletion resource was not found",
            Self::Forbidden => "deletion action is forbidden",
            Self::Conflict => "deletion workflow changed concurrently",
            Self::CorruptRecord => "deletion workflow is corrupt",
            Self::Backend(_) => "deletion backend operation failed",
            Self::Model(_) => "deletion lifecycle transition failed",
            Self::Organization(_) => "deletion organization operation failed",
            Self::Project(_) => "deletion project operation failed",
            Self::Keyspace(_) => "deletion key is invalid",
            Self::Storage(_) => "deletion storage operation failed",
            Self::Json(_) => "deletion record is invalid",
        })
    }
}

impl Error for DeletionError {}

macro_rules! deletion_error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for DeletionError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

deletion_error_from!(DeletionBackendError, Backend);
deletion_error_from!(ControlModelError, Model);
deletion_error_from!(OrganizationStoreError, Organization);
deletion_error_from!(ProjectStoreError, Project);
deletion_error_from!(ControlKeyspaceError, Keyspace);
deletion_error_from!(StorageError, Storage);
deletion_error_from!(serde_json::Error, Json);

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{DeveloperIdentityId, LifecycleState, MembershipRecord, OrganizationRole};

    #[derive(Default)]
    struct Backend(Mutex<Vec<&'static str>>);

    #[async_trait]
    impl DeletionBackend for Backend {
        async fn revoke_data_plane(&self, _: &DeletionScope) -> Result<(), DeletionBackendError> {
            self.0.lock().expect("calls").push("revoke");
            Ok(())
        }

        async fn restore_data_plane(&self, _: &DeletionScope) -> Result<(), DeletionBackendError> {
            self.0.lock().expect("calls").push("restore");
            Ok(())
        }

        async fn destroy_secrets(&self, _: &DeletionScope) -> Result<(), DeletionBackendError> {
            self.0.lock().expect("calls").push("secrets");
            Ok(())
        }

        async fn destroy_data(&self, _: &DeletionScope) -> Result<(), DeletionBackendError> {
            self.0.lock().expect("calls").push("data");
            Ok(())
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<DeletionAuditEvent>>);

    impl DeletionAuditSink for Audit {
        fn record(&self, event: DeletionAuditEvent) {
            self.0.lock().expect("audit").push(event);
        }
    }

    #[test]
    fn deletion_revokes_restores_and_destroys_in_durable_order() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let organizations =
                OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
            let projects =
                ProjectStore::new(adapter.clone(), Durability::Memory).expect("projects");
            let organization_id = OrganizationId::parse("org_example00").expect("organization");
            let developer_id = DeveloperIdentityId::parse("dev_example00").expect("developer");
            organizations
                .create_organization(
                    &OrganizationRecord::new(organization_id.clone(), "Example", 1)
                        .expect("organization"),
                    &MembershipRecord::new(
                        organization_id.clone(),
                        developer_id.clone(),
                        OrganizationRole::Owner,
                        1,
                    ),
                )
                .await
                .expect("organization");
            let project_id = ProjectId::parse("prj_example00").expect("project");
            let mut project =
                ProjectRecord::new(project_id.clone(), organization_id, "Mako", "local", 1)
                    .expect("project");
            project
                .transition(LifecycleState::Active, 2, None)
                .expect("active");
            projects.create_project(&project).await.expect("project");
            let backend = Arc::new(Backend::default());
            let audit = Arc::new(Audit::default());
            let service = DeletionLifecycleService::new(
                adapter,
                Durability::Memory,
                organizations,
                projects.clone(),
                backend.clone(),
                audit.clone(),
            )
            .expect("service");
            let actor = DeveloperPrincipal::for_test(developer_id, "owner@example.test");
            let scope = DeletionScope::Project {
                project_id: project_id.clone(),
            };
            let grace = service
                .request_deletion(&actor, scope.clone(), 10, 3)
                .await
                .expect("request");
            assert_eq!(grace.state(), DeletionWorkflowState::Grace);
            assert_eq!(
                projects
                    .get_project(&project_id)
                    .await
                    .expect("project")
                    .expect("exists")
                    .lifecycle(),
                LifecycleState::DeletionGrace
            );
            let restored = service.restore(&actor, &scope, 4).await.expect("restore");
            assert_eq!(restored.state(), DeletionWorkflowState::Restored);
            assert_eq!(
                projects
                    .get_project(&project_id)
                    .await
                    .expect("project")
                    .expect("exists")
                    .lifecycle(),
                LifecycleState::Active
            );
            service
                .request_deletion(&actor, scope.clone(), 10, 5)
                .await
                .expect("request again");
            assert!(matches!(
                service.finalize(&scope, 9).await,
                Err(DeletionError::GraceNotExpired)
            ));
            let complete = service.finalize(&scope, 10).await.expect("finalize");
            assert_eq!(complete.state(), DeletionWorkflowState::Completed);
            assert!(complete.destruction_complete());
            assert_eq!(
                backend.0.lock().expect("calls").as_slice(),
                ["revoke", "restore", "revoke", "secrets", "data"]
            );
            assert_eq!(
                projects
                    .get_project(&project_id)
                    .await
                    .expect("project")
                    .expect("exists")
                    .lifecycle(),
                LifecycleState::Deleted
            );
            assert_eq!(
                audit.0.lock().expect("audit").last().expect("event").action,
                DeletionAuditAction::Completed
            );
        });
    }
}
