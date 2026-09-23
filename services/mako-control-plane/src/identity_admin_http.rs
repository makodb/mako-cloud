use std::{collections::BTreeSet, sync::Arc};

use mako_api::TenantScope;
use mako_control_plane::{DeveloperPrincipal, OrganizationRole};
use mako_internal_rpc::{
    IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission, InternalClientError,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    ControlPlaneGraph,
    http_support::{public_value, query_value, reject_unknown_query, tenant},
    management_http::{
        conflict, forbidden, invalid, no_payload, no_query, not_found, parse_json,
        require_idempotency, require_json, unavailable, with_developer,
    },
};

pub(crate) fn add_identity_admin_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/users",
            handle_search_users as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/users",
            handle_create_user,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/users/invitations",
            handle_invite_user,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}",
            handle_get_user,
        ),
        (
            HttpMethod::Patch,
            "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}",
            handle_update_user,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}",
            handle_delete_user,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}/actions/disable",
            handle_disable_user,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}/actions/restore",
            handle_restore_user,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}/actions/revoke-sessions",
            handle_revoke_sessions,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}/sessions/{sessionId}",
            handle_revoke_session,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/credentials/public",
            handle_create_public_credential,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/credentials/service",
            handle_create_service_credential,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/credentials/{credentialId}",
            handle_get_credential,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/credentials/{credentialId}",
            handle_retire_credential,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/credentials/{credentialId}/actions/rotate",
            handle_rotate_credential,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/signing-keys",
            handle_list_signing_keys,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/signing-keys",
            handle_rotate_signing_key,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/signing-keys/actions/initialize",
            handle_initialize_signing_key,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_search_users(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    if !request.body().is_empty() {
        return Err(invalid(request, "request body is not supported"));
    }
    reject_unknown_query(request, &["query", "limit"])?;
    let query = query_value(request, "query")?.map(str::to_owned);
    let limit = match query_value(request, "limit")? {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| invalid(request, "user search limit is invalid"))?,
        None => 50,
    };
    if !(1..=100).contains(&limit) || query.as_ref().is_some_and(|value| value.len() > 320) {
        return Err(invalid(request, "user search query is invalid"));
    }
    identity_command(
        graph,
        request,
        IdentityAdminOperation::SearchUsers,
        json!({"query": query, "limit": limit}),
        200,
        false,
    )
}

fn handle_create_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    user_create(graph, request, IdentityAdminOperation::CreateUser, 201)
}

fn handle_invite_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    user_create(graph, request, IdentityAdminOperation::InviteUser, 202)
}

fn user_create(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: IdentityAdminOperation,
    status: u16,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: CreateUserWire = parse_json(request)?;
    identity_command(
        graph,
        request,
        operation,
        json!({
            "email": body.email,
            "trustedMetadata": body.trusted_metadata,
            "profileMetadata": body.profile_metadata,
        }),
        status,
        true,
    )
}

fn handle_get_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    simple_user(graph, request, IdentityAdminOperation::InspectUser, 200)
}

fn handle_delete_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    simple_user(graph, request, IdentityAdminOperation::DeleteUser, 200)
}

fn handle_disable_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    simple_user(graph, request, IdentityAdminOperation::DisableUser, 200)
}

fn handle_restore_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    simple_user(graph, request, IdentityAdminOperation::RestoreUser, 200)
}

fn handle_revoke_sessions(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    simple_user(
        graph,
        request,
        IdentityAdminOperation::RevokeAllSessions,
        200,
    )
}

fn simple_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: IdentityAdminOperation,
    status: u16,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let user_id = path(request, "userId", "application user path is invalid")?;
    identity_command(
        graph,
        request,
        operation,
        json!({"userId": user_id}),
        status,
        false,
    )
}

fn handle_update_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let body: UpdateUserWire = parse_json(request)?;
    let user_id = path(request, "userId", "application user path is invalid")?;
    identity_command(
        graph,
        request,
        IdentityAdminOperation::UpdateUserMetadata,
        json!({
            "userId": user_id,
            "trustedMetadata": body.trusted_metadata,
            "profileMetadata": body.profile_metadata,
        }),
        200,
        false,
    )
}

fn handle_revoke_session(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let user_id = path(request, "userId", "application user path is invalid")?;
    let session_id = path(request, "sessionId", "application session path is invalid")?;
    identity_command(
        graph,
        request,
        IdentityAdminOperation::RevokeSession,
        json!({"userId": user_id, "sessionId": session_id}),
        200,
        false,
    )
}

fn handle_create_public_credential(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: CredentialIdWire = parse_json(request)?;
    identity_command(
        graph,
        request,
        IdentityAdminOperation::CreateProjectCredential,
        json!({"id": body.id, "kind": "public"}),
        201,
        true,
    )
}

