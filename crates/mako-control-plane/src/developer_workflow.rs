use std::{
    error::Error,
    fmt,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use mako_identity::{
    Argon2idParameters, NormalizedEmail, PasswordError, PasswordPolicy, PasswordService,
    PasswordVerification, StoredPasswordHash,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{
    AuthenticationIdentityRecord, AuthenticationSecurityStatus, DeveloperAccessAudience,
    DeveloperAccount, DeveloperAuthToken, DeveloperAuthTokenId, DeveloperAuthTokenPurpose,
    DeveloperAuthTokenRecord, DeveloperDecisionKind, DeveloperDecisionRecord, DeveloperIdentityId,
    DeveloperIdentityStatus, DeveloperMailKind, DeveloperMailOutboxId, DeveloperMailOutboxRecord,
    DeveloperMailOutboxState, DeveloperRateLimitRecord, DeveloperRefreshCredential,
    DeveloperRefreshSessionId, DeveloperRefreshSessionRecord, DeveloperRegistrationError,
    DeveloperRegistrationStore, DeveloperReviewPage, DeveloperRoleRecord, EncryptedDeveloperMail,
    OperatorPermission, OperatorPrincipal,
};

const MAXIMUM_PUBLIC_ORIGIN_BYTES: usize = 2_048;
const MAXIMUM_ISSUER_BYTES: usize = 2_048;
const MAXIMUM_SOURCE_BYTES: usize = 512;
const MAXIMUM_PASSWORD_BYTES: usize = 1_024;
const MAXIMUM_IDEMPOTENCY_BYTES: usize = 256;
/// How many refresh rotations a sign-out follows to the current session; the
/// bound only stops a loop, as for access tokens.
const MAXIMUM_SIGN_OUT_ROTATIONS: usize = 16;
const MAXIMUM_REQUEST_ID_BYTES: usize = 256;
const MAIL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperRegistrationConfig {
    pub enabled: bool,
    pub mail_ready: bool,
    pub public_origin: String,
    pub issuer: String,
    pub verification_lifetime_seconds: u64,
    pub recovery_lifetime_seconds: u64,
    pub access_lifetime_seconds: u64,
    pub refresh_lifetime_seconds: u64,
    pub decision_retention_seconds: u64,
    pub rate_window_seconds: u64,
    pub global_requests_per_window: u32,
    pub source_requests_per_window: u32,
    pub email_requests_per_window: u32,
    pub token_attempts_per_window: u32,
    pub maximum_parallel_password_work: usize,
    pub maximum_outbox_batch: usize,
    pub maximum_pending_outbox: usize,
    pub outbox_lease_seconds: u64,
    pub outbox_maximum_attempts: u32,
    pub outbox_maximum_backoff_seconds: u64,
    pub delivered_mail_retention_seconds: u64,
}

impl DeveloperRegistrationConfig {
    pub fn validate(&self) -> Result<(), DeveloperWorkflowError> {
        if self.public_origin.len() > MAXIMUM_PUBLIC_ORIGIN_BYTES
            || !(self.public_origin.starts_with("https://")
                || self.public_origin.starts_with("http://127.0.0.1")
                || self.public_origin.starts_with("http://localhost"))
            || self.public_origin.ends_with('/')
            || self.public_origin.chars().any(char::is_control)
            || !(8..=MAXIMUM_ISSUER_BYTES).contains(&self.issuer.len())
            || self.issuer.trim() != self.issuer
            || self.issuer.chars().any(char::is_control)
            || self.verification_lifetime_seconds == 0
            || self.recovery_lifetime_seconds == 0
            || !(60..=3_600).contains(&self.access_lifetime_seconds)
            || self.refresh_lifetime_seconds <= self.access_lifetime_seconds
            || self.decision_retention_seconds == 0
            || self.rate_window_seconds == 0
            || self.global_requests_per_window == 0
            || self.source_requests_per_window == 0
            || self.email_requests_per_window == 0
            || self.token_attempts_per_window == 0
            || self.maximum_parallel_password_work == 0
            || !(1..=100).contains(&self.maximum_outbox_batch)
            || !(1..10_000).contains(&self.maximum_pending_outbox)
            || self.outbox_lease_seconds == 0
            || self.outbox_maximum_attempts == 0
            || self.outbox_maximum_backoff_seconds == 0
            || self.delivered_mail_retention_seconds == 0
        {
            return Err(DeveloperWorkflowError::InvalidConfiguration);
        }
        Ok(())
    }

    #[must_use]
    pub const fn registration_ready(&self) -> bool {
        self.enabled && self.mail_ready
    }
}

impl Default for DeveloperRegistrationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mail_ready: false,
            public_origin: "https://localhost".to_owned(),
            issuer: "https://localhost/v1/developer-auth".to_owned(),
            verification_lifetime_seconds: 24 * 60 * 60,
            recovery_lifetime_seconds: 60 * 60,
            access_lifetime_seconds: 15 * 60,
            refresh_lifetime_seconds: 30 * 24 * 60 * 60,
            decision_retention_seconds: 90 * 24 * 60 * 60,
            rate_window_seconds: 15 * 60,
            global_requests_per_window: 500,
            source_requests_per_window: 30,
            email_requests_per_window: 8,
            token_attempts_per_window: 20,
            maximum_parallel_password_work: 4,
            maximum_outbox_batch: 32,
            maximum_pending_outbox: 5_000,
            outbox_lease_seconds: 60,
            outbox_maximum_attempts: 8,
            outbox_maximum_backoff_seconds: 6 * 60 * 60,
            delivered_mail_retention_seconds: 30 * 24 * 60 * 60,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct DeveloperMailEncryptionKey([u8; 32]);

impl DeveloperMailEncryptionKey {
    #[must_use]
    pub fn derive(secret: &[u8]) -> Self {
        Self(blake3::derive_key(
            "mako/control-plane/developer-mail-outbox/v1",
            secret,
        ))
    }
}

impl fmt::Debug for DeveloperMailEncryptionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeveloperMailEncryptionKey([REDACTED])")
    }
}

#[derive(Clone)]
pub struct DeveloperMailCipher {
    key: DeveloperMailEncryptionKey,
}

impl fmt::Debug for DeveloperMailCipher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperMailCipher")
            .finish_non_exhaustive()
    }
}

impl DeveloperMailCipher {
    #[must_use]
    pub const fn new(key: DeveloperMailEncryptionKey) -> Self {
        Self { key }
    }

    pub fn encrypt(
        &self,
        outbox_id: &DeveloperMailOutboxId,
        identity_id: &DeveloperIdentityId,
        kind: DeveloperMailKind,
        envelope: &DeveloperMailEnvelope,
    ) -> Result<EncryptedDeveloperMail, DeveloperWorkflowError> {
        self.seal(&mail_aad(outbox_id, identity_id, kind), envelope)
    }

    pub fn decrypt(
        &self,
        record: &DeveloperMailOutboxRecord,
    ) -> Result<DeveloperMailEnvelope, DeveloperWorkflowError> {
        self.open(
            &mail_aad(record.id(), record.identity_id(), record.kind()),
            record.encrypted_mail(),
        )
    }

    /// Encrypts an envelope under caller-supplied associated data. The
    /// developer outbox binds its records to a developer identity; the
    /// application outbox binds its own to a tenant and intent id with the
    /// same key, so both stay unreadable at rest without it.
    pub(crate) fn seal(
        &self,
        aad: &[u8],
        envelope: &DeveloperMailEnvelope,
    ) -> Result<EncryptedDeveloperMail, DeveloperWorkflowError> {
        envelope.validate()?;
        let plaintext = serde_json::to_vec(envelope)?;
        self.seal_bytes(aad, &plaintext)
    }

    pub(crate) fn open(
        &self,
        aad: &[u8],
        encrypted: &EncryptedDeveloperMail,
    ) -> Result<DeveloperMailEnvelope, DeveloperWorkflowError> {
        let plaintext = self.open_bytes(aad, encrypted)?;
        let envelope: DeveloperMailEnvelope = serde_json::from_slice(&plaintext)?;
        envelope.validate()?;
        Ok(envelope)
    }

    /// Seals arbitrary bytes under caller-supplied associated data with the
    /// same key and construction the mail outboxes use. Webhook signing
    /// secrets rest under this: they are bytes, not mail, but the key,
    /// the cipher, and the redacted ciphertext type are already right.
    pub(crate) fn seal_bytes(
        &self,
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<EncryptedDeveloperMail, DeveloperWorkflowError> {
        let mut nonce = [0_u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = XChaCha20Poly1305::new((&self.key.0).into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| DeveloperWorkflowError::MailEncryption)?;
        EncryptedDeveloperMail::new(
            URL_SAFE_NO_PAD.encode(nonce),
            URL_SAFE_NO_PAD.encode(ciphertext),
        )
        .map_err(DeveloperWorkflowError::Registration)
    }

    pub(crate) fn open_bytes(
        &self,
        aad: &[u8],
        encrypted: &EncryptedDeveloperMail,
    ) -> Result<Vec<u8>, DeveloperWorkflowError> {
        let nonce = URL_SAFE_NO_PAD
            .decode(encrypted.nonce())
            .map_err(|_| DeveloperWorkflowError::MailEncryption)?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(encrypted.ciphertext())
            .map_err(|_| DeveloperWorkflowError::MailEncryption)?;
        XChaCha20Poly1305::new((&self.key.0).into())
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad,
                },
            )
            .map_err(|_| DeveloperWorkflowError::MailEncryption)
    }
}

