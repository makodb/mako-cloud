use std::{future::Future, num::NonZeroUsize, sync::Arc, time::SystemTime};

use futures::executor::block_on;
use mako_api::{EnvironmentId, ErrorCode, ProjectId, RetryAdvice, TenantScope};
use mako_control_plane::{
    DeveloperAuthenticationError, DeveloperIdentityId, DeveloperPrincipal, EnvironmentRecord,
    InvitationId, InvitationRecord, InvitationStatus, InvitationToken, LifecycleState,
    MembershipRecord, NewEnvironment, NewInvitation, NewProject, OrganizationId,
    OrganizationRecord, OrganizationRole, OrganizationServiceError, ProjectRecord,
    ProjectStoreError,
};
use mako_provisioning::{
    ProvisioningOperation, ProvisioningResource, ProvisioningWorkflowError, ProvisioningWorkflowId,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{ControlPlaneGraph, explorer_invalidation};

const AUTHORIZATION_HEADER: &str = "authorization";
const CONFIRMATION_HEADER: &str = "confirmation";
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const JSON_CONTENT_TYPE: &str = "application/json";
const LIST_LIMIT: usize = 1_000;
const DELETION_GRACE_SECONDS: u64 = 7 * 24 * 60 * 60;

pub(crate) fn add_management_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/organizations",
            handle_list_organizations as Handler,
        ),
        (
            HttpMethod::Get,
            "/v1/organizations/{organizationId}/bill",
            handle_organization_bill,
        ),
        (
            HttpMethod::Post,
            "/v1/organizations",
            handle_create_organization,
        ),
        (
            HttpMethod::Get,
            "/v1/organizations/{organizationId}",
            handle_get_organization,
        ),
        (
            HttpMethod::Patch,
            "/v1/organizations/{organizationId}",
            handle_update_organization,
        ),
        (
            HttpMethod::Delete,
            "/v1/organizations/{organizationId}",
            handle_delete_organization,
        ),
        (
            HttpMethod::Post,
            "/v1/organizations/{organizationId}/actions/restore",
            handle_restore_organization,
        ),
        (
            HttpMethod::Post,
            "/v1/organizations/{organizationId}/invitations",
            handle_create_invitation,
        ),
        (
            HttpMethod::Post,
            "/v1/invitations/{invitationId}/accept",
            handle_accept_invitation,
        ),
        (
            HttpMethod::Get,
            "/v1/organizations/{organizationId}/members",
            handle_list_members,
        ),
        (
            HttpMethod::Patch,
            "/v1/organizations/{organizationId}/members/{developerIdentityId}",
            handle_update_member,
        ),
        (
            HttpMethod::Delete,
            "/v1/organizations/{organizationId}/members/{developerIdentityId}",
            handle_delete_member,
        ),
        (HttpMethod::Get, "/v1/projects", handle_list_projects),
        (HttpMethod::Post, "/v1/projects", handle_create_project),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}",
            handle_get_project,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}",
            handle_delete_project,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/actions/suspend",
            handle_suspend_project,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/actions/restore",
            handle_restore_project,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments",
            handle_list_environments,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments",
            handle_create_environment,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}",
            handle_get_environment,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}",
            handle_delete_environment,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/actions/suspend",
            handle_suspend_environment,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/actions/restore",
            handle_restore_environment,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_list_organizations(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    with_developer(graph, request, |actor, now| async move {
        let records = graph
            .organization_service()
            .list_organizations(&actor, limit(), now)
            .await
            .map_err(|error| organization_error(request, error))?;
        let items = records
            .iter()
            .map(|record| organization_wire(request, record))
            .collect::<Result<Vec<_>, _>>()?;
        json(request, 200, &ItemsWire { items })
    })
}

fn handle_create_organization(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let body: NameWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let id = OrganizationId::parse(stable_id(
            "org",
            &[
                actor.identity_id().as_str(),
                request.request_id(),
                &body.name,
            ],
        ))
        .map_err(|_| internal(request, "organization identifier generation failed"))?;
        let record = graph
            .organization_service()
            .create_organization(&actor, id, body.name, now)
            .await
            .map_err(|error| organization_error(request, error))?;
        json(request, 201, &organization_wire(request, &record)?)
    })
}

fn handle_get_organization(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let id = organization_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .organization_service()
            .get_organization(&actor, &id, now)
            .await
            .map_err(|error| organization_error(request, error))?;
        json(request, 200, &organization_wire(request, &record)?)
    })
}

fn handle_update_organization(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let id = organization_id(request)?;
    let body: NameWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .organization_service()
            .rename_organization(&actor, &id, body.name, now)
            .await
            .map_err(|error| organization_error(request, error))?;
        json(request, 200, &organization_wire(request, &record)?)
    })
}

fn handle_delete_organization(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_confirmation(request)?;
    let id = organization_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let current = graph
            .organization_service()
            .get_organization(&actor, &id, now)
            .await
            .map_err(|error| organization_error(request, error))?;
        let record = if current.lifecycle() == LifecycleState::DeletionGrace {
            current
        } else {
            graph
                .organization_service()
                .request_organization_deletion(
                    &actor,
                    &id,
                    now,
                    now.saturating_add(DELETION_GRACE_SECONDS),
                )
                .await
                .map_err(|error| organization_error(request, error))?
        };
        json(request, 202, &organization_wire(request, &record)?)
    })
}

