use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use mako_api::TenantScope;
use mako_identity::{
    AdminRequestContext, AdminUserPermission, AppUserId, TrustedMetadataInvalidationError,
    TrustedMetadataInvalidationSink,
};
use mako_policy::{AuthorizationEpochStore, SubjectId};
use mako_storage::{Durability, KvAdapter};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink,
    DeveloperPrincipal, OrganizationRole, OrganizationStore, OrganizationStoreError, ProjectStore,
    ProjectStoreError,
};

#[derive(Clone)]
pub struct ApplicationUserAccess {
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
}

impl fmt::Debug for ApplicationUserAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplicationUserAccess")
            .field("projects", &self.projects)
            .field("organizations", &self.organizations)
            .finish_non_exhaustive()
    }
}

impl ApplicationUserAccess {
    #[must_use]
    pub fn new(
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
    ) -> Self {
        Self {
            projects,
            organizations,
            audit,
        }
    }

    pub async fn request_context(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        request_id: &str,
        now_unix_seconds: u64,
    ) -> Result<AdminRequestContext, ApplicationUserAccessError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(ApplicationUserAccessError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(ApplicationUserAccessError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?;
        let Some(membership) = membership else {
            self.audit.record(ControlAuditEvent {
                organization_id: project.organization_id().clone(),
                actor_id: actor.identity_id().clone(),
                action: ControlAuditAction::ApplicationUserAccess,
                target: format!(
                    "{}/{}/users",
                    tenant.project_id().as_str(),
                    tenant.environment_id().as_str()
                ),
                outcome: ControlAuditOutcome::Denied,
                at_unix_seconds: now_unix_seconds,
            });
            return Err(ApplicationUserAccessError::Forbidden);
        };
        let permissions = permissions_for_role(membership.role());
        AdminRequestContext::new(
            tenant.clone(),
            actor.identity_id().as_str(),
            request_id,
            permissions,
            now_unix_seconds,
        )
        .map_err(ApplicationUserAccessError::Identity)
    }
}

fn permissions_for_role(role: OrganizationRole) -> Vec<AdminUserPermission> {
    match role {
        OrganizationRole::Owner | OrganizationRole::Administrator => vec![
            AdminUserPermission::Read,
            AdminUserPermission::Create,
            AdminUserPermission::UpdateMetadata,
            AdminUserPermission::ManageLifecycle,
            AdminUserPermission::RevokeSessions,
            AdminUserPermission::Delete,
        ],
        OrganizationRole::Developer => vec![AdminUserPermission::Read],
        OrganizationRole::Viewer => Vec::new(),
    }
}

#[derive(Clone)]
pub struct PolicyMetadataInvalidation {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for PolicyMetadataInvalidation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PolicyMetadataInvalidation")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl PolicyMetadataInvalidation {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
    ) -> Result<Self, ApplicationUserAccessError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(ApplicationUserAccessError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
        })
    }
}

#[async_trait]
impl TrustedMetadataInvalidationSink for PolicyMetadataInvalidation {
    async fn trusted_metadata_changed(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
    ) -> Result<(), TrustedMetadataInvalidationError> {
        let subject = SubjectId::parse(user_id.as_str()).map_err(|_| {
            TrustedMetadataInvalidationError::new("application-user subject is invalid")
        })?;
        let epochs =
            AuthorizationEpochStore::new(self.adapter.clone(), tenant, tenant, self.durability)
                .map_err(|_| {
                    TrustedMetadataInvalidationError::new(
                        "authorization epoch store is unavailable",
                    )
                })?;
        epochs.trusted_claims_changed(&subject).await.map_err(|_| {
            TrustedMetadataInvalidationError::new("authorization epoch could not be advanced")
        })?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum ApplicationUserAccessError {
    UnsupportedDurability,
    NotFound,
    Forbidden,
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Identity(mako_identity::AdminUserApiError),
}

impl fmt::Display for ApplicationUserAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedDurability => "application-user durability is unsupported",
            Self::NotFound => "application-user tenant was not found",
            Self::Forbidden => "application-user administration is forbidden",
            Self::Project(_) => "application-user project lookup failed",
            Self::Organization(_) => "application-user organization lookup failed",
            Self::Identity(_) => "application-user request context is invalid",
        })
    }
}

impl Error for ApplicationUserAccessError {}

impl From<ProjectStoreError> for ApplicationUserAccessError {
    fn from(error: ProjectStoreError) -> Self {
        Self::Project(error)
    }
}

