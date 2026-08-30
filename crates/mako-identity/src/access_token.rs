use std::{error::Error, fmt};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::Signer;
use mako_api::TenantScope;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{AppUserId, KeyEncryptionKey, ProjectSigningKeyRing, SessionId, SigningKeyError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessAuthorizationEpochs {
    pub environment: u64,
    pub user: u64,
}

#[derive(Clone, Debug)]
pub struct AccessTokenConfig {
    issuer: String,
    audience: String,
    lifetime_seconds: u64,
}

impl AccessTokenConfig {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        lifetime_seconds: u64,
    ) -> Result<Self, AccessTokenError> {
        let issuer = issuer.into();
        let audience = audience.into();
        if issuer.is_empty() || issuer.len() > 512 || audience.is_empty() || audience.len() > 256 {
            return Err(AccessTokenError::InvalidConfiguration);
        }
        if !(60..=60 * 60).contains(&lifetime_seconds) {
            return Err(AccessTokenError::InvalidConfiguration);
        }
        Ok(Self {
            issuer,
            audience,
            lifetime_seconds,
        })
    }
}

#[derive(Clone, Debug)]
pub struct AccessTokenInput {
    pub tenant: TenantScope,
    pub user_id: AppUserId,
    pub role: String,
    /// The address the account authenticated as, and whether the environment
    /// has confirmed the holder controls it. Policies read both, and read
    /// them separately: the address alone says only what somebody typed.
    pub email: String,
    pub email_verified: bool,
    pub session_id: SessionId,
    pub authorization_epochs: AccessAuthorizationEpochs,
    pub trusted_claims: Map<String, Value>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct AccessToken(String);

impl AccessToken {
    #[must_use]
    pub fn expose_for_authorization_header(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccessToken([REDACTED])")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct JwtHeader {
    alg: String,
    typ: String,
    kid: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AccessTokenClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub project_id: String,
    pub environment_id: String,
    pub role: String,
    pub iat: u64,
    pub exp: u64,
    pub session_id: String,
    pub environment_authorization_epoch: u64,
    pub user_authorization_epoch: u64,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
    pub trusted_claims: Map<String, Value>,
}

pub struct AccessTokenIssuer<'a> {
    key_ring: &'a ProjectSigningKeyRing,
    encryption_key: &'a KeyEncryptionKey,
    config: AccessTokenConfig,
}

impl<'a> AccessTokenIssuer<'a> {
    #[must_use]
    pub const fn new(
        key_ring: &'a ProjectSigningKeyRing,
        encryption_key: &'a KeyEncryptionKey,
        config: AccessTokenConfig,
    ) -> Self {
        Self {
            key_ring,
            encryption_key,
            config,
        }
    }

    pub fn issue(
        &self,
        input: AccessTokenInput,
        now_unix_seconds: u64,
    ) -> Result<AccessToken, AccessTokenError> {
        if self
            .key_ring
            .records()
            .first()
            .is_some_and(|record| record.tenant() != &input.tenant)
            || !valid_role(&input.role)
            || !valid_email(&input.email)
        {
            return Err(AccessTokenError::InvalidInput);
        }
        let exp = now_unix_seconds
            .checked_add(self.config.lifetime_seconds)
            .ok_or(AccessTokenError::TimeOverflow)?;
        let (key_id, signing_key) = self
            .key_ring
            .active_signing_key(self.encryption_key)
            .map_err(AccessTokenError::SigningKey)?;
        let header = JwtHeader {
            alg: "EdDSA".to_owned(),
            typ: "JWT".to_owned(),
            kid: key_id,
        };
        let claims = AccessTokenClaims {
            iss: self.config.issuer.clone(),
            aud: self.config.audience.clone(),
            sub: input.user_id.as_str().to_owned(),
            project_id: input.tenant.project_id().as_str().to_owned(),
            environment_id: input.tenant.environment_id().as_str().to_owned(),
            role: input.role,
            iat: now_unix_seconds,
            exp,
            session_id: input.session_id.as_str().to_owned(),
            environment_authorization_epoch: input.authorization_epochs.environment,
            user_authorization_epoch: input.authorization_epochs.user,
            email: input.email,
            email_verified: input.email_verified,
            trusted_claims: input.trusted_claims,
        };
        let encoded_header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header)?);
        let encoded_claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?);
        let signing_input = format!("{encoded_header}.{encoded_claims}");
        let signature = signing_key.sign(signing_input.as_bytes());
        Ok(AccessToken(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        )))
    }
}

