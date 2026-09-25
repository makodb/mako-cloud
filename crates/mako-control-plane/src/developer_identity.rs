use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;

use crate::{
    AuthenticationSecurityStatus, DeveloperIdentityId, DeveloperIdentityStatus,
    DeveloperRegistrationStore,
};

/// A control-plane bearer token. Its private bytes cannot be confused with project auth tokens.
#[derive(Clone, Eq, PartialEq)]
pub struct DeveloperSessionToken(String);

impl DeveloperSessionToken {
    fn parse_authorization(value: Option<&str>) -> Result<Self, DeveloperAuthenticationError> {
        let value = value.ok_or(DeveloperAuthenticationError::MissingAuthorization)?;
        let token = value
            .strip_prefix("Bearer ")
            .ok_or(DeveloperAuthenticationError::MalformedAuthorization)?;
        if token.len() < 16
            || token.len() > 16 * 1024
            || token.chars().any(char::is_whitespace)
            || token.chars().any(char::is_control)
        {
            return Err(DeveloperAuthenticationError::MalformedAuthorization);
        }
        Ok(Self(token.to_owned()))
    }

    #[must_use]
    pub fn expose_to_provider(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeveloperSessionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeveloperSessionToken([REDACTED])")
    }
}

/// Claims accepted only from the configured developer identity provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperSessionClaims {
    pub developer_identity_id: DeveloperIdentityId,
    pub issuer: String,
    pub subject: String,
    pub audiences: Vec<String>,
    pub normalized_email: String,
    pub email_verified: bool,
    pub display_name: String,
    pub session_id: String,
    pub status: DeveloperIdentityStatus,
    pub credential_epoch: u64,
    pub authorization_epoch: u64,
    pub authenticated_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperPrincipal {
    identity_id: DeveloperIdentityId,
    normalized_email: String,
    display_name: String,
    session_id: String,
    authenticated_at_unix_seconds: u64,
}

impl DeveloperPrincipal {
    #[cfg(test)]
    pub(crate) fn for_test(identity_id: DeveloperIdentityId, normalized_email: &str) -> Self {
        Self {
            identity_id,
            normalized_email: normalized_email.to_owned(),
            display_name: "Test Developer".to_owned(),
            session_id: "test-developer-session".to_owned(),
            authenticated_at_unix_seconds: 1,
        }
    }

    /// Build a principal for the local bootstrap tool.
    ///
    /// This is an actor value object, not an authority: every administrative
    /// service still resolves the identity's organization membership from the
    /// store before permitting anything, so a principal for an identity that is
    /// not a real member grants nothing. The bootstrap needs one because hosted
    /// developer sign-in requires mail delivery a local environment does not
    /// have.
    #[must_use]
    pub fn for_local_bootstrap(identity_id: DeveloperIdentityId, normalized_email: &str) -> Self {
        Self {
            identity_id,
            normalized_email: normalized_email.to_owned(),
            display_name: "Local Bootstrap".to_owned(),
            session_id: "local-bootstrap-session".to_owned(),
            authenticated_at_unix_seconds: 1,
        }
    }

    #[must_use]
    pub fn identity_id(&self) -> &DeveloperIdentityId {
        &self.identity_id
    }

    #[must_use]
    pub fn normalized_email(&self) -> &str {
        &self.normalized_email
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub const fn authenticated_at_unix_seconds(&self) -> u64 {
        self.authenticated_at_unix_seconds
    }
}

#[async_trait]
pub trait DeveloperIdentityProvider: Send + Sync {
    async fn verify_session(
        &self,
        token: &DeveloperSessionToken,
        now_unix_seconds: u64,
    ) -> Result<DeveloperSessionClaims, IdentityProviderError>;
}

#[derive(Clone)]
pub struct ControlPlaneAuthenticator {
    provider: Arc<dyn DeveloperIdentityProvider>,
    expected_issuer: String,
    expected_audience: String,
    current_state: Option<DeveloperRegistrationStore>,
}

impl fmt::Debug for ControlPlaneAuthenticator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlPlaneAuthenticator")
            .field("expected_issuer", &self.expected_issuer)
            .field("expected_audience", &self.expected_audience)
            .field("persistent_state", &self.current_state.is_some())
            .finish_non_exhaustive()
    }
}

