use std::{collections::BTreeMap, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::executor::block_on;
use mako_api::{ErrorCode, RetryAdvice, TenantScope};
use mako_audit::{ActorIdentity, AuditCategory, AuditOutcome};
use mako_auth_providers::{AuthProviderSettings, FlowStateError, ProviderExchangeError};
use mako_identity::{
    AppUserId, AppUserRecord, AppUserStatus, CredentialDigest, IdentityProvider, MagicLinkOutcome,
    NormalizedEmail, TrustedAppMetadata, UserCredentialId, UserCredentialKind,
    UserCredentialRecord, UserIdentityId, UserIdentityRecord, UserProfileMetadata,
};
use mako_internal_rpc::{
    IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission,
    InstallAuthProvidersInput,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, TenantKeyspace,
    WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{application_mail, auth_http, graph::DataPlaneGraph, internal_http};

const BASE: &str = "/v1/projects/{projectId}/environments/{environmentId}";
const EXCHANGE_DOMAIN_PREFIX: &str = "mako:auth-exchange-codes:v1";
const EXCHANGE_CODE_LIFETIME_SECONDS: u64 = 120;
const SETTINGS_DOMAIN_PREFIX: &str = "mako:auth-providers:v1";

/// The installed settings of one environment, read on every provider call.
#[derive(Clone)]
pub struct AuthProviderSettingsStore {
    adapter: Arc<dyn KvAdapter>,
}

impl AuthProviderSettingsStore {
    #[must_use]
    pub fn new(adapter: Arc<dyn KvAdapter>) -> Self {
        Self { adapter }
    }

    fn key(tenant: &TenantScope) -> Result<Vec<u8>, ()> {
        TenantKeyspace::system_key(
            format!(
                "{SETTINGS_DOMAIN_PREFIX}:{}:{}",
                tenant.project_id(),
                tenant.environment_id()
            )
            .into_bytes(),
            "current",
        )
        .map_err(|_| ())
    }

    pub async fn load(&self, tenant: &TenantScope) -> Result<Option<AuthProviderSettings>, ()> {
        let key = Self::key(tenant)?;
        let Some(bytes) = self.adapter.get(&key).await.map_err(|_| ())? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|_| ())
    }

    pub async fn install(
        &self,
        tenant: &TenantScope,
        settings: &AuthProviderSettings,
    ) -> Result<(), ()> {
        let key = Self::key(tenant)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(settings).map_err(|_| ())?);
        self.adapter
            .write(batch, Durability::Sync)
            .await
            .map_err(|_| ())
    }
}

/// Strips sealed secrets for anything that leaves the data plane.
fn public_view(settings: &AuthProviderSettings) -> serde_json::Value {
    json!({
        "providers": settings.providers.iter().map(|provider| json!({
            "name": provider.name,
            "kind": provider.kind,
            "clientId": provider.client_id,
            "scopes": provider.scopes,
            "enabled": provider.enabled,
            "hasSecret": !provider.client_secret.ciphertext.is_empty(),
        })).collect::<Vec<_>>(),
        "redirectUrls": settings.redirect_urls,
        "magicLinks": settings.magic_links,
        "version": settings.version,
    })
}

