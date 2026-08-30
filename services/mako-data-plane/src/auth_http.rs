use std::{num::NonZeroU64, sync::Arc, time::SystemTime};

use futures::executor::block_on;
use mako_api::{ErrorCode, RetryAdvice, TenantScope};
use mako_audit::{
    ActorIdentity, AuditCategory, AuditEvent, AuditOutcome, CorrelationId, RequestId,
    ResourceReference, SafeAttributes, SignalContext, SignalScope,
};
use mako_gateway::{
    GatewayQuotaCharge, GatewayQuotaDecision, GatewayQuotaPolicySource, GatewayQuotaResource,
    VerifiedAccessIdentity,
};
use mako_identity::{
    AppUserStatus, AuthenticationAuditError, AuthenticationAuditEvent, AuthenticationAuditOutcome,
    AuthenticationAuditSink, EmailSignupConfig, SignInRequestMetadata, SignInResponse,
    SignInService, SignupService, TransactionalEmailProvider, VerificationEmail,
    VerifiedProjectCredential,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};

use crate::{DataPlaneGraph, DataPlaneRefreshOutcome, DataPlaneSessionGrant};

const PUBLIC_KEY_HEADER: &str = "x-mako-key";
const AUTHORIZATION_HEADER: &str = "authorization";
const JSON_CONTENT_TYPE: &str = "application/json";

pub fn add_auth_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/auth/signup",
        Arc::clone(&graph),
        handle_signup,
    )?;
    add_route(
        router,
        HttpMethod::Get,
        "/v1/projects/{projectId}/environments/{environmentId}/auth/jwks",
        Arc::clone(&graph),
        handle_jwks,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/auth/signin",
        Arc::clone(&graph),
        handle_signin,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/auth/token",
        Arc::clone(&graph),
        handle_refresh,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/auth/signout",
        Arc::clone(&graph),
        handle_signout,
    )?;
    add_route(
        router,
        HttpMethod::Get,
        "/v1/projects/{projectId}/environments/{environmentId}/auth/user",
        graph,
        handle_current_user,
    )?;
    Ok(())
}

fn add_route(
    router: &mut HttpRouter,
    method: HttpMethod,
    path: &str,
    graph: Arc<DataPlaneGraph>,
    handler: fn(&Arc<DataPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>,
) -> Result<(), RouteRegistrationError> {
    router.add_route(method, path, move |request| handler(&graph, &request))
}

fn handle_signup(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_json(request)?;
    let tenant = tenant_for(graph, request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        verify_public_key(graph, &tenant, request, now).await?;
        charge_auth(graph, &tenant, request, now).await?;
        let body: SignUpWire = parse_json(request)?;
        validate_email_password(request, &body.email, &body.password, 8)?;
        let store = graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?;
        let email = DisabledEmailProvider;
        let service = SignupService::new(
            &store,
            graph.password_service(),
            &email,
            EmailSignupConfig {
                enabled: true,
                // Email verification is deliberately disabled until its public
                // completion route and durable SMTP outbox are composed.
                require_verification: false,
                verification_ttl_seconds: 24 * 60 * 60,
            },
        )
        .map_err(|_| unavailable(request, "sign-up is unavailable"))?;
        service
            .sign_up(&tenant, &body.email, &body.password, now)
            .await
            .map_err(|error| match error {
                mako_identity::SignupError::Password(_) | mako_identity::SignupError::Record(_) => {
                    invalid(request, "sign-up input is invalid")
                }
                _ => unavailable(request, "sign-up is unavailable"),
            })?;
        // A new user changes the count this tenant is measured on.
        graph.storage_sampler().mark_users(&tenant);
        append_audit(
            graph,
            &tenant,
            AuditCategory::Authentication,
            ActorIdentity::Anonymous,
            "application_auth",
            "signup",
            "application_signup",
            AuditOutcome::Allowed,
            "accepted",
            request.request_id(),
            now,
        )
        .await?;
        json(request, 202, &SignUpAcceptedWire { accepted: true })
    })
}