impl ControlPlaneAuthenticator {
    pub fn new(
        provider: Arc<dyn DeveloperIdentityProvider>,
        expected_issuer: impl Into<String>,
        expected_audience: impl Into<String>,
    ) -> Result<Self, DeveloperAuthenticationError> {
        let expected_issuer = expected_issuer.into();
        let expected_audience = expected_audience.into();
        if expected_issuer.trim() != expected_issuer
            || expected_issuer.len() < 8
            || expected_issuer.len() > 2_048
            || expected_audience.trim() != expected_audience
            || expected_audience.len() < 3
            || expected_audience.len() > 256
        {
            return Err(DeveloperAuthenticationError::InvalidConfiguration);
        }
        Ok(Self {
            provider,
            expected_issuer,
            expected_audience,
            current_state: None,
        })
    }

    pub fn new_persistent(
        provider: Arc<dyn DeveloperIdentityProvider>,
        expected_issuer: impl Into<String>,
        expected_audience: impl Into<String>,
        current_state: DeveloperRegistrationStore,
    ) -> Result<Self, DeveloperAuthenticationError> {
        let mut authenticator = Self::new(provider, expected_issuer, expected_audience)?;
        authenticator.current_state = Some(current_state);
        Ok(authenticator)
    }

    pub async fn authenticate(
        &self,
        authorization: Option<&str>,
        now_unix_seconds: u64,
    ) -> Result<DeveloperPrincipal, DeveloperAuthenticationError> {
        let token = DeveloperSessionToken::parse_authorization(authorization)?;
        let claims = self
            .provider
            .verify_session(&token, now_unix_seconds)
            .await
            .map_err(DeveloperAuthenticationError::Provider)?;
        if claims.issuer != self.expected_issuer {
            return Err(DeveloperAuthenticationError::WrongIssuer);
        }
        if !claims
            .audiences
            .iter()
            .any(|audience| audience == &self.expected_audience)
        {
            return Err(DeveloperAuthenticationError::WrongAudience);
        }
        if now_unix_seconds >= claims.expires_at_unix_seconds
            || claims.authenticated_at_unix_seconds > now_unix_seconds
        {
            return Err(DeveloperAuthenticationError::ExpiredOrInvalidTime);
        }
        if !claims.email_verified {
            return Err(DeveloperAuthenticationError::EmailNotVerified);
        }
        if claims.status != DeveloperIdentityStatus::Active {
            return Err(DeveloperAuthenticationError::IdentityDisabled);
        }
        if claims.credential_epoch == 0 || claims.authorization_epoch == 0 {
            return Err(DeveloperAuthenticationError::InvalidClaim(
                "authorization epoch",
            ));
        }
        validate_claim("subject", &claims.subject, 1, 512)?;
        validate_claim("session id", &claims.session_id, 8, 512)?;
        validate_claim("normalized email", &claims.normalized_email, 3, 320)?;
        validate_claim("display name", &claims.display_name, 1, 200)?;
        let (normalized_email, display_name) = if let Some(store) = &self.current_state {
            let current = store
                .get_account(&claims.developer_identity_id)
                .await
                .map_err(|_| DeveloperAuthenticationError::CurrentStateUnavailable)?
                .ok_or(DeveloperAuthenticationError::StaleAuthority)?;
            if current.status() != DeveloperIdentityStatus::Active
                || current.authentication_security_status() != AuthenticationSecurityStatus::Active
                || current.email_verified_at_unix_seconds().is_none()
                || current.credential_epoch() != claims.credential_epoch
                || current.authorization_epoch() != claims.authorization_epoch
                || current.normalized_email().as_str() != claims.normalized_email
            {
                return Err(DeveloperAuthenticationError::StaleAuthority);
            }
            (
                current.normalized_email().as_str().to_owned(),
                current.display_name().to_owned(),
            )
        } else {
            (claims.normalized_email, claims.display_name)
        };
        Ok(DeveloperPrincipal {
            identity_id: claims.developer_identity_id,
            normalized_email,
            display_name,
            session_id: claims.session_id,
            authenticated_at_unix_seconds: claims.authenticated_at_unix_seconds,
        })
    }
}

