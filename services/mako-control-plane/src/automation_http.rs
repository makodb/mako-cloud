//! Team automation tokens: the routes that issue, list, revoke, and rotate
//! them, and the check that lets a request made with one act.
//!
//! A token acts as the developer who issued it, so it never reaches past that
//! developer's current role, and only within its own scope: the team, project,
//! and environment it names and the permissions it lists. Requests are sorted
//! into the authorizer's actions by method and path. Anything not sorted --
//! managing tokens, members, invitations, sign-in, operator routes, and the
//! data explorer -- refuses a token outright and needs a developer session.

use std::{collections::BTreeSet, num::NonZeroUsize, sync::Arc};

use mako_api::{EnvironmentId, ProjectId};
use mako_control_plane::{
    AutomationPermission, AutomationScope, AutomationTokenId, AutomationTokenIssue,
    AutomationTokenRecord, AutomationTokenSecret, AutomationTokenStatus, ControlAuditAction,
    ControlAuditEvent, ControlAuditOutcome, DeveloperPrincipal, ManagementAccessError,
    ManagementAction, ManagementActor, ManagementAuthorizer, ManagementResourceScope,
    OrganizationId, TokenServiceError,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    ControlPlaneGraph,
    management_http::{
        forbidden, internal, invalid, json as json_response, no_payload, no_query, not_found,
        parse_json, require_idempotency, require_json, unavailable, with_developer,
    },
};

/// The prefix of every automation token secret: `mako_at.<id>.<secret>`.
pub(crate) const TOKEN_PREFIX: &str = "mako_at.";
const LIST_LIMIT: usize = 1_000;
/// How far ahead a token may expire: a year, as a standing credential should.
const MAXIMUM_LIFETIME_SECONDS: u64 = 366 * 24 * 60 * 60;

