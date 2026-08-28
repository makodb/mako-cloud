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
            "/v1/teams",
            handle_list_organizations as Handler,
        ),
        (
            HttpMethod::Get,
            "/v1/teams/{teamId}/bill",
            handle_organization_bill,
        ),
        (HttpMethod::Post, "/v1/teams", handle_create_organization),
        (
            HttpMethod::Get,
            "/v1/teams/{teamId}",
            handle_get_organization,
        ),
        (
            HttpMethod::Patch,
            "/v1/teams/{teamId}",
            handle_update_organization,
        ),
        (
            HttpMethod::Delete,
            "/v1/teams/{teamId}",
            handle_delete_organization,
        ),
        (
            HttpMethod::Post,
            "/v1/teams/{teamId}/actions/restore",
            handle_restore_organization,
        ),
        (
            HttpMethod::Post,
            "/v1/teams/{teamId}/invitations",
            handle_create_invitation,
        ),
        (
            HttpMethod::Post,
            "/v1/invitations/{invitationId}/accept",
            handle_accept_invitation,
        ),
        (
            HttpMethod::Get,
            "/v1/teams/{teamId}/members",
            handle_list_members,
        ),
        (
            HttpMethod::Patch,
            "/v1/teams/{teamId}/members/{developerIdentityId}",
            handle_update_member,
        ),
        (
            HttpMethod::Delete,
            "/v1/teams/{teamId}/members/{developerIdentityId}",
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
        .map_err(|_| internal(request, "team identifier generation failed"))?;
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
        .map_err(|_| unavailable(request, "team is unavailable"))?
        .ok_or_else(|| not_found(request, "team was not found"))?;
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
        .ok_or_else(|| internal(request, "the team's plan is not in the catalog"))?;
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
    if !request.body().is_empty() {
        return Err(invalid(request, "request body is not supported"));
    }
    let organization_id =
        OrganizationId::parse(request.path_parameter("teamId").unwrap_or_default())
            .map_err(|_| invalid(request, "team path is invalid"))?;
    // The only query this endpoint understands is a past period to show.
    let requested_period = match request.query() {
        [] => None,
        [(name, value)] if name == "period" => Some(
            parse_billing_period(value)
                .ok_or_else(|| invalid(request, "period must be a calendar month as YYYY-MM"))?,
        ),
        _ => return Err(invalid(request, "unexpected query parameters")),
    };
    with_developer(graph, request, |actor, now| async move {
        let organization = graph
            .organization_store()
            .get_organization(&organization_id)
            .await
            .map_err(|_| unavailable(request, "team is unavailable"))?
            .ok_or_else(|| not_found(request, "team was not found"))?;
        graph
            .organization_store()
            .get_membership(&organization_id, actor.identity_id())
            .await
            .map_err(|_| unavailable(request, "membership is unavailable"))?
            .ok_or_else(|| forbidden(request, "team access is forbidden"))?;
        let exceptions = graph
            .operator_service()
            .plan_exceptions(&organization_id, now)
            .await
            .map_err(|_| unavailable(request, "plan exceptions are unavailable"))?;

        let now_milliseconds = now.saturating_mul(1_000);
        let (current_year, current_month) = year_and_month(now);
        let current_start = month_start_milliseconds(current_year, current_month);

        // Ended months still inside the closable horizon are closed before
        // anything is answered, so the balance below counts every period and
        // evidence is snapshotted before retention can take it.
        for months_back in (1..=CLOSABLE_MONTHS_BACK).rev() {
            let (year, month) = months_before(current_year, current_month, months_back);
            let period_start = month_start_milliseconds(year, month);
            let period_end = month_end_milliseconds(year, month);
            if period_end <= organization.created_at_unix_seconds().saturating_mul(1_000) {
                continue;
            }
            if graph
                .organization_store()
                .get_invoice(&organization_id, period_start)
                .await
                .map_err(|_| unavailable(request, "closed periods are unavailable"))?
                .is_some()
            {
                continue;
            }
            let (rated, derived_from) = derive_rated_period(
                graph,
                &actor,
                request,
                &organization,
                &exceptions,
                period_start,
                period_end,
                now,
            )
            .await?;
            let invoice = mako_control_plane::InvoiceRecord {
                organization_id: organization_id.clone(),
                period_start_unix_milliseconds: period_start,
                period_end_unix_milliseconds: period_end,
                derived_from_unix_milliseconds: derived_from,
                rated,
                closed_at_unix_seconds: now,
            };
            match graph.organization_store().close_invoice(&invoice).await {
                Ok(()) => {}
                // Another read closed it first; theirs is the invoice.
                Err(mako_control_plane::OrganizationStoreError::Conflict) => {}
                Err(_) => return Err(unavailable(request, "the period could not be closed")),
            }
        }

        let invoices = graph
            .organization_store()
            .list_invoices(&organization_id)
            .await
            .map_err(|_| unavailable(request, "closed periods are unavailable"))?;
        let closed_total = invoices.iter().fold(0_i64, |total, invoice| {
            total.saturating_add(invoice.rated.total_micro_dollars)
        });
        let credits = graph
            .operator_service()
            .credit_total(&organization_id)
            .await
            .map_err(|_| unavailable(request, "credits are unavailable"))?;

        match requested_period {
            // The live view: the current month so far, never finalized. The
            // window's exclusive end reaches one second past now so a usage
            // record stamped in the second the bill is read still counts --
            // timestamps carry whole seconds, and a sample that observability
            // already serves must not be missing from the bill read alongside.
            None => {
                let (rated, derived_from) = derive_rated_period(
                    graph,
                    &actor,
                    request,
                    &organization,
                    &exceptions,
                    current_start,
                    now_milliseconds.saturating_add(1_000),
                    now,
                )
                .await?;
                let balance = credits
                    .saturating_sub(closed_total)
                    .saturating_sub(rated.total_micro_dollars);
                bill_response(
                    request,
                    &organization_id,
                    &rated,
                    derived_from,
                    now_milliseconds,
                    now,
                    credits,
                    balance,
                    None,
                )
            }
            Some((year, month)) => {
                let period_start = month_start_milliseconds(year, month);
                if period_start >= now_milliseconds {
                    return Err(invalid(request, "the requested period has not started"));
                }
                if period_start == current_start {
                    return Err(invalid(
                        request,
                        "the current month is the bill itself; request it without a period",
                    ));
                }
                // Past months answer from what was closed. One that was never
                // closed while its evidence existed cannot be reconstructed,
                // and saying so beats inventing a number.
                let invoice = graph
                    .organization_store()
                    .get_invoice(&organization_id, period_start)
                    .await
                    .map_err(|_| unavailable(request, "closed periods are unavailable"))?
                    .ok_or_else(|| {
                        not_found(request, "no invoice was closed for the requested period")
                    })?;
                let balance = credits.saturating_sub(closed_total);
                bill_response(
                    request,
                    &organization_id,
                    &invoice.rated,
                    invoice.derived_from_unix_milliseconds,
                    invoice.period_end_unix_milliseconds,
                    now,
                    credits,
                    balance,
                    Some(invoice.closed_at_unix_seconds),
                )
            }
        }
    })
}

