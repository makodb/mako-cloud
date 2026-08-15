use std::{error::Error, fmt};

use mako_api::TenantScope;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value};

const MAX_METADATA_BYTES: usize = 64 * 1024;
const MAX_METADATA_DEPTH: usize = 16;

macro_rules! record_id {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, IdentityRecordError> {
                let value = value.into();
                validate_id($field, &value)?;
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

record_id!(AppUserId, "user id");
record_id!(UserIdentityId, "identity id");
record_id!(UserCredentialId, "credential id");
record_id!(SessionId, "session id");
record_id!(TokenFamilyId, "token family id");

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppUserStatus {
    PendingVerification,
    Active,
    Disabled,
    Deleted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Revoked,
    Expired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenFamilyStatus {
    Active,
    Revoked,
    ReplayDetected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "name")]
pub enum IdentityProvider {
    Email,
    Oidc(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserCredentialKind {
    Password,
    EmailVerification,
    PasswordRecovery,
}

#[derive(Clone, Eq, PartialEq)]
pub struct CredentialDigest(Vec<u8>);

impl CredentialDigest {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, IdentityRecordError> {
        let bytes = bytes.into();
        if !(16..=1024).contains(&bytes.len()) {
            return Err(IdentityRecordError::InvalidField {
                field: "credential digest",
                reason: "must contain 16-1024 bytes",
            });
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for CredentialDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialDigest([REDACTED])")
    }
}

impl Serialize for CredentialDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for CredentialDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        Self::new(bytes).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TrustedAppMetadata(Map<String, Value>);

impl TrustedAppMetadata {
    pub fn new(value: Value) -> Result<Self, IdentityRecordError> {
        validate_metadata("trusted app metadata", value).map(Self)
    }

    #[must_use]
    pub fn values(&self) -> &Map<String, Value> {
        &self.0
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UserProfileMetadata(Map<String, Value>);

impl UserProfileMetadata {
    pub fn new(value: Value) -> Result<Self, IdentityRecordError> {
        validate_metadata("user profile metadata", value).map(Self)
    }

    #[must_use]
    pub fn values(&self) -> &Map<String, Value> {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppUserRecord {
    scope: TenantScope,
    id: AppUserId,
    status: AppUserStatus,
    trusted_metadata: TrustedAppMetadata,
    profile_metadata: UserProfileMetadata,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    session_epoch: u64,
}

impl AppUserRecord {
    #[must_use]
    pub fn new(
        scope: TenantScope,
        id: AppUserId,
        status: AppUserStatus,
        trusted_metadata: TrustedAppMetadata,
        profile_metadata: UserProfileMetadata,
        created_at_unix_seconds: u64,
    ) -> Self {
        Self {
            scope,
            id,
            status,
            trusted_metadata,
            profile_metadata,
            created_at_unix_seconds,
            updated_at_unix_seconds: created_at_unix_seconds,
            session_epoch: 0,
        }
    }

    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    #[must_use]
    pub fn id(&self) -> &AppUserId {
        &self.id
    }

    #[must_use]
    pub const fn status(&self) -> AppUserStatus {
        self.status
    }

    #[must_use]
    pub fn trusted_metadata(&self) -> &TrustedAppMetadata {
        &self.trusted_metadata
    }

    #[must_use]
    pub fn profile_metadata(&self) -> &UserProfileMetadata {
        &self.profile_metadata
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
    pub fn with_metadata(
        mut self,
        trusted_metadata: TrustedAppMetadata,
        profile_metadata: UserProfileMetadata,
        changed_at_unix_seconds: u64,
    ) -> Self {
        self.trusted_metadata = trusted_metadata;
        self.profile_metadata = profile_metadata;
        self.updated_at_unix_seconds = changed_at_unix_seconds;
        self
    }

    #[must_use]
    pub fn with_status(mut self, status: AppUserStatus, changed_at_unix_seconds: u64) -> Self {
        self.status = status;
        self.updated_at_unix_seconds = changed_at_unix_seconds;
        self
    }

    #[must_use]
    pub const fn session_epoch(&self) -> u64 {
        self.session_epoch
    }

    pub fn revoke_all_sessions(
        mut self,
        changed_at_unix_seconds: u64,
    ) -> Result<Self, IdentityRecordError> {
        self.session_epoch = self
            .session_epoch
            .checked_add(1)
            .ok_or(IdentityRecordError::SessionEpochExhausted)?;
        self.updated_at_unix_seconds = changed_at_unix_seconds;
        Ok(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserIdentityRecord {
    scope: TenantScope,
    id: UserIdentityId,
    user_id: AppUserId,
    provider: IdentityProvider,
    provider_subject: String,
    created_at_unix_seconds: u64,
}

impl UserIdentityRecord {
    pub fn new(
        scope: TenantScope,
        id: UserIdentityId,
        user_id: AppUserId,
        provider: IdentityProvider,
        provider_subject: impl Into<String>,
        created_at_unix_seconds: u64,
    ) -> Result<Self, IdentityRecordError> {
        let provider_subject = provider_subject.into();
        if provider_subject.is_empty() || provider_subject.len() > 1024 {
            return Err(IdentityRecordError::InvalidField {
                field: "provider subject",
                reason: "must contain 1-1024 characters",
            });
        }
        Ok(Self {
            scope,
            id,
            user_id,
            provider,
            provider_subject,
            created_at_unix_seconds,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    #[must_use]
    pub fn user_id(&self) -> &AppUserId {
        &self.user_id
    }

    #[must_use]
    pub fn id(&self) -> &UserIdentityId {
        &self.id
    }

    #[must_use]
    pub fn provider(&self) -> &IdentityProvider {
        &self.provider
    }

    #[must_use]
    pub fn provider_subject(&self) -> &str {
        &self.provider_subject
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserCredentialRecord {
    scope: TenantScope,
    id: UserCredentialId,
    user_id: AppUserId,
    kind: UserCredentialKind,
    digest: CredentialDigest,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: Option<u64>,
    invalidated_at_unix_seconds: Option<u64>,
}

impl UserCredentialRecord {
    #[must_use]
    pub fn new(
        scope: TenantScope,
        id: UserCredentialId,
        user_id: AppUserId,
        kind: UserCredentialKind,
        digest: CredentialDigest,
        created_at_unix_seconds: u64,
        expires_at_unix_seconds: Option<u64>,
    ) -> Self {
        Self {
            scope,
            id,
            user_id,
            kind,
            digest,
            created_at_unix_seconds,
            expires_at_unix_seconds,
            invalidated_at_unix_seconds: None,
        }
    }

    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    #[must_use]
    pub fn digest(&self) -> &CredentialDigest {
        &self.digest
    }

    #[must_use]
    pub fn id(&self) -> &UserCredentialId {
        &self.id
    }

    #[must_use]
    pub fn user_id(&self) -> &AppUserId {
        &self.user_id
    }

    #[must_use]
    pub const fn kind(&self) -> UserCredentialKind {
        self.kind
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> Option<u64> {
        self.expires_at_unix_seconds
    }

    #[must_use]
    pub const fn invalidated_at_unix_seconds(&self) -> Option<u64> {
        self.invalidated_at_unix_seconds
    }

    #[must_use]
    pub fn invalidate(mut self, at_unix_seconds: u64) -> Self {
        self.invalidated_at_unix_seconds = Some(at_unix_seconds);
        self
    }

    #[must_use]
    pub fn with_digest(mut self, digest: CredentialDigest) -> Self {
        self.digest = digest;
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionRecord {
    scope: TenantScope,
    id: SessionId,
    user_id: AppUserId,
    status: SessionStatus,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    revoked_at_unix_seconds: Option<u64>,
    user_session_epoch: u64,
}

impl SessionRecord {
    #[must_use]
    pub fn new(
        scope: TenantScope,
        id: SessionId,
        user_id: AppUserId,
        created_at_unix_seconds: u64,
        expires_at_unix_seconds: u64,
        user_session_epoch: u64,
    ) -> Self {
        Self {
            scope,
            id,
            user_id,
            status: SessionStatus::Active,
            created_at_unix_seconds,
            expires_at_unix_seconds,
            revoked_at_unix_seconds: None,
            user_session_epoch,
        }
    }

    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    #[must_use]
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    #[must_use]
    pub fn user_id(&self) -> &AppUserId {
        &self.user_id
    }

    #[must_use]
    pub const fn status(&self) -> SessionStatus {
        self.status
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    #[must_use]
    pub const fn revoked_at_unix_seconds(&self) -> Option<u64> {
        self.revoked_at_unix_seconds
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn user_session_epoch(&self) -> u64 {
        self.user_session_epoch
    }

    #[must_use]
    pub fn revoke(mut self, at_unix_seconds: u64) -> Self {
        self.status = SessionStatus::Revoked;
        self.revoked_at_unix_seconds = Some(at_unix_seconds);
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TokenFamilyRecord {
    scope: TenantScope,
    id: TokenFamilyId,
    session_id: SessionId,
    user_id: AppUserId,
    status: TokenFamilyStatus,
    current_generation: u64,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
}

impl TokenFamilyRecord {
    #[must_use]
    pub fn new(
        scope: TenantScope,
        id: TokenFamilyId,
        session_id: SessionId,
        user_id: AppUserId,
        created_at_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Self {
        Self {
            scope,
            id,
            session_id,
            user_id,
            status: TokenFamilyStatus::Active,
            current_generation: 0,
            created_at_unix_seconds,
            expires_at_unix_seconds,
        }
    }

    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }
}

fn validate_id(field: &'static str, value: &str) -> Result<(), IdentityRecordError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(IdentityRecordError::InvalidField {
            field,
            reason: "must contain 1-128 ASCII letters, digits, hyphens, or underscores",
        });
    }
    Ok(())
}

fn validate_metadata(
    field: &'static str,
    value: Value,
) -> Result<Map<String, Value>, IdentityRecordError> {
    let Value::Object(values) = value else {
        return Err(IdentityRecordError::InvalidField {
            field,
            reason: "must be a JSON object",
        });
    };
    let value = Value::Object(values.clone());
    if serde_json::to_vec(&value)?.len() > MAX_METADATA_BYTES {
        return Err(IdentityRecordError::InvalidField {
            field,
            reason: "exceeds the metadata size limit",
        });
    }
    validate_depth(&value, 0, field)?;
    Ok(values)
}

fn validate_depth(
    value: &Value,
    depth: usize,
    field: &'static str,
) -> Result<(), IdentityRecordError> {
    if depth > MAX_METADATA_DEPTH {
        return Err(IdentityRecordError::InvalidField {
            field,
            reason: "exceeds the metadata depth limit",
        });
    }
    match value {
        Value::Array(values) => {
            for value in values {
                validate_depth(value, depth + 1, field)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_depth(value, depth + 1, field)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug)]
pub enum IdentityRecordError {
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    Json(serde_json::Error),
    SessionEpochExhausted,
}

impl fmt::Display for IdentityRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::Json(error) => error.fmt(formatter),
            Self::SessionEpochExhausted => formatter.write_str("user session epoch is exhausted"),
        }
    }
}

impl Error for IdentityRecordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::InvalidField { .. } | Self::SessionEpochExhausted => None,
        }
    }
}

impl From<serde_json::Error> for IdentityRecordError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId};
    use serde_json::json;

    use super::*;

    #[test]
    fn records_retain_project_environment_scope_and_separate_metadata() {
        let user = AppUserRecord::new(
            tenant(),
            AppUserId::parse("usr_abcdefgh").expect("user"),
            AppUserStatus::Active,
            TrustedAppMetadata::new(json!({"role": "admin"})).expect("trusted"),
            UserProfileMetadata::new(json!({"displayName": "Admin"})).expect("profile"),
            10,
        );
        let encoded = serde_json::to_vec(&user).expect("encode");
        let decoded: AppUserRecord = serde_json::from_slice(&encoded).expect("decode");

        assert_eq!(decoded.scope(), &tenant());
        assert_eq!(decoded.trusted_metadata().values()["role"], "admin");
        assert_eq!(decoded.profile_metadata().values()["displayName"], "Admin");
    }

    #[test]
    fn credential_material_is_never_exposed_by_debug() {
        let digest = CredentialDigest::new(vec![7; 32]).expect("digest");
        assert_eq!(format!("{digest:?}"), "CredentialDigest([REDACTED])");
        assert!(!format!("{digest:?}").contains('7'));
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
