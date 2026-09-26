use std::{error::Error, fmt};

use crate::{
    AutomationPermission, AutomationPrincipal, DeveloperPrincipal, ManagementResourceScope,
    OrganizationRole, OrganizationStore, OrganizationStoreError, ProjectStore, ProjectStoreError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementAction {
    OrganizationRead,
    ProjectRead,
    ProjectWrite,
    EnvironmentRead,
    EnvironmentWrite,
    CollectionWrite,
    PolicyWrite,
    FunctionDeploy,
    AuditRead,
}

impl ManagementAction {
    /// The token permission this action needs.
    #[must_use]
    pub const fn automation_permission(self) -> AutomationPermission {
        match self {
            Self::OrganizationRead => AutomationPermission::OrganizationRead,
            Self::ProjectRead => AutomationPermission::ProjectRead,
            Self::ProjectWrite => AutomationPermission::ProjectWrite,
            Self::EnvironmentRead => AutomationPermission::EnvironmentRead,
            Self::EnvironmentWrite => AutomationPermission::EnvironmentWrite,
            Self::CollectionWrite => AutomationPermission::CollectionWrite,
            Self::PolicyWrite => AutomationPermission::PolicyWrite,
            Self::FunctionDeploy => AutomationPermission::FunctionDeploy,
            Self::AuditRead => AutomationPermission::AuditRead,
        }
    }

    const fn developer_role_allows(self, role: OrganizationRole) -> bool {
        match self {
            Self::OrganizationRead
            | Self::ProjectRead
            | Self::EnvironmentRead
            | Self::AuditRead => true,
            Self::ProjectWrite
            | Self::EnvironmentWrite
            | Self::CollectionWrite
            | Self::PolicyWrite
            | Self::FunctionDeploy => role.can_mutate_projects(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ManagementActor<'a> {
    Developer(&'a DeveloperPrincipal),
    Automation(&'a AutomationPrincipal),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorizedManagementActor {
    Developer(String),
    Automation(String),
}

#[derive(Clone, Debug)]
pub struct ManagementAuthorizer {
    organizations: OrganizationStore,
    projects: ProjectStore,
}

impl ManagementAuthorizer {
    #[must_use]
    pub const fn new(organizations: OrganizationStore, projects: ProjectStore) -> Self {
        Self {
            organizations,
            projects,
        }
    }

    pub async fn authorize(
        &self,
        actor: ManagementActor<'_>,
        action: ManagementAction,
        resource: &ManagementResourceScope,
    ) -> Result<AuthorizedManagementActor, ManagementAccessError> {
        validate_shape(resource)?;
        let authorized = match actor {
            ManagementActor::Developer(principal) => {
                let membership = self
                    .organizations
                    .get_membership(&resource.organization_id, principal.identity_id())
                    .await?
                    .ok_or(ManagementAccessError::Forbidden)?;
                if !action.developer_role_allows(membership.role()) {
                    return Err(ManagementAccessError::Forbidden);
                }
                AuthorizedManagementActor::Developer(principal.identity_id().as_str().to_owned())
            }
            ManagementActor::Automation(principal) => {
                if !principal.allows(action.automation_permission(), resource) {
                    return Err(ManagementAccessError::Forbidden);
                }
                AuthorizedManagementActor::Automation(principal.token_id().as_str().to_owned())
            }
        };
        self.validate_resource(resource).await?;
        Ok(authorized)
    }

    async fn validate_resource(
        &self,
        resource: &ManagementResourceScope,
    ) -> Result<(), ManagementAccessError> {
        self.organizations
            .get_organization(&resource.organization_id)
            .await?
            .ok_or(ManagementAccessError::NotFound)?;
        if let Some(project_id) = &resource.project_id {
            let project = self
                .projects
                .get_project(project_id)
                .await?
                .ok_or(ManagementAccessError::NotFound)?;
            if project.organization_id() != &resource.organization_id {
                return Err(ManagementAccessError::ScopeMismatch);
            }
            if let Some(environment_id) = &resource.environment_id {
                self.projects
                    .get_environment(project_id, environment_id)
                    .await?
                    .ok_or(ManagementAccessError::NotFound)?;
            }
        }
        Ok(())
    }
}

fn validate_shape(resource: &ManagementResourceScope) -> Result<(), ManagementAccessError> {
    if resource.environment_id.is_some() && resource.project_id.is_none() {
        return Err(ManagementAccessError::InvalidScope);
    }
    Ok(())
}

#[derive(Debug)]
pub enum ManagementAccessError {
    InvalidScope,
    ScopeMismatch,
    NotFound,
    Forbidden,
    Organization(OrganizationStoreError),
    Project(ProjectStoreError),
}

impl ManagementAccessError {
    #[must_use]
    pub const fn stable_code(&self) -> &'static str {
        match self {
            Self::InvalidScope | Self::ScopeMismatch => "invalid_request",
            Self::NotFound => "not_found",
            Self::Forbidden => "permission_denied",
            Self::Organization(_) | Self::Project(_) => "unavailable",
        }
    }
}

impl fmt::Display for ManagementAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidScope => "management resource scope is invalid",
            Self::ScopeMismatch => "management resource scope does not match",
            Self::NotFound => "management resource was not found",
            Self::Forbidden => "management action is forbidden",
            Self::Organization(_) => "management organization lookup failed",
            Self::Project(_) => "management project lookup failed",
        })
    }
}

impl Error for ManagementAccessError {}

impl From<OrganizationStoreError> for ManagementAccessError {
    fn from(error: OrganizationStoreError) -> Self {
        Self::Organization(error)
    }
}

impl From<ProjectStoreError> for ManagementAccessError {
    fn from(error: ProjectStoreError) -> Self {
        Self::Project(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::{Durability, KvAdapter, MemoryAdapter};

    use super::*;
    use crate::{
        AutomationScope, AutomationTokenId, DeveloperIdentityId, EnvironmentRecord,
        MembershipRecord, OrganizationId, OrganizationRecord, ProjectRecord,
    };

    #[test]
    fn console_and_automation_paths_share_validation_and_rbac_outcomes() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let organizations =
                OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
            let projects = ProjectStore::new(adapter, Durability::Memory).expect("projects");
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
            let environment_id = EnvironmentId::parse("env_example00").expect("environment");
            projects
                .create_project(
                    &ProjectRecord::new(
                        project_id.clone(),
                        organization_id.clone(),
                        "Mako",
                        "local",
                        1,
                    )
                    .expect("project"),
                )
                .await
                .expect("project");
            projects
                .create_environment(
                    &EnvironmentRecord::new(environment_id.clone(), project_id.clone(), "Dev", 1)
                        .expect("environment"),
                )
                .await
                .expect("environment");
            let authorizer = ManagementAuthorizer::new(organizations, projects);
            let developer = DeveloperPrincipal::for_test(developer_id, "owner@example.test");
            let every_permission = [
                AutomationPermission::OrganizationRead,
                AutomationPermission::ProjectRead,
                AutomationPermission::ProjectWrite,
                AutomationPermission::EnvironmentRead,
                AutomationPermission::EnvironmentWrite,
                AutomationPermission::CollectionWrite,
                AutomationPermission::PolicyWrite,
                AutomationPermission::FunctionDeploy,
                AutomationPermission::AuditRead,
            ];
            let automation = AutomationPrincipal::for_test(
                AutomationTokenId::parse("atm_example00").expect("token"),
                AutomationScope::new(
                    organization_id.clone(),
                    Some(project_id.clone()),
                    Some(environment_id.clone()),
                    every_permission,
                )
                .expect("scope"),
            );
            let resource = ManagementResourceScope {
                organization_id: organization_id.clone(),
                project_id: Some(project_id),
                environment_id: Some(environment_id),
            };
            for action in [
                ManagementAction::ProjectRead,
                ManagementAction::ProjectWrite,
                ManagementAction::EnvironmentRead,
                ManagementAction::EnvironmentWrite,
                ManagementAction::CollectionWrite,
                ManagementAction::PolicyWrite,
                ManagementAction::FunctionDeploy,
                ManagementAction::AuditRead,
            ] {
                assert!(
                    authorizer
                        .authorize(ManagementActor::Developer(&developer), action, &resource)
                        .await
                        .is_ok()
                );
                assert!(
                    authorizer
                        .authorize(ManagementActor::Automation(&automation), action, &resource)
                        .await
                        .is_ok()
                );
            }

            let invalid = ManagementResourceScope {
                organization_id,
                project_id: None,
                environment_id: resource.environment_id.clone(),
            };
            let developer_error = authorizer
                .authorize(
                    ManagementActor::Developer(&developer),
                    ManagementAction::ProjectRead,
                    &invalid,
                )
                .await
                .expect_err("invalid scope");
            let automation_error = authorizer
                .authorize(
                    ManagementActor::Automation(&automation),
                    ManagementAction::ProjectRead,
                    &invalid,
                )
                .await
                .expect_err("invalid scope");
            assert_eq!(
                developer_error.stable_code(),
                automation_error.stable_code()
            );
            assert_eq!(developer_error.stable_code(), "invalid_request");
        });
    }
}