fn handle_restore_organization(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_idempotency(request)?;
    let id = organization_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let current = graph
            .organization_service()
            .get_organization(&actor, &id, now)
            .await
            .map_err(|error| organization_error(request, error))?;
        let record = if current.lifecycle() == LifecycleState::Active {
            current
        } else {
            graph
                .organization_service()
                .restore_organization(&actor, &id, now)
                .await
                .map_err(|error| organization_error(request, error))?
        };
        json(request, 202, &organization_wire(request, &record)?)
    })
}

fn handle_create_invitation(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let organization_id = organization_id(request)?;
    let body: CreateInvitationWire = parse_json(request)?;
    let expires_at = parse_timestamp(request, &body.expires_at)?;
    with_developer(graph, request, |actor, now| async move {
        let id = InvitationId::parse(stable_id(
            "inv",
            &[
                organization_id.as_str(),
                actor.identity_id().as_str(),
                request.request_id(),
            ],
        ))
        .map_err(|_| internal(request, "invitation identifier generation failed"))?;
        let issued = graph
            .organization_service()
            .invite(
                &actor,
                NewInvitation {
                    id,
                    organization_id,
                    normalized_email: body.email,
                    role: body.role,
                    created_at_unix_seconds: now,
                    expires_at_unix_seconds: expires_at,
                },
            )
            .await
            .map_err(|error| organization_error(request, error))?;
        let invitation = invitation_wire(request, &issued.invitation)?;
        json(
            request,
            201,
            &InvitationIssueWire {
                invitation,
                token: issued.token.expose_once().to_owned(),
            },
        )
    })
}

fn handle_accept_invitation(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let invitation_id = invitation_id(request)?;
    let body: AcceptInvitationWire = parse_json(request)?;
    let token = InvitationToken::parse(body.token)
        .map_err(|_| invalid(request, "invitation token is invalid"))?;
    with_developer(graph, request, |actor, now| async move {
        let membership = graph
            .organization_service()
            .accept_invitation(&actor, &invitation_id, &token, now)
            .await
            .map_err(|error| organization_error(request, error))?;
        json(request, 200, &membership_wire(request, &membership)?)
    })
}

fn handle_list_members(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let organization_id = organization_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let records = graph
            .organization_service()
            .list_memberships(&actor, &organization_id, limit(), now)
            .await
            .map_err(|error| organization_error(request, error))?;
        let items = records
            .iter()
            .map(|record| membership_wire(request, record))
            .collect::<Result<Vec<_>, _>>()?;
        json(request, 200, &ItemsWire { items })
    })
}

fn handle_update_member(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let organization_id = organization_id(request)?;
    let developer_id = developer_id(request)?;
    let body: RoleWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let tenants = organization_tenants(graph, request, &actor, &organization_id, now).await?;
        let membership = graph
            .organization_service()
            .change_membership_role(&actor, &organization_id, &developer_id, body.role, now)
            .await
            .map_err(|error| organization_error(request, error))?;
        invalidate_developer_tenants(
            graph,
            request,
            &actor,
            &tenants,
            developer_id.as_str(),
            "membership-role",
        )?;
        json(request, 200, &membership_wire(request, &membership)?)
    })
}

fn handle_delete_member(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let organization_id = organization_id(request)?;
    let developer_id = developer_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let tenants = organization_tenants(graph, request, &actor, &organization_id, now).await?;
        graph
            .organization_service()
            .remove_membership(&actor, &organization_id, &developer_id, now)
            .await
            .map_err(|error| organization_error(request, error))?;
        invalidate_developer_tenants(
            graph,
            request,
            &actor,
            &tenants,
            developer_id.as_str(),
            "membership-remove",
        )?;
        Ok(HttpResponse::empty(204))
    })
}

fn handle_list_projects(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let organization_id = only_query_organization(request)?;
    with_developer(graph, request, |actor, now| async move {
        let records = graph
            .project_service()
            .list_projects(&actor, &organization_id, limit(), now)
            .await
            .map_err(|error| project_error(request, error))?;
        let items = records
            .iter()
            .map(|record| project_wire(request, record))
            .collect::<Result<Vec<_>, _>>()?;
        json(request, 200, &ItemsWire { items })
    })
}

fn handle_create_project(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let idempotency = require_idempotency(request)?.to_owned();
    let body: CreateProjectWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let id = ProjectId::parse(stable_id(
            "prj",
            &[
                body.organization_id.as_str(),
                actor.identity_id().as_str(),
                &idempotency,
            ],
        ))
        .map_err(|_| internal(request, "project identifier generation failed"))?;
        let record = match graph
            .project_service()
            .create_project(
                &actor,
                NewProject {
                    id: id.clone(),
                    organization_id: body.organization_id.clone(),
                    name: body.name.clone(),
                    region: body.region.clone(),
                    now_unix_seconds: now,
                },
            )
            .await
        {
            Ok(record) => record,
            Err(ProjectStoreError::Conflict) => {
                let existing = graph
                    .project_service()
                    .get_project(&actor, &id, now)
                    .await
                    .map_err(|error| project_error(request, error))?;
                if existing.organization_id() != &body.organization_id
                    || existing.name() != body.name
                    || existing.region() != body.region
                {
                    return Err(conflict(
                        request,
                        "idempotency key has different project input",
                    ));
                }
                existing
            }
            Err(error) => return Err(project_error(request, error)),
        };
        enqueue(
            graph,
            request,
            &idempotency,
            ProvisioningResource::Project(id),
            ProvisioningOperation::Create,
            now,
        )
        .await?;
        json(request, 202, &project_wire(request, &record)?)
    })
}

