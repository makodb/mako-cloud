use std::{error::Error, fmt, num::NonZeroUsize};

use mako_api::{EnvironmentId, ProjectId};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, ControlModelError, DeveloperPrincipal, EnvironmentRecord, OrganizationId,
    OrganizationStore, OrganizationStoreError, ProjectRecord,
};

#[derive(Clone)]
pub struct ProjectStore {
    adapter: std::sync::Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for ProjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl ProjectStore {
    pub fn new(
        adapter: std::sync::Arc<dyn KvAdapter>,
        durability: Durability,
    ) -> Result<Self, ProjectStoreError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(ProjectStoreError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
        })
    }

    pub async fn create_project(&self, project: &ProjectRecord) -> Result<(), ProjectStoreError> {
        let project_key = ControlKeyspace::project_key(project.id())?;
        let organization_key =
            ControlKeyspace::organization_project_key(project.organization_id(), project.id())?;
        let value = serde_json::to_vec(project)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&project_key, value.clone());
        batch.put(&organization_key, value);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::Missing { key: project_key },
                KeyCondition::Missing {
                    key: organization_key,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn get_project(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<ProjectRecord>, ProjectStoreError> {
        self.read(&ControlKeyspace::project_key(project_id)?).await
    }

    pub async fn list_projects(
        &self,
        organization_id: &OrganizationId,
        limit: NonZeroUsize,
    ) -> Result<Vec<ProjectRecord>, ProjectStoreError> {
        let values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::organization_projects_range(organization_id)?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let records = values
            .into_iter()
            .map(|value| serde_json::from_slice::<ProjectRecord>(&value.value))
            .collect::<Result<Vec<_>, _>>()?;
        if records
            .iter()
            .any(|project| project.organization_id() != organization_id)
        {
            return Err(ProjectStoreError::CorruptRecord);
        }
        Ok(records)
    }

    pub async fn replace_project(
        &self,
        previous: &ProjectRecord,
        next: &ProjectRecord,
    ) -> Result<(), ProjectStoreError> {
        if previous.id() != next.id() || previous.organization_id() != next.organization_id() {
            return Err(ProjectStoreError::RecordScopeMismatch);
        }
        let project_key = ControlKeyspace::project_key(previous.id())?;
        let organization_key =
            ControlKeyspace::organization_project_key(previous.organization_id(), previous.id())?;
        let previous_value = serde_json::to_vec(previous)?;
        let next_value = serde_json::to_vec(next)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&project_key, next_value.clone());
        batch.put(&organization_key, next_value);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: project_key,
                    value: previous_value.clone(),
                },
                KeyCondition::ValueEquals {
                    key: organization_key,
                    value: previous_value,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn create_environment(
        &self,
        environment: &EnvironmentRecord,
    ) -> Result<(), ProjectStoreError> {
        self.create(
            ControlKeyspace::environment_key(environment.project_id(), environment.id())?,
            serde_json::to_vec(environment)?,
        )
        .await
    }

    pub async fn get_environment(
        &self,
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
    ) -> Result<Option<EnvironmentRecord>, ProjectStoreError> {
        self.read(&ControlKeyspace::environment_key(
            project_id,
            environment_id,
        )?)
        .await
    }

    pub async fn list_environments(
        &self,
        project_id: &ProjectId,
        limit: NonZeroUsize,
    ) -> Result<Vec<EnvironmentRecord>, ProjectStoreError> {
        let values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::environments_range(project_id)?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let records = values
            .into_iter()
            .map(|value| serde_json::from_slice::<EnvironmentRecord>(&value.value))
            .collect::<Result<Vec<_>, _>>()?;
        if records
            .iter()
            .any(|environment| environment.project_id() != project_id)
        {
            return Err(ProjectStoreError::CorruptRecord);
        }
        Ok(records)
    }

    pub async fn replace_environment(
        &self,
        previous: &EnvironmentRecord,
        next: &EnvironmentRecord,
    ) -> Result<(), ProjectStoreError> {
        if previous.id() != next.id() || previous.project_id() != next.project_id() {
            return Err(ProjectStoreError::RecordScopeMismatch);
        }
        self.replace(
            ControlKeyspace::environment_key(previous.project_id(), previous.id())?,
            previous,
            next,
        )
        .await
    }

    async fn read<T: DeserializeOwned>(&self, key: &[u8]) -> Result<Option<T>, ProjectStoreError> {
        self.adapter
            .get(key)
            .await?
            .map(|value| serde_json::from_slice(&value).map_err(ProjectStoreError::from))
            .transpose()
    }

    async fn create(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), ProjectStoreError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, value);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn replace<T: Serialize>(
        &self,
        key: Vec<u8>,
        previous: &T,
        next: &T,
    ) -> Result<(), ProjectStoreError> {
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

    async fn apply(&self, write: AtomicWrite) -> Result<(), ProjectStoreError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(ProjectStoreError::Conflict),
        }
    }
}

