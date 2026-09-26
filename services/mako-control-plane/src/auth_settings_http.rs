//! Management routes for an environment's application sign-in settings:
//! external providers, the redirect allowlist, and magic links.
//!
//! The settings live in the data plane that mints application sessions; the
//! control plane keeps no copy. A read forwards one inspection and renders
//! what comes back, which never carries a secret. A replacement reads the
//! installed view first, seals every client secret the developer supplied
//! under the key both planes derive from the shared internal secret, marks
//! every omitted one as "keep what is installed", numbers the new version
//! after the installed one, and forwards one installation. A refusal the
//! data plane phrases for a developer crosses verbatim.

use std::sync::Arc;

use mako_api::{ErrorCode, RetryAdvice, TenantScope};
use mako_auth_providers::{
    AuthProviderSettings, EmailVerificationSettings, MagicLinkSettings, ProviderConfig,
    ProviderKind, ProviderSecretKey, SealedSecret, SettingsError,
};
use mako_control_plane::DeveloperPrincipal;
use mako_internal_rpc::{IdentityAdminCommand, IdentityAdminOperation, InternalClientError};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    ControlPlaneGraph,
    http_support::tenant,
    identity_admin_http::{self, identity_permissions},
    management_http::{
        forbidden, json, no_payload, no_query, parse_json, require_idempotency, require_json,
        unavailable, with_developer,
    },
};

const PATH: &str = "/v1/projects/{projectId}/environments/{environmentId}/auth-settings";

pub(crate) fn add_auth_settings_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    let get_graph = Arc::clone(&graph);
    router.add_route(HttpMethod::Get, PATH, move |request| {
        handle_get_settings(&get_graph, &request)
    })?;
    router.add_route(HttpMethod::Put, PATH, move |request| {
        handle_update_settings(&graph, &request)
    })
}

fn handle_get_settings(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, _| async move {
        let view = inspect(graph, request, &actor, &tenant).await?;
        json(request, 200, &view)
    })
}

fn handle_update_settings(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let body: AuthSettingsUpdateWire = parse_json(request)?;
    with_developer(graph, request, |actor, _| async move {
        // The installed view says which providers already hold a secret and
        // which version this replacement succeeds; both decide what is sent.
        let installed = inspect(graph, request, &actor, &tenant).await?;
        let settings =
            body.into_settings(request, graph.provider_secret_key(), &tenant, &installed)?;
        let view: AuthSettingsView = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::InstallAuthProviders,
            json!({ "settings": settings }),
            true,
        )
        .await?;
        json(request, 200, &view)
    })
}

async fn inspect(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
) -> Result<AuthSettingsView, HttpApiError> {
    administer(
        graph,
        request,
        actor,
        tenant,
        IdentityAdminOperation::InspectAuthProviders,
        json!({}),
        false,
    )
    .await
}

/// Forward one identity-admin operation as the developer, with the
/// permissions their team role grants, and read back the shape the data
/// plane promised for it.
async fn administer<T: DeserializeOwned>(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    operation: IdentityAdminOperation,
    input: Value,
    idempotent: bool,
) -> Result<T, HttpApiError> {
    let permissions = identity_permissions(graph, request, actor, tenant).await?;
    let command = IdentityAdminCommand {
        operation,
        actor_id: actor.identity_id().as_str().to_owned(),
        permissions,
        input,
    };
    let idempotency = if idempotent {
        require_idempotency(request)?
    } else {
        request.request_id()
    };
    let value: Value = graph
        .data_plane_identity_admin()
        .administer(tenant, request.request_id(), idempotency, &command)
        .map_err(|error| settings_error(request, error))?;
    serde_json::from_value(value).map_err(|_| {
        unavailable(
            request,
            "sign-in settings authority returned an unexpected response",
        )
    })
}