fn mail_aad(
    outbox_id: &DeveloperMailOutboxId,
    identity_id: &DeveloperIdentityId,
    kind: DeveloperMailKind,
) -> Vec<u8> {
    format!(
        "mako-mail-v1\0{}\0{}\0{kind:?}",
        outbox_id.as_str(),
        identity_id.as_str()
    )
    .into_bytes()
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperMailEnvelope {
    schema_version: u32,
    recipient: NormalizedEmail,
    subject: String,
    text_body: String,
}

impl DeveloperMailEnvelope {
    pub fn new(
        recipient: NormalizedEmail,
        subject: impl Into<String>,
        text_body: impl Into<String>,
    ) -> Result<Self, DeveloperWorkflowError> {
        let envelope = Self {
            schema_version: MAIL_SCHEMA_VERSION,
            recipient,
            subject: subject.into(),
            text_body: text_body.into(),
        };
        envelope.validate()?;
        Ok(envelope)
    }

    fn validate(&self) -> Result<(), DeveloperWorkflowError> {
        if self.schema_version != MAIL_SCHEMA_VERSION
            || !(1..=200).contains(&self.subject.len())
            || self.subject.contains(['\r', '\n'])
            || !(1..=32 * 1024).contains(&self.text_body.len())
            || self.text_body.contains('\0')
        {
            return Err(DeveloperWorkflowError::InvalidMail);
        }
        Ok(())
    }

    #[must_use]
    pub fn recipient(&self) -> &NormalizedEmail {
        &self.recipient
    }

    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    #[must_use]
    pub fn text_body(&self) -> &str {
        &self.text_body
    }
}

impl fmt::Debug for DeveloperMailEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperMailEnvelope")
            .field("schema_version", &self.schema_version)
            .field("private_fields", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperGenericOutcome {
    Accepted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperVerificationOutcome {
    Verified,
    InvalidOrExpired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeveloperRecoveryOutcome {
    Completed { identity_id: DeveloperIdentityId },
    InvalidOrExpired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperAccessClaims {
    pub developer_identity_id: DeveloperIdentityId,
    pub issuer: String,
    pub subject: String,
    pub audience: DeveloperAccessAudience,
    pub normalized_email: String,
    pub email_verified: bool,
    pub display_name: String,
    pub session_id: DeveloperRefreshSessionId,
    pub status: DeveloperIdentityStatus,
    pub credential_epoch: u64,
    pub authorization_epoch: u64,
    pub authenticated_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

#[derive(Clone, Eq, PartialEq)]
pub struct DeveloperSessionGrant {
    pub claims: DeveloperAccessClaims,
    refresh_credential: DeveloperRefreshCredential,
    cookie: String,
}

impl DeveloperSessionGrant {
    #[must_use]
    pub fn refresh_credential(&self) -> &DeveloperRefreshCredential {
        &self.refresh_credential
    }

    #[must_use]
    pub fn set_cookie_header(&self) -> &str {
        &self.cookie
    }
}

impl fmt::Debug for DeveloperSessionGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperSessionGrant")
            .field("claims", &self.claims)
            .field("refresh_credential", &"[REDACTED]")
            .field("cookie", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperWaitlistStatus {
    pub identity_id: DeveloperIdentityId,
    pub status: DeveloperIdentityStatus,
}

#[derive(Debug)]
struct PasswordWorkLimiter {
    active: AtomicUsize,
    maximum: usize,
}

impl PasswordWorkLimiter {
    const fn new(maximum: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            maximum,
        }
    }

    fn run<T>(
        &self,
        operation: impl FnOnce() -> Result<T, PasswordError>,
    ) -> Result<T, DeveloperWorkflowError> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.maximum).then_some(active + 1)
            })
            .map_err(|_| DeveloperWorkflowError::Busy)?;
        struct Guard<'a>(&'a AtomicUsize);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::Release);
            }
        }
        let _guard = Guard(&self.active);
        operation().map_err(DeveloperWorkflowError::Password)
    }
}

#[derive(Clone)]
pub struct DeveloperRegistrationService {
    store: DeveloperRegistrationStore,
    config: DeveloperRegistrationConfig,
    passwords: PasswordService,
    password_work: Arc<PasswordWorkLimiter>,
    dummy_password_hash: StoredPasswordHash,
    mail_cipher: DeveloperMailCipher,
}

impl fmt::Debug for DeveloperRegistrationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperRegistrationService")
            .field("registration_ready", &self.config.registration_ready())
            .finish_non_exhaustive()
    }
}

impl DeveloperRegistrationService {
    pub fn new(
        store: DeveloperRegistrationStore,
        config: DeveloperRegistrationConfig,
        mail_key: DeveloperMailEncryptionKey,
    ) -> Result<Self, DeveloperWorkflowError> {
        config.validate()?;
        let passwords =
            PasswordService::new(PasswordPolicy::default(), Argon2idParameters::default());
        let password_work = Arc::new(PasswordWorkLimiter::new(
            config.maximum_parallel_password_work,
        ));
        let dummy_password_hash =
            password_work.run(|| passwords.hash("mako-nonexistent-account-password"))?;
        Ok(Self {
            store,
            config,
            passwords,
            password_work,
            dummy_password_hash,
            mail_cipher: DeveloperMailCipher::new(mail_key),
        })
    }

    #[must_use]
    pub const fn config(&self) -> &DeveloperRegistrationConfig {
        &self.config
    }

    #[must_use]
    pub const fn store(&self) -> &DeveloperRegistrationStore {
        &self.store
    }