fn handle_jwks(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let tenant = tenant_for(graph, request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        verify_public_key(graph, &tenant, request, now).await?;
        charge_auth(graph, &tenant, request, now).await?;
        let store = graph
            .signing_key_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "signing keys are unavailable"))?;
        let mut jwks = store
            .jwks()
            .await
            .map_err(|_| unavailable(request, "signing keys are unavailable"))?;
        if jwks.keys.is_empty() {
            let _ = graph.initialize_signing_key(&tenant, now).await;
            jwks = store
                .jwks()
                .await
                .map_err(|_| unavailable(request, "signing keys are unavailable"))?;
            if jwks.keys.is_empty() {
                return Err(unavailable(request, "signing keys are unavailable"));
            }
        }
        append_audit(
            graph,
            &tenant,
            AuditCategory::Authentication,
            ActorIdentity::Anonymous,
            "application_auth",
            "jwks",
            "application_jwks_read",
            AuditOutcome::Allowed,
            "verified_public_project_key",
            request.request_id(),
            now,
        )
        .await?;
        json(request, 200, &jwks)
    })
}

fn handle_signin(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_json(request)?;
    let tenant = tenant_for(graph, request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        verify_public_key(graph, &tenant, request, now).await?;
        charge_auth(graph, &tenant, request, now).await?;
        let body: PasswordSignInWire = parse_json(request)?;
        validate_email_password(request, &body.email, &body.password, 1)?;
        let store = graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?;
        let audit = DataPlaneAuthenticationAudit {
            graph: Arc::clone(graph),
        };
        let service = SignInService::new(
            &store,
            graph.password_service(),
            graph.sign_in_throttle(),
            &audit,
        )
        .map_err(|_| unavailable(request, "sign-in is unavailable"))?;
        let partition = request
            .remote_address()
            .map_or_else(|| "unknown".to_owned(), |address| address.ip().to_string());
        let metadata = SignInRequestMetadata::new(request.request_id(), partition)
            .map_err(|_| invalid(request, "request metadata is invalid"))?;
        match service
            .sign_in(&tenant, &body.email, &body.password, &metadata, now)
            .await
            .map_err(|_| unavailable(request, "sign-in is unavailable"))?
        {
            SignInResponse::Authenticated { user_id } => {
                let grant = graph
                    .create_application_session(&tenant, &user_id, now)
                    .await
                    .map_err(|_| unavailable(request, "session issuance is unavailable"))?;
                json(request, 200, &session_wire(request, grant)?)
            }
            SignInResponse::Denied {
                retry_after_seconds: Some(seconds),
            } => Err(HttpApiError::new(
                429,
                ErrorCode::RateLimited,
                "sign-in attempts are temporarily limited",
                request.request_id(),
                RetryAdvice::AfterDelay {
                    after_ms: seconds.saturating_mul(1_000),
                },
            )),
            SignInResponse::Denied {
                retry_after_seconds: None,
            } => Err(unauthenticated(request, "email or password is invalid")),
        }
    })
}

fn handle_refresh(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_json(request)?;
    let tenant = tenant_for(graph, request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        verify_public_key(graph, &tenant, request, now).await?;
        charge_auth(graph, &tenant, request, now).await?;
        let body: RefreshWire = parse_json(request)?;
        if !(32..=512).contains(&body.refresh_token.len())
            || body.refresh_token.chars().any(char::is_control)
        {
            return Err(invalid(request, "refresh token is invalid"));
        }
        match graph
            .refresh_application_session(&tenant, &body.refresh_token, now)
            .await
            .map_err(|_| unavailable(request, "session refresh is unavailable"))?
        {
            DataPlaneRefreshOutcome::Rotated(grant) => {
                append_audit(
                    graph,
                    &tenant,
                    AuditCategory::Authentication,
                    ActorIdentity::ApplicationUser {
                        actor_id: grant.user.id().as_str().to_owned(),
                        session_id: grant.session_id.as_str().to_owned(),
                    },
                    "application_session",
                    grant.session_id.as_str(),
                    "application_session_refresh",
                    AuditOutcome::Allowed,
                    "refresh_rotated",
                    request.request_id(),
                    now,
                )
                .await?;
                json(request, 200, &session_wire(request, *grant)?)
            }
            DataPlaneRefreshOutcome::Invalid => {
                Err(unauthenticated(request, "refresh credential is invalid"))
            }
            DataPlaneRefreshOutcome::ReplayDetected => Err(unauthenticated(
                request,
                "refresh credential replay revoked the session",
            )),
        }
    })
}