impl ControlPlaneAuthenticator {
    /// The principal a request made with an automation token acts as: the
    /// developer who issued it, as their account stands now. A disabled,
    /// locked, or unverified account authorizes nothing through its tokens,
    /// as it would not through a session. The session id names the token,
    /// so the audit trail shows which one acted.
    pub async fn authenticate_automation(
        &self,
        created_by: &DeveloperIdentityId,
        token_id: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperPrincipal, DeveloperAuthenticationError> {
        let store = self
            .current_state
            .as_ref()
            .ok_or(DeveloperAuthenticationError::CurrentStateUnavailable)?;
        let current = store
            .get_account(created_by)
            .await
            .map_err(|_| DeveloperAuthenticationError::CurrentStateUnavailable)?
            .ok_or(DeveloperAuthenticationError::StaleAuthority)?;
        if current.status() != DeveloperIdentityStatus::Active
            || current.authentication_security_status() != AuthenticationSecurityStatus::Active
            || current.email_verified_at_unix_seconds().is_none()
        {
            return Err(DeveloperAuthenticationError::StaleAuthority);
        }
        Ok(DeveloperPrincipal {
            identity_id: created_by.clone(),
            normalized_email: current.normalized_email().as_str().to_owned(),
            display_name: current.display_name().to_owned(),
            session_id: format!("automation:{token_id}"),
            authenticated_at_unix_seconds: now_unix_seconds,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityProviderError {
    InvalidSession,
    Unavailable,
}

impl fmt::Display for IdentityProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSession => "developer identity session is invalid",
            Self::Unavailable => "developer identity provider is unavailable",
        })
    }
}

impl Error for IdentityProviderError {}

#[derive(Debug, Eq, PartialEq)]
pub enum DeveloperAuthenticationError {
    MissingAuthorization,
    MalformedAuthorization,
    InvalidConfiguration,
    Provider(IdentityProviderError),
    WrongIssuer,
    WrongAudience,
    ExpiredOrInvalidTime,
    EmailNotVerified,
    IdentityDisabled,
    StaleAuthority,
    CurrentStateUnavailable,
    InvalidClaim(&'static str),
}

impl fmt::Display for DeveloperAuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingAuthorization => "developer authorization is required",
            Self::MalformedAuthorization => "developer authorization is malformed",
            Self::InvalidConfiguration => "developer identity configuration is invalid",
            Self::Provider(IdentityProviderError::InvalidSession) => {
                "developer identity session is invalid"
            }
            Self::Provider(IdentityProviderError::Unavailable) => {
                "developer identity provider is unavailable"
            }
            Self::WrongIssuer => "developer identity issuer does not match",
            Self::WrongAudience => "developer identity audience does not match",
            Self::ExpiredOrInvalidTime => "developer identity session is expired or not yet valid",
            Self::EmailNotVerified => "developer identity email is not verified",
            Self::IdentityDisabled => "developer identity is disabled",
            Self::StaleAuthority => "developer identity authority is stale",
            Self::CurrentStateUnavailable => "developer identity state is unavailable",
            Self::InvalidClaim(field) => return write!(formatter, "developer {field} is invalid"),
        })
    }
}

impl Error for DeveloperAuthenticationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Provider(error) => Some(error),
            _ => None,
        }
    }
}