/// The data plane names what was wrong with a provider, a redirect, or a
/// lifetime in terms a developer can act on, and its envelope has already
/// been correlated to this request, so a client-side refusal crosses
/// verbatim. An authentication failure between the two planes is a
/// deployment fault, not the developer's, and reads as unavailability like
/// every other one.
fn settings_error(request: &HttpRequest, error: InternalClientError) -> HttpApiError {
    match error {
        // The only 403 the data plane sends is a permission this role lacks.
        InternalClientError::Remote { status: 403, .. } => {
            forbidden(request, identity_admin_http::ROLE_REFUSED)
        }
        InternalClientError::Remote { status, envelope }
            if (400..500).contains(&status) && status != 401 =>
        {
            HttpApiError::from_envelope(status, *envelope)
        }
        _ => unavailable(request, "sign-in settings administration is unavailable"),
    }
}

fn invalid_settings(request: &HttpRequest, message: impl Into<String>) -> HttpApiError {
    HttpApiError::new(
        400,
        ErrorCode::InvalidRequest,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

/// What the data plane is sent for a provider whose secret the developer did
/// not restate: an empty seal, which the installation replaces with the
/// secret already installed under the same name.
fn keep_installed_secret() -> SealedSecret {
    SealedSecret {
        nonce: String::new(),
        ciphertext: String::new(),
    }
}

fn keeps_installed_secret(secret: &SealedSecret) -> bool {
    secret.nonce.is_empty() && secret.ciphertext.is_empty()
}

/// The settings model refuses an empty seal, rightly, because it has no way
/// to know one stands for an installed secret. Everything else it checks —
/// names, client ids, issuers, scopes, redirects, the link lifetime — is
/// checked here as it will be there, with a stand-in seal where a secret is
/// being kept.
fn validate_with_kept_secrets(settings: &AuthProviderSettings) -> Result<(), SettingsError> {
    let mut checked = settings.clone();
    for provider in &mut checked.providers {
        if keeps_installed_secret(&provider.client_secret) {
            provider.client_secret = SealedSecret {
                nonce: "installed".to_owned(),
                ciphertext: "installed".to_owned(),
            };
        }
    }
    checked.validate()
}

/// A provider as the management wire shows it: everything but the secret,
/// and whether one is installed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthProviderView {
    name: String,
    kind: ProviderKind,
    client_id: String,
    #[serde(default)]
    scopes: Vec<String>,
    enabled: bool,
    has_secret: bool,
}

/// The environment's settings as the data plane reports them and the
/// management wire shows them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthSettingsView {
    #[serde(default)]
    providers: Vec<AuthProviderView>,
    #[serde(default)]
    redirect_urls: Vec<String>,
    #[serde(default)]
    magic_links: MagicLinkSettings,
    #[serde(default)]
    email_verification: EmailVerificationSettings,
    version: u64,
}

/// A provider as a developer submits it: the secret in the clear, once, or
/// not at all when the installed one is to stay.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AuthProviderUpdateWire {
    name: String,
    kind: ProviderKind,
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    enabled: bool,
}

/// The whole of an environment's settings; a replacement, never a patch.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AuthSettingsUpdateWire {
    providers: Vec<AuthProviderUpdateWire>,
    redirect_urls: Vec<String>,
    magic_links: MagicLinkSettings,
    /// Optional so a client written before the setting existed still
    /// replaces the settings; leaving it out turns verification off.
    #[serde(default)]
    email_verification: EmailVerificationSettings,
}

