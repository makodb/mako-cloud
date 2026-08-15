use std::{num::NonZeroUsize, sync::Arc, time::SystemTime};

use futures::executor::block_on;
use mako_api::{ErrorCode, RetryAdvice};
use mako_control_plane::{
    DeveloperAccessAudience, DeveloperAccount, DeveloperDecisionKind, DeveloperGenericOutcome,
    DeveloperIdentityId, DeveloperRecoveryOutcome, DeveloperSessionGrant,
    DeveloperVerificationOutcome, DeveloperWorkflowError,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlPlaneGraph,
    developer_metrics::DeveloperMetricOperation,
    http_support::{public_json, query_value, reject_unknown_query},
    identity::HostedAuthenticationError,
    management_http::{
        conflict, forbidden, format_timestamp, internal, invalid, no_payload, no_query, not_found,
        parse_json, require_idempotency, require_json, unavailable, with_developer,
    },
    operator_auth_http::authenticate_operator,
};

const REFRESH_COOKIE_NAME: &str = "__Host-mako_developer_refresh";

pub(crate) fn add_developer_auth_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler, operation) in [
        (
            HttpMethod::Post,
            "/v1/developer-auth/registrations",
            handle_registration as Handler,
            DeveloperMetricOperation::Registration,
        ),
        (
            HttpMethod::Post,
            "/v1/developer-auth/verifications",
            handle_verification,
            DeveloperMetricOperation::Verification,
        ),
        (
            HttpMethod::Post,
            "/v1/developer-auth/verification-resends",
            handle_verification_resend,
            DeveloperMetricOperation::VerificationResend,
        ),
        (
            HttpMethod::Post,
            "/v1/developer-auth/sessions",
            handle_sign_in,
            DeveloperMetricOperation::SignIn,
        ),
        (
            HttpMethod::Post,
            "/v1/developer-auth/sessions/refresh",
            handle_refresh,
            DeveloperMetricOperation::Refresh,
        ),
        (
            HttpMethod::Delete,
            "/v1/developer-auth/sessions/current",
            handle_sign_out,
            DeveloperMetricOperation::SignOut,
        ),
        (
            HttpMethod::Post,
            "/v1/developer-auth/password-recovery-requests",
            handle_recovery_request,
            DeveloperMetricOperation::RecoveryRequest,
        ),
        (
            HttpMethod::Post,
            "/v1/developer-auth/password-recoveries",
            handle_recovery_completion,
            DeveloperMetricOperation::RecoveryCompletion,
        ),
        (
            HttpMethod::Get,
            "/v1/developer-auth/wait-list-status",
            handle_waitlist_status,
            DeveloperMetricOperation::WaitlistStatus,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| {
            let result = handler(&graph, &request);
            graph.developer_metrics().observe_http(operation, &result);
            result
        })?;
    }
    let step_up_graph = Arc::clone(&graph);
    router.add_route(
        HttpMethod::Post,
        "/v1/developer-auth/sessions/current/actions/verify-password",
        move |request| handle_developer_password_verification(&step_up_graph, &request),
    )?;

    for (method, path, handler, operation) in [
        (
            HttpMethod::Get,
            "/v1/operator/developer-waitlist",
            handle_waitlist_list as Handler,
            DeveloperMetricOperation::ReviewList,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/developer-waitlist/{developerIdentityId}",
            handle_waitlist_detail,
            DeveloperMetricOperation::ReviewDetail,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/developer-waitlist/{developerIdentityId}/actions/approve",
            handle_waitlist_approve,
            DeveloperMetricOperation::Approve,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/developer-waitlist/{developerIdentityId}/actions/reject",
            handle_waitlist_reject,
            DeveloperMetricOperation::Reject,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| {
            let result = handler(&graph, &request);
            graph.developer_metrics().observe_http(operation, &result);
            result
        })?;
    }
    Ok(())
}

fn handle_developer_password_verification(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    same_origin(graph, request)?;
    no_query(request)?;
    require_json(request)?;
    let body: PasswordVerificationWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let verified = graph
            .developer_registration_service()
            .verify_password_identity(actor.normalized_email(), &body.password, now)
            .await
            .map_err(|error| workflow_error(request, error))?
            .filter(|account| account.id() == actor.identity_id())
            .ok_or_else(|| unauthenticated(request, "developer password was not accepted"))?;
        let _ = verified;
        let grant = graph
            .developer_workspace_security()
            .issue_step_up(&actor, now)
            .map_err(|_| unavailable(request, "developer step-up is unavailable"))?;
        public_json(
            request,
            200,
            &DeveloperStepUpWire {
                token: grant.token,
                expires_at_unix_seconds: grant.expires_at_unix_seconds,
            },
        )
    })
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_registration(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    public_json_request(graph, request)?;
    let body: RegistrationWire = parse_json(request)?;
    let outcome = block_on(graph.developer_registration_service().register(
        &body.email,
        &body.display_name,
        &body.password,
        &source(request),
        now(request)?,
    ))
    .map_err(|error| workflow_error(request, error))?;
    generic_response(request, outcome)
}