impl From<OrganizationStoreError> for ApplicationUserAccessError {
    fn from(error: OrganizationStoreError) -> Self {
        Self::Organization(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mako_api::{EnvironmentId, ProjectId};
    use mako_identity::{
        AdminAuditOutcome, AdminCreateUserRequest, AdminUserAction, AdminUserApiError,
        AdminUserAuditError, AdminUserAuditEvent, AdminUserAuditSink, AdminUserService,
        ApplicationUserInvitation, ApplicationUserInvitationSink, IdentityStore, NormalizedEmail,
        TrustedAppMetadata, UserProfileMetadata,
    };
    use mako_storage::MemoryAdapter;
    use serde_json::json;

    use super::*;
    use crate::{
        DeveloperIdentityId, EnvironmentRecord, MembershipRecord, OrganizationId,
        OrganizationRecord, ProjectRecord,
    };

    #[derive(Default)]
    struct ControlAudit;

    impl ControlAuditSink for ControlAudit {
        fn record(&self, _: ControlAuditEvent) {}
    }

    #[derive(Default)]
    struct IdentityAudit(Mutex<Vec<AdminUserAuditEvent>>);

    impl AdminUserAuditSink for IdentityAudit {
        fn record(&self, event: &AdminUserAuditEvent) -> Result<(), AdminUserAuditError> {
            self.0.lock().expect("audit").push(event.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct Invitations;

    impl ApplicationUserInvitationSink for Invitations {
        fn enqueue(&self, _: ApplicationUserInvitation) {}
    }

    async fn add_member(
        organizations: &OrganizationStore,
        organization_id: &OrganizationId,
        developer_id: DeveloperIdentityId,
        role: OrganizationRole,
    ) {
        let owner = organizations
            .list_memberships(
                organization_id,
                std::num::NonZeroUsize::new(10).expect("limit"),
            )
            .await
            .expect("members")
            .into_iter()
            .next()
            .expect("owner");
        let invitation = crate::InvitationRecord::new(crate::InvitationInput {
            id: crate::InvitationId::parse(format!("inv_{}", developer_id.as_str()))
                .expect("invitation"),
            organization_id: organization_id.clone(),
            normalized_email: format!("{}@example.test", developer_id.as_str()),
            role,
            invited_by: owner.developer_identity_id().clone(),
            token_digest: "12345678901234567890123456789012".to_owned(),
            created_at_unix_seconds: 2,
            expires_at_unix_seconds: 100,
        })
        .expect("invitation");
        organizations
            .create_invitation(&invitation)
            .await
            .expect("invitation");
        let mut accepted = invitation.clone();
        accepted.accept(developer_id.clone(), 3).expect("accept");
        organizations
            .accept_invitation(
                &invitation,
                &accepted,
                &MembershipRecord::new(organization_id.clone(), developer_id, role, 3),
            )
            .await
            .expect("membership");
    }

    #[test]
    fn organization_roles_become_identity_permissions_and_core_service_audits_denials() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let organizations =
                OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
            let projects =
                ProjectStore::new(adapter.clone(), Durability::Memory).expect("projects");
            let organization_id = OrganizationId::parse("org_example00").expect("organization");
            let owner_id = DeveloperIdentityId::parse("dev_owner000").expect("owner");
            let developer_id = DeveloperIdentityId::parse("dev_develop00").expect("developer");
            organizations
                .create_organization(
                    &OrganizationRecord::new(organization_id.clone(), "Example", 1)
                        .expect("organization"),
                    &MembershipRecord::new(
                        organization_id.clone(),
                        owner_id.clone(),
                        OrganizationRole::Owner,
                        1,
                    ),
                )
                .await
                .expect("organization");
            add_member(
                &organizations,
                &organization_id,
                developer_id.clone(),
                OrganizationRole::Developer,
            )
            .await;
            let project_id = ProjectId::parse("prj_example00").expect("project");
            let environment_id = EnvironmentId::parse("env_example00").expect("environment");
            projects
                .create_project(
                    &ProjectRecord::new(project_id.clone(), organization_id, "Mako", "local", 1)
                        .expect("project"),
                )
                .await
                .expect("project");
            projects
                .create_environment(
                    &EnvironmentRecord::new(
                        environment_id.clone(),
                        project_id.clone(),
                        "Development",
                        1,
                    )
                    .expect("environment"),
                )
                .await
                .expect("environment");
            let tenant = TenantScope::new(project_id, environment_id);
            let access =
                ApplicationUserAccess::new(projects, organizations, Arc::new(ControlAudit));
            let owner = DeveloperPrincipal::for_test(owner_id, "owner@example.test");
            let developer = DeveloperPrincipal::for_test(developer_id, "developer@example.test");
            let owner_context = access
                .request_context(&owner, &tenant, "req_owner000", 4)
                .await
                .expect("owner context");
            let developer_context = access
                .request_context(&developer, &tenant, "req_develop0", 4)
                .await
                .expect("developer context");
            let store = IdentityStore::new(adapter.clone(), &tenant, &tenant, Durability::Memory)
                .expect("store");
            let audit = IdentityAudit::default();
            let invitations = Invitations;
            let invalidations = PolicyMetadataInvalidation::new(adapter, Durability::Memory)
                .expect("invalidations");
            let service = AdminUserService::new(&store, &audit, &invitations, &invalidations);
            let created = service
                .create(
                    &owner_context,
                    AdminCreateUserRequest {
                        email: NormalizedEmail::parse("user@example.test").expect("email"),
                        trusted_metadata: TrustedAppMetadata::new(json!({})).expect("trusted"),
                        profile_metadata: UserProfileMetadata::new(json!({})).expect("profile"),
                    },
                )
                .await
                .expect("create");
            assert_eq!(
                service
                    .search(&developer_context, None, 10)
                    .await
                    .expect("search")
                    .users
                    .len(),
                1
            );
            assert!(matches!(
                service
                    .delete(&developer_context, created.user().id())
                    .await,
                Err(AdminUserApiError::PermissionDenied)
            ));
            assert!(
                audit
                    .0
                    .lock()
                    .expect("audit")
                    .iter()
                    .any(|event| event.action() == AdminUserAction::Delete
                        && event.outcome() == AdminAuditOutcome::Denied)
            );
            assert!(
                !serde_json::to_string(&created)
                    .expect("json")
                    .contains("password")
            );
        });
    }
}
