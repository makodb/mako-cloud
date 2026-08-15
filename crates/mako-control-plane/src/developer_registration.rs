use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_identity::NormalizedEmail;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter,
    ScanDirection, ScanRequest, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{ControlKeyspace, ControlKeyspaceError, DeveloperIdentityId, DeveloperIdentityStatus};

const AUTHENTICATION_IDENTITY_SCHEMA_VERSION: u32 = 1;
const DEVELOPER_ROLE_SCHEMA_VERSION: u32 = 1;
const DEVELOPER_MIGRATION_VERSION: u32 = 2;
const DEVELOPER_REFRESH_SESSION_SCHEMA_VERSION: u32 = 2;
const SECRET_BYTES: usize = 32;
const MAXIMUM_REVIEW_PAGE: usize = 100;
const MAXIMUM_MIGRATION_RECORDS: usize = 10_000;
const MAXIMUM_ENCRYPTED_MAIL_BYTES: usize = 64 * 1024;

macro_rules! developer_id {
    ($name:ident, $prefix:literal, $field:literal) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, DeveloperRegistrationError> {
                let value = value.into();
                validate_identifier($field, &value, $prefix)?;
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

developer_id!(DeveloperAuthTokenId, "dat_", "developer auth token id");
developer_id!(
    DeveloperRefreshSessionId,
    "drs_",
    "developer refresh session id"
);
developer_id!(DeveloperMailOutboxId, "dmo_", "developer mail outbox id");

#[derive(Clone, Eq, PartialEq)]
pub struct DeveloperLookupKey([u8; 32]);

impl DeveloperLookupKey {
    #[must_use]
    pub fn derive(secret: &[u8]) -> Self {
        Self(blake3::derive_key(
            "mako/control-plane/developer-private-index/v1",
            secret,
        ))
    }

    #[must_use]
    pub fn email_digest(&self, email: &NormalizedEmail) -> String {
        blake3::keyed_hash(&self.0, email.as_str().as_bytes())
            .to_hex()
            .to_string()
    }

    pub(crate) fn secret_digest(&self, purpose: &str, secret: &str) -> String {
        let mut input = Vec::with_capacity(purpose.len() + 1 + secret.len());
        input.extend_from_slice(purpose.as_bytes());
        input.push(0);
        input.extend_from_slice(secret.as_bytes());
        blake3::keyed_hash(&self.0, &input).to_hex().to_string()
    }
}

impl fmt::Debug for DeveloperLookupKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeveloperLookupKey([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationSecurityStatus {
    Active,
    Suspended,
    Deleted,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthenticationIdentityRecord {
    schema_version: u32,
    id: DeveloperIdentityId,
    normalized_email: NormalizedEmail,
    display_name: String,
    password_hash: Option<String>,
    email_verified_at_unix_seconds: Option<u64>,
    security_status: AuthenticationSecurityStatus,
    credential_epoch: u64,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    last_authenticated_at_unix_seconds: Option<u64>,
}

impl AuthenticationIdentityRecord {
    fn new_unverified(
        id: DeveloperIdentityId,
        normalized_email: NormalizedEmail,
        display_name: impl Into<String>,
        password_hash: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let identity = Self {
            schema_version: AUTHENTICATION_IDENTITY_SCHEMA_VERSION,
            id,
            normalized_email,
            display_name: display_name.into(),
            password_hash: Some(password_hash.into()),
            email_verified_at_unix_seconds: None,
            security_status: AuthenticationSecurityStatus::Active,
            credential_epoch: 1,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            last_authenticated_at_unix_seconds: None,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn new_verified(
        id: DeveloperIdentityId,
        normalized_email: NormalizedEmail,
        display_name: impl Into<String>,
        password_hash: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let mut identity = Self::new_unverified(
            id,
            normalized_email,
            display_name,
            password_hash,
            now_unix_seconds,
        )?;
        identity.email_verified_at_unix_seconds = Some(now_unix_seconds);
        identity.validate()?;
        Ok(identity)
    }

    #[allow(clippy::too_many_arguments)]
    fn migrated(
        id: DeveloperIdentityId,
        normalized_email: NormalizedEmail,
        display_name: impl Into<String>,
        password_hash: Option<String>,
        email_verified_at_unix_seconds: Option<u64>,
        security_status: AuthenticationSecurityStatus,
        credential_epoch: u64,
        created_at_unix_seconds: u64,
        updated_at_unix_seconds: u64,
        last_authenticated_at_unix_seconds: Option<u64>,
    ) -> Result<Self, DeveloperRegistrationError> {
        let identity = Self {
            schema_version: AUTHENTICATION_IDENTITY_SCHEMA_VERSION,
            id,
            normalized_email,
            display_name: display_name.into(),
            password_hash,
            email_verified_at_unix_seconds,
            security_status,
            credential_epoch,
            created_at_unix_seconds,
            updated_at_unix_seconds,
            last_authenticated_at_unix_seconds,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), DeveloperRegistrationError> {
        if self.schema_version != AUTHENTICATION_IDENTITY_SCHEMA_VERSION {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        validate_text("authentication display name", &self.display_name, 1, 200)?;
        if self.password_hash.as_ref().is_some_and(|value| {
            value.len() < 32
                || value.len() > 1_024
                || value.chars().any(char::is_control)
                || !value.starts_with("$argon2id$")
        }) {
            return Err(DeveloperRegistrationError::InvalidField(
                "authentication password hash",
            ));
        }
        if self.credential_epoch == 0
            || self.updated_at_unix_seconds < self.created_at_unix_seconds
            || self
                .last_authenticated_at_unix_seconds
                .is_some_and(|value| value < self.created_at_unix_seconds)
            || self
                .email_verified_at_unix_seconds
                .is_some_and(|value| value < self.created_at_unix_seconds)
        {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &DeveloperIdentityId {
        &self.id
    }

    #[must_use]
    pub fn normalized_email(&self) -> &NormalizedEmail {
        &self.normalized_email
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    #[must_use]
    pub fn password_hash(&self) -> Option<&str> {
        self.password_hash.as_deref()
    }

    #[must_use]
    pub const fn email_verified_at_unix_seconds(&self) -> Option<u64> {
        self.email_verified_at_unix_seconds
    }

    #[must_use]
    pub const fn security_status(&self) -> AuthenticationSecurityStatus {
        self.security_status
    }

    #[must_use]
    pub const fn credential_epoch(&self) -> u64 {
        self.credential_epoch
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    pub(crate) fn mark_email_verified(
        &mut self,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if now_unix_seconds < self.updated_at_unix_seconds {
            return Err(DeveloperRegistrationError::InvalidField(
                "email verification timestamp",
            ));
        }
        self.email_verified_at_unix_seconds = Some(now_unix_seconds);
        self.updated_at_unix_seconds = now_unix_seconds;
        self.validate()
    }

    pub fn transition_security(
        &mut self,
        next: AuthenticationSecurityStatus,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        let allowed = self.security_status == next
            || matches!(
                (self.security_status, next),
                (
                    AuthenticationSecurityStatus::Active,
                    AuthenticationSecurityStatus::Suspended
                ) | (
                    AuthenticationSecurityStatus::Suspended,
                    AuthenticationSecurityStatus::Active
                ) | (_, AuthenticationSecurityStatus::Deleted)
            );
        if !allowed || now_unix_seconds < self.updated_at_unix_seconds {
            return Err(DeveloperRegistrationError::InvalidSecurityTransition);
        }
        if self.security_status != next {
            self.security_status = next;
            self.credential_epoch = self
                .credential_epoch
                .checked_add(1)
                .ok_or(DeveloperRegistrationError::CorruptRecord)?;
            self.updated_at_unix_seconds = now_unix_seconds;
        }
        self.validate()
    }

    pub fn replace_password_hash(
        &mut self,
        encoded: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if now_unix_seconds < self.updated_at_unix_seconds {
            return Err(DeveloperRegistrationError::InvalidField(
                "authentication password timestamp",
            ));
        }
        self.password_hash = Some(encoded.into());
        self.credential_epoch = self
            .credential_epoch
            .checked_add(1)
            .ok_or(DeveloperRegistrationError::CorruptRecord)?;
        self.updated_at_unix_seconds = now_unix_seconds;
        self.validate()
    }

    pub(crate) fn upgrade_password_hash(
        &mut self,
        encoded: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if now_unix_seconds < self.updated_at_unix_seconds {
            return Err(DeveloperRegistrationError::InvalidField(
                "authentication password timestamp",
            ));
        }
        self.password_hash = Some(encoded.into());
        self.updated_at_unix_seconds = now_unix_seconds;
        self.validate()
    }

    pub(crate) fn record_authentication(
        &mut self,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if now_unix_seconds < self.updated_at_unix_seconds {
            return Err(DeveloperRegistrationError::InvalidField(
                "authentication timestamp",
            ));
        }
        self.last_authenticated_at_unix_seconds = Some(now_unix_seconds);
        self.updated_at_unix_seconds = now_unix_seconds;
        self.validate()
    }
}

impl fmt::Debug for AuthenticationIdentityRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticationIdentityRecord")
            .field("id", &self.id)
            .field("security_status", &self.security_status)
            .field("credential_epoch", &self.credential_epoch)
            .field("private_fields", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperRoleRecord {
    schema_version: u32,
    identity_id: DeveloperIdentityId,
    status: DeveloperIdentityStatus,
    authorization_epoch: u64,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    review: Option<DeveloperDecisionSummary>,
}

impl DeveloperRoleRecord {
    fn new_unverified(
        identity_id: DeveloperIdentityId,
        now_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let role = Self {
            schema_version: DEVELOPER_ROLE_SCHEMA_VERSION,
            identity_id,
            status: DeveloperIdentityStatus::Unverified,
            authorization_epoch: 1,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            review: None,
        };
        role.validate()?;
        Ok(role)
    }

    pub fn new_waitlisted(
        identity_id: DeveloperIdentityId,
        now_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let role = Self {
            schema_version: DEVELOPER_ROLE_SCHEMA_VERSION,
            identity_id,
            status: DeveloperIdentityStatus::Waitlisted,
            authorization_epoch: 1,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            review: None,
        };
        role.validate()?;
        Ok(role)
    }

    /// Build an already-active developer role for the local bootstrap tool.
    ///
    /// Hosted registration reaches `Active` only through operator wait-list
    /// review, which requires mail delivery and an entitled operator. A local
    /// development environment has neither, and a tool that already owns the
    /// control store could write these bytes directly regardless, so this
    /// constructor grants no authority that direct store access does not. It
    /// does not bypass any check on the serving path: session issuance still
    /// reads the stored status and epoch.
    pub fn new_active_for_local_bootstrap(
        identity_id: DeveloperIdentityId,
        now_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let role = Self {
            schema_version: DEVELOPER_ROLE_SCHEMA_VERSION,
            identity_id,
            status: DeveloperIdentityStatus::Active,
            authorization_epoch: 1,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            review: None,
        };
        role.validate()?;
        Ok(role)
    }

    fn migrated(
        identity_id: DeveloperIdentityId,
        status: DeveloperIdentityStatus,
        authorization_epoch: u64,
        created_at_unix_seconds: u64,
        updated_at_unix_seconds: u64,
        review: Option<DeveloperDecisionSummary>,
    ) -> Result<Self, DeveloperRegistrationError> {
        let role = Self {
            schema_version: DEVELOPER_ROLE_SCHEMA_VERSION,
            identity_id,
            status,
            authorization_epoch,
            created_at_unix_seconds,
            updated_at_unix_seconds,
            review,
        };
        role.validate()?;
        Ok(role)
    }

    pub fn validate(&self) -> Result<(), DeveloperRegistrationError> {
        if self.schema_version != DEVELOPER_ROLE_SCHEMA_VERSION
            || self.authorization_epoch == 0
            || self.updated_at_unix_seconds < self.created_at_unix_seconds
        {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        if self.review.as_ref().is_some_and(|review| {
            review.identity_id != self.identity_id
                || review.decided_at_unix_seconds < self.created_at_unix_seconds
        }) {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(())
    }

    #[must_use]
    pub const fn status(&self) -> DeveloperIdentityStatus {
        self.status
    }

    #[must_use]
    pub const fn authorization_epoch(&self) -> u64 {
        self.authorization_epoch
    }

    #[must_use]
    pub fn identity_id(&self) -> &DeveloperIdentityId {
        &self.identity_id
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    #[must_use]
    pub fn review(&self) -> Option<&DeveloperDecisionSummary> {
        self.review.as_ref()
    }

    pub fn transition(
        &mut self,
        next: DeveloperIdentityStatus,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if !self.status.can_transition_to(next) || now_unix_seconds < self.updated_at_unix_seconds {
            return Err(DeveloperRegistrationError::InvalidLifecycleTransition {
                from: self.status,
                to: next,
            });
        }
        if self.status == next {
            return Ok(());
        }
        self.status = next;
        self.authorization_epoch = self
            .authorization_epoch
            .checked_add(1)
            .ok_or(DeveloperRegistrationError::CorruptRecord)?;
        self.updated_at_unix_seconds = now_unix_seconds;
        self.validate()
    }

    pub fn record_decision(
        &mut self,
        decision: &DeveloperDecisionRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if decision.identity_id != self.identity_id {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        let next = match decision.kind {
            DeveloperDecisionKind::Approve => DeveloperIdentityStatus::Active,
            DeveloperDecisionKind::Reject => DeveloperIdentityStatus::Rejected,
        };
        self.transition(next, decision.decided_at_unix_seconds)?;
        self.review = Some(DeveloperDecisionSummary::from(decision));
        self.validate()
    }

    pub(crate) fn repair_combined_bootstrap_to_waitlisted(
        &mut self,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if self.status != DeveloperIdentityStatus::Active
            || self.review.is_some()
            || now_unix_seconds < self.updated_at_unix_seconds
        {
            return Err(DeveloperRegistrationError::InvalidLifecycleTransition {
                from: self.status,
                to: DeveloperIdentityStatus::Waitlisted,
            });
        }
        self.status = DeveloperIdentityStatus::Waitlisted;
        self.authorization_epoch = self
            .authorization_epoch
            .checked_add(1)
            .ok_or(DeveloperRegistrationError::CorruptRecord)?;
        self.updated_at_unix_seconds = now_unix_seconds;
        self.validate()
    }
}

impl fmt::Debug for DeveloperRoleRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperRoleRecord")
            .field("identity_id", &self.identity_id)
            .field("status", &self.status)
            .field("authorization_epoch", &self.authorization_epoch)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperAccount {
    identity: AuthenticationIdentityRecord,
    role: DeveloperRoleRecord,
}

impl DeveloperAccount {
    pub fn new_unverified(
        id: DeveloperIdentityId,
        normalized_email: NormalizedEmail,
        display_name: impl Into<String>,
        password_hash: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        Self::from_parts(
            AuthenticationIdentityRecord::new_unverified(
                id.clone(),
                normalized_email,
                display_name,
                password_hash,
                now_unix_seconds,
            )?,
            DeveloperRoleRecord::new_unverified(id, now_unix_seconds)?,
        )
    }

    fn migrated_active(
        id: DeveloperIdentityId,
        normalized_email: NormalizedEmail,
        display_name: impl Into<String>,
        created_at_unix_seconds: u64,
        last_authenticated_at_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        Self::from_parts(
            AuthenticationIdentityRecord::migrated(
                id.clone(),
                normalized_email,
                display_name,
                None,
                Some(created_at_unix_seconds),
                AuthenticationSecurityStatus::Active,
                1,
                created_at_unix_seconds,
                last_authenticated_at_unix_seconds,
                Some(last_authenticated_at_unix_seconds),
            )?,
            DeveloperRoleRecord::migrated(
                id,
                DeveloperIdentityStatus::Active,
                1,
                created_at_unix_seconds,
                last_authenticated_at_unix_seconds,
                None,
            )?,
        )
    }

    pub fn from_parts(
        identity: AuthenticationIdentityRecord,
        role: DeveloperRoleRecord,
    ) -> Result<Self, DeveloperRegistrationError> {
        let account = Self { identity, role };
        account.validate()?;
        Ok(account)
    }

    pub fn validate(&self) -> Result<(), DeveloperRegistrationError> {
        self.identity.validate()?;
        self.role.validate()?;
        if self.identity.id != self.role.identity_id
            || self.role.created_at_unix_seconds < self.identity.created_at_unix_seconds
            || matches!(
                self.role.status,
                DeveloperIdentityStatus::Waitlisted
                    | DeveloperIdentityStatus::Active
                    | DeveloperIdentityStatus::Rejected
                    | DeveloperIdentityStatus::Disabled
            ) && self.identity.email_verified_at_unix_seconds.is_none()
            || self.role.status == DeveloperIdentityStatus::Deleted
                && self.identity.security_status != AuthenticationSecurityStatus::Deleted
        {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(())
    }

    #[must_use]
    pub fn identity(&self) -> &AuthenticationIdentityRecord {
        &self.identity
    }

    #[must_use]
    pub fn role(&self) -> &DeveloperRoleRecord {
        &self.role
    }

    #[must_use]
    pub fn id(&self) -> &DeveloperIdentityId {
        self.identity.id()
    }

    #[must_use]
    pub fn normalized_email(&self) -> &NormalizedEmail {
        self.identity.normalized_email()
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        self.identity.display_name()
    }

    #[must_use]
    pub fn password_hash(&self) -> Option<&str> {
        self.identity.password_hash()
    }

    #[must_use]
    pub const fn status(&self) -> DeveloperIdentityStatus {
        self.role.status()
    }

    #[must_use]
    pub const fn authorization_epoch(&self) -> u64 {
        self.role.authorization_epoch()
    }

    #[must_use]
    pub const fn credential_epoch(&self) -> u64 {
        self.identity.credential_epoch()
    }

    #[must_use]
    pub const fn authentication_security_status(&self) -> AuthenticationSecurityStatus {
        self.identity.security_status()
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.role.created_at_unix_seconds()
    }

    #[must_use]
    pub fn updated_at_unix_seconds(&self) -> u64 {
        self.identity
            .updated_at_unix_seconds()
            .max(self.role.updated_at_unix_seconds())
    }

    #[must_use]
    pub const fn email_verified_at_unix_seconds(&self) -> Option<u64> {
        self.identity.email_verified_at_unix_seconds()
    }

    #[must_use]
    pub fn review(&self) -> Option<&DeveloperDecisionSummary> {
        self.role.review()
    }

    pub fn transition(
        &mut self,
        next: DeveloperIdentityStatus,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if next == DeveloperIdentityStatus::Waitlisted
            && self.identity.email_verified_at_unix_seconds.is_none()
        {
            self.identity.mark_email_verified(now_unix_seconds)?;
        }
        if next == DeveloperIdentityStatus::Deleted {
            self.identity
                .transition_security(AuthenticationSecurityStatus::Deleted, now_unix_seconds)?;
        }
        self.role.transition(next, now_unix_seconds)?;
        self.validate()
    }

    pub fn record_decision(
        &mut self,
        decision: &DeveloperDecisionRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        self.role.record_decision(decision)?;
        self.validate()
    }

    pub fn replace_password_hash(
        &mut self,
        encoded: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        self.identity
            .replace_password_hash(encoded, now_unix_seconds)?;
        self.validate()
    }

    pub fn upgrade_password_hash(
        &mut self,
        encoded: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        self.identity
            .upgrade_password_hash(encoded, now_unix_seconds)?;
        self.validate()
    }

    pub fn record_authentication(
        &mut self,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        self.identity.record_authentication(now_unix_seconds)?;
        self.validate()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperAuthTokenPurpose {
    EmailVerification,
    PasswordRecovery,
}

impl DeveloperAuthTokenPurpose {
    const fn domain(self) -> &'static str {
        match self {
            Self::EmailVerification => "email-verification",
            Self::PasswordRecovery => "password-recovery",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct DeveloperAuthToken(String);

impl DeveloperAuthToken {
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; SECRET_BYTES];
        OsRng.fill_bytes(&mut bytes);
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, DeveloperRegistrationError> {
        let value = value.into();
        let decoded = URL_SAFE_NO_PAD
            .decode(&value)
            .map_err(|_| DeveloperRegistrationError::InvalidToken)?;
        if decoded.len() != SECRET_BYTES || value.chars().any(char::is_whitespace) {
            return Err(DeveloperRegistrationError::InvalidToken);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose_once(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn digest(
        &self,
        lookup: &DeveloperLookupKey,
        purpose: DeveloperAuthTokenPurpose,
    ) -> String {
        lookup.secret_digest(purpose.domain(), &self.0)
    }
}

impl fmt::Debug for DeveloperAuthToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeveloperAuthToken([REDACTED])")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperAuthTokenRecord {
    id: DeveloperAuthTokenId,
    identity_id: DeveloperIdentityId,
    purpose: DeveloperAuthTokenPurpose,
    digest: String,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    consumed_at_unix_seconds: Option<u64>,
}

impl DeveloperAuthTokenRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: DeveloperAuthTokenId,
        identity_id: DeveloperIdentityId,
        purpose: DeveloperAuthTokenPurpose,
        digest: impl Into<String>,
        created_at_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let record = Self {
            id,
            identity_id,
            purpose,
            digest: digest.into(),
            created_at_unix_seconds,
            expires_at_unix_seconds,
            consumed_at_unix_seconds: None,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), DeveloperRegistrationError> {
        validate_digest(&self.digest)?;
        if self.expires_at_unix_seconds <= self.created_at_unix_seconds
            || self.consumed_at_unix_seconds.is_some_and(|value| {
                value < self.created_at_unix_seconds || value > self.expires_at_unix_seconds
            })
        {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &DeveloperAuthTokenId {
        &self.id
    }

    #[must_use]
    pub fn identity_id(&self) -> &DeveloperIdentityId {
        &self.identity_id
    }

    #[must_use]
    pub const fn purpose(&self) -> DeveloperAuthTokenPurpose {
        self.purpose
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    #[must_use]
    pub const fn consumed_at_unix_seconds(&self) -> Option<u64> {
        self.consumed_at_unix_seconds
    }

    pub fn consume(&mut self, now_unix_seconds: u64) -> Result<(), DeveloperRegistrationError> {
        if self.consumed_at_unix_seconds.is_some()
            || now_unix_seconds >= self.expires_at_unix_seconds
        {
            return Err(DeveloperRegistrationError::InvalidToken);
        }
        self.consumed_at_unix_seconds = Some(now_unix_seconds);
        self.validate()
    }

    pub fn invalidate(&mut self, now_unix_seconds: u64) -> Result<(), DeveloperRegistrationError> {
        if self.consumed_at_unix_seconds.is_some()
            || now_unix_seconds < self.created_at_unix_seconds
        {
            return Err(DeveloperRegistrationError::InvalidToken);
        }
        self.consumed_at_unix_seconds = Some(now_unix_seconds.min(self.expires_at_unix_seconds));
        self.validate()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperAccessAudience {
    Active,
    Waitlist,
}

impl DeveloperAccessAudience {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "mako-management",
            Self::Waitlist => "mako-developer-waitlist",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct DeveloperRefreshCredential {
    session_id: DeveloperRefreshSessionId,
    secret: String,
    encoded: String,
}

impl DeveloperRefreshCredential {
    #[must_use]
    pub fn generate(session_id: DeveloperRefreshSessionId) -> Self {
        let mut bytes = [0_u8; SECRET_BYTES];
        OsRng.fill_bytes(&mut bytes);
        let secret = URL_SAFE_NO_PAD.encode(bytes);
        let encoded = format!("{}.{}", session_id.as_str(), secret);
        Self {
            session_id,
            secret,
            encoded,
        }
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, DeveloperRegistrationError> {
        let value = value.into();
        let (session_id, secret) = value
            .split_once('.')
            .ok_or(DeveloperRegistrationError::InvalidSession)?;
        let session_id = DeveloperRefreshSessionId::parse(session_id)?;
        DeveloperAuthToken::parse(secret.to_owned())?;
        Ok(Self {
            session_id,
            secret: secret.to_owned(),
            encoded: value,
        })
    }

    #[must_use]
    pub fn expose_once(&self) -> &str {
        &self.encoded
    }

    #[must_use]
    pub fn encoded(&self) -> &str {
        &self.encoded
    }

    #[must_use]
    pub fn session_id(&self) -> &DeveloperRefreshSessionId {
        &self.session_id
    }

    #[must_use]
    pub fn digest(&self, lookup: &DeveloperLookupKey) -> String {
        lookup.secret_digest("refresh-session", &self.secret)
    }
}

impl fmt::Debug for DeveloperRefreshCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeveloperRefreshCredential([REDACTED])")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperRefreshSessionRecord {
    schema_version: u32,
    id: DeveloperRefreshSessionId,
    identity_id: DeveloperIdentityId,
    credential_digest: String,
    audience: DeveloperAccessAudience,
    credential_epoch: u64,
    developer_authorization_epoch: u64,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    revoked_at_unix_seconds: Option<u64>,
    rotated_to: Option<DeveloperRefreshSessionId>,
}

impl DeveloperRefreshSessionRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: DeveloperRefreshSessionId,
        identity_id: DeveloperIdentityId,
        credential_digest: impl Into<String>,
        audience: DeveloperAccessAudience,
        credential_epoch: u64,
        developer_authorization_epoch: u64,
        created_at_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let record = Self {
            schema_version: DEVELOPER_REFRESH_SESSION_SCHEMA_VERSION,
            id,
            identity_id,
            credential_digest: credential_digest.into(),
            audience,
            credential_epoch,
            developer_authorization_epoch,
            created_at_unix_seconds,
            expires_at_unix_seconds,
            revoked_at_unix_seconds: None,
            rotated_to: None,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), DeveloperRegistrationError> {
        validate_digest(&self.credential_digest)?;
        if self.schema_version != DEVELOPER_REFRESH_SESSION_SCHEMA_VERSION
            || self.credential_epoch == 0
            || self.developer_authorization_epoch == 0
            || self.expires_at_unix_seconds <= self.created_at_unix_seconds
            || self.revoked_at_unix_seconds.is_some_and(|value| {
                value < self.created_at_unix_seconds || value > self.expires_at_unix_seconds
            })
            || (self.rotated_to.is_some() && self.revoked_at_unix_seconds.is_none())
        {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &DeveloperRefreshSessionId {
        &self.id
    }

    #[must_use]
    pub fn identity_id(&self) -> &DeveloperIdentityId {
        &self.identity_id
    }

    #[must_use]
    pub fn credential_digest(&self) -> &str {
        &self.credential_digest
    }

    #[must_use]
    pub const fn audience(&self) -> DeveloperAccessAudience {
        self.audience
    }

    #[must_use]
    pub const fn credential_epoch(&self) -> u64 {
        self.credential_epoch
    }

    #[must_use]
    pub const fn developer_authorization_epoch(&self) -> u64 {
        self.developer_authorization_epoch
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    #[must_use]
    pub const fn revoked_at_unix_seconds(&self) -> Option<u64> {
        self.revoked_at_unix_seconds
    }

    pub fn rotate_to(
        &mut self,
        next: DeveloperRefreshSessionId,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if self.revoked_at_unix_seconds.is_some()
            || now_unix_seconds >= self.expires_at_unix_seconds
        {
            return Err(DeveloperRegistrationError::InvalidSession);
        }
        self.revoked_at_unix_seconds = Some(now_unix_seconds);
        self.rotated_to = Some(next);
        self.validate()
    }

    pub fn revoke(&mut self, now_unix_seconds: u64) -> Result<(), DeveloperRegistrationError> {
        if now_unix_seconds < self.created_at_unix_seconds {
            return Err(DeveloperRegistrationError::InvalidSession);
        }
        self.revoked_at_unix_seconds.get_or_insert(now_unix_seconds);
        self.validate()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyDeveloperRefreshSessionRecordV1 {
    id: DeveloperRefreshSessionId,
    identity_id: DeveloperIdentityId,
    credential_digest: String,
    audience: DeveloperAccessAudience,
    authorization_epoch: u64,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    revoked_at_unix_seconds: Option<u64>,
    rotated_to: Option<DeveloperRefreshSessionId>,
}

impl LegacyDeveloperRefreshSessionRecordV1 {
    fn validate(&self) -> Result<(), DeveloperRegistrationError> {
        validate_digest(&self.credential_digest)?;
        if self.authorization_epoch == 0
            || self.expires_at_unix_seconds <= self.created_at_unix_seconds
            || self.revoked_at_unix_seconds.is_some_and(|value| {
                value < self.created_at_unix_seconds || value > self.expires_at_unix_seconds
            })
            || (self.rotated_to.is_some() && self.revoked_at_unix_seconds.is_none())
            || self.id.as_str().is_empty()
            || self.identity_id.as_str().is_empty()
            || !matches!(
                self.audience,
                DeveloperAccessAudience::Active | DeveloperAccessAudience::Waitlist
            )
        {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperDecisionKind {
    Approve,
    Reject,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperDecisionRecord {
    identity_id: DeveloperIdentityId,
    kind: DeveloperDecisionKind,
    operator_id: String,
    #[serde(default)]
    self_review: bool,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason_provided: Option<bool>,
    request_id: String,
    idempotency_digest: String,
    request_digest: String,
    prior_status: DeveloperIdentityStatus,
    resulting_status: DeveloperIdentityStatus,
    resulting_authorization_epoch: u64,
    decided_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
}

impl DeveloperDecisionRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity_id: DeveloperIdentityId,
        kind: DeveloperDecisionKind,
        operator_id: impl Into<String>,
        reason: impl Into<String>,
        request_id: impl Into<String>,
        idempotency_digest: impl Into<String>,
        request_digest: impl Into<String>,
        resulting_authorization_epoch: u64,
        decided_at_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let reason = reason.into();
        let reason = reason.trim().to_owned();
        let resulting_status = match kind {
            DeveloperDecisionKind::Approve => DeveloperIdentityStatus::Active,
            DeveloperDecisionKind::Reject => DeveloperIdentityStatus::Rejected,
        };
        let record = Self {
            identity_id,
            kind,
            operator_id: operator_id.into(),
            self_review: false,
            reason_provided: Some(!reason.is_empty()),
            reason,
            request_id: request_id.into(),
            idempotency_digest: idempotency_digest.into(),
            request_digest: request_digest.into(),
            prior_status: DeveloperIdentityStatus::Waitlisted,
            resulting_status,
            resulting_authorization_epoch,
            decided_at_unix_seconds,
            expires_at_unix_seconds,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), DeveloperRegistrationError> {
        validate_text("review operator id", &self.operator_id, 3, 256)?;
        if !self.reason.is_empty() {
            validate_text("review reason", &self.reason, 8, 1_024)?;
        }
        if self
            .reason_provided
            .is_some_and(|value| value != !self.reason.is_empty())
        {
            return Err(DeveloperRegistrationError::InvalidField(
                "review reason presence",
            ));
        }
        validate_text("review request id", &self.request_id, 8, 256)?;
        validate_digest(&self.idempotency_digest)?;
        validate_digest(&self.request_digest)?;
        if self.resulting_authorization_epoch < 2
            || self.expires_at_unix_seconds <= self.decided_at_unix_seconds
        {
            return Err(DeveloperRegistrationError::InvalidField(
                "review idempotency expiry",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn identity_id(&self) -> &DeveloperIdentityId {
        &self.identity_id
    }

    #[must_use]
    pub const fn kind(&self) -> DeveloperDecisionKind {
        self.kind
    }

    #[must_use]
    pub fn idempotency_digest(&self) -> &str {
        &self.idempotency_digest
    }

    #[must_use]
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }

    #[must_use]
    pub fn operator_id(&self) -> &str {
        &self.operator_id
    }

    #[must_use]
    pub const fn self_review(&self) -> bool {
        self.self_review
    }

    pub(crate) fn mark_self_review(&mut self) {
        self.self_review = true;
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    #[must_use]
    pub fn reason_provided(&self) -> bool {
        self.reason_provided.unwrap_or(!self.reason.is_empty())
    }

    #[must_use]
    pub const fn resulting_status(&self) -> DeveloperIdentityStatus {
        self.resulting_status
    }

    #[must_use]
    pub const fn resulting_authorization_epoch(&self) -> u64 {
        self.resulting_authorization_epoch
    }

    #[must_use]
    pub const fn decided_at_unix_seconds(&self) -> u64 {
        self.decided_at_unix_seconds
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }
}

impl fmt::Debug for DeveloperDecisionRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperDecisionRecord")
            .field("identity_id", &self.identity_id)
            .field("kind", &self.kind)
            .field("operator_id", &self.operator_id)
            .field("self_review", &self.self_review)
            .field("reason_provided", &self.reason_provided())
            .field("request_id", &self.request_id)
            .field("idempotency_digest", &self.idempotency_digest)
            .field("request_digest", &self.request_digest)
            .field("prior_status", &self.prior_status)
            .field("resulting_status", &self.resulting_status)
            .field(
                "resulting_authorization_epoch",
                &self.resulting_authorization_epoch,
            )
            .field("decided_at_unix_seconds", &self.decided_at_unix_seconds)
            .field("expires_at_unix_seconds", &self.expires_at_unix_seconds)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperDecisionSummary {
    identity_id: DeveloperIdentityId,
    kind: DeveloperDecisionKind,
    operator_id: String,
    decided_at_unix_seconds: u64,
}

impl From<&DeveloperDecisionRecord> for DeveloperDecisionSummary {
    fn from(value: &DeveloperDecisionRecord) -> Self {
        Self {
            identity_id: value.identity_id.clone(),
            kind: value.kind,
            operator_id: value.operator_id.clone(),
            decided_at_unix_seconds: value.decided_at_unix_seconds,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperRateLimitRecord {
    digest: String,
    window_started_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    count: u32,
}

impl DeveloperRateLimitRecord {
    pub fn new(
        digest: impl Into<String>,
        now_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<Self, DeveloperRegistrationError> {
        let record = Self {
            digest: digest.into(),
            window_started_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
            count: 1,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), DeveloperRegistrationError> {
        validate_digest(&self.digest)?;
        if self.count == 0 || self.expires_at_unix_seconds <= self.window_started_at_unix_seconds {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(())
    }

    pub fn increment(&mut self) -> Result<(), DeveloperRegistrationError> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or(DeveloperRegistrationError::RateLimited)?;
        Ok(())
    }

    pub fn increment_or_reset(
        &mut self,
        now_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if now_unix_seconds >= self.expires_at_unix_seconds {
            if expires_at_unix_seconds <= now_unix_seconds {
                return Err(DeveloperRegistrationError::InvalidField(
                    "rate limit expiry",
                ));
            }
            self.window_started_at_unix_seconds = now_unix_seconds;
            self.expires_at_unix_seconds = expires_at_unix_seconds;
            self.count = 1;
            return Ok(());
        }
        self.increment()
    }

    #[must_use]
    pub const fn count(&self) -> u32 {
        self.count
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperMailKind {
    VerifyEmail,
    PasswordRecovery,
    Approved,
    Rejected,
}

#[derive(Clone, Eq, PartialEq)]
pub struct EncryptedDeveloperMail {
    nonce: String,
    ciphertext: String,
}

impl EncryptedDeveloperMail {
    pub fn new(
        nonce: impl Into<String>,
        ciphertext: impl Into<String>,
    ) -> Result<Self, DeveloperRegistrationError> {
        let value = Self {
            nonce: nonce.into(),
            ciphertext: ciphertext.into(),
        };
        let nonce = URL_SAFE_NO_PAD
            .decode(&value.nonce)
            .map_err(|_| DeveloperRegistrationError::InvalidField("mail nonce"))?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(&value.ciphertext)
            .map_err(|_| DeveloperRegistrationError::InvalidField("mail ciphertext"))?;
        if nonce.len() != 24
            || ciphertext.is_empty()
            || ciphertext.len() > MAXIMUM_ENCRYPTED_MAIL_BYTES
        {
            return Err(DeveloperRegistrationError::InvalidField(
                "encrypted developer mail",
            ));
        }
        Ok(value)
    }

    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    #[must_use]
    pub fn ciphertext(&self) -> &str {
        &self.ciphertext
    }
}

impl fmt::Debug for EncryptedDeveloperMail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EncryptedDeveloperMail([REDACTED])")
    }
}

impl Serialize for EncryptedDeveloperMail {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire<'a> {
            nonce: &'a str,
            ciphertext: &'a str,
        }
        Wire {
            nonce: &self.nonce,
            ciphertext: &self.ciphertext,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EncryptedDeveloperMail {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        struct Wire {
            nonce: String,
            ciphertext: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.nonce, wire.ciphertext).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperMailOutboxState {
    Pending,
    Leased,
    Delivered,
    DeadLetter,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeveloperMailOutboxRecord {
    id: DeveloperMailOutboxId,
    identity_id: DeveloperIdentityId,
    kind: DeveloperMailKind,
    encrypted_mail: EncryptedDeveloperMail,
    state: DeveloperMailOutboxState,
    attempts: u32,
    created_at_unix_seconds: u64,
    next_attempt_at_unix_seconds: u64,
    lease_expires_at_unix_seconds: Option<u64>,
    delivered_at_unix_seconds: Option<u64>,
    last_error_code: Option<String>,
}

impl DeveloperMailOutboxRecord {
    pub fn new(
        id: DeveloperMailOutboxId,
        identity_id: DeveloperIdentityId,
        kind: DeveloperMailKind,
        encrypted_mail: EncryptedDeveloperMail,
        now_unix_seconds: u64,
    ) -> Self {
        Self {
            id,
            identity_id,
            kind,
            encrypted_mail,
            state: DeveloperMailOutboxState::Pending,
            attempts: 0,
            created_at_unix_seconds: now_unix_seconds,
            next_attempt_at_unix_seconds: now_unix_seconds,
            lease_expires_at_unix_seconds: None,
            delivered_at_unix_seconds: None,
            last_error_code: None,
        }
    }

    #[must_use]
    pub fn id(&self) -> &DeveloperMailOutboxId {
        &self.id
    }

    #[must_use]
    pub fn identity_id(&self) -> &DeveloperIdentityId {
        &self.identity_id
    }

    #[must_use]
    pub const fn kind(&self) -> DeveloperMailKind {
        self.kind
    }

    #[must_use]
    pub fn encrypted_mail(&self) -> &EncryptedDeveloperMail {
        &self.encrypted_mail
    }

    #[must_use]
    pub const fn state(&self) -> DeveloperMailOutboxState {
        self.state
    }

    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    #[must_use]
    pub const fn next_attempt_at_unix_seconds(&self) -> u64 {
        self.next_attempt_at_unix_seconds
    }

    #[must_use]
    pub const fn lease_expires_at_unix_seconds(&self) -> Option<u64> {
        self.lease_expires_at_unix_seconds
    }

    pub fn lease(
        &mut self,
        now_unix_seconds: u64,
        lease_seconds: u64,
    ) -> Result<(), DeveloperRegistrationError> {
        if !matches!(
            self.state,
            DeveloperMailOutboxState::Pending | DeveloperMailOutboxState::Leased
        ) || self.next_attempt_at_unix_seconds > now_unix_seconds
            || self
                .lease_expires_at_unix_seconds
                .is_some_and(|expiry| expiry > now_unix_seconds)
            || lease_seconds == 0
        {
            return Err(DeveloperRegistrationError::Conflict);
        }
        self.state = DeveloperMailOutboxState::Leased;
        self.attempts = self
            .attempts
            .checked_add(1)
            .ok_or(DeveloperRegistrationError::CorruptRecord)?;
        self.lease_expires_at_unix_seconds = Some(
            now_unix_seconds
                .checked_add(lease_seconds)
                .ok_or(DeveloperRegistrationError::CorruptRecord)?,
        );
        Ok(())
    }

    pub fn delivered(&mut self, now_unix_seconds: u64) -> Result<(), DeveloperRegistrationError> {
        if self.state != DeveloperMailOutboxState::Leased {
            return Err(DeveloperRegistrationError::Conflict);
        }
        self.state = DeveloperMailOutboxState::Delivered;
        self.delivered_at_unix_seconds = Some(now_unix_seconds);
        self.lease_expires_at_unix_seconds = None;
        self.last_error_code = None;
        Ok(())
    }

    pub fn retry(
        &mut self,
        now_unix_seconds: u64,
        retry_at_unix_seconds: u64,
        error_code: impl Into<String>,
        maximum_attempts: u32,
    ) -> Result<(), DeveloperRegistrationError> {
        let error_code = error_code.into();
        validate_text("mail error code", &error_code, 1, 64)?;
        if self.state != DeveloperMailOutboxState::Leased
            || retry_at_unix_seconds <= now_unix_seconds
            || maximum_attempts == 0
        {
            return Err(DeveloperRegistrationError::Conflict);
        }
        self.state = if self.attempts >= maximum_attempts {
            DeveloperMailOutboxState::DeadLetter
        } else {
            DeveloperMailOutboxState::Pending
        };
        self.next_attempt_at_unix_seconds = retry_at_unix_seconds;
        self.lease_expires_at_unix_seconds = None;
        self.last_error_code = Some(error_code);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperReviewPage {
    pub accounts: Vec<DeveloperAccount>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperMigrationReport {
    pub version: u32,
    pub scanned: usize,
    pub migrated: usize,
    pub already_current: usize,
    pub completed: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeveloperCleanupReport {
    pub auth_tokens: usize,
    pub refresh_sessions: usize,
    pub decisions: usize,
    pub rate_limits: usize,
    pub mail_outbox: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeveloperRegistrationHealthSnapshot {
    pub unverified_identities: usize,
    pub waitlisted_identities: usize,
    pub active_identities: usize,
    pub rejected_identities: usize,
    pub disabled_identities: usize,
    pub deleted_identities: usize,
    pub oldest_waitlisted_age_seconds: u64,
    pub pending_outbox: usize,
    pub dead_letter_outbox: usize,
    pub oldest_pending_outbox_age_seconds: u64,
    pub refresh_sessions: usize,
    pub auth_tokens: usize,
    pub decisions: usize,
}

#[derive(Clone)]
pub struct DeveloperRegistrationStore {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    lookup_key: DeveloperLookupKey,
}

impl fmt::Debug for DeveloperRegistrationStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperRegistrationStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl DeveloperRegistrationStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        lookup_key: DeveloperLookupKey,
    ) -> Result<Self, DeveloperRegistrationError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(DeveloperRegistrationError::UnsupportedDurability(
                durability,
            ));
        }
        Ok(Self {
            adapter,
            durability,
            lookup_key,
        })
    }

    #[must_use]
    pub fn lookup_key(&self) -> &DeveloperLookupKey {
        &self.lookup_key
    }

    pub async fn create_unverified(
        &self,
        account: &DeveloperAccount,
        token: &DeveloperAuthTokenRecord,
        outbox: &DeveloperMailOutboxRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if account.status() != DeveloperIdentityStatus::Unverified
            || token.identity_id != *account.id()
            || token.purpose != DeveloperAuthTokenPurpose::EmailVerification
            || outbox.identity_id != *account.id()
            || outbox.kind != DeveloperMailKind::VerifyEmail
        {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        account.validate()?;
        token.validate()?;
        let legacy_account_key = ControlKeyspace::developer_key(account.id())?;
        let identity_key = ControlKeyspace::authentication_identity_key(account.id())?;
        let role_key = ControlKeyspace::developer_role_key(account.id())?;
        let email_key = ControlKeyspace::developer_email_key(
            &self.lookup_key.email_digest(account.normalized_email()),
        )?;
        let status_key = ControlKeyspace::developer_status_key(
            account.status(),
            account.created_at_unix_seconds(),
            account.id(),
        )?;
        let token_key = ControlKeyspace::developer_token_key(&token.digest)?;
        let identity_token_key =
            ControlKeyspace::developer_identity_token_key(account.id(), token.purpose)?;
        let outbox_key = ControlKeyspace::developer_mail_outbox_key(outbox.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(7);
        batch.put(&identity_key, serde_json::to_vec(account.identity())?);
        batch.put(&role_key, serde_json::to_vec(account.role())?);
        batch.put(&email_key, serde_json::to_vec(account.id())?);
        batch.put(&status_key, serde_json::to_vec(account.id())?);
        batch.put(&token_key, serde_json::to_vec(token)?);
        batch.put(&identity_token_key, serde_json::to_vec(&token.digest)?);
        batch.put(&outbox_key, serde_json::to_vec(outbox)?);
        self.apply(AtomicWrite {
            conditions: vec![
                legacy_account_key,
                identity_key,
                role_key,
                email_key,
                status_key,
                token_key,
                identity_token_key,
                outbox_key,
            ]
            .into_iter()
            .map(|key| KeyCondition::Missing { key })
            .collect(),
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn get_account(
        &self,
        identity_id: &DeveloperIdentityId,
    ) -> Result<Option<DeveloperAccount>, DeveloperRegistrationError> {
        let Some(identity) = self.get_authentication_identity(identity_id).await? else {
            return Ok(None);
        };
        let Some(role) = self.get_developer_role(identity_id).await? else {
            return Ok(None);
        };
        DeveloperAccount::from_parts(identity, role).map(Some)
    }

    pub async fn get_authentication_identity(
        &self,
        identity_id: &DeveloperIdentityId,
    ) -> Result<Option<AuthenticationIdentityRecord>, DeveloperRegistrationError> {
        if let Some(identity) = self
            .read(&ControlKeyspace::authentication_identity_key(identity_id)?)
            .await?
        {
            return Ok(Some(identity));
        }
        self.read_legacy_account(identity_id)
            .await
            .map(|account| account.map(|account| account.identity))
    }

    pub async fn get_developer_role(
        &self,
        identity_id: &DeveloperIdentityId,
    ) -> Result<Option<DeveloperRoleRecord>, DeveloperRegistrationError> {
        if let Some(role) = self
            .read(&ControlKeyspace::developer_role_key(identity_id)?)
            .await?
        {
            return Ok(Some(role));
        }
        self.read_legacy_account(identity_id)
            .await
            .map(|account| account.map(|account| account.role))
    }

    pub async fn get_authentication_identity_by_email(
        &self,
        email: &NormalizedEmail,
    ) -> Result<Option<AuthenticationIdentityRecord>, DeveloperRegistrationError> {
        let index: Option<DeveloperIdentityId> = self
            .read(&ControlKeyspace::developer_email_key(
                &self.lookup_key.email_digest(email),
            )?)
            .await?;
        let Some(identity_id) = index else {
            return Ok(None);
        };
        let identity = self
            .get_authentication_identity(&identity_id)
            .await?
            .ok_or(DeveloperRegistrationError::CorruptRecord)?;
        if identity.normalized_email() != email {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(Some(identity))
    }

    pub async fn get_account_by_email(
        &self,
        email: &NormalizedEmail,
    ) -> Result<Option<DeveloperAccount>, DeveloperRegistrationError> {
        let index: Option<DeveloperIdentityId> = self
            .read(&ControlKeyspace::developer_email_key(
                &self.lookup_key.email_digest(email),
            )?)
            .await?;
        let Some(identity_id) = index else {
            return Ok(None);
        };
        let account = self.get_account(&identity_id).await?;
        let Some(account) = account else {
            return Ok(None);
        };
        if account.normalized_email() != email {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(Some(account))
    }

    pub async fn create_authentication_identity(
        &self,
        identity: &AuthenticationIdentityRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        identity.validate()?;
        let identity_key = ControlKeyspace::authentication_identity_key(identity.id())?;
        let legacy_key = ControlKeyspace::developer_key(identity.id())?;
        let email_key = ControlKeyspace::developer_email_key(
            &self.lookup_key.email_digest(identity.normalized_email()),
        )?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&identity_key, serde_json::to_vec(identity)?);
        batch.put(&email_key, serde_json::to_vec(identity.id())?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::Missing { key: identity_key },
                KeyCondition::Missing { key: legacy_key },
                KeyCondition::Missing { key: email_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn create_developer_role(
        &self,
        identity: &AuthenticationIdentityRecord,
        role: &DeveloperRoleRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        identity.validate()?;
        role.validate()?;
        if identity.id() != role.identity_id() {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        let identity_key = ControlKeyspace::authentication_identity_key(identity.id())?;
        let role_key = ControlKeyspace::developer_role_key(identity.id())?;
        let status_key = ControlKeyspace::developer_status_key(
            role.status(),
            role.created_at_unix_seconds(),
            identity.id(),
        )?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&role_key, serde_json::to_vec(role)?);
        batch.put(&status_key, serde_json::to_vec(identity.id())?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: identity_key,
                    value: serde_json::to_vec(identity)?,
                },
                KeyCondition::Missing { key: role_key },
                KeyCondition::Missing { key: status_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn replace_authentication_identity(
        &self,
        previous: &AuthenticationIdentityRecord,
        next: &AuthenticationIdentityRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous.id() != next.id() || previous.normalized_email() != next.normalized_email() {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        next.validate()?;
        self.replace_record(
            ControlKeyspace::authentication_identity_key(previous.id())?,
            previous,
            next,
        )
        .await
    }

    pub async fn replace_account(
        &self,
        previous: &DeveloperAccount,
        next: &DeveloperAccount,
    ) -> Result<(), DeveloperRegistrationError> {
        self.replace_account_with(previous, next, Vec::new(), WriteBatch::new())
            .await
    }

    pub async fn consume_token_and_replace_account(
        &self,
        previous_account: &DeveloperAccount,
        next_account: &DeveloperAccount,
        previous_token: &DeveloperAuthTokenRecord,
        consumed_token: &DeveloperAuthTokenRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous_token.identity_id != *previous_account.id()
            || consumed_token.identity_id != *previous_account.id()
            || previous_token.digest != consumed_token.digest
        {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        let token_key = ControlKeyspace::developer_token_key(&previous_token.digest)?;
        let identity_token_key = ControlKeyspace::developer_identity_token_key(
            &previous_token.identity_id,
            previous_token.purpose,
        )?;
        let mut batch = WriteBatch::new();
        batch.put(&token_key, serde_json::to_vec(consumed_token)?);
        batch.delete(&identity_token_key);
        self.replace_account_with(
            previous_account,
            next_account,
            vec![
                KeyCondition::ValueEquals {
                    key: token_key,
                    value: serde_json::to_vec(previous_token)?,
                },
                KeyCondition::ValueEquals {
                    key: identity_token_key,
                    value: serde_json::to_vec(&previous_token.digest)?,
                },
            ],
            batch,
        )
        .await
    }

    pub async fn consume_token_and_replace_authentication_identity(
        &self,
        previous_identity: &AuthenticationIdentityRecord,
        next_identity: &AuthenticationIdentityRecord,
        previous_token: &DeveloperAuthTokenRecord,
        consumed_token: &DeveloperAuthTokenRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous_identity.id() != next_identity.id()
            || previous_identity.normalized_email() != next_identity.normalized_email()
            || previous_token.identity_id != *previous_identity.id()
            || consumed_token.identity_id != *previous_identity.id()
            || previous_token.digest != consumed_token.digest
        {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        next_identity.validate()?;
        let identity_key = ControlKeyspace::authentication_identity_key(previous_identity.id())?;
        let token_key = ControlKeyspace::developer_token_key(&previous_token.digest)?;
        let identity_token_key = ControlKeyspace::developer_identity_token_key(
            &previous_token.identity_id,
            previous_token.purpose,
        )?;
        let mut batch = WriteBatch::new();
        batch.put(&identity_key, serde_json::to_vec(next_identity)?);
        batch.put(&token_key, serde_json::to_vec(consumed_token)?);
        batch.delete(&identity_token_key);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: identity_key,
                    value: serde_json::to_vec(previous_identity)?,
                },
                KeyCondition::ValueEquals {
                    key: token_key,
                    value: serde_json::to_vec(previous_token)?,
                },
                KeyCondition::ValueEquals {
                    key: identity_token_key,
                    value: serde_json::to_vec(&previous_token.digest)?,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn get_current_token(
        &self,
        identity_id: &DeveloperIdentityId,
        purpose: DeveloperAuthTokenPurpose,
    ) -> Result<Option<DeveloperAuthTokenRecord>, DeveloperRegistrationError> {
        let digest: Option<String> = self
            .read(&ControlKeyspace::developer_identity_token_key(
                identity_id,
                purpose,
            )?)
            .await?;
        let Some(digest) = digest else {
            return Ok(None);
        };
        let token = self
            .get_token(&digest)
            .await?
            .ok_or(DeveloperRegistrationError::CorruptRecord)?;
        if token.identity_id != *identity_id || token.purpose != purpose {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        Ok(Some(token))
    }

    pub async fn create_auth_token(
        &self,
        account: &DeveloperAccount,
        token: &DeveloperAuthTokenRecord,
        outbox: &DeveloperMailOutboxRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if token.identity_id != *account.id() || outbox.identity_id != *account.id() {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        let identity_key = ControlKeyspace::authentication_identity_key(account.id())?;
        let role_key = ControlKeyspace::developer_role_key(account.id())?;
        let token_key = ControlKeyspace::developer_token_key(&token.digest)?;
        let index_key = ControlKeyspace::developer_identity_token_key(account.id(), token.purpose)?;
        let outbox_key = ControlKeyspace::developer_mail_outbox_key(outbox.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(3);
        batch.put(&token_key, serde_json::to_vec(token)?);
        batch.put(&index_key, serde_json::to_vec(&token.digest)?);
        batch.put(&outbox_key, serde_json::to_vec(outbox)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: identity_key,
                    value: serde_json::to_vec(account.identity())?,
                },
                KeyCondition::ValueEquals {
                    key: role_key,
                    value: serde_json::to_vec(account.role())?,
                },
                KeyCondition::Missing { key: token_key },
                KeyCondition::Missing { key: index_key },
                KeyCondition::Missing { key: outbox_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn create_auth_token_for_authentication_identity(
        &self,
        identity: &AuthenticationIdentityRecord,
        token: &DeveloperAuthTokenRecord,
        outbox: &DeveloperMailOutboxRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if token.identity_id != *identity.id() || outbox.identity_id != *identity.id() {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        identity.validate()?;
        let identity_key = ControlKeyspace::authentication_identity_key(identity.id())?;
        let token_key = ControlKeyspace::developer_token_key(&token.digest)?;
        let index_key =
            ControlKeyspace::developer_identity_token_key(identity.id(), token.purpose)?;
        let outbox_key = ControlKeyspace::developer_mail_outbox_key(outbox.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(3);
        batch.put(&token_key, serde_json::to_vec(token)?);
        batch.put(&index_key, serde_json::to_vec(&token.digest)?);
        batch.put(&outbox_key, serde_json::to_vec(outbox)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: identity_key,
                    value: serde_json::to_vec(identity)?,
                },
                KeyCondition::Missing { key: token_key },
                KeyCondition::Missing { key: index_key },
                KeyCondition::Missing { key: outbox_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn replace_auth_token(
        &self,
        previous: &DeveloperAuthTokenRecord,
        invalidated: &DeveloperAuthTokenRecord,
        next: &DeveloperAuthTokenRecord,
        outbox: &DeveloperMailOutboxRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous.identity_id != next.identity_id
            || previous.purpose != next.purpose
            || invalidated.digest != previous.digest
            || outbox.identity_id != previous.identity_id
        {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        let previous_key = ControlKeyspace::developer_token_key(&previous.digest)?;
        let next_key = ControlKeyspace::developer_token_key(&next.digest)?;
        let index_key =
            ControlKeyspace::developer_identity_token_key(&previous.identity_id, previous.purpose)?;
        let outbox_key = ControlKeyspace::developer_mail_outbox_key(outbox.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(4);
        batch.put(&previous_key, serde_json::to_vec(invalidated)?);
        batch.put(&next_key, serde_json::to_vec(next)?);
        batch.put(&index_key, serde_json::to_vec(&next.digest)?);
        batch.put(&outbox_key, serde_json::to_vec(outbox)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: previous_key,
                    value: serde_json::to_vec(previous)?,
                },
                KeyCondition::Missing { key: next_key },
                KeyCondition::ValueEquals {
                    key: index_key,
                    value: serde_json::to_vec(&previous.digest)?,
                },
                KeyCondition::Missing { key: outbox_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn decide(
        &self,
        previous_account: &DeveloperAccount,
        next_account: &DeveloperAccount,
        decision: &DeveloperDecisionRecord,
        outbox: &DeveloperMailOutboxRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if decision.identity_id != *previous_account.id()
            || outbox.identity_id != *previous_account.id()
            || next_account.review().is_none_or(|review| {
                review.identity_id != decision.identity_id || review.kind != decision.kind
            })
        {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        let decision_key = ControlKeyspace::developer_decision_key(&decision.idempotency_digest)?;
        let outbox_key = ControlKeyspace::developer_mail_outbox_key(outbox.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&decision_key, serde_json::to_vec(decision)?);
        batch.put(&outbox_key, serde_json::to_vec(outbox)?);
        self.replace_account_with(
            previous_account,
            next_account,
            vec![
                KeyCondition::Missing { key: decision_key },
                KeyCondition::Missing { key: outbox_key },
            ],
            batch,
        )
        .await
    }

    pub async fn get_decision(
        &self,
        idempotency_digest: &str,
    ) -> Result<Option<DeveloperDecisionRecord>, DeveloperRegistrationError> {
        validate_digest(idempotency_digest)?;
        let record: Option<DeveloperDecisionRecord> = self
            .read(&ControlKeyspace::developer_decision_key(
                idempotency_digest,
            )?)
            .await?;
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }

    pub async fn get_token(
        &self,
        digest: &str,
    ) -> Result<Option<DeveloperAuthTokenRecord>, DeveloperRegistrationError> {
        validate_digest(digest)?;
        self.read(&ControlKeyspace::developer_token_key(digest)?)
            .await
    }

    pub async fn create_refresh_session(
        &self,
        session: &DeveloperRefreshSessionRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        let key = ControlKeyspace::developer_session_key(session.id.as_str())?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(session)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn get_refresh_session(
        &self,
        id: &DeveloperRefreshSessionId,
    ) -> Result<Option<DeveloperRefreshSessionRecord>, DeveloperRegistrationError> {
        let Some(value) = self
            .adapter
            .get(&ControlKeyspace::developer_session_key(id.as_str())?)
            .await?
        else {
            return Ok(None);
        };
        if let Ok(record) = serde_json::from_slice::<DeveloperRefreshSessionRecord>(&value) {
            record.validate()?;
            return Ok(Some(record));
        }
        let legacy: LegacyDeveloperRefreshSessionRecordV1 = serde_json::from_slice(&value)?;
        legacy.validate()?;
        Ok(None)
    }

    pub async fn rotate_refresh_session(
        &self,
        previous: &DeveloperRefreshSessionRecord,
        revoked: &DeveloperRefreshSessionRecord,
        next: &DeveloperRefreshSessionRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous.id != revoked.id
            || previous.identity_id != next.identity_id
            || revoked.rotated_to.as_ref() != Some(&next.id)
        {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        let previous_key = ControlKeyspace::developer_session_key(previous.id.as_str())?;
        let next_key = ControlKeyspace::developer_session_key(next.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&previous_key, serde_json::to_vec(revoked)?);
        batch.put(&next_key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: previous_key,
                    value: serde_json::to_vec(previous)?,
                },
                KeyCondition::Missing { key: next_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn replace_refresh_session(
        &self,
        previous: &DeveloperRefreshSessionRecord,
        next: &DeveloperRefreshSessionRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous.id != next.id {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        self.replace_record(
            ControlKeyspace::developer_session_key(previous.id.as_str())?,
            previous,
            next,
        )
        .await
    }

    pub async fn review_page(
        &self,
        status: DeveloperIdentityStatus,
        cursor: Option<&str>,
        limit: NonZeroUsize,
    ) -> Result<DeveloperReviewPage, DeveloperRegistrationError> {
        if limit.get() > MAXIMUM_REVIEW_PAGE {
            return Err(DeveloperRegistrationError::LimitExceeded);
        }
        let mut range = ControlKeyspace::developer_status_range(status)?;
        if let Some(cursor) = cursor {
            let mut start = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| DeveloperRegistrationError::InvalidCursor)?;
            if !range.contains(&start) {
                return Err(DeveloperRegistrationError::InvalidCursor);
            }
            start.push(0);
            if start >= range.end_exclusive {
                return Ok(DeveloperReviewPage {
                    accounts: Vec::new(),
                    next_cursor: None,
                });
            }
            range.start_inclusive = start;
        }
        let requested = limit
            .get()
            .checked_add(1)
            .and_then(NonZeroUsize::new)
            .ok_or(DeveloperRegistrationError::LimitExceeded)?;
        let mut values = self
            .adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, requested))
            .await?;
        let has_more = values.len() > limit.get();
        if has_more {
            values.truncate(limit.get());
        }
        let next_cursor = has_more
            .then(|| {
                values
                    .last()
                    .map(|value| URL_SAFE_NO_PAD.encode(&value.key))
            })
            .flatten();
        let mut accounts = Vec::with_capacity(values.len());
        for value in values {
            let identity_id: DeveloperIdentityId = serde_json::from_slice(&value.value)?;
            let account = self
                .get_account(&identity_id)
                .await?
                .ok_or(DeveloperRegistrationError::CorruptRecord)?;
            if account.status() != status {
                return Err(DeveloperRegistrationError::CorruptRecord);
            }
            accounts.push(account);
        }
        Ok(DeveloperReviewPage {
            accounts,
            next_cursor,
        })
    }

    pub async fn get_outbox(
        &self,
        id: &DeveloperMailOutboxId,
    ) -> Result<Option<DeveloperMailOutboxRecord>, DeveloperRegistrationError> {
        self.read(&ControlKeyspace::developer_mail_outbox_key(id.as_str())?)
            .await
    }

    pub async fn replace_outbox(
        &self,
        previous: &DeveloperMailOutboxRecord,
        next: &DeveloperMailOutboxRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous.id != next.id || previous.identity_id != next.identity_id {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        self.replace_record(
            ControlKeyspace::developer_mail_outbox_key(previous.id.as_str())?,
            previous,
            next,
        )
        .await
    }

    pub async fn pending_outbox(
        &self,
        now_unix_seconds: u64,
        limit: NonZeroUsize,
    ) -> Result<Vec<DeveloperMailOutboxRecord>, DeveloperRegistrationError> {
        if limit.get() > MAXIMUM_REVIEW_PAGE {
            return Err(DeveloperRegistrationError::LimitExceeded);
        }
        let values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_mail_outbox_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        values
            .into_iter()
            .map(|value| serde_json::from_slice::<DeveloperMailOutboxRecord>(&value.value))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|record| {
                matches!(
                    record.state,
                    DeveloperMailOutboxState::Pending | DeveloperMailOutboxState::Leased
                ) && record.next_attempt_at_unix_seconds <= now_unix_seconds
                    && record
                        .lease_expires_at_unix_seconds
                        .is_none_or(|expiry| expiry <= now_unix_seconds)
            })
            .take(limit.get())
            .collect::<Vec<_>>()
            .pipe(Ok)
    }

    pub async fn get_rate_limit(
        &self,
        digest: &str,
    ) -> Result<Option<DeveloperRateLimitRecord>, DeveloperRegistrationError> {
        validate_digest(digest)?;
        self.read(&ControlKeyspace::developer_rate_limit_key(digest)?)
            .await
    }

    pub async fn outbox_depth(&self) -> Result<usize, DeveloperRegistrationError> {
        let values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_mail_outbox_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        // The outbox is globally bounded. If a complete accounting cannot be
        // made within that bound, fail closed instead of under-counting pending
        // work hidden behind retained terminal records.
        if values.len() == MAXIMUM_MIGRATION_RECORDS {
            return Err(DeveloperRegistrationError::LimitExceeded);
        }
        values.into_iter().try_fold(0_usize, |count, value| {
            let record: DeveloperMailOutboxRecord = serde_json::from_slice(&value.value)?;
            Ok::<_, DeveloperRegistrationError>(
                count
                    + usize::from(matches!(
                        record.state,
                        DeveloperMailOutboxState::Pending | DeveloperMailOutboxState::Leased
                    )),
            )
        })
    }

    pub async fn health_snapshot(
        &self,
        now_unix_seconds: u64,
    ) -> Result<DeveloperRegistrationHealthSnapshot, DeveloperRegistrationError> {
        let mut snapshot = DeveloperRegistrationHealthSnapshot::default();
        for status in [
            DeveloperIdentityStatus::Unverified,
            DeveloperIdentityStatus::Waitlisted,
            DeveloperIdentityStatus::Active,
            DeveloperIdentityStatus::Rejected,
            DeveloperIdentityStatus::Disabled,
            DeveloperIdentityStatus::Deleted,
        ] {
            let values = self
                .adapter
                .scan(ScanRequest::new(
                    ControlKeyspace::developer_status_range(status)?,
                    ScanDirection::Forward,
                    NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
                ))
                .await?;
            if values.len() == MAXIMUM_MIGRATION_RECORDS {
                return Err(DeveloperRegistrationError::LimitExceeded);
            }
            match status {
                DeveloperIdentityStatus::Unverified => {
                    snapshot.unverified_identities = values.len()
                }
                DeveloperIdentityStatus::Waitlisted => {
                    snapshot.waitlisted_identities = values.len();
                    if let Some(value) = values.first() {
                        let identity_id: DeveloperIdentityId =
                            serde_json::from_slice(&value.value)?;
                        let account = self
                            .get_account(&identity_id)
                            .await?
                            .ok_or(DeveloperRegistrationError::CorruptRecord)?;
                        snapshot.oldest_waitlisted_age_seconds =
                            now_unix_seconds.saturating_sub(account.created_at_unix_seconds());
                    }
                }
                DeveloperIdentityStatus::Active => snapshot.active_identities = values.len(),
                DeveloperIdentityStatus::Rejected => snapshot.rejected_identities = values.len(),
                DeveloperIdentityStatus::Disabled => snapshot.disabled_identities = values.len(),
                DeveloperIdentityStatus::Deleted => snapshot.deleted_identities = values.len(),
            }
        }

        let outbox = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_mail_outbox_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        if outbox.len() == MAXIMUM_MIGRATION_RECORDS {
            return Err(DeveloperRegistrationError::LimitExceeded);
        }
        let mut oldest_pending = None;
        for value in outbox {
            let record: DeveloperMailOutboxRecord = serde_json::from_slice(&value.value)?;
            match record.state {
                DeveloperMailOutboxState::Pending | DeveloperMailOutboxState::Leased => {
                    snapshot.pending_outbox += 1;
                    oldest_pending = Some(
                        oldest_pending.map_or(record.created_at_unix_seconds, |current: u64| {
                            current.min(record.created_at_unix_seconds)
                        }),
                    );
                }
                DeveloperMailOutboxState::DeadLetter => snapshot.dead_letter_outbox += 1,
                DeveloperMailOutboxState::Delivered => {}
            }
        }
        snapshot.oldest_pending_outbox_age_seconds =
            oldest_pending.map_or(0, |created| now_unix_seconds.saturating_sub(created));
        snapshot.refresh_sessions = self
            .domain_count(ControlKeyspace::developer_sessions_range()?)
            .await?;
        snapshot.auth_tokens = self
            .domain_count(ControlKeyspace::developer_tokens_range()?)
            .await?;
        snapshot.decisions = self
            .domain_count(ControlKeyspace::developer_decisions_range()?)
            .await?;
        Ok(snapshot)
    }

    async fn domain_count(&self, range: KeyRange) -> Result<usize, DeveloperRegistrationError> {
        let values = self
            .adapter
            .scan(ScanRequest::new(
                range,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        if values.len() == MAXIMUM_MIGRATION_RECORDS {
            Err(DeveloperRegistrationError::LimitExceeded)
        } else {
            Ok(values.len())
        }
    }

    pub async fn create_rate_limit(
        &self,
        record: &DeveloperRateLimitRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        let key = ControlKeyspace::developer_rate_limit_key(&record.digest)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(record)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn replace_rate_limit(
        &self,
        previous: &DeveloperRateLimitRecord,
        next: &DeveloperRateLimitRecord,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous.digest != next.digest {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        self.replace_record(
            ControlKeyspace::developer_rate_limit_key(&previous.digest)?,
            previous,
            next,
        )
        .await
    }

    pub async fn migrate_legacy_developers(
        &self,
    ) -> Result<DeveloperMigrationReport, DeveloperRegistrationError> {
        let marker_key = ControlKeyspace::developer_migration_key(DEVELOPER_MIGRATION_VERSION)?;
        if self.adapter.get(&marker_key).await?.is_some() {
            return Ok(DeveloperMigrationReport {
                version: DEVELOPER_MIGRATION_VERSION,
                scanned: 0,
                migrated: 0,
                already_current: 0,
                completed: true,
            });
        }
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developers_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        if entries.len() == MAXIMUM_MIGRATION_RECORDS {
            return Err(DeveloperRegistrationError::LimitExceeded);
        }
        let mut parsed = Vec::with_capacity(entries.len());
        let mut emails = BTreeMap::<String, DeveloperIdentityId>::new();
        for entry in &entries {
            let account = parse_legacy_account(&entry.value)?;
            let digest = self.lookup_key.email_digest(account.normalized_email());
            if let Some(existing) = emails.insert(digest, account.id().clone())
                && existing != *account.id()
            {
                return Err(DeveloperRegistrationError::MigrationCollision);
            }
            parsed.push((entry.key.clone(), entry.value.clone(), account));
        }
        let mut migrated = 0;
        let mut already_current = 0;
        for (legacy_key, legacy_value, account) in &parsed {
            let identity_key = ControlKeyspace::authentication_identity_key(account.id())?;
            let role_key = ControlKeyspace::developer_role_key(account.id())?;
            let email_key = ControlKeyspace::developer_email_key(
                &self.lookup_key.email_digest(account.normalized_email()),
            )?;
            let status_key = ControlKeyspace::developer_status_key(
                account.status(),
                account.created_at_unix_seconds(),
                account.id(),
            )?;
            let existing_identity = self.adapter.get(&identity_key).await?;
            let existing_role = self.adapter.get(&role_key).await?;
            let existing_email = self.adapter.get(&email_key).await?;
            let existing_status = self.adapter.get(&status_key).await?;
            let expected_identity = serde_json::to_vec(account.identity())?;
            let expected_role = serde_json::to_vec(account.role())?;
            let expected_id = serde_json::to_vec(account.id())?;

            if let Some(value) = &existing_identity
                && value != &expected_identity
            {
                return Err(DeveloperRegistrationError::CorruptRecord);
            }
            if let Some(value) = &existing_role
                && value != &expected_role
            {
                return Err(DeveloperRegistrationError::CorruptRecord);
            }
            if let Some(value) = &existing_email {
                let owner: DeveloperIdentityId = serde_json::from_slice(value)?;
                if owner != *account.id() {
                    return Err(DeveloperRegistrationError::MigrationCollision);
                }
            }
            if let Some(value) = &existing_status
                && value != &expected_id
            {
                return Err(DeveloperRegistrationError::CorruptRecord);
            }

            if existing_identity.is_some()
                && existing_role.is_some()
                && existing_email.is_some()
                && existing_status.is_some()
            {
                already_current += 1;
                continue;
            }

            let mut conditions = vec![KeyCondition::ValueEquals {
                key: legacy_key.clone(),
                value: legacy_value.clone(),
            }];
            let mut batch = WriteBatch::with_capacity(4);
            for (key, existing, expected) in [
                (&identity_key, &existing_identity, &expected_identity),
                (&role_key, &existing_role, &expected_role),
                (&email_key, &existing_email, &expected_id),
                (&status_key, &existing_status, &expected_id),
            ] {
                if existing.is_none() {
                    conditions.push(KeyCondition::Missing { key: key.clone() });
                    batch.put(key, expected.clone());
                }
            }
            self.apply(AtomicWrite {
                conditions,
                batch,
                durability: self.durability,
            })
            .await?;
            migrated += 1;
        }
        let marker = serde_json::json!({
            "schemaVersion": 1,
            "migrationVersion": DEVELOPER_MIGRATION_VERSION,
            "completed": true,
        });
        let mut batch = WriteBatch::new();
        batch.put(&marker_key, serde_json::to_vec(&marker)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key: marker_key }],
            batch,
            durability: self.durability,
        })
        .await?;
        Ok(DeveloperMigrationReport {
            version: DEVELOPER_MIGRATION_VERSION,
            scanned: entries.len(),
            migrated,
            already_current,
            completed: true,
        })
    }

    pub async fn cleanup_expired(
        &self,
        now_unix_seconds: u64,
        delivered_mail_retention_seconds: u64,
        limit_per_domain: NonZeroUsize,
    ) -> Result<DeveloperCleanupReport, DeveloperRegistrationError> {
        if limit_per_domain.get() > MAXIMUM_REVIEW_PAGE || delivered_mail_retention_seconds == 0 {
            return Err(DeveloperRegistrationError::LimitExceeded);
        }
        let mut report = DeveloperCleanupReport::default();
        let mut batch = WriteBatch::new();

        let token_values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_tokens_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        require_unsaturated_cleanup_scan(token_values.len())?;
        for value in token_values {
            if report.auth_tokens >= limit_per_domain.get() {
                break;
            }
            let record: DeveloperAuthTokenRecord = serde_json::from_slice(&value.value)?;
            if record.expires_at_unix_seconds <= now_unix_seconds {
                let index_key = ControlKeyspace::developer_identity_token_key(
                    &record.identity_id,
                    record.purpose,
                )?;
                let current_index = self.adapter.get(&index_key).await?;
                let mut token_batch = WriteBatch::new();
                token_batch.delete(&value.key);
                let mut conditions = vec![KeyCondition::ValueEquals {
                    key: value.key,
                    value: value.value,
                }];
                if current_index.as_deref() == Some(serde_json::to_vec(&record.digest)?.as_slice())
                {
                    conditions.push(KeyCondition::ValueEquals {
                        key: index_key.clone(),
                        value: current_index.expect("matched current token index"),
                    });
                    token_batch.delete(index_key);
                }
                match self
                    .adapter
                    .compare_and_write(AtomicWrite {
                        conditions,
                        batch: token_batch,
                        durability: self.durability,
                    })
                    .await?
                {
                    CompareAndWriteResult::Applied => report.auth_tokens += 1,
                    CompareAndWriteResult::Conflict { .. } => {}
                }
            }
        }

        let session_values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_sessions_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        require_unsaturated_cleanup_scan(session_values.len())?;
        for value in session_values {
            if report.refresh_sessions >= limit_per_domain.get() {
                break;
            }
            let expired_or_legacy =
                match serde_json::from_slice::<DeveloperRefreshSessionRecord>(&value.value) {
                    Ok(record) => {
                        record.validate()?;
                        record.expires_at_unix_seconds <= now_unix_seconds
                    }
                    Err(_) => {
                        let legacy: LegacyDeveloperRefreshSessionRecordV1 =
                            serde_json::from_slice(&value.value)?;
                        legacy.validate()?;
                        true
                    }
                };
            if expired_or_legacy {
                batch.delete(value.key);
                report.refresh_sessions += 1;
            }
        }

        let decision_values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_decisions_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        require_unsaturated_cleanup_scan(decision_values.len())?;
        for value in decision_values {
            if report.decisions >= limit_per_domain.get() {
                break;
            }
            let record: DeveloperDecisionRecord = serde_json::from_slice(&value.value)?;
            record.validate()?;
            if record.expires_at_unix_seconds <= now_unix_seconds {
                batch.delete(value.key);
                report.decisions += 1;
            }
        }

        let rate_values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_rate_limits_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        require_unsaturated_cleanup_scan(rate_values.len())?;
        for value in rate_values {
            if report.rate_limits >= limit_per_domain.get() {
                break;
            }
            let record: DeveloperRateLimitRecord = serde_json::from_slice(&value.value)?;
            if record.expires_at_unix_seconds <= now_unix_seconds {
                batch.delete(value.key);
                report.rate_limits += 1;
            }
        }

        let mail_retention_cutoff =
            now_unix_seconds.saturating_sub(delivered_mail_retention_seconds);
        let outbox_values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_mail_outbox_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_MIGRATION_RECORDS).expect("constant is positive"),
            ))
            .await?;
        require_unsaturated_cleanup_scan(outbox_values.len())?;
        for value in outbox_values {
            if report.mail_outbox >= limit_per_domain.get() {
                break;
            }
            let record: DeveloperMailOutboxRecord = serde_json::from_slice(&value.value)?;
            if matches!(
                record.state,
                DeveloperMailOutboxState::Delivered | DeveloperMailOutboxState::DeadLetter
            ) && record.created_at_unix_seconds <= mail_retention_cutoff
            {
                batch.delete(value.key);
                report.mail_outbox += 1;
            }
        }

        if !batch.is_empty() {
            self.adapter.write(batch, self.durability).await?;
        }
        Ok(report)
    }

    async fn replace_account_with(
        &self,
        previous: &DeveloperAccount,
        next: &DeveloperAccount,
        mut conditions: Vec<KeyCondition>,
        mut batch: WriteBatch,
    ) -> Result<(), DeveloperRegistrationError> {
        if previous.id() != next.id() || previous.normalized_email() != next.normalized_email() {
            return Err(DeveloperRegistrationError::ScopeMismatch);
        }
        next.validate()?;
        let identity_key = ControlKeyspace::authentication_identity_key(previous.id())?;
        let role_key = ControlKeyspace::developer_role_key(previous.id())?;
        conditions.insert(
            0,
            KeyCondition::ValueEquals {
                key: identity_key.clone(),
                value: serde_json::to_vec(previous.identity())?,
            },
        );
        conditions.insert(
            1,
            KeyCondition::ValueEquals {
                key: role_key.clone(),
                value: serde_json::to_vec(previous.role())?,
            },
        );
        batch.put(&identity_key, serde_json::to_vec(next.identity())?);
        batch.put(&role_key, serde_json::to_vec(next.role())?);
        if previous.status() != next.status() {
            let previous_status_key = ControlKeyspace::developer_status_key(
                previous.status(),
                previous.created_at_unix_seconds(),
                previous.id(),
            )?;
            let next_status_key = ControlKeyspace::developer_status_key(
                next.status(),
                next.created_at_unix_seconds(),
                next.id(),
            )?;
            conditions.push(KeyCondition::Present {
                key: previous_status_key.clone(),
            });
            conditions.push(KeyCondition::Missing {
                key: next_status_key.clone(),
            });
            batch.delete(previous_status_key);
            batch.put(next_status_key, serde_json::to_vec(next.id())?);
        }
        self.apply(AtomicWrite {
            conditions,
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn replace_record<T: Serialize>(
        &self,
        key: Vec<u8>,
        previous: &T,
        next: &T,
    ) -> Result<(), DeveloperRegistrationError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn read<T: DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<Option<T>, DeveloperRegistrationError> {
        self.adapter
            .get(key)
            .await?
            .map(|value| serde_json::from_slice(&value).map_err(DeveloperRegistrationError::from))
            .transpose()
    }

    async fn read_legacy_account(
        &self,
        identity_id: &DeveloperIdentityId,
    ) -> Result<Option<DeveloperAccount>, DeveloperRegistrationError> {
        self.adapter
            .get(&ControlKeyspace::developer_key(identity_id)?)
            .await?
            .map(|value| parse_legacy_account(&value))
            .transpose()
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), DeveloperRegistrationError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(DeveloperRegistrationError::Conflict),
        }
    }
}

fn require_unsaturated_cleanup_scan(scanned: usize) -> Result<(), DeveloperRegistrationError> {
    if scanned >= MAXIMUM_MIGRATION_RECORDS {
        Err(DeveloperRegistrationError::LimitExceeded)
    } else {
        Ok(())
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}

impl<T> Pipe for T {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyDeveloperAccountV1 {
    schema_version: u32,
    id: DeveloperIdentityId,
    normalized_email: NormalizedEmail,
    display_name: String,
    password_hash: Option<String>,
    status: DeveloperIdentityStatus,
    email_verified_at_unix_seconds: Option<u64>,
    authorization_epoch: u64,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    last_authenticated_at_unix_seconds: Option<u64>,
    review: Option<DeveloperDecisionSummary>,
}

impl LegacyDeveloperAccountV1 {
    fn into_account(self) -> Result<DeveloperAccount, DeveloperRegistrationError> {
        if self.schema_version != 1 {
            return Err(DeveloperRegistrationError::CorruptRecord);
        }
        let security_status = if self.status == DeveloperIdentityStatus::Deleted {
            AuthenticationSecurityStatus::Deleted
        } else {
            AuthenticationSecurityStatus::Active
        };
        DeveloperAccount::from_parts(
            AuthenticationIdentityRecord::migrated(
                self.id.clone(),
                self.normalized_email,
                self.display_name,
                self.password_hash,
                self.email_verified_at_unix_seconds,
                security_status,
                self.authorization_epoch,
                self.created_at_unix_seconds,
                self.updated_at_unix_seconds,
                self.last_authenticated_at_unix_seconds,
            )?,
            DeveloperRoleRecord::migrated(
                self.id,
                self.status,
                self.authorization_epoch,
                self.created_at_unix_seconds,
                self.updated_at_unix_seconds,
                self.review,
            )?,
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyDeveloperIdentity {
    id: DeveloperIdentityId,
    issuer: String,
    subject: String,
    normalized_email: NormalizedEmail,
    display_name: String,
    status: DeveloperIdentityStatus,
    created_at_unix_seconds: u64,
    last_authenticated_at_unix_seconds: u64,
}

fn parse_legacy_account(value: &[u8]) -> Result<DeveloperAccount, DeveloperRegistrationError> {
    if let Ok(account) = serde_json::from_slice::<LegacyDeveloperAccountV1>(value) {
        return account.into_account();
    }
    let legacy: LegacyDeveloperIdentity = serde_json::from_slice(value)?;
    validate_text("legacy identity issuer", &legacy.issuer, 1, 2_048)?;
    validate_text("legacy identity subject", &legacy.subject, 1, 512)?;
    if legacy.status != DeveloperIdentityStatus::Active {
        return Err(DeveloperRegistrationError::MigrationCollision);
    }
    DeveloperAccount::migrated_active(
        legacy.id,
        legacy.normalized_email,
        legacy.display_name,
        legacy.created_at_unix_seconds,
        legacy.last_authenticated_at_unix_seconds,
    )
}

#[derive(Debug)]
pub enum DeveloperRegistrationError {
    InvalidField(&'static str),
    InvalidLifecycleTransition {
        from: DeveloperIdentityStatus,
        to: DeveloperIdentityStatus,
    },
    InvalidSecurityTransition,
    InvalidToken,
    InvalidSession,
    InvalidCursor,
    ScopeMismatch,
    Conflict,
    RateLimited,
    LimitExceeded,
    CorruptRecord,
    MigrationCollision,
    UnsupportedDurability(Durability),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for DeveloperRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField(field) => write!(formatter, "{field} is invalid"),
            Self::InvalidLifecycleTransition { .. } => {
                formatter.write_str("developer lifecycle transition is invalid")
            }
            Self::InvalidSecurityTransition => {
                formatter.write_str("authentication security transition is invalid")
            }
            Self::InvalidToken => formatter.write_str("developer token is invalid"),
            Self::InvalidSession => formatter.write_str("developer session is invalid"),
            Self::InvalidCursor => formatter.write_str("developer review cursor is invalid"),
            Self::ScopeMismatch => formatter.write_str("developer record scope does not match"),
            Self::Conflict => formatter.write_str("developer state changed concurrently"),
            Self::RateLimited => formatter.write_str("developer operation is rate limited"),
            Self::LimitExceeded => formatter.write_str("developer operation limit was exceeded"),
            Self::CorruptRecord => formatter.write_str("developer record is corrupt"),
            Self::MigrationCollision => {
                formatter.write_str("developer migration found an ambiguous identity collision")
            }
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "developer store does not support {durability:?} durability"
                )
            }
            Self::Keyspace(_) => formatter.write_str("developer storage key is invalid"),
            Self::Storage(_) => formatter.write_str("developer storage operation failed"),
            Self::Json(_) => formatter.write_str("developer record encoding failed"),
        }
    }
}

impl Error for DeveloperRegistrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ControlKeyspaceError> for DeveloperRegistrationError {
    fn from(value: ControlKeyspaceError) -> Self {
        Self::Keyspace(value)
    }
}

impl From<StorageError> for DeveloperRegistrationError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<serde_json::Error> for DeveloperRegistrationError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

fn validate_identifier(
    field: &'static str,
    value: &str,
    prefix: &str,
) -> Result<(), DeveloperRegistrationError> {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return Err(DeveloperRegistrationError::InvalidField(field));
    };
    if !(8..=96).contains(&suffix.len())
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DeveloperRegistrationError::InvalidField(field));
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), DeveloperRegistrationError> {
    if !(minimum..=maximum).contains(&value.len())
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DeveloperRegistrationError::InvalidField(field));
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), DeveloperRegistrationError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DeveloperRegistrationError::InvalidField(
            "developer private digest",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mako_storage::{Durability, KvAdapter, MemoryAdapter, WriteBatch};

    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn test_store(adapter: MemoryAdapter) -> DeveloperRegistrationStore {
        DeveloperRegistrationStore::new(
            Arc::new(adapter),
            Durability::Memory,
            DeveloperLookupKey::derive(b"test developer private lookup key"),
        )
        .expect("store")
    }

    fn account() -> DeveloperAccount {
        DeveloperAccount::new_unverified(
            DeveloperIdentityId::parse("dev_waitlist01").expect("id"),
            NormalizedEmail::parse("person@example.test").expect("email"),
            "Wait List Person",
            "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$YmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmI",
            NOW,
        )
        .expect("account")
    }

    fn encrypted_mail() -> EncryptedDeveloperMail {
        EncryptedDeveloperMail::new(
            URL_SAFE_NO_PAD.encode([1_u8; 24]),
            URL_SAFE_NO_PAD.encode(b"encrypted message"),
        )
        .expect("mail")
    }

    fn registration_records(
        store: &DeveloperRegistrationStore,
        account: &DeveloperAccount,
    ) -> (DeveloperAuthTokenRecord, DeveloperMailOutboxRecord) {
        let raw = DeveloperAuthToken::generate();
        let token = DeveloperAuthTokenRecord::new(
            DeveloperAuthTokenId::parse("dat_verify0001").expect("id"),
            account.id().clone(),
            DeveloperAuthTokenPurpose::EmailVerification,
            raw.digest(
                store.lookup_key(),
                DeveloperAuthTokenPurpose::EmailVerification,
            ),
            NOW,
            NOW + 3_600,
        )
        .expect("token");
        let outbox = DeveloperMailOutboxRecord::new(
            DeveloperMailOutboxId::parse("dmo_verify0001").expect("id"),
            account.id().clone(),
            DeveloperMailKind::VerifyEmail,
            encrypted_mail(),
            NOW,
        );
        (token, outbox)
    }

    #[test]
    fn account_lifecycle_is_explicit_and_advances_authority() {
        let mut account = account();
        assert!(!format!("{account:?}").contains("person@example.test"));
        account
            .transition(DeveloperIdentityStatus::Waitlisted, NOW + 1)
            .expect("verify");
        assert_eq!(account.authorization_epoch(), 2);
        assert_eq!(account.email_verified_at_unix_seconds(), Some(NOW + 1));
        assert!(
            account
                .transition(DeveloperIdentityStatus::Disabled, NOW + 2)
                .is_err()
        );
        let decision = DeveloperDecisionRecord::new(
            account.id().clone(),
            DeveloperDecisionKind::Approve,
            "opr_reviewer01",
            "approved for public preview",
            "request_review01",
            "a".repeat(64),
            "b".repeat(64),
            3,
            NOW + 2,
            NOW + 86_400,
        )
        .expect("decision");
        account.record_decision(&decision).expect("approve");
        assert_eq!(account.status(), DeveloperIdentityStatus::Active);
        assert_eq!(account.authorization_epoch(), 3);
    }

    #[test]
    fn decision_reasons_are_optional_redacted_and_storage_compatible() {
        let identity_id = account().id().clone();
        let without_reason = DeveloperDecisionRecord::new(
            identity_id.clone(),
            DeveloperDecisionKind::Approve,
            "opr_reviewer01",
            " \t ",
            "request_reason01",
            "a".repeat(64),
            "b".repeat(64),
            3,
            NOW + 1,
            NOW + 86_400,
        )
        .expect("reason-free decision");
        assert_eq!(without_reason.reason(), "");
        assert!(!without_reason.reason_provided());
        let encoded = serde_json::to_value(&without_reason).expect("serialize decision");
        assert_eq!(encoded["reason"], "");
        assert_eq!(encoded["reasonProvided"], false);
        let rejected_without_reason = DeveloperDecisionRecord::new(
            identity_id.clone(),
            DeveloperDecisionKind::Reject,
            "opr_reviewer01",
            "",
            "request_reason00",
            "0".repeat(64),
            "1".repeat(64),
            3,
            NOW + 1,
            NOW + 86_400,
        )
        .expect("reason-free rejection");
        assert_eq!(
            rejected_without_reason.resulting_status(),
            DeveloperIdentityStatus::Rejected
        );
        assert!(!rejected_without_reason.reason_provided());

        let private_reason = "approved for private beta review";
        let with_reason = DeveloperDecisionRecord::new(
            identity_id.clone(),
            DeveloperDecisionKind::Approve,
            "opr_reviewer01",
            format!("  {private_reason}  "),
            "request_reason02",
            "c".repeat(64),
            "d".repeat(64),
            3,
            NOW + 1,
            NOW + 86_400,
        )
        .expect("reasoned decision");
        assert_eq!(with_reason.reason(), private_reason);
        assert!(with_reason.reason_provided());
        assert!(!format!("{with_reason:?}").contains(private_reason));

        let mut legacy = serde_json::to_value(&with_reason).expect("serialize legacy fixture");
        legacy
            .as_object_mut()
            .expect("decision object")
            .remove("reasonProvided");
        let legacy: DeveloperDecisionRecord =
            serde_json::from_value(legacy).expect("read pre-field decision");
        legacy.validate().expect("validate pre-field decision");
        assert!(legacy.reason_provided());

        assert!(
            DeveloperDecisionRecord::new(
                identity_id,
                DeveloperDecisionKind::Reject,
                "opr_reviewer01",
                "short",
                "request_reason03",
                "e".repeat(64),
                "f".repeat(64),
                3,
                NOW + 1,
                NOW + 86_400,
            )
            .is_err()
        );
    }

    #[test]
    fn credential_security_and_developer_role_epochs_are_independent() {
        let mut account = account();
        let initial_credential_epoch = account.credential_epoch();
        account
            .transition(DeveloperIdentityStatus::Waitlisted, NOW + 1)
            .expect("waitlist");
        assert_eq!(account.credential_epoch(), initial_credential_epoch);
        assert_eq!(account.authorization_epoch(), 2);

        let developer_epoch = account.authorization_epoch();
        account
            .replace_password_hash(
                "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$Y2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2M",
                NOW + 2,
            )
            .expect("password change");
        assert_eq!(account.authorization_epoch(), developer_epoch);
        assert_eq!(account.credential_epoch(), initial_credential_epoch + 1);

        let credential_epoch = account.credential_epoch();
        account
            .identity
            .transition_security(AuthenticationSecurityStatus::Suspended, NOW + 3)
            .expect("suspend");
        assert_eq!(account.authorization_epoch(), developer_epoch);
        assert_eq!(account.credential_epoch(), credential_epoch + 1);
        assert_eq!(
            account.authentication_security_status(),
            AuthenticationSecurityStatus::Suspended
        );
        assert!(!format!("{:?}", account.identity()).contains("person@example.test"));
        assert!(!format!("{:?}", account.identity()).contains("argon2id"));
    }

    #[test]
    fn authentication_identity_can_exist_without_a_developer_role() {
        let adapter = MemoryAdapter::new();
        let store = test_store(adapter);
        let identity = AuthenticationIdentityRecord::new_verified(
            DeveloperIdentityId::parse("dev_operatoronly").expect("id"),
            NormalizedEmail::parse("operator-only@example.test").expect("email"),
            "Operator Only",
            "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$YmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmI",
            NOW,
        )
        .expect("identity");
        block_on(store.create_authentication_identity(&identity)).expect("create identity");
        assert_eq!(
            block_on(store.get_authentication_identity(identity.id()))
                .expect("read identity")
                .expect("identity"),
            identity
        );
        assert!(
            block_on(store.get_developer_role(identity.id()))
                .expect("read role")
                .is_none()
        );
        assert!(
            block_on(store.get_account(identity.id()))
                .expect("read account")
                .is_none()
        );

        let role = DeveloperRoleRecord::new_waitlisted(identity.id().clone(), NOW + 1)
            .expect("developer role");
        block_on(store.create_developer_role(&identity, &role)).expect("attach role");
        let account = block_on(store.get_account(identity.id()))
            .expect("read account")
            .expect("account");
        assert_eq!(account.status(), DeveloperIdentityStatus::Waitlisted);
        assert_eq!(account.credential_epoch(), identity.credential_epoch());
    }

    #[test]
    fn registration_is_atomic_unique_and_recovers_from_restart() {
        let adapter = MemoryAdapter::new();
        let store = test_store(adapter.clone());
        let account = account();
        let (token, outbox) = registration_records(&store, &account);
        block_on(store.create_unverified(&account, &token, &outbox)).expect("create");
        assert!(matches!(
            block_on(store.create_unverified(&account, &token, &outbox)),
            Err(DeveloperRegistrationError::Conflict)
        ));
        let reopened = test_store(adapter);
        assert_eq!(
            block_on(reopened.get_account_by_email(account.normalized_email()))
                .expect("read")
                .expect("account"),
            account
        );
        assert_eq!(
            block_on(reopened.get_token(token.digest()))
                .expect("token read")
                .expect("token"),
            token
        );
    }

    #[test]
    fn concurrent_decisions_have_one_winner_and_stable_indexes() {
        let adapter = MemoryAdapter::new();
        let store = test_store(adapter);
        let mut waitlisted = account();
        let (mut token, outbox) = registration_records(&store, &waitlisted);
        block_on(store.create_unverified(&waitlisted, &token, &outbox)).expect("create");
        let previous = waitlisted.clone();
        waitlisted
            .transition(DeveloperIdentityStatus::Waitlisted, NOW + 1)
            .expect("waitlist");
        token.consume(NOW + 1).expect("consume");
        let stored_token = block_on(store.get_token(token.digest()))
            .expect("read token")
            .expect("token");
        let mut consumed = stored_token.clone();
        consumed.consume(NOW + 1).expect("consume stored");
        block_on(store.consume_token_and_replace_account(
            &previous,
            &waitlisted,
            &stored_token,
            &consumed,
        ))
        .expect("verify");

        let approve = DeveloperDecisionRecord::new(
            waitlisted.id().clone(),
            DeveloperDecisionKind::Approve,
            "opr_reviewer01",
            "approved after review",
            "request_approve01",
            "c".repeat(64),
            "d".repeat(64),
            3,
            NOW + 2,
            NOW + 86_400,
        )
        .expect("approve");
        let reject = DeveloperDecisionRecord::new(
            waitlisted.id().clone(),
            DeveloperDecisionKind::Reject,
            "opr_reviewer02",
            "rejected after review",
            "request_reject01",
            "e".repeat(64),
            "f".repeat(64),
            3,
            NOW + 2,
            NOW + 86_400,
        )
        .expect("reject");
        let mut approved = waitlisted.clone();
        approved.record_decision(&approve).expect("approved");
        let mut rejected = waitlisted.clone();
        rejected.record_decision(&reject).expect("rejected");
        let approve_mail = DeveloperMailOutboxRecord::new(
            DeveloperMailOutboxId::parse("dmo_approve001").expect("id"),
            waitlisted.id().clone(),
            DeveloperMailKind::Approved,
            encrypted_mail(),
            NOW + 2,
        );
        let reject_mail = DeveloperMailOutboxRecord::new(
            DeveloperMailOutboxId::parse("dmo_reject0001").expect("id"),
            waitlisted.id().clone(),
            DeveloperMailKind::Rejected,
            encrypted_mail(),
            NOW + 2,
        );
        block_on(store.decide(&waitlisted, &approved, &approve, &approve_mail)).expect("winner");
        assert!(matches!(
            block_on(store.decide(&waitlisted, &rejected, &reject, &reject_mail)),
            Err(DeveloperRegistrationError::Conflict)
        ));
        let page = block_on(store.review_page(
            DeveloperIdentityStatus::Active,
            None,
            NonZeroUsize::new(10).expect("limit"),
        ))
        .expect("page");
        assert_eq!(page.accounts, vec![approved]);
    }

    #[test]
    fn legacy_migration_is_resumable_and_refuses_email_collisions() {
        let adapter = MemoryAdapter::new();
        let first = serde_json::json!({
            "id": "dev_legacy0001",
            "issuer": "https://issuer.example.test",
            "subject": "legacy-one",
            "normalizedEmail": "person@example.test",
            "displayName": "Legacy One",
            "status": "active",
            "createdAtUnixSeconds": NOW,
            "lastAuthenticatedAtUnixSeconds": NOW,
        });
        let key = ControlKeyspace::developer_key(
            &DeveloperIdentityId::parse("dev_legacy0001").expect("id"),
        )
        .expect("key");
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&first).expect("json"));
        block_on(adapter.write(batch, Durability::Memory)).expect("seed");
        let store = test_store(adapter.clone());
        let report = block_on(store.migrate_legacy_developers()).expect("migrate");
        assert_eq!(report.migrated, 1);
        let report = block_on(store.migrate_legacy_developers()).expect("repeat");
        assert_eq!(report.scanned, 0);
        let migrated =
            block_on(store.get_account(&DeveloperIdentityId::parse("dev_legacy0001").expect("id")))
                .expect("read")
                .expect("account");
        assert_eq!(migrated.status(), DeveloperIdentityStatus::Active);
        assert!(migrated.password_hash().is_none());

        let collision_adapter = MemoryAdapter::new();
        let mut seed = WriteBatch::new();
        for (id, subject) in [("dev_collision1", "one"), ("dev_collision2", "two")] {
            let value = serde_json::json!({
                "id": id,
                "issuer": "https://issuer.example.test",
                "subject": subject,
                "normalizedEmail": "same@example.test",
                "displayName": "Collision",
                "status": "active",
                "createdAtUnixSeconds": NOW,
                "lastAuthenticatedAtUnixSeconds": NOW,
            });
            let id = DeveloperIdentityId::parse(id).expect("id");
            seed.put(
                ControlKeyspace::developer_key(&id).expect("key"),
                serde_json::to_vec(&value).expect("json"),
            );
        }
        block_on(collision_adapter.write(seed, Durability::Memory)).expect("seed collisions");
        let collision_store = test_store(collision_adapter);
        assert!(matches!(
            block_on(collision_store.migrate_legacy_developers()),
            Err(DeveloperRegistrationError::MigrationCollision)
        ));
    }

    #[test]
    fn version_one_account_backfill_resumes_and_maps_deleted_security() {
        let adapter = MemoryAdapter::new();
        let id = DeveloperIdentityId::parse("dev_legacyv1001").expect("id");
        let legacy = serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "normalizedEmail": "deleted@example.test",
            "displayName": "Deleted Legacy",
            "passwordHash": null,
            "status": "deleted",
            "emailVerifiedAtUnixSeconds": NOW,
            "authorizationEpoch": 7,
            "createdAtUnixSeconds": NOW,
            "updatedAtUnixSeconds": NOW + 5,
            "lastAuthenticatedAtUnixSeconds": null,
            "review": null,
        });
        let legacy_key = ControlKeyspace::developer_key(&id).expect("legacy key");
        let account = parse_legacy_account(&serde_json::to_vec(&legacy).expect("legacy json"))
            .expect("parse legacy");
        let identity_key = ControlKeyspace::authentication_identity_key(&id).expect("identity key");
        let mut seed = WriteBatch::new();
        seed.put(
            &legacy_key,
            serde_json::to_vec(&legacy).expect("legacy json"),
        );
        seed.put(
            &identity_key,
            serde_json::to_vec(account.identity()).expect("identity json"),
        );
        block_on(adapter.write(seed, Durability::Memory)).expect("partial migration seed");

        let store = test_store(adapter);
        let report = block_on(store.migrate_legacy_developers()).expect("resume migration");
        assert_eq!(report.migrated, 1);
        let migrated = block_on(store.get_account(&id))
            .expect("read")
            .expect("account");
        assert_eq!(migrated.status(), DeveloperIdentityStatus::Deleted);
        assert_eq!(
            migrated.authentication_security_status(),
            AuthenticationSecurityStatus::Deleted
        );
        assert_eq!(migrated.authorization_epoch(), 7);
        assert_eq!(migrated.credential_epoch(), 7);
    }

    #[test]
    fn credentials_and_mail_records_redact_and_enforce_bounds() {
        let token = DeveloperAuthToken::generate();
        let refresh = DeveloperRefreshCredential::generate(
            DeveloperRefreshSessionId::parse("drs_redaction01").expect("session id"),
        );
        assert_eq!(format!("{token:?}"), "DeveloperAuthToken([REDACTED])");
        assert_eq!(
            format!("{refresh:?}"),
            "DeveloperRefreshCredential([REDACTED])"
        );
        assert!(EncryptedDeveloperMail::new("bad", "bad").is_err());
        let account = account();
        assert!(!format!("{account:?}").contains(account.normalized_email().as_str()));
    }

    #[test]
    fn cleanup_is_bounded_and_removes_only_expired_records() {
        let adapter = MemoryAdapter::new();
        let store = test_store(adapter);
        let account = account();
        let (token, outbox) = registration_records(&store, &account);
        block_on(store.create_unverified(&account, &token, &outbox)).expect("create");
        let report = block_on(store.cleanup_expired(
            NOW + 3_601,
            86_400,
            NonZeroUsize::new(10).expect("limit"),
        ))
        .expect("cleanup");
        assert_eq!(report.auth_tokens, 1);
        assert_eq!(report.mail_outbox, 0);
        assert!(
            block_on(store.get_token(token.digest()))
                .expect("read")
                .is_none()
        );
        assert!(
            block_on(store.get_outbox(outbox.id()))
                .expect("outbox")
                .is_some()
        );
        assert!(
            block_on(store.cleanup_expired(
                NOW,
                86_400,
                NonZeroUsize::new(MAXIMUM_REVIEW_PAGE + 1).expect("limit"),
            ))
            .is_err()
        );
        assert!(require_unsaturated_cleanup_scan(MAXIMUM_MIGRATION_RECORDS - 1).is_ok());
        assert!(matches!(
            require_unsaturated_cleanup_scan(MAXIMUM_MIGRATION_RECORDS),
            Err(DeveloperRegistrationError::LimitExceeded)
        ));
    }
}