#[derive(Clone)]
pub struct ProjectEnvironmentService {
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: std::sync::Arc<dyn ControlAuditSink>,
}

impl fmt::Debug for ProjectEnvironmentService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectEnvironmentService")
            .field("projects", &self.projects)
            .field("organizations", &self.organizations)
            .finish_non_exhaustive()
    }
}

impl ProjectEnvironmentService {
    #[must_use]
    pub fn new(
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: std::sync::Arc<dyn ControlAuditSink>,
    ) -> Self {
        Self {
            projects,
            organizations,
            audit,
        }
    }

    pub async fn create_project(
        &self,
        actor: &DeveloperPrincipal,
        input: NewProject,
    ) -> Result<ProjectRecord, ProjectStoreError> {
        self.authorize_mutation(
            actor,
            &input.organization_id,
            ControlAuditAction::ProjectCreate,
            input.now_unix_seconds,
        )
        .await?;
        let project = ProjectRecord::new(
            input.id,
            input.organization_id.clone(),
            input.name,
            input.region,
            input.now_unix_seconds,
        )?;
        self.projects.create_project(&project).await?;
        self.audit(
            actor,
            &input.organization_id,
            ControlAuditAction::ProjectCreate,
            project.id().as_str(),
            ControlAuditOutcome::Allowed,
            input.now_unix_seconds,
        );
        Ok(project)
    }

