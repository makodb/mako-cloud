use std::{error::Error, fmt};

use mako_api::TenantScope;
use rand_core::{OsRng, RngCore};
use serde_json::json;

use crate::{
    AppUserId, AppUserRecord, AppUserStatus, CredentialDigest, EmailVerificationOutcome,
    IdentityProvider, IdentityRecordError, IdentityStore, IdentityStoreError, NormalizedEmail,
    PasswordError, PasswordService, TrustedAppMetadata, UserCredentialId, UserCredentialKind,
    UserCredentialRecord, UserIdentityId, UserIdentityRecord, UserProfileMetadata,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmailSignupConfig {
    pub enabled: bool,
    pub require_verification: bool,
    pub verification_ttl_seconds: u64,
}

impl EmailSignupConfig {
    pub fn validate(self) -> Result<Self, SignupError> {
        if self.require_verification
            && !(60..=7 * 24 * 60 * 60).contains(&self.verification_ttl_seconds)
        {
            return Err(SignupError::InvalidConfiguration(
                "verification TTL must be between one minute and seven days",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct VerificationToken(String);

impl VerificationToken {
    #[must_use]
    pub fn expose_for_delivery(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for VerificationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerificationToken([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct VerificationEmail {
    tenant: TenantScope,
    recipient: NormalizedEmail,
    user_id: AppUserId,
    token: VerificationToken,
    expires_at_unix_seconds: u64,
}

impl VerificationEmail {
    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn recipient(&self) -> &NormalizedEmail {
        &self.recipient
    }

    #[must_use]
    pub fn user_id(&self) -> &AppUserId {
        &self.user_id
    }

    #[must_use]
    pub fn token(&self) -> &VerificationToken {
        &self.token
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }
}

impl fmt::Debug for VerificationEmail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerificationEmail")
            .field("tenant", &self.tenant)
            .field("recipient", &self.recipient)
            .field("user_id", &self.user_id)
            .field("token", &"[REDACTED]")
            .field("expires_at_unix_seconds", &self.expires_at_unix_seconds)
            .finish()
    }
}

/// Implementations enqueue delivery durably and own retry behavior. The auth
/// API response never depends on provider-specific delivery status.
pub trait TransactionalEmailProvider: Send + Sync {
    fn enqueue_verification(&self, email: VerificationEmail);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignupResponse {
    Accepted,
}

pub struct SignupService<'a> {
    store: &'a IdentityStore,
    password_service: &'a PasswordService,
    email_provider: &'a dyn TransactionalEmailProvider,
    config: EmailSignupConfig,
}

impl<'a> SignupService<'a> {
    pub fn new(
        store: &'a IdentityStore,
        password_service: &'a PasswordService,
        email_provider: &'a dyn TransactionalEmailProvider,
        config: EmailSignupConfig,
    ) -> Result<Self, SignupError> {
        Ok(Self {
            store,
            password_service,
            email_provider,
            config: config.validate()?,
        })
    }

    pub async fn sign_up(
        &self,
        tenant: &TenantScope,
        email: &str,
        password: &str,
        now_unix_seconds: u64,
    ) -> Result<SignupResponse, SignupError> {
        if !self.config.enabled {
            return Err(SignupError::SignupDisabled);
        }
        let email = NormalizedEmail::parse(email)?;
        let password_hash = self.password_service.hash(password)?;
        let user_id = AppUserId::parse(random_id("usr"))?;
        let user = AppUserRecord::new(
            tenant.clone(),
            user_id.clone(),
            if self.config.require_verification {
                AppUserStatus::PendingVerification
            } else {
                AppUserStatus::Active
            },
            TrustedAppMetadata::new(json!({}))?,
            UserProfileMetadata::new(json!({}))?,
            now_unix_seconds,
        );
        let identity = UserIdentityRecord::new(
            tenant.clone(),
            UserIdentityId::parse(random_id("idn"))?,
            user_id.clone(),
            IdentityProvider::Email,
            email.as_str(),
            now_unix_seconds,
        )?;
        let password = UserCredentialRecord::new(
            tenant.clone(),
            UserCredentialId::parse(random_id("pwd"))?,
            user_id.clone(),
            UserCredentialKind::Password,
            CredentialDigest::new(password_hash.encoded().as_bytes().to_vec())?,
            now_unix_seconds,
            None,
        );

        let verification = if self.config.require_verification {
            let token = VerificationToken(random_secret());
            let digest = blake3::hash(token.expose_for_delivery().as_bytes());
            let expires_at_unix_seconds = now_unix_seconds
                .checked_add(self.config.verification_ttl_seconds)
                .ok_or(SignupError::TimeOverflow)?;
            let credential = UserCredentialRecord::new(
                tenant.clone(),
                UserCredentialId::parse(random_id("evc"))?,
                user_id.clone(),
                UserCredentialKind::EmailVerification,
                CredentialDigest::new(digest.as_bytes().to_vec())?,
                now_unix_seconds,
                Some(expires_at_unix_seconds),
            );
            Some((
                credential,
                digest.as_bytes().to_vec(),
                token,
                expires_at_unix_seconds,
            ))
        } else {
            None
        };

        let stored = self
            .store
            .create_email_signup(
                &user,
                &identity,
                &password,
                verification
                    .as_ref()
                    .map(|(credential, digest, _, _)| (credential, digest.as_slice())),
                &email,
            )
            .await;
        match stored {
            Ok(()) => {}
            Err(IdentityStoreError::EmailAlreadyExists) => return Ok(SignupResponse::Accepted),
            Err(error) => return Err(error.into()),
        }
        if let Some((_, _, token, expires_at_unix_seconds)) = verification {
            self.email_provider.enqueue_verification(VerificationEmail {
                tenant: tenant.clone(),
                recipient: email,
                user_id,
                token,
                expires_at_unix_seconds,
            });
        }
        Ok(SignupResponse::Accepted)
    }

    pub async fn verify_email(
        &self,
        token: &str,
        now_unix_seconds: u64,
    ) -> Result<EmailVerificationOutcome, SignupError> {
        let digest = blake3::hash(token.as_bytes());
        self.store
            .complete_email_verification(digest.as_bytes(), now_unix_seconds)
            .await
            .map_err(Into::into)
    }
}

fn random_id(prefix: &str) -> String {
    format!("{prefix}_{}", random_secret())
}

fn random_secret() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write;
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

#[derive(Debug)]
pub enum SignupError {
    InvalidConfiguration(&'static str),
    SignupDisabled,
    TimeOverflow,
    Record(IdentityRecordError),
    Password(PasswordError),
    Store(IdentityStoreError),
}

impl fmt::Display for SignupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => formatter.write_str(message),
            Self::SignupDisabled => formatter.write_str("email sign-up is disabled"),
            Self::TimeOverflow => formatter.write_str("verification expiry overflowed"),
            Self::Record(error) => error.fmt(formatter),
            Self::Password(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl Error for SignupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Record(error) => Some(error),
            Self::Password(error) => Some(error),
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<IdentityRecordError> for SignupError {
    fn from(error: IdentityRecordError) -> Self {
        Self::Record(error)
    }
}

impl From<PasswordError> for SignupError {
    fn from(error: PasswordError) -> Self {
        Self::Password(error)
    }
}

impl From<IdentityStoreError> for SignupError {
    fn from(error: IdentityStoreError) -> Self {
        Self::Store(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::{Durability, MemoryAdapter};

    use super::*;
    use crate::{Argon2idParameters, PasswordPolicy};

    #[derive(Default)]
    struct CapturingEmailProvider(Mutex<Vec<VerificationEmail>>);

    impl TransactionalEmailProvider for CapturingEmailProvider {
        fn enqueue_verification(&self, email: VerificationEmail) {
            self.0.lock().expect("email lock").push(email);
        }
    }

    #[test]
    fn signup_is_enumeration_safe_and_verification_is_single_use() {
        futures::executor::block_on(async {
            let tenant = TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            );
            let adapter = std::sync::Arc::new(MemoryAdapter::new());
            let store =
                IdentityStore::new(adapter, &tenant, &tenant, Durability::Memory).expect("store");
            let passwords = PasswordService::new(
                PasswordPolicy::default(),
                Argon2idParameters::new(8 * 1024, 2, 1, 32).expect("parameters"),
            );
            let email_provider = CapturingEmailProvider::default();
            let service = SignupService::new(
                &store,
                &passwords,
                &email_provider,
                EmailSignupConfig {
                    enabled: true,
                    require_verification: true,
                    verification_ttl_seconds: 600,
                },
            )
            .expect("service");

            let first = service
                .sign_up(&tenant, "Person@Example.COM", "long-enough-password", 10)
                .await
                .expect("sign up");
            let duplicate = service
                .sign_up(&tenant, "person@example.com", "another-long-password", 11)
                .await
                .expect("duplicate response");
            assert_eq!(first, duplicate);
            let token = {
                let messages = email_provider.0.lock().expect("messages");
                assert_eq!(messages.len(), 1);
                let token = messages[0].token().expose_for_delivery().to_owned();
                assert!(!format!("{:?}", messages[0]).contains(&token));
                token
            };

            assert_eq!(
                service.verify_email(&token, 20).await.expect("verify"),
                EmailVerificationOutcome::Verified
            );
            assert_eq!(
                service
                    .verify_email(&token, 21)
                    .await
                    .expect("verify again"),
                EmailVerificationOutcome::InvalidOrExpired
            );
            assert_eq!(
                store
                    .user_by_email(&NormalizedEmail::parse("person@example.com").expect("email"))
                    .await
                    .expect("lookup")
                    .expect("user")
                    .status(),
                AppUserStatus::Active
            );
        });
    }
}