fn handle_get_project(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let project_id = project_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .project_service()
            .get_project(&actor, &project_id, now)
            .await
            .map_err(|error| project_error(request, error))?;
        json(request, 200, &project_wire(request, &record)?)
    })
}

fn handle_delete_project(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_confirmation(request)?;
    let project_id = project_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let current = graph
            .project_service()
            .get_project(&actor, &project_id, now)
            .await
            .map_err(|error| project_error(request, error))?;
        let record = if current.lifecycle() == LifecycleState::DeletionGrace {
            current
        } else {
            graph
                .project_service()
                .request_project_deletion(
                    &actor,
                    &project_id,
                    now,
                    now.saturating_add(DELETION_GRACE_SECONDS),
                )
                .await
                .map_err(|error| project_error(request, error))?
        };
        enqueue(
            graph,
            request,
            "lifecycle-delete",
            ProvisioningResource::Project(project_id),
            ProvisioningOperation::Delete,
            now,
        )
        .await?;
        invalidate_project_explorer(graph, request, &actor, record.id(), now, "project-delete")
            .await?;
        json(request, 202, &project_wire(request, &record)?)
    })
}

fn handle_suspend_project(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    project_lifecycle(graph, request, ProvisioningOperation::Suspend)
}

fn handle_restore_project(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    project_lifecycle(graph, request, ProvisioningOperation::Restore)
}

fn project_lifecycle(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: ProvisioningOperation,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let idempotency = require_idempotency(request)?.to_owned();
    let project_id = project_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let current = graph
            .project_service()
            .get_project(&actor, &project_id, now)
            .await
            .map_err(|error| project_error(request, error))?;
        let target = match operation {
            ProvisioningOperation::Suspend => LifecycleState::Suspended,
            ProvisioningOperation::Restore => LifecycleState::Active,
            _ => return Err(internal(request, "unsupported project lifecycle operation")),
        };
        let record = if current.lifecycle() == target {
            current
        } else {
            match operation {
                ProvisioningOperation::Suspend => {
                    graph
                        .project_service()
                        .suspend_project(&actor, &project_id, now)
                        .await
                }
                ProvisioningOperation::Restore => {
                    graph
                        .project_service()
                        .restore_project(&actor, &project_id, now)
                        .await
                }
                _ => unreachable!(),
            }
            .map_err(|error| project_error(request, error))?
        };
        enqueue(
            graph,
            request,
            &idempotency,
            ProvisioningResource::Project(project_id),
            operation,
            now,
        )
        .await?;
        invalidate_project_explorer(
            graph,
            request,
            &actor,
            record.id(),
            now,
            "project-lifecycle",
        )
        .await?;
        json(request, 202, &project_wire(request, &record)?)
    })
}

fn handle_list_environments(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let project_id = project_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let records = graph
            .project_service()
            .list_environments(&actor, &project_id, limit(), now)
            .await
            .map_err(|error| project_error(request, error))?;
        let items = records
            .iter()
            .map(|record| environment_wire(request, record))
            .collect::<Result<Vec<_>, _>>()?;
        json(request, 200, &ItemsWire { items })
    })
}

fn handle_create_environment(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let idempotency = require_idempotency(request)?.to_owned();
    let project_id = project_id(request)?;
    let body: NameWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let id = EnvironmentId::parse(stable_id(
            "env",
            &[
                project_id.as_str(),
                actor.identity_id().as_str(),
                &idempotency,
            ],
        ))
        .map_err(|_| internal(request, "environment identifier generation failed"))?;
        let record = match graph
            .project_service()
            .create_environment(
                &actor,
                NewEnvironment {
                    id: id.clone(),
                    project_id: project_id.clone(),
                    name: body.name.clone(),
                    now_unix_seconds: now,
                },
            )
            .await
        {
            Ok(record) => record,
            Err(ProjectStoreError::Conflict) => {
                let existing = graph
                    .project_service()
                    .get_environment(&actor, &project_id, &id, now)
                    .await
                    .map_err(|error| project_error(request, error))?;
                if existing.name() != body.name {
                    return Err(conflict(
                        request,
                        "idempotency key has different environment input",
                    ));
                }
                existing
            }
            Err(error) => return Err(project_error(request, error)),
        };
        let tenant = TenantScope::new(project_id, id);
        // A new environment starts on its organization's plan. Without this it
        // would be served under the deployment default instead, which is not
        // what anyone subscribed to.
        install_plan_limits(graph, request, &actor, &tenant, now).await?;
        enqueue(
            graph,
            request,
            &idempotency,
            ProvisioningResource::Environment(tenant),
            ProvisioningOperation::Create,
            now,
        )
        .await?;
        json(request, 202, &environment_wire(request, &record)?)
    })
}