impl AuthSettingsUpdateWire {
    fn into_settings(
        self,
        request: &HttpRequest,
        key: &ProviderSecretKey,
        tenant: &TenantScope,
        installed: &AuthSettingsView,
    ) -> Result<AuthProviderSettings, HttpApiError> {
        let mut providers = Vec::with_capacity(self.providers.len());
        for provider in self.providers {
            let client_secret = match provider.client_secret {
                Some(secret) => key
                    .seal(tenant, &provider.name, &secret)
                    .map_err(|error| invalid_settings(request, error.to_string()))?,
                None => {
                    let installed_secret = installed
                        .providers
                        .iter()
                        .any(|current| current.name == provider.name && current.has_secret);
                    if !installed_secret {
                        return Err(invalid_settings(
                            request,
                            format!(
                                "provider {} has no installed client secret; clientSecret is required",
                                provider.name
                            ),
                        ));
                    }
                    keep_installed_secret()
                }
            };
            providers.push(ProviderConfig {
                name: provider.name,
                kind: provider.kind,
                client_id: provider.client_id,
                client_secret,
                scopes: provider.scopes,
                enabled: provider.enabled,
            });
        }
        let settings = AuthProviderSettings {
            providers,
            redirect_urls: self.redirect_urls,
            magic_links: self.magic_links,
            email_verification: self.email_verification,
            version: installed.version.saturating_add(1),
        };
        validate_with_kept_secrets(&settings)
            .map_err(|error| invalid_settings(request, error.to_string()))?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{ApiError, ApiErrorEnvelope, EnvironmentId, ProjectId};

    use super::*;

    fn request() -> HttpRequest {
        HttpRequest::for_test(HttpMethod::Put, "/", Vec::new(), Vec::new(), None)
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_authsettings").expect("project id"),
            EnvironmentId::parse("env_authsettings").expect("environment id"),
        )
    }

    fn key() -> ProviderSecretKey {
        ProviderSecretKey::derive(b"an internal secret at least thirty-two bytes long")
    }

    fn installed(providers: Vec<AuthProviderView>, version: u64) -> AuthSettingsView {
        AuthSettingsView {
            providers,
            redirect_urls: vec!["https://app.example.test/callback".to_owned()],
            magic_links: MagicLinkSettings::default(),
            email_verification: EmailVerificationSettings::default(),
            version,
        }
    }

    fn github(has_secret: bool) -> AuthProviderView {
        AuthProviderView {
            name: "github".to_owned(),
            kind: ProviderKind::GitHub,
            client_id: "Iv1.github".to_owned(),
            scopes: Vec::new(),
            enabled: true,
            has_secret,
        }
    }

    fn update(providers: Value) -> AuthSettingsUpdateWire {
        serde_json::from_value(json!({
            "providers": providers,
            "redirectUrls": ["https://app.example.test/callback"],
            "magicLinks": { "enabled": true, "linkTtlSeconds": 600 },
        }))
        .expect("update body")
    }

    #[test]
    fn a_supplied_secret_is_sealed_for_the_tenant_and_the_version_follows_the_installed_one() {
        let body = update(json!([{
            "name": "google",
            "kind": { "type": "oidc", "issuer": "https://accounts.google.com" },
            "clientId": "client-id.apps.googleusercontent.com",
            "clientSecret": "GOCSPX-plain-secret",
            "scopes": ["https://www.googleapis.com/auth/calendar.readonly"],
            "enabled": true,
        }]));
        let settings = body
            .into_settings(&request(), &key(), &tenant(), &installed(Vec::new(), 4))
            .expect("settings");
        assert_eq!(settings.version, 5);
        assert_eq!(settings.providers.len(), 1);
        let provider = &settings.providers[0];
        assert_eq!(provider.name, "google");
        assert_eq!(provider.client_id, "client-id.apps.googleusercontent.com");
        assert_eq!(
            provider.scopes,
            vec!["https://www.googleapis.com/auth/calendar.readonly".to_owned()]
        );
        assert!(provider.enabled);
        assert!(!keeps_installed_secret(&provider.client_secret));
        assert_eq!(
            key()
                .open(&tenant(), "google", &provider.client_secret)
                .expect("the sealed secret opens under the shared key"),
            "GOCSPX-plain-secret"
        );
        assert!(
            key()
                .open(&tenant(), "github", &provider.client_secret)
                .is_err(),
            "the seal is bound to the provider name"
        );
        assert_eq!(settings.magic_links.link_ttl_seconds, 600);
        assert!(settings.magic_links.enabled);

        let wire = serde_json::to_value(&settings).expect("install input");
        assert_eq!(wire["version"], 5);
        assert_eq!(wire["providers"][0]["kind"]["type"], "oidc");
        assert_eq!(
            wire["providers"][0]["kind"]["issuer"],
            "https://accounts.google.com"
        );
        assert_ne!(wire["providers"][0]["clientSecret"]["ciphertext"], "");
        assert_eq!(
            wire["providers"][0]["clientSecret"]
                .as_object()
                .expect("sealed secret")
                .keys()
                .collect::<Vec<_>>(),
            vec!["ciphertext", "nonce"]
        );
        assert!(
            !wire.to_string().contains("GOCSPX-plain-secret"),
            "the plain secret never appears in what is sent"
        );
    }

