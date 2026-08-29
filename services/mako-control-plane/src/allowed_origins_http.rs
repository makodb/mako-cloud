//! Management routes for an environment's cross-origin allowlist.
//!
//! The control plane stores the list the developer sets and installs it
//! into the data plane, which is what actually answers a browser. A `PUT`
//! that stores the list but cannot install it answers `503`: the stored
//! list is the developer's intent, and repeating the same `PUT` installs it
//! again, so a retry is always the right response to that failure.

use std::{collections::BTreeSet, sync::Arc};

use mako_api::{ErrorCode, RetryAdvice, TenantScope};
use mako_control_plane::AllowedOriginsError;
use mako_internal_rpc::{
    IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission,
    InstallAllowedOriginsInput,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ControlPlaneGraph,
    http_support::{public_json, tenant},
    identity_admin_http::rpc_error,
    management_http::{
        conflict, forbidden, no_payload, no_query, not_found, parse_json, require_idempotency,
        require_json, unavailable, with_developer,
    },
};

const PATH: &str = "/v1/projects/{projectId}/environments/{environmentId}/allowed-origins";

pub(crate) fn add_allowed_origins_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    let read_graph = Arc::clone(&graph);
    router.add_route(HttpMethod::Get, PATH, move |request| {
        handle_get(&read_graph, &request)
    })?;
    router.add_route(HttpMethod::Put, PATH, move |request| {
        handle_replace(&graph, &request)
    })
}

fn handle_get(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let allowed_origins = graph
            .allowed_origins_service()
            .get(&actor, &tenant, now)
            .await
            .map_err(|error| origins_error(request, error))?;
        public_json(request, 200, &AllowedOriginsWire { allowed_origins })
    })
}

fn handle_replace(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let body: AllowedOriginsWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let allowed_origins = graph
            .allowed_origins_service()
            .replace(&actor, &tenant, &body.allowed_origins, now)
            .await
            .map_err(|error| origins_error(request, error))?;
        install(graph, request, &tenant, &allowed_origins).await?;
        public_json(request, 200, &AllowedOriginsWire { allowed_origins })
    })
}

/// Hands the stored list to the data plane, which decides every browser
/// request from it. The install is the platform's own act on the
/// developer's behalf, so it carries exactly the permission the operation
/// needs, like the quota policy an environment is created with.
async fn install(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    origins: &[String],
) -> Result<(), HttpApiError> {
    let command = IdentityAdminCommand {
        operation: IdentityAdminOperation::InstallAllowedOrigins,
        actor_id: "control-plane".to_owned(),
        permissions: BTreeSet::from([IdentityAdminPermission::ManageProjectCredentials]),
        input: serde_json::to_value(InstallAllowedOriginsInput {
            origins: origins.to_vec(),
        })
        .map_err(|_| unavailable(request, "allowed origins could not be installed"))?,
    };
    let _: Value = graph
        .data_plane_identity_admin()
        .administer(
            tenant,
            request.request_id(),
            require_idempotency(request)?,
            &command,
        )
        .map_err(|error| rpc_error(request, error))?;
    Ok(())
}

/// `AllowedOrigins` on the wire, for both the request body and the response.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AllowedOriginsWire {
    allowed_origins: Vec<String>,
}

fn origins_error(request: &HttpRequest, error: AllowedOriginsError) -> HttpApiError {
    match error {
        AllowedOriginsError::NotFound => not_found(request, "environment was not found"),
        AllowedOriginsError::Forbidden => {
            forbidden(request, "allowed origin operation is forbidden")
        }
        AllowedOriginsError::Conflict => conflict(
            request,
            "allowed origins changed concurrently; read them again",
        ),
        // The message names the rule the origin broke; it is derived from
        // the developer's own input, never from stored data.
        AllowedOriginsError::InvalidOrigin(message) => HttpApiError::new(
            400,
            ErrorCode::InvalidRequest,
            message,
            request.request_id(),
            RetryAdvice::Never,
        ),
        _ => unavailable(request, "allowed origin service is unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_wire_shape_is_exactly_the_allowlist() {
        let wire: AllowedOriginsWire =
            serde_json::from_value(json!({ "allowedOrigins": ["https://app.example.com"] }))
                .expect("origins");
        assert_eq!(wire.allowed_origins, ["https://app.example.com"]);
        assert_eq!(
            serde_json::to_value(AllowedOriginsWire {
                allowed_origins: Vec::new(),
            })
            .expect("json"),
            json!({ "allowedOrigins": [] }),
            "an empty allowlist is present, never omitted"
        );
        assert!(
            serde_json::from_value::<AllowedOriginsWire>(json!({})).is_err(),
            "allowedOrigins is required: a replacement always says what the list is"
        );
        assert!(
            serde_json::from_value::<AllowedOriginsWire>(
                json!({ "allowedOrigins": [], "hostname": "api.example.com" })
            )
            .is_err(),
            "a replacement sets origins and nothing else"
        );
    }

    #[test]
    fn refusals_are_addressed_to_the_developer() {
        let request = HttpRequest::for_test(
            HttpMethod::Put,
            "/v1/projects/prj_example00/environments/env_example00/allowed-origins",
            [],
            Vec::new(),
            None,
        );
        for (error, code) in [
            (AllowedOriginsError::NotFound, ErrorCode::NotFound),
            (AllowedOriginsError::Forbidden, ErrorCode::PermissionDenied),
            (AllowedOriginsError::Conflict, ErrorCode::Conflict),
            (
                AllowedOriginsError::InvalidOrigin("origins must be unique"),
                ErrorCode::InvalidRequest,
            ),
            (AllowedOriginsError::CorruptRecord, ErrorCode::Unavailable),
        ] {
            assert_eq!(origins_error(&request, error).envelope().error.code, code);
        }
    }
}