/// Resolve what an organization is entitled to and install the limits that
/// follow into the data plane that serves the environment.
///
/// The plan and any exception made to it are control-plane state; the limits
/// they imply are what the gateway enforces. Translating here keeps that in one
/// place, so the data plane holds what it was given rather than a second copy
/// of the rules.
async fn install_plan_limits(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    now: u64,
) -> Result<(), HttpApiError> {
    let project = graph
        .project_store()
        .get_project(tenant.project_id())
        .await
        .map_err(|_| unavailable(request, "project authorization is unavailable"))?
        .ok_or_else(|| not_found(request, "project resource was not found"))?;
    let organization = graph
        .organization_store()
        .get_organization(project.organization_id())
        .await
        .map_err(|_| unavailable(request, "organization is unavailable"))?
        .ok_or_else(|| not_found(request, "organization was not found"))?;
    let exceptions = graph
        .operator_service()
        .plan_exceptions(project.organization_id(), now)
        .await
        .map_err(|_| unavailable(request, "plan exceptions are unavailable"))?;
    let policy = resolve_plan_policy(request, organization.plan_id(), &exceptions, now)?;
    crate::identity_admin_http::install_quota_policy(
        graph,
        request,
        actor.identity_id().as_str(),
        tenant,
        &policy,
    )
    .await
}

/// The limits an organization's plan implies, as the gateway will store them.
pub(crate) fn resolve_plan_policy(
    request: &HttpRequest,
    plan_id: &str,
    exceptions: &[mako_billing::PlanException],
    now_unix_seconds: u64,
) -> Result<serde_json::Value, HttpApiError> {
    // An organization on a plan the catalog no longer names is a deployment
    // bug, and inventing limits for it would hide that.
    let plan = mako_billing::plan(plan_id)
        .ok_or_else(|| internal(request, "the organization's plan is not in the catalog"))?;
    let plan = mako_billing::effective_plan(&plan, exceptions, now_unix_seconds);
    let policy = mako_billing::enforcement_policy(&plan.entitlements)
        .map_err(|_| internal(request, "plan limits could not be resolved"))?;
    serde_json::to_value(&policy).map_err(|_| internal(request, "plan limits could not be encoded"))
}

/// The organization's bill for the current period so far, and the balance it
/// implies. Shown, never collected: the beta charges nobody, and every
/// response says so rather than leaving it to be inferred.
fn handle_organization_bill(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let organization_id =
        OrganizationId::parse(request.path_parameter("organizationId").unwrap_or_default())
            .map_err(|_| invalid(request, "organization path is invalid"))?;
    with_developer(graph, request, |actor, now| async move {
        let organization = graph
            .organization_store()
            .get_organization(&organization_id)
            .await
            .map_err(|_| unavailable(request, "organization is unavailable"))?
            .ok_or_else(|| not_found(request, "organization was not found"))?;
        graph
            .organization_store()
            .get_membership(&organization_id, actor.identity_id())
            .await
            .map_err(|_| unavailable(request, "membership is unavailable"))?
            .ok_or_else(|| forbidden(request, "organization access is forbidden"))?;
        let plan = mako_billing::plan(organization.plan_id())
            .ok_or_else(|| internal(request, "the organization's plan is not in the catalog"))?;
        let exceptions = graph
            .operator_service()
            .plan_exceptions(&organization_id, now)
            .await
            .map_err(|_| unavailable(request, "plan exceptions are unavailable"))?;
        let plan = mako_billing::effective_plan(&plan, &exceptions, now);
        let card = mako_billing::rating::default_rate_card();

        // The period is the current calendar month so far. Nothing closes
        // periods yet; this is the live view the beta shows. Telemetry retains
        // less than a month, so the rated window starts at whichever is later:
        // the month, or the oldest evidence -- and the response reports the
        // start actually used rather than pretending to a window it cannot
        // have seen.
        let now_milliseconds = now.saturating_mul(1_000);
        let mut period_start = current_month_start_milliseconds(now);

        // Levels average per environment and then sum across them: two
        // environments each storing a gigabyte are two stored gigabytes, but
        // twelve samples of one environment's gigabyte are still one.
        let limit = NonZeroUsize::new(100).expect("listing limit");
        let mut usage: std::collections::BTreeMap<mako_api::QuotaResource, u64> =
            std::collections::BTreeMap::new();
        let projects = graph
            .project_store()
            .list_projects(&organization_id, limit)
            .await
            .map_err(|_| unavailable(request, "the organization's projects are unavailable"))?;
        for project in &projects {
            let environments = graph
                .project_store()
                .list_environments(project.id(), limit)
                .await
                .map_err(|_| unavailable(request, "the project's environments are unavailable"))?;
            for environment in &environments {
                let tenant = TenantScope::new(project.id().clone(), environment.id().clone());
                let mut per_tenant: std::collections::BTreeMap<mako_api::QuotaResource, Vec<u64>> =
                    std::collections::BTreeMap::new();
                let mut cursor: Option<String> = None;
                for _page in 0..16 {
                    // No `from` bound: the retention boundary is recomputed by
                    // the store on every call, and chasing it loses the race.
                    // The whole retained window is fetched and the period is
                    // applied here, where the clock stands still.
                    let page = graph
                        .observability_service()
                        .query_usage(
                            &actor,
                            &tenant,
                            &mako_api::ObservabilityQuery {
                                cursor: cursor.clone(),
                                from_unix_milliseconds: None,
                                until_unix_milliseconds: None,
                                limit: 1_000,
                            },
                            now_milliseconds,
                        )
                        .await
                        .map_err(|_| unavailable(request, "usage records are unavailable"))?;
                    // The bill covers the evidence that still exists; when
                    // retention is shorter than the month, the reported period
                    // start says so instead of pretending.
                    period_start = period_start.max(page.retention.retained_from_unix_milliseconds);
                    for record in &page.items {
                        if record.timestamp_unix_milliseconds < period_start {
                            continue;
                        }
                        if let mako_api::ObservabilityPayload::Usage {
                            resource, quantity, ..
                        } = &record.payload
                        {
                            per_tenant.entry(*resource).or_default().push(*quantity);
                        }
                    }
                    cursor = page.next_cursor;
                    if cursor.is_none() {
                        break;
                    }
                }
                for (resource, records) in per_tenant {
                    let quantity = mako_billing::rating::period_quantity(resource, &records);
                    let entry = usage.entry(resource).or_insert(0);
                    *entry = entry.saturating_add(quantity);
                }
            }
        }

        let rated = mako_billing::rating::rate_period(&plan, &card, &usage);
        json(
            request,
            200,
            &serde_json::json!({
                "organizationId": organization_id.as_str(),
                "planId": rated.plan_id,
                "periodStart": format_timestamp(request, period_start / 1_000)?,
                "observedAt": format_timestamp(request, now)?,
                "baseMicroDollars": rated.base_micro_dollars,
                "lineItems": rated.line_items,
                "totalMicroDollars": rated.total_micro_dollars,
                // Credits minus charges; with no credits and nothing
                // collected, accrued charges read negative.
                "balanceMicroDollars": -rated.total_micro_dollars,
                "collectable": false,
                "notice": "This bill is informational. Nothing is payable and no charge will be made during the beta.",
            }),
        )
    })
}