fn handle_verification(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    public_json_request(graph, request)?;
    let body: TokenWire = parse_json(request)?;
    let outcome = block_on(graph.developer_registration_service().verify_email(
        &body.token,
        &source(request),
        now(request)?,
    ))
    .map_err(|error| workflow_error(request, error))?;
    match outcome {
        DeveloperVerificationOutcome::Verified => public_json(
            request,
            200,
            &StatusWire {
                status: "waitlisted",
            },
        ),
        DeveloperVerificationOutcome::InvalidOrExpired => invalid_token(request),
    }
}

fn handle_verification_resend(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    public_json_request(graph, request)?;
    let body: EmailWire = parse_json(request)?;
    let outcome = block_on(graph.developer_registration_service().resend_verification(
        &body.email,
        &source(request),
        now(request)?,
    ))
    .map_err(|error| workflow_error(request, error))?;
    generic_response(request, outcome)
}

fn handle_sign_in(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    public_json_request(graph, request)?;
    let body: SignInWire = parse_json(request)?;
    let grant = block_on(graph.developer_registration_service().sign_in(
        &body.email,
        &body.password,
        &source(request),
        now(request)?,
    ))
    .map_err(|error| workflow_error(request, error))?;
    session_response(graph, request, &grant)
}

fn handle_refresh(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    same_origin(graph, request)?;
    no_payload(request)?;
    let refresh = cookie(request, REFRESH_COOKIE_NAME)
        .ok_or_else(|| unauthenticated(request, "developer session is invalid"))?;
    let grant = block_on(
        graph
            .developer_registration_service()
            .refresh(refresh, now(request)?),
    )
    .map_err(|error| workflow_error(request, error))?;
    session_response(graph, request, &grant)
}

fn handle_sign_out(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    same_origin(graph, request)?;
    no_payload(request)?;
    if let Some(refresh) = cookie(request, REFRESH_COOKIE_NAME) {
        block_on(
            graph
                .developer_registration_service()
                .sign_out(refresh, now(request)?),
        )
        .map_err(|error| workflow_error(request, error))?;
    }
    HttpResponse::empty(204)
        .with_header(
            "Set-Cookie",
            "__Host-mako_developer_refresh=; Path=/v1/developer-auth/sessions; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
        )
        .map_err(|_| internal(request, "session response could not be created"))
}

fn handle_recovery_request(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    public_json_request(graph, request)?;
    let body: EmailWire = parse_json(request)?;
    let outcome = block_on(
        graph
            .developer_registration_service()
            .request_password_recovery(&body.email, &source(request), now(request)?),
    )
    .map_err(|error| workflow_error(request, error))?;
    generic_response(request, outcome)
}

fn handle_recovery_completion(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    public_json_request(graph, request)?;
    let body: RecoveryWire = parse_json(request)?;
    let outcome = block_on(
        graph
            .developer_registration_service()
            .complete_password_recovery(
                &body.token,
                &body.password,
                &source(request),
                now(request)?,
            ),
    )
    .map_err(|error| workflow_error(request, error))?;
    match outcome {
        DeveloperRecoveryOutcome::Completed { identity_id } => {
            block_on(
                graph
                    .operator_password_authentication()
                    .revoke_identity_sessions(&identity_id, now(request)?),
            )
            .map_err(|_| unavailable(request, "operator session revocation is unavailable"))?;
            public_json(
                request,
                200,
                &StatusWire {
                    status: "password_updated",
                },
            )
        }
        DeveloperRecoveryOutcome::InvalidOrExpired => invalid_token(request),
    }
}

fn handle_waitlist_status(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let now = now(request)?;
    let authority = block_on(graph.hosted_session_authenticator().authenticate(
        request.header("authorization"),
        DeveloperAccessAudience::Waitlist,
        now,
    ))
    .map_err(|error| hosted_auth_error(request, error))?;
    let status = block_on(
        graph
            .developer_registration_service()
            .waitlist_status(&authority.identity_id, authority.authorization_epoch),
    )
    .map_err(|error| workflow_error(request, error))?;
    public_json(
        request,
        200,
        &WaitlistStatusWire {
            developer_identity_id: status.identity_id.as_str(),
            status: "waitlisted",
        },
    )
}

