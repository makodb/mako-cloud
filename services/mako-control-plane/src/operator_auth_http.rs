use std::sync::Arc;

use futures::executor::block_on;
use mako_api::{ErrorCode, RetryAdvice};
use mako_control_plane::{
    AuthenticatedOperatorSession, OperatorPasswordAuthenticationError, OperatorSessionProfile,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlPlaneGraph,
    developer_auth_http::{cookie, now, same_origin, source},
    developer_metrics::OperatorAuthMetricOperation,
    http_support::public_json,
    management_http::{
        forbidden, format_timestamp, internal, no_payload, no_query, parse_json, require_json,
        unavailable,
    },
};

const OPERATOR_COOKIE_NAME: &str = "__Secure-mako_operator";

pub(crate) fn add_operator_auth_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler, operation) in [
        (
            HttpMethod::Post,
            "/v1/operator-auth/sessions",
            handle_sign_in as Handler,
            OperatorAuthMetricOperation::SignIn,
        ),
        (
            HttpMethod::Get,
            "/v1/operator-auth/sessions/current",
            handle_current_session,
            OperatorAuthMetricOperation::Inspect,
        ),
        (
            HttpMethod::Delete,
            "/v1/operator-auth/sessions/current",
            handle_sign_out,
            OperatorAuthMetricOperation::SignOut,
        ),
        (
            HttpMethod::Post,
            "/v1/operator-auth/sessions/current/actions/verify-password",
            handle_password_verification,
            OperatorAuthMetricOperation::StepUp,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| {
            let result = handler(&graph, &request);
            graph
                .developer_metrics()
                .observe_operator_auth(operation, &result);
            result
        })?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_sign_in(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    same_origin(graph, request)?;
    no_query(request)?;
    require_json(request)?;
    let body: SignInWire = parse_json(request)?;
    let grant = block_on(graph.operator_password_authentication().sign_in(
        &body.email,
        &body.password,
        &source(request),
        now(request)?,
    ))
    .map_err(|error| operator_password_error(request, error))?;
    let response = profile_response(request, &grant.profile)?;
    response
        .with_header("Set-Cookie", grant.set_cookie_header())
        .map_err(|_| internal(request, "operator session response could not be created"))
}

fn handle_current_session(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    no_payload(request)?;
    let authenticated = authenticate_password_session(graph, request, now(request)?)?;
    profile_response(request, &authenticated.profile)
}

fn handle_sign_out(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    same_origin(graph, request)?;
    no_query(request)?;
    no_payload(request)?;
    block_on(
        graph
            .operator_password_authentication()
            .sign_out(cookie(request, OPERATOR_COOKIE_NAME), now(request)?),
    )
    .map_err(|error| operator_password_error(request, error))?;
    HttpResponse::empty(204)
        .with_header(
            "Set-Cookie",
            &graph
                .operator_password_authentication()
                .expired_cookie_header(),
        )
        .map_err(|_| internal(request, "operator sign-out response could not be created"))
}

fn handle_password_verification(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    same_origin(graph, request)?;
    no_query(request)?;
    require_json(request)?;
    let body: PasswordWire = parse_json(request)?;
    let profile = block_on(
        graph
            .operator_password_authentication()
            .verify_current_password(
                cookie(request, OPERATOR_COOKIE_NAME),
                &body.password,
                &source(request),
                now(request)?,
            ),
    )
    .map_err(|error| operator_password_error(request, error))?;
    profile_response(request, &profile)
}

pub(crate) fn authenticate_operator(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    now_unix_seconds: u64,
    mutation: bool,
) -> Result<mako_control_plane::OperatorPrincipal, HttpApiError> {
    if mutation {
        same_origin(graph, request)?;
    }
    if let Some(credential) = cookie(request, OPERATOR_COOKIE_NAME) {
        let authenticated = block_on(
            graph
                .operator_password_authentication()
                .authenticate(Some(credential), now_unix_seconds),
        )
        .map_err(|error| operator_password_error(request, error))?;
        if mutation
            && let Err(error) = graph
                .operator_password_authentication()
                .require_mutation_freshness(&authenticated, now_unix_seconds)
        {
            if matches!(error, OperatorPasswordAuthenticationError::StepUpRequired) {
                graph
                    .developer_metrics()
                    .observe_operator_step_up_required();
            }
            return Err(operator_password_error(request, error));
        }
        return Ok(authenticated.principal);
    }
    if !graph.operator_break_glass_bearer_enabled() {
        return Err(unauthenticated(request));
    }
    if mutation {
        graph
            .developer_metrics()
            .observe_operator_step_up_required();
        return Err(step_up_required(request));
    }
    block_on(
        graph
            .operator_authenticator()
            .authenticate(request.header("authorization"), now_unix_seconds),
    )
    .map_err(|_| unauthenticated(request))
}

fn authenticate_password_session(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    now_unix_seconds: u64,
) -> Result<AuthenticatedOperatorSession, HttpApiError> {
    block_on(
        graph
            .operator_password_authentication()
            .authenticate(cookie(request, OPERATOR_COOKIE_NAME), now_unix_seconds),
    )
    .map_err(|error| operator_password_error(request, error))
}

fn profile_response(
    request: &HttpRequest,
    profile: &OperatorSessionProfile,
) -> Result<HttpResponse, HttpApiError> {
    public_json(
        request,
        200,
        &ProfileWire {
            operator_id: profile.operator_id.as_str(),
            developer_identity_id: profile.developer_identity_id.as_str(),
            email: &profile.email,
            display_name: &profile.display_name,
            developer_status: profile.developer_status.map(developer_status_name),
            permissions: profile.permissions.iter().map(permission_name).collect(),
            password_verified_at: format_timestamp(
                request,
                profile.password_verified_at_unix_seconds,
            )?,
            expires_at: format_timestamp(request, profile.expires_at_unix_seconds)?,
        },
    )
}

fn developer_status_name(status: mako_control_plane::DeveloperIdentityStatus) -> &'static str {
    match status {
        mako_control_plane::DeveloperIdentityStatus::Unverified => "unverified",
        mako_control_plane::DeveloperIdentityStatus::Waitlisted => "waitlisted",
        mako_control_plane::DeveloperIdentityStatus::Active => "active",
        mako_control_plane::DeveloperIdentityStatus::Rejected => "rejected",
        mako_control_plane::DeveloperIdentityStatus::Disabled => "disabled",
        mako_control_plane::DeveloperIdentityStatus::Deleted => "deleted",
    }
}

