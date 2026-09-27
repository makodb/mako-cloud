use std::{collections::BTreeSet, sync::Arc};

use mako_api::TenantScope;
use mako_control_plane::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, DeveloperPrincipal,
    OrganizationRole,
};
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
        conflict, forbidden, invalid, no_payload, no_query, not_found, now_unix_seconds,
        parse_json, require_idempotency, require_json, unavailable, with_developer,
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

/// Invites a user by mail. The invitation carries a link that lets them
/// choose a password, and the link has to land in the app: on the first
/// redirect URL the environment registered, where a magic link lands too. An
/// environment with none is told so before any user is created -- inviting
/// used to create the user and send nothing, leaving them no way in.
fn handle_invite_user(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: CreateUserWire = parse_json(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, _| async move {
        let settings = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::InspectAuthProviders,
            json!({}),
            false,
        )
        .await?;
        let Some(redirect_url) = settings["redirectUrls"]
            .as_array()
            .and_then(|urls| urls.first())
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Err(conflict(
                request,
                "register a redirect URL for this environment before inviting users (Auth providers, Redirect URLs): the invitation links to your app's sign-in page",
            ));
        };
        let value = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::InviteUser,
            json!({
                "email": body.email,
                "trustedMetadata": body.trusted_metadata,
                "profileMetadata": body.profile_metadata,
                "invitationRedirectUrl": redirect_url,
                "inviter": actor.normalized_email(),
            }),
            true,
        )
        .await?;
        public_value(request, 202, value)
    })
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
    let result = graph.data_plane_identity_admin().administer(
        tenant,
        &internal_request_id(request.request_id(), &command),
        &key,
        &command,
    );
    audit_command(
        graph,
        request,
        actor,
        tenant,
        &command,
        result.as_ref().map(|_| ()),
    )
    .await;
    result.map_err(|error| match existing_resource(command.operation, &error) {
        Some(message) => conflict(request, message),
        None => rpc_error(request, error),
    })
}

/// A create refused because what it names is taken, said in the words of
/// that resource. The generic "identity operation conflicts with current
/// state" read like a server fault, and the forms stayed filled, so people
/// kept retrying the same ID.
fn existing_resource(
    operation: IdentityAdminOperation,
    error: &InternalClientError,
) -> Option<&'static str> {
    let InternalClientError::Remote {
        status: 409,
        envelope,
    } = error
    else {
        return None;
    };
    if envelope.error.message != "identity resource already exists" {
        return None;
    }
    match operation {
        IdentityAdminOperation::CreateProjectCredential => {
            Some("a credential with this ID already exists; choose another ID")
        }
        IdentityAdminOperation::RotateProjectCredential => {
            Some("the replacement credential ID is already in use; choose another ID")
        }
        IdentityAdminOperation::CreateUser | IdentityAdminOperation::InviteUser => {
            Some("an application user with this email already exists")
        }
        _ => None,
    }
}

/// The audit action for a change the control plane hands to the data plane,
/// and whether its success is audited here. Collections, indexes and policies
/// are audited by their own services when they are recorded, so only their
/// refusal at install is added; everything else the data plane carries out --
/// keys, signing keys, buckets, sign-in settings, application users -- used to
/// leave no trace in the environment's activity at all.
fn command_action(
    operation: IdentityAdminOperation,
    method: HttpMethod,
) -> Option<(ControlAuditAction, bool)> {
    use ControlAuditAction as A;
    use IdentityAdminOperation as Op;
    Some(match operation {
        Op::CreateProjectCredential => (A::CredentialCreate, true),
        Op::RotateProjectCredential => (A::CredentialRotate, true),
        Op::RetireProjectCredential => (A::CredentialRetire, true),
        Op::InitializeSigningKey => (A::SigningKeyInitialize, true),
        Op::RotateSigningKey => (A::SigningKeyRotate, true),
        Op::InstallAuthProviders => (A::AuthSettingsUpdate, true),
        Op::InstallBucket if method == HttpMethod::Post => (A::StorageBucketCreate, true),
        Op::InstallBucket => (A::StorageBucketUpdate, true),
        Op::RemoveBucket => (A::StorageBucketDelete, true),
        Op::DeleteBucketObject => (A::StorageObjectDelete, true),
        Op::CreateUser => (A::ApplicationUserCreate, true),
        Op::InviteUser => (A::ApplicationUserInvite, true),
        Op::UpdateUserMetadata => (A::ApplicationUserUpdate, true),
        Op::DisableUser => (A::ApplicationUserDisable, true),
        Op::RestoreUser => (A::ApplicationUserRestore, true),
        Op::RevokeSession | Op::RevokeAllSessions => (A::ApplicationUserRevokeSessions, true),
        Op::DeleteUser => (A::ApplicationUserDelete, true),
        Op::InstallCollection => (A::CollectionCreate, false),
        Op::InstallIndex => (A::CollectionIndexCreate, false),
        Op::RemoveIndex => (A::CollectionIndexDelete, false),
        Op::InstallPolicy => (A::PolicyActivate, false),
        _ => return None,
    })
}