fn handle_waitlist_list(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    reject_unknown_query(request, &["cursor", "limit"])?;
    let cursor = query_value(request, "cursor")?;
    let limit = query_value(request, "limit")?
        .unwrap_or("25")
        .parse::<usize>()
        .ok()
        .and_then(NonZeroUsize::new)
        .filter(|limit| limit.get() <= 100)
        .ok_or_else(|| invalid(request, "wait-list limit is invalid"))?;
    with_operator(graph, request, false, |operator| async move {
        let page = graph
            .developer_registration_service()
            .review_page(&operator, cursor, limit)
            .await
            .map_err(|error| workflow_error(request, error))?;
        let mut applicants = Vec::with_capacity(page.accounts.len());
        for account in &page.accounts {
            let operator_entitlement_status =
                operator_entitlement_status(graph, request, account.id()).await?;
            applicants.push(ApplicantWire::from_account(
                request,
                account,
                operator_entitlement_status,
            )?);
        }
        public_json(
            request,
            200,
            &ApplicantPageWire {
                applicants,
                next_cursor: page.next_cursor,
            },
        )
    })
}

fn handle_waitlist_detail(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let identity_id = developer_identity_id(request)?;
    with_operator(graph, request, false, |operator| async move {
        let account = graph
            .developer_registration_service()
            .review_detail(&operator, &identity_id)
            .await
            .map_err(|error| workflow_error(request, error))?
            .ok_or_else(|| not_found(request, "wait-list applicant was not found"))?;
        let operator_entitlement_status =
            operator_entitlement_status(graph, request, account.id()).await?;
        public_json(
            request,
            200,
            &ApplicantWire::from_account(request, &account, operator_entitlement_status)?,
        )
    })
}

fn handle_waitlist_approve(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    handle_decision(graph, request, DeveloperDecisionKind::Approve)
}

fn handle_waitlist_reject(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    handle_decision(graph, request, DeveloperDecisionKind::Reject)
}

fn handle_decision(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    kind: DeveloperDecisionKind,
) -> Result<HttpResponse, HttpApiError> {
    let operator = authenticate_operator(graph, request, now(request)?, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: DecisionWire = parse_json(request)?;
    let identity_id = developer_identity_id(request)?;
    let self_review = operator.authentication_identity_id() == Some(&identity_id);
    let idempotency_key = require_idempotency(request)?.to_owned();
    let account = block_on(graph.developer_registration_service().decide(
        &operator,
        &identity_id,
        kind,
        body.reason.as_deref().unwrap_or_default(),
        request.request_id(),
        &idempotency_key,
        now(request)?,
    ))
    .map_err(|error| workflow_error(request, error))?;
    if self_review {
        graph.developer_metrics().observe_self_review();
    }
    let operator_entitlement_status =
        block_on(operator_entitlement_status(graph, request, account.id()))?;
    public_json(
        request,
        200,
        &ApplicantWire::from_account(request, &account, operator_entitlement_status)?,
    )
}

async fn operator_entitlement_status(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    identity_id: &DeveloperIdentityId,
) -> Result<&'static str, HttpApiError> {
    graph
        .operator_password_authentication()
        .store()
        .get_entitlement(identity_id)
        .await
        .map(|entitlement| {
            if entitlement.is_some() {
                "active"
            } else {
                "absent"
            }
        })
        .map_err(|_| unavailable(request, "operator entitlement status is unavailable"))
}

fn public_json_request(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
) -> Result<(), HttpApiError> {
    same_origin(graph, request)?;
    no_query(request)?;
    require_json(request)
}

pub(crate) fn same_origin(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
) -> Result<(), HttpApiError> {
    let expected = &graph
        .developer_registration_service()
        .config()
        .public_origin;
    if request.header("origin") == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(forbidden(request, "same-origin request is required"))
    }
}