    pub async fn list_projects(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<ProjectRecord>, ProjectStoreError> {
        self.require_member(
            actor,
            organization_id,
            ControlAuditAction::ProjectRead,
            now_unix_seconds,
        )
        .await?;
        let projects = self.projects.list_projects(organization_id, limit).await?;
        self.audit(
            actor,
            organization_id,
            ControlAuditAction::ProjectRead,
            "projects",
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(projects)
    }

    pub async fn get_project(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        now_unix_seconds: u64,
    ) -> Result<ProjectRecord, ProjectStoreError> {
        let project = self.project(project_id).await?;
        self.require_member(
            actor,
            project.organization_id(),
            ControlAuditAction::ProjectRead,
            now_unix_seconds,
        )
        .await?;
        self.audit(
            actor,
            project.organization_id(),
            ControlAuditAction::ProjectRead,
            project.id().as_str(),
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(project)
    }

    pub async fn suspend_project(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        now_unix_seconds: u64,
    ) -> Result<ProjectRecord, ProjectStoreError> {
        self.mutate_project(
            actor,
            project_id,
            ControlAuditAction::ProjectSuspend,
            now_unix_seconds,
            |project| project.suspend(now_unix_seconds),
        )
        .await
    }

    pub async fn restore_project(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        now_unix_seconds: u64,
    ) -> Result<ProjectRecord, ProjectStoreError> {
        self.mutate_project(
            actor,
            project_id,
            ControlAuditAction::ProjectRestore,
            now_unix_seconds,
            |project| project.restore(now_unix_seconds),
        )
        .await
    }

    pub async fn request_project_deletion(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        now_unix_seconds: u64,
        deadline_unix_seconds: u64,
    ) -> Result<ProjectRecord, ProjectStoreError> {
        self.mutate_project(
            actor,
            project_id,
            ControlAuditAction::ProjectDeleteRequest,
            now_unix_seconds,
            |project| project.request_deletion(now_unix_seconds, deadline_unix_seconds),
        )
        .await
    }

    pub async fn create_environment(
        &self,
        actor: &DeveloperPrincipal,
        input: NewEnvironment,
    ) -> Result<EnvironmentRecord, ProjectStoreError> {
        let project = self.project(&input.project_id).await?;
        self.authorize_mutation(
            actor,
            project.organization_id(),
            ControlAuditAction::EnvironmentCreate,
            input.now_unix_seconds,
        )
        .await?;
        let environment = EnvironmentRecord::new(
            input.id,
            input.project_id,
            input.name,
            input.now_unix_seconds,
        )?;
        self.projects.create_environment(&environment).await?;
        self.audit(
            actor,
            project.organization_id(),
            ControlAuditAction::EnvironmentCreate,
            environment.id().as_str(),
            ControlAuditOutcome::Allowed,
            input.now_unix_seconds,
        );
        Ok(environment)
    }

    pub async fn get_environment(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        now_unix_seconds: u64,
    ) -> Result<EnvironmentRecord, ProjectStoreError> {
        let project = self.project(project_id).await?;
        self.require_member(
            actor,
            project.organization_id(),
            ControlAuditAction::EnvironmentRead,
            now_unix_seconds,
        )
        .await?;
        let environment = self.environment(project_id, environment_id).await?;
        self.audit(
            actor,
            project.organization_id(),
            ControlAuditAction::EnvironmentRead,
            environment.id().as_str(),
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(environment)
    }

    pub async fn list_environments(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<EnvironmentRecord>, ProjectStoreError> {
        let project = self.project(project_id).await?;
        self.require_member(
            actor,
            project.organization_id(),
            ControlAuditAction::EnvironmentRead,
            now_unix_seconds,
        )
        .await?;
        self.projects.list_environments(project_id, limit).await
    }

    pub async fn suspend_environment(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        now_unix_seconds: u64,
    ) -> Result<EnvironmentRecord, ProjectStoreError> {
        self.mutate_environment(
            actor,
            project_id,
            environment_id,
            ControlAuditAction::EnvironmentSuspend,
            now_unix_seconds,
            |environment| environment.suspend(now_unix_seconds),
        )
        .await
    }

    pub async fn restore_environment(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        now_unix_seconds: u64,
    ) -> Result<EnvironmentRecord, ProjectStoreError> {
        self.mutate_environment(
            actor,
            project_id,
            environment_id,
            ControlAuditAction::EnvironmentRestore,
            now_unix_seconds,
            |environment| environment.restore(now_unix_seconds),
        )
        .await
    }

    pub async fn request_environment_deletion(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        now_unix_seconds: u64,
        deadline_unix_seconds: u64,
    ) -> Result<EnvironmentRecord, ProjectStoreError> {
        self.mutate_environment(
            actor,
            project_id,
            environment_id,
            ControlAuditAction::EnvironmentDeleteRequest,
            now_unix_seconds,
            |environment| environment.request_deletion(now_unix_seconds, deadline_unix_seconds),
        )
        .await
    }

    async fn mutate_project<F>(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        action: ControlAuditAction,
        now_unix_seconds: u64,
        mutate: F,
    ) -> Result<ProjectRecord, ProjectStoreError>
    where
        F: FnOnce(&mut ProjectRecord) -> Result<(), ControlModelError>,
    {
        let previous = self.project(project_id).await?;
        self.authorize_mutation(actor, previous.organization_id(), action, now_unix_seconds)
            .await?;
        let mut next = previous.clone();
        mutate(&mut next)?;
        self.projects.replace_project(&previous, &next).await?;
        self.audit(
            actor,
            previous.organization_id(),
            action,
            project_id.as_str(),
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(next)
    }

    async fn mutate_environment<F>(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        action: ControlAuditAction,
        now_unix_seconds: u64,
        mutate: F,
    ) -> Result<EnvironmentRecord, ProjectStoreError>
    where
        F: FnOnce(&mut EnvironmentRecord) -> Result<(), ControlModelError>,
    {
        let project = self.project(project_id).await?;
        self.authorize_mutation(actor, project.organization_id(), action, now_unix_seconds)
            .await?;
        let previous = self.environment(project_id, environment_id).await?;
        let mut next = previous.clone();
        mutate(&mut next)?;
        self.projects.replace_environment(&previous, &next).await?;
        self.audit(
            actor,
            project.organization_id(),
            action,
            environment_id.as_str(),
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(next)
    }

    async fn authorize_mutation(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        action: ControlAuditAction,
        now_unix_seconds: u64,
    ) -> Result<(), ProjectStoreError> {
        let membership = self
            .require_member(actor, organization_id, action, now_unix_seconds)
            .await?;
        if !membership.role().can_mutate_projects() {
            self.audit(
                actor,
                organization_id,
                action,
                "authorization",
                ControlAuditOutcome::Denied,
                now_unix_seconds,
            );
            return Err(ProjectStoreError::Forbidden);
        }
        Ok(())
    }

    async fn require_member(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        action: ControlAuditAction,
        now_unix_seconds: u64,
    ) -> Result<crate::MembershipRecord, ProjectStoreError> {
        let membership = self
            .organizations
            .get_membership(organization_id, actor.identity_id())
            .await?;
        membership.ok_or_else(|| {
            self.audit(
                actor,
                organization_id,
                action,
                "authorization",
                ControlAuditOutcome::Denied,
                now_unix_seconds,
            );
            ProjectStoreError::Forbidden
        })
    }

    async fn project(&self, project_id: &ProjectId) -> Result<ProjectRecord, ProjectStoreError> {
        self.projects
            .get_project(project_id)
            .await?
            .ok_or(ProjectStoreError::NotFound)
    }

    async fn environment(
        &self,
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
    ) -> Result<EnvironmentRecord, ProjectStoreError> {
        self.projects
            .get_environment(project_id, environment_id)
            .await?
            .ok_or(ProjectStoreError::NotFound)
    }

    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        action: ControlAuditAction,
        target: impl Into<String>,
        outcome: ControlAuditOutcome,
        at_unix_seconds: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization_id.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: target.into(),
            outcome,
            at_unix_seconds,
        });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewProject {
    pub id: ProjectId,
    pub organization_id: OrganizationId,
    pub name: String,
    pub region: String,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewEnvironment {
    pub id: EnvironmentId,
    pub project_id: ProjectId,
    pub name: String,
    pub now_unix_seconds: u64,
}

#[derive(Debug)]
pub enum ProjectStoreError {
    UnsupportedDurability,
    NotFound,
    Forbidden,
    Conflict,
    CorruptRecord,
    RecordScopeMismatch,
    Model(ControlModelError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Serialization(serde_json::Error),
}

impl fmt::Display for ProjectStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedDurability => "project durability is unsupported",
            Self::NotFound => "project resource was not found",
            Self::Forbidden => "project action is forbidden",
            Self::Conflict => "project resource changed concurrently",
            Self::CorruptRecord => "project resource is corrupt",
            Self::RecordScopeMismatch => "project resource scope does not match",
            Self::Model(_) => "project lifecycle is invalid",
            Self::Organization(_) => "organization lookup failed",
            Self::Keyspace(_) => "project key encoding failed",
            Self::Storage(_) => "project storage operation failed",
            Self::Serialization(_) => "project serialization failed",
        })
    }
}

impl Error for ProjectStoreError {}

impl From<ControlModelError> for ProjectStoreError {
    fn from(error: ControlModelError) -> Self {
        Self::Model(error)
    }
}
impl From<OrganizationStoreError> for ProjectStoreError {
    fn from(error: OrganizationStoreError) -> Self {
        Self::Organization(error)
    }
}
impl From<ControlKeyspaceError> for ProjectStoreError {
    fn from(error: ControlKeyspaceError) -> Self {
        Self::Keyspace(error)
    }
}
impl From<StorageError> for ProjectStoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}
impl From<serde_json::Error> for ProjectStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{
        DeveloperIdentityId, InvitationId, InvitationInput, InvitationRecord, MembershipRecord,
        OrganizationRecord, OrganizationRole,
    };

    #[derive(Default)]
    struct AuditLog(Mutex<Vec<ControlAuditEvent>>);

    impl ControlAuditSink for AuditLog {
        fn record(&self, event: ControlAuditEvent) {
            self.0.lock().expect("audit lock").push(event);
        }
    }

    #[test]
    fn project_and_environment_endpoints_enforce_roles_and_lifecycle_intents() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let organizations =
                OrganizationStore::new(adapter.clone(), Durability::Memory).expect("org store");
            let projects = ProjectStore::new(adapter, Durability::Memory).expect("project store");
            let audit = Arc::new(AuditLog::default());
            let service = ProjectEnvironmentService::new(
                projects.clone(),
                organizations.clone(),
                audit.clone(),
            );
            let organization_id = OrganizationId::parse("org_example00").expect("organization");
            let owner_id = DeveloperIdentityId::parse("dev_owner000").expect("owner id");
            let viewer_id = DeveloperIdentityId::parse("dev_viewer00").expect("viewer id");
            let owner = DeveloperPrincipal::for_test(owner_id.clone(), "owner@example.test");
            let viewer = DeveloperPrincipal::for_test(viewer_id.clone(), "viewer@example.test");
            let organization =
                OrganizationRecord::new(organization_id.clone(), "Example", 10).expect("org");
            organizations
                .create_organization(
                    &organization,
                    &MembershipRecord::new(
                        organization_id.clone(),
                        owner_id.clone(),
                        OrganizationRole::Owner,
                        10,
                    ),
                )
                .await
                .expect("create org");
            let invitation = InvitationRecord::new(InvitationInput {
                id: InvitationId::parse("inv_viewer000").expect("invitation"),
                organization_id: organization_id.clone(),
                normalized_email: "viewer@example.test".to_owned(),
                role: OrganizationRole::Viewer,
                invited_by: owner_id,
                token_digest: "12345678901234567890123456789012".to_owned(),
                created_at_unix_seconds: 11,
                expires_at_unix_seconds: 100,
            })
            .expect("invitation");
            organizations
                .create_invitation(&invitation)
                .await
                .expect("store invitation");
            let mut accepted = invitation.clone();
            accepted.accept(viewer_id.clone(), 12).expect("accept");
            organizations
                .accept_invitation(
                    &invitation,
                    &accepted,
                    &MembershipRecord::new(
                        organization_id.clone(),
                        viewer_id,
                        OrganizationRole::Viewer,
                        12,
                    ),
                )
                .await
                .expect("viewer membership");

            let project_id = ProjectId::parse("prj_example00").expect("project");
            let project = service
                .create_project(
                    &owner,
                    NewProject {
                        id: project_id.clone(),
                        organization_id: organization_id.clone(),
                        name: "Project".to_owned(),
                        region: "us-east-1".to_owned(),
                        now_unix_seconds: 20,
                    },
                )
                .await
                .expect("create project");
            assert!(matches!(
                service
                    .create_project(
                        &viewer,
                        NewProject {
                            id: ProjectId::parse("prj_denied000").expect("project"),
                            organization_id: organization_id.clone(),
                            name: "Denied".to_owned(),
                            region: "us-east-1".to_owned(),
                            now_unix_seconds: 20,
                        },
                    )
                    .await,
                Err(ProjectStoreError::Forbidden)
            ));

            let mut active_project = project.clone();
            active_project
                .transition(crate::LifecycleState::Active, 21, None)
                .expect("activate project");
            projects
                .replace_project(&project, &active_project)
                .await
                .expect("store active project");
            let suspended = service
                .suspend_project(&owner, &project_id, 22)
                .await
                .expect("suspend project");
            assert_eq!(suspended.lifecycle(), crate::LifecycleState::Suspended);
            let restored = service
                .restore_project(&owner, &project_id, 23)
                .await
                .expect("restore project");
            assert_eq!(restored.lifecycle(), crate::LifecycleState::Active);

            let environment_id = EnvironmentId::parse("env_example00").expect("environment");
            let environment = service
                .create_environment(
                    &owner,
                    NewEnvironment {
                        id: environment_id.clone(),
                        project_id: project_id.clone(),
                        name: "Production".to_owned(),
                        now_unix_seconds: 24,
                    },
                )
                .await
                .expect("create environment");
            let mut active_environment = environment.clone();
            active_environment
                .transition(crate::LifecycleState::Active, 25, None)
                .expect("activate environment");
            projects
                .replace_environment(&environment, &active_environment)
                .await
                .expect("store active environment");
            assert_eq!(
                service
                    .suspend_environment(&owner, &project_id, &environment_id, 26)
                    .await
                    .expect("suspend environment")
                    .lifecycle(),
                crate::LifecycleState::Suspended
            );
            service
                .restore_environment(&owner, &project_id, &environment_id, 27)
                .await
                .expect("restore environment");
            let deletion = service
                .request_environment_deletion(&owner, &project_id, &environment_id, 28, 100)
                .await
                .expect("environment deletion intent");
            assert_eq!(deletion.lifecycle(), crate::LifecycleState::DeletionGrace);
            assert_eq!(
                service
                    .list_environments(
                        &viewer,
                        &project_id,
                        NonZeroUsize::new(10).expect("limit"),
                        29,
                    )
                    .await
                    .expect("viewer can inspect")
                    .len(),
                1
            );
            assert!(audit.0.lock().expect("audit lock").iter().any(|event| {
                event.actor_id == *viewer.identity_id()
                    && event.action == ControlAuditAction::ProjectCreate
                    && event.outcome == ControlAuditOutcome::Denied
            }));
        });
    }
}