/// What a command acted on, from its input, for the audit target.
fn command_target(command: &IdentityAdminCommand) -> String {
    let field = |name: &str| command.input.get(name).and_then(Value::as_str);
    match command.operation {
        IdentityAdminOperation::InstallAuthProviders => "auth-settings".to_owned(),
        IdentityAdminOperation::InitializeSigningKey | IdentityAdminOperation::RotateSigningKey => {
            "signing-keys".to_owned()
        }
        IdentityAdminOperation::InstallIndex | IdentityAdminOperation::RemoveIndex => {
            match (field("collectionId"), field("name")) {
                (Some(collection), Some(name)) => format!("{collection}/{name}"),
                _ => "indexes".to_owned(),
            }
        }
        // A bucket's settings travel whole, under "bucket".
        IdentityAdminOperation::InstallBucket => command
            .input
            .get("bucket")
            .and_then(|bucket| bucket.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("buckets")
            .to_owned(),
        _ => ["bucketId", "credentialId", "id", "userId", "collectionId"]
            .into_iter()
            .find_map(field)
            .unwrap_or("identity")
            .to_owned(),
    }
}

/// Audits a change the data plane carried out, or refused for the caller's
/// role, into the control audit the environment's activity is read from.
pub(crate) async fn audit_command(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    command: &IdentityAdminCommand,
    result: Result<(), &InternalClientError>,
) {
    let Some((action, audit_success)) = command_action(command.operation, request.method()) else {
        return;
    };
    let outcome = match result {
        Ok(()) if audit_success => ControlAuditOutcome::Allowed,
        Err(InternalClientError::Remote { status: 403, .. }) => ControlAuditOutcome::Denied,
        _ => return,
    };
    let Ok(Some(project)) = graph.project_store().get_project(tenant.project_id()).await else {
        return;
    };
    graph.control_audit().record(ControlAuditEvent {
        organization_id: project.organization_id().clone(),
        actor_id: actor.identity_id().clone(),
        action,
        target: format!(
            "{}/{}/{}",
            tenant.project_id().as_str(),
            tenant.environment_id().as_str(),
            command_target(command)
        ),
        outcome,
        at_unix_seconds: now_unix_seconds(request).unwrap_or_default(),
    });
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

/// What a refusal from the data plane means to the person who asked: their
/// team role does not cover the change, and who can make it instead.
pub(crate) const ROLE_REFUSED: &str =
    "your team role does not allow this change; a team administrator or owner can make it";
const NOT_A_MEMBER: &str = "you are not a member of the team that owns this project";

/// What each team role may do in an environment's data plane. It follows the
/// control plane's own role rules: whoever may change a project's data model
/// (`OrganizationRole::can_mutate_projects`, which includes developers) may
/// also install what that change needs -- collections, their indexes and
/// policies, and buckets -- or the control plane would record work the data
/// plane then refuses. Credentials, signing keys and application users stay
/// with administrators and owners; a developer may read, not change, the
/// first two.
pub(crate) fn role_permissions(role: OrganizationRole) -> BTreeSet<IdentityAdminPermission> {
    match role {
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
            IdentityAdminPermission::ReadProjectCredentials,
            IdentityAdminPermission::ReadSigningKeys,
            IdentityAdminPermission::ManageCollections,
            IdentityAdminPermission::ManagePolicies,
            IdentityAdminPermission::ReadBuckets,
            IdentityAdminPermission::ManageBuckets,
        ]),
        OrganizationRole::Viewer => BTreeSet::new(),
    }
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
        .ok_or_else(|| forbidden(request, NOT_A_MEMBER))?;
    Ok(role_permissions(membership.role()))
}

