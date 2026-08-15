use std::collections::BTreeSet;

use mako_api::TenantScope;
use mako_internal_rpc::{IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission};
use mako_service_runtime::{HttpApiError, HttpRequest};

use crate::{ControlPlaneGraph, management_http::unavailable};

pub(crate) fn advance_tenant(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    actor_id: &str,
    reason: &str,
) -> Result<(), HttpApiError> {
    advance(
        graph,
        request,
        tenant,
        actor_id,
        reason,
        serde_json::json!({
            "allDevelopers": true,
        }),
    )
}

pub(crate) fn advance_developer(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    actor_id: &str,
    developer_identity_id: &str,
    reason: &str,
) -> Result<(), HttpApiError> {
    advance(
        graph,
        request,
        tenant,
        actor_id,
        reason,
        serde_json::json!({
            "developerIdentityId": developer_identity_id,
            "allDevelopers": false,
        }),
    )
}

fn advance(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    actor_id: &str,
    reason: &str,
    input: serde_json::Value,
) -> Result<(), HttpApiError> {
    let idempotency = format!(
        "explorer-epoch-{reason}-{}-{}",
        tenant.project_id(),
        tenant.environment_id()
    );
    graph
        .data_plane_identity_admin()
        .administer::<serde_json::Value>(
            tenant,
            request.request_id(),
            &idempotency,
            &IdentityAdminCommand {
                operation: IdentityAdminOperation::AdvanceExplorerEpoch,
                actor_id: actor_id.to_owned(),
                permissions: BTreeSet::from([IdentityAdminPermission::ManageExplorerGrants]),
                input,
            },
        )
        .map(|_| ())
        .map_err(|_| unavailable(request, "explorer authorization could not be invalidated"))
}