pub(crate) async fn execute_auth_provider_operation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    let store = AuthProviderSettingsStore::new(Arc::clone(graph.storage_adapter()));
    let body = match command.operation {
        IdentityAdminOperation::InstallAuthProviders => {
            internal_http::require_permission(
                graph,
                request,
                tenant,
                command,
                IdentityAdminPermission::ManageProjectCredentials,
                "auth_providers_install",
                "auth-providers",
                now,
            )
            .await?;
            let input: InstallAuthProvidersInput =
                internal_http::parse_input(request, &command.input)?;
            let mut settings: AuthProviderSettings = serde_json::from_value(input.settings)
                .map_err(|_| auth_http::invalid(request, "sign-in settings are invalid"))?;
            // A provider sent with an empty sealed secret keeps the secret that is
            // already installed under the same name; validation rejects the case
            // where nothing is installed to keep.
            if settings.providers.iter().any(|provider| {
                provider.client_secret.nonce.is_empty()
                    && provider.client_secret.ciphertext.is_empty()
            }) && let Some(installed) = store.load(tenant).await.map_err(|()| {
                auth_http::unavailable(request, "sign-in settings storage is unavailable")
            })? {
                for provider in &mut settings.providers {
                    if provider.client_secret.nonce.is_empty()
                        && provider.client_secret.ciphertext.is_empty()
                        && let Some(current) = installed
                            .providers
                            .iter()
                            .find(|current| current.name == provider.name)
                    {
                        provider.client_secret = current.client_secret.clone();
                    }
                }
            }
            settings.validate().map_err(|error| {
                HttpApiError::new(
                    400,
                    mako_api::ErrorCode::InvalidRequest,
                    error.to_string(),
                    request.request_id(),
                    mako_api::RetryAdvice::Never,
                )
            })?;
            store.install(tenant, &settings).await.map_err(|()| {
                auth_http::unavailable(request, "sign-in settings storage is unavailable")
            })?;
            public_view(&settings)
        }
        IdentityAdminOperation::InspectAuthProviders => {
            internal_http::require_permission(
                graph,
                request,
                tenant,
                command,
                IdentityAdminPermission::ReadProjectCredentials,
                "auth_providers_inspect",
                "auth-providers",
                now,
            )
            .await?;
            match store.load(tenant).await.map_err(|()| {
                auth_http::unavailable(request, "sign-in settings storage is unavailable")
            })? {
                Some(settings) => public_view(&settings),
                None => public_view(&AuthProviderSettings {
                    providers: Vec::new(),
                    redirect_urls: Vec::new(),
                    magic_links: mako_auth_providers::MagicLinkSettings::default(),
                    version: 0,
                }),
            }
        }
        _ => {
            return Err(auth_http::invalid(
                request,
                "identity operation dispatch is invalid",
            ));
        }
    };
    serde_json::to_vec(&body)
        .map_err(|_| auth_http::unavailable(request, "response could not be encoded"))
}

// ---- application routes ------------------------------------------------------

