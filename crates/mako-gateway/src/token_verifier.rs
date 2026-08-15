use std::{error::Error, fmt};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use mako_api::TenantScope;
use mako_identity::{
    AccessAuthorizationEpochs, AccessTokenClaims, AppUserId, JsonWebKeySet, SessionId,
};
use serde::Deserialize;
use serde_json::{Map, Value};

pub trait GatewaySessionResolver: Send + Sync {
    fn is_active(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> bool;
}

pub trait GatewayAuthorizationEpochResolver: Send + Sync {
    fn current_epochs(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
    ) -> Option<AccessAuthorizationEpochs>;
}

#[derive(Clone, Debug)]
pub struct AccessTokenVerificationConfig {
    issuer: String,
    audience: String,
    maximum_clock_skew_seconds: u64,
}

impl AccessTokenVerificationConfig {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        maximum_clock_skew_seconds: u64,
    ) -> Result<Self, AccessTokenVerificationError> {
        let issuer = issuer.into();
        let audience = audience.into();
        if issuer.is_empty() || audience.is_empty() || maximum_clock_skew_seconds > 300 {
            return Err(AccessTokenVerificationError::InvalidConfiguration);
        }
        Ok(Self {
            issuer,
            audience,
            maximum_clock_skew_seconds,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedAccessIdentity {
    tenant: TenantScope,
    user_id: AppUserId,
    role: String,
    session_id: SessionId,
    authorization_epochs: AccessAuthorizationEpochs,
    trusted_claims: Map<String, Value>,
}

impl VerifiedAccessIdentity {
    pub fn from_authority(
        tenant: TenantScope,
        user_id: AppUserId,
        role: String,
        session_id: SessionId,
        authorization_epochs: AccessAuthorizationEpochs,
        trusted_claims: Map<String, Value>,
    ) -> Result<Self, AccessTokenVerificationError> {
        if role.is_empty()
            || role.len() > 128
            || role.chars().any(char::is_control)
            || serde_json::to_vec(&trusted_claims).map_or(true, |claims| claims.len() > 64 * 1024)
        {
            return Err(AccessTokenVerificationError::Malformed);
        }
        Ok(Self {
            tenant,
            user_id,
            role,
            session_id,
            authorization_epochs,
            trusted_claims,
        })
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn user_id(&self) -> &AppUserId {
        &self.user_id
    }

    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn authorization_epochs(&self) -> AccessAuthorizationEpochs {
        self.authorization_epochs
    }

    #[must_use]
    pub fn trusted_claims(&self) -> &Map<String, Value> {
        &self.trusted_claims
    }
}

pub struct GatewayAccessTokenVerifier<'a> {
    jwks: &'a JsonWebKeySet,
    sessions: &'a dyn GatewaySessionResolver,
    epochs: &'a dyn GatewayAuthorizationEpochResolver,
    config: AccessTokenVerificationConfig,
}

impl<'a> GatewayAccessTokenVerifier<'a> {
    #[must_use]
    pub const fn new(
        jwks: &'a JsonWebKeySet,
        sessions: &'a dyn GatewaySessionResolver,
        epochs: &'a dyn GatewayAuthorizationEpochResolver,
        config: AccessTokenVerificationConfig,
    ) -> Self {
        Self {
            jwks,
            sessions,
            epochs,
            config,
        }
    }

    pub fn verify(
        &self,
        token: &str,
        target_tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<VerifiedAccessIdentity, AccessTokenVerificationError> {
        let mut parts = token.split('.');
        let header_segment = parts
            .next()
            .ok_or(AccessTokenVerificationError::Malformed)?;
        let claims_segment = parts
            .next()
            .ok_or(AccessTokenVerificationError::Malformed)?;
        let signature_segment = parts
            .next()
            .ok_or(AccessTokenVerificationError::Malformed)?;
        if parts.next().is_some() {
            return Err(AccessTokenVerificationError::Malformed);
        }
        let header: JwtHeader = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(header_segment)
                .map_err(|_| AccessTokenVerificationError::Malformed)?,
        )?;
        if header.alg != "EdDSA" || header.typ != "JWT" {
            return Err(AccessTokenVerificationError::UnsupportedAlgorithm);
        }
        let jwk = self
            .jwks
            .keys
            .iter()
            .find(|key| key.kid == header.kid)
            .ok_or(AccessTokenVerificationError::UnknownKey)?;
        if jwk.kty != "OKP" || jwk.crv != "Ed25519" || jwk.alg != "EdDSA" {
            return Err(AccessTokenVerificationError::UnsupportedAlgorithm);
        }
        let public_bytes: [u8; 32] = URL_SAFE_NO_PAD
            .decode(&jwk.x)
            .map_err(|_| AccessTokenVerificationError::Malformed)?
            .try_into()
            .map_err(|_| AccessTokenVerificationError::Malformed)?;
        let verifying_key = VerifyingKey::from_bytes(&public_bytes)
            .map_err(|_| AccessTokenVerificationError::Malformed)?;
        let signature = Signature::from_slice(
            &URL_SAFE_NO_PAD
                .decode(signature_segment)
                .map_err(|_| AccessTokenVerificationError::Malformed)?,
        )
        .map_err(|_| AccessTokenVerificationError::Malformed)?;
        verifying_key
            .verify(
                format!("{header_segment}.{claims_segment}").as_bytes(),
                &signature,
            )
            .map_err(|_| AccessTokenVerificationError::InvalidSignature)?;

        let claims: AccessTokenClaims = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(claims_segment)
                .map_err(|_| AccessTokenVerificationError::Malformed)?,
        )?;
        if claims.iss != self.config.issuer || claims.aud != self.config.audience {
            return Err(AccessTokenVerificationError::IssuerOrAudienceMismatch);
        }
        if claims.project_id != target_tenant.project_id().as_str()
            || claims.environment_id != target_tenant.environment_id().as_str()
        {
            return Err(AccessTokenVerificationError::TenantMismatch);
        }
        if now_unix_seconds >= claims.exp
            || claims.iat > now_unix_seconds.saturating_add(self.config.maximum_clock_skew_seconds)
        {
            return Err(AccessTokenVerificationError::ExpiredOrNotYetValid);
        }
        let user_id =
            AppUserId::parse(claims.sub).map_err(|_| AccessTokenVerificationError::Malformed)?;
        let session_id = SessionId::parse(claims.session_id)
            .map_err(|_| AccessTokenVerificationError::Malformed)?;
        if !self
            .sessions
            .is_active(target_tenant, &user_id, &session_id, now_unix_seconds)
        {
            return Err(AccessTokenVerificationError::SessionRevoked);
        }
        let token_epochs = AccessAuthorizationEpochs {
            environment: claims.environment_authorization_epoch,
            user: claims.user_authorization_epoch,
        };
        if self.epochs.current_epochs(target_tenant, &user_id) != Some(token_epochs) {
            return Err(AccessTokenVerificationError::AuthorizationEpochMismatch);
        }
        Ok(VerifiedAccessIdentity {
            tenant: target_tenant.clone(),
            user_id,
            role: claims.role,
            session_id,
            authorization_epochs: token_epochs,
            trusted_claims: claims.trusted_claims,
        })
    }
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    typ: String,
    kid: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessTokenVerificationError {
    InvalidConfiguration,
    Malformed,
    UnsupportedAlgorithm,
    UnknownKey,
    InvalidSignature,
    IssuerOrAudienceMismatch,
    TenantMismatch,
    ExpiredOrNotYetValid,
    SessionRevoked,
    AuthorizationEpochMismatch,
}

impl fmt::Display for AccessTokenVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "invalid token-verification configuration",
            Self::Malformed => "access token is malformed",
            Self::UnsupportedAlgorithm => "access token algorithm is unsupported",
            Self::UnknownKey => "access token signing key is unknown",
            Self::InvalidSignature => "access token signature is invalid",
            Self::IssuerOrAudienceMismatch => "access token issuer or audience is invalid",
            Self::TenantMismatch => "access token tenant does not match the request",
            Self::ExpiredOrNotYetValid => "access token is expired or not yet valid",
            Self::SessionRevoked => "access token session is revoked",
            Self::AuthorizationEpochMismatch => "access token authorization epoch is stale",
        })
    }
}