/// Midnight UTC on the first of the current month, in milliseconds.
fn current_month_start_milliseconds(now_unix_seconds: u64) -> u64 {
    const DAY: u64 = 24 * 60 * 60;
    let days = now_unix_seconds / DAY;
    // Civil-date arithmetic (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day_of_month = doy - (153 * mp + 2) / 5;
    let first_of_month_days = z - 719_468 - day_of_month;
    u64::try_from(first_of_month_days)
        .unwrap_or(0)
        .saturating_mul(DAY)
        .saturating_mul(1_000)
}

fn handle_get_environment(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let (project_id, environment_id) = environment_scope(request)?;
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .project_service()
            .get_environment(&actor, &project_id, &environment_id, now)
            .await
            .map_err(|error| project_error(request, error))?;
        json(request, 200, &environment_wire(request, &record)?)
    })
}

fn handle_delete_environment(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_confirmation(request)?;
    let (project_id, environment_id) = environment_scope(request)?;
    with_developer(graph, request, |actor, now| async move {
        let current = graph
            .project_service()
            .get_environment(&actor, &project_id, &environment_id, now)
            .await
            .map_err(|error| project_error(request, error))?;
        let record = if current.lifecycle() == LifecycleState::DeletionGrace {
            current
        } else {
            graph
                .project_service()
                .request_environment_deletion(
                    &actor,
                    &project_id,
                    &environment_id,
                    now,
                    now.saturating_add(DELETION_GRACE_SECONDS),
                )
                .await
                .map_err(|error| project_error(request, error))?
        };
        let tenant = TenantScope::new(project_id, environment_id);
        enqueue(
            graph,
            request,
            "lifecycle-delete",
            ProvisioningResource::Environment(tenant.clone()),
            ProvisioningOperation::Delete,
            now,
        )
        .await?;
        explorer_invalidation::advance_tenant(
            graph,
            request,
            &tenant,
            actor.identity_id().as_str(),
            "environment-delete",
        )?;
        json(request, 202, &environment_wire(request, &record)?)
    })
}

fn handle_suspend_environment(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    environment_lifecycle(graph, request, ProvisioningOperation::Suspend)
}

fn handle_restore_environment(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    environment_lifecycle(graph, request, ProvisioningOperation::Restore)
}

fn environment_lifecycle(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: ProvisioningOperation,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let idempotency = require_idempotency(request)?.to_owned();
    let (project_id, environment_id) = environment_scope(request)?;
    with_developer(graph, request, |actor, now| async move {
        let current = graph
            .project_service()
            .get_environment(&actor, &project_id, &environment_id, now)
            .await
            .map_err(|error| project_error(request, error))?;
        let target = match operation {
            ProvisioningOperation::Suspend => LifecycleState::Suspended,
            ProvisioningOperation::Restore => LifecycleState::Active,
            _ => {
                return Err(internal(
                    request,
                    "unsupported environment lifecycle operation",
                ));
            }
        };
        let record = if current.lifecycle() == target {
            current
        } else {
            match operation {
                ProvisioningOperation::Suspend => {
                    graph
                        .project_service()
                        .suspend_environment(&actor, &project_id, &environment_id, now)
                        .await
                }
                ProvisioningOperation::Restore => {
                    graph
                        .project_service()
                        .restore_environment(&actor, &project_id, &environment_id, now)
                        .await
                }
                _ => unreachable!(),
            }
            .map_err(|error| project_error(request, error))?
        };
        let tenant = TenantScope::new(project_id, environment_id);
        enqueue(
            graph,
            request,
            &idempotency,
            ProvisioningResource::Environment(tenant.clone()),
            operation,
            now,
        )
        .await?;
        explorer_invalidation::advance_tenant(
            graph,
            request,
            &tenant,
            actor.identity_id().as_str(),
            "environment-lifecycle",
        )?;
        json(request, 202, &environment_wire(request, &record)?)
    })
}