fn handle_create_service_credential(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: ServiceCredentialWire = parse_json(request)?;
    identity_command(
        graph,
        request,
        IdentityAdminOperation::CreateProjectCredential,
        json!({"id": body.id, "kind": "service", "serviceScope": body.scope}),
        201,
        true,
    )
}

fn handle_get_credential(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    credential_id_command(
        graph,
        request,
        IdentityAdminOperation::InspectProjectCredential,
        200,
    )
}

fn handle_retire_credential(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    credential_id_command(
        graph,
        request,
        IdentityAdminOperation::RetireProjectCredential,
        204,
    )
}

fn credential_id_command(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: IdentityAdminOperation,
    status: u16,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let id = path(request, "credentialId", "credential path is invalid")?;
    if status == 204 {
        identity_command_no_content(graph, request, operation, json!({"id": id}))
    } else {
        identity_command(graph, request, operation, json!({"id": id}), status, false)
    }
}

fn handle_rotate_credential(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: RotateCredentialWire = parse_json(request)?;
    if body.overlap_seconds > 2_592_000 {
        return Err(invalid(request, "credential overlap is invalid"));
    }
    let current_id = path(request, "credentialId", "credential path is invalid")?;
    identity_command(
        graph,
        request,
        IdentityAdminOperation::RotateProjectCredential,
        json!({
            "currentId": current_id,
            "replacementId": body.replacement_id,
            "overlapSeconds": body.overlap_seconds,
        }),
        201,
        true,
    )
}

fn handle_list_signing_keys(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, _| async move {
        let mut value = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::ListSigningKeys,
            json!({}),
            false,
        )
        .await?;
        if let Value::Object(object) = &mut value
            && let Some(keys) = object.remove("keys")
        {
            object.insert("items".to_owned(), keys);
        }
        public_value(request, 200, value)
    })
}

/// Create an environment's first signing key.
///
/// Rotation cannot stand in for this: it replaces an existing active key and
/// fails when there is none, so without this route a freshly created
/// environment can never issue an application-user session.
fn handle_initialize_signing_key(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    no_payload(request)?;
    require_idempotency(request)?;
    identity_command(
        graph,
        request,
        IdentityAdminOperation::InitializeSigningKey,
        json!({}),
        201,
        true,
    )
}

fn handle_rotate_signing_key(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: RotateSigningKeyWire = parse_json(request)?;
    if !(1..=2_592_000).contains(&body.overlap_seconds) {
        return Err(invalid(request, "signing-key overlap is invalid"));
    }
    identity_command(
        graph,
        request,
        IdentityAdminOperation::RotateSigningKey,
        json!({"overlapSeconds": body.overlap_seconds}),
        201,
        true,
    )
}

fn identity_command(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: IdentityAdminOperation,
    input: Value,
    status: u16,
    idempotent: bool,
) -> Result<HttpResponse, HttpApiError> {
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, _| async move {
        let value = administer(
            graph, request, &actor, &tenant, operation, input, idempotent,
        )
        .await?;
        public_value(request, status, value)
    })
}

fn identity_command_no_content(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: IdentityAdminOperation,
    input: Value,
) -> Result<HttpResponse, HttpApiError> {
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, _| async move {
        let _: Value = administer(graph, request, &actor, &tenant, operation, input, false).await?;
        Ok(HttpResponse::empty(204))
    })
}

/// Install the quota policy a tenant is held to, as a named authority.
///
/// A developer transferring a project and an operator changing a plan end
/// here; a new environment gets its limits from the provisioning worker when
/// it activates, through the same command. Who is acting differs, so the
/// actor is explicit rather than derived from a developer session.
pub(crate) async fn install_quota_policy(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor_id: &str,
    tenant: &TenantScope,
    policy: &serde_json::Value,
) -> Result<(), HttpApiError> {
    let command = crate::graph::quota_policy_command(actor_id, policy);
    let _: Value = graph
        .data_plane_identity_admin()
        .administer(tenant, request.request_id(), request.request_id(), &command)
        .map_err(|error| rpc_error(request, error))?;
    Ok(())
}

pub(crate) async fn administer(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    operation: IdentityAdminOperation,
    input: Value,
    idempotent: bool,
) -> Result<Value, HttpApiError> {
    let permissions = identity_permissions(graph, request, actor, tenant).await?;
    let command = IdentityAdminCommand {
        operation,
        actor_id: actor.identity_id().as_str().to_owned(),
        permissions,
        input,
    };
    // The caller's key when there is one, so a client's retry is still the
    // same request; otherwise this request's own id.
    let caller_key = if idempotent {
        require_idempotency(request)?
    } else {
        request.request_id()
    };
    let key = internal_key(caller_key, &command);
    graph
        .data_plane_identity_admin()
        .administer(
            tenant,
            &internal_request_id(request.request_id(), &command),
            &key,
            &command,
        )
        .map_err(|error| rpc_error(request, error))
}