fn validate_claim(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), DeveloperAuthenticationError> {
    if !(minimum..=maximum).contains(&value.len())
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DeveloperAuthenticationError::InvalidClaim(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use mako_storage::{Durability, KvAdapter, MemoryAdapter, WriteBatch};

    use super::*;
    use crate::{ControlKeyspace, DeveloperLookupKey};

    struct FakeProvider {
        claims: DeveloperSessionClaims,
    }

    #[async_trait]
    impl DeveloperIdentityProvider for FakeProvider {
        async fn verify_session(
            &self,
            token: &DeveloperSessionToken,
            _now_unix_seconds: u64,
        ) -> Result<DeveloperSessionClaims, IdentityProviderError> {
            if token.expose_to_provider() != "developer-session-token" {
                return Err(IdentityProviderError::InvalidSession);
            }
            Ok(self.claims.clone())
        }
    }

    fn claims(audience: &str) -> DeveloperSessionClaims {
        DeveloperSessionClaims {
            developer_identity_id: DeveloperIdentityId::parse("dev_abcdefgh")
                .expect("developer id"),
            issuer: "https://identity.example.test".to_owned(),
            subject: "provider-subject".to_owned(),
            audiences: vec![audience.to_owned()],
            normalized_email: "developer@example.test".to_owned(),
            email_verified: true,
            display_name: "Developer".to_owned(),
            session_id: "developer-session-id".to_owned(),
            status: DeveloperIdentityStatus::Active,
            credential_epoch: 1,
            authorization_epoch: 1,
            authenticated_at_unix_seconds: 90,
            expires_at_unix_seconds: 200,
        }
    }

    fn authenticator(audience: &str) -> ControlPlaneAuthenticator {
        ControlPlaneAuthenticator::new(
            Arc::new(FakeProvider {
                claims: claims(audience),
            }),
            "https://identity.example.test",
            "mako-control-plane",
        )
        .expect("authenticator")
    }

    #[test]
    fn token_debug_never_exposes_session_bytes() {
        let token =
            DeveloperSessionToken::parse_authorization(Some("Bearer developer-session-token"))
                .expect("token");
        assert_eq!(format!("{token:?}"), "DeveloperSessionToken([REDACTED])");
    }

    #[test]
    fn accepts_only_control_plane_audience() {
        futures::executor::block_on(async {
            let principal = authenticator("mako-control-plane")
                .authenticate(Some("Bearer developer-session-token"), 100)
                .await
                .expect("developer principal");
            assert_eq!(principal.identity_id().as_str(), "dev_abcdefgh");

            let error = authenticator("mako-project-application")
                .authenticate(Some("Bearer developer-session-token"), 100)
                .await;
            assert_eq!(error, Err(DeveloperAuthenticationError::WrongAudience));
        });
    }

    #[test]
    fn application_tokens_cannot_cross_the_provider_boundary() {
        futures::executor::block_on(async {
            let error = authenticator("mako-control-plane")
                .authenticate(Some("Bearer mako-application-token"), 100)
                .await;
            assert_eq!(
                error,
                Err(DeveloperAuthenticationError::Provider(
                    IdentityProviderError::InvalidSession
                ))
            );
        });
    }

    #[test]
    fn persistent_authority_rejects_stale_active_claims_and_epochs() {
        futures::executor::block_on(async {
            let adapter = MemoryAdapter::new();
            let identity_id = DeveloperIdentityId::parse("dev_abcdefgh").expect("identity");
            let legacy = serde_json::json!({
                "id": identity_id,
                "issuer": "https://identity.example.test",
                "subject": "persistent-subject",
                "normalizedEmail": "developer@example.test",
                "displayName": "Persistent Developer",
                "status": "active",
                "createdAtUnixSeconds": 10,
                "lastAuthenticatedAtUnixSeconds": 10,
            });
            let mut batch = WriteBatch::new();
            batch.put(
                ControlKeyspace::developer_key(&identity_id).expect("key"),
                serde_json::to_vec(&legacy).expect("legacy json"),
            );
            adapter
                .write(batch, Durability::Memory)
                .await
                .expect("seed");
            let store = DeveloperRegistrationStore::new(
                Arc::new(adapter),
                Durability::Memory,
                DeveloperLookupKey::derive(b"persistent auth lookup key"),
            )
            .expect("store");
            store.migrate_legacy_developers().await.expect("migration");
            let provider = Arc::new(FakeProvider {
                claims: claims("mako-control-plane"),
            });
            let authenticator = ControlPlaneAuthenticator::new_persistent(
                provider,
                "https://identity.example.test",
                "mako-control-plane",
                store.clone(),
            )
            .expect("authenticator");
            authenticator
                .authenticate(Some("Bearer developer-session-token"), 100)
                .await
                .expect("current authority");

            let previous = store
                .get_account(&identity_id)
                .await
                .expect("read")
                .expect("account");
            let mut disabled = previous.clone();
            disabled
                .transition(DeveloperIdentityStatus::Disabled, 101)
                .expect("disable");
            store
                .replace_account(&previous, &disabled)
                .await
                .expect("persist disable");
            assert_eq!(
                authenticator
                    .authenticate(Some("Bearer developer-session-token"), 102)
                    .await,
                Err(DeveloperAuthenticationError::StaleAuthority)
            );
        });
    }
}