async fn organization_tenants(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    organization_id: &OrganizationId,
    now: u64,
) -> Result<Vec<TenantScope>, HttpApiError> {
    let projects = graph
        .project_service()
        .list_projects(actor, organization_id, limit(), now)
        .await
        .map_err(|_| unavailable(request, "explorer authorization scope is unavailable"))?;
    let mut tenants = Vec::new();
    for project in projects {
        let environments = graph
            .project_service()
            .list_environments(actor, project.id(), limit(), now)
            .await
            .map_err(|_| unavailable(request, "explorer authorization scope is unavailable"))?;
        tenants.extend(
            environments.into_iter().map(|environment| {
                TenantScope::new(project.id().clone(), environment.id().clone())
            }),
        );
    }
    Ok(tenants)
}

fn invalidate_developer_tenants(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenants: &[TenantScope],
    developer_identity_id: &str,
    reason: &str,
) -> Result<(), HttpApiError> {
    for tenant in tenants {
        explorer_invalidation::advance_developer(
            graph,
            request,
            tenant,
            actor.identity_id().as_str(),
            developer_identity_id,
            reason,
        )?;
    }
    Ok(())
}

async fn invalidate_project_explorer(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    project_id: &ProjectId,
    now: u64,
    reason: &str,
) -> Result<(), HttpApiError> {
    let environments = graph
        .project_service()
        .list_environments(actor, project_id, limit(), now)
        .await
        .map_err(|_| unavailable(request, "explorer authorization scope is unavailable"))?;
    for environment in environments {
        explorer_invalidation::advance_tenant(
            graph,
            request,
            &TenantScope::new(project_id.clone(), environment.id().clone()),
            actor.identity_id().as_str(),
            reason,
        )?;
    }
    Ok(())
}

async fn enqueue(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    idempotency: &str,
    resource: ProvisioningResource,
    operation: ProvisioningOperation,
    now: u64,
) -> Result<(), HttpApiError> {
    let resource_bytes = serde_json::to_vec(&resource)
        .map_err(|_| internal(request, "provisioning resource serialization failed"))?;
    let resource_text = String::from_utf8(resource_bytes)
        .map_err(|_| internal(request, "provisioning resource serialization failed"))?;
    let workflow_id = ProvisioningWorkflowId::parse(stable_id(
        "wf",
        &[
            idempotency,
            &resource_text,
            provisioning_operation(operation),
        ],
    ))
    .map_err(|_| {
        internal(
            request,
            "provisioning workflow identifier generation failed",
        )
    })?;
    graph
        .provisioner()
        .enqueue(workflow_id, resource, operation, now)
        .await
        .map(|_| ())
        .map_err(|error| provisioning_error(request, error))
}

pub(crate) fn with_developer<F, Fut>(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: F,
) -> Result<HttpResponse, HttpApiError>
where
    F: FnOnce(DeveloperPrincipal, u64) -> Fut,
    Fut: Future<Output = Result<HttpResponse, HttpApiError>>,
{
    let now = now_unix_seconds(request)?;
    let actor = block_on(
        graph
            .developer_authenticator()
            .authenticate(request.header(AUTHORIZATION_HEADER), now),
    )
    .map_err(|error| authentication_error(request, error))?;
    block_on(operation(actor, now))
}

fn organization_id(request: &HttpRequest) -> Result<OrganizationId, HttpApiError> {
    OrganizationId::parse(request.path_parameter("organizationId").unwrap_or_default())
        .map_err(|_| invalid(request, "organization path is invalid"))
}

fn invitation_id(request: &HttpRequest) -> Result<InvitationId, HttpApiError> {
    InvitationId::parse(request.path_parameter("invitationId").unwrap_or_default())
        .map_err(|_| invalid(request, "invitation path is invalid"))
}

fn developer_id(request: &HttpRequest) -> Result<DeveloperIdentityId, HttpApiError> {
    DeveloperIdentityId::parse(
        request
            .path_parameter("developerIdentityId")
            .unwrap_or_default(),
    )
    .map_err(|_| invalid(request, "developer identity path is invalid"))
}

fn project_id(request: &HttpRequest) -> Result<ProjectId, HttpApiError> {
    ProjectId::parse(request.path_parameter("projectId").unwrap_or_default())
        .map_err(|_| invalid(request, "project path is invalid"))
}

fn environment_scope(request: &HttpRequest) -> Result<(ProjectId, EnvironmentId), HttpApiError> {
    Ok((
        project_id(request)?,
        EnvironmentId::parse(request.path_parameter("environmentId").unwrap_or_default())
            .map_err(|_| invalid(request, "environment path is invalid"))?,
    ))
}

fn only_query_organization(request: &HttpRequest) -> Result<OrganizationId, HttpApiError> {
    if !request.body().is_empty() {
        return Err(invalid(request, "request body is not supported"));
    }
    if request.query().len() != 1 || request.query()[0].0 != "organizationId" {
        return Err(invalid(request, "organizationId query is required"));
    }
    OrganizationId::parse(request.query()[0].1.clone())
        .map_err(|_| invalid(request, "organizationId query is invalid"))
}

pub(crate) fn no_query(request: &HttpRequest) -> Result<(), HttpApiError> {
    if request.query().is_empty() {
        Ok(())
    } else {
        Err(invalid(request, "query parameters are not supported"))
    }
}

pub(crate) fn no_payload(request: &HttpRequest) -> Result<(), HttpApiError> {
    no_query(request)?;
    if request.body().is_empty() {
        Ok(())
    } else {
        Err(invalid(request, "request body is not supported"))
    }
}

pub(crate) fn require_json(request: &HttpRequest) -> Result<(), HttpApiError> {
    let content_type = request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type == Some(JSON_CONTENT_TYPE) {
        Ok(())
    } else {
        Err(invalid(request, "content type must be application/json"))
    }
}

pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(
    request: &HttpRequest,
) -> Result<T, HttpApiError> {
    serde_json::from_slice(request.body()).map_err(|_| invalid(request, "JSON body is invalid"))
}

fn require_confirmation(request: &HttpRequest) -> Result<&str, HttpApiError> {
    let value = request
        .header(CONFIRMATION_HEADER)
        .ok_or_else(|| invalid(request, "confirmation header is required"))?;
    validate_header_value(request, value, 8, 200, "confirmation header is invalid")?;
    Ok(value)
}

pub(crate) fn require_idempotency(request: &HttpRequest) -> Result<&str, HttpApiError> {
    let value = request
        .header(IDEMPOTENCY_HEADER)
        .ok_or_else(|| invalid(request, "idempotency key is required"))?;
    validate_header_value(request, value, 16, 200, "idempotency key is invalid")?;
    Ok(value)
}

fn validate_header_value(
    request: &HttpRequest,
    value: &str,
    minimum: usize,
    maximum: usize,
    message: &'static str,
) -> Result<(), HttpApiError> {
    if !(minimum..=maximum).contains(&value.len())
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(invalid(request, message))
    } else {
        Ok(())
    }
}

fn parse_timestamp(request: &HttpRequest, value: &str) -> Result<u64, HttpApiError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .and_then(|time| u64::try_from(time.unix_timestamp()).ok())
        .ok_or_else(|| invalid(request, "date-time is invalid"))
}

pub(crate) fn format_timestamp(request: &HttpRequest, value: u64) -> Result<String, HttpApiError> {
    i64::try_from(value)
        .ok()
        .and_then(|timestamp| OffsetDateTime::from_unix_timestamp(timestamp).ok())
        .and_then(|time| time.format(&Rfc3339).ok())
        .ok_or_else(|| internal(request, "stored timestamp is invalid"))
}

fn now_unix_seconds(request: &HttpRequest) -> Result<u64, HttpApiError> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| internal(request, "system clock is unavailable"))
}