fn handle_signout(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let tenant = tenant_for(graph, request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let identity = verify_bearer(graph, &tenant, request, now).await?;
        charge_auth(graph, &tenant, request, now).await?;
        graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?
            .sign_out_session(identity.user_id(), identity.session_id(), now)
            .await
            .map_err(|_| unavailable(request, "session revocation is unavailable"))?;
        append_audit(
            graph,
            &tenant,
            AuditCategory::Authentication,
            application_actor(&identity),
            "application_session",
            identity.session_id().as_str(),
            "application_signout",
            AuditOutcome::Allowed,
            "session_revoked",
            request.request_id(),
            now,
        )
        .await?;
        Ok(HttpResponse::empty(204))
    })
}

fn handle_current_user(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let tenant = tenant_for(graph, request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let identity = verify_bearer(graph, &tenant, request, now).await?;
        charge_auth(graph, &tenant, request, now).await?;
        let store = graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?;
        let user = store
            .user_by_id(identity.user_id())
            .await
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?
            .ok_or_else(|| unauthenticated(request, "application user is unavailable"))?;
        let email = store
            .email_for_user(user.id())
            .await
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?
            .ok_or_else(|| unavailable(request, "application user email is unavailable"))?;
        append_audit(
            graph,
            &tenant,
            AuditCategory::Authentication,
            application_actor(&identity),
            "application_user",
            user.id().as_str(),
            "application_user_read",
            AuditOutcome::Allowed,
            "verified_access_token",
            request.request_id(),
            now,
        )
        .await?;
        json(
            request,
            200,
            &AuthUserWire {
                id: user.id().as_str().to_owned(),
                email: email.as_str().to_owned(),
                status: status_wire(user.status()),
                authorization_epoch: identity.authorization_epochs().user,
            },
        )
    })
}

pub(crate) async fn verify_bearer(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    request: &HttpRequest,
    now_unix_seconds: u64,
) -> Result<VerifiedAccessIdentity, HttpApiError> {
    let header = request
        .header(AUTHORIZATION_HEADER)
        .ok_or_else(|| unauthenticated(request, "application access token is required"))?;
    let mut parts = header.split_ascii_whitespace();
    let scheme = parts
        .next()
        .ok_or_else(|| unauthenticated(request, "application access token is required"))?;
    let token = parts
        .next()
        .ok_or_else(|| unauthenticated(request, "application access token is required"))?;
    if !scheme.eq_ignore_ascii_case("bearer") || parts.next().is_some() {
        return Err(unauthenticated(
            request,
            "application access token is invalid",
        ));
    }
    graph
        .verify_access_token(token, tenant, now_unix_seconds)
        .await
        .map_err(|_| unauthenticated(request, "application access token is invalid"))
}

pub(crate) async fn verify_public_key(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    request: &HttpRequest,
    now_unix_seconds: u64,
) -> Result<(), HttpApiError> {
    let presented = request
        .header(PUBLIC_KEY_HEADER)
        .ok_or_else(|| unauthenticated(request, "public project key is required"))?;
    let verified = graph
        .identity_store(tenant, tenant)
        .map_err(|_| unavailable(request, "project credentials are unavailable"))?
        .verify_project_credential(presented, now_unix_seconds)
        .await
        .map_err(|_| unavailable(request, "project credentials are unavailable"))?;
    if !matches!(verified, Some(VerifiedProjectCredential::Public(public)) if public.scope() == tenant)
    {
        return Err(unauthenticated(request, "public project key is invalid"));
    }
    Ok(())
}