fn permission_name(permission: &mako_control_plane::OperatorPermission) -> &'static str {
    match permission {
        mako_control_plane::OperatorPermission::TenantRead => "tenant_read",
        mako_control_plane::OperatorPermission::OverviewRead => "overview_read",
        mako_control_plane::OperatorPermission::OperationsRead => "operations_read",
        mako_control_plane::OperatorPermission::IncidentRead => "incident_read",
        mako_control_plane::OperatorPermission::IncidentManage => "incident_manage",
        mako_control_plane::OperatorPermission::BackupRead => "backup_read",
        mako_control_plane::OperatorPermission::RecoveryManage => "recovery_manage",
        mako_control_plane::OperatorPermission::FleetRead => "fleet_read",
        mako_control_plane::OperatorPermission::SecurityRead => "security_read",
        mako_control_plane::OperatorPermission::SecurityManage => "security_manage",
        mako_control_plane::OperatorPermission::ActivityRead => "activity_read",
        mako_control_plane::OperatorPermission::ActivityExport => "activity_export",
        mako_control_plane::OperatorPermission::ProvisioningRepair => "provisioning_repair",
        mako_control_plane::OperatorPermission::QuotaOverride => "quota_override",
        mako_control_plane::OperatorPermission::AbuseResponse => "abuse_response",
        mako_control_plane::OperatorPermission::SupportAccess => "support_access",
        mako_control_plane::OperatorPermission::WaitlistReview => "waitlist_review",
    }
}