pub(crate) fn rpc_error(request: &HttpRequest, error: InternalClientError) -> HttpApiError {
    match error {
        InternalClientError::Remote { status: 400, .. } => {
            invalid(request, "identity administration request is invalid")
        }
        InternalClientError::Remote { status: 403, .. } => forbidden(request, ROLE_REFUSED),
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

#[cfg(test)]
mod tests {
    use mako_control_plane::OrganizationRole;
    use mako_internal_rpc::IdentityAdminPermission as P;

    use mako_control_plane::ControlAuditAction as A;
    use mako_internal_rpc::IdentityAdminOperation as Op;
    use mako_service_runtime::HttpMethod;

    use mako_internal_rpc::IdentityAdminCommand;

    use super::{command_action, command_target, existing_resource, role_permissions};

    #[test]
    fn a_taken_id_or_email_is_named_as_such() {
        use mako_api::{ApiError, ApiErrorEnvelope, ErrorCode, RetryAdvice};
        use mako_internal_rpc::InternalClientError;

        let refused = |message: &str| InternalClientError::Remote {
            status: 409,
            envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                ErrorCode::Conflict,
                message,
                "req_test",
                RetryAdvice::Never,
            ))),
        };
        let taken = refused("identity resource already exists");
        assert_eq!(
            existing_resource(Op::CreateProjectCredential, &taken),
            Some("a credential with this ID already exists; choose another ID")
        );
        assert_eq!(
            existing_resource(Op::CreateUser, &taken),
            Some("an application user with this email already exists")
        );
        assert_eq!(existing_resource(Op::DisableUser, &taken), None);
        assert_eq!(
            existing_resource(
                Op::CreateProjectCredential,
                &refused("idempotency key conflicts")
            ),
            None
        );
    }

    #[test]
    fn changes_the_data_plane_makes_are_audited_and_reads_are_not() {
        assert_eq!(
            command_action(Op::InstallBucket, HttpMethod::Post),
            Some((A::StorageBucketCreate, true))
        );
        assert_eq!(
            command_action(Op::InstallBucket, HttpMethod::Patch),
            Some((A::StorageBucketUpdate, true))
        );
        assert_eq!(
            command_action(Op::InstallAuthProviders, HttpMethod::Put),
            Some((A::AuthSettingsUpdate, true))
        );
        assert_eq!(
            command_action(Op::InitializeSigningKey, HttpMethod::Post),
            Some((A::SigningKeyInitialize, true))
        );
        assert_eq!(
            command_action(Op::DisableUser, HttpMethod::Post),
            Some((A::ApplicationUserDisable, true))
        );
        // Their own services audit these when recorded; only a refusal is added.
        assert_eq!(
            command_action(Op::InstallCollection, HttpMethod::Post),
            Some((A::CollectionCreate, false))
        );
        let install = |input: serde_json::Value| IdentityAdminCommand {
            operation: Op::InstallBucket,
            actor_id: "dev_abcdefgh".to_owned(),
            permissions: Default::default(),
            input,
        };
        assert_eq!(
            command_target(&install(
                serde_json::json!({"bucket": {"id": "attachments"}})
            )),
            "attachments"
        );
        for read in [
            Op::SearchUsers,
            Op::ListBuckets,
            Op::InspectIndex,
            Op::ListSigningKeys,
        ] {
            assert_eq!(command_action(read, HttpMethod::Get), None, "{read:?}");
        }
    }

    #[test]
    fn a_role_that_may_change_the_data_model_may_install_it() {
        for role in [
            OrganizationRole::Owner,
            OrganizationRole::Administrator,
            OrganizationRole::Developer,
        ] {
            assert!(role.can_mutate_projects());
            let granted = role_permissions(role);
            for needed in [P::ManageCollections, P::ManagePolicies, P::ManageBuckets] {
                assert!(granted.contains(&needed), "{role:?} lacks {needed:?}");
            }
        }
        let developer = role_permissions(OrganizationRole::Developer);
        for withheld in [
            P::ManageProjectCredentials,
            P::ManageSigningKeys,
            P::ManageApplicationUsers,
        ] {
            assert!(
                !developer.contains(&withheld),
                "developer holds {withheld:?}"
            );
        }
        assert!(developer.contains(&P::ReadProjectCredentials));
        assert!(developer.contains(&P::ReadSigningKeys));
        assert!(role_permissions(OrganizationRole::Viewer).is_empty());
    }
}
