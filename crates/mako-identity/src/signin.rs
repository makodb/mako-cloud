use std::{error::Error, fmt};

use mako_api::TenantScope;

use crate::{
    AppUserId, AppUserStatus, CredentialDigest, IdentityRecordError, IdentityStore,
    IdentityStoreError, NormalizedEmail, PasswordError, PasswordService, PasswordVerification,
    StoredPasswordHash,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignInRequestMetadata {
    request_id: String,
    rate_limit_partition: String,
}

impl SignInRequestMetadata {
    pub fn new(
        request_id: impl Into<String>,
        rate_limit_partition: impl Into<String>,
    ) -> Result<Self, SignInError> {
        let request_id = request_id.into();
        let rate_limit_partition = rate_limit_partition.into();
        validate_safe_identifier("request id", &request_id)?;
        validate_safe_identifier("rate-limit partition", &rate_limit_partition)?;
        Ok(Self {
            request_id,
            rate_limit_partition,
        })
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SignInThrottleKey(pub(crate) [u8; 32]);

impl fmt::Debug for SignInThrottleKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SignInThrottleKey([HASHED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignInThrottleDecision {
    Allowed,
    Throttled { retry_after_seconds: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignInAttemptOutcome {
    Succeeded,
    Failed,
    Throttled,
}

pub trait SignInThrottle: Send + Sync {
    fn check(&self, key: &SignInThrottleKey, now_unix_seconds: u64) -> SignInThrottleDecision;

    fn record(&self, key: &SignInThrottleKey, outcome: SignInAttemptOutcome);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthenticationAuditOutcome {
    Succeeded,
    InvalidCredentials,
    Throttled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticationAuditEvent {
    tenant: TenantScope,
    actor: Option<AppUserId>,
    outcome: AuthenticationAuditOutcome,
    request_id: String,
    occurred_at_unix_seconds: u64,
}

impl AuthenticationAuditEvent {
    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn actor(&self) -> Option<&AppUserId> {
        self.actor.as_ref()
    }

    #[must_use]
    pub const fn outcome(&self) -> AuthenticationAuditOutcome {
        self.outcome
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    #[must_use]
    pub const fn occurred_at_unix_seconds(&self) -> u64 {
        self.occurred_at_unix_seconds
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticationAuditError(String);

impl AuthenticationAuditError {
    #[must_use]
    pub fn new(safe_message: impl Into<String>) -> Self {
        Self(safe_message.into())
    }
}

impl fmt::Display for AuthenticationAuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for AuthenticationAuditError {}

pub trait AuthenticationAuditSink: Send + Sync {
    fn record(&self, event: &AuthenticationAuditEvent) -> Result<(), AuthenticationAuditError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignInResponse {
    Authenticated { user_id: AppUserId },
    Denied { retry_after_seconds: Option<u64> },
}

pub struct SignInService<'a> {
    store: &'a IdentityStore,
    passwords: &'a PasswordService,
    throttle: &'a dyn SignInThrottle,
    audit: &'a dyn AuthenticationAuditSink,
    dummy_hash: StoredPasswordHash,
}

impl<'a> SignInService<'a> {
    pub fn new(
        store: &'a IdentityStore,
        passwords: &'a PasswordService,
        throttle: &'a dyn SignInThrottle,
        audit: &'a dyn AuthenticationAuditSink,
    ) -> Result<Self, SignInError> {
        Ok(Self {
            store,
            passwords,
            throttle,
            audit,
            dummy_hash: passwords.hash("mako-dummy-password-never-used")?,
        })
    }

    pub async fn sign_in(
        &self,
        tenant: &TenantScope,
        email: &str,
        password: &str,
        metadata: &SignInRequestMetadata,
        now_unix_seconds: u64,
    ) -> Result<SignInResponse, SignInError> {
        let normalized = NormalizedEmail::parse(email).ok();
        let throttle_key = throttle_key(tenant, normalized.as_ref(), email, metadata);
        if let SignInThrottleDecision::Throttled {
            retry_after_seconds,
        } = self.throttle.check(&throttle_key, now_unix_seconds)
        {
            self.throttle
                .record(&throttle_key, SignInAttemptOutcome::Throttled);
            self.record_audit(
                tenant,
                None,
                AuthenticationAuditOutcome::Throttled,
                metadata,
                now_unix_seconds,
            )?;
            return Ok(SignInResponse::Denied {
                retry_after_seconds: Some(retry_after_seconds),
            });
        }

        let login = match &normalized {
            Some(email) => self.store.password_login_by_email(email).await?,
            None => None,
        };
        let candidate = login
            .as_ref()
            .and_then(|login| std::str::from_utf8(login.credential.digest().as_bytes()).ok())
            .and_then(|encoded| StoredPasswordHash::parse(encoded).ok());
        let hash = candidate.as_ref().unwrap_or(&self.dummy_hash);
        let verification = self.passwords.verify(password, hash)?;
        let authenticated = login.as_ref().is_some_and(|login| {
            login.user.status() == AppUserStatus::Active
                && login.credential.invalidated_at_unix_seconds().is_none()
                && matches!(verification, PasswordVerification::Valid { .. })
        });

        if authenticated {
            let login = login.as_ref().expect("authenticated login is present");
            if let PasswordVerification::Valid {
                upgraded: Some(upgraded),
            } = verification
            {
                self.store
                    .upgrade_password_hash(
                        login,
                        CredentialDigest::new(upgraded.encoded().as_bytes().to_vec())?,
                    )
                    .await?;
            }
            self.throttle
                .record(&throttle_key, SignInAttemptOutcome::Succeeded);
            self.record_audit(
                tenant,
                Some(login.user.id().clone()),
                AuthenticationAuditOutcome::Succeeded,
                metadata,
                now_unix_seconds,
            )?;
            Ok(SignInResponse::Authenticated {
                user_id: login.user.id().clone(),
            })
        } else {
            self.throttle
                .record(&throttle_key, SignInAttemptOutcome::Failed);
            self.record_audit(
                tenant,
                None,
                AuthenticationAuditOutcome::InvalidCredentials,
                metadata,
                now_unix_seconds,
            )?;
            Ok(SignInResponse::Denied {
                retry_after_seconds: None,
            })
        }
    }

    fn record_audit(
        &self,
        tenant: &TenantScope,
        actor: Option<AppUserId>,
        outcome: AuthenticationAuditOutcome,
        metadata: &SignInRequestMetadata,
        now_unix_seconds: u64,
    ) -> Result<(), SignInError> {
        self.audit
            .record(&AuthenticationAuditEvent {
                tenant: tenant.clone(),
                actor,
                outcome,
                request_id: metadata.request_id.clone(),
                occurred_at_unix_seconds: now_unix_seconds,
            })
            .map_err(Into::into)
    }
}

fn throttle_key(
    tenant: &TenantScope,
    normalized: Option<&NormalizedEmail>,
    raw_email: &str,
    metadata: &SignInRequestMetadata,
) -> SignInThrottleKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tenant.project_id().as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(tenant.environment_id().as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(metadata.rate_limit_partition.as_bytes());
    hasher.update(&[0]);
    hasher
        .update(normalized.map_or_else(|| raw_email.as_bytes(), |email| email.as_str().as_bytes()));
    SignInThrottleKey(*hasher.finalize().as_bytes())
}

fn validate_safe_identifier(field: &'static str, value: &str) -> Result<(), SignInError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(SignInError::InvalidMetadata(field));
    }
    Ok(())
}

#[derive(Debug)]
pub enum SignInError {
    InvalidMetadata(&'static str),
    Record(IdentityRecordError),
    Password(PasswordError),
    Store(IdentityStoreError),
    Audit(AuthenticationAuditError),
}

impl fmt::Display for SignInError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMetadata(field) => write!(formatter, "invalid {field}"),
            Self::Record(error) => error.fmt(formatter),
            Self::Password(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Audit(error) => error.fmt(formatter),
        }
    }
}

impl Error for SignInError {}

impl From<IdentityRecordError> for SignInError {
    fn from(error: IdentityRecordError) -> Self {
        Self::Record(error)
    }
}

impl From<PasswordError> for SignInError {
    fn from(error: PasswordError) -> Self {
        Self::Password(error)
    }
}

impl From<IdentityStoreError> for SignInError {
    fn from(error: IdentityStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<AuthenticationAuditError> for SignInError {
    fn from(error: AuthenticationAuditError) -> Self {
        Self::Audit(error)
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

    #[derive(Default)]
    struct AllowThrottle(Mutex<Vec<SignInAttemptOutcome>>);

    impl SignInThrottle for AllowThrottle {
        fn check(&self, _: &SignInThrottleKey, _: u64) -> SignInThrottleDecision {
            SignInThrottleDecision::Allowed
        }

        fn record(&self, _: &SignInThrottleKey, outcome: SignInAttemptOutcome) {
            self.0.lock().expect("throttle lock").push(outcome);
        }
    }

    #[derive(Default)]
    struct CapturingAudit(Mutex<Vec<AuthenticationAuditEvent>>);

    impl AuthenticationAuditSink for CapturingAudit {
        fn record(&self, event: &AuthenticationAuditEvent) -> Result<(), AuthenticationAuditError> {
            self.0.lock().expect("audit lock").push(event.clone());
            Ok(())
        }
    }

    struct NoEmail;

    impl TransactionalEmailProvider for NoEmail {
        fn enqueue_verification(&self, _: VerificationEmail) {}
    }

    #[test]
    fn wrong_absent_and_unverified_accounts_share_one_failure_response() {
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
                &NoEmail,
                EmailSignupConfig {
                    enabled: true,
                    require_verification: false,
                    verification_ttl_seconds: 600,
                },
            )
            .expect("signup")
            .sign_up(&tenant, "user@example.com", "correct-long-password", 1)
            .await
            .expect("signup");
            let throttle = AllowThrottle::default();
            let audit = CapturingAudit::default();
            let service =
                SignInService::new(&store, &passwords, &throttle, &audit).expect("sign in service");
            let metadata =
                SignInRequestMetadata::new("req_abcdefgh", "ip_hash_1").expect("metadata");

            let wrong = service
                .sign_in(
                    &tenant,
                    "user@example.com",
                    "wrong-long-password",
                    &metadata,
                    2,
                )
                .await
                .expect("wrong");
            let absent = service
                .sign_in(
                    &tenant,
                    "absent@example.com",
                    "wrong-long-password",
                    &metadata,
                    3,
                )
                .await
                .expect("absent");
            assert_eq!(wrong, absent);
            assert_eq!(
                service
                    .sign_in(
                        &tenant,
                        "user@example.com",
                        "correct-long-password",
                        &metadata,
                        4,
                    )
                    .await
                    .expect("success"),
                SignInResponse::Authenticated {
                    user_id: store
                        .user_by_email(&NormalizedEmail::parse("user@example.com").expect("email"))
                        .await
                        .expect("lookup")
                        .expect("user")
                        .id()
                        .clone()
                }
            );
            let events = audit.0.lock().expect("events");
            assert_eq!(events.len(), 3);
            assert_eq!(
                events[0].outcome(),
                AuthenticationAuditOutcome::InvalidCredentials
            );
            assert!(!format!("{events:?}").contains("correct-long-password"));
        });
    }
}