pub(crate) fn add_automation_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/teams/{teamId}/automation-tokens",
            handle_list as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/teams/{teamId}/automation-tokens",
            handle_create,
        ),
        (
            HttpMethod::Delete,
            "/v1/teams/{teamId}/automation-tokens/{automationTokenId}",
            handle_revoke,
        ),
        (
            HttpMethod::Post,
            "/v1/teams/{teamId}/automation-tokens/{automationTokenId}/actions/rotate",
            handle_rotate,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScopeWire {
    project_id: Option<String>,
    environment_id: Option<String>,
    permissions: BTreeSet<AutomationPermission>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateWire {
    name: String,
    scope: ScopeWire,
    expires_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RotateWire {
    replacement_id: String,
    expires_at: String,
}

fn handle_list(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let team = team_id(request)?;
    with_session(graph, request, |actor, now| async move {
        require_token_manager(graph, request, &actor, &team).await?;
        let records = graph
            .automation_token_service()
            .list(&team, NonZeroUsize::new(LIST_LIMIT).expect("list limit"))
            .await
            .map_err(|error| token_error(request, error))?;
        let items = records
            .iter()
            .map(|record| token_view(request, record, now))
            .collect::<Result<Vec<_>, _>>()?;
        json_response(request, 200, &json!({ "items": items }))
    })
}

fn handle_create(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let team = team_id(request)?;
    let body: CreateWire = parse_json(request)?;
    with_session(graph, request, |actor, now| async move {
        require_token_manager(graph, request, &actor, &team).await?;
        let scope = scope_from(request, &team, body.scope)?;
        validate_scope_resource(graph, request, &scope).await?;
        let expires_at = expiry(request, &body.expires_at, now)?;
        let existing = graph
            .automation_token_service()
            .list(&team, NonZeroUsize::new(LIST_LIMIT).expect("list limit"))
            .await
            .map_err(|error| token_error(request, error))?;
        if name_in_use(&existing, &body.name, now) {
            return Err(crate::management_http::conflict(request, NAME_IN_USE));
        }
        let issued = graph
            .automation_token_service()
            .issue(
                new_token_id(request)?,
                body.name,
                scope,
                actor.identity_id().clone(),
                now,
                expires_at,
            )
            .await
            .map_err(|error| token_error(request, error))?;
        audit_token(
            graph,
            &actor,
            &team,
            ControlAuditAction::AutomationTokenCreate,
            issued.record.id().as_str(),
            now,
        );
        issue_response(request, &issued, now)
    })
}

const NAME_IN_USE: &str =
    "an active automation token already has this name; rotate or revoke it, or choose another name";

/// Whether a live token in the team already carries this name. Names are how
/// people tell tokens apart in the console and the CLI; a create repeated by a
/// second click used to mint another live token under the same name, whose
/// one-time secret was replaced on screen before anyone could keep it.
fn name_in_use(tokens: &[AutomationTokenRecord], name: &str, now: u64) -> bool {
    tokens.iter().any(|token| {
        token.name() == name
            && token.status() == AutomationTokenStatus::Active
            && now < token.expires_at_unix_seconds()
    })
}

/// Token changes are audited to the team, so its activity shows who issued,
/// rotated, or revoked a credential that acts for the team.
fn audit_token(
    graph: &ControlPlaneGraph,
    actor: &DeveloperPrincipal,
    team: &OrganizationId,
    action: ControlAuditAction,
    token: &str,
    now: u64,
) {
    graph.control_audit().record(ControlAuditEvent {
        organization_id: team.clone(),
        actor_id: actor.identity_id().clone(),
        action,
        target: format!("{}/automation-tokens/{token}", team.as_str()),
        outcome: ControlAuditOutcome::Allowed,
        at_unix_seconds: now,
    });
}

fn handle_revoke(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let team = team_id(request)?;
    let token = token_id(request)?;
    with_session(graph, request, |actor, now| async move {
        require_token_manager(graph, request, &actor, &team).await?;
        team_token(graph, request, &team, &token).await?;
        graph
            .automation_token_service()
            .revoke(&token, now)
            .await
            .map_err(|error| token_error(request, error))?;
        audit_token(
            graph,
            &actor,
            &team,
            ControlAuditAction::AutomationTokenRevoke,
            token.as_str(),
            now,
        );
        Ok(HttpResponse::empty(204))
    })
}

fn handle_rotate(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let team = team_id(request)?;
    let token = token_id(request)?;
    let body: RotateWire = parse_json(request)?;
    let replacement = AutomationTokenId::parse(body.replacement_id)
        .map_err(|_| invalid(request, "replacement token id is invalid"))?;
    with_session(graph, request, |actor, now| async move {
        require_token_manager(graph, request, &actor, &team).await?;
        team_token(graph, request, &team, &token).await?;
        let expires_at = expiry(request, &body.expires_at, now)?;
        let issued = graph
            .automation_token_service()
            .rotate(&token, replacement, now, expires_at)
            .await
            .map_err(|error| token_error(request, error))?;
        audit_token(
            graph,
            &actor,
            &team,
            ControlAuditAction::AutomationTokenRotate,
            token.as_str(),
            now,
        );
        issue_response(request, &issued, now)
    })
}

/// Token management needs a developer session: a token may not mint or
/// retire tokens, so a leaked one cannot keep itself alive.
fn with_session<F, Fut>(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: F,
) -> Result<HttpResponse, HttpApiError>
where
    F: FnOnce(DeveloperPrincipal, u64) -> Fut,
    Fut: Future<Output = Result<HttpResponse, HttpApiError>>,
{
    if is_automation_token(request) {
        return Err(forbidden(
            request,
            "automation tokens cannot manage automation tokens",
        ));
    }
    with_developer(graph, request, operation)
}

async fn require_token_manager(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    team: &OrganizationId,
) -> Result<(), HttpApiError> {
    let membership = graph
        .organization_store()
        .get_membership(team, actor.identity_id())
        .await
        .map_err(|_| unavailable(request, "team membership is unavailable"))?
        .ok_or_else(|| not_found(request, "team was not found"))?;
    if !membership.role().can_manage_members() {
        return Err(forbidden(
            request,
            "only team administrators and owners manage automation tokens",
        ));
    }
    Ok(())
}

async fn team_token(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    team: &OrganizationId,
    token: &AutomationTokenId,
) -> Result<AutomationTokenRecord, HttpApiError> {
    graph
        .automation_token_service()
        .get_token(token)
        .await
        .map_err(|error| token_error(request, error))?
        .filter(|record| record.scope().organization_id() == team)
        .ok_or_else(|| not_found(request, "automation token was not found"))
}

fn scope_from(
    request: &HttpRequest,
    team: &OrganizationId,
    wire: ScopeWire,
) -> Result<AutomationScope, HttpApiError> {
    let project = wire
        .project_id
        .map(ProjectId::parse)
        .transpose()
        .map_err(|_| invalid(request, "scope project id is invalid"))?;
    let environment = wire
        .environment_id
        .map(EnvironmentId::parse)
        .transpose()
        .map_err(|_| invalid(request, "scope environment id is invalid"))?;
    AutomationScope::new(team.clone(), project, environment, wire.permissions)
        .map_err(|_| invalid(request, "automation scope is invalid"))
}

/// The project and environment a scope names must exist and belong to the team.
async fn validate_scope_resource(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    scope: &AutomationScope,
) -> Result<(), HttpApiError> {
    let Some(project_id) = scope.project_id() else {
        return Ok(());
    };
    let project = graph
        .project_store()
        .get_project(project_id)
        .await
        .map_err(|_| unavailable(request, "project is unavailable"))?
        .ok_or_else(|| invalid(request, "scope project was not found in this team"))?;
    if project.organization_id() != scope.organization_id() {
        return Err(invalid(request, "scope project was not found in this team"));
    }
    if let Some(environment_id) = scope.environment_id() {
        graph
            .project_store()
            .get_environment(project_id, environment_id)
            .await
            .map_err(|_| unavailable(request, "environment is unavailable"))?
            .ok_or_else(|| invalid(request, "scope environment was not found in this project"))?;
    }
    Ok(())
}

fn expiry(request: &HttpRequest, value: &str, now: u64) -> Result<u64, HttpApiError> {
    let expires_at = OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .and_then(|time| u64::try_from(time.unix_timestamp()).ok())
        .ok_or_else(|| invalid(request, "expiry is invalid"))?;
    if expires_at <= now || expires_at - now > MAXIMUM_LIFETIME_SECONDS {
        return Err(invalid(
            request,
            "expiry must be in the future and within a year",
        ));
    }
    Ok(expires_at)
}

fn new_token_id(request: &HttpRequest) -> Result<AutomationTokenId, HttpApiError> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut random = [0_u8; 24];
    OsRng.fill_bytes(&mut random);
    let suffix = random
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect::<String>();
    AutomationTokenId::parse(format!("atm_{suffix}"))
        .map_err(|_| internal(request, "automation token id could not be generated"))
}

fn issue_response(
    request: &HttpRequest,
    issued: &AutomationTokenIssue,
    now: u64,
) -> Result<HttpResponse, HttpApiError> {
    json_response(
        request,
        201,
        &json!({
            "token": token_view(request, &issued.record, now)?,
            "secret": issued.secret.expose_once(),
        }),
    )
}

/// A token as the `AutomationToken` schema describes it: never its digest.
fn token_view(
    request: &HttpRequest,
    record: &AutomationTokenRecord,
    now: u64,
) -> Result<Value, HttpApiError> {
    let scope = record.scope();
    let mut scope_view = json!({ "permissions": scope.permissions() });
    if let Some(project) = scope.project_id() {
        scope_view["projectId"] = json!(project.as_str());
    }
    if let Some(environment) = scope.environment_id() {
        scope_view["environmentId"] = json!(environment.as_str());
    }
    let status = match record.status() {
        AutomationTokenStatus::Active if now >= record.expires_at_unix_seconds() => "expired",
        AutomationTokenStatus::Active => "active",
        AutomationTokenStatus::Revoked => "revoked",
        AutomationTokenStatus::Expired => "expired",
    };
    let mut view = json!({
        "id": record.id().as_str(),
        "teamId": scope.organization_id().as_str(),
        "name": record.name(),
        "scope": scope_view,
        "status": status,
        "expiresAt": rfc3339(request, record.expires_at_unix_seconds())?,
        "createdAt": rfc3339(request, record.created_at_unix_seconds())?,
    });
    if let Some(revoked) = record.revoked_at_unix_seconds() {
        view["revokedAt"] = json!(rfc3339(request, revoked)?);
    }
    Ok(view)
}

fn rfc3339(request: &HttpRequest, unix_seconds: u64) -> Result<String, HttpApiError> {
    i64::try_from(unix_seconds)
        .ok()
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .and_then(|time| time.format(&Rfc3339).ok())
        .ok_or_else(|| internal(request, "timestamp could not be formatted"))
}

fn team_id(request: &HttpRequest) -> Result<OrganizationId, HttpApiError> {
    OrganizationId::parse(request.path_parameter("teamId").unwrap_or_default())
        .map_err(|_| invalid(request, "team path is invalid"))
}

fn token_id(request: &HttpRequest) -> Result<AutomationTokenId, HttpApiError> {
    AutomationTokenId::parse(
        request
            .path_parameter("automationTokenId")
            .unwrap_or_default(),
    )
    .map_err(|_| invalid(request, "automation token path is invalid"))
}

fn token_error(request: &HttpRequest, error: TokenServiceError) -> HttpApiError {
    match error {
        TokenServiceError::NotFound => not_found(request, "automation token was not found"),
        TokenServiceError::InvalidToken | TokenServiceError::InvalidScope => {
            invalid(request, "automation token request is invalid")
        }
        TokenServiceError::Conflict => crate::management_http::conflict(
            request,
            "automation token changed or is no longer active",
        ),
        _ => unavailable(request, "automation tokens are unavailable"),
    }
}

// ---- acting with a token --------------------------------------------------

pub(crate) fn is_automation_token(request: &HttpRequest) -> bool {
    request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token.starts_with(TOKEN_PREFIX))
}

/// The principal a request made with an automation token acts as, once the
/// token is valid, the request is one tokens may make, and it lies within the
/// token's scope and permissions.
pub(crate) async fn automation_actor(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    now: u64,
) -> Result<DeveloperPrincipal, HttpApiError> {
    let secret = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .and_then(|token| AutomationTokenSecret::parse(token).ok())
        .ok_or_else(|| unauthenticated(request))?;
    let principal = graph
        .automation_token_service()
        .authenticate(&secret, now)
        .await
        .map_err(|error| match error {
            TokenServiceError::InvalidToken | TokenServiceError::NotFound => {
                unauthenticated(request)
            }
            _ => unavailable(request, "automation tokens are unavailable"),
        })?;
    let (action, resource) = match request_action(request) {
        Some((action, RequestResource::Team(team))) => (
            action,
            ManagementResourceScope {
                organization_id: team,
                project_id: None,
                environment_id: None,
            },
        ),
        Some((action, RequestResource::Project(project, environment))) => {
            let record = graph
                .project_store()
                .get_project(&project)
                .await
                .map_err(|_| unavailable(request, "project is unavailable"))?
                .ok_or_else(|| not_found(request, "project was not found"))?;
            (
                action,
                ManagementResourceScope {
                    organization_id: record.organization_id().clone(),
                    project_id: Some(project),
                    environment_id: environment,
                },
            )
        }
        None => {
            return Err(forbidden(
                request,
                "automation tokens cannot make this request",
            ));
        }
    };
    ManagementAuthorizer::new(
        graph.organization_store().clone(),
        graph.project_store().clone(),
    )
    .authorize(ManagementActor::Automation(&principal), action, &resource)
    .await
    .map_err(|error| match error {
        ManagementAccessError::NotFound => not_found(request, "resource was not found"),
        ManagementAccessError::Forbidden | ManagementAccessError::ScopeMismatch => forbidden(
            request,
            "automation token scope does not allow this request",
        ),
        _ => unavailable(request, "management authorization is unavailable"),
    })?;
    graph
        .developer_authenticator()
        .authenticate_automation(principal.created_by(), principal.token_id().as_str(), now)
        .await
        .map_err(|_| unauthenticated(request))
}

fn unauthenticated(request: &HttpRequest) -> HttpApiError {
    HttpApiError::new(
        401,
        mako_api::ErrorCode::Unauthenticated,
        "developer authorization is invalid",
        request.request_id(),
        mako_api::RetryAdvice::Never,
    )
}

#[derive(Debug, Eq, PartialEq)]
enum RequestResource {
    Team(OrganizationId),
    Project(ProjectId, Option<EnvironmentId>),
}

/// Which authorizer action a request is, and on what, or `None` when tokens
/// may not make it at all.
fn request_action(request: &HttpRequest) -> Option<(ManagementAction, RequestResource)> {
    classify(request.method(), request.path())
}

fn classify(method: HttpMethod, path: &str) -> Option<(ManagementAction, RequestResource)> {
    let read = matches!(method, HttpMethod::Get | HttpMethod::Head);
    let segments = path.strip_prefix("/v1/")?.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        ["teams", team, rest @ ..] => {
            let team = OrganizationId::parse(*team).ok()?;
            // A team's activity is its audit trail, read as an environment's is.
            if read && rest == ["activity"] {
                return Some((ManagementAction::AuditRead, RequestResource::Team(team)));
            }
            let delegated = !matches!(
                rest.first(),
                Some(&("automation-tokens" | "members" | "invitations"))
            );
            (read && delegated).then_some((
                ManagementAction::OrganizationRead,
                RequestResource::Team(team),
            ))
        }
        ["projects", project, "environments", environment, rest @ ..] => {
            let project = ProjectId::parse(*project).ok()?;
            let environment = EnvironmentId::parse(*environment).ok()?;
            let action = match (read, rest) {
                // Direct document access has its own grants and audit.
                (_, ["explorer", ..]) => return None,
                (true, ["observability", "audit-events", ..] | ["activity", ..]) => {
                    ManagementAction::AuditRead
                }
                (true, _) => ManagementAction::EnvironmentRead,
                (false, ["policies", ..] | ["collections", _, "policies", ..]) => {
                    ManagementAction::PolicyWrite
                }
                (false, ["collections", ..]) => ManagementAction::CollectionWrite,
                (false, ["functions" | "function-bundles" | "function-secrets", ..]) => {
                    ManagementAction::FunctionDeploy
                }
                (false, _) => ManagementAction::EnvironmentWrite,
            };
            Some((action, RequestResource::Project(project, Some(environment))))
        }
        ["projects", project, rest @ ..] => {
            let project = ProjectId::parse(*project).ok()?;
            if rest.iter().any(|segment| segment.contains("transfer")) {
                return None;
            }
            let action = if read {
                ManagementAction::ProjectRead
            } else {
                ManagementAction::ProjectWrite
            };
            Some((action, RequestResource::Project(project, None)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId};
    use mako_control_plane::{ManagementAction, OrganizationId};
    use mako_service_runtime::HttpMethod;

    use super::{RequestResource, classify};

    const ENV: &str = "/v1/projects/prj_abcdefgh/environments/env_abcdefgh";

    fn environment() -> RequestResource {
        RequestResource::Project(
            ProjectId::parse("prj_abcdefgh").unwrap(),
            Some(EnvironmentId::parse("env_abcdefgh").unwrap()),
        )
    }

    #[test]
    fn a_live_token_name_cannot_be_issued_twice() {
        use std::{num::NonZeroUsize, sync::Arc};

        use mako_control_plane::{
            AutomationPermission, AutomationScope, AutomationTokenId, AutomationTokenService,
            DeveloperIdentityId,
        };
        use mako_storage::{Durability, MemoryAdapter};

        use super::name_in_use;

        futures::executor::block_on(async {
            let service =
                AutomationTokenService::new(Arc::new(MemoryAdapter::new()), Durability::Memory)
                    .unwrap();
            let team = OrganizationId::parse("org_abcdefgh").unwrap();
            let scope = AutomationScope::new(
                team.clone(),
                None,
                None,
                [AutomationPermission::ProjectRead],
            )
            .unwrap();
            let issue = |id: &str, expires| {
                service.issue(
                    AutomationTokenId::parse(id).unwrap(),
                    "ci-deploy",
                    scope.clone(),
                    DeveloperIdentityId::parse("dev_abcdefgh").unwrap(),
                    10,
                    expires,
                )
            };
            let listed = || service.list(&team, NonZeroUsize::new(50).unwrap());
            assert!(!name_in_use(&listed().await.unwrap(), "ci-deploy", 20));
            let first = issue("atm_first0001", 1_000).await.unwrap();
            let tokens = listed().await.unwrap();
            assert!(name_in_use(&tokens, "ci-deploy", 20));
            assert!(
                !name_in_use(&tokens, "ci-deploy-2", 20),
                "another name is free"
            );
            assert!(
                !name_in_use(&tokens, "ci-deploy", 1_000),
                "an expired token frees its name"
            );
            service.revoke(first.record.id(), 30).await.unwrap();
            assert!(
                !name_in_use(&listed().await.unwrap(), "ci-deploy", 40),
                "so does revoking it"
            );
        });
    }

    #[test]
    fn requests_are_sorted_into_the_permission_they_need() {
        for (method, path, action) in [
            (
                HttpMethod::Get,
                format!("{ENV}/collections"),
                ManagementAction::EnvironmentRead,
            ),
            (
                HttpMethod::Post,
                format!("{ENV}/collections"),
                ManagementAction::CollectionWrite,
            ),
            (
                HttpMethod::Put,
                format!("{ENV}/collections/todos/policies/draft"),
                ManagementAction::PolicyWrite,
            ),
            (
                HttpMethod::Post,
                format!("{ENV}/functions"),
                ManagementAction::FunctionDeploy,
            ),
            (
                HttpMethod::Post,
                format!("{ENV}/function-bundles"),
                ManagementAction::FunctionDeploy,
            ),
            (
                HttpMethod::Post,
                format!("{ENV}/webhooks"),
                ManagementAction::EnvironmentWrite,
            ),
            (
                HttpMethod::Get,
                format!("{ENV}/observability/audit-events"),
                ManagementAction::AuditRead,
            ),
        ] {
            assert_eq!(
                classify(method, &path),
                Some((action, environment())),
                "{method:?} {path}"
            );
        }
        assert_eq!(
            classify(HttpMethod::Get, "/v1/projects/prj_abcdefgh"),
            Some((
                ManagementAction::ProjectRead,
                RequestResource::Project(ProjectId::parse("prj_abcdefgh").unwrap(), None)
            ))
        );
        assert_eq!(
            classify(HttpMethod::Get, "/v1/teams/org_abcdefgh"),
            Some((
                ManagementAction::OrganizationRead,
                RequestResource::Team(OrganizationId::parse("org_abcdefgh").unwrap())
            ))
        );
        assert_eq!(
            classify(HttpMethod::Get, "/v1/teams/org_abcdefgh/activity"),
            Some((
                ManagementAction::AuditRead,
                RequestResource::Team(OrganizationId::parse("org_abcdefgh").unwrap())
            ))
        );
    }

    #[test]
    fn tokens_cannot_manage_tokens_members_sign_in_or_browse_documents() {
        for (method, path) in [
            (
                HttpMethod::Get,
                "/v1/teams/org_abcdefgh/automation-tokens".to_owned(),
            ),
            (
                HttpMethod::Post,
                "/v1/teams/org_abcdefgh/automation-tokens".to_owned(),
            ),
            (HttpMethod::Get, "/v1/teams/org_abcdefgh/members".to_owned()),
            (
                HttpMethod::Post,
                "/v1/teams/org_abcdefgh/invitations".to_owned(),
            ),
            (HttpMethod::Patch, "/v1/teams/org_abcdefgh".to_owned()),
            (HttpMethod::Post, "/v1/developer-auth/sessions".to_owned()),
            (HttpMethod::Get, "/v1/operator/tenants".to_owned()),
            (HttpMethod::Post, format!("{ENV}/explorer/grants")),
            (
                HttpMethod::Post,
                "/v1/projects/prj_abcdefgh/actions/transfer-ownership".to_owned(),
            ),
            (HttpMethod::Get, "/v1/projects".to_owned()),
        ] {
            assert_eq!(classify(method, &path), None, "{method:?} {path}");
        }
    }
}