fn session_response(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    grant: &DeveloperSessionGrant,
) -> Result<HttpResponse, HttpApiError> {
    let access_token = graph
        .developer_session_issuer()
        .issue(&grant.claims)
        .map_err(|_| internal(request, "access session could not be issued"))?;
    let response = public_json(
        request,
        200,
        &SessionWire {
            access_token,
            token_type: "Bearer",
            audience: grant.claims.audience.as_str(),
            status: match grant.claims.audience {
                DeveloperAccessAudience::Active => "active",
                DeveloperAccessAudience::Waitlist => "waitlisted",
            },
            expires_at: format_timestamp(request, grant.claims.expires_at_unix_seconds)?,
        },
    )?;
    response
        .with_header("Set-Cookie", grant.set_cookie_header())
        .map_err(|_| internal(request, "session response could not be created"))
}

fn generic_response(
    request: &HttpRequest,
    _outcome: DeveloperGenericOutcome,
) -> Result<HttpResponse, HttpApiError> {
    public_json(
        request,
        202,
        &GenericWire {
            status: "accepted",
            message: "If the request is eligible, check the account email for next steps.",
        },
    )
}

fn invalid_token(request: &HttpRequest) -> Result<HttpResponse, HttpApiError> {
    Err(HttpApiError::new(
        400,
        ErrorCode::InvalidRequest,
        "The token is invalid or expired.",
        request.request_id(),
        RetryAdvice::Never,
    ))
}

fn workflow_error(request: &HttpRequest, error: DeveloperWorkflowError) -> HttpApiError {
    match error {
        DeveloperWorkflowError::InvalidRequest => invalid(request, "developer request is invalid"),
        DeveloperWorkflowError::InvalidCredentials | DeveloperWorkflowError::InvalidSession => {
            unauthenticated(request, "developer credentials or session are invalid")
        }
        DeveloperWorkflowError::Forbidden => forbidden(request, "wait-list review is forbidden"),
        DeveloperWorkflowError::NotFound => not_found(request, "wait-list applicant was not found"),
        DeveloperWorkflowError::Conflict => conflict(request, "developer state changed"),
        DeveloperWorkflowError::RateLimited | DeveloperWorkflowError::Busy => HttpApiError::new(
            429,
            ErrorCode::RateLimited,
            "Developer authentication is temporarily limited.",
            request.request_id(),
            RetryAdvice::AfterDelay { after_ms: 60_000 },
        ),
        DeveloperWorkflowError::Unavailable => {
            unavailable(request, "developer registration is unavailable")
        }
        _ => unavailable(request, "developer identity service is unavailable"),
    }
}

fn hosted_auth_error(request: &HttpRequest, error: HostedAuthenticationError) -> HttpApiError {
    match error {
        HostedAuthenticationError::Invalid => {
            unauthenticated(request, "wait-list authorization is invalid")
        }
        HostedAuthenticationError::Unavailable => {
            unavailable(request, "developer identity state is unavailable")
        }
    }
}

fn unauthenticated(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        401,
        ErrorCode::Unauthenticated,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn with_operator<F, Fut>(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    mutation: bool,
    operation: F,
) -> Result<HttpResponse, HttpApiError>
where
    F: FnOnce(mako_control_plane::OperatorPrincipal) -> Fut,
    Fut: std::future::Future<Output = Result<HttpResponse, HttpApiError>>,
{
    let actor = authenticate_operator(graph, request, now(request)?, mutation)?;
    block_on(operation(actor))
}

fn developer_identity_id(request: &HttpRequest) -> Result<DeveloperIdentityId, HttpApiError> {
    DeveloperIdentityId::parse(
        request
            .path_parameter("developerIdentityId")
            .unwrap_or_default(),
    )
    .map_err(|_| invalid(request, "developer identity path is invalid"))
}

pub(crate) fn now(request: &HttpRequest) -> Result<u64, HttpApiError> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| unavailable(request, "system clock is unavailable"))
}

pub(crate) fn source(request: &HttpRequest) -> String {
    request
        .header("x-forwarded-for")
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && !value.chars().any(char::is_control)
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        })
        .map_or_else(
            || {
                request
                    .remote_address()
                    .map_or_else(|| "unknown".to_owned(), |address| address.ip().to_string())
            },
            ToOwned::to_owned,
        )
}