pub fn add_auth_provider_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    let start_graph = Arc::clone(&graph);
    router.add_route(
        HttpMethod::Post,
        &format!("{BASE}/auth/providers/{{provider}}/start"),
        move |request| handle_start(&start_graph, &request),
    )?;
    let callback_graph = Arc::clone(&graph);
    router.add_route(
        HttpMethod::Get,
        &format!("{BASE}/auth/providers/{{provider}}/callback"),
        move |request| handle_callback(&callback_graph, &request),
    )?;
    let exchange_graph = Arc::clone(&graph);
    router.add_route(
        HttpMethod::Post,
        &format!("{BASE}/auth/providers/exchange"),
        move |request| handle_exchange(&exchange_graph, &request),
    )?;
    let magic_graph = Arc::clone(&graph);
    router.add_route(
        HttpMethod::Post,
        &format!("{BASE}/auth/magic-link"),
        move |request| handle_magic_link_request(&magic_graph, &request),
    )?;
    router.add_route(
        HttpMethod::Post,
        &format!("{BASE}/auth/magic-link/redeem"),
        move |request| handle_magic_link_redeem(&graph, &request),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StartWire {
    redirect_url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExchangeWire {
    code: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MagicLinkRequestWire {
    email: String,
    redirect_url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MagicLinkRedeemWire {
    token: String,
}

async fn settings_for(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
) -> Result<AuthProviderSettings, HttpApiError> {
    AuthProviderSettingsStore::new(Arc::clone(graph.storage_adapter()))
        .load(tenant)
        .await
        .map_err(|()| auth_http::unavailable(request, "sign-in settings are unavailable"))?
        .ok_or_else(|| {
            forbidden(
                request,
                "no sign-in providers are enabled for this environment",
            )
        })
}

fn callback_uri(graph: &DataPlaneGraph, tenant: &TenantScope, provider: &str) -> String {
    format!(
        "{}/v1/projects/{}/environments/{}/auth/providers/{provider}/callback",
        graph.public_url().trim_end_matches('/'),
        tenant.project_id(),
        tenant.environment_id()
    )
}

fn provider_name(request: &HttpRequest) -> Result<String, HttpApiError> {
    let name = request
        .path_parameter("provider")
        .ok_or_else(|| auth_http::invalid(request, "provider is required"))?;
    if !mako_auth_providers::valid_provider_name(name) {
        return Err(auth_http::invalid(request, "provider name is invalid"));
    }
    Ok(name.to_owned())
}

/// The application asks for a flow: it names where the browser should come
/// back to, which must be one of the environment's registered redirects.
fn handle_start(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    auth_http::require_json(request)?;
    let tenant = auth_http::tenant_for(graph, request)?;
    let provider_name = provider_name(request)?;
    let now = auth_http::now_unix_seconds(request.request_id())?;
    block_on(async {
        auth_http::verify_public_key(graph, &tenant, request, now).await?;
        auth_http::charge_auth(graph, &tenant, request, now).await?;
        let body: StartWire = auth_http::parse_json(request)?;
        let settings = settings_for(graph, request, &tenant).await?;
        let provider = settings
            .provider(&provider_name)
            .ok_or_else(|| forbidden(request, "sign-in provider is not enabled"))?;
        if !settings.admits_redirect(&body.redirect_url) {
            return Err(auth_http::invalid(
                request,
                "redirect url is not registered for this environment",
            ));
        }
        let (state, token) = graph
            .flow_state()
            .issue(&tenant, &provider_name, &body.redirect_url, now)
            .map_err(|_| auth_http::unavailable(request, "sign-in flow could not start"))?;
        let authorization_url = graph
            .provider_client()
            .authorization_url(
                provider,
                &callback_uri(graph, &tenant, &provider_name),
                &token,
                &state.nonce,
            )
            .map_err(|error| provider_error(request, &error))?;
        auth_http::json(
            request,
            200,
            &json!({ "authorizationUrl": authorization_url, "provider": provider_name }),
        )
    })
}

/// The provider sends the browser here. Nothing on this request is trusted
/// until the state verifies; after that the code is redeemed with the
/// provider, the person is matched to an application user, and the browser is
/// sent back to the registered redirect with a one-time code.
fn handle_callback(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let tenant = auth_http::tenant_for(graph, request)?;
    let provider_name = provider_name(request)?;
    let now = auth_http::now_unix_seconds(request.request_id())?;
    let query: BTreeMap<&str, &str> = request
        .query()
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let state_token = query
        .get("state")
        .copied()
        .ok_or_else(|| auth_http::invalid(request, "state is required"))?;
    let state = match graph.flow_state().verify(&tenant, state_token, now) {
        Ok(state) => state,
        Err(FlowStateError::Expired) => {
            return Err(forbidden(request, "sign-in flow has expired; start again"));
        }
        Err(_) => return Err(forbidden(request, "sign-in flow state is invalid")),
    };
    if state.provider != provider_name {
        return Err(forbidden(
            request,
            "sign-in flow state names another provider",
        ));
    }
    // From here on the redirect is trusted, so failures go back to the
    // application as a stable error rather than as a bare JSON page.
    let outcome = block_on(complete_callback(
        graph,
        request,
        &tenant,
        &provider_name,
        &state,
        &query,
        now,
    ));
    match outcome {
        Ok(code) => redirect(request, &state.redirect_url, &format!("code={code}")),
        Err(CallbackFailure::Denied(reason)) => {
            redirect(request, &state.redirect_url, &format!("error={reason}"))
        }
        Err(CallbackFailure::Http(error)) => Err(error),
    }
}

enum CallbackFailure {
    /// Reported to the application on its redirect with a stable reason.
    Denied(&'static str),
    Http(HttpApiError),
}

impl From<HttpApiError> for CallbackFailure {
    fn from(error: HttpApiError) -> Self {
        Self::Http(error)
    }
}

async fn complete_callback(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &TenantScope,
    provider_name: &str,
    state: &mako_auth_providers::FlowState,
    query: &BTreeMap<&str, &str>,
    now: u64,
) -> Result<String, CallbackFailure> {
    auth_http::charge_auth(graph, tenant, request, now).await?;
    if let Some(error) = query.get("error") {
        audit_provider(
            graph,
            tenant,
            provider_name,
            None,
            AuditOutcome::Denied,
            "provider_refused",
            request,
            now,
        )
        .await;
        let _ = error;
        return Err(CallbackFailure::Denied("provider_refused"));
    }
    let code = query
        .get("code")
        .copied()
        .ok_or(CallbackFailure::Denied("code_missing"))?;
    let settings = settings_for(graph, request, tenant).await?;
    let provider = settings
        .provider(provider_name)
        .ok_or(CallbackFailure::Denied("provider_disabled"))?;
    let client_secret = graph
        .provider_secret_key()
        .open(tenant, provider_name, &provider.client_secret)
        .map_err(|_| {
            CallbackFailure::Http(auth_http::unavailable(
                request,
                "provider secret is unavailable",
            ))
        })?;
    let identity = match graph.provider_client().complete(
        provider,
        &client_secret,
        code,
        &callback_uri(graph, tenant, provider_name),
        &state.nonce,
        now,
    ) {
        Ok(identity) => identity,
        Err(ProviderExchangeError::Unavailable) => {
            return Err(CallbackFailure::Http(auth_http::unavailable(
                request,
                "sign-in provider is unavailable",
            )));
        }
        Err(error) => {
            audit_provider(
                graph,
                tenant,
                provider_name,
                None,
                AuditOutcome::Denied,
                "provider_exchange_failed",
                request,
                now,
            )
            .await;
            let _ = error;
            return Err(CallbackFailure::Denied("exchange_failed"));
        }
    };
    let store = graph.identity_store(tenant, tenant).map_err(|_| {
        CallbackFailure::Http(auth_http::unavailable(
            request,
            "identity storage is unavailable",
        ))
    })?;
    let unavailable = || {
        CallbackFailure::Http(auth_http::unavailable(
            request,
            "identity storage is unavailable",
        ))
    };
    // Match by provider subject first: that is the identity the provider vouches for.
    let user = match store
        .user_by_provider(provider_name, &identity.subject)
        .await
        .map_err(|_| unavailable())?
    {
        Some(user) => user,
        None => {
            // Link to an existing user only by an email the provider verified;
            // an unverified address proves nothing and creates nothing.
            let email = match (&identity.email, identity.email_verified) {
                (Some(email), true) => NormalizedEmail::parse(email)
                    .map_err(|_| CallbackFailure::Denied("email_invalid"))?,
                _ => {
                    audit_provider(
                        graph,
                        tenant,
                        provider_name,
                        None,
                        AuditOutcome::Denied,
                        "email_unverified",
                        request,
                        now,
                    )
                    .await;
                    return Err(CallbackFailure::Denied("email_unverified"));
                }
            };
            let provider_identity = |user_id: AppUserId| {
                UserIdentityRecord::new(
                    tenant.clone(),
                    UserIdentityId::parse(random_id("idn")).expect("identity id"),
                    user_id,
                    IdentityProvider::oidc(provider_name).expect("validated provider name"),
                    identity.subject.clone(),
                    now,
                )
                .map_err(|_| CallbackFailure::Denied("identity_invalid"))
            };
            match store
                .user_by_email(&email)
                .await
                .map_err(|_| unavailable())?
            {
                Some(existing) => {
                    if !matches!(
                        existing.status(),
                        AppUserStatus::Active | AppUserStatus::PendingVerification
                    ) {
                        audit_provider(
                            graph,
                            tenant,
                            provider_name,
                            Some(existing.id()),
                            AuditOutcome::Denied,
                            "user_disabled",
                            request,
                            now,
                        )
                        .await;
                        return Err(CallbackFailure::Denied("user_disabled"));
                    }
                    let link = provider_identity(existing.id().clone())?;
                    match store.link_provider_identity(existing.id(), &link).await {
                        Ok(()) => {}
                        Err(mako_identity::IdentityStoreError::ProviderIdentityAlreadyLinked) => {}
                        Err(_) => return Err(unavailable()),
                    }
                    existing
                }
                None => {
                    let user_id = AppUserId::parse(random_id("usr")).expect("user id");
                    let user = AppUserRecord::new(
                        tenant.clone(),
                        user_id.clone(),
                        AppUserStatus::Active,
                        TrustedAppMetadata::new(json!({})).expect("empty metadata"),
                        UserProfileMetadata::new(json!({
                            "email": email.as_str(),
                            "name": identity.display_name,
                            "provider": provider_name,
                        }))
                        .map_err(|_| CallbackFailure::Denied("profile_invalid"))?,
                        now,
                    );
                    let link = provider_identity(user_id)?;
                    match store.create_provider_user(&user, &link, &email).await {
                        Ok(()) => user,
                        Err(mako_identity::IdentityStoreError::EmailAlreadyExists) => {
                            // Raced with a signup for the same address; the next attempt links.
                            return Err(CallbackFailure::Denied("retry"));
                        }
                        Err(_) => return Err(unavailable()),
                    }
                }
            }
        }
    };
    if !matches!(
        user.status(),
        AppUserStatus::Active | AppUserStatus::PendingVerification
    ) {
        audit_provider(
            graph,
            tenant,
            provider_name,
            Some(user.id()),
            AuditOutcome::Denied,
            "user_disabled",
            request,
            now,
        )
        .await;
        return Err(CallbackFailure::Denied("user_disabled"));
    }
    let code = issue_exchange_code(graph, request, tenant, user.id(), now).await?;
    audit_provider(
        graph,
        tenant,
        provider_name,
        Some(user.id()),
        AuditOutcome::Allowed,
        "provider_verified",
        request,
        now,
    )
    .await;
    Ok(code)
}

/// The application trades the callback's one-time code for a session, with
/// its public key, exactly as password sign-in answers.
fn handle_exchange(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    auth_http::require_json(request)?;
    let tenant = auth_http::tenant_for(graph, request)?;
    let now = auth_http::now_unix_seconds(request.request_id())?;
    block_on(async {
        auth_http::verify_public_key(graph, &tenant, request, now).await?;
        auth_http::charge_auth(graph, &tenant, request, now).await?;
        let body: ExchangeWire = auth_http::parse_json(request)?;
        let user_id = redeem_exchange_code(graph, request, &tenant, &body.code, now)
            .await?
            .ok_or_else(|| {
                auth_http::unauthenticated(request, "sign-in code is invalid or already used")
            })?;
        let grant = graph
            .create_application_session(&tenant, &user_id, now)
            .await
            .map_err(|_| auth_http::unauthenticated(request, "application user cannot sign in"))?;
        auth_http::append_audit(
            graph,
            &tenant,
            AuditCategory::Authentication,
            ActorIdentity::ApplicationUser {
                actor_id: user_id.as_str().to_owned(),
                session_id: grant.session_id.as_str().to_owned(),
            },
            "application_auth",
            "provider",
            "application_provider_signin",
            AuditOutcome::Allowed,
            "exchange_code_redeemed",
            request.request_id(),
            now,
        )
        .await?;
        auth_http::json(request, 200, &auth_http::session_wire(request, grant)?)
    })
}

/// Asks for a magic link. The answer never says whether the address exists;
/// when it does, a single-use link goes out by mail to the registered redirect.
fn handle_magic_link_request(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    auth_http::require_json(request)?;
    let tenant = auth_http::tenant_for(graph, request)?;
    let now = auth_http::now_unix_seconds(request.request_id())?;
    block_on(async {
        auth_http::verify_public_key(graph, &tenant, request, now).await?;
        auth_http::charge_auth(graph, &tenant, request, now).await?;
        let body: MagicLinkRequestWire = auth_http::parse_json(request)?;
        let settings = settings_for(graph, request, &tenant).await?;
        if !settings.magic_links.enabled {
            return Err(forbidden(
                request,
                "magic links are not enabled for this environment",
            ));
        }
        if !settings.admits_redirect(&body.redirect_url) {
            return Err(auth_http::invalid(
                request,
                "redirect url is not registered for this environment",
            ));
        }
        let accepted = json!({ "accepted": true });
        let Ok(email) = NormalizedEmail::parse(&body.email) else {
            return auth_http::json(request, 202, &accepted);
        };
        let store = graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| auth_http::unavailable(request, "identity storage is unavailable"))?;
        // A magic link is also how a new user signs up: the address is not
        // proven until the link is opened, so the user starts pending and is
        // activated by redemption. The answer is the same either way.
        let user = match store
            .user_by_email(&email)
            .await
            .map_err(|_| auth_http::unavailable(request, "identity storage is unavailable"))?
        {
            Some(user) => user,
            None => match create_pending_user(&store, &tenant, &email, now).await {
                Ok(user) => user,
                Err(PendingUserFailure::Raced) => {
                    match store.user_by_email(&email).await.map_err(|_| {
                        auth_http::unavailable(request, "identity storage is unavailable")
                    })? {
                        Some(user) => user,
                        None => return auth_http::json(request, 202, &accepted),
                    }
                }
                Err(PendingUserFailure::Unavailable) => {
                    return Err(auth_http::unavailable(
                        request,
                        "identity storage is unavailable",
                    ));
                }
            },
        };
        if !matches!(
            user.status(),
            AppUserStatus::Active | AppUserStatus::PendingVerification
        ) {
            return auth_http::json(request, 202, &accepted);
        }
        let mut token_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut token_bytes);
        let token = URL_SAFE_NO_PAD.encode(token_bytes);
        let digest = blake3::hash(token.as_bytes());
        let expires_at = now.saturating_add(settings.magic_links.link_ttl_seconds);
        let credential = UserCredentialRecord::new(
            tenant.clone(),
            UserCredentialId::parse(random_id("mlk")).expect("credential id"),
            user.id().clone(),
            UserCredentialKind::MagicLink,
            CredentialDigest::new(digest.as_bytes().to_vec()).expect("digest"),
            now,
            Some(expires_at),
        );
        store
            .create_magic_link(&user, &credential, digest.as_bytes())
            .await
            .map_err(|_| auth_http::unavailable(request, "identity storage is unavailable"))?;
        let link = format!("{}#magic_link_token={token}", body.redirect_url);
        let variables = BTreeMap::from([
            ("link".to_owned(), link),
            ("expires_at".to_owned(), rfc3339(expires_at)),
            ("email".to_owned(), email.as_str().to_owned()),
        ]);
        // One mail per address per minute: the intent id derives from this token.
        let dedupe = format!("{}:{}", email.as_str(), now / 60);
        graph
            .application_mail()
            .enqueue(
                &tenant,
                application_mail::KIND_MAGIC_LINK,
                email.as_str(),
                variables,
                &dedupe,
                now,
            )
            .await
            .map_err(|_| auth_http::unavailable(request, "mail could not be queued"))?;
        auth_http::append_audit(
            graph,
            &tenant,
            AuditCategory::Authentication,
            ActorIdentity::Anonymous,
            "application_auth",
            "magic_link",
            "application_magic_link_requested",
            AuditOutcome::Allowed,
            "magic_link_queued",
            request.request_id(),
            now,
        )
        .await?;
        auth_http::json(request, 202, &accepted)
    })
}

fn handle_magic_link_redeem(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    auth_http::require_json(request)?;
    let tenant = auth_http::tenant_for(graph, request)?;
    let now = auth_http::now_unix_seconds(request.request_id())?;
    block_on(async {
        auth_http::verify_public_key(graph, &tenant, request, now).await?;
        auth_http::charge_auth(graph, &tenant, request, now).await?;
        let body: MagicLinkRedeemWire = auth_http::parse_json(request)?;
        if body.token.len() > 256 || body.token.is_empty() {
            return Err(auth_http::unauthenticated(request, "magic link is invalid"));
        }
        let store = graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| auth_http::unavailable(request, "identity storage is unavailable"))?;
        let digest = blake3::hash(body.token.as_bytes());
        let outcome = store
            .redeem_magic_link(digest.as_bytes(), now)
            .await
            .map_err(|_| auth_http::unavailable(request, "identity storage is unavailable"))?;
        let (user_id, reason) = match outcome {
            MagicLinkOutcome::Redeemed { user_id } => (user_id, "magic_link_redeemed"),
            MagicLinkOutcome::Expired => {
                return Err(auth_http::unauthenticated(
                    request,
                    "magic link has expired",
                ));
            }
            MagicLinkOutcome::AlreadyUsed | MagicLinkOutcome::Unknown => {
                return Err(auth_http::unauthenticated(
                    request,
                    "magic link is invalid or already used",
                ));
            }
        };
        let grant = graph
            .create_application_session(&tenant, &user_id, now)
            .await
            .map_err(|_| auth_http::unauthenticated(request, "application user cannot sign in"))?;
        auth_http::append_audit(
            graph,
            &tenant,
            AuditCategory::Authentication,
            ActorIdentity::ApplicationUser {
                actor_id: user_id.as_str().to_owned(),
                session_id: grant.session_id.as_str().to_owned(),
            },
            "application_auth",
            "magic_link",
            "application_magic_link_signin",
            AuditOutcome::Allowed,
            reason,
            request.request_id(),
            now,
        )
        .await?;
        auth_http::json(request, 200, &auth_http::session_wire(request, grant)?)
    })
}

// ---- one-time exchange codes -----------------------------------------------------

fn exchange_key(tenant: &TenantScope, code: &str) -> Result<Vec<u8>, ()> {
    TenantKeyspace::system_key(
        format!(
            "{EXCHANGE_DOMAIN_PREFIX}:{}:{}",
            tenant.project_id(),
            tenant.environment_id()
        )
        .into_bytes(),
        blake3::hash(code.as_bytes()).to_hex().as_str(),
    )
    .map_err(|_| ())
}

async fn issue_exchange_code(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    user_id: &AppUserId,
    now: u64,
) -> Result<String, HttpApiError> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let code = URL_SAFE_NO_PAD.encode(bytes);
    let key = exchange_key(tenant, &code)
        .map_err(|()| auth_http::unavailable(request, "sign-in code could not be issued"))?;
    let record = json!({ "userId": user_id.as_str(), "expiresAtUnixSeconds": now + EXCHANGE_CODE_LIFETIME_SECONDS });
    let mut batch = WriteBatch::new();
    batch.put(&key, serde_json::to_vec(&record).expect("record"));
    graph
        .storage_adapter()
        .write(batch, Durability::Sync)
        .await
        .map_err(|_| auth_http::unavailable(request, "sign-in code could not be issued"))?;
    Ok(code)
}

async fn redeem_exchange_code(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    code: &str,
    now: u64,
) -> Result<Option<AppUserId>, HttpApiError> {
    if code.is_empty() || code.len() > 128 {
        return Ok(None);
    }
    let key = exchange_key(tenant, code)
        .map_err(|()| auth_http::unavailable(request, "sign-in code storage is unavailable"))?;
    let Some(current) = graph
        .storage_adapter()
        .get(&key)
        .await
        .map_err(|_| auth_http::unavailable(request, "sign-in code storage is unavailable"))?
    else {
        return Ok(None);
    };
    let mut batch = WriteBatch::new();
    batch.delete(&key);
    let applied = graph
        .storage_adapter()
        .compare_and_write(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: current.clone(),
            }],
            batch,
            durability: Durability::Sync,
        })
        .await
        .map_err(|_| auth_http::unavailable(request, "sign-in code storage is unavailable"))?;
    if !matches!(applied, CompareAndWriteResult::Applied) {
        return Ok(None);
    }
    let record: Value = serde_json::from_slice(&current)
        .map_err(|_| auth_http::unavailable(request, "sign-in code is corrupt"))?;
    if record
        .get("expiresAtUnixSeconds")
        .and_then(Value::as_u64)
        .is_none_or(|expires| expires < now)
    {
        return Ok(None);
    }
    Ok(record
        .get("userId")
        .and_then(Value::as_str)
        .and_then(|id| AppUserId::parse(id).ok()))
}