/// The idempotency key one identity-admin command travels under.
///
/// The data plane journals a response per `(caller, idempotency key)` so that
/// a retry returns the first answer rather than acting twice. One HTTP request
/// can issue several commands, though -- listing indexes inspects every index
/// it lists -- and they all carried the same key, so the second command looked
/// like the first one being replayed with a different body. The journal
/// refused it, correctly, and listing indexes failed with a conflict for any
/// collection holding more than one: the console's index view, `mako-cloud indexes
/// list`, and every setup script. Deriving the key from the caller's key *and
/// the command* keeps both properties -- distinct commands are distinct
/// requests, and a retry of the same command under the same caller key is
/// still the same request.
fn internal_key(caller: &str, command: &IdentityAdminCommand) -> String {
    format!("idem_{}", &digest(caller, command)[..32])
}

/// The request id that command travels under, distinct for the same reason.
fn internal_request_id(caller: &str, command: &IdentityAdminCommand) -> String {
    format!("req_{}", &digest(caller, command)[..32])
}

fn digest(caller: &str, command: &IdentityAdminCommand) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mako/identity-admin/command/v1");
    hasher.update(caller.as_bytes());
    hasher.update(&[0]);
    hasher.update(format!("{:?}", command.operation).as_bytes());
    hasher.update(&[0]);
    hasher.update(command.actor_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(command.input.to_string().as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(crate) async fn identity_permissions(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
) -> Result<BTreeSet<IdentityAdminPermission>, HttpApiError> {
    let project = graph
        .project_store()
        .get_project(tenant.project_id())
        .await
        .map_err(|_| unavailable(request, "project authorization is unavailable"))?
        .ok_or_else(|| not_found(request, "project resource was not found"))?;
    graph
        .project_store()
        .get_environment(tenant.project_id(), tenant.environment_id())
        .await
        .map_err(|_| unavailable(request, "environment authorization is unavailable"))?
        .ok_or_else(|| not_found(request, "environment resource was not found"))?;
    let membership = graph
        .organization_store()
        .get_membership(project.organization_id(), actor.identity_id())
        .await
        .map_err(|_| unavailable(request, "organization authorization is unavailable"))?
        .ok_or_else(|| forbidden(request, "identity administration is forbidden"))?;
    let permissions = match membership.role() {
        OrganizationRole::Owner | OrganizationRole::Administrator => BTreeSet::from([
            IdentityAdminPermission::ReadApplicationUsers,
            IdentityAdminPermission::ManageApplicationUsers,
            IdentityAdminPermission::ReadProjectCredentials,
            IdentityAdminPermission::ManageProjectCredentials,
            IdentityAdminPermission::ReadSigningKeys,
            IdentityAdminPermission::ManageSigningKeys,
            IdentityAdminPermission::ManageCollections,
            IdentityAdminPermission::ManagePolicies,
            IdentityAdminPermission::ReadBuckets,
            IdentityAdminPermission::ManageBuckets,
        ]),
        OrganizationRole::Developer => BTreeSet::from([
            IdentityAdminPermission::ReadApplicationUsers,
            IdentityAdminPermission::ReadBuckets,
        ]),
        OrganizationRole::Viewer => BTreeSet::new(),
    };
    Ok(permissions)
}

pub(crate) fn rpc_error(request: &HttpRequest, error: InternalClientError) -> HttpApiError {
    match error {
        InternalClientError::Remote { status: 400, .. } => {
            invalid(request, "identity administration request is invalid")
        }
        InternalClientError::Remote { status: 403, .. } => {
            forbidden(request, "identity administration is forbidden")
        }
        InternalClientError::Remote { status: 404, .. } => {
            not_found(request, "identity resource was not found")
        }
        InternalClientError::Remote { status: 409, .. } => {
            conflict(request, "identity operation conflicts with current state")
        }
        _ => unavailable(request, "data-plane identity authority is unavailable"),
    }
}

fn path(request: &HttpRequest, name: &str, message: &'static str) -> Result<String, HttpApiError> {
    let value = request.path_parameter(name).unwrap_or_default();
    if value.is_empty() || value.len() > 320 || value.chars().any(char::is_control) {
        Err(invalid(request, message))
    } else {
        Ok(value.to_owned())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateUserWire {
    email: String,
    trusted_metadata: Value,
    profile_metadata: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateUserWire {
    trusted_metadata: Value,
    profile_metadata: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialIdWire {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceCredentialWire {
    id: String,
    scope: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RotateCredentialWire {
    replacement_id: String,
    overlap_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RotateSigningKeyWire {
    overlap_seconds: u64,
}
