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

/// The key one invalidation travels under. The data plane journals one answer
/// per key, so a key that named only the reason and the environment made
/// every later invalidation of that kind a replay of the first: a second
/// policy activation or role change in an environment advanced no epoch at
/// all (explorer grants issued before it kept working), and a role change for
/// a different developer arrived as the same key with a different body, was
/// refused as an idempotency conflict, and failed the request with 503 after
/// the change itself had been saved. The key now names the request as well,
/// and the command, so one request advancing several environments or
/// developers sends distinct commands and a retry of the same request is
/// still the same one.
fn epoch_idempotency_key(
    request_id: &str,
    reason: &str,
    tenant: &TenantScope,
    input: &serde_json::Value,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mako/explorer-epoch/v2");
    for part in [
        request_id,
        reason,
        tenant.project_id().as_str(),
        tenant.environment_id().as_str(),
        &input.to_string(),
    ] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    format!("explorer-epoch-{}", &hasher.finalize().to_hex()[..32])
}

fn advance(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    actor_id: &str,
    reason: &str,
    input: serde_json::Value,
) -> Result<(), HttpApiError> {
    let idempotency = epoch_idempotency_key(request.request_id(), reason, tenant, &input);
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

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId, TenantScope};
    use serde_json::json;

    use super::epoch_idempotency_key;

    #[test]
    fn each_request_and_target_advances_under_its_own_key() {
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").unwrap(),
            EnvironmentId::parse("env_abcdefgh").unwrap(),
        );
        let other = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").unwrap(),
            EnvironmentId::parse("env_zyxwvuts").unwrap(),
        );
        let jordan = json!({"developerIdentityId": "dev_jordan", "allDevelopers": false});
        let devon = json!({"developerIdentityId": "dev_devon", "allDevelopers": false});
        let key = |request: &str, tenant: &TenantScope, input: &serde_json::Value| {
            epoch_idempotency_key(request, "membership-update", tenant, input)
        };
        // A retry of the same request is the same command.
        assert_eq!(
            key("req_1", &tenant, &jordan),
            key("req_1", &tenant, &jordan)
        );
        // A later role change for the same developer is a new invalidation.
        assert_ne!(
            key("req_1", &tenant, &jordan),
            key("req_2", &tenant, &jordan)
        );
        // Another developer, or another environment, is a different command.
        assert_ne!(
            key("req_2", &tenant, &jordan),
            key("req_2", &tenant, &devon)
        );
        assert_ne!(
            key("req_2", &tenant, &jordan),
            key("req_2", &other, &jordan)
        );
        assert_ne!(
            epoch_idempotency_key(
                "req_3",
                "policy-activate",
                &tenant,
                &json!({"allDevelopers": true})
            ),
            epoch_idempotency_key(
                "req_4",
                "policy-activate",
                &tenant,
                &json!({"allDevelopers": true})
            ),
        );
    }
}