    #[test]
    fn an_omitted_secret_keeps_the_installed_one_as_an_empty_seal() {
        let body = update(json!([{
            "name": "github",
            "kind": { "type": "git_hub" },
            "clientId": "Iv1.github-rotated",
            "enabled": false,
        }]));
        let settings = body
            .into_settings(
                &request(),
                &key(),
                &tenant(),
                &installed(vec![github(true)], 1),
            )
            .expect("settings");
        assert_eq!(settings.version, 2);
        let provider = &settings.providers[0];
        assert!(keeps_installed_secret(&provider.client_secret));
        assert_eq!(provider.client_id, "Iv1.github-rotated");
        assert!(!provider.enabled);
        let wire = serde_json::to_value(&settings).expect("install input");
        assert_eq!(
            wire["providers"][0]["clientSecret"],
            json!({ "nonce": "", "ciphertext": "" })
        );
        assert_eq!(wire["providers"][0]["kind"], json!({ "type": "git_hub" }));
    }

    #[test]
    fn a_provider_without_an_installed_secret_needs_one() {
        for installed_view in [
            installed(Vec::new(), 0),
            installed(vec![github(false)], 3),
            installed(
                vec![AuthProviderView {
                    name: "gitlab".to_owned(),
                    ..github(true)
                }],
                3,
            ),
        ] {
            let body = update(json!([{
                "name": "github",
                "kind": { "type": "git_hub" },
                "clientId": "Iv1.github",
                "enabled": true,
            }]));
            let refused = body
                .into_settings(&request(), &key(), &tenant(), &installed_view)
                .expect_err("no secret to keep");
            assert_eq!(refused.envelope().error.code, ErrorCode::InvalidRequest);
            assert_eq!(
                refused.envelope().error.message,
                "provider github has no installed client secret; clientSecret is required"
            );
        }
    }

    #[test]
    fn the_settings_model_is_checked_before_anything_is_sent() {
        let bad_issuer = update(json!([{
            "name": "okta",
            "kind": { "type": "oidc", "issuer": "not a url" },
            "clientId": "okta-client",
            "clientSecret": "okta-secret",
            "enabled": true,
        }]));
        let refused = bad_issuer
            .into_settings(&request(), &key(), &tenant(), &installed(Vec::new(), 0))
            .expect_err("issuer must be a URL");
        assert_eq!(refused.envelope().error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            refused.envelope().error.message,
            "sign-in settings are invalid: issuer must be a URL"
        );

        let bad_redirect: AuthSettingsUpdateWire = serde_json::from_value(json!({
            "providers": [],
            "redirectUrls": ["http://app.example.test/callback"],
            "magicLinks": { "enabled": false, "linkTtlSeconds": 900 },
        }))
        .expect("body");
        let refused = bad_redirect
            .into_settings(
                &request(),
                &key(),
                &tenant(),
                &installed(vec![github(true)], 1),
            )
            .expect_err("plain http off loopback");
        assert_eq!(
            refused.envelope().error.message,
            "sign-in settings are invalid: redirect url must be https, or http to loopback"
        );

        let empty_secret = update(json!([{
            "name": "github",
            "kind": { "type": "git_hub" },
            "clientId": "Iv1.github",
            "clientSecret": "",
            "enabled": true,
        }]));
        let refused = empty_secret
            .into_settings(
                &request(),
                &key(),
                &tenant(),
                &installed(vec![github(true)], 1),
            )
            .expect_err("an empty secret is not a kept one");
        assert_eq!(
            refused.envelope().error.message,
            "sign-in settings are invalid: client secret must be 1-4096 printable characters"
        );

        // A kept secret does not stand in the way of checking the rest.
        let kept_but_bad_ttl: AuthSettingsUpdateWire = serde_json::from_value(json!({
            "providers": [{
                "name": "github",
                "kind": { "type": "git_hub" },
                "clientId": "Iv1.github",
                "enabled": true,
            }],
            "redirectUrls": [],
            "magicLinks": { "enabled": true, "linkTtlSeconds": 5 },
        }))
        .expect("body");
        let refused = kept_but_bad_ttl
            .into_settings(
                &request(),
                &key(),
                &tenant(),
                &installed(vec![github(true)], 1),
            )
            .expect_err("lifetime below the floor");
        assert_eq!(
            refused.envelope().error.message,
            "sign-in settings are invalid: magic link lifetime must be between 60 and 3600 seconds"
        );

        for body in [
            json!({ "providers": [], "redirectUrls": [] }),
            json!({ "providers": [], "redirectUrls": [], "magicLinks": { "enabled": false, "linkTtlSeconds": 900 }, "version": 9 }),
            json!({ "providers": [{ "name": "github", "kind": { "type": "git_hub" }, "clientId": "x", "enabled": true, "hasSecret": true }], "redirectUrls": [], "magicLinks": { "enabled": false, "linkTtlSeconds": 900 } }),
        ] {
            assert!(
                serde_json::from_value::<AuthSettingsUpdateWire>(body.clone()).is_err(),
                "{body} must be refused: the body is whole and carries no version or view fields"
            );
        }
    }