pub(crate) async fn charge_auth(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    request: &HttpRequest,
    now_unix_seconds: u64,
) -> Result<(), HttpApiError> {
    let reservation = format!("{}.auth", request.request_id());
    let charges = [GatewayQuotaCharge {
        resource: GatewayQuotaResource::AuthenticationRequests,
        amount: NonZeroU64::new(1).expect("one is non-zero"),
    }];
    match graph
        .quota_engine()
        .check_and_reserve(
            tenant,
            &reservation,
            &charges,
            // Resolved per tenant, so an operator override or a plan actually
            // changes what this tenant is held to.
            &graph
                .quota_policies()
                .policy_for(tenant)
                .await
                .map_err(|_| unavailable(request, "quota authority is unavailable"))?,
            now_unix_seconds.saturating_mul(1_000),
        )
        .await
        .map_err(|_| unavailable(request, "quota authority is unavailable"))?
    {
        GatewayQuotaDecision::Allowed => Ok(()),
        GatewayQuotaDecision::Throttled {
            retry_after_milliseconds,
            ..
        } => Err(HttpApiError::new(
            429,
            ErrorCode::RateLimited,
            "authentication request rate exceeded",
            request.request_id(),
            RetryAdvice::AfterDelay {
                after_ms: retry_after_milliseconds,
            },
        )),
        GatewayQuotaDecision::HardLimit { .. } => Err(HttpApiError::new(
            429,
            ErrorCode::QuotaExceeded,
            "authentication request quota exceeded",
            request.request_id(),
            RetryAdvice::Never,
        )),
    }
}

struct DisabledEmailProvider;

impl TransactionalEmailProvider for DisabledEmailProvider {
    fn enqueue_verification(&self, _email: VerificationEmail) {}
}

pub(crate) struct DataPlaneAuthenticationAudit {
    graph: Arc<DataPlaneGraph>,
}

impl DataPlaneAuthenticationAudit {
    #[cfg(test)]
    pub(crate) fn new(graph: Arc<DataPlaneGraph>) -> Self {
        Self { graph }
    }
}