pub(crate) fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("mako/control-plane/http-resource-id/v1");
    for part in parts {
        hasher.update(&(part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{prefix}_{}", &hasher.finalize().to_hex()[..32])
}

fn provisioning_operation(operation: ProvisioningOperation) -> &'static str {
    match operation {
        ProvisioningOperation::Create => "create",
        ProvisioningOperation::Suspend => "suspend",
        ProvisioningOperation::Restore => "restore",
        ProvisioningOperation::Delete => "delete",
    }
}

pub(crate) fn limit() -> NonZeroUsize {
    NonZeroUsize::new(LIST_LIMIT).expect("management list limit is positive")
}

pub(crate) fn json<T: Serialize>(
    request: &HttpRequest,
    status: u16,
    value: &T,
) -> Result<HttpResponse, HttpApiError> {
    HttpResponse::json(status, value)
        .map_err(|_| internal(request, "response serialization failed"))
}

fn authentication_error(
    request: &HttpRequest,
    error: DeveloperAuthenticationError,
) -> HttpApiError {
    if matches!(
        error,
        DeveloperAuthenticationError::Provider(
            mako_control_plane::IdentityProviderError::Unavailable
        )
    ) {
        unavailable(request, "developer identity provider is unavailable")
    } else {
        HttpApiError::new(
            401,
            ErrorCode::Unauthenticated,
            "developer authorization is invalid",
            request.request_id(),
            RetryAdvice::Never,
        )
    }
}

fn organization_error(request: &HttpRequest, error: OrganizationServiceError) -> HttpApiError {
    match error {
        OrganizationServiceError::NotFound => {
            not_found(request, "organization resource was not found")
        }
        OrganizationServiceError::Forbidden => {
            forbidden(request, "organization action is forbidden")
        }
        OrganizationServiceError::InvalidInvitation => {
            invalid(request, "invitation is invalid or expired")
        }
        OrganizationServiceError::LastOwner => {
            conflict(request, "organization must retain an owner")
        }
        OrganizationServiceError::Model(_) => {
            conflict(request, "organization lifecycle or input is invalid")
        }
        OrganizationServiceError::Store(_) => {
            unavailable(request, "organization storage is unavailable")
        }
    }
}

fn project_error(request: &HttpRequest, error: ProjectStoreError) -> HttpApiError {
    match error {
        ProjectStoreError::NotFound => not_found(request, "project resource was not found"),
        ProjectStoreError::Forbidden => forbidden(request, "project action is forbidden"),
        ProjectStoreError::Conflict => conflict(request, "project resource changed concurrently"),
        ProjectStoreError::Model(_) => conflict(request, "project lifecycle or input is invalid"),
        ProjectStoreError::RecordScopeMismatch => invalid(request, "project scope does not match"),
        ProjectStoreError::UnsupportedDurability
        | ProjectStoreError::CorruptRecord
        | ProjectStoreError::Organization(_)
        | ProjectStoreError::Keyspace(_)
        | ProjectStoreError::Storage(_)
        | ProjectStoreError::Serialization(_) => {
            unavailable(request, "project storage is unavailable")
        }
    }
}

fn provisioning_error(request: &HttpRequest, error: ProvisioningWorkflowError) -> HttpApiError {
    match error {
        ProvisioningWorkflowError::IdempotencyConflict | ProvisioningWorkflowError::Conflict => {
            conflict(request, "provisioning idempotency conflict")
        }
        _ => unavailable(request, "provisioning workflow is unavailable"),
    }
}

pub(crate) fn invalid(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        400,
        ErrorCode::InvalidRequest,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

pub(crate) fn forbidden(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        403,
        ErrorCode::PermissionDenied,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

pub(crate) fn not_found(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        404,
        ErrorCode::NotFound,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

pub(crate) fn conflict(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        409,
        ErrorCode::Conflict,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

pub(crate) fn unavailable(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        503,
        ErrorCode::Unavailable,
        message,
        request.request_id(),
        RetryAdvice::AfterDelay { after_ms: 1_000 },
    )
}

pub(crate) fn internal(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        500,
        ErrorCode::Internal,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn organization_wire(
    request: &HttpRequest,
    record: &OrganizationRecord,
) -> Result<OrganizationWire, HttpApiError> {
    Ok(OrganizationWire {
        id: record.id().as_str().to_owned(),
        name: record.name().to_owned(),
        state: lifecycle(record.lifecycle()),
        created_at: format_timestamp(request, record.created_at_unix_seconds())?,
        updated_at: format_timestamp(request, record.updated_at_unix_seconds())?,
        deletion_deadline: record
            .deletion_deadline_unix_seconds()
            .map(|value| format_timestamp(request, value))
            .transpose()?,
    })
}

fn membership_wire(
    request: &HttpRequest,
    record: &MembershipRecord,
) -> Result<MembershipWire, HttpApiError> {
    Ok(MembershipWire {
        organization_id: record.organization_id().as_str().to_owned(),
        developer_identity_id: record.developer_identity_id().as_str().to_owned(),
        role: record.role(),
        created_at: format_timestamp(request, record.created_at_unix_seconds())?,
        updated_at: format_timestamp(request, record.updated_at_unix_seconds())?,
    })
}

fn invitation_wire(
    request: &HttpRequest,
    record: &InvitationRecord,
) -> Result<InvitationWire, HttpApiError> {
    Ok(InvitationWire {
        id: record.id().as_str().to_owned(),
        organization_id: record.organization_id().as_str().to_owned(),
        email: record.normalized_email().to_owned(),
        role: record.role(),
        status: match record.status() {
            InvitationStatus::Pending => "pending",
            InvitationStatus::Accepted => "accepted",
            InvitationStatus::Revoked => "revoked",
            InvitationStatus::Expired => "expired",
        },
        expires_at: format_timestamp(request, record.expires_at_unix_seconds())?,
        created_at: format_timestamp(request, record.created_at_unix_seconds())?,
    })
}

fn project_wire(
    request: &HttpRequest,
    record: &ProjectRecord,
) -> Result<ProjectWire, HttpApiError> {
    Ok(ProjectWire {
        id: record.id().as_str().to_owned(),
        organization_id: record.organization_id().as_str().to_owned(),
        name: record.name().to_owned(),
        region: record.region().to_owned(),
        state: lifecycle(record.lifecycle()),
        created_at: format_timestamp(request, record.created_at_unix_seconds())?,
        updated_at: format_timestamp(request, record.updated_at_unix_seconds())?,
        deletion_deadline: record
            .deletion_deadline_unix_seconds()
            .map(|value| format_timestamp(request, value))
            .transpose()?,
        failure_diagnostic: record.failure_diagnostic().map(str::to_owned),
    })
}

fn environment_wire(
    request: &HttpRequest,
    record: &EnvironmentRecord,
) -> Result<EnvironmentWire, HttpApiError> {
    Ok(EnvironmentWire {
        id: record.id().as_str().to_owned(),
        project_id: record.project_id().as_str().to_owned(),
        name: record.name().to_owned(),
        state: lifecycle(record.lifecycle()),
        created_at: format_timestamp(request, record.created_at_unix_seconds())?,
        updated_at: format_timestamp(request, record.updated_at_unix_seconds())?,
        deletion_deadline: record
            .deletion_deadline_unix_seconds()
            .map(|value| format_timestamp(request, value))
            .transpose()?,
        failure_diagnostic: record.failure_diagnostic().map(str::to_owned),
    })
}

fn lifecycle(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Provisioning => "provisioning",
        LifecycleState::Active => "active",
        LifecycleState::Suspended => "suspended",
        LifecycleState::Failed => "failed",
        LifecycleState::DeletionGrace => "deletion_grace",
        LifecycleState::Deleting => "deleting",
        LifecycleState::Deleted => "deleted",
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameWire {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateInvitationWire {
    email: String,
    role: OrganizationRole,
    expires_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptInvitationWire {
    token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleWire {
    role: OrganizationRole,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateProjectWire {
    organization_id: OrganizationId,
    name: String,
    region: String,
}

#[derive(Serialize)]
struct ItemsWire<T> {
    items: Vec<T>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OrganizationWire {
    id: String,
    name: String,
    state: &'static str,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deletion_deadline: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MembershipWire {
    organization_id: String,
    developer_identity_id: String,
    role: OrganizationRole,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InvitationWire {
    id: String,
    organization_id: String,
    email: String,
    role: OrganizationRole,
    status: &'static str,
    expires_at: String,
    created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InvitationIssueWire {
    invitation: InvitationWire,
    token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectWire {
    id: String,
    organization_id: String,
    name: String,
    region: String,
    state: &'static str,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deletion_deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_diagnostic: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentWire {
    id: String,
    project_id: String,
    name: String,
    state: &'static str,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deletion_deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_diagnostic: Option<String>,
}