fn operator_password_error(
    request: &HttpRequest,
    error: OperatorPasswordAuthenticationError,
) -> HttpApiError {
    match error {
        OperatorPasswordAuthenticationError::InvalidCredentials => HttpApiError::new(
            401,
            ErrorCode::Unauthenticated,
            "The email or password was not accepted.",
            request.request_id(),
            RetryAdvice::Never,
        ),
        OperatorPasswordAuthenticationError::Unauthenticated => unauthenticated(request),
        OperatorPasswordAuthenticationError::StepUpRequired => step_up_required(request),
        OperatorPasswordAuthenticationError::RateLimited {
            retry_after_seconds,
        } => HttpApiError::new(
            429,
            ErrorCode::RateLimited,
            "Too many attempts. Try again later.",
            request.request_id(),
            RetryAdvice::AfterDelay {
                after_ms: retry_after_seconds.saturating_mul(1_000),
            },
        )
        .with_header("Retry-After", retry_after_seconds.max(1).to_string()),
        OperatorPasswordAuthenticationError::Unavailable => {
            unavailable(request, "operator password authentication is unavailable")
        }
        OperatorPasswordAuthenticationError::InvalidConfiguration
        | OperatorPasswordAuthenticationError::Developer(_)
        | OperatorPasswordAuthenticationError::Store(_) => {
            unavailable(request, "operator authentication dependency is unavailable")
        }
        OperatorPasswordAuthenticationError::NotFound
        | OperatorPasswordAuthenticationError::Ineligible
        | OperatorPasswordAuthenticationError::Conflict
        | OperatorPasswordAuthenticationError::ConfirmationMismatch
        | OperatorPasswordAuthenticationError::LastRecoverableAdministrator => {
            forbidden(request, "operator entitlement operation is unavailable")
        }
    }
}

fn unauthenticated(request: &HttpRequest) -> HttpApiError {
    HttpApiError::new(
        401,
        ErrorCode::Unauthenticated,
        "Operator session is not authenticated.",
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn step_up_required(request: &HttpRequest) -> HttpApiError {
    HttpApiError::new(
        401,
        ErrorCode::OperatorStepUpRequired,
        "Password verification is required before this operator action.",
        request.request_id(),
        RetryAdvice::Never,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignInWire {
    email: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordWire {
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileWire<'a> {
    operator_id: &'a str,
    developer_identity_id: &'a str,
    email: &'a str,
    display_name: &'a str,
    developer_status: Option<&'static str>,
    permissions: Vec<&'static str>,
    password_verified_at: String,
    expires_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_profile_presents_nullable_developer_state_separately() {
        let profile = serde_json::to_value(ProfileWire {
            operator_id: "opr_operator01",
            developer_identity_id: "dev_operator01",
            email: "operator@example.test",
            display_name: "Operator",
            developer_status: Some("waitlisted"),
            permissions: vec!["waitlist_review"],
            password_verified_at: "2026-08-12T00:00:00Z".to_owned(),
            expires_at: "2026-08-12T01:00:00Z".to_owned(),
        })
        .expect("profile wire");
        assert_eq!(profile["developerStatus"], "waitlisted");
        assert!(profile.get("operatorStatus").is_none());

        let operator_only = serde_json::to_value(ProfileWire {
            developer_status: None,
            operator_id: "opr_operator01",
            developer_identity_id: "dev_operator01",
            email: "operator@example.test",
            display_name: "Operator",
            permissions: vec!["waitlist_review"],
            password_verified_at: "2026-08-12T00:00:00Z".to_owned(),
            expires_at: "2026-08-12T01:00:00Z".to_owned(),
        })
        .expect("operator-only profile wire");
        assert!(operator_only["developerStatus"].is_null());
    }
}