impl Error for AccessTokenVerificationError {}

impl From<serde_json::Error> for AccessTokenVerificationError {
    fn from(_: serde_json::Error) -> Self {
        Self::Malformed
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId};
    use mako_identity::{
        AccessTokenConfig, AccessTokenInput, AccessTokenIssuer, KeyEncryptionKey,
        ProjectSigningKeyRing,
    };

    use super::*;

    struct State;

    impl GatewaySessionResolver for State {
        fn is_active(&self, _: &TenantScope, _: &AppUserId, _: &SessionId, _: u64) -> bool {
            true
        }
    }

    impl GatewayAuthorizationEpochResolver for State {
        fn current_epochs(
            &self,
            _: &TenantScope,
            _: &AppUserId,
        ) -> Option<AccessAuthorizationEpochs> {
            Some(AccessAuthorizationEpochs {
                environment: 2,
                user: 4,
            })
        }
    }

    #[test]
    fn verifies_every_boundary_and_rejects_cross_project_tokens() {
        let target_tenant = tenant("prj_abcdefgh");
        let encryption_key = KeyEncryptionKey::generate();
        let mut ring = ProjectSigningKeyRing::new(target_tenant.clone());
        ring.create_initial(&encryption_key, 1).expect("key");
        let token = AccessTokenIssuer::new(
            &ring,
            &encryption_key,
            AccessTokenConfig::new("https://issuer.test", "mako-data", 300).expect("config"),
        )
        .issue(
            AccessTokenInput {
                tenant: target_tenant.clone(),
                user_id: AppUserId::parse("usr_abcdefgh").expect("user"),
                role: "member".to_owned(),
                session_id: SessionId::parse("ses_abcdefgh").expect("session"),
                authorization_epochs: AccessAuthorizationEpochs {
                    environment: 2,
                    user: 4,
                },
                trusted_claims: Map::new(),
            },
            100,
        )
        .expect("token");
        let jwks = ring.jwks();
        let verifier = GatewayAccessTokenVerifier::new(
            &jwks,
            &State,
            &State,
            AccessTokenVerificationConfig::new("https://issuer.test", "mako-data", 30)
                .expect("config"),
        );
        assert_eq!(
            verifier
                .verify(token.expose_for_authorization_header(), &target_tenant, 101)
                .expect("verified")
                .user_id()
                .as_str(),
            "usr_abcdefgh"
        );
        assert_eq!(
            verifier
                .verify(
                    token.expose_for_authorization_header(),
                    &tenant("prj_ijklmnop"),
                    101,
                )
                .expect_err("cross project"),
            AccessTokenVerificationError::TenantMismatch
        );
    }

    fn tenant(project: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(project).expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
