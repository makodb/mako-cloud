use std::{error::Error, fmt};

use mako_api::TenantScope;
use rand_core::{OsRng, RngCore};

use crate::{
    AppUserId, AppUserStatus, CredentialDigest, IdentityRecordError, IdentityStore,
    IdentityStoreError, NormalizedEmail, PasswordError, PasswordResetOutcome, PasswordService,
    PasswordVerification, StoredPasswordHash, UserCredentialId, UserCredentialKind,
    UserCredentialRecord,
};

#[derive(Clone, Eq, PartialEq)]
pub struct PasswordRecoveryToken(String);

impl PasswordRecoveryToken {
    #[must_use]
    pub fn expose_for_delivery(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PasswordRecoveryToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasswordRecoveryToken([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct PasswordRecoveryEmail {
    tenant: TenantScope,
    recipient: NormalizedEmail,
    token: PasswordRecoveryToken,
    expires_at_unix_seconds: u64,
}

impl PasswordRecoveryEmail {
    #[must_use]
    pub fn recipient(&self) -> &NormalizedEmail {
        &self.recipient
    }

    #[must_use]
    pub fn token(&self) -> &PasswordRecoveryToken {
        &self.token
    }
}

impl fmt::Debug for PasswordRecoveryEmail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PasswordRecoveryEmail")
            .field("tenant", &self.tenant)
            .field("recipient", &self.recipient)
            .field("token", &"[REDACTED]")
            .field("expires_at_unix_seconds", &self.expires_at_unix_seconds)
            .finish()
    }
}

pub trait PasswordRecoveryEmailProvider: Send + Sync {
    fn enqueue_recovery(&self, email: PasswordRecoveryEmail);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordRecoveryResponse {
    Accepted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordChangeOutcome {
    Changed,
    InvalidCurrentPassword,
}

pub struct PasswordRecoveryService<'a> {
    store: &'a IdentityStore,
    passwords: &'a PasswordService,
    email_provider: &'a dyn PasswordRecoveryEmailProvider,
    token_ttl_seconds: u64,
}

impl<'a> PasswordRecoveryService<'a> {
    pub fn new(
        store: &'a IdentityStore,
        passwords: &'a PasswordService,
        email_provider: &'a dyn PasswordRecoveryEmailProvider,
        token_ttl_seconds: u64,
    ) -> Result<Self, PasswordRecoveryError> {
        if !(60..=24 * 60 * 60).contains(&token_ttl_seconds) {
            return Err(PasswordRecoveryError::InvalidConfiguration);
        }
        Ok(Self {
            store,
            passwords,
            email_provider,
            token_ttl_seconds,
        })
    }

    pub async fn request(
        &self,
        tenant: &TenantScope,
        email: &str,
        now_unix_seconds: u64,
    ) -> Result<PasswordRecoveryResponse, PasswordRecoveryError> {
        let email = NormalizedEmail::parse(email)?;
        let Some(user) = self.store.user_by_email(&email).await? else {
            return Ok(PasswordRecoveryResponse::Accepted);
        };
        if user.status() != AppUserStatus::Active {
            return Ok(PasswordRecoveryResponse::Accepted);
        }
        let token = PasswordRecoveryToken(random_secret());
        let digest = blake3::hash(token.expose_for_delivery().as_bytes());
        let expires_at_unix_seconds = now_unix_seconds
            .checked_add(self.token_ttl_seconds)
            .ok_or(PasswordRecoveryError::TimeOverflow)?;
        let credential = UserCredentialRecord::new(
            tenant.clone(),
            UserCredentialId::parse(format!("prc_{}", random_secret()))?,
            user.id().clone(),
            UserCredentialKind::PasswordRecovery,
            CredentialDigest::new(digest.as_bytes().to_vec())?,
            now_unix_seconds,
            Some(expires_at_unix_seconds),
        );
        self.store
            .create_password_recovery(&user, &credential, digest.as_bytes())
            .await?;
        self.email_provider.enqueue_recovery(PasswordRecoveryEmail {
            tenant: tenant.clone(),
            recipient: email,
            token,
            expires_at_unix_seconds,
        });
        Ok(PasswordRecoveryResponse::Accepted)
    }

    pub async fn reset(
        &self,
        token: &str,
        new_password: &str,
        now_unix_seconds: u64,
    ) -> Result<PasswordResetOutcome, PasswordRecoveryError> {
        let password = self.passwords.hash(new_password)?;
        let digest = blake3::hash(token.as_bytes());
        self.store
            .reset_password_with_token(
                digest.as_bytes(),
                CredentialDigest::new(password.encoded().as_bytes().to_vec())?,
                now_unix_seconds,
            )
            .await
            .map_err(Into::into)
    }

    pub async fn change_password(
        &self,
        user_id: &AppUserId,
        current_password: &str,
        new_password: &str,
        now_unix_seconds: u64,
    ) -> Result<PasswordChangeOutcome, PasswordRecoveryError> {
        let Some(login) = self.store.password_login_by_user_id(user_id).await? else {
            return Ok(PasswordChangeOutcome::InvalidCurrentPassword);
        };
        if login.user.status() != AppUserStatus::Active
            || login.credential.invalidated_at_unix_seconds().is_some()
        {
            return Ok(PasswordChangeOutcome::InvalidCurrentPassword);
        }
        let encoded = std::str::from_utf8(login.credential.digest().as_bytes())
            .map_err(|_| PasswordRecoveryError::CorruptPasswordCredential)?;
        let stored = StoredPasswordHash::parse(encoded)?;
        if !matches!(
            self.passwords.verify(current_password, &stored)?,
            PasswordVerification::Valid { .. }
        ) {
            return Ok(PasswordChangeOutcome::InvalidCurrentPassword);
        }
        let replacement = self.passwords.hash(new_password)?;
        let outcome = self
            .store
            .replace_password_and_revoke_sessions(
                &login,
                CredentialDigest::new(replacement.encoded().as_bytes().to_vec())?,
                now_unix_seconds,
            )
            .await?;
        Ok(match outcome {
            PasswordResetOutcome::Changed => PasswordChangeOutcome::Changed,
            PasswordResetOutcome::InvalidOrExpired => PasswordChangeOutcome::InvalidCurrentPassword,
        })
    }
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
pub enum PasswordRecoveryError {
    InvalidConfiguration,
    TimeOverflow,
    CorruptPasswordCredential,
    Record(IdentityRecordError),
    Password(PasswordError),
    Store(IdentityStoreError),
}

impl fmt::Display for PasswordRecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => formatter.write_str("invalid recovery token TTL"),
            Self::TimeOverflow => formatter.write_str("recovery token expiry overflowed"),
            Self::CorruptPasswordCredential => {
                formatter.write_str("password credential is corrupt")
            }
            Self::Record(error) => error.fmt(formatter),
            Self::Password(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl Error for PasswordRecoveryError {}

impl From<IdentityRecordError> for PasswordRecoveryError {
    fn from(error: IdentityRecordError) -> Self {
        Self::Record(error)
    }
}

impl From<PasswordError> for PasswordRecoveryError {
    fn from(error: PasswordError) -> Self {
        Self::Password(error)
    }
}

impl From<IdentityStoreError> for PasswordRecoveryError {
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
    use crate::{
        Argon2idParameters, EmailSignupConfig, PasswordPolicy, SignupService,
        TransactionalEmailProvider, VerificationEmail,
    };

    struct NoVerificationEmail;

    impl TransactionalEmailProvider for NoVerificationEmail {
        fn enqueue_verification(&self, _: VerificationEmail) {}
    }

    #[derive(Default)]
    struct CapturingRecoveryEmail(Mutex<Vec<PasswordRecoveryEmail>>);

    impl PasswordRecoveryEmailProvider for CapturingRecoveryEmail {
        fn enqueue_recovery(&self, email: PasswordRecoveryEmail) {
            self.0.lock().expect("email lock").push(email);
        }
    }

    #[test]
    fn recovery_is_enumeration_safe_single_use_and_revokes_sessions() {
        futures::executor::block_on(async {
            let tenant = TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            );
            let store = IdentityStore::new(
                std::sync::Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("store");
            let passwords = PasswordService::new(
                PasswordPolicy::default(),
                Argon2idParameters::new(8 * 1024, 2, 1, 32).expect("parameters"),
            );
            SignupService::new(
                &store,
                &passwords,
                &NoVerificationEmail,
                EmailSignupConfig {
                    enabled: true,
                    require_verification: false,
                    verification_ttl_seconds: 600,
                },
            )
            .expect("signup")
            .sign_up(&tenant, "user@example.com", "old-long-password", 1)
            .await
            .expect("signup");
            let provider = CapturingRecoveryEmail::default();
            let recovery =
                PasswordRecoveryService::new(&store, &passwords, &provider, 600).expect("recovery");

            let existing = recovery
                .request(&tenant, "user@example.com", 10)
                .await
                .expect("existing");
            let absent = recovery
                .request(&tenant, "absent@example.com", 10)
                .await
                .expect("absent");
            assert_eq!(existing, absent);
            let token = {
                let messages = provider.0.lock().expect("messages");
                assert_eq!(messages.len(), 1);
                messages[0].token().expose_for_delivery().to_owned()
            };
            assert_eq!(
                recovery
                    .reset(&token, "new-long-password", 20)
                    .await
                    .expect("reset"),
                PasswordResetOutcome::Changed
            );
            assert_eq!(
                recovery
                    .reset(&token, "another-long-password", 21)
                    .await
                    .expect("replay"),
                PasswordResetOutcome::InvalidOrExpired
            );
            let user = store
                .user_by_email(&NormalizedEmail::parse("user@example.com").expect("email"))
                .await
                .expect("lookup")
                .expect("user");
            assert_eq!(user.session_epoch(), 1);
        });
    }
}