    pub async fn register(
        &self,
        email: &str,
        display_name: &str,
        password: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperGenericOutcome, DeveloperWorkflowError> {
        self.require_registration_ready()?;
        validate_public_text(source, 1, MAXIMUM_SOURCE_BYTES)?;
        validate_public_text(display_name, 1, 200)?;
        if password.len() > MAXIMUM_PASSWORD_BYTES {
            return Err(DeveloperWorkflowError::InvalidRequest);
        }
        let email =
            NormalizedEmail::parse(email).map_err(|_| DeveloperWorkflowError::InvalidRequest)?;
        self.enforce_request_limits("registration", source, &email, now_unix_seconds)
            .await?;
        self.require_outbox_capacity().await?;
        if let Some(identity) = self
            .store
            .get_authentication_identity_by_email(&email)
            .await?
        {
            let stored = identity
                .password_hash()
                .and_then(|encoded| StoredPasswordHash::parse(encoded).ok())
                .unwrap_or_else(|| self.dummy_password_hash.clone());
            let verification = self
                .password_work
                .run(|| self.passwords.verify(password, &stored))?;
            if matches!(verification, PasswordVerification::Valid { .. })
                && identity.password_hash().is_some()
                && identity.email_verified_at_unix_seconds().is_some()
                && identity.security_status() == AuthenticationSecurityStatus::Active
                && self
                    .store
                    .get_developer_role(identity.id())
                    .await?
                    .is_none()
            {
                let role =
                    DeveloperRoleRecord::new_waitlisted(identity.id().clone(), now_unix_seconds)?;
                match self.store.create_developer_role(&identity, &role).await {
                    Ok(()) | Err(DeveloperRegistrationError::Conflict) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            return Ok(DeveloperGenericOutcome::Accepted);
        }
        let password_hash = self.password_work.run(|| self.passwords.hash(password))?;

        let identity_id = random_developer_id()?;
        let account = DeveloperAccount::new_unverified(
            identity_id.clone(),
            email.clone(),
            display_name,
            password_hash.encoded(),
            now_unix_seconds,
        )?;
        let raw_token = DeveloperAuthToken::generate();
        let token = self.token_record(
            &identity_id,
            DeveloperAuthTokenPurpose::EmailVerification,
            &raw_token,
            now_unix_seconds,
            self.config.verification_lifetime_seconds,
        )?;
        let outbox = self.token_mail(
            &account,
            DeveloperMailKind::VerifyEmail,
            &raw_token,
            token.expires_at_unix_seconds(),
            now_unix_seconds,
        )?;

        match self
            .store
            .create_unverified(&account, &token, &outbox)
            .await
        {
            Ok(()) | Err(DeveloperRegistrationError::Conflict) => {
                Ok(DeveloperGenericOutcome::Accepted)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn resend_verification(
        &self,
        email: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperGenericOutcome, DeveloperWorkflowError> {
        self.require_registration_ready()?;
        validate_public_text(source, 1, MAXIMUM_SOURCE_BYTES)?;
        let email =
            NormalizedEmail::parse(email).map_err(|_| DeveloperWorkflowError::InvalidRequest)?;
        self.enforce_request_limits("verification-resend", source, &email, now_unix_seconds)
            .await?;
        self.require_outbox_capacity().await?;
        let Some(account) = self.store.get_account_by_email(&email).await? else {
            return Ok(DeveloperGenericOutcome::Accepted);
        };
        if account.status() != DeveloperIdentityStatus::Unverified {
            return Ok(DeveloperGenericOutcome::Accepted);
        }
        self.replace_or_create_token(
            &account,
            DeveloperAuthTokenPurpose::EmailVerification,
            DeveloperMailKind::VerifyEmail,
            self.config.verification_lifetime_seconds,
            now_unix_seconds,
        )
        .await?;
        Ok(DeveloperGenericOutcome::Accepted)
    }

    pub async fn verify_email(
        &self,
        raw_token: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperVerificationOutcome, DeveloperWorkflowError> {
        self.require_enabled()?;
        validate_public_text(source, 1, MAXIMUM_SOURCE_BYTES)?;
        self.enforce_token_limit("verify-token", source, now_unix_seconds)
            .await?;
        let Ok(raw_token) = DeveloperAuthToken::parse(raw_token) else {
            return Ok(DeveloperVerificationOutcome::InvalidOrExpired);
        };
        let digest = raw_token.digest(
            self.store.lookup_key(),
            DeveloperAuthTokenPurpose::EmailVerification,
        );
        let Some(previous_token) = self.store.get_token(&digest).await? else {
            return Ok(DeveloperVerificationOutcome::InvalidOrExpired);
        };
        if previous_token.purpose() != DeveloperAuthTokenPurpose::EmailVerification
            || previous_token.consumed_at_unix_seconds().is_some()
            || now_unix_seconds >= previous_token.expires_at_unix_seconds()
        {
            return Ok(DeveloperVerificationOutcome::InvalidOrExpired);
        }
        let Some(previous_account) = self.store.get_account(previous_token.identity_id()).await?
        else {
            return Err(DeveloperWorkflowError::Registration(
                DeveloperRegistrationError::CorruptRecord,
            ));
        };
        if previous_account.status() != DeveloperIdentityStatus::Unverified {
            return Ok(DeveloperVerificationOutcome::InvalidOrExpired);
        }
        let mut consumed = previous_token.clone();
        consumed.consume(now_unix_seconds)?;
        let mut next_account = previous_account.clone();
        next_account.transition(DeveloperIdentityStatus::Waitlisted, now_unix_seconds)?;
        match self
            .store
            .consume_token_and_replace_account(
                &previous_account,
                &next_account,
                &previous_token,
                &consumed,
            )
            .await
        {
            Ok(()) => Ok(DeveloperVerificationOutcome::Verified),
            Err(DeveloperRegistrationError::Conflict) => {
                Ok(DeveloperVerificationOutcome::InvalidOrExpired)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn sign_in(
        &self,
        email: &str,
        password: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperSessionGrant, DeveloperWorkflowError> {
        self.require_enabled()?;
        validate_public_text(source, 1, MAXIMUM_SOURCE_BYTES)?;
        if password.len() > MAXIMUM_PASSWORD_BYTES {
            return Err(DeveloperWorkflowError::InvalidCredentials);
        }
        let normalized = NormalizedEmail::parse(email).ok();
        if let Some(email) = &normalized {
            self.enforce_request_limits("sign-in", source, email, now_unix_seconds)
                .await?;
        } else {
            self.enforce_token_limit("sign-in-invalid-email", source, now_unix_seconds)
                .await?;
        }
        let account = match &normalized {
            Some(email) => self.store.get_account_by_email(email).await?,
            None => None,
        };
        let stored = account
            .as_ref()
            .and_then(DeveloperAccount::password_hash)
            .and_then(|encoded| StoredPasswordHash::parse(encoded).ok())
            .unwrap_or_else(|| self.dummy_password_hash.clone());
        let verification = self
            .password_work
            .run(|| self.passwords.verify(password, &stored))?;
        let Some(mut account) = account else {
            return Err(DeveloperWorkflowError::InvalidCredentials);
        };
        let PasswordVerification::Valid { upgraded } = verification else {
            return Err(DeveloperWorkflowError::InvalidCredentials);
        };
        if account.authentication_security_status() != AuthenticationSecurityStatus::Active {
            return Err(DeveloperWorkflowError::InvalidCredentials);
        }
        let audience = match account.status() {
            DeveloperIdentityStatus::Active => DeveloperAccessAudience::Active,
            DeveloperIdentityStatus::Waitlisted => DeveloperAccessAudience::Waitlist,
            _ => return Err(DeveloperWorkflowError::InvalidCredentials),
        };
        let previous = account.clone();
        if let Some(upgraded) = upgraded {
            account.upgrade_password_hash(upgraded.encoded(), now_unix_seconds)?;
        }
        account.record_authentication(now_unix_seconds)?;
        self.store.replace_account(&previous, &account).await?;
        self.issue_session(&account, audience, now_unix_seconds)
            .await
    }

    /// Verifies the developer password without issuing or authorizing a developer session.
    /// Operator authentication applies its separate lifecycle and entitlement checks to the
    /// returned identity. Unknown identities still execute the same Argon2id dummy work.
    pub async fn verify_password_identity(
        &self,
        email: &str,
        password: &str,
        now_unix_seconds: u64,
    ) -> Result<Option<DeveloperAccount>, DeveloperWorkflowError> {
        if password.len() > MAXIMUM_PASSWORD_BYTES {
            return Ok(None);
        }
        let normalized = NormalizedEmail::parse(email).ok();
        let account = match &normalized {
            Some(email) => self.store.get_account_by_email(email).await?,
            None => None,
        };
        let has_stored_password = account
            .as_ref()
            .and_then(DeveloperAccount::password_hash)
            .is_some();
        let stored = account
            .as_ref()
            .and_then(DeveloperAccount::password_hash)
            .and_then(|encoded| StoredPasswordHash::parse(encoded).ok())
            .unwrap_or_else(|| self.dummy_password_hash.clone());
        let verification = self
            .password_work
            .run(|| self.passwords.verify(password, &stored))?;
        let (Some(mut account), PasswordVerification::Valid { upgraded }) = (account, verification)
        else {
            return Ok(None);
        };
        if !has_stored_password {
            return Ok(None);
        }
        if let Some(upgraded) = upgraded {
            let previous = account.clone();
            account.upgrade_password_hash(upgraded.encoded(), now_unix_seconds)?;
            self.store.replace_account(&previous, &account).await?;
        }
        Ok(Some(account))
    }

    /// Verifies credentials against the shared authentication identity without requiring a
    /// developer role. Operator authorization applies security and entitlement policy separately.
    pub async fn verify_password_authentication_identity(
        &self,
        email: &str,
        password: &str,
        now_unix_seconds: u64,
    ) -> Result<Option<AuthenticationIdentityRecord>, DeveloperWorkflowError> {
        if password.len() > MAXIMUM_PASSWORD_BYTES {
            return Ok(None);
        }
        let normalized = NormalizedEmail::parse(email).ok();
        let identity = match &normalized {
            Some(email) => {
                self.store
                    .get_authentication_identity_by_email(email)
                    .await?
            }
            None => None,
        };
        let has_stored_password = identity
            .as_ref()
            .and_then(AuthenticationIdentityRecord::password_hash)
            .is_some();
        let stored = identity
            .as_ref()
            .and_then(AuthenticationIdentityRecord::password_hash)
            .and_then(|encoded| StoredPasswordHash::parse(encoded).ok())
            .unwrap_or_else(|| self.dummy_password_hash.clone());
        let verification = self
            .password_work
            .run(|| self.passwords.verify(password, &stored))?;
        let (Some(mut identity), PasswordVerification::Valid { upgraded }) =
            (identity, verification)
        else {
            return Ok(None);
        };
        if !has_stored_password {
            return Ok(None);
        }
        if let Some(upgraded) = upgraded {
            let previous = identity.clone();
            identity.upgrade_password_hash(upgraded.encoded(), now_unix_seconds)?;
            self.store
                .replace_authentication_identity(&previous, &identity)
                .await?;
        }
        Ok(Some(identity))
    }

    pub async fn refresh(
        &self,
        encoded_refresh: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperSessionGrant, DeveloperWorkflowError> {
        self.require_enabled()?;
        let credential = DeveloperRefreshCredential::parse(encoded_refresh)
            .map_err(|_| DeveloperWorkflowError::InvalidSession)?;
        let Some(previous) = self
            .store
            .get_refresh_session(credential.session_id())
            .await?
        else {
            return Err(DeveloperWorkflowError::InvalidSession);
        };
        if previous.revoked_at_unix_seconds().is_some()
            || now_unix_seconds >= previous.expires_at_unix_seconds()
            || !constant_time_equal(
                previous.credential_digest().as_bytes(),
                credential.digest(self.store.lookup_key()).as_bytes(),
            )
        {
            return Err(DeveloperWorkflowError::InvalidSession);
        }
        let Some(account) = self.store.get_account(previous.identity_id()).await? else {
            return Err(DeveloperWorkflowError::InvalidSession);
        };
        if account.authentication_security_status() != AuthenticationSecurityStatus::Active
            || account.credential_epoch() != previous.credential_epoch()
            || account.authorization_epoch() != previous.developer_authorization_epoch()
            || access_audience(account.status()) != Some(previous.audience())
        {
            return Err(DeveloperWorkflowError::InvalidSession);
        }
        let next_id = random_refresh_session_id()?;
        let next_credential = DeveloperRefreshCredential::generate(next_id.clone());
        let next = DeveloperRefreshSessionRecord::new(
            next_id,
            account.id().clone(),
            next_credential.digest(self.store.lookup_key()),
            previous.audience(),
            account.credential_epoch(),
            account.authorization_epoch(),
            now_unix_seconds,
            checked_add(now_unix_seconds, self.config.refresh_lifetime_seconds)?,
        )?;
        let mut revoked = previous.clone();
        revoked.rotate_to(next.id().clone(), now_unix_seconds)?;
        self.store
            .rotate_refresh_session(&previous, &revoked, &next)
            .await
            .map_err(|error| match error {
                DeveloperRegistrationError::Conflict => DeveloperWorkflowError::InvalidSession,
                other => other.into(),
            })?;
        Ok(self.session_grant(&account, &next, next_credential, now_unix_seconds))
    }

    pub async fn sign_out(
        &self,
        encoded_refresh: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperGenericOutcome, DeveloperWorkflowError> {
        let Ok(credential) = DeveloperRefreshCredential::parse(encoded_refresh) else {
            return Ok(DeveloperGenericOutcome::Accepted);
        };
        let Some(previous) = self
            .store
            .get_refresh_session(credential.session_id())
            .await?
        else {
            return Ok(DeveloperGenericOutcome::Accepted);
        };
        if !constant_time_equal(
            previous.credential_digest().as_bytes(),
            credential.digest(self.store.lookup_key()).as_bytes(),
        ) {
            return Ok(DeveloperGenericOutcome::Accepted);
        }
        // A cookie from before a refresh names the session that refresh
        // rotated. Revoking that one again changed nothing (a rotated session
        // already carries its revocation time), so the session it was rotated
        // into stayed signed in while sign-out answered success. The rotation
        // is followed to the current session, as access-token checks follow
        // it, and that one is revoked; a refresh racing the sign-out is
        // followed too.
        let mut current = previous;
        for _ in 0..MAXIMUM_SIGN_OUT_ROTATIONS {
            match (
                current.revoked_at_unix_seconds(),
                current.rotated_to().cloned(),
            ) {
                (None, _) => {
                    let mut revoked = current.clone();
                    revoked.revoke(now_unix_seconds)?;
                    match self.store.replace_refresh_session(&current, &revoked).await {
                        Ok(()) => return Ok(DeveloperGenericOutcome::Accepted),
                        Err(DeveloperRegistrationError::Conflict) => {}
                        Err(error) => return Err(error.into()),
                    }
                    let Some(reread) = self.store.get_refresh_session(current.id()).await? else {
                        return Ok(DeveloperGenericOutcome::Accepted);
                    };
                    current = reread;
                }
                (Some(_), Some(next)) => {
                    let Some(successor) = self.store.get_refresh_session(&next).await? else {
                        return Ok(DeveloperGenericOutcome::Accepted);
                    };
                    current = successor;
                }
                (Some(_), None) => return Ok(DeveloperGenericOutcome::Accepted),
            }
        }
        Ok(DeveloperGenericOutcome::Accepted)
    }

    pub async fn waitlist_status(
        &self,
        identity_id: &DeveloperIdentityId,
        authorization_epoch: u64,
    ) -> Result<DeveloperWaitlistStatus, DeveloperWorkflowError> {
        let account = self
            .store
            .get_account(identity_id)
            .await?
            .ok_or(DeveloperWorkflowError::InvalidSession)?;
        if account.status() != DeveloperIdentityStatus::Waitlisted
            || account.authorization_epoch() != authorization_epoch
        {
            return Err(DeveloperWorkflowError::InvalidSession);
        }
        Ok(DeveloperWaitlistStatus {
            identity_id: account.id().clone(),
            status: DeveloperIdentityStatus::Waitlisted,
        })
    }

    pub async fn request_password_recovery(
        &self,
        email: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperGenericOutcome, DeveloperWorkflowError> {
        self.require_registration_ready()?;
        validate_public_text(source, 1, MAXIMUM_SOURCE_BYTES)?;
        let email =
            NormalizedEmail::parse(email).map_err(|_| DeveloperWorkflowError::InvalidRequest)?;
        self.enforce_request_limits("password-recovery", source, &email, now_unix_seconds)
            .await?;
        self.require_outbox_capacity().await?;
        let Some(identity) = self
            .store
            .get_authentication_identity_by_email(&email)
            .await?
        else {
            return Ok(DeveloperGenericOutcome::Accepted);
        };
        if identity.email_verified_at_unix_seconds().is_none()
            || identity.security_status() != AuthenticationSecurityStatus::Active
        {
            return Ok(DeveloperGenericOutcome::Accepted);
        }
        self.replace_or_create_token_for_authentication_identity(
            &identity,
            DeveloperAuthTokenPurpose::PasswordRecovery,
            DeveloperMailKind::PasswordRecovery,
            self.config.recovery_lifetime_seconds,
            now_unix_seconds,
        )
        .await?;
        Ok(DeveloperGenericOutcome::Accepted)
    }

    pub async fn complete_password_recovery(
        &self,
        raw_token: &str,
        new_password: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperRecoveryOutcome, DeveloperWorkflowError> {
        self.require_enabled()?;
        validate_public_text(source, 1, MAXIMUM_SOURCE_BYTES)?;
        if new_password.len() > MAXIMUM_PASSWORD_BYTES {
            return Err(DeveloperWorkflowError::InvalidRequest);
        }
        self.enforce_token_limit("recovery-token", source, now_unix_seconds)
            .await?;
        let Ok(raw_token) = DeveloperAuthToken::parse(raw_token) else {
            return Ok(DeveloperRecoveryOutcome::InvalidOrExpired);
        };
        let digest = raw_token.digest(
            self.store.lookup_key(),
            DeveloperAuthTokenPurpose::PasswordRecovery,
        );
        let Some(previous_token) = self.store.get_token(&digest).await? else {
            return Ok(DeveloperRecoveryOutcome::InvalidOrExpired);
        };
        if previous_token.purpose() != DeveloperAuthTokenPurpose::PasswordRecovery
            || previous_token.consumed_at_unix_seconds().is_some()
            || now_unix_seconds >= previous_token.expires_at_unix_seconds()
        {
            return Ok(DeveloperRecoveryOutcome::InvalidOrExpired);
        }
        let Some(previous_identity) = self
            .store
            .get_authentication_identity(previous_token.identity_id())
            .await?
        else {
            return Err(DeveloperWorkflowError::Registration(
                DeveloperRegistrationError::CorruptRecord,
            ));
        };
        if previous_identity.email_verified_at_unix_seconds().is_none()
            || previous_identity.security_status() != AuthenticationSecurityStatus::Active
        {
            return Ok(DeveloperRecoveryOutcome::InvalidOrExpired);
        }
        let new_hash = self
            .password_work
            .run(|| self.passwords.hash(new_password))?;
        let mut next_identity = previous_identity.clone();
        next_identity.replace_password_hash(new_hash.encoded(), now_unix_seconds)?;
        let mut consumed = previous_token.clone();
        consumed.consume(now_unix_seconds)?;
        match self
            .store
            .consume_token_and_replace_authentication_identity(
                &previous_identity,
                &next_identity,
                &previous_token,
                &consumed,
            )
            .await
        {
            Ok(()) => Ok(DeveloperRecoveryOutcome::Completed {
                identity_id: previous_identity.id().clone(),
            }),
            Err(DeveloperRegistrationError::Conflict) => {
                Ok(DeveloperRecoveryOutcome::InvalidOrExpired)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn review_page(
        &self,
        operator: &OperatorPrincipal,
        cursor: Option<&str>,
        limit: NonZeroUsize,
    ) -> Result<DeveloperReviewPage, DeveloperWorkflowError> {
        require_waitlist_review(operator)?;
        self.store
            .review_page(DeveloperIdentityStatus::Waitlisted, cursor, limit)
            .await
            .map_err(Into::into)
    }

    pub async fn review_detail(
        &self,
        operator: &OperatorPrincipal,
        identity_id: &DeveloperIdentityId,
    ) -> Result<Option<DeveloperAccount>, DeveloperWorkflowError> {
        require_waitlist_review(operator)?;
        self.store
            .get_account(identity_id)
            .await
            .map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn decide(
        &self,
        operator: &OperatorPrincipal,
        identity_id: &DeveloperIdentityId,
        kind: DeveloperDecisionKind,
        reason: &str,
        request_id: &str,
        idempotency_key: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperAccount, DeveloperWorkflowError> {
        require_waitlist_review(operator)?;
        self.require_outbox_capacity().await?;
        let reason = reason.trim();
        if !reason.is_empty() {
            validate_public_text(reason, 8, 1_024)?;
        }
        validate_public_text(request_id, 8, MAXIMUM_REQUEST_ID_BYTES)?;
        validate_public_text(idempotency_key, 8, MAXIMUM_IDEMPOTENCY_BYTES)?;
        let idempotency_digest = self
            .store
            .lookup_key()
            .secret_digest("waitlist-decision-idempotency", idempotency_key);
        let request_digest = decision_request_digest(identity_id, kind, reason);
        if let Some(existing) = self.store.get_decision(&idempotency_digest).await? {
            if existing.request_digest() != request_digest {
                return Err(DeveloperWorkflowError::Conflict);
            }
            return self.store.get_account(existing.identity_id()).await?.ok_or(
                DeveloperWorkflowError::Registration(DeveloperRegistrationError::CorruptRecord),
            );
        }
        let previous = self
            .store
            .get_account(identity_id)
            .await?
            .ok_or(DeveloperWorkflowError::NotFound)?;
        if previous.status() != DeveloperIdentityStatus::Waitlisted {
            return Err(DeveloperWorkflowError::Conflict);
        }
        let resulting_epoch = previous
            .authorization_epoch()
            .checked_add(1)
            .ok_or(DeveloperWorkflowError::InvalidRequest)?;
        let mut decision = DeveloperDecisionRecord::new(
            identity_id.clone(),
            kind,
            operator.id().as_str(),
            reason,
            request_id,
            idempotency_digest,
            request_digest,
            resulting_epoch,
            now_unix_seconds,
            checked_add(now_unix_seconds, self.config.decision_retention_seconds)?,
        )?;
        if operator.authentication_identity_id() == Some(identity_id) {
            decision.mark_self_review();
        }
        let mut next = previous.clone();
        next.record_decision(&decision)?;
        let mail_kind = match kind {
            DeveloperDecisionKind::Approve => DeveloperMailKind::Approved,
            DeveloperDecisionKind::Reject => DeveloperMailKind::Rejected,
        };
        let outbox = self.decision_mail(&next, mail_kind, now_unix_seconds)?;
        match self
            .store
            .decide(&previous, &next, &decision, &outbox)
            .await
        {
            Ok(()) => Ok(next),
            Err(DeveloperRegistrationError::Conflict) => {
                if let Some(existing) = self
                    .store
                    .get_decision(decision.idempotency_digest())
                    .await?
                    && existing.request_digest() == decision.request_digest()
                {
                    return self
                        .store
                        .get_account(identity_id)
                        .await?
                        .ok_or(DeveloperWorkflowError::NotFound);
                }
                Err(DeveloperWorkflowError::Conflict)
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn issue_session(
        &self,
        account: &DeveloperAccount,
        audience: DeveloperAccessAudience,
        now_unix_seconds: u64,
    ) -> Result<DeveloperSessionGrant, DeveloperWorkflowError> {
        let session_id = random_refresh_session_id()?;
        let credential = DeveloperRefreshCredential::generate(session_id.clone());
        let record = DeveloperRefreshSessionRecord::new(
            session_id,
            account.id().clone(),
            credential.digest(self.store.lookup_key()),
            audience,
            account.credential_epoch(),
            account.authorization_epoch(),
            now_unix_seconds,
            checked_add(now_unix_seconds, self.config.refresh_lifetime_seconds)?,
        )?;
        self.store.create_refresh_session(&record).await?;
        Ok(self.session_grant(account, &record, credential, now_unix_seconds))
    }

    fn session_grant(
        &self,
        account: &DeveloperAccount,
        session: &DeveloperRefreshSessionRecord,
        credential: DeveloperRefreshCredential,
        now_unix_seconds: u64,
    ) -> DeveloperSessionGrant {
        let cookie = format!(
            "__Secure-mako_developer_refresh={}; Path=/v1/developer-auth/sessions; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
            credential.encoded(),
            self.config.refresh_lifetime_seconds
        );
        DeveloperSessionGrant {
            claims: DeveloperAccessClaims {
                developer_identity_id: account.id().clone(),
                issuer: self.config.issuer.clone(),
                subject: account.id().as_str().to_owned(),
                audience: session.audience(),
                normalized_email: account.normalized_email().as_str().to_owned(),
                email_verified: account.email_verified_at_unix_seconds().is_some(),
                display_name: account.display_name().to_owned(),
                session_id: session.id().clone(),
                status: account.status(),
                credential_epoch: account.credential_epoch(),
                authorization_epoch: account.authorization_epoch(),
                authenticated_at_unix_seconds: now_unix_seconds,
                expires_at_unix_seconds: now_unix_seconds + self.config.access_lifetime_seconds,
            },
            refresh_credential: credential,
            cookie,
        }
    }

    async fn replace_or_create_token(
        &self,
        account: &DeveloperAccount,
        purpose: DeveloperAuthTokenPurpose,
        kind: DeveloperMailKind,
        lifetime_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperWorkflowError> {
        let raw = DeveloperAuthToken::generate();
        let next = self.token_record(
            account.id(),
            purpose,
            &raw,
            now_unix_seconds,
            lifetime_seconds,
        )?;
        let outbox = self.token_mail(
            account,
            kind,
            &raw,
            next.expires_at_unix_seconds(),
            now_unix_seconds,
        )?;
        if let Some(previous) = self.store.get_current_token(account.id(), purpose).await? {
            let mut invalidated = previous.clone();
            if invalidated.consumed_at_unix_seconds().is_some() {
                return Ok(());
            }
            invalidated.invalidate(now_unix_seconds)?;
            match self
                .store
                .replace_auth_token(&previous, &invalidated, &next, &outbox)
                .await
            {
                Ok(()) | Err(DeveloperRegistrationError::Conflict) => Ok(()),
                Err(error) => Err(error.into()),
            }
        } else {
            match self.store.create_auth_token(account, &next, &outbox).await {
                Ok(()) | Err(DeveloperRegistrationError::Conflict) => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    }

    async fn replace_or_create_token_for_authentication_identity(
        &self,
        identity: &AuthenticationIdentityRecord,
        purpose: DeveloperAuthTokenPurpose,
        kind: DeveloperMailKind,
        lifetime_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperWorkflowError> {
        let raw = DeveloperAuthToken::generate();
        let next = self.token_record(
            identity.id(),
            purpose,
            &raw,
            now_unix_seconds,
            lifetime_seconds,
        )?;
        let outbox = self.token_mail_for_authentication_identity(
            identity,
            kind,
            &raw,
            next.expires_at_unix_seconds(),
            now_unix_seconds,
        )?;
        if let Some(previous) = self.store.get_current_token(identity.id(), purpose).await? {
            let mut invalidated = previous.clone();
            if invalidated.consumed_at_unix_seconds().is_some() {
                return Ok(());
            }
            invalidated.invalidate(now_unix_seconds)?;
            match self
                .store
                .replace_auth_token(&previous, &invalidated, &next, &outbox)
                .await
            {
                Ok(()) | Err(DeveloperRegistrationError::Conflict) => Ok(()),
                Err(error) => Err(error.into()),
            }
        } else {
            match self
                .store
                .create_auth_token_for_authentication_identity(identity, &next, &outbox)
                .await
            {
                Ok(()) | Err(DeveloperRegistrationError::Conflict) => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    }

    fn token_record(
        &self,
        identity_id: &DeveloperIdentityId,
        purpose: DeveloperAuthTokenPurpose,
        raw: &DeveloperAuthToken,
        now_unix_seconds: u64,
        lifetime_seconds: u64,
    ) -> Result<DeveloperAuthTokenRecord, DeveloperWorkflowError> {
        DeveloperAuthTokenRecord::new(
            random_auth_token_id()?,
            identity_id.clone(),
            purpose,
            raw.digest(self.store.lookup_key(), purpose),
            now_unix_seconds,
            checked_add(now_unix_seconds, lifetime_seconds)?,
        )
        .map_err(Into::into)
    }

    fn token_mail(
        &self,
        account: &DeveloperAccount,
        kind: DeveloperMailKind,
        token: &DeveloperAuthToken,
        expires_at_unix_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<DeveloperMailOutboxRecord, DeveloperWorkflowError> {
        let (subject, route, action) = match kind {
            DeveloperMailKind::VerifyEmail => (
                "Verify your Mako Cloud developer email",
                "verify-email",
                "verify your email and join the developer wait list",
            ),
            DeveloperMailKind::PasswordRecovery => (
                "Reset your Mako Cloud developer password",
                "reset-password",
                "reset your developer password",
            ),
            _ => return Err(DeveloperWorkflowError::InvalidMail),
        };
        let body = format!(
            "Hello {},\n\nUse this single-use link to {action}:\n{}/{route}#token={}\n\nThis link expires at Unix time {expires_at_unix_seconds}. If you did not request this, ignore this message.\n",
            account.display_name(),
            self.config.public_origin,
            token.expose_once(),
        );
        self.mail_record(account, kind, subject, body, now_unix_seconds)
    }

    fn token_mail_for_authentication_identity(
        &self,
        identity: &AuthenticationIdentityRecord,
        kind: DeveloperMailKind,
        token: &DeveloperAuthToken,
        expires_at_unix_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<DeveloperMailOutboxRecord, DeveloperWorkflowError> {
        if kind != DeveloperMailKind::PasswordRecovery {
            return Err(DeveloperWorkflowError::InvalidMail);
        }
        let body = format!(
            "Hello {},\n\nUse this single-use link to reset your Mako Cloud password:\n{}/reset-password#token={}\n\nThis link expires at Unix time {expires_at_unix_seconds}. If you did not request this, ignore this message.\n",
            identity.display_name(),
            self.config.public_origin,
            token.expose_once(),
        );
        self.mail_record_for_authentication_identity(
            identity,
            kind,
            "Reset your Mako Cloud password",
            body,
            now_unix_seconds,
        )
    }

    fn decision_mail(
        &self,
        account: &DeveloperAccount,
        kind: DeveloperMailKind,
        now_unix_seconds: u64,
    ) -> Result<DeveloperMailOutboxRecord, DeveloperWorkflowError> {
        let (subject, body) = match kind {
            DeveloperMailKind::Approved => (
                "Your Mako Cloud developer account was approved",
                format!(
                    "Hello {},\n\nYour Mako Cloud developer account is active. Sign in again at {}/sign-in to continue. Approval does not automatically create or grant access to an organization.\n",
                    account.display_name(),
                    self.config.public_origin
                ),
            ),
            DeveloperMailKind::Rejected => (
                "Your Mako Cloud developer application was reviewed",
                format!(
                    "Hello {},\n\nYour Mako Cloud developer application was not approved. Private review notes are not included in email.\n",
                    account.display_name()
                ),
            ),
            _ => return Err(DeveloperWorkflowError::InvalidMail),
        };
        self.mail_record(account, kind, subject, body, now_unix_seconds)
    }

    fn mail_record(
        &self,
        account: &DeveloperAccount,
        kind: DeveloperMailKind,
        subject: impl Into<String>,
        body: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<DeveloperMailOutboxRecord, DeveloperWorkflowError> {
        let outbox_id = random_mail_outbox_id()?;
        let envelope =
            DeveloperMailEnvelope::new(account.normalized_email().clone(), subject, body)?;
        let encrypted = self
            .mail_cipher
            .encrypt(&outbox_id, account.id(), kind, &envelope)?;
        Ok(DeveloperMailOutboxRecord::new(
            outbox_id,
            account.id().clone(),
            kind,
            encrypted,
            now_unix_seconds,
        ))
    }

    fn mail_record_for_authentication_identity(
        &self,
        identity: &AuthenticationIdentityRecord,
        kind: DeveloperMailKind,
        subject: impl Into<String>,
        body: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<DeveloperMailOutboxRecord, DeveloperWorkflowError> {
        let outbox_id = random_mail_outbox_id()?;
        let envelope =
            DeveloperMailEnvelope::new(identity.normalized_email().clone(), subject, body)?;
        let encrypted = self
            .mail_cipher
            .encrypt(&outbox_id, identity.id(), kind, &envelope)?;
        Ok(DeveloperMailOutboxRecord::new(
            outbox_id,
            identity.id().clone(),
            kind,
            encrypted,
            now_unix_seconds,
        ))
    }

    async fn enforce_request_limits(
        &self,
        operation: &str,
        source: &str,
        email: &NormalizedEmail,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperWorkflowError> {
        self.enforce_rate_limit(
            &format!("{operation}:global"),
            "all",
            self.config.global_requests_per_window,
            now_unix_seconds,
        )
        .await?;
        self.enforce_rate_limit(
            &format!("{operation}:source"),
            source,
            self.config.source_requests_per_window,
            now_unix_seconds,
        )
        .await?;
        self.enforce_rate_limit(
            &format!("{operation}:email"),
            email.as_str(),
            self.config.email_requests_per_window,
            now_unix_seconds,
        )
        .await
    }

    async fn enforce_token_limit(
        &self,
        operation: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperWorkflowError> {
        self.enforce_rate_limit(
            &format!("{operation}:global"),
            "all",
            self.config.global_requests_per_window,
            now_unix_seconds,
        )
        .await?;
        self.enforce_rate_limit(
            &format!("{operation}:source"),
            source,
            self.config.token_attempts_per_window,
            now_unix_seconds,
        )
        .await
    }

    async fn enforce_rate_limit(
        &self,
        purpose: &str,
        value: &str,
        maximum: u32,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperWorkflowError> {
        let digest = self.store.lookup_key().secret_digest(purpose, value);
        let expires = checked_add(now_unix_seconds, self.config.rate_window_seconds)?;
        for _ in 0..4 {
            let previous = self.store.get_rate_limit(&digest).await?;
            match previous {
                None => {
                    let next = DeveloperRateLimitRecord::new(&digest, now_unix_seconds, expires)?;
                    match self.store.create_rate_limit(&next).await {
                        Ok(()) => return Ok(()),
                        Err(DeveloperRegistrationError::Conflict) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                Some(previous) => {
                    if now_unix_seconds < previous.expires_at_unix_seconds()
                        && previous.count() >= maximum
                    {
                        return Err(DeveloperWorkflowError::RateLimited);
                    }
                    let mut next = previous.clone();
                    next.increment_or_reset(now_unix_seconds, expires)?;
                    match self.store.replace_rate_limit(&previous, &next).await {
                        Ok(()) => return Ok(()),
                        Err(DeveloperRegistrationError::Conflict) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        Err(DeveloperWorkflowError::RateLimited)
    }

    fn require_enabled(&self) -> Result<(), DeveloperWorkflowError> {
        self.config
            .enabled
            .then_some(())
            .ok_or(DeveloperWorkflowError::Unavailable)
    }

    fn require_registration_ready(&self) -> Result<(), DeveloperWorkflowError> {
        self.config
            .registration_ready()
            .then_some(())
            .ok_or(DeveloperWorkflowError::Unavailable)
    }

    async fn require_outbox_capacity(&self) -> Result<(), DeveloperWorkflowError> {
        let depth = self.store.outbox_depth().await?;
        if depth >= self.config.maximum_pending_outbox {
            Err(DeveloperWorkflowError::Unavailable)
        } else {
            Ok(())
        }
    }
}

fn require_waitlist_review(operator: &OperatorPrincipal) -> Result<(), DeveloperWorkflowError> {
    operator
        .allows(OperatorPermission::WaitlistReview)
        .then_some(())
        .ok_or(DeveloperWorkflowError::Forbidden)
}

fn access_audience(status: DeveloperIdentityStatus) -> Option<DeveloperAccessAudience> {
    match status {
        DeveloperIdentityStatus::Active => Some(DeveloperAccessAudience::Active),
        DeveloperIdentityStatus::Waitlisted => Some(DeveloperAccessAudience::Waitlist),
        _ => None,
    }
}

fn decision_request_digest(
    identity_id: &DeveloperIdentityId,
    kind: DeveloperDecisionKind,
    reason: &str,
) -> String {
    blake3::hash(format!("{}\0{kind:?}\0{reason}", identity_id.as_str()).as_bytes())
        .to_hex()
        .to_string()
}

fn validate_public_text(
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), DeveloperWorkflowError> {
    if !(minimum..=maximum).contains(&value.len())
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DeveloperWorkflowError::InvalidRequest);
    }
    Ok(())
}

fn checked_add(value: u64, increment: u64) -> Result<u64, DeveloperWorkflowError> {
    value
        .checked_add(increment)
        .ok_or(DeveloperWorkflowError::InvalidRequest)
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let maximum = left.len().max(right.len());
    for index in 0..maximum {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn random_suffix() -> String {
    let mut bytes = [0_u8; 18];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn random_developer_id() -> Result<DeveloperIdentityId, DeveloperWorkflowError> {
    DeveloperIdentityId::parse(format!("dev_{}", random_suffix()))
        .map_err(|_| DeveloperWorkflowError::InvalidConfiguration)
}

fn random_auth_token_id() -> Result<DeveloperAuthTokenId, DeveloperWorkflowError> {
    DeveloperAuthTokenId::parse(format!("dat_{}", random_suffix())).map_err(Into::into)
}

fn random_refresh_session_id() -> Result<DeveloperRefreshSessionId, DeveloperWorkflowError> {
    DeveloperRefreshSessionId::parse(format!("drs_{}", random_suffix())).map_err(Into::into)
}

fn random_mail_outbox_id() -> Result<DeveloperMailOutboxId, DeveloperWorkflowError> {
    DeveloperMailOutboxId::parse(format!("dmo_{}", random_suffix())).map_err(Into::into)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeveloperMailFailureKind {
    Transient,
    Permanent,
    Ambiguous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperMailTransportError {
    pub kind: DeveloperMailFailureKind,
    pub stable_code: &'static str,
}

impl fmt::Display for DeveloperMailTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("developer mail delivery failed")
    }
}

impl Error for DeveloperMailTransportError {}

#[async_trait]
pub trait DeveloperMailTransport: Send + Sync {
    async fn readiness(&self) -> Result<(), DeveloperMailTransportError>;

    async fn deliver(
        &self,
        delivery_id: &str,
        envelope: &DeveloperMailEnvelope,
    ) -> Result<(), DeveloperMailTransportError>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeveloperOutboxWorkerReport {
    pub leased: usize,
    pub delivered: usize,
    pub retried: usize,
    pub dead_lettered: usize,
    pub conflicts: usize,
}

#[derive(Clone)]
pub struct DeveloperMailOutboxWorker {
    store: DeveloperRegistrationStore,
    cipher: DeveloperMailCipher,
    transport: Arc<dyn DeveloperMailTransport>,
    config: DeveloperRegistrationConfig,
}

impl fmt::Debug for DeveloperMailOutboxWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperMailOutboxWorker")
            .finish_non_exhaustive()
    }
}

impl DeveloperMailOutboxWorker {
    pub fn new(
        store: DeveloperRegistrationStore,
        cipher: DeveloperMailCipher,
        transport: Arc<dyn DeveloperMailTransport>,
        config: DeveloperRegistrationConfig,
    ) -> Result<Self, DeveloperWorkflowError> {
        config.validate()?;
        Ok(Self {
            store,
            cipher,
            transport,
            config,
        })
    }

    pub async fn readiness(&self) -> Result<(), DeveloperWorkflowError> {
        if !self.config.mail_ready {
            return Err(DeveloperWorkflowError::Unavailable);
        }
        self.transport
            .readiness()
            .await
            .map_err(DeveloperWorkflowError::MailTransport)
    }

    pub async fn run_once(
        &self,
        now_unix_seconds: u64,
    ) -> Result<DeveloperOutboxWorkerReport, DeveloperWorkflowError> {
        let mut report = DeveloperOutboxWorkerReport::default();
        let records = self
            .store
            .pending_outbox(
                now_unix_seconds,
                NonZeroUsize::new(self.config.maximum_outbox_batch)
                    .ok_or(DeveloperWorkflowError::InvalidConfiguration)?,
            )
            .await?;
        for previous in records {
            let mut leased = previous.clone();
            leased.lease(now_unix_seconds, self.config.outbox_lease_seconds)?;
            match self.store.replace_outbox(&previous, &leased).await {
                Ok(()) => report.leased += 1,
                Err(DeveloperRegistrationError::Conflict) => {
                    report.conflicts += 1;
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            let delivery = match self.cipher.decrypt(&leased) {
                Ok(envelope) => {
                    self.transport
                        .deliver(leased.id().as_str(), &envelope)
                        .await
                }
                Err(_) => Err(DeveloperMailTransportError {
                    kind: DeveloperMailFailureKind::Permanent,
                    stable_code: "invalid_envelope",
                }),
            };
            let mut completed = leased.clone();
            match delivery {
                Ok(()) => {
                    completed.delivered(now_unix_seconds)?;
                    report.delivered += 1;
                }
                Err(error) => {
                    let permanent = error.kind == DeveloperMailFailureKind::Permanent;
                    let maximum_attempts = if permanent {
                        completed.attempts()
                    } else {
                        self.config.outbox_maximum_attempts
                    };
                    let backoff = outbox_backoff(
                        completed.attempts(),
                        self.config.outbox_maximum_backoff_seconds,
                    );
                    completed.retry(
                        now_unix_seconds,
                        checked_add(now_unix_seconds, backoff)?,
                        error.stable_code,
                        maximum_attempts,
                    )?;
                    if completed.state() == DeveloperMailOutboxState::DeadLetter {
                        report.dead_lettered += 1;
                    } else {
                        report.retried += 1;
                    }
                }
            }
            match self.store.replace_outbox(&leased, &completed).await {
                Ok(()) => {}
                Err(DeveloperRegistrationError::Conflict) => report.conflicts += 1,
                Err(error) => return Err(error.into()),
            }
        }
        self.store
            .cleanup_expired(
                now_unix_seconds,
                self.config.delivered_mail_retention_seconds,
                NonZeroUsize::new(self.config.maximum_outbox_batch)
                    .ok_or(DeveloperWorkflowError::InvalidConfiguration)?,
            )
            .await?;
        Ok(report)
    }
}

pub(crate) fn outbox_backoff(attempt: u32, maximum: u64) -> u64 {
    let exponent = attempt.saturating_sub(1).min(20);
    30_u64.saturating_mul(1_u64 << exponent).min(maximum)
}

#[derive(Debug)]
pub enum DeveloperWorkflowError {
    InvalidConfiguration,
    InvalidRequest,
    InvalidCredentials,
    InvalidSession,
    Unavailable,
    Busy,
    RateLimited,
    Forbidden,
    NotFound,
    Conflict,
    InvalidMail,
    MailEncryption,
    Password(PasswordError),
    Registration(DeveloperRegistrationError),
    MailTransport(DeveloperMailTransportError),
    Json(serde_json::Error),
}

impl fmt::Display for DeveloperWorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "developer registration configuration is invalid",
            Self::InvalidRequest => "developer request is invalid",
            Self::InvalidCredentials => "developer credentials are invalid",
            Self::InvalidSession => "developer session is invalid",
            Self::Unavailable => "developer registration is unavailable",
            Self::Busy => "developer authentication is busy",
            Self::RateLimited => "developer operation is rate limited",
            Self::Forbidden => "developer wait-list operation is forbidden",
            Self::NotFound => "developer wait-list identity was not found",
            Self::Conflict => "developer wait-list state changed",
            Self::InvalidMail => "developer mail is invalid",
            Self::MailEncryption => "developer mail encryption failed",
            Self::Password(_) => "developer password operation failed",
            Self::Registration(_) => "developer registration storage failed",
            Self::MailTransport(_) => "developer mail delivery failed",
            Self::Json(_) => "developer workflow encoding failed",
        })
    }
}

impl Error for DeveloperWorkflowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Password(error) => Some(error),
            Self::Registration(error) => Some(error),
            Self::MailTransport(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DeveloperRegistrationError> for DeveloperWorkflowError {
    fn from(value: DeveloperRegistrationError) -> Self {
        match value {
            DeveloperRegistrationError::RateLimited => Self::RateLimited,
            DeveloperRegistrationError::Conflict => Self::Conflict,
            other => Self::Registration(other),
        }
    }
}

impl From<serde_json::Error> for DeveloperWorkflowError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::executor::block_on;
    use mako_storage::{Durability, MemoryAdapter};

    use super::*;
    use crate::{DeveloperLookupKey, OperatorId};

    const NOW: u64 = 1_800_000_000;
    const PASSWORD: &str = "correct horse battery staple";

    fn test_service() -> (DeveloperRegistrationService, DeveloperMailCipher) {
        let store = DeveloperRegistrationStore::new(
            Arc::new(MemoryAdapter::new()),
            Durability::Memory,
            DeveloperLookupKey::derive(b"workflow private lookup key"),
        )
        .expect("store");
        let config = DeveloperRegistrationConfig {
            enabled: true,
            mail_ready: true,
            public_origin: "https://cloud.example.test".to_owned(),
            issuer: "https://cloud.example.test/v1/developer-auth".to_owned(),
            ..DeveloperRegistrationConfig::default()
        };
        let key = DeveloperMailEncryptionKey::derive(b"workflow private mail key");
        let cipher = DeveloperMailCipher::new(key.clone());
        (
            DeveloperRegistrationService::new(store, config, key).expect("service"),
            cipher,
        )
    }

    async fn token_from_pending_mail(
        service: &DeveloperRegistrationService,
        cipher: &DeveloperMailCipher,
        kind: DeveloperMailKind,
        now: u64,
    ) -> String {
        let records = service
            .store()
            .pending_outbox(now, NonZeroUsize::new(100).expect("limit"))
            .await
            .expect("outbox");
        let record = records
            .iter()
            .rev()
            .find(|record| record.kind() == kind)
            .expect("mail kind");
        let envelope = cipher.decrypt(record).expect("decrypt");
        envelope
            .text_body()
            .split("#token=")
            .nth(1)
            .and_then(|value| value.lines().next())
            .expect("mail token")
            .to_owned()
    }

    #[test]
    fn hosted_registration_waitlist_approval_and_recovery_are_isolated() {
        block_on(async {
            let (service, cipher) = test_service();
            assert_eq!(
                service
                    .register(
                        "Person@Example.Test",
                        "Preview Developer",
                        PASSWORD,
                        "source-a",
                        NOW,
                    )
                    .await
                    .expect("registration"),
                DeveloperGenericOutcome::Accepted
            );
            assert_eq!(
                service
                    .register(
                        "person@example.test",
                        "Different Name",
                        PASSWORD,
                        "source-a",
                        NOW + 1,
                    )
                    .await
                    .expect("duplicate registration"),
                DeveloperGenericOutcome::Accepted
            );
            let email = NormalizedEmail::parse("person@example.test").expect("email");
            let account = service
                .store()
                .get_account_by_email(&email)
                .await
                .expect("account read")
                .expect("account");
            assert_eq!(account.display_name(), "Preview Developer");
            assert_eq!(account.status(), DeveloperIdentityStatus::Unverified);

            let verification =
                token_from_pending_mail(&service, &cipher, DeveloperMailKind::VerifyEmail, NOW + 2)
                    .await;
            assert_eq!(
                service
                    .verify_email(&verification, "source-a", NOW + 2)
                    .await
                    .expect("verification"),
                DeveloperVerificationOutcome::Verified
            );
            assert_eq!(
                service
                    .verify_email(&verification, "source-a", NOW + 3)
                    .await
                    .expect("replay"),
                DeveloperVerificationOutcome::InvalidOrExpired
            );

            let waitlist = service
                .sign_in("person@example.test", PASSWORD, "source-a", NOW + 4)
                .await
                .expect("wait-list sign in");
            assert_eq!(waitlist.claims.audience, DeveloperAccessAudience::Waitlist);
            assert!(
                waitlist
                    .set_cookie_header()
                    .contains("Secure; HttpOnly; SameSite=Strict")
            );
            let original_refresh = waitlist.refresh_credential().encoded().to_owned();
            let rotated = service
                .refresh(&original_refresh, NOW + 5)
                .await
                .expect("refresh rotation");
            assert!(matches!(
                service.refresh(&original_refresh, NOW + 6).await,
                Err(DeveloperWorkflowError::InvalidSession)
            ));

            let denied = OperatorPrincipal::for_test(
                OperatorId::parse("opr_denied001").expect("operator"),
                [OperatorPermission::TenantRead],
            );
            assert!(matches!(
                service
                    .review_page(&denied, None, NonZeroUsize::new(10).expect("limit"))
                    .await,
                Err(DeveloperWorkflowError::Forbidden)
            ));
            let reviewer = OperatorPrincipal::for_test_identity(
                OperatorId::parse("opr_reviewer01").expect("operator"),
                account.id().clone(),
                [OperatorPermission::WaitlistReview],
            );
            let page = service
                .review_page(&reviewer, None, NonZeroUsize::new(10).expect("limit"))
                .await
                .expect("queue");
            assert_eq!(page.accounts.len(), 1);
            assert!(matches!(
                service
                    .decide(
                        &reviewer,
                        account.id(),
                        DeveloperDecisionKind::Approve,
                        "short",
                        "request_invalid_reason",
                        "idempotency_invalid_reason",
                        NOW + 7,
                    )
                    .await,
                Err(DeveloperWorkflowError::InvalidRequest)
            ));
            let approved = service
                .decide(
                    &reviewer,
                    account.id(),
                    DeveloperDecisionKind::Approve,
                    " \t ",
                    "request_approve_001",
                    "idempotency_approve_001",
                    NOW + 7,
                )
                .await
                .expect("approval");
            assert_eq!(approved.status(), DeveloperIdentityStatus::Active);
            let decision_digest = service
                .store()
                .lookup_key()
                .secret_digest("waitlist-decision-idempotency", "idempotency_approve_001");
            let decision = service
                .store()
                .get_decision(&decision_digest)
                .await
                .expect("decision read")
                .expect("decision");
            assert!(decision.self_review());
            assert!(!decision.reason_provided());
            assert_eq!(decision.reason(), "");
            let outbox_before_replay = service
                .store()
                .pending_outbox(NOW + 8, NonZeroUsize::new(100).expect("limit"))
                .await
                .expect("outbox before replay")
                .len();
            let repeated = service
                .decide(
                    &reviewer,
                    account.id(),
                    DeveloperDecisionKind::Approve,
                    "",
                    "request_approve_001",
                    "idempotency_approve_001",
                    NOW + 8,
                )
                .await
                .expect("idempotent approval");
            assert_eq!(repeated, approved);
            assert_eq!(
                service
                    .store()
                    .pending_outbox(NOW + 8, NonZeroUsize::new(100).expect("limit"))
                    .await
                    .expect("outbox after replay")
                    .len(),
                outbox_before_replay
            );
            assert!(matches!(
                service
                    .refresh(rotated.refresh_credential().encoded(), NOW + 8)
                    .await,
                Err(DeveloperWorkflowError::InvalidSession)
            ));
            let active = service
                .sign_in("person@example.test", PASSWORD, "source-a", NOW + 9)
                .await
                .expect("active sign in");
            assert_eq!(active.claims.audience, DeveloperAccessAudience::Active);

            // Signing out with the cookie from before a refresh ends the
            // session that refresh rotated it into.
            let other = service
                .sign_in("person@example.test", PASSWORD, "source-b", NOW + 9)
                .await
                .expect("second sign in");
            let stale = other.refresh_credential().encoded().to_owned();
            let refreshed = service
                .refresh(&stale, NOW + 9)
                .await
                .expect("refresh of the second session");
            service.sign_out(&stale, NOW + 9).await.expect("sign out");
            assert!(matches!(
                service
                    .refresh(refreshed.refresh_credential().encoded(), NOW + 9)
                    .await,
                Err(DeveloperWorkflowError::InvalidSession)
            ));
            assert!(
                service
                    .refresh(active.refresh_credential().encoded(), NOW + 9)
                    .await
                    .is_ok(),
                "another session of the same developer stays signed in"
            );
            let before_recovery = service
                .store()
                .get_account(account.id())
                .await
                .expect("account read")
                .expect("account");

            service
                .request_password_recovery("person@example.test", "source-b", NOW + 10)
                .await
                .expect("recovery request");
            let recovery = token_from_pending_mail(
                &service,
                &cipher,
                DeveloperMailKind::PasswordRecovery,
                NOW + 10,
            )
            .await;
            assert!(matches!(
                service
                    .complete_password_recovery(
                        &recovery,
                        "a newly recovered developer password",
                        "source-b",
                        NOW + 11,
                    )
                    .await
                    .expect("recovery completion"),
                DeveloperRecoveryOutcome::Completed { .. }
            ));
            assert_eq!(
                service
                    .complete_password_recovery(
                        &recovery,
                        "another valid developer password",
                        "source-b",
                        NOW + 12,
                    )
                    .await
                    .expect("recovery replay"),
                DeveloperRecoveryOutcome::InvalidOrExpired
            );
            assert!(matches!(
                service
                    .refresh(active.refresh_credential().encoded(), NOW + 12)
                    .await,
                Err(DeveloperWorkflowError::InvalidSession)
            ));
            let after_recovery = service
                .store()
                .get_account(account.id())
                .await
                .expect("account read")
                .expect("account");
            assert_eq!(after_recovery.status(), before_recovery.status());
            assert_eq!(
                after_recovery.authorization_epoch(),
                before_recovery.authorization_epoch()
            );
            assert_eq!(
                after_recovery.credential_epoch(),
                before_recovery.credential_epoch() + 1
            );
            assert!(!format!("{active:?}").contains(active.refresh_credential().encoded()));
        });
    }

    #[test]
    fn operator_only_identity_can_recover_shared_credentials_without_gaining_developer_role() {
        block_on(async {
            let (service, cipher) = test_service();
            let identity_id =
                DeveloperIdentityId::parse("dev_operatoronly01").expect("identity id");
            let password_hash = service.passwords.hash(PASSWORD).expect("password hash");
            let identity = AuthenticationIdentityRecord::new_verified(
                identity_id.clone(),
                NormalizedEmail::parse("operator-only@example.test").expect("email"),
                "Operator Only",
                password_hash.encoded(),
                NOW,
            )
            .expect("identity");
            service
                .store()
                .create_authentication_identity(&identity)
                .await
                .expect("create identity");

            service
                .request_password_recovery(
                    "operator-only@example.test",
                    "source-operator-only",
                    NOW + 1,
                )
                .await
                .expect("recovery request");
            let token = token_from_pending_mail(
                &service,
                &cipher,
                DeveloperMailKind::PasswordRecovery,
                NOW + 1,
            )
            .await;
            assert!(matches!(
                service
                    .complete_password_recovery(
                        &token,
                        "a newly recovered operator password",
                        "source-operator-only",
                        NOW + 2,
                    )
                    .await
                    .expect("recovery completion"),
                DeveloperRecoveryOutcome::Completed { .. }
            ));
            let recovered = service
                .store()
                .get_authentication_identity(&identity_id)
                .await
                .expect("identity read")
                .expect("identity");
            assert_eq!(
                recovered.credential_epoch(),
                identity.credential_epoch() + 1
            );
            assert!(
                service
                    .store()
                    .get_developer_role(&identity_id)
                    .await
                    .expect("role read")
                    .is_none()
            );
        });
    }

    #[derive(Default)]
    struct CapturingTransport(Mutex<Vec<String>>);

    #[async_trait]
    impl DeveloperMailTransport for CapturingTransport {
        async fn readiness(&self) -> Result<(), DeveloperMailTransportError> {
            Ok(())
        }

        async fn deliver(
            &self,
            delivery_id: &str,
            envelope: &DeveloperMailEnvelope,
        ) -> Result<(), DeveloperMailTransportError> {
            assert_eq!(envelope.recipient().as_str(), "person@example.test");
            self.0
                .lock()
                .expect("capture lock")
                .push(delivery_id.to_owned());
            Ok(())
        }
    }

    #[test]
    fn durable_worker_claims_and_delivers_each_outbox_record_once() {
        block_on(async {
            let (service, cipher) = test_service();
            service
                .register(
                    "person@example.test",
                    "Preview Developer",
                    PASSWORD,
                    "source-a",
                    NOW,
                )
                .await
                .expect("registration");
            let transport = Arc::new(CapturingTransport::default());
            let worker = DeveloperMailOutboxWorker::new(
                service.store().clone(),
                cipher,
                transport.clone(),
                service.config().clone(),
            )
            .expect("worker");
            worker.readiness().await.expect("readiness");
            let first = worker.run_once(NOW).await.expect("first pass");
            assert_eq!(first.delivered, 1);
            let second = worker.run_once(NOW + 1).await.expect("second pass");
            assert_eq!(second.delivered, 0);
            assert_eq!(transport.0.lock().expect("capture lock").len(), 1);
        });
    }

    #[test]
    fn registration_is_disabled_without_mail_and_password_work_is_bounded() {
        let (mut service, _) = test_service();
        service.config.mail_ready = false;
        assert!(matches!(
            block_on(service.register(
                "person@example.test",
                "Preview Developer",
                PASSWORD,
                "source-a",
                NOW,
            )),
            Err(DeveloperWorkflowError::Unavailable)
        ));
        service
            .password_work
            .active
            .store(service.password_work.maximum, Ordering::Release);
        assert!(matches!(
            service
                .password_work
                .run(|| service.passwords.hash(PASSWORD)),
            Err(DeveloperWorkflowError::Busy)
        ));
    }
}