/// Ended months this many back are closed on any bill read. Three keeps every
/// month whose end can still be inside the ninety-day telemetry retention.
const CLOSABLE_MONTHS_BACK: u32 = 3;

/// Derive the rated period for one window of an organization's history.
///
/// The window is split into stretches by the organization's recorded plan
/// changes, usage records are bucketed into the stretch their timestamp fell
/// in, and each stretch is rated under its own plan's terms prorated by how
/// long it held. Returns the rated period and where retained evidence
/// actually began -- the calendar window start when everything was still on
/// record, later when part of it had aged out.
#[allow(clippy::too_many_arguments)]
async fn derive_rated_period(
    graph: &Arc<ControlPlaneGraph>,
    actor: &mako_control_plane::DeveloperPrincipal,
    request: &HttpRequest,
    organization: &mako_control_plane::OrganizationRecord,
    exceptions: &[mako_billing::PlanException],
    period_start: u64,
    period_end: u64,
    now: u64,
) -> Result<(mako_billing::rating::RatedPeriod, u64), HttpApiError> {
    let now_milliseconds = now.saturating_mul(1_000);
    // Which plan held when, as stretch starts in milliseconds clipped to the
    // window. Weights come from the calendar window, not from retention: the
    // base fee covers time subscribed, which no telemetry needs to prove.
    let stretches = organization.plan_stretches(period_start / 1_000);
    let mut segments: Vec<(String, u64)> = Vec::new();
    for (plan_id, from_unix_seconds) in stretches {
        let from = from_unix_seconds.saturating_mul(1_000).max(period_start);
        if from >= period_end {
            break;
        }
        segments.push((plan_id, from));
    }
    if segments.is_empty() {
        segments.push((organization.plan_id().to_owned(), period_start));
    }
    let segment_end = |index: usize| segments.get(index + 1).map_or(period_end, |next| next.1);

    // Usage per stretch. Levels average per environment and then sum across
    // them: two environments each storing a gigabyte are two stored
    // gigabytes, but twelve samples of one environment's gigabyte are one.
    let mut usage_by_segment: Vec<std::collections::BTreeMap<mako_api::QuotaResource, u64>> =
        vec![std::collections::BTreeMap::new(); segments.len()];
    let mut evidence_start = period_start;
    let limit = NonZeroUsize::new(100).expect("listing limit");
    let projects = graph
        .project_store()
        .list_projects(organization.id(), limit)
        .await
        .map_err(|_| unavailable(request, "the team's projects are unavailable"))?;
    for project in &projects {
        let environments = graph
            .project_store()
            .list_environments(project.id(), limit)
            .await
            .map_err(|_| unavailable(request, "the project's environments are unavailable"))?;
        for environment in &environments {
            let tenant = TenantScope::new(project.id().clone(), environment.id().clone());
            let mut per_segment: Vec<
                std::collections::BTreeMap<mako_api::QuotaResource, Vec<u64>>,
            > = vec![std::collections::BTreeMap::new(); segments.len()];
            let mut cursor: Option<String> = None;
            for _page in 0..16 {
                // No `from` bound: the retention boundary is recomputed by
                // the store on every call, and chasing it loses the race.
                // The whole retained window is fetched and the period is
                // applied here, where the clock stands still.
                let page = graph
                    .observability_service()
                    .query_usage(
                        actor,
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
                // retention starts inside the window, the reported start
                // says so instead of pretending.
                evidence_start = evidence_start.max(page.retention.retained_from_unix_milliseconds);
                for record in &page.items {
                    let at = record.timestamp_unix_milliseconds;
                    if at < evidence_start || at >= period_end {
                        continue;
                    }
                    if let mako_api::ObservabilityPayload::Usage {
                        resource, quantity, ..
                    } = &record.payload
                    {
                        let index = segments
                            .iter()
                            .rposition(|(_, from)| *from <= at)
                            .unwrap_or(0);
                        per_segment[index]
                            .entry(*resource)
                            .or_default()
                            .push(*quantity);
                    }
                }
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            for (index, resources) in per_segment.into_iter().enumerate() {
                for (resource, records) in resources {
                    let quantity = mako_billing::rating::period_quantity(resource, &records);
                    let entry = usage_by_segment[index].entry(resource).or_insert(0);
                    *entry = entry.saturating_add(quantity);
                }
            }
        }
    }

    let card = mako_billing::rating::default_rate_card();
    let mut rated_segments = Vec::with_capacity(segments.len());
    for (index, (plan_id, from)) in segments.iter().enumerate() {
        let plan = mako_billing::plan(plan_id)
            .ok_or_else(|| internal(request, "the team's plan is not in the catalog"))?;
        rated_segments.push(mako_billing::rating::PlanSegment {
            plan: mako_billing::effective_plan(&plan, exceptions, now),
            milliseconds: segment_end(index).saturating_sub(*from),
            usage: std::mem::take(&mut usage_by_segment[index]),
        });
    }
    let rated = mako_billing::rating::rate_period_prorated(&card, &rated_segments)
        .ok_or_else(|| internal(request, "the period has no time to rate"))?;
    // Evidence beginning after the window ended means none of it remained;
    // the clamp records that as an empty window rather than an inverted one.
    Ok((rated, evidence_start.min(period_end)))
}

#[allow(clippy::too_many_arguments)]
fn bill_response(
    request: &HttpRequest,
    organization_id: &OrganizationId,
    rated: &mako_billing::rating::RatedPeriod,
    period_start: u64,
    period_end: u64,
    now: u64,
    credits: i64,
    balance: i64,
    closed_at_unix_seconds: Option<u64>,
) -> Result<HttpResponse, HttpApiError> {
    json(
        request,
        200,
        &serde_json::json!({
            "teamId": organization_id.as_str(),
            "planId": rated.plan_id,
            "periodStart": format_timestamp(request, period_start / 1_000)?,
            "periodEnd": format_timestamp(request, period_end / 1_000)?,
            "observedAt": format_timestamp(request, now)?,
            "finalized": closed_at_unix_seconds.is_some(),
            "closedAt": closed_at_unix_seconds
                .map(|at| format_timestamp(request, at))
                .transpose()?,
            "baseMicroDollars": rated.base_micro_dollars,
            "lineItems": rated.line_items,
            "totalMicroDollars": rated.total_micro_dollars,
            "creditsMicroDollars": credits,
            // Credits minus charges; accrued use beyond credit reads
            // negative, and nothing clamps it at zero -- hiding the number
            // is the one thing this surface must not do.
            "balanceMicroDollars": balance,
            "collectable": false,
            "notice": "This bill is informational. Nothing is payable and no charge will be made during the beta.",
        }),
    )
}

/// `YYYY-MM`, strictly: four digits, a dash, a month `01..=12`.
fn parse_billing_period(value: &str) -> Option<(i64, u32)> {
    let (year, month) = value.split_once('-')?;
    if year.len() != 4 || month.len() != 2 {
        return None;
    }
    let year = year.parse::<i64>().ok()?;
    let month = month
        .parse::<u32>()
        .ok()
        .filter(|month| (1..=12).contains(month))?;
    (year >= 1970).then_some((year, month))
}

/// The civil year and month `now` falls in, UTC.
fn year_and_month(now_unix_seconds: u64) -> (i64, u32) {
    const DAY: u64 = 24 * 60 * 60;
    // Civil-date arithmetic (Howard Hinnant's algorithms).
    let z = i64::try_from(now_unix_seconds / DAY).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, u32::try_from(month).unwrap_or(1))
}

/// Midnight UTC on the first of the month, in milliseconds.
fn month_start_milliseconds(year: i64, month: u32) -> u64 {
    const DAY: u64 = 24 * 60 * 60;
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days)
        .unwrap_or(0)
        .saturating_mul(DAY)
        .saturating_mul(1_000)
}