/// The address a token may carry. An empty one is allowed and means the
/// environment did not supply one, which policies read as no address at all.
fn valid_email(email: &str) -> bool {
    email.is_empty()
        || (email.len() <= 256 && email.contains('@') && !email.chars().any(char::is_control))
}

fn valid_role(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= 128
        && role
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Debug)]
pub enum AccessTokenError {
    InvalidConfiguration,
    InvalidInput,
    TimeOverflow,
    SigningKey(SigningKeyError),
    Json(serde_json::Error),
}

impl fmt::Display for AccessTokenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => formatter.write_str("invalid access-token configuration"),
            Self::InvalidInput => formatter.write_str("invalid access-token input"),
            Self::TimeOverflow => formatter.write_str("access-token expiry overflowed"),
            Self::SigningKey(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
        }
    }
}

impl Error for AccessTokenError {}

impl From<serde_json::Error> for AccessTokenError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signature, Verifier};
    use mako_api::{EnvironmentId, ProjectId};

    use super::*;

    #[test]
    fn issued_token_contains_every_required_scoped_claim_and_valid_signature() {
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        );
        let encryption_key = KeyEncryptionKey::generate();
        let mut ring = ProjectSigningKeyRing::new(tenant.clone());
        ring.create_initial(&encryption_key, 1).expect("key");
        let issuer = AccessTokenIssuer::new(
            &ring,
            &encryption_key,
            AccessTokenConfig::new("https://auth.example.test", "mako-data", 300).expect("config"),
        );
        let token = issuer
            .issue(
                AccessTokenInput {
                    tenant: tenant.clone(),
                    user_id: AppUserId::parse("usr_abcdefgh").expect("user"),
                    role: "member".to_owned(),
                    email: "Member@Example.test".to_owned(),
                    email_verified: true,
                    session_id: SessionId::parse("ses_abcdefgh").expect("session"),
                    authorization_epochs: AccessAuthorizationEpochs {
                        environment: 7,
                        user: 3,
                    },
                    trusted_claims: Map::new(),
                },
                100,
            )
            .expect("token");
        let parts = token
            .expose_for_authorization_header()
            .split('.')
            .collect::<Vec<_>>();
        assert_eq!(parts.len(), 3);
        let claims: AccessTokenClaims =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).expect("claims"))
                .expect("claims JSON");
        assert_eq!(claims.sub, "usr_abcdefgh");
        assert_eq!(claims.project_id, tenant.project_id().as_str());
        assert_eq!(claims.environment_id, tenant.environment_id().as_str());
        assert_eq!(claims.session_id, "ses_abcdefgh");
        // The address the session authenticated as, and its confirmation,
        // travel as separate claims: a policy that hands a document to an
        // address has to be able to require both.
        assert_eq!(claims.email, "Member@Example.test");
        assert!(claims.email_verified);
        assert_eq!(claims.environment_authorization_epoch, 7);
        assert_eq!(claims.user_authorization_epoch, 3);
        assert_eq!(claims.iat, 100);
        assert_eq!(claims.exp, 400);

        let (_, signing_key) = ring.active_signing_key(&encryption_key).expect("key");
        let signature =
            Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).expect("signature"))
                .expect("signature");
        signing_key
            .verifying_key()
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("verify");
        assert!(!format!("{token:?}").contains(parts[2]));
    }
}
