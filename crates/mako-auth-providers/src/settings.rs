use std::{error::Error, fmt};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use mako_api::TenantScope;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use url::Url;

const SECRET_KEY_CONTEXT: &str = "mako/auth-providers/client-secret/v1";
const MAX_PROVIDERS: usize = 16;
const MAX_REDIRECTS: usize = 32;
const MIN_LINK_TTL: u64 = 60;
const MAX_LINK_TTL: u64 = 60 * 60;

/// Which protocol a provider speaks; the platform knows two shapes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ProviderKind {
    /// OpenID Connect with discovery at `{issuer}/.well-known/openid-configuration`.
    Oidc { issuer: String },
    /// GitHub's OAuth 2.0 with its user and emails endpoints.
    GitHub,
}

/// A client secret as it is stored and installed: ciphertext under the key
/// both planes derive from the shared internal secret, bound to the tenant
/// and provider name so it cannot be moved between environments.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SealedSecret {
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderConfig {
    /// `google`, `github`, `okta-acme`: lowercase, digits, hyphens.
    pub name: String,
    pub kind: ProviderKind,
    pub client_id: String,
    pub client_secret: SealedSecret,
    /// OIDC scopes beyond `openid email profile`; ignored for GitHub.
    #[serde(default)]
    pub scopes: Vec<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MagicLinkSettings {
    pub enabled: bool,
    pub link_ttl_seconds: u64,
}

impl Default for MagicLinkSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            link_ttl_seconds: 15 * 60,
        }
    }
}

/// Whether a password sign-up must confirm its address before it can sign in.
/// When required, sign-up mails a single-use link to a registered redirect
/// URL and the account stays unverified until the link is redeemed.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmailVerificationSettings {
    pub required: bool,
}

/// An environment's sign-in settings as installed into the data plane.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthProviderSettings {
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Where a provider callback may send the browser back to, exactly.
    #[serde(default)]
    pub redirect_urls: Vec<String>,
    #[serde(default)]
    pub magic_links: MagicLinkSettings,
    #[serde(default)]
    pub email_verification: EmailVerificationSettings,
    pub version: u64,
}

