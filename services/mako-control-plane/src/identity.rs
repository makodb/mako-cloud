use std::{collections::BTreeSet, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use mako_control_plane::{
    AuthenticationSecurityStatus, ControlPlaneAuthenticator, DeveloperAccessAudience,
    DeveloperAccessClaims, DeveloperAuthenticationError, DeveloperIdentityId,
    DeveloperIdentityProvider, DeveloperIdentityStatus, DeveloperRegistrationStore,
    DeveloperSessionClaims, DeveloperSessionToken, IdentityProviderError,
    OperatorAuthenticationError, OperatorAuthenticator, OperatorId, OperatorIdentityProvider,
    OperatorIdentityProviderError, OperatorPermission, OperatorSessionClaims, OperatorSessionToken,
};
use serde::{Deserialize, de::DeserializeOwned};

const DEVELOPER_AUDIENCE: &str = "mako-management";
const OPERATOR_AUDIENCE: &str = "mako-operator";
const DEVELOPER_KEY_DOMAIN: &str = "mako/control-plane/developer-session-signing/v1";
const OPERATOR_KEY_DOMAIN: &str = "mako/control-plane/operator-session-signing/v1";

#[derive(Clone)]
pub(crate) struct DeploymentDeveloperSessionIssuer {
    signing_key: SigningKey,
    key_id: String,
}

impl fmt::Debug for DeploymentDeveloperSessionIssuer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeploymentDeveloperSessionIssuer")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl DeploymentDeveloperSessionIssuer {
    pub(crate) fn derive(secret: &str) -> Self {
        let signing_key =
            SigningKey::from_bytes(&blake3::derive_key(DEVELOPER_KEY_DOMAIN, secret.as_bytes()));
        let digest = blake3::hash(signing_key.verifying_key().as_bytes())
            .to_hex()
            .to_string();
        Self {
            signing_key,
            key_id: format!("devkid_{}", &digest[..16]),
        }
    }

    pub(crate) fn issue(&self, claims: &DeveloperAccessClaims) -> Result<String, JwtIssueError> {
        let header = serde_json::json!({
            "alg": "EdDSA",
            "typ": "JWT",
            "kid": self.key_id,
        });
        let claims = serde_json::json!({
            "iss": claims.issuer,
            "sub": claims.subject,
            "aud": [claims.audience.as_str()],
            "email": claims.normalized_email,
            "emailVerified": claims.email_verified,
            "name": claims.display_name,
            "sid": claims.session_id.as_str(),
            "developerIdentityId": claims.developer_identity_id,
            "status": claims.status,
            "credentialEpoch": claims.credential_epoch,
            "authorizationEpoch": claims.authorization_epoch,
            "iat": claims.authenticated_at_unix_seconds,
            "exp": claims.expires_at_unix_seconds,
        });
        let header =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).map_err(|_| JwtIssueError)?);
        let claims =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| JwtIssueError)?);
        let signed = format!("{header}.{claims}");
        let signature = self.signing_key.sign(signed.as_bytes());
        Ok(format!(
            "{signed}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct JwtIssueError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostedDeveloperAuthority {
    pub identity_id: DeveloperIdentityId,
    pub authorization_epoch: u64,
    pub audience: DeveloperAccessAudience,
}

#[derive(Clone)]
pub(crate) struct DeploymentHostedSessionAuthenticator {
    verifier: DeploymentJwtVerifier,
    store: DeveloperRegistrationStore,
    issuer: String,
}

impl fmt::Debug for DeploymentHostedSessionAuthenticator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeploymentHostedSessionAuthenticator")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

impl DeploymentHostedSessionAuthenticator {
    pub(crate) fn new(
        secret: &str,
        issuer: impl Into<String>,
        store: DeveloperRegistrationStore,
    ) -> Self {
        Self {
            verifier: DeploymentJwtVerifier::derive(secret, DEVELOPER_KEY_DOMAIN, "devkid"),
            store,
            issuer: issuer.into(),
        }
    }

    pub(crate) async fn authenticate(
        &self,
        authorization: Option<&str>,
        expected_audience: DeveloperAccessAudience,
        now_unix_seconds: u64,
    ) -> Result<HostedDeveloperAuthority, HostedAuthenticationError> {
        let encoded = authorization
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| {
                (16..=16 * 1024).contains(&value.len())
                    && !value.chars().any(char::is_whitespace)
                    && !value.chars().any(char::is_control)
            })
            .ok_or(HostedAuthenticationError::Invalid)?;
        let claims: DeveloperJwtClaims = self
            .verifier
            .verify(encoded)
            .map_err(|_| HostedAuthenticationError::Invalid)?;
        let expected_status = match expected_audience {
            DeveloperAccessAudience::Active => DeveloperIdentityStatus::Active,
            DeveloperAccessAudience::Waitlist => DeveloperIdentityStatus::Waitlisted,
        };
        if claims.iss != self.issuer
            || claims.aud != [expected_audience.as_str()]
            || claims.status != expected_status
            || claims.credential_epoch == 0
            || claims.authorization_epoch == 0
            || !claims.email_verified
            || claims.iat > now_unix_seconds
            || claims.exp <= now_unix_seconds
        {
            return Err(HostedAuthenticationError::Invalid);
        }
        let identity_id = DeveloperIdentityId::parse(claims.developer_identity_id)
            .map_err(|_| HostedAuthenticationError::Invalid)?;
        let current = self
            .store
            .get_account(&identity_id)
            .await
            .map_err(|_| HostedAuthenticationError::Unavailable)?
            .ok_or(HostedAuthenticationError::Invalid)?;
        if current.status() != expected_status
            || current.authentication_security_status() != AuthenticationSecurityStatus::Active
            || current.credential_epoch() != claims.credential_epoch
            || current.authorization_epoch() != claims.authorization_epoch
            || current.normalized_email().as_str() != claims.email
            || current.email_verified_at_unix_seconds().is_none()
        {
            return Err(HostedAuthenticationError::Invalid);
        }
        Ok(HostedDeveloperAuthority {
            identity_id,
            authorization_epoch: current.authorization_epoch(),
            audience: expected_audience,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostedAuthenticationError {
    Invalid,
    Unavailable,
}

#[derive(Clone)]
struct DeploymentJwtVerifier {
    key: VerifyingKey,
    key_id: String,
}

impl DeploymentJwtVerifier {
    fn derive(secret: &str, domain: &str, key_prefix: &str) -> Self {
        let signing = SigningKey::from_bytes(&blake3::derive_key(domain, secret.as_bytes()));
        let key = signing.verifying_key();
        let digest = blake3::hash(key.as_bytes()).to_hex().to_string();
        Self {
            key,
            key_id: format!("{key_prefix}_{}", &digest[..16]),
        }
    }

    fn verify<T: DeserializeOwned>(&self, encoded: &str) -> Result<T, JwtVerificationError> {
        let mut segments = encoded.split('.');
        let header_segment = segments.next().ok_or(JwtVerificationError)?;
        let claims_segment = segments.next().ok_or(JwtVerificationError)?;
        let signature_segment = segments.next().ok_or(JwtVerificationError)?;
        if segments.next().is_some()
            || header_segment.is_empty()
            || claims_segment.is_empty()
            || signature_segment.is_empty()
        {
            return Err(JwtVerificationError);
        }

        let header: JwtHeader = decode_json(header_segment)?;
        if header.algorithm != "EdDSA" || header.token_type != "JWT" || header.key_id != self.key_id
        {
            return Err(JwtVerificationError);
        }
        let signature_bytes = URL_SAFE_NO_PAD
            .decode(signature_segment)
            .map_err(|_| JwtVerificationError)?;
        let signature =
            Signature::from_slice(&signature_bytes).map_err(|_| JwtVerificationError)?;
        self.key
            .verify(
                format!("{header_segment}.{claims_segment}").as_bytes(),
                &signature,
            )
            .map_err(|_| JwtVerificationError)?;
        decode_json(claims_segment)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JwtHeader {
    #[serde(rename = "alg")]
    algorithm: String,
    #[serde(rename = "typ")]
    token_type: String,
    #[serde(rename = "kid")]
    key_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeveloperJwtClaims {
    iss: String,
    sub: String,
    aud: Vec<String>,
    email: String,
    email_verified: bool,
    name: String,
    sid: String,
    developer_identity_id: String,
    status: DeveloperIdentityStatus,
    credential_epoch: u64,
    authorization_epoch: u64,
    iat: u64,
    exp: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorJwtClaims {
    iss: String,
    sub: String,
    aud: Vec<String>,
    permissions: BTreeSet<OperatorPermission>,
    exp: u64,
}

#[derive(Clone)]
struct DeploymentDeveloperIdentityProvider(DeploymentJwtVerifier);

#[async_trait]
impl DeveloperIdentityProvider for DeploymentDeveloperIdentityProvider {
    async fn verify_session(
        &self,
        token: &DeveloperSessionToken,
        _now_unix_seconds: u64,
    ) -> Result<DeveloperSessionClaims, IdentityProviderError> {
        let claims: DeveloperJwtClaims = self
            .0
            .verify(token.expose_to_provider())
            .map_err(|_| IdentityProviderError::InvalidSession)?;
        Ok(DeveloperSessionClaims {
            developer_identity_id: DeveloperIdentityId::parse(claims.developer_identity_id)
                .map_err(|_| IdentityProviderError::InvalidSession)?,
            issuer: claims.iss,
            subject: claims.sub,
            audiences: claims.aud,
            normalized_email: claims.email,
            email_verified: claims.email_verified,
            display_name: claims.name,
            session_id: claims.sid,
            status: claims.status,
            credential_epoch: claims.credential_epoch,
            authorization_epoch: claims.authorization_epoch,
            authenticated_at_unix_seconds: claims.iat,
            expires_at_unix_seconds: claims.exp,
        })
    }
}

#[derive(Clone)]
struct DeploymentOperatorIdentityProvider(DeploymentJwtVerifier);

#[async_trait]
impl OperatorIdentityProvider for DeploymentOperatorIdentityProvider {
    async fn verify_session(
        &self,
        token: &OperatorSessionToken,
        _now_unix_seconds: u64,
    ) -> Result<OperatorSessionClaims, OperatorIdentityProviderError> {
        let claims: OperatorJwtClaims = self
            .0
            .verify(token.expose_to_provider())
            .map_err(|_| OperatorIdentityProviderError::InvalidToken)?;
        Ok(OperatorSessionClaims {
            operator_id: OperatorId::parse(claims.sub)
                .map_err(|_| OperatorIdentityProviderError::InvalidToken)?,
            issuer: claims.iss,
            audiences: claims.aud,
            permissions: claims.permissions,
            expires_at_unix_seconds: claims.exp,
        })
    }
}

pub(crate) fn deployment_authenticators(
    secret: &str,
    issuer: &str,
    developer_store: Option<mako_control_plane::DeveloperRegistrationStore>,
) -> Result<(ControlPlaneAuthenticator, OperatorAuthenticator), IdentityCompositionError> {
    let developer_provider = Arc::new(DeploymentDeveloperIdentityProvider(
        DeploymentJwtVerifier::derive(secret, DEVELOPER_KEY_DOMAIN, "devkid"),
    ));
    let operator_provider = Arc::new(DeploymentOperatorIdentityProvider(
        DeploymentJwtVerifier::derive(secret, OPERATOR_KEY_DOMAIN, "oprkid"),
    ));
    let developer_authenticator = match developer_store {
        Some(store) => ControlPlaneAuthenticator::new_persistent(
            developer_provider,
            issuer,
            DEVELOPER_AUDIENCE,
            store,
        ),
        None => ControlPlaneAuthenticator::new(developer_provider, issuer, DEVELOPER_AUDIENCE),
    }
    .map_err(IdentityCompositionError::Developer)?;
    Ok((
        developer_authenticator,
        OperatorAuthenticator::new(operator_provider, issuer, OPERATOR_AUDIENCE)
            .map_err(IdentityCompositionError::Operator)?,
    ))
}

fn decode_json<T: DeserializeOwned>(segment: &str) -> Result<T, JwtVerificationError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|_| JwtVerificationError)?;
    serde_json::from_slice(&bytes).map_err(|_| JwtVerificationError)
}

#[derive(Clone, Copy, Debug)]
struct JwtVerificationError;

#[derive(Debug)]
pub(crate) enum IdentityCompositionError {
    Developer(DeveloperAuthenticationError),
    Operator(OperatorAuthenticationError),
}

impl fmt::Display for IdentityCompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("control-plane identity verification could not be configured")
    }
}

impl Error for IdentityCompositionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Developer(error) => Some(error),
            Self::Operator(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use serde_json::json;

    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const ISSUER: &str = "https://api.example.test/control-identity";

    #[test]
    fn deployment_tokens_are_domain_separated_and_strictly_verified() {
        let (developer, operator) =
            deployment_authenticators(SECRET, ISSUER, None).expect("authenticators");
        let developer_token = sign(
            DEVELOPER_KEY_DOMAIN,
            "devkid",
            json!({
                "iss": ISSUER,
                "sub": "developer@example.test",
                "aud": [DEVELOPER_AUDIENCE],
                "email": "developer@example.test",
                "emailVerified": true,
                "name": "Beta Developer",
                "sid": "session_example00",
                "developerIdentityId": "dev_example00",
                "status": "active",
                "credentialEpoch": 1,
                "authorizationEpoch": 1,
                "iat": 100,
                "exp": 200
            }),
        );
        let principal = futures::executor::block_on(
            developer.authenticate(Some(&format!("Bearer {developer_token}")), 150),
        )
        .expect("developer principal");
        assert_eq!(principal.identity_id().as_str(), "dev_example00");

        let operator_token = sign(
            OPERATOR_KEY_DOMAIN,
            "oprkid",
            json!({
                "iss": ISSUER,
                "sub": "opr_example00",
                "aud": [OPERATOR_AUDIENCE],
                "permissions": ["tenant_read"],
                "exp": 200
            }),
        );
        let principal = futures::executor::block_on(
            operator.authenticate(Some(&format!("Bearer {operator_token}")), 150),
        )
        .expect("operator principal");
        assert!(principal.allows(OperatorPermission::TenantRead));

        assert!(
            futures::executor::block_on(
                developer.authenticate(Some(&format!("Bearer {operator_token}")), 150,)
            )
            .is_err()
        );
    }

    fn sign(domain: &str, key_prefix: &str, claims: serde_json::Value) -> String {
        let signing = SigningKey::from_bytes(&blake3::derive_key(domain, SECRET.as_bytes()));
        let digest = blake3::hash(signing.verifying_key().as_bytes())
            .to_hex()
            .to_string();
        let header = json!({
            "alg": "EdDSA",
            "typ": "JWT",
            "kid": format!("{key_prefix}_{}", &digest[..16])
        });
        let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header"));
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims"));
        let signed = format!("{header}.{claims}");
        let signature = signing.sign(signed.as_bytes());
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }
}
