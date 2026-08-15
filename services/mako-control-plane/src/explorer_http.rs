use std::{collections::BTreeSet, sync::Arc};

use mako_api::ExplorerGrantRequest;
use mako_control_plane::ExplorerGrantError;
use mako_internal_rpc::{IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission};
use mako_policy::ExplorerGrantAuthorityRecord;
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};

use crate::{
    ControlPlaneGraph,
    http_support::tenant,
    management_http::{
        forbidden, invalid, json, no_payload, no_query, not_found, parse_json, require_json,
        unavailable, with_developer,
    },
};

pub(crate) fn add_explorer_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/explorer/grants",
            issue_grant as Handler,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/explorer/grants/{grantId}",
            revoke_grant,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn issue_grant(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let trusted_tenant = tenant(request)?;
    let body: ExplorerGrantRequest = parse_json(request)?;
    body.tenant
        .ensure_matches(&trusted_tenant)
        .map_err(|_| not_found(request, "explorer resource was not found"))?;
    with_developer(graph, request, |actor, now| async move {
        let grant = graph
            .explorer_grant_service()
            .issue(&actor, body, now)
            .await
            .map_err(|error| explorer_error(request, error))?;
        let nonce = nonce_for_grant_id(&grant.grant_id)
            .ok_or_else(|| unavailable(request, "explorer authorization is unavailable"))?;
        let command = IdentityAdminCommand {
            operation: IdentityAdminOperation::IssueExplorerGrant,
            actor_id: actor.identity_id().as_str().to_owned(),
            permissions: BTreeSet::from([IdentityAdminPermission::ManageExplorerGrants]),
            input: serde_json::to_value(ExplorerGrantAuthorityRecord {
                nonce,
                developer_identity_id: actor.identity_id().as_str().to_owned(),
                mode: grant.mode,
                authorization_epoch: grant.authorization_epoch,
                issued_at_unix_seconds: grant.issued_at_unix_seconds,
                expires_at_unix_seconds: grant.expires_at_unix_seconds,
                revoked_at_unix_seconds: None,
            })
            .map_err(|_| unavailable(request, "explorer authorization is unavailable"))?,
        };
        graph
            .data_plane_identity_admin()
            .administer::<serde_json::Value>(
                &trusted_tenant,
                request.request_id(),
                &grant.grant_id,
                &command,
            )
            .map_err(|_| unavailable(request, "explorer authorization is unavailable"))?;
        json(request, 201, &grant)
    })
}

fn revoke_grant(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let trusted_tenant = tenant(request)?;
    let grant_id = request
        .path_parameter("grantId")
        .filter(|value| value.starts_with("xgr_") && value.len() == 36)
        .ok_or_else(|| not_found(request, "explorer resource was not found"))?
        .to_owned();
    with_developer(graph, request, |actor, now| async move {
        let nonce = nonce_for_grant_id(&grant_id)
            .ok_or_else(|| not_found(request, "explorer resource was not found"))?;
        graph
            .data_plane_identity_admin()
            .administer::<serde_json::Value>(
                &trusted_tenant,
                request.request_id(),
                &format!("revoke-{grant_id}"),
                &IdentityAdminCommand {
                    operation: IdentityAdminOperation::RevokeExplorerGrant,
                    actor_id: actor.identity_id().as_str().to_owned(),
                    permissions: BTreeSet::from([IdentityAdminPermission::ManageExplorerGrants]),
                    input: serde_json::json!({
                        "nonce": nonce,
                        "developerIdentityId": actor.identity_id().as_str(),
                    }),
                },
            )
            .map_err(|_| unavailable(request, "explorer authorization is unavailable"))?;
        let result = graph
            .explorer_grant_service()
            .revoke_by_grant_id(&actor, &trusted_tenant, &grant_id, now)
            .await
            .map_err(|error| explorer_error(request, error))?;
        json(request, 200, &result)
    })
}

fn nonce_for_grant_id(grant_id: &str) -> Option<String> {
    grant_id
        .strip_prefix("xgr_")
        .filter(|suffix| suffix.len() == 32)
        .map(|suffix| format!("xnonce_{suffix}"))
}

fn explorer_error(request: &HttpRequest, error: ExplorerGrantError) -> HttpApiError {
    match error {
        ExplorerGrantError::InvalidRequest => invalid(request, "explorer request is invalid"),
        ExplorerGrantError::NotFound => not_found(request, "explorer resource was not found"),
        ExplorerGrantError::Forbidden => forbidden(request, "explorer action is forbidden"),
        ExplorerGrantError::TenantUnavailable => {
            unavailable(request, "explorer tenant is unavailable")
        }
        _ => unavailable(request, "explorer authorization is unavailable"),
    }
}