    #[test]
    fn the_data_plane_view_renders_on_the_management_wire_without_secrets() {
        let view: AuthSettingsView = serde_json::from_value(json!({
            "providers": [{
                "name": "google",
                "kind": { "type": "oidc", "issuer": "https://accounts.google.com" },
                "clientId": "client-id",
                "scopes": [],
                "enabled": true,
                "hasSecret": true,
                "somethingNewer": 1,
            }],
            "redirectUrls": ["https://app.example.test/callback"],
            "magicLinks": { "enabled": true, "linkTtlSeconds": 900 },
            "version": 7,
        }))
        .expect("inspect shape");
        assert_eq!(
            serde_json::to_value(&view).expect("wire"),
            json!({
                "providers": [{
                    "name": "google",
                    "kind": { "type": "oidc", "issuer": "https://accounts.google.com" },
                    "clientId": "client-id",
                    "scopes": [],
                    "enabled": true,
                    "hasSecret": true,
                }],
                "redirectUrls": ["https://app.example.test/callback"],
                "magicLinks": { "enabled": true, "linkTtlSeconds": 900 },
                // A data plane that predates the setting reads as off.
                "emailVerification": { "required": false },
                "version": 7,
            })
        );
        let never_configured: AuthSettingsView =
            serde_json::from_value(json!({ "version": 0 })).expect("bare view");
        assert!(never_configured.providers.is_empty());
        assert!(!never_configured.magic_links.enabled);
    }

    #[test]
    fn a_data_plane_refusal_crosses_verbatim_but_a_failure_reads_as_unavailable() {
        let refused = settings_error(
            &request(),
            InternalClientError::Remote {
                status: 400,
                envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                    ErrorCode::InvalidRequest,
                    "sign-in settings are invalid: too many providers",
                    "req_test_support",
                    RetryAdvice::Never,
                ))),
            },
        );
        assert_eq!(refused.envelope().error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            refused.envelope().error.message,
            "sign-in settings are invalid: too many providers"
        );
        for failure in [
            InternalClientError::Unavailable,
            InternalClientError::InvalidResponse,
            InternalClientError::Remote {
                status: 401,
                envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                    ErrorCode::Unauthenticated,
                    "internal caller is not authenticated",
                    "req_test_support",
                    RetryAdvice::Never,
                ))),
            },
            InternalClientError::Remote {
                status: 500,
                envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                    ErrorCode::Internal,
                    "storage failed",
                    "req_test_support",
                    RetryAdvice::Never,
                ))),
            },
        ] {
            let mapped = settings_error(&request(), failure);
            assert_eq!(mapped.envelope().error.code, ErrorCode::Unavailable);
            assert_eq!(
                mapped.envelope().error.message,
                "sign-in settings administration is unavailable"
            );
        }
    }
}