// ---- helpers -------------------------------------------------------------------------

enum PendingUserFailure {
    /// Another request created a user for the address first.
    Raced,
    Unavailable,
}

/// Creates the user a first magic-link request is for, pending until the
/// link proves the address.
async fn create_pending_user(
    store: &mako_identity::IdentityStore,
    tenant: &TenantScope,
    email: &NormalizedEmail,
    now: u64,
) -> Result<AppUserRecord, PendingUserFailure> {
    let user_id =
        AppUserId::parse(random_id("usr")).map_err(|_| PendingUserFailure::Unavailable)?;
    let user = AppUserRecord::new(
        tenant.clone(),
        user_id.clone(),
        AppUserStatus::PendingVerification,
        TrustedAppMetadata::new(json!({})).map_err(|_| PendingUserFailure::Unavailable)?,
        UserProfileMetadata::new(json!({ "email": email.as_str() }))
            .map_err(|_| PendingUserFailure::Unavailable)?,
        now,
    );
    let identity = UserIdentityRecord::new(
        tenant.clone(),
        UserIdentityId::parse(random_id("idn")).map_err(|_| PendingUserFailure::Unavailable)?,
        user_id,
        IdentityProvider::Email,
        email.as_str(),
        now,
    )
    .map_err(|_| PendingUserFailure::Unavailable)?;
    match store.create_email_user(&user, &identity, email).await {
        Ok(()) => Ok(user),
        Err(mako_identity::IdentityStoreError::EmailAlreadyExists) => {
            Err(PendingUserFailure::Raced)
        }
        Err(_) => Err(PendingUserFailure::Unavailable),
    }
}