/// The instant the month ends: the start of the next one.
fn month_end_milliseconds(year: i64, month: u32) -> u64 {
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    month_start_milliseconds(year, month)
}

/// The year and month `months` before the given one.
fn months_before(year: i64, month: u32, months: u32) -> (i64, u32) {
    let total = year * 12 + i64::from(month) - 1 - i64::from(months);
    (
        total.div_euclid(12),
        u32::try_from(total.rem_euclid(12)).unwrap_or(0) + 1,
    )
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
    OrganizationId::parse(request.path_parameter("teamId").unwrap_or_default())
        .map_err(|_| invalid(request, "team path is invalid"))
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
    if request.query().len() != 1 || request.query()[0].0 != "teamId" {
        return Err(invalid(request, "teamId query is required"));
    }
    OrganizationId::parse(request.query()[0].1.clone())
        .map_err(|_| invalid(request, "teamId query is invalid"))
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
        OrganizationServiceError::NotFound => not_found(request, "team resource was not found"),
        OrganizationServiceError::Forbidden => forbidden(request, "team action is forbidden"),
        OrganizationServiceError::InvalidInvitation => {
            invalid(request, "invitation is invalid or expired")
        }
        OrganizationServiceError::LastOwner => conflict(request, "team must retain an owner"),
        OrganizationServiceError::Model(_) => {
            conflict(request, "team lifecycle or input is invalid")
        }
        OrganizationServiceError::Store(_) => unavailable(request, "team storage is unavailable"),
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
    #[serde(rename = "teamId")]
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
    #[serde(rename = "teamId")]
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
    #[serde(rename = "teamId")]
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
    #[serde(rename = "teamId")]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Calendar arithmetic the biller stands on: a month's start must invert
    /// the year-and-month read, the end must be the next month's start across
    /// a year boundary, and stepping back must borrow through January.
    #[test]
    fn billing_month_arithmetic_is_civil_and_inverts_itself() {
        // 2026-08-01T00:00:00Z.
        const AUGUST_FIRST: u64 = 1_785_542_400;
        assert_eq!(year_and_month(AUGUST_FIRST + 24 * 24 * 60 * 60), (2026, 8));
        assert_eq!(month_start_milliseconds(2026, 8), AUGUST_FIRST * 1_000);
        assert_eq!(
            month_end_milliseconds(2026, 8),
            (AUGUST_FIRST + 31 * 24 * 60 * 60) * 1_000,
            "August ends where September begins"
        );
        assert_eq!(
            month_end_milliseconds(2026, 12),
            month_start_milliseconds(2027, 1)
        );
        assert_eq!(months_before(2026, 1, 1), (2025, 12));
        assert_eq!(months_before(2026, 8, 3), (2026, 5));
        // Every month of a leap and a common year round-trips.
        for year in [2024_i64, 2026] {
            for month in 1..=12 {
                let start = month_start_milliseconds(year, month);
                assert_eq!(year_and_month(start / 1_000), (year, month));
                assert!(month_end_milliseconds(year, month) > start);
            }
        }
    }

    /// The period parameter is a wire input; anything but a strict calendar
    /// month must be refused rather than guessed at.
    #[test]
    fn a_billing_period_parses_strictly_or_not_at_all() {
        assert_eq!(parse_billing_period("2026-07"), Some((2026, 7)));
        assert_eq!(parse_billing_period("2026-12"), Some((2026, 12)));
        for rejected in [
            "2026-13",
            "2026-00",
            "2026-7",
            "26-07",
            "2026/07",
            "2026-07-01",
            "-2026-07",
            "1969-12",
            "",
            "yyyy-mm",
        ] {
            assert_eq!(
                parse_billing_period(rejected),
                None,
                "{rejected} was accepted"
            );
        }
    }
}