impl AuthenticationAuditSink for DataPlaneAuthenticationAudit {
    fn record(&self, event: &AuthenticationAuditEvent) -> Result<(), AuthenticationAuditError> {
        let (outcome, reason) = match event.outcome() {
            AuthenticationAuditOutcome::Succeeded => (AuditOutcome::Allowed, "authenticated"),
            AuthenticationAuditOutcome::InvalidCredentials => {
                (AuditOutcome::Denied, "invalid_credentials")
            }
            AuthenticationAuditOutcome::Throttled => (AuditOutcome::Denied, "rate_limited"),
        };
        let resource_id = event.actor().map_or("signin", |actor| actor.as_str());
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    block_on(append_audit(
                        &self.graph,
                        event.tenant(),
                        AuditCategory::Authentication,
                        ActorIdentity::Anonymous,
                        "application_auth",
                        resource_id,
                        "application_signin",
                        outcome,
                        reason,
                        event.request_id(),
                        event.occurred_at_unix_seconds(),
                    ))
                })
                .join()
        })
        .map_err(|_| AuthenticationAuditError::new("authentication audit worker failed"))?
        .map_err(|_| AuthenticationAuditError::new("authentication audit is unavailable"))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn append_audit(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    category: AuditCategory,
    actor: ActorIdentity,
    resource_kind: &str,
    resource_id: &str,
    action: &str,
    outcome: AuditOutcome,
    reason: &str,
    request_id: &str,
    now_unix_seconds: u64,
) -> Result<(), HttpApiError> {
    append_audit_with_details(
        graph,
        tenant,
        category,
        actor,
        resource_kind,
        resource_id,
        action,
        outcome,
        reason,
        request_id,
        now_unix_seconds,
        SafeAttributes::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn append_audit_with_details(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    category: AuditCategory,
    actor: ActorIdentity,
    resource_kind: &str,
    resource_id: &str,
    action: &str,
    outcome: AuditOutcome,
    reason: &str,
    request_id: &str,
    now_unix_seconds: u64,
    details: SafeAttributes,
) -> Result<(), HttpApiError> {
    let request = RequestId::parse(request_id)
        .map_err(|_| internal_from_id(request_id, "audit context is invalid"))?;
    let correlation = CorrelationId::parse(request_id)
        .map_err(|_| internal_from_id(request_id, "audit context is invalid"))?;
    let event_id = audit_event_id(action, resource_kind, resource_id, request_id);
    let application_user_id = match &actor {
        ActorIdentity::ApplicationUser { actor_id, .. } => Some(actor_id.clone()),
        _ => None,
    };
    let organization_id = authority_organization_id(tenant);
    let context = SignalContext::new(
        SignalScope::Tenant {
            tenant: tenant.clone(),
            organization_id: Some(organization_id),
        },
        actor,
        ResourceReference::new(resource_kind, resource_id)
            .map_err(|_| internal_from_id(request_id, "audit context is invalid"))?,
        request,
        correlation,
        None,
    )
    .map_err(|_| internal_from_id(request_id, "audit context is invalid"))?;
    // Every authenticated outcome the audit trail records is also an
    // observability signal, and this is the one place all of them pass
    // through. Emitting is best effort: it must never fail the request.
    if category == AuditCategory::Authentication {
        graph.telemetry().record(mako_api::ObservabilityRecord {
            tenant: tenant.clone(),
            timestamp_unix_milliseconds: now_unix_seconds.saturating_mul(1_000),
            payload: mako_api::ObservabilityPayload::AuthenticationEvent {
                category: resource_kind.to_owned(),
                outcome: match outcome {
                    AuditOutcome::Allowed => mako_api::EventOutcome::Allowed,
                    AuditOutcome::Denied => mako_api::EventOutcome::Denied,
                    _ => mako_api::EventOutcome::Failed,
                },
                application_user_id: application_user_id.clone(),
                message: action.to_owned(),
                correlation_id: request_id.to_owned(),
            },
        });
    }
    graph
        .audit_store()
        .append(
            tenant,
            category,
            AuditEvent {
                context,
                event_id,
                occurred_at_unix_milliseconds: now_unix_seconds.saturating_mul(1_000),
                action: action.to_owned(),
                outcome,
                reason_code: reason.to_owned(),
                details,
            },
            graph.telemetry_redactor(),
        )
        .await
        .map(|_| ())
        .map_err(|_| internal_from_id(request_id, "audit write is unavailable"))
}

fn authority_organization_id(tenant: &TenantScope) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tenant.project_id().as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(tenant.environment_id().as_str().as_bytes());
    format!(
        "org_project_authority_{}",
        &hasher.finalize().to_hex()[..16]
    )
}

/// The identity of one audited event.
///
/// The resource is part of it because one request may audit more than one:
/// a live stream carrying several collections opens each of them, and each
/// opening is its own event about its own collection. Deriving the id from
/// the action and the request alone made the second look to the audit store
/// like the first being written again with a different body, which it
/// refused -- correctly -- and the stream failed with `audit write is
/// unavailable`. A genuine retry of the same action on the same resource
/// under the same request id still coalesces, which is what the id is for.
fn audit_event_id(
    action: &str,
    resource_kind: &str,
    resource_id: &str,
    request_id: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [action, resource_kind, resource_id, request_id] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    format!("evt_{}", &hasher.finalize().to_hex()[..32])
}

fn application_actor(identity: &VerifiedAccessIdentity) -> ActorIdentity {
    ActorIdentity::ApplicationUser {
        actor_id: identity.user_id().as_str().to_owned(),
        session_id: identity.session_id().as_str().to_owned(),
    }
}

pub(crate) fn session_wire(
    request: &HttpRequest,
    grant: DataPlaneSessionGrant,
) -> Result<AuthSessionWire, HttpApiError> {
    let email = grant
        .email
        .ok_or_else(|| unavailable(request, "application user email is unavailable"))?;
    Ok(AuthSessionWire {
        access_token: grant
            .access_token
            .expose_for_authorization_header()
            .to_owned(),
        refresh_token: grant
            .refresh_credential
            .expose_for_token_response()
            .to_owned(),
        expires_in: grant.expires_in_seconds,
        user: AuthUserWire {
            id: grant.user.id().as_str().to_owned(),
            email: email.as_str().to_owned(),
            status: status_wire(grant.user.status()),
            authorization_epoch: grant.authorization_epoch,
        },
    })
}

fn validate_email_password(
    request: &HttpRequest,
    email: &str,
    password: &str,
    minimum_password_length: usize,
) -> Result<(), HttpApiError> {
    if email.is_empty()
        || email.len() > 320
        || email.chars().any(char::is_control)
        || !(minimum_password_length..=1_024).contains(&password.len())
        || password.chars().any(char::is_control)
    {
        return Err(invalid(request, "email or password is invalid"));
    }
    Ok(())
}

const fn status_wire(status: AppUserStatus) -> &'static str {
    match status {
        AppUserStatus::PendingVerification => "unverified",
        AppUserStatus::Active => "active",
        AppUserStatus::Disabled => "disabled",
        AppUserStatus::Deleted => "deleted",
    }
}