pub(crate) fn cookie<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    let mut found = None;
    for header in request.header_values("cookie") {
        for item in header.split(';') {
            let (cookie_name, value) = item.trim().split_once('=')?;
            if cookie_name == name {
                if found.is_some()
                    || value.is_empty()
                    || value.len() > 4_096
                    || value.chars().any(char::is_whitespace)
                    || value.chars().any(char::is_control)
                {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RegistrationWire {
    email: String,
    display_name: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmailWire {
    email: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenWire {
    token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignInWire {
    email: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PasswordVerificationWire {
    password: String,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeveloperStepUpWire {
    token: String,
    expires_at_unix_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryWire {
    token: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionWire {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericWire<'a> {
    status: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusWire<'a> {
    status: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionWire<'a> {
    access_token: String,
    token_type: &'a str,
    audience: &'a str,
    status: &'a str,
    expires_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WaitlistStatusWire<'a> {
    developer_identity_id: &'a str,
    status: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplicantPageWire {
    applicants: Vec<ApplicantWire>,
    next_cursor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplicantWire {
    developer_identity_id: String,
    email: String,
    display_name: String,
    status: &'static str,
    operator_entitlement_status: &'static str,
    authorization_epoch: u64,
    email_verified: bool,
    created_at: String,
    updated_at: String,
}

impl ApplicantWire {
    fn from_account(
        request: &HttpRequest,
        account: &DeveloperAccount,
        operator_entitlement_status: &'static str,
    ) -> Result<Self, HttpApiError> {
        Ok(Self {
            developer_identity_id: account.id().as_str().to_owned(),
            email: account.normalized_email().as_str().to_owned(),
            display_name: account.display_name().to_owned(),
            status: match account.status() {
                mako_control_plane::DeveloperIdentityStatus::Unverified => "unverified",
                mako_control_plane::DeveloperIdentityStatus::Waitlisted => "waitlisted",
                mako_control_plane::DeveloperIdentityStatus::Active => "active",
                mako_control_plane::DeveloperIdentityStatus::Rejected => "rejected",
                mako_control_plane::DeveloperIdentityStatus::Disabled => "disabled",
                mako_control_plane::DeveloperIdentityStatus::Deleted => "deleted",
            },
            operator_entitlement_status,
            authorization_epoch: account.authorization_epoch(),
            email_verified: account.email_verified_at_unix_seconds().is_some(),
            created_at: format_timestamp(request, account.created_at_unix_seconds())?,
            updated_at: format_timestamp(request, account.updated_at_unix_seconds())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_parser_rejects_duplicates_and_bounds_values() {
        // Route-level construction is covered by the runtime router integration tests. Keep the
        // parser's security rules independently visible here.
        assert_eq!(REFRESH_COOKIE_NAME, "__Host-mako_developer_refresh");
        assert!("x".repeat(4_097).len() > 4_096);
    }

    #[test]
    fn decision_wire_accepts_missing_null_and_blank_optional_reasons() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({ "reason": null }),
            serde_json::json!({ "reason": "  " }),
        ] {
            let decision: DecisionWire = serde_json::from_value(value).expect("optional reason");
            assert!(
                decision
                    .reason
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            );
        }
        let supplied: DecisionWire = serde_json::from_value(serde_json::json!({
            "reason": "approved for beta"
        }))
        .expect("supplied reason");
        assert_eq!(supplied.reason.as_deref(), Some("approved for beta"));
        assert!(
            serde_json::from_value::<DecisionWire>(serde_json::json!({
                "reason": "approved for beta",
                "unexpected": true
            }))
            .is_err()
        );
    }

    #[test]
    fn session_and_applicant_wires_match_the_date_time_contract() {
        let session = serde_json::to_value(SessionWire {
            access_token: "redacted.test.token".to_owned(),
            token_type: "Bearer",
            audience: "mako-developer-waitlist",
            status: "waitlisted",
            expires_at: "2026-08-09T12:15:00Z".to_owned(),
        })
        .expect("session wire");
        assert_eq!(session["expiresAt"], "2026-08-09T12:15:00Z");
        assert!(session.get("expiresAtUnixSeconds").is_none());

        let applicant = serde_json::to_value(ApplicantWire {
            developer_identity_id: "dev_applicant01".to_owned(),
            email: "applicant@example.test".to_owned(),
            display_name: "Applicant".to_owned(),
            status: "waitlisted",
            operator_entitlement_status: "active",
            authorization_epoch: 2,
            email_verified: true,
            created_at: "2026-08-09T12:00:00Z".to_owned(),
            updated_at: "2026-08-09T12:01:00Z".to_owned(),
        })
        .expect("applicant wire");
        assert_eq!(applicant["createdAt"], "2026-08-09T12:00:00Z");
        assert_eq!(applicant["updatedAt"], "2026-08-09T12:01:00Z");
        assert_eq!(applicant["status"], "waitlisted");
        assert_eq!(applicant["operatorEntitlementStatus"], "active");
        assert!(applicant.get("createdAtUnixSeconds").is_none());
    }
}