impl AuthProviderSettings {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.providers.len() > MAX_PROVIDERS {
            return Err(SettingsError::Invalid("too many providers"));
        }
        let mut names = std::collections::BTreeSet::new();
        for provider in &self.providers {
            if !valid_provider_name(&provider.name) || !names.insert(provider.name.as_str()) {
                return Err(SettingsError::Invalid(
                    "provider names must be unique, lowercase letters, digits, and hyphens",
                ));
            }
            if provider.client_id.is_empty()
                || provider.client_id.len() > 512
                || provider.client_id.chars().any(char::is_control)
            {
                return Err(SettingsError::Invalid(
                    "client id must be 1-512 printable characters",
                ));
            }
            if let ProviderKind::Oidc { issuer } = &provider.kind {
                let url = Url::parse(issuer)
                    .map_err(|_| SettingsError::Invalid("issuer must be a URL"))?;
                if url.query().is_some() || url.fragment().is_some() {
                    return Err(SettingsError::Invalid(
                        "issuer must not carry a query or fragment",
                    ));
                }
            }
            if provider.scopes.len() > 16
                || provider.scopes.iter().any(|scope| {
                    scope.is_empty()
                        || scope.len() > 64
                        || !scope.bytes().all(|b| b.is_ascii_graphic())
                })
            {
                return Err(SettingsError::Invalid("scopes are invalid"));
            }
            if provider.client_secret.nonce.is_empty()
                || provider.client_secret.ciphertext.is_empty()
            {
                return Err(SettingsError::Invalid("client secret is missing"));
            }
        }
        if self.redirect_urls.len() > MAX_REDIRECTS {
            return Err(SettingsError::Invalid("too many redirect urls"));
        }
        for redirect in &self.redirect_urls {
            validate_redirect(redirect)?;
        }
        if self.magic_links.link_ttl_seconds < MIN_LINK_TTL
            || self.magic_links.link_ttl_seconds > MAX_LINK_TTL
        {
            return Err(SettingsError::Invalid(
                "magic link lifetime must be between 60 and 3600 seconds",
            ));
        }
        if self.email_verification.required && self.redirect_urls.is_empty() {
            return Err(SettingsError::Invalid(
                "email verification needs a redirect url for its links",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers
            .iter()
            .find(|provider| provider.name == name && provider.enabled)
    }

    /// Whether a callback may send the browser to `redirect`: exact match only.
    #[must_use]
    pub fn admits_redirect(&self, redirect: &str) -> bool {
        self.redirect_urls.iter().any(|allowed| allowed == redirect)
    }
}

/// Redirect targets are absolute `https` URLs (or `http` to loopback for
/// local development) without fragments; matched exactly, never by prefix.
pub fn validate_redirect(redirect: &str) -> Result<(), SettingsError> {
    let url = Url::parse(redirect)
        .map_err(|_| SettingsError::Invalid("redirect url must be absolute"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback)) {
        return Err(SettingsError::Invalid(
            "redirect url must be https, or http to loopback",
        ));
    }
    if url.fragment().is_some() || url.username() != "" || url.password().is_some() {
        return Err(SettingsError::Invalid(
            "redirect url must not carry credentials or a fragment",
        ));
    }
    if redirect.len() > 2048 {
        return Err(SettingsError::Invalid("redirect url is too long"));
    }
    Ok(())
}

#[must_use]
pub fn valid_provider_name(name: &str) -> bool {
    (2..=64).contains(&name.len())
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// The key client secrets are sealed under: derived by both planes from the
/// internal-auth secret they already share, never stored.
#[derive(Clone)]
pub struct ProviderSecretKey([u8; 32]);

impl fmt::Debug for ProviderSecretKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderSecretKey([REDACTED])")
    }
}

impl ProviderSecretKey {
    #[must_use]
    pub fn derive(internal_secret: &[u8]) -> Self {
        Self(blake3::derive_key(SECRET_KEY_CONTEXT, internal_secret))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.0).into())
    }

    pub fn seal(
        &self,
        tenant: &TenantScope,
        provider_name: &str,
        secret: &str,
    ) -> Result<SealedSecret, SettingsError> {
        if secret.is_empty() || secret.len() > 4096 || secret.chars().any(char::is_control) {
            return Err(SettingsError::Invalid(
                "client secret must be 1-4096 printable characters",
            ));
        }
        let mut nonce = [0u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: secret.as_bytes(),
                    aad: &aad(tenant, provider_name),
                },
            )
            .map_err(|_| SettingsError::Seal)?;
        Ok(SealedSecret {
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
        })
    }

    pub fn open(
        &self,
        tenant: &TenantScope,
        provider_name: &str,
        sealed: &SealedSecret,
    ) -> Result<String, SettingsError> {
        let nonce = URL_SAFE_NO_PAD
            .decode(&sealed.nonce)
            .map_err(|_| SettingsError::Seal)?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(&sealed.ciphertext)
            .map_err(|_| SettingsError::Seal)?;
        if nonce.len() != 24 {
            return Err(SettingsError::Seal);
        }
        let plaintext = self
            .cipher()
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: &aad(tenant, provider_name),
                },
            )
            .map_err(|_| SettingsError::Seal)?;
        String::from_utf8(plaintext).map_err(|_| SettingsError::Seal)
    }
}

fn aad(tenant: &TenantScope, provider_name: &str) -> Vec<u8> {
    format!(
        "{}\0{}\0{provider_name}",
        tenant.project_id(),
        tenant.environment_id()
    )
    .into_bytes()
}

/// Builds sealed settings from plain client secrets, for the control plane at
/// configuration time and for tests.
pub fn sealed_settings_from_plain(
    key: &ProviderSecretKey,
    tenant: &TenantScope,
    providers: Vec<(ProviderConfigPlain, String)>,
    redirect_urls: Vec<String>,
    magic_links: MagicLinkSettings,
    version: u64,
) -> Result<AuthProviderSettings, SettingsError> {
    let providers = providers
        .into_iter()
        .map(|(plain, secret)| {
            Ok(ProviderConfig {
                client_secret: key.seal(tenant, &plain.name, &secret)?,
                name: plain.name,
                kind: plain.kind,
                client_id: plain.client_id,
                scopes: plain.scopes,
                enabled: plain.enabled,
            })
        })
        .collect::<Result<Vec<_>, SettingsError>>()?;
    let settings = AuthProviderSettings {
        providers,
        redirect_urls,
        magic_links,
        email_verification: EmailVerificationSettings::default(),
        version,
    };
    settings.validate()?;
    Ok(settings)
}

/// A provider as a developer submits it: the secret travels separately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderConfigPlain {
    pub name: String,
    pub kind: ProviderKind,
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsError {
    Invalid(&'static str),
    Seal,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(formatter, "sign-in settings are invalid: {reason}"),
            Self::Seal => formatter.write_str("client secret could not be sealed or opened"),
        }
    }
}

impl Error for SettingsError {}