fn random_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    format!(
        "{prefix}_{}",
        blake3::hash(&bytes).to_hex().get(..24).unwrap_or("")
    )
}

fn rfc3339(unix_seconds: u64) -> String {
    // A minimal UTC formatter: templates show it, nothing parses it back.
    let days = unix_seconds / 86_400;
    let seconds = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn forbidden(request: &HttpRequest, message: &str) -> HttpApiError {
    HttpApiError::new(
        403,
        ErrorCode::PermissionDenied,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn provider_error(request: &HttpRequest, error: &ProviderExchangeError) -> HttpApiError {
    match error {
        ProviderExchangeError::Unavailable => {
            auth_http::unavailable(request, "sign-in provider is unavailable")
        }
        _ => HttpApiError::new(
            502,
            ErrorCode::Unavailable,
            error.to_string(),
            request.request_id(),
            RetryAdvice::Immediate,
        ),
    }
}

fn redirect(
    request: &HttpRequest,
    target: &str,
    fragment: &str,
) -> Result<HttpResponse, HttpApiError> {
    HttpResponse::empty(302)
        .with_header("location", &format!("{target}#{fragment}"))
        .map_err(|_| auth_http::unavailable(request, "redirect could not be composed"))
}

#[allow(clippy::too_many_arguments)]
async fn audit_provider(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    provider: &str,
    user_id: Option<&AppUserId>,
    outcome: AuditOutcome,
    reason: &str,
    request: &HttpRequest,
    now: u64,
) {
    let actor = match user_id {
        Some(user_id) => ActorIdentity::ApplicationUser {
            actor_id: user_id.as_str().to_owned(),
            session_id: "provider-callback".to_owned(),
        },
        None => ActorIdentity::Anonymous,
    };
    let _ = auth_http::append_audit(
        graph,
        tenant,
        AuditCategory::Authentication,
        actor,
        "application_auth",
        provider,
        "application_provider_callback",
        outcome,
        reason,
        request.request_id(),
        now,
    )
    .await;
}