pub(crate) fn tenant(request: &HttpRequest) -> Result<TenantScope, HttpApiError> {
    TenantScope::require(
        request.path_parameter("projectId"),
        request.path_parameter("environmentId"),
    )
    .map_err(|_| invalid(request, "tenant path is invalid"))
}

/// Set by the reverse proxy on the custom-domain listener and stripped on
/// the platform's own hostname.
pub(crate) const CUSTOM_DOMAIN_HEADER: &str = "x-mako-custom-domain";

/// The path's tenant, on a hostname that may serve it: every application
/// route resolves its tenant here, so a request that arrived on a custom
/// domain is refused unless that domain is verified for exactly this
/// environment. Nothing is said about whether the environment exists.
pub(crate) fn tenant_for(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
) -> Result<TenantScope, HttpApiError> {
    let tenant = tenant(request)?;
    require_custom_domain(graph, request, &tenant)?;
    Ok(tenant)
}

pub(crate) fn require_custom_domain(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
) -> Result<(), HttpApiError> {
    let Some(header) = request.header(CUSTOM_DOMAIN_HEADER) else {
        return Ok(());
    };
    let permitted = normalize_custom_domain(header)
        .is_some_and(|hostname| graph.custom_domains().permits(tenant, &hostname));
    if permitted {
        Ok(())
    } else {
        Err(not_found(
            request,
            "resource was not found on this hostname",
        ))
    }
}

/// The header value as the control plane stores hostnames: lowercase,
/// without a port or trailing dot; `None` if it is not a name.
fn normalize_custom_domain(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let without_port = trimmed
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
        .map_or(trimmed, |(host, _)| host);
    let hostname = without_port
        .strip_suffix('.')
        .unwrap_or(without_port)
        .to_ascii_lowercase();
    let valid = !hostname.is_empty()
        && hostname.len() <= 253
        && hostname.contains('.')
        && hostname.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        });
    valid.then_some(hostname)
}

pub(crate) fn now_unix_seconds(request_id: &str) -> Result<u64, HttpApiError> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| internal_from_id(request_id, "system clock is unavailable"))
}

pub(crate) fn require_json(request: &HttpRequest) -> Result<(), HttpApiError> {
    let content_type = request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type != Some(JSON_CONTENT_TYPE) {
        return Err(invalid(request, "content type must be application/json"));
    }
    Ok(())
}

pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(
    request: &HttpRequest,
) -> Result<T, HttpApiError> {
    serde_json::from_slice(request.body()).map_err(|_| invalid(request, "JSON body is invalid"))
}

pub(crate) fn json<T: Serialize>(
    request: &HttpRequest,
    status: u16,
    value: &T,
) -> Result<HttpResponse, HttpApiError> {
    HttpResponse::json(status, value)
        .map_err(|_| internal_from_id(request.request_id(), "response serialization failed"))
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

pub(crate) fn not_found(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        404,
        ErrorCode::NotFound,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

pub(crate) fn unauthenticated(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        401,
        ErrorCode::Unauthenticated,
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

pub(crate) fn internal_from_id(request_id: &str, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        500,
        ErrorCode::Internal,
        message,
        request_id,
        RetryAdvice::Never,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SignUpWire {
    email: String,
    password: String,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SignUpAcceptedWire {
    accepted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PasswordSignInWire {
    email: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RefreshWire {
    refresh_token: String,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AuthSessionWire {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    user: AuthUserWire,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AuthUserWire {
    id: String,
    email: String,
    status: &'static str,
    authorization_epoch: u64,
}
