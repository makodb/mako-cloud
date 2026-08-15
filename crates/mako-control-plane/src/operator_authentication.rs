use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    AuthenticationIdentityRecord, AuthenticationSecurityStatus, ControlKeyspace,
    ControlKeyspaceError, DeveloperIdentityId, DeveloperIdentityStatus,
    DeveloperRegistrationService, DeveloperRoleRecord, DeveloperWorkflowError, OperatorId,
    OperatorPermission, OperatorPrincipal,
};

const RECORD_SCHEMA_VERSION: u32 = 1;
const OPERATOR_SESSION_SCHEMA_VERSION: u32 = 2;
const MIGRATION_VERSION: u32 = 1;
const SECRET_BYTES: usize = 32;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_SCAN_RECORDS: usize = 10_000;
const MAX_SESSION_SECONDS: u64 = 60 * 60;
const MAX_ATTEMPT_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Eq, PartialEq)]
pub struct OperatorAuthenticationKey([u8; 32]);

impl OperatorAuthenticationKey {
    #[must_use]
    pub fn derive(secret: &[u8]) -> Self {
        Self(blake3::derive_key(
            "mako/control-plane/operator-authentication/v1",
            secret,
        ))
    }

    #[must_use]
    pub fn stable_operator_id(&self, identity_id: &DeveloperIdentityId) -> OperatorId {
        let digest = self.digest("operator-id", identity_id.as_str());
        OperatorId::parse(format!("opr_{}", &digest[..32]))
            .expect("a keyed lowercase hex digest is a valid operator id")
    }

    #[must_use]
    pub fn session_digest(&self, credential: &OperatorSessionCredential) -> String {
        self.digest("session", credential.expose_to_store())
    }

    #[must_use]
    pub fn attempt_digest(&self, class: &str, value: &str) -> String {
        self.digest(&format!("attempt/{class}"), value)
    }

    #[must_use]
    pub fn idempotency_digest(&self, value: &str) -> String {
        self.digest("idempotency", value)
    }

    fn digest(&self, purpose: &str, value: &str) -> String {
        let mut input = Vec::with_capacity(purpose.len() + value.len() + 1);
        input.extend_from_slice(purpose.as_bytes());
        input.push(0);
        input.extend_from_slice(value.as_bytes());
        blake3::keyed_hash(&self.0, &input).to_hex().to_string()
    }
}

impl fmt::Debug for OperatorAuthenticationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperatorAuthenticationKey([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct OperatorSessionCredential(String);

impl OperatorSessionCredential {
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; SECRET_BYTES];
        OsRng.fill_bytes(&mut bytes);
        Self(format!("mos_{}", URL_SAFE_NO_PAD.encode(bytes)))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, OperatorAuthenticationStoreError> {
        let value = value.into();
        let encoded = value
            .strip_prefix("mos_")
            .ok_or(OperatorAuthenticationStoreError::InvalidCredential)?;
        let decoded = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| OperatorAuthenticationStoreError::InvalidCredential)?;
        if decoded.len() != SECRET_BYTES || value.chars().any(char::is_control) {
            return Err(OperatorAuthenticationStoreError::InvalidCredential);
        }
        Ok(Self(value))
    }

    fn expose_to_store(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn expose_to_cookie(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OperatorSessionCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperatorSessionCredential([REDACTED])")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorEntitlementRecord {
    schema_version: u32,
    developer_identity_id: DeveloperIdentityId,
    operator_id: OperatorId,
    operator_epoch: u64,
    permissions: BTreeSet<OperatorPermission>,
    private_reason: String,
    audit_event_id: String,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

impl OperatorEntitlementRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        developer_identity_id: DeveloperIdentityId,
        operator_id: OperatorId,
        operator_epoch: u64,
        permissions: impl IntoIterator<Item = OperatorPermission>,
        private_reason: impl Into<String>,
        audit_event_id: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, OperatorAuthenticationStoreError> {
        let record = Self {
            schema_version: RECORD_SCHEMA_VERSION,
            developer_identity_id,
            operator_id,
            operator_epoch,
            permissions: permissions.into_iter().collect(),
            private_reason: private_reason.into(),
            audit_event_id: audit_event_id.into(),
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn replacement(
        &self,
        permissions: impl IntoIterator<Item = OperatorPermission>,
        private_reason: impl Into<String>,
        audit_event_id: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<Self, OperatorAuthenticationStoreError> {
        let record = Self {
            schema_version: RECORD_SCHEMA_VERSION,
            developer_identity_id: self.developer_identity_id.clone(),
            operator_id: self.operator_id.clone(),
            operator_epoch: self
                .operator_epoch
                .checked_add(1)
                .ok_or(OperatorAuthenticationStoreError::InvalidRecord)?,
            permissions: permissions.into_iter().collect(),
            private_reason: private_reason.into(),
            audit_event_id: audit_event_id.into(),
            created_at_unix_seconds: self.created_at_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), OperatorAuthenticationStoreError> {
        if self.schema_version != RECORD_SCHEMA_VERSION
            || self.operator_epoch == 0
            || self.permissions.is_empty()
            || self.permissions.len() > 17
            || self.updated_at_unix_seconds < self.created_at_unix_seconds
            || !valid_text(&self.private_reason, 8, 1_024)
            || !valid_text(&self.audit_event_id, 8, 256)
        {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        Ok(())
    }

    #[must_use]
    pub fn developer_identity_id(&self) -> &DeveloperIdentityId {
        &self.developer_identity_id
    }

    #[must_use]
    pub fn operator_id(&self) -> &OperatorId {
        &self.operator_id
    }

    #[must_use]
    pub const fn operator_epoch(&self) -> u64 {
        self.operator_epoch
    }

    #[must_use]
    pub fn permissions(&self) -> &BTreeSet<OperatorPermission> {
        &self.permissions
    }
}

impl fmt::Debug for OperatorEntitlementRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorEntitlementRecord")
            .field("developer_identity_id", &self.developer_identity_id)
            .field("operator_id", &self.operator_id)
            .field("operator_epoch", &self.operator_epoch)
            .field("permissions", &self.permissions)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorSessionRecord {
    schema_version: u32,
    digest: String,
    developer_identity_id: DeveloperIdentityId,
    operator_id: OperatorId,
    credential_epoch: u64,
    operator_epoch: u64,
    permissions: BTreeSet<OperatorPermission>,
    issued_at_unix_seconds: u64,
    password_verified_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    revoked_at_unix_seconds: Option<u64>,
    audit_event_id: String,
}

impl OperatorSessionRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        digest: impl Into<String>,
        developer_identity_id: DeveloperIdentityId,
        operator_id: OperatorId,
        credential_epoch: u64,
        operator_epoch: u64,
        permissions: impl IntoIterator<Item = OperatorPermission>,
        now_unix_seconds: u64,
        expires_at_unix_seconds: u64,
        audit_event_id: impl Into<String>,
    ) -> Result<Self, OperatorAuthenticationStoreError> {
        let record = Self {
            schema_version: OPERATOR_SESSION_SCHEMA_VERSION,
            digest: digest.into(),
            developer_identity_id,
            operator_id,
            credential_epoch,
            operator_epoch,
            permissions: permissions.into_iter().collect(),
            issued_at_unix_seconds: now_unix_seconds,
            password_verified_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
            revoked_at_unix_seconds: None,
            audit_event_id: audit_event_id.into(),
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), OperatorAuthenticationStoreError> {
        let lifetime = self
            .expires_at_unix_seconds
            .checked_sub(self.issued_at_unix_seconds)
            .ok_or(OperatorAuthenticationStoreError::InvalidRecord)?;
        if self.schema_version != OPERATOR_SESSION_SCHEMA_VERSION
            || !valid_digest(&self.digest)
            || self.credential_epoch == 0
            || self.operator_epoch == 0
            || self.permissions.is_empty()
            || self.permissions.len() > 17
            || lifetime == 0
            || lifetime > MAX_SESSION_SECONDS
            || self.password_verified_at_unix_seconds < self.issued_at_unix_seconds
            || self.password_verified_at_unix_seconds > self.expires_at_unix_seconds
            || self.revoked_at_unix_seconds.is_some_and(|value| {
                value < self.issued_at_unix_seconds || value > self.expires_at_unix_seconds
            })
            || !valid_text(&self.audit_event_id, 8, 256)
        {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        Ok(())
    }

    pub fn verify_password(
        &mut self,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        if self.revoked_at_unix_seconds.is_some()
            || now_unix_seconds >= self.expires_at_unix_seconds
        {
            return Err(OperatorAuthenticationStoreError::InvalidSession);
        }
        self.password_verified_at_unix_seconds = now_unix_seconds;
        self.validate()
    }

    pub fn revoke(
        &mut self,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        if now_unix_seconds < self.issued_at_unix_seconds {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        self.revoked_at_unix_seconds = Some(now_unix_seconds.min(self.expires_at_unix_seconds));
        self.validate()
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn developer_identity_id(&self) -> &DeveloperIdentityId {
        &self.developer_identity_id
    }

    #[must_use]
    pub fn operator_id(&self) -> &OperatorId {
        &self.operator_id
    }

    #[must_use]
    pub const fn credential_epoch(&self) -> u64 {
        self.credential_epoch
    }

    #[must_use]
    pub const fn operator_epoch(&self) -> u64 {
        self.operator_epoch
    }

    #[must_use]
    pub fn permissions(&self) -> &BTreeSet<OperatorPermission> {
        &self.permissions
    }

    #[must_use]
    pub const fn password_verified_at_unix_seconds(&self) -> u64 {
        self.password_verified_at_unix_seconds
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    #[must_use]
    pub const fn revoked_at_unix_seconds(&self) -> Option<u64> {
        self.revoked_at_unix_seconds
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyOperatorSessionRecordV1 {
    schema_version: u32,
    digest: String,
    developer_identity_id: DeveloperIdentityId,
    operator_id: OperatorId,
    developer_authorization_epoch: u64,
    operator_epoch: u64,
    permissions: BTreeSet<OperatorPermission>,
    issued_at_unix_seconds: u64,
    password_verified_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    revoked_at_unix_seconds: Option<u64>,
    audit_event_id: String,
}

impl LegacyOperatorSessionRecordV1 {
    fn validate(&self) -> Result<(), OperatorAuthenticationStoreError> {
        let lifetime = self
            .expires_at_unix_seconds
            .checked_sub(self.issued_at_unix_seconds)
            .ok_or(OperatorAuthenticationStoreError::InvalidRecord)?;
        if self.schema_version != RECORD_SCHEMA_VERSION
            || !valid_digest(&self.digest)
            || self.developer_authorization_epoch == 0
            || self.operator_epoch == 0
            || self.permissions.is_empty()
            || self.permissions.len() > 17
            || lifetime == 0
            || lifetime > MAX_SESSION_SECONDS
            || self.password_verified_at_unix_seconds < self.issued_at_unix_seconds
            || self.password_verified_at_unix_seconds > self.expires_at_unix_seconds
            || self.revoked_at_unix_seconds.is_some_and(|value| {
                value < self.issued_at_unix_seconds || value > self.expires_at_unix_seconds
            })
            || !valid_text(&self.audit_event_id, 8, 256)
            || self.developer_identity_id.as_str().is_empty()
            || self.operator_id.as_str().is_empty()
        {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        Ok(())
    }
}

enum DecodedOperatorSession {
    Current(OperatorSessionRecord),
    Legacy(LegacyOperatorSessionRecordV1),
}

fn decode_operator_session(
    value: &[u8],
) -> Result<DecodedOperatorSession, OperatorAuthenticationStoreError> {
    if let Ok(record) = decode_record::<OperatorSessionRecord>(value) {
        record.validate()?;
        return Ok(DecodedOperatorSession::Current(record));
    }
    let legacy: LegacyOperatorSessionRecordV1 = decode_record(value)?;
    legacy.validate()?;
    Ok(DecodedOperatorSession::Legacy(legacy))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorAttemptClass {
    Source,
    Identity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorAttemptRecord {
    schema_version: u32,
    digest: String,
    class: OperatorAttemptClass,
    count: u32,
    window_started_at_unix_seconds: u64,
    next_allowed_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
}

impl OperatorAttemptRecord {
    pub fn new(
        digest: impl Into<String>,
        class: OperatorAttemptClass,
        now_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<Self, OperatorAuthenticationStoreError> {
        let record = Self {
            schema_version: RECORD_SCHEMA_VERSION,
            digest: digest.into(),
            class,
            count: 1,
            window_started_at_unix_seconds: now_unix_seconds,
            next_allowed_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), OperatorAuthenticationStoreError> {
        if self.schema_version != RECORD_SCHEMA_VERSION
            || !valid_digest(&self.digest)
            || self.count == 0
            || self.expires_at_unix_seconds <= self.window_started_at_unix_seconds
            || self.expires_at_unix_seconds - self.window_started_at_unix_seconds
                > MAX_ATTEMPT_SECONDS
            || self.next_allowed_at_unix_seconds < self.window_started_at_unix_seconds
            || self.next_allowed_at_unix_seconds > self.expires_at_unix_seconds
        {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        Ok(())
    }

    pub fn record_failure(
        &mut self,
        now_unix_seconds: u64,
        base_backoff_seconds: u64,
        maximum_backoff_seconds: u64,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        if now_unix_seconds >= self.expires_at_unix_seconds {
            return Err(OperatorAuthenticationStoreError::Expired);
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or(OperatorAuthenticationStoreError::InvalidRecord)?;
        let shift = self.count.saturating_sub(1).min(20);
        let backoff = base_backoff_seconds
            .saturating_mul(1_u64 << shift)
            .min(maximum_backoff_seconds);
        self.next_allowed_at_unix_seconds = now_unix_seconds
            .saturating_add(backoff)
            .min(self.expires_at_unix_seconds);
        self.validate()
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub const fn class(&self) -> OperatorAttemptClass {
        self.class
    }

    #[must_use]
    pub const fn count(&self) -> u32 {
        self.count
    }

    #[must_use]
    pub const fn next_allowed_at_unix_seconds(&self) -> u64 {
        self.next_allowed_at_unix_seconds
    }

    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorIdempotencyRecord {
    schema_version: u32,
    digest: String,
    request_digest: String,
    operation_digest: String,
    developer_identity_id: DeveloperIdentityId,
    resulting_operator_epoch: u64,
    audit_event_id: String,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
}

impl OperatorIdempotencyRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        digest: impl Into<String>,
        request_digest: impl Into<String>,
        operation_digest: impl Into<String>,
        developer_identity_id: DeveloperIdentityId,
        resulting_operator_epoch: u64,
        audit_event_id: impl Into<String>,
        now_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<Self, OperatorAuthenticationStoreError> {
        let record = Self {
            schema_version: RECORD_SCHEMA_VERSION,
            digest: digest.into(),
            request_digest: request_digest.into(),
            operation_digest: operation_digest.into(),
            developer_identity_id,
            resulting_operator_epoch,
            audit_event_id: audit_event_id.into(),
            created_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), OperatorAuthenticationStoreError> {
        if self.schema_version != RECORD_SCHEMA_VERSION
            || !valid_digest(&self.digest)
            || !valid_digest(&self.request_digest)
            || !valid_digest(&self.operation_digest)
            || self.resulting_operator_epoch == 0
            || !valid_text(&self.audit_event_id, 8, 256)
            || self.expires_at_unix_seconds <= self.created_at_unix_seconds
        {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        Ok(())
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }

    #[must_use]
    pub fn operation_digest(&self) -> &str {
        &self.operation_digest
    }

    #[must_use]
    pub const fn resulting_operator_epoch(&self) -> u64 {
        self.resulting_operator_epoch
    }

    #[must_use]
    pub fn developer_identity_id(&self) -> &DeveloperIdentityId {
        &self.developer_identity_id
    }

    #[must_use]
    pub fn audit_event_id(&self) -> &str {
        &self.audit_event_id
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OperatorAuthenticationCleanupReport {
    pub sessions: usize,
    pub attempts: usize,
    pub idempotency: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OperatorAuthenticationHealthSnapshot {
    pub active_sessions: usize,
    pub revoked_sessions: usize,
    pub entitlements: usize,
    pub attempt_records: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorAuthenticationMigrationReport {
    pub version: u32,
    pub completed: bool,
}

#[derive(Clone)]
pub struct OperatorAuthenticationStore {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    authentication_key: OperatorAuthenticationKey,
}

impl fmt::Debug for OperatorAuthenticationStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorAuthenticationStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl OperatorAuthenticationStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        authentication_key: OperatorAuthenticationKey,
    ) -> Result<Self, OperatorAuthenticationStoreError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(OperatorAuthenticationStoreError::UnsupportedDurability(
                durability,
            ));
        }
        Ok(Self {
            adapter,
            durability,
            authentication_key,
        })
    }

    #[must_use]
    pub fn authentication_key(&self) -> &OperatorAuthenticationKey {
        &self.authentication_key
    }

    pub async fn migrate(
        &self,
    ) -> Result<OperatorAuthenticationMigrationReport, OperatorAuthenticationStoreError> {
        let key = ControlKeyspace::operator_migration_key(MIGRATION_VERSION)?;
        if self.adapter.get(&key).await?.is_none() {
            let value = serde_json::to_vec(&serde_json::json!({
                "schemaVersion": RECORD_SCHEMA_VERSION,
                "migrationVersion": MIGRATION_VERSION,
                "completed": true,
            }))?;
            let mut batch = WriteBatch::new();
            batch.put(&key, value);
            match self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::Missing { key: key.clone() }],
                    batch,
                    durability: self.durability,
                })
                .await?
            {
                CompareAndWriteResult::Applied => {}
                CompareAndWriteResult::Conflict { .. }
                    if self.adapter.get(&key).await?.is_some() => {}
                CompareAndWriteResult::Conflict { .. } => {
                    return Err(OperatorAuthenticationStoreError::Conflict);
                }
            }
        }
        Ok(OperatorAuthenticationMigrationReport {
            version: MIGRATION_VERSION,
            completed: true,
        })
    }

    pub async fn get_entitlement(
        &self,
        identity_id: &DeveloperIdentityId,
    ) -> Result<Option<OperatorEntitlementRecord>, OperatorAuthenticationStoreError> {
        let value = self
            .read::<OperatorEntitlementRecord>(&ControlKeyspace::operator_entitlement_key(
                identity_id,
            )?)
            .await?;
        if let Some(record) = &value {
            record.validate()?;
            if record.developer_identity_id != *identity_id {
                return Err(OperatorAuthenticationStoreError::InvalidRecord);
            }
        }
        Ok(value)
    }

    pub async fn list_entitlements(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<OperatorEntitlementRecord>, OperatorAuthenticationStoreError> {
        self.scan_bounded(ControlKeyspace::operator_entitlements_range()?, limit)
            .await?
            .into_iter()
            .map(|entry| {
                let record: OperatorEntitlementRecord = decode_record(&entry.value)?;
                record.validate()?;
                Ok(record)
            })
            .collect()
    }

    pub async fn create_entitlement(
        &self,
        record: &OperatorEntitlementRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        record.validate()?;
        let key = ControlKeyspace::operator_entitlement_key(&record.developer_identity_id)?;
        self.create_record(key, record).await
    }

    pub async fn replace_entitlement(
        &self,
        previous: &OperatorEntitlementRecord,
        next: &OperatorEntitlementRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        if previous.developer_identity_id != next.developer_identity_id
            || previous.operator_id != next.operator_id
            || next.operator_epoch != previous.operator_epoch.saturating_add(1)
        {
            return Err(OperatorAuthenticationStoreError::ScopeMismatch);
        }
        next.validate()?;
        self.replace_record(
            ControlKeyspace::operator_entitlement_key(&previous.developer_identity_id)?,
            previous,
            next,
        )
        .await
    }

    pub async fn commit_entitlement_change(
        &self,
        previous: Option<&OperatorEntitlementRecord>,
        next: Option<&OperatorEntitlementRecord>,
        idempotency: &OperatorIdempotencyRecord,
        now_unix_seconds: u64,
    ) -> Result<usize, OperatorAuthenticationStoreError> {
        let identity_id = previous
            .map(|record| &record.developer_identity_id)
            .or_else(|| next.map(|record| &record.developer_identity_id))
            .ok_or(OperatorAuthenticationStoreError::InvalidRecord)?;
        if previous.is_some_and(|record| &record.developer_identity_id != identity_id)
            || next.is_some_and(|record| &record.developer_identity_id != identity_id)
            || idempotency.developer_identity_id != *identity_id
        {
            return Err(OperatorAuthenticationStoreError::ScopeMismatch);
        }
        if let Some(record) = next {
            record.validate()?;
        }
        idempotency.validate()?;
        let entitlement_key = ControlKeyspace::operator_entitlement_key(identity_id)?;
        let idempotency_key = ControlKeyspace::operator_idempotency_key(&idempotency.digest)?;
        let entitlement_condition = match previous {
            Some(record) => KeyCondition::ValueEquals {
                key: entitlement_key.clone(),
                value: encode_record(record)?,
            },
            None => KeyCondition::Missing {
                key: entitlement_key.clone(),
            },
        };
        let mut conditions = vec![
            entitlement_condition,
            KeyCondition::Missing {
                key: idempotency_key.clone(),
            },
        ];
        let mut batch = WriteBatch::new();
        if let Some(record) = next {
            batch.put(&entitlement_key, encode_record(record)?);
        } else {
            batch.delete(&entitlement_key);
        }
        batch.put(&idempotency_key, encode_record(idempotency)?);

        let mut revoked = 0;
        for entry in self
            .scan_all(ControlKeyspace::operator_sessions_range()?)
            .await?
        {
            match decode_operator_session(&entry.value)? {
                DecodedOperatorSession::Current(mut record)
                    if record.developer_identity_id == *identity_id
                        && record.revoked_at_unix_seconds.is_none()
                        && record.expires_at_unix_seconds > now_unix_seconds =>
                {
                    conditions.push(KeyCondition::ValueEquals {
                        key: entry.key.clone(),
                        value: entry.value,
                    });
                    record.revoke(now_unix_seconds)?;
                    batch.put(entry.key, encode_record(&record)?);
                    revoked += 1;
                }
                DecodedOperatorSession::Legacy(record)
                    if record.developer_identity_id == *identity_id =>
                {
                    conditions.push(KeyCondition::ValueEquals {
                        key: entry.key.clone(),
                        value: entry.value,
                    });
                    batch.delete(entry.key);
                    revoked += 1;
                }
                _ => {}
            }
        }
        self.apply(AtomicWrite {
            conditions,
            batch,
            durability: self.durability,
        })
        .await?;
        Ok(revoked)
    }

    pub async fn commit_bootstrap(
        &self,
        identity: &AuthenticationIdentityRecord,
        entitlement: &OperatorEntitlementRecord,
        idempotency: &OperatorIdempotencyRecord,
        _now_unix_seconds: u64,
    ) -> Result<usize, OperatorAuthenticationStoreError> {
        if identity.id() != entitlement.developer_identity_id()
            || identity.id() != &idempotency.developer_identity_id
            || identity.email_verified_at_unix_seconds().is_none()
            || identity.security_status() != AuthenticationSecurityStatus::Active
        {
            return Err(OperatorAuthenticationStoreError::ScopeMismatch);
        }
        entitlement.validate()?;
        idempotency.validate()?;
        let identity_key = ControlKeyspace::authentication_identity_key(identity.id())?;
        let entitlement_key = ControlKeyspace::operator_entitlement_key(identity.id())?;
        let idempotency_key = ControlKeyspace::operator_idempotency_key(&idempotency.digest)?;
        let conditions = vec![
            KeyCondition::ValueEquals {
                key: identity_key,
                value: encode_record(identity)?,
            },
            KeyCondition::Missing {
                key: entitlement_key.clone(),
            },
            KeyCondition::Missing {
                key: idempotency_key.clone(),
            },
        ];
        let mut batch = WriteBatch::new();
        batch.put(entitlement_key, encode_record(entitlement)?);
        batch.put(idempotency_key, encode_record(idempotency)?);
        self.apply(AtomicWrite {
            conditions,
            batch,
            durability: self.durability,
        })
        .await?;
        Ok(0)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn commit_bootstrap_developer_admission_repair(
        &self,
        identity: &AuthenticationIdentityRecord,
        previous_role: &DeveloperRoleRecord,
        next_role: &DeveloperRoleRecord,
        entitlement: &OperatorEntitlementRecord,
        bootstrap_provenance: &OperatorIdempotencyRecord,
        repair_idempotency: &OperatorIdempotencyRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        if identity.id() != previous_role.identity_id()
            || identity.id() != next_role.identity_id()
            || identity.id() != entitlement.developer_identity_id()
            || identity.id() != bootstrap_provenance.developer_identity_id()
            || identity.id() != repair_idempotency.developer_identity_id()
            || previous_role.status() != DeveloperIdentityStatus::Active
            || previous_role.review().is_some()
            || next_role.status() != DeveloperIdentityStatus::Waitlisted
            || next_role.review().is_some()
            || next_role.authorization_epoch()
                != previous_role.authorization_epoch().saturating_add(1)
            || bootstrap_provenance.audit_event_id() != "operator_bootstrap"
            || bootstrap_provenance.resulting_operator_epoch() != entitlement.operator_epoch()
            || repair_idempotency.audit_event_id() != "developer_admission_repair"
            || repair_idempotency.resulting_operator_epoch() != entitlement.operator_epoch()
        {
            return Err(OperatorAuthenticationStoreError::ScopeMismatch);
        }
        identity
            .validate()
            .map_err(|_| OperatorAuthenticationStoreError::InvalidRecord)?;
        previous_role
            .validate()
            .map_err(|_| OperatorAuthenticationStoreError::InvalidRecord)?;
        next_role
            .validate()
            .map_err(|_| OperatorAuthenticationStoreError::InvalidRecord)?;
        entitlement.validate()?;
        bootstrap_provenance.validate()?;
        repair_idempotency.validate()?;

        let identity_key = ControlKeyspace::authentication_identity_key(identity.id())?;
        let role_key = ControlKeyspace::developer_role_key(identity.id())?;
        let entitlement_key = ControlKeyspace::operator_entitlement_key(identity.id())?;
        let provenance_key =
            ControlKeyspace::operator_idempotency_key(bootstrap_provenance.digest())?;
        let repair_key = ControlKeyspace::operator_idempotency_key(repair_idempotency.digest())?;
        let previous_status_key = ControlKeyspace::developer_status_key(
            previous_role.status(),
            previous_role.created_at_unix_seconds(),
            identity.id(),
        )?;
        let next_status_key = ControlKeyspace::developer_status_key(
            next_role.status(),
            next_role.created_at_unix_seconds(),
            identity.id(),
        )?;
        let mut batch = WriteBatch::new();
        batch.put(&role_key, encode_record(next_role)?);
        batch.delete(&previous_status_key);
        batch.put(&next_status_key, encode_record(identity.id())?);
        batch.put(&repair_key, encode_record(repair_idempotency)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: identity_key,
                    value: encode_record(identity)?,
                },
                KeyCondition::ValueEquals {
                    key: role_key,
                    value: encode_record(previous_role)?,
                },
                KeyCondition::ValueEquals {
                    key: entitlement_key,
                    value: encode_record(entitlement)?,
                },
                KeyCondition::ValueEquals {
                    key: provenance_key,
                    value: encode_record(bootstrap_provenance)?,
                },
                KeyCondition::ValueEquals {
                    key: previous_status_key,
                    value: encode_record(identity.id())?,
                },
                KeyCondition::Missing {
                    key: next_status_key,
                },
                KeyCondition::Missing { key: repair_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn get_session(
        &self,
        digest: &str,
    ) -> Result<Option<OperatorSessionRecord>, OperatorAuthenticationStoreError> {
        if !valid_digest(digest) {
            return Err(OperatorAuthenticationStoreError::InvalidCredential);
        }
        let Some(value) = self
            .adapter
            .get(&ControlKeyspace::operator_session_key(digest)?)
            .await?
        else {
            return Ok(None);
        };
        if let Ok(record) = decode_record::<OperatorSessionRecord>(&value) {
            record.validate()?;
            if record.digest != digest {
                return Err(OperatorAuthenticationStoreError::InvalidRecord);
            }
            return Ok(Some(record));
        }
        let legacy: LegacyOperatorSessionRecordV1 = decode_record(&value)?;
        legacy.validate()?;
        if legacy.digest != digest {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        Ok(None)
    }

    pub async fn list_sessions(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<OperatorSessionRecord>, OperatorAuthenticationStoreError> {
        self.scan_bounded(ControlKeyspace::operator_sessions_range()?, limit)
            .await?
            .into_iter()
            .filter_map(|entry| match decode_operator_session(&entry.value) {
                Ok(DecodedOperatorSession::Current(record)) => Some(Ok(record)),
                Ok(DecodedOperatorSession::Legacy(_)) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub async fn create_session(
        &self,
        record: &OperatorSessionRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        record.validate()?;
        self.create_record(
            ControlKeyspace::operator_session_key(&record.digest)?,
            record,
        )
        .await
    }

    pub async fn replace_session(
        &self,
        previous: &OperatorSessionRecord,
        next: &OperatorSessionRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        if previous.digest != next.digest
            || previous.developer_identity_id != next.developer_identity_id
            || previous.operator_id != next.operator_id
        {
            return Err(OperatorAuthenticationStoreError::ScopeMismatch);
        }
        next.validate()?;
        self.replace_record(
            ControlKeyspace::operator_session_key(&previous.digest)?,
            previous,
            next,
        )
        .await
    }

    pub async fn get_attempt(
        &self,
        digest: &str,
    ) -> Result<Option<OperatorAttemptRecord>, OperatorAuthenticationStoreError> {
        if !valid_digest(digest) {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        let value = self
            .read::<OperatorAttemptRecord>(&ControlKeyspace::operator_attempt_key(digest)?)
            .await?;
        if let Some(record) = &value {
            record.validate()?;
        }
        Ok(value)
    }

    pub async fn list_attempts(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<OperatorAttemptRecord>, OperatorAuthenticationStoreError> {
        self.scan_bounded(ControlKeyspace::operator_attempts_range()?, limit)
            .await?
            .into_iter()
            .map(|entry| {
                let record: OperatorAttemptRecord = decode_record(&entry.value)?;
                record.validate()?;
                Ok(record)
            })
            .collect()
    }

    pub async fn create_attempt(
        &self,
        record: &OperatorAttemptRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        record.validate()?;
        self.create_record(
            ControlKeyspace::operator_attempt_key(&record.digest)?,
            record,
        )
        .await
    }

    pub async fn replace_attempt(
        &self,
        previous: &OperatorAttemptRecord,
        next: &OperatorAttemptRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        if previous.digest != next.digest {
            return Err(OperatorAuthenticationStoreError::ScopeMismatch);
        }
        next.validate()?;
        self.replace_record(
            ControlKeyspace::operator_attempt_key(&previous.digest)?,
            previous,
            next,
        )
        .await
    }

    pub async fn clear_attempt(
        &self,
        record: &OperatorAttemptRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        let key = ControlKeyspace::operator_attempt_key(&record.digest)?;
        let mut batch = WriteBatch::new();
        batch.delete(&key);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: encode_record(record)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn get_idempotency(
        &self,
        digest: &str,
    ) -> Result<Option<OperatorIdempotencyRecord>, OperatorAuthenticationStoreError> {
        if !valid_digest(digest) {
            return Err(OperatorAuthenticationStoreError::InvalidRecord);
        }
        let value = self
            .read::<OperatorIdempotencyRecord>(&ControlKeyspace::operator_idempotency_key(digest)?)
            .await?;
        if let Some(record) = &value {
            record.validate()?;
        }
        Ok(value)
    }

    pub async fn create_idempotency(
        &self,
        record: &OperatorIdempotencyRecord,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        record.validate()?;
        self.create_record(
            ControlKeyspace::operator_idempotency_key(&record.digest)?,
            record,
        )
        .await
    }

    pub async fn revoke_identity_sessions(
        &self,
        identity_id: &DeveloperIdentityId,
        now_unix_seconds: u64,
    ) -> Result<usize, OperatorAuthenticationStoreError> {
        let entries = self
            .scan_all(ControlKeyspace::operator_sessions_range()?)
            .await?;
        let mut conditions = Vec::new();
        let mut batch = WriteBatch::new();
        let mut revoked = 0;
        for entry in entries {
            match decode_operator_session(&entry.value)? {
                DecodedOperatorSession::Current(mut record)
                    if record.developer_identity_id == *identity_id
                        && record.revoked_at_unix_seconds.is_none()
                        && record.expires_at_unix_seconds > now_unix_seconds =>
                {
                    conditions.push(KeyCondition::ValueEquals {
                        key: entry.key.clone(),
                        value: entry.value,
                    });
                    record.revoke(now_unix_seconds)?;
                    batch.put(entry.key, encode_record(&record)?);
                    revoked += 1;
                }
                DecodedOperatorSession::Legacy(record)
                    if record.developer_identity_id == *identity_id =>
                {
                    conditions.push(KeyCondition::ValueEquals {
                        key: entry.key.clone(),
                        value: entry.value,
                    });
                    batch.delete(entry.key);
                    revoked += 1;
                }
                _ => {}
            }
        }
        if !batch.is_empty() {
            self.apply(AtomicWrite {
                conditions,
                batch,
                durability: self.durability,
            })
            .await?;
        }
        Ok(revoked)
    }

    pub async fn cleanup_expired(
        &self,
        now_unix_seconds: u64,
        limit_per_domain: NonZeroUsize,
    ) -> Result<OperatorAuthenticationCleanupReport, OperatorAuthenticationStoreError> {
        if limit_per_domain.get() > 1_000 {
            return Err(OperatorAuthenticationStoreError::LimitExceeded);
        }
        let mut report = OperatorAuthenticationCleanupReport::default();
        let mut batch = WriteBatch::new();
        for entry in self
            .scan_all(ControlKeyspace::operator_sessions_range()?)
            .await?
        {
            if report.sessions >= limit_per_domain.get() {
                break;
            }
            let delete = match decode_operator_session(&entry.value)? {
                DecodedOperatorSession::Current(record) => {
                    record.expires_at_unix_seconds <= now_unix_seconds
                }
                DecodedOperatorSession::Legacy(_) => true,
            };
            if delete {
                batch.delete(entry.key);
                report.sessions += 1;
            }
        }
        for entry in self
            .scan_all(ControlKeyspace::operator_attempts_range()?)
            .await?
        {
            if report.attempts >= limit_per_domain.get() {
                break;
            }
            let record: OperatorAttemptRecord = decode_record(&entry.value)?;
            record.validate()?;
            if record.expires_at_unix_seconds <= now_unix_seconds {
                batch.delete(entry.key);
                report.attempts += 1;
            }
        }
        for entry in self
            .scan_all(ControlKeyspace::operator_idempotency_range()?)
            .await?
        {
            if report.idempotency >= limit_per_domain.get() {
                break;
            }
            let record: OperatorIdempotencyRecord = decode_record(&entry.value)?;
            record.validate()?;
            if record.expires_at_unix_seconds <= now_unix_seconds {
                batch.delete(entry.key);
                report.idempotency += 1;
            }
        }
        if !batch.is_empty() {
            self.adapter.write(batch, self.durability).await?;
        }
        Ok(report)
    }

    pub async fn health_snapshot(
        &self,
        now_unix_seconds: u64,
    ) -> Result<OperatorAuthenticationHealthSnapshot, OperatorAuthenticationStoreError> {
        let mut snapshot = OperatorAuthenticationHealthSnapshot::default();
        for entry in self
            .scan_all(ControlKeyspace::operator_sessions_range()?)
            .await?
        {
            if let DecodedOperatorSession::Current(record) = decode_operator_session(&entry.value)?
            {
                if record.revoked_at_unix_seconds.is_some() {
                    snapshot.revoked_sessions += 1;
                } else if record.expires_at_unix_seconds > now_unix_seconds {
                    snapshot.active_sessions += 1;
                }
            }
        }
        snapshot.entitlements = self
            .scan_all(ControlKeyspace::operator_entitlements_range()?)
            .await?
            .len();
        snapshot.attempt_records = self
            .scan_all(ControlKeyspace::operator_attempts_range()?)
            .await?
            .len();
        Ok(snapshot)
    }

    async fn create_record<T: Serialize>(
        &self,
        key: Vec<u8>,
        record: &T,
    ) -> Result<(), OperatorAuthenticationStoreError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, encode_record(record)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
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
    ) -> Result<(), OperatorAuthenticationStoreError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, encode_record(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: encode_record(previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn read<T: DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<Option<T>, OperatorAuthenticationStoreError> {
        self.adapter
            .get(key)
            .await?
            .map(|value| decode_record(&value))
            .transpose()
    }

    async fn scan_all(
        &self,
        range: mako_storage::KeyRange,
    ) -> Result<Vec<mako_storage::KeyValue>, OperatorAuthenticationStoreError> {
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                range,
                ScanDirection::Forward,
                NonZeroUsize::new(MAX_SCAN_RECORDS).expect("constant is positive"),
            ))
            .await?;
        if entries.len() == MAX_SCAN_RECORDS {
            return Err(OperatorAuthenticationStoreError::LimitExceeded);
        }
        Ok(entries)
    }

    async fn scan_bounded(
        &self,
        range: mako_storage::KeyRange,
        limit: NonZeroUsize,
    ) -> Result<Vec<mako_storage::KeyValue>, OperatorAuthenticationStoreError> {
        if limit.get() > 100 {
            return Err(OperatorAuthenticationStoreError::LimitExceeded);
        }
        self.adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await
            .map_err(Into::into)
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), OperatorAuthenticationStoreError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => {
                Err(OperatorAuthenticationStoreError::Conflict)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorAuthenticationConfig {
    pub enabled: bool,
    pub session_lifetime_seconds: u64,
    pub mutation_freshness_seconds: u64,
    pub attempt_window_seconds: u64,
    pub source_attempts_per_window: u32,
    pub identity_attempts_per_window: u32,
    pub base_backoff_seconds: u64,
    pub maximum_backoff_seconds: u64,
    pub cookie_name: String,
}

impl Default for OperatorAuthenticationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            session_lifetime_seconds: MAX_SESSION_SECONDS,
            mutation_freshness_seconds: 5 * 60,
            attempt_window_seconds: 15 * 60,
            source_attempts_per_window: 20,
            identity_attempts_per_window: 10,
            base_backoff_seconds: 1,
            maximum_backoff_seconds: 5 * 60,
            cookie_name: "__Secure-mako_operator".to_owned(),
        }
    }
}

impl OperatorAuthenticationConfig {
    pub fn validate(&self) -> Result<(), OperatorPasswordAuthenticationError> {
        if self.session_lifetime_seconds == 0
            || self.session_lifetime_seconds > MAX_SESSION_SECONDS
            || self.mutation_freshness_seconds == 0
            || self.mutation_freshness_seconds > 5 * 60
            || self.attempt_window_seconds == 0
            || self.attempt_window_seconds > MAX_ATTEMPT_SECONDS
            || self.source_attempts_per_window == 0
            || self.identity_attempts_per_window == 0
            || self.base_backoff_seconds == 0
            || self.maximum_backoff_seconds < self.base_backoff_seconds
            || self.maximum_backoff_seconds > self.attempt_window_seconds
            || self.cookie_name != "__Secure-mako_operator"
        {
            return Err(OperatorPasswordAuthenticationError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorAuthenticationAuditAction {
    SignIn,
    SignOut,
    PasswordStepUp,
    SessionRevocation,
    EntitlementChange,
    DeveloperRoleRepair,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorAuthenticationAuditOutcome {
    Allowed,
    Denied,
    Throttled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorAuthenticationAuditEvent {
    pub action: OperatorAuthenticationAuditAction,
    pub outcome: OperatorAuthenticationAuditOutcome,
    pub operator_id: Option<OperatorId>,
    pub classification: &'static str,
    pub at_unix_seconds: u64,
}

pub trait OperatorAuthenticationAuditSink: Send + Sync {
    fn record(&self, event: OperatorAuthenticationAuditEvent);
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorEntitlementChangeKind {
    Grant,
    Replace,
    Revoke,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorEntitlementChangeResult {
    pub entitlement: Option<OperatorEntitlementRecord>,
    pub resulting_operator_epoch: u64,
    pub revoked_sessions: usize,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct OperatorEntitlementService {
    store: OperatorAuthenticationStore,
    developers: DeveloperRegistrationService,
    audit: Arc<dyn OperatorAuthenticationAuditSink>,
}

impl fmt::Debug for OperatorEntitlementService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorEntitlementService")
            .finish_non_exhaustive()
    }
}

impl OperatorEntitlementService {
    #[must_use]
    pub fn new(
        store: OperatorAuthenticationStore,
        developers: DeveloperRegistrationService,
        audit: Arc<dyn OperatorAuthenticationAuditSink>,
    ) -> Self {
        Self {
            store,
            developers,
            audit,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_change(
        &self,
        kind: OperatorEntitlementChangeKind,
        identity_id: &DeveloperIdentityId,
        permissions: impl IntoIterator<Item = OperatorPermission>,
        private_reason: &str,
        request_id: &str,
        idempotency_key: &str,
        now_unix_seconds: u64,
        idempotency_retention_seconds: u64,
    ) -> Result<OperatorEntitlementChangeResult, OperatorPasswordAuthenticationError> {
        let permissions = permissions.into_iter().collect::<BTreeSet<_>>();
        let operation_digest =
            entitlement_request_digest(kind, identity_id, &permissions, private_reason);
        self.apply_planned_change(
            kind,
            identity_id,
            permissions,
            private_reason,
            request_id,
            idempotency_key,
            &operation_digest,
            now_unix_seconds,
            idempotency_retention_seconds,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_planned_change(
        &self,
        kind: OperatorEntitlementChangeKind,
        identity_id: &DeveloperIdentityId,
        permissions: impl IntoIterator<Item = OperatorPermission>,
        private_reason: &str,
        request_id: &str,
        idempotency_key: &str,
        operation_digest: &str,
        now_unix_seconds: u64,
        idempotency_retention_seconds: u64,
    ) -> Result<OperatorEntitlementChangeResult, OperatorPasswordAuthenticationError> {
        if !valid_text(private_reason, 8, 1_024)
            || !valid_text(request_id, 8, 256)
            || !valid_text(idempotency_key, 16, 200)
            || !valid_digest(operation_digest)
            || idempotency_retention_seconds == 0
            || idempotency_retention_seconds > 365 * 24 * 60 * 60
        {
            return Err(OperatorPasswordAuthenticationError::InvalidConfiguration);
        }
        let permissions = permissions.into_iter().collect::<BTreeSet<_>>();
        if matches!(kind, OperatorEntitlementChangeKind::Revoke) != permissions.is_empty() {
            return Err(OperatorPasswordAuthenticationError::InvalidConfiguration);
        }
        let request_digest =
            entitlement_request_digest(kind, identity_id, &permissions, private_reason);
        let idempotency_digest = self
            .store
            .authentication_key
            .idempotency_digest(idempotency_key);
        if let Some(existing) = self.store.get_idempotency(&idempotency_digest).await? {
            if existing.request_digest != request_digest
                || existing.developer_identity_id != *identity_id
            {
                return Err(OperatorPasswordAuthenticationError::Conflict);
            }
            return Ok(OperatorEntitlementChangeResult {
                entitlement: self.store.get_entitlement(identity_id).await?,
                resulting_operator_epoch: existing.resulting_operator_epoch,
                revoked_sessions: 0,
                replayed: true,
            });
        }
        let identity = self
            .developers
            .store()
            .get_authentication_identity(identity_id)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::NotFound)?;
        if identity.security_status() != AuthenticationSecurityStatus::Active
            || identity.email_verified_at_unix_seconds().is_none()
        {
            return Err(OperatorPasswordAuthenticationError::Ineligible);
        }
        let previous = self.store.get_entitlement(identity_id).await?;
        let next = match (kind, previous.as_ref()) {
            (OperatorEntitlementChangeKind::Grant, None) => Some(OperatorEntitlementRecord::new(
                identity_id.clone(),
                self.store
                    .authentication_key
                    .stable_operator_id(identity_id),
                1,
                permissions,
                private_reason,
                request_id,
                now_unix_seconds,
            )?),
            (OperatorEntitlementChangeKind::Replace, Some(previous)) => Some(
                previous.replacement(permissions, private_reason, request_id, now_unix_seconds)?,
            ),
            (OperatorEntitlementChangeKind::Revoke, Some(_)) => None,
            _ => return Err(OperatorPasswordAuthenticationError::Conflict),
        };
        let resulting_operator_epoch = next.as_ref().map_or_else(
            || {
                previous
                    .as_ref()
                    .map_or(1, |value| value.operator_epoch + 1)
            },
            |value| value.operator_epoch,
        );
        let idempotency = OperatorIdempotencyRecord::new(
            idempotency_digest,
            request_digest.clone(),
            operation_digest,
            identity_id.clone(),
            resulting_operator_epoch,
            request_id,
            now_unix_seconds,
            now_unix_seconds
                .checked_add(idempotency_retention_seconds)
                .ok_or(OperatorPasswordAuthenticationError::InvalidConfiguration)?,
        )?;
        let revoked_sessions = self
            .store
            .commit_entitlement_change(
                previous.as_ref(),
                next.as_ref(),
                &idempotency,
                now_unix_seconds,
            )
            .await?;
        self.audit.record(OperatorAuthenticationAuditEvent {
            action: OperatorAuthenticationAuditAction::EntitlementChange,
            outcome: OperatorAuthenticationAuditOutcome::Allowed,
            operator_id: next
                .as_ref()
                .map(|record| record.operator_id.clone())
                .or_else(|| previous.as_ref().map(|record| record.operator_id.clone())),
            classification: match kind {
                OperatorEntitlementChangeKind::Grant => "grant",
                OperatorEntitlementChangeKind::Replace => "replace",
                OperatorEntitlementChangeKind::Revoke => "revoke",
            },
            at_unix_seconds: now_unix_seconds,
        });
        Ok(OperatorEntitlementChangeResult {
            entitlement: next,
            resulting_operator_epoch,
            revoked_sessions,
            replayed: false,
        })
    }
}

fn entitlement_request_digest(
    kind: OperatorEntitlementChangeKind,
    identity_id: &DeveloperIdentityId,
    permissions: &BTreeSet<OperatorPermission>,
    private_reason: &str,
) -> String {
    let permission_names = permissions
        .iter()
        .map(|permission| format!("{permission:?}"))
        .collect::<Vec<_>>()
        .join(",");
    blake3::hash(
        format!(
            "operator-entitlement-v1\0{kind:?}\0{}\0{permission_names}\0{private_reason}",
            identity_id.as_str()
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string()
}

#[derive(Clone, Eq, PartialEq)]
pub struct OperatorEntitlementAdminInput {
    pub kind: OperatorEntitlementChangeKind,
    pub target_email: String,
    pub permissions: BTreeSet<OperatorPermission>,
    pub private_reason: String,
    pub environment_binding: String,
    pub idempotency_key: String,
}

impl fmt::Debug for OperatorEntitlementAdminInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorEntitlementAdminInput")
            .field("kind", &self.kind)
            .field("permissions", &self.permissions)
            .field("environment_binding", &self.environment_binding)
            .field("private_fields", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorEntitlementAdminPlan {
    pub environment_binding: String,
    pub developer_identity_id: DeveloperIdentityId,
    pub email_digest: String,
    pub permission_digest: String,
    pub request_digest: String,
    pub operation_digest: String,
    pub developer_status_before: Option<DeveloperIdentityStatus>,
    pub developer_status_after: Option<DeveloperIdentityStatus>,
    pub operator_status_before: &'static str,
    pub operator_status_after: &'static str,
    pub typed_confirmation: String,
    pub replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorEntitlementAdminResult {
    pub environment_binding: String,
    pub developer_identity_id: DeveloperIdentityId,
    pub permission_digest: String,
    pub operation_digest: String,
    pub developer_status_before: Option<DeveloperIdentityStatus>,
    pub developer_status_after: Option<DeveloperIdentityStatus>,
    pub operator_status_before: &'static str,
    pub operator_status_after: &'static str,
    pub operator_epoch: u64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct OperatorEntitlementAdminService {
    store: OperatorAuthenticationStore,
    developers: DeveloperRegistrationService,
    entitlements: OperatorEntitlementService,
}

impl fmt::Debug for OperatorEntitlementAdminService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorEntitlementAdminService")
            .finish_non_exhaustive()
    }
}

impl OperatorEntitlementAdminService {
    #[must_use]
    pub fn new(
        store: OperatorAuthenticationStore,
        developers: DeveloperRegistrationService,
        audit: Arc<dyn OperatorAuthenticationAuditSink>,
    ) -> Self {
        let entitlements =
            OperatorEntitlementService::new(store.clone(), developers.clone(), audit);
        Self {
            store,
            developers,
            entitlements,
        }
    }

    pub async fn plan(
        &self,
        input: &OperatorEntitlementAdminInput,
    ) -> Result<OperatorEntitlementAdminPlan, OperatorPasswordAuthenticationError> {
        validate_entitlement_admin_input(input)?;
        let email = mako_identity::NormalizedEmail::parse(&input.target_email)
            .map_err(|_| OperatorPasswordAuthenticationError::Ineligible)?;
        let identity = self
            .developers
            .store()
            .get_authentication_identity_by_email(&email)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::NotFound)?;
        if identity.security_status() != AuthenticationSecurityStatus::Active
            || identity.email_verified_at_unix_seconds().is_none()
        {
            return Err(OperatorPasswordAuthenticationError::Ineligible);
        }
        let developer_status = self
            .developers
            .store()
            .get_developer_role(identity.id())
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .map(|role| role.status());
        let email_digest = self.developers.store().lookup_key().email_digest(&email);
        let target_permission_digest = permission_digest(&input.permissions);
        let request_digest = entitlement_request_digest(
            input.kind,
            identity.id(),
            &input.permissions,
            &input.private_reason,
        );
        let idempotency_digest = self
            .store
            .authentication_key
            .idempotency_digest(&input.idempotency_key);
        if let Some(existing) = self.store.get_idempotency(&idempotency_digest).await? {
            if existing.request_digest != request_digest
                || existing.developer_identity_id != *identity.id()
            {
                return Err(OperatorPasswordAuthenticationError::Conflict);
            }
            let current = self.store.get_entitlement(identity.id()).await?;
            let state_matches = match input.kind {
                OperatorEntitlementChangeKind::Grant | OperatorEntitlementChangeKind::Replace => {
                    current.as_ref().is_some_and(|value| {
                        value.permissions == input.permissions
                            && value.operator_epoch == existing.resulting_operator_epoch
                    })
                }
                OperatorEntitlementChangeKind::Revoke => current.is_none(),
            };
            if !state_matches {
                return Err(OperatorPasswordAuthenticationError::Conflict);
            }
            return Ok(entitlement_admin_plan(
                input,
                identity.id(),
                email_digest,
                target_permission_digest,
                request_digest,
                existing.operation_digest.clone(),
                developer_status,
                current.is_some(),
                true,
            ));
        }
        let current = self.store.get_entitlement(identity.id()).await?;
        match input.kind {
            OperatorEntitlementChangeKind::Grant if current.is_none() => {}
            OperatorEntitlementChangeKind::Replace | OperatorEntitlementChangeKind::Revoke
                if current.is_some() => {}
            _ => return Err(OperatorPasswordAuthenticationError::Conflict),
        }
        let removes_recoverable_administrator = current.as_ref().is_some_and(|record| {
            record
                .permissions()
                .contains(&OperatorPermission::SecurityManage)
                && (matches!(input.kind, OperatorEntitlementChangeKind::Revoke)
                    || !input
                        .permissions
                        .contains(&OperatorPermission::SecurityManage))
        });
        if removes_recoverable_administrator {
            let administrators = self
                .store
                .list_entitlements(NonZeroUsize::new(100).expect("positive"))
                .await?
                .into_iter()
                .filter(|record| {
                    record
                        .permissions()
                        .contains(&OperatorPermission::SecurityManage)
                })
                .count();
            if administrators <= 1 {
                return Err(OperatorPasswordAuthenticationError::LastRecoverableAdministrator);
            }
        }
        let current_epoch = current.as_ref().map_or(0, |value| value.operator_epoch);
        let current_permissions = current.as_ref().map_or_else(
            || permission_digest(&BTreeSet::new()),
            |value| permission_digest(&value.permissions),
        );
        let operation_digest = blake3::hash(
            format!(
                "operator-entitlement-plan-v1\0{}\0{request_digest}\0{}\0{}\0{current_epoch}\0{current_permissions}",
                input.environment_binding,
                identity.credential_epoch(),
                identity.updated_at_unix_seconds(),
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        Ok(entitlement_admin_plan(
            input,
            identity.id(),
            email_digest,
            target_permission_digest,
            request_digest,
            operation_digest,
            developer_status,
            current.is_some(),
            false,
        ))
    }

    pub async fn apply(
        &self,
        input: &OperatorEntitlementAdminInput,
        typed_confirmation: &str,
        request_id: &str,
        now_unix_seconds: u64,
    ) -> Result<OperatorEntitlementAdminResult, OperatorPasswordAuthenticationError> {
        let plan = self.plan(input).await?;
        if typed_confirmation != plan.typed_confirmation {
            return Err(OperatorPasswordAuthenticationError::ConfirmationMismatch);
        }
        let result = self
            .entitlements
            .apply_planned_change(
                input.kind,
                &plan.developer_identity_id,
                input.permissions.iter().copied(),
                &input.private_reason,
                request_id,
                &input.idempotency_key,
                &plan.operation_digest,
                now_unix_seconds,
                365 * 24 * 60 * 60,
            )
            .await?;
        Ok(OperatorEntitlementAdminResult {
            environment_binding: plan.environment_binding,
            developer_identity_id: plan.developer_identity_id,
            permission_digest: plan.permission_digest,
            operation_digest: plan.operation_digest,
            developer_status_before: plan.developer_status_before,
            developer_status_after: plan.developer_status_after,
            operator_status_before: plan.operator_status_before,
            operator_status_after: plan.operator_status_after,
            operator_epoch: result.resulting_operator_epoch,
            replayed: result.replayed,
        })
    }
}

fn validate_entitlement_admin_input(
    input: &OperatorEntitlementAdminInput,
) -> Result<(), OperatorPasswordAuthenticationError> {
    let permission_shape_valid = match input.kind {
        OperatorEntitlementChangeKind::Grant | OperatorEntitlementChangeKind::Replace => {
            !input.permissions.is_empty() && input.permissions.len() <= 17
        }
        OperatorEntitlementChangeKind::Revoke => input.permissions.is_empty(),
    };
    if !permission_shape_valid
        || !valid_text(&input.private_reason, 8, 1_024)
        || !valid_text(&input.environment_binding, 3, 256)
        || !valid_text(&input.idempotency_key, 16, 200)
        || input.target_email.len() > 320
    {
        return Err(OperatorPasswordAuthenticationError::InvalidConfiguration);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn entitlement_admin_plan(
    input: &OperatorEntitlementAdminInput,
    identity_id: &DeveloperIdentityId,
    email_digest: String,
    permission_digest: String,
    request_digest: String,
    operation_digest: String,
    developer_status: Option<DeveloperIdentityStatus>,
    operator_active_before: bool,
    replayed: bool,
) -> OperatorEntitlementAdminPlan {
    let environment_digest = blake3::hash(input.environment_binding.as_bytes())
        .to_hex()
        .to_string();
    let operation = match input.kind {
        OperatorEntitlementChangeKind::Grant => "GRANT",
        OperatorEntitlementChangeKind::Replace => "REPLACE",
        OperatorEntitlementChangeKind::Revoke => "REVOKE",
    };
    let operator_active_after = !matches!(input.kind, OperatorEntitlementChangeKind::Revoke);
    let typed_confirmation = format!(
        "APPLY_OPERATOR_ENTITLEMENT:{operation}:{environment_digest}:{}:{email_digest}:{permission_digest}:{operation_digest}",
        identity_id.as_str(),
    );
    OperatorEntitlementAdminPlan {
        environment_binding: input.environment_binding.clone(),
        developer_identity_id: identity_id.clone(),
        email_digest,
        permission_digest,
        request_digest,
        operation_digest,
        developer_status_before: developer_status,
        developer_status_after: developer_status,
        operator_status_before: if operator_active_before {
            "active"
        } else {
            "absent"
        },
        operator_status_after: if operator_active_after {
            "active"
        } else {
            "absent"
        },
        typed_confirmation,
        replayed,
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct OperatorBootstrapInput {
    pub target_email: String,
    pub permissions: BTreeSet<OperatorPermission>,
    pub private_reason: String,
    pub environment_binding: String,
    pub idempotency_key: String,
    pub activate_waitlisted: bool,
}

impl fmt::Debug for OperatorBootstrapInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorBootstrapInput")
            .field("permissions", &self.permissions)
            .field("environment_binding", &self.environment_binding)
            .field("activate_waitlisted", &self.activate_waitlisted)
            .field("private_fields", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorBootstrapPlan {
    pub environment_binding: String,
    pub developer_identity_id: DeveloperIdentityId,
    pub email_digest: String,
    pub permission_digest: String,
    pub request_digest: String,
    pub operation_digest: String,
    pub lifecycle_result: &'static str,
    pub developer_status_before: Option<DeveloperIdentityStatus>,
    pub developer_status_after: Option<DeveloperIdentityStatus>,
    pub operator_status_before: &'static str,
    pub operator_status_after: &'static str,
    pub typed_confirmation: String,
    pub replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorBootstrapResult {
    pub environment_binding: String,
    pub developer_identity_id: DeveloperIdentityId,
    pub permission_digest: String,
    pub operation_digest: String,
    pub lifecycle_result: &'static str,
    pub developer_status_before: Option<DeveloperIdentityStatus>,
    pub developer_status_after: Option<DeveloperIdentityStatus>,
    pub operator_status_before: &'static str,
    pub operator_status_after: &'static str,
    pub operator_epoch: u64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct OperatorBootstrapService {
    store: OperatorAuthenticationStore,
    developers: DeveloperRegistrationService,
    audit: Arc<dyn OperatorAuthenticationAuditSink>,
}

impl fmt::Debug for OperatorBootstrapService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorBootstrapService")
            .finish_non_exhaustive()
    }
}

impl OperatorBootstrapService {
    #[must_use]
    pub fn new(
        store: OperatorAuthenticationStore,
        developers: DeveloperRegistrationService,
        audit: Arc<dyn OperatorAuthenticationAuditSink>,
    ) -> Self {
        Self {
            store,
            developers,
            audit,
        }
    }

    pub async fn plan(
        &self,
        input: &OperatorBootstrapInput,
    ) -> Result<OperatorBootstrapPlan, OperatorPasswordAuthenticationError> {
        validate_bootstrap_input(input)?;
        let email = mako_identity::NormalizedEmail::parse(&input.target_email)
            .map_err(|_| OperatorPasswordAuthenticationError::Ineligible)?;
        let identity = self
            .developers
            .store()
            .get_authentication_identity_by_email(&email)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::NotFound)?;
        if input.activate_waitlisted
            || identity.email_verified_at_unix_seconds().is_none()
            || identity.security_status() != AuthenticationSecurityStatus::Active
        {
            return Err(OperatorPasswordAuthenticationError::Ineligible);
        }
        let developer_status = self
            .developers
            .store()
            .get_developer_role(identity.id())
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .map(|role| role.status());
        let email_digest = self.developers.store().lookup_key().email_digest(&email);
        let permission_digest = permission_digest(&input.permissions);
        let request_digest =
            bootstrap_request_digest(input, identity.id(), &email_digest, &permission_digest);
        let idempotency_digest = self
            .store
            .authentication_key
            .idempotency_digest(&input.idempotency_key);
        if let Some(existing) = self.store.get_idempotency(&idempotency_digest).await? {
            if existing.request_digest != request_digest
                || existing.developer_identity_id != *identity.id()
            {
                return Err(OperatorPasswordAuthenticationError::Conflict);
            }
            let entitlement = self
                .store
                .get_entitlement(identity.id())
                .await?
                .ok_or(OperatorPasswordAuthenticationError::Conflict)?;
            if entitlement.permissions != input.permissions
                || entitlement.operator_epoch != existing.resulting_operator_epoch
            {
                return Err(OperatorPasswordAuthenticationError::Conflict);
            }
            return Ok(bootstrap_plan(
                input,
                identity.id(),
                email_digest,
                permission_digest,
                request_digest,
                existing.operation_digest.clone(),
                "unchanged",
                developer_status,
                true,
                true,
            ));
        }
        if self.store.get_entitlement(identity.id()).await?.is_some() {
            return Err(OperatorPasswordAuthenticationError::Conflict);
        }
        let operation_digest = blake3::hash(
            format!(
                "operator-bootstrap-plan-v2\0{request_digest}\0{}\0{}",
                identity.credential_epoch(),
                identity.updated_at_unix_seconds()
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        Ok(bootstrap_plan(
            input,
            identity.id(),
            email_digest,
            permission_digest,
            request_digest,
            operation_digest,
            "unchanged",
            developer_status,
            false,
            false,
        ))
    }

    pub async fn apply(
        &self,
        input: &OperatorBootstrapInput,
        typed_confirmation: &str,
        now_unix_seconds: u64,
    ) -> Result<OperatorBootstrapResult, OperatorPasswordAuthenticationError> {
        let plan = self.plan(input).await?;
        if typed_confirmation != plan.typed_confirmation {
            return Err(OperatorPasswordAuthenticationError::ConfirmationMismatch);
        }
        if plan.replayed {
            let entitlement = self
                .store
                .get_entitlement(&plan.developer_identity_id)
                .await?
                .ok_or(OperatorPasswordAuthenticationError::Conflict)?;
            return Ok(OperatorBootstrapResult {
                environment_binding: plan.environment_binding,
                developer_identity_id: plan.developer_identity_id,
                permission_digest: plan.permission_digest,
                operation_digest: plan.operation_digest,
                lifecycle_result: "unchanged",
                developer_status_before: plan.developer_status_before,
                developer_status_after: plan.developer_status_after,
                operator_status_before: plan.operator_status_before,
                operator_status_after: plan.operator_status_after,
                operator_epoch: entitlement.operator_epoch,
                replayed: true,
            });
        }
        let identity = self
            .developers
            .store()
            .get_authentication_identity(&plan.developer_identity_id)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::NotFound)?;
        let entitlement = OperatorEntitlementRecord::new(
            plan.developer_identity_id.clone(),
            self.store
                .authentication_key
                .stable_operator_id(&plan.developer_identity_id),
            1,
            input.permissions.iter().copied(),
            &input.private_reason,
            "operator_bootstrap",
            now_unix_seconds,
        )?;
        let idempotency = OperatorIdempotencyRecord::new(
            self.store
                .authentication_key
                .idempotency_digest(&input.idempotency_key),
            plan.request_digest.clone(),
            plan.operation_digest.clone(),
            plan.developer_identity_id.clone(),
            1,
            "operator_bootstrap",
            now_unix_seconds,
            now_unix_seconds
                .checked_add(365 * 24 * 60 * 60)
                .ok_or(OperatorPasswordAuthenticationError::InvalidConfiguration)?,
        )?;
        self.store
            .commit_bootstrap(&identity, &entitlement, &idempotency, now_unix_seconds)
            .await?;
        self.audit.record(OperatorAuthenticationAuditEvent {
            action: OperatorAuthenticationAuditAction::EntitlementChange,
            outcome: OperatorAuthenticationAuditOutcome::Allowed,
            operator_id: Some(entitlement.operator_id.clone()),
            classification: "initial_bootstrap",
            at_unix_seconds: now_unix_seconds,
        });
        Ok(OperatorBootstrapResult {
            environment_binding: plan.environment_binding,
            developer_identity_id: plan.developer_identity_id,
            permission_digest: plan.permission_digest,
            operation_digest: plan.operation_digest,
            lifecycle_result: plan.lifecycle_result,
            developer_status_before: plan.developer_status_before,
            developer_status_after: plan.developer_status_after,
            operator_status_before: plan.operator_status_before,
            operator_status_after: plan.operator_status_after,
            operator_epoch: 1,
            replayed: false,
        })
    }
}

fn validate_bootstrap_input(
    input: &OperatorBootstrapInput,
) -> Result<(), OperatorPasswordAuthenticationError> {
    if input.permissions.is_empty()
        || input.permissions.len() > 17
        || !valid_text(&input.private_reason, 8, 1_024)
        || !valid_text(&input.environment_binding, 3, 256)
        || !valid_text(&input.idempotency_key, 16, 200)
        || input.target_email.len() > 320
        || input.activate_waitlisted
    {
        return Err(OperatorPasswordAuthenticationError::InvalidConfiguration);
    }
    Ok(())
}

fn permission_digest(permissions: &BTreeSet<OperatorPermission>) -> String {
    let value = permissions
        .iter()
        .map(|permission| format!("{permission:?}"))
        .collect::<Vec<_>>()
        .join(",");
    blake3::hash(value.as_bytes()).to_hex().to_string()
}

fn bootstrap_request_digest(
    input: &OperatorBootstrapInput,
    identity_id: &DeveloperIdentityId,
    email_digest: &str,
    permission_digest: &str,
) -> String {
    let reason_digest = blake3::hash(input.private_reason.as_bytes()).to_hex();
    blake3::hash(
        format!(
            "operator-bootstrap-request-v1\0{}\0{}\0{email_digest}\0{permission_digest}\0{reason_digest}\0{}\0{}",
            input.environment_binding,
            identity_id.as_str(),
            input.idempotency_key,
            input.activate_waitlisted
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string()
}

#[allow(clippy::too_many_arguments)]
fn bootstrap_plan(
    input: &OperatorBootstrapInput,
    identity_id: &DeveloperIdentityId,
    email_digest: String,
    permission_digest: String,
    request_digest: String,
    operation_digest: String,
    lifecycle_result: &'static str,
    developer_status: Option<DeveloperIdentityStatus>,
    operator_active_before: bool,
    replayed: bool,
) -> OperatorBootstrapPlan {
    let environment_digest = blake3::hash(input.environment_binding.as_bytes())
        .to_hex()
        .to_string();
    let typed_confirmation = format!(
        "APPLY_OPERATOR_BOOTSTRAP:{environment_digest}:{}:{email_digest}:{permission_digest}:{operation_digest}",
        identity_id.as_str()
    );
    OperatorBootstrapPlan {
        environment_binding: input.environment_binding.clone(),
        developer_identity_id: identity_id.clone(),
        email_digest,
        permission_digest,
        request_digest,
        operation_digest,
        lifecycle_result,
        developer_status_before: developer_status,
        developer_status_after: developer_status,
        operator_status_before: if operator_active_before {
            "active"
        } else {
            "absent"
        },
        operator_status_after: "active",
        typed_confirmation,
        replayed,
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct BootstrapDeveloperAdmissionRepairInput {
    pub target_email: String,
    pub private_reason: String,
    pub environment_binding: String,
    pub idempotency_key: String,
    pub prior_bootstrap_idempotency_key: String,
}

impl fmt::Debug for BootstrapDeveloperAdmissionRepairInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BootstrapDeveloperAdmissionRepairInput")
            .field("environment_binding", &self.environment_binding)
            .field("private_fields", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapDeveloperAdmissionRepairPlan {
    pub environment_binding: String,
    pub developer_identity_id: DeveloperIdentityId,
    pub email_digest: String,
    pub permission_digest: String,
    pub request_digest: String,
    pub operation_digest: String,
    pub developer_status_before: DeveloperIdentityStatus,
    pub developer_status_after: DeveloperIdentityStatus,
    pub operator_status_before: &'static str,
    pub operator_status_after: &'static str,
    pub typed_confirmation: String,
    pub replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapDeveloperAdmissionRepairResult {
    pub environment_binding: String,
    pub developer_identity_id: DeveloperIdentityId,
    pub permission_digest: String,
    pub operation_digest: String,
    pub developer_status_before: DeveloperIdentityStatus,
    pub developer_status_after: DeveloperIdentityStatus,
    pub operator_status_before: &'static str,
    pub operator_status_after: &'static str,
    pub operator_epoch: u64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct BootstrapDeveloperAdmissionRepairService {
    store: OperatorAuthenticationStore,
    developers: DeveloperRegistrationService,
    audit: Arc<dyn OperatorAuthenticationAuditSink>,
}

impl fmt::Debug for BootstrapDeveloperAdmissionRepairService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BootstrapDeveloperAdmissionRepairService")
            .finish_non_exhaustive()
    }
}

impl BootstrapDeveloperAdmissionRepairService {
    #[must_use]
    pub fn new(
        store: OperatorAuthenticationStore,
        developers: DeveloperRegistrationService,
        audit: Arc<dyn OperatorAuthenticationAuditSink>,
    ) -> Self {
        Self {
            store,
            developers,
            audit,
        }
    }

    pub async fn plan(
        &self,
        input: &BootstrapDeveloperAdmissionRepairInput,
    ) -> Result<BootstrapDeveloperAdmissionRepairPlan, OperatorPasswordAuthenticationError> {
        validate_bootstrap_developer_admission_repair_input(input)?;
        let email = mako_identity::NormalizedEmail::parse(&input.target_email)
            .map_err(|_| OperatorPasswordAuthenticationError::Ineligible)?;
        let identity = self
            .developers
            .store()
            .get_authentication_identity_by_email(&email)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::NotFound)?;
        if identity.email_verified_at_unix_seconds().is_none()
            || identity.security_status() != AuthenticationSecurityStatus::Active
        {
            return Err(OperatorPasswordAuthenticationError::Ineligible);
        }
        let entitlement = self
            .store
            .get_entitlement(identity.id())
            .await?
            .ok_or(OperatorPasswordAuthenticationError::Ineligible)?;
        let provenance_digest = self
            .store
            .authentication_key
            .idempotency_digest(&input.prior_bootstrap_idempotency_key);
        let provenance = self
            .store
            .get_idempotency(&provenance_digest)
            .await?
            .ok_or(OperatorPasswordAuthenticationError::Ineligible)?;
        if provenance.developer_identity_id() != identity.id()
            || provenance.audit_event_id() != "operator_bootstrap"
            || provenance.resulting_operator_epoch() != entitlement.operator_epoch()
        {
            return Err(OperatorPasswordAuthenticationError::Ineligible);
        }
        let email_digest = self.developers.store().lookup_key().email_digest(&email);
        let permissions = permission_digest(entitlement.permissions());
        let request_digest = bootstrap_developer_admission_repair_request_digest(
            input,
            identity.id(),
            &email_digest,
            &permissions,
            provenance.operation_digest(),
        );
        let repair_digest = self
            .store
            .authentication_key
            .idempotency_digest(&input.idempotency_key);
        if let Some(existing) = self.store.get_idempotency(&repair_digest).await? {
            if existing.request_digest() != request_digest
                || existing.developer_identity_id() != identity.id()
                || existing.audit_event_id() != "developer_admission_repair"
                || existing.resulting_operator_epoch() != entitlement.operator_epoch()
            {
                return Err(OperatorPasswordAuthenticationError::Conflict);
            }
            let role = self
                .developers
                .store()
                .get_developer_role(identity.id())
                .await
                .map_err(DeveloperWorkflowError::from)
                .map_err(OperatorPasswordAuthenticationError::Developer)?
                .ok_or(OperatorPasswordAuthenticationError::Conflict)?;
            if role.status() != DeveloperIdentityStatus::Waitlisted || role.review().is_some() {
                return Err(OperatorPasswordAuthenticationError::Conflict);
            }
            return Ok(bootstrap_developer_admission_repair_plan(
                input,
                identity.id(),
                email_digest,
                permissions,
                request_digest,
                existing.operation_digest().to_owned(),
                true,
            ));
        }
        let role = self
            .developers
            .store()
            .get_developer_role(identity.id())
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::Ineligible)?;
        if role.status() != DeveloperIdentityStatus::Active || role.review().is_some() {
            return Err(OperatorPasswordAuthenticationError::Ineligible);
        }
        let operation_digest = blake3::hash(
            format!(
                "bootstrap-developer-admission-repair-plan-v1\0{request_digest}\0{}\0{}\0{}\0{}\0{}",
                identity.credential_epoch(),
                role.authorization_epoch(),
                role.updated_at_unix_seconds(),
                entitlement.operator_epoch(),
                provenance.operation_digest(),
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        Ok(bootstrap_developer_admission_repair_plan(
            input,
            identity.id(),
            email_digest,
            permissions,
            request_digest,
            operation_digest,
            false,
        ))
    }

    pub async fn apply(
        &self,
        input: &BootstrapDeveloperAdmissionRepairInput,
        typed_confirmation: &str,
        now_unix_seconds: u64,
    ) -> Result<BootstrapDeveloperAdmissionRepairResult, OperatorPasswordAuthenticationError> {
        let plan = self.plan(input).await?;
        if typed_confirmation != plan.typed_confirmation {
            return Err(OperatorPasswordAuthenticationError::ConfirmationMismatch);
        }
        let entitlement = self
            .store
            .get_entitlement(&plan.developer_identity_id)
            .await?
            .ok_or(OperatorPasswordAuthenticationError::Conflict)?;
        if plan.replayed {
            return Ok(BootstrapDeveloperAdmissionRepairResult {
                environment_binding: plan.environment_binding,
                developer_identity_id: plan.developer_identity_id,
                permission_digest: plan.permission_digest,
                operation_digest: plan.operation_digest,
                developer_status_before: DeveloperIdentityStatus::Active,
                developer_status_after: DeveloperIdentityStatus::Waitlisted,
                operator_status_before: "active",
                operator_status_after: "active",
                operator_epoch: entitlement.operator_epoch(),
                replayed: true,
            });
        }
        let identity = self
            .developers
            .store()
            .get_authentication_identity(&plan.developer_identity_id)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::Conflict)?;
        let previous_role = self
            .developers
            .store()
            .get_developer_role(&plan.developer_identity_id)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::Conflict)?;
        let provenance_digest = self
            .store
            .authentication_key
            .idempotency_digest(&input.prior_bootstrap_idempotency_key);
        let provenance = self
            .store
            .get_idempotency(&provenance_digest)
            .await?
            .ok_or(OperatorPasswordAuthenticationError::Conflict)?;
        let mut next_role = previous_role.clone();
        next_role
            .repair_combined_bootstrap_to_waitlisted(now_unix_seconds)
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?;
        let repair_idempotency = OperatorIdempotencyRecord::new(
            self.store
                .authentication_key
                .idempotency_digest(&input.idempotency_key),
            plan.request_digest,
            plan.operation_digest.clone(),
            plan.developer_identity_id.clone(),
            entitlement.operator_epoch(),
            "developer_admission_repair",
            now_unix_seconds,
            now_unix_seconds
                .checked_add(365 * 24 * 60 * 60)
                .ok_or(OperatorPasswordAuthenticationError::InvalidConfiguration)?,
        )?;
        self.store
            .commit_bootstrap_developer_admission_repair(
                &identity,
                &previous_role,
                &next_role,
                &entitlement,
                &provenance,
                &repair_idempotency,
            )
            .await?;
        self.audit.record(OperatorAuthenticationAuditEvent {
            action: OperatorAuthenticationAuditAction::DeveloperRoleRepair,
            outcome: OperatorAuthenticationAuditOutcome::Allowed,
            operator_id: Some(entitlement.operator_id().clone()),
            classification: "combined_bootstrap_developer_admission_repair",
            at_unix_seconds: now_unix_seconds,
        });
        Ok(BootstrapDeveloperAdmissionRepairResult {
            environment_binding: plan.environment_binding,
            developer_identity_id: plan.developer_identity_id,
            permission_digest: plan.permission_digest,
            operation_digest: plan.operation_digest,
            developer_status_before: DeveloperIdentityStatus::Active,
            developer_status_after: DeveloperIdentityStatus::Waitlisted,
            operator_status_before: "active",
            operator_status_after: "active",
            operator_epoch: entitlement.operator_epoch(),
            replayed: false,
        })
    }
}

fn validate_bootstrap_developer_admission_repair_input(
    input: &BootstrapDeveloperAdmissionRepairInput,
) -> Result<(), OperatorPasswordAuthenticationError> {
    if !valid_text(&input.private_reason, 8, 1_024)
        || !valid_text(&input.environment_binding, 3, 256)
        || !valid_text(&input.idempotency_key, 16, 200)
        || !valid_text(&input.prior_bootstrap_idempotency_key, 16, 200)
        || input.target_email.len() > 320
        || input.idempotency_key == input.prior_bootstrap_idempotency_key
    {
        return Err(OperatorPasswordAuthenticationError::InvalidConfiguration);
    }
    Ok(())
}

fn bootstrap_developer_admission_repair_request_digest(
    input: &BootstrapDeveloperAdmissionRepairInput,
    identity_id: &DeveloperIdentityId,
    email_digest: &str,
    permission_digest: &str,
    provenance_operation_digest: &str,
) -> String {
    let reason_digest = blake3::hash(input.private_reason.as_bytes()).to_hex();
    let provenance_key_digest = blake3::hash(input.prior_bootstrap_idempotency_key.as_bytes());
    blake3::hash(
        format!(
            "bootstrap-developer-admission-repair-request-v1\0{}\0{}\0{email_digest}\0{permission_digest}\0{reason_digest}\0{}\0{provenance_key_digest}\0{provenance_operation_digest}",
            input.environment_binding,
            identity_id.as_str(),
            input.idempotency_key,
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string()
}

#[allow(clippy::too_many_arguments)]
fn bootstrap_developer_admission_repair_plan(
    input: &BootstrapDeveloperAdmissionRepairInput,
    identity_id: &DeveloperIdentityId,
    email_digest: String,
    permission_digest: String,
    request_digest: String,
    operation_digest: String,
    replayed: bool,
) -> BootstrapDeveloperAdmissionRepairPlan {
    let environment_digest = blake3::hash(input.environment_binding.as_bytes());
    let typed_confirmation = format!(
        "APPLY_BOOTSTRAP_DEVELOPER_ADMISSION_REPAIR:{environment_digest}:{}:{email_digest}:{permission_digest}:{operation_digest}",
        identity_id.as_str(),
    );
    BootstrapDeveloperAdmissionRepairPlan {
        environment_binding: input.environment_binding.clone(),
        developer_identity_id: identity_id.clone(),
        email_digest,
        permission_digest,
        request_digest,
        operation_digest,
        developer_status_before: DeveloperIdentityStatus::Active,
        developer_status_after: DeveloperIdentityStatus::Waitlisted,
        operator_status_before: "active",
        operator_status_after: "active",
        typed_confirmation,
        replayed,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorSessionProfile {
    pub operator_id: OperatorId,
    pub developer_identity_id: DeveloperIdentityId,
    pub email: String,
    pub display_name: String,
    pub developer_status: Option<DeveloperIdentityStatus>,
    pub permissions: BTreeSet<OperatorPermission>,
    pub password_verified_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

#[derive(Clone, Eq, PartialEq)]
pub struct OperatorSessionGrant {
    pub profile: OperatorSessionProfile,
    credential: OperatorSessionCredential,
    set_cookie_header: String,
}

impl OperatorSessionGrant {
    #[must_use]
    pub fn credential(&self) -> &OperatorSessionCredential {
        &self.credential
    }

    #[must_use]
    pub fn set_cookie_header(&self) -> &str {
        &self.set_cookie_header
    }
}

impl fmt::Debug for OperatorSessionGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorSessionGrant")
            .field("profile", &self.profile)
            .field("credential", &"[REDACTED]")
            .field("set_cookie_header", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedOperatorSession {
    pub principal: OperatorPrincipal,
    pub profile: OperatorSessionProfile,
    digest: String,
}

impl AuthenticatedOperatorSession {
    #[must_use]
    pub fn has_recent_password_verification(
        &self,
        now_unix_seconds: u64,
        freshness_seconds: u64,
    ) -> bool {
        now_unix_seconds
            .checked_sub(self.profile.password_verified_at_unix_seconds)
            .is_some_and(|age| age <= freshness_seconds)
    }
}

#[derive(Clone)]
pub struct OperatorAuthenticationService {
    store: OperatorAuthenticationStore,
    developers: DeveloperRegistrationService,
    config: OperatorAuthenticationConfig,
    audit: Arc<dyn OperatorAuthenticationAuditSink>,
}

impl fmt::Debug for OperatorAuthenticationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperatorAuthenticationService")
            .field("enabled", &self.config.enabled)
            .field(
                "session_lifetime_seconds",
                &self.config.session_lifetime_seconds,
            )
            .field(
                "mutation_freshness_seconds",
                &self.config.mutation_freshness_seconds,
            )
            .finish_non_exhaustive()
    }
}

impl OperatorAuthenticationService {
    pub fn new(
        store: OperatorAuthenticationStore,
        developers: DeveloperRegistrationService,
        config: OperatorAuthenticationConfig,
        audit: Arc<dyn OperatorAuthenticationAuditSink>,
    ) -> Result<Self, OperatorPasswordAuthenticationError> {
        config.validate()?;
        Ok(Self {
            store,
            developers,
            config,
            audit,
        })
    }

    #[must_use]
    pub const fn config(&self) -> &OperatorAuthenticationConfig {
        &self.config
    }

    #[must_use]
    pub const fn store(&self) -> &OperatorAuthenticationStore {
        &self.store
    }

    #[must_use]
    pub fn entitlement_service(&self) -> OperatorEntitlementService {
        OperatorEntitlementService::new(
            self.store.clone(),
            self.developers.clone(),
            Arc::clone(&self.audit),
        )
    }

    #[must_use]
    pub fn entitlement_admin_service(&self) -> OperatorEntitlementAdminService {
        OperatorEntitlementAdminService::new(
            self.store.clone(),
            self.developers.clone(),
            Arc::clone(&self.audit),
        )
    }

    #[must_use]
    pub fn bootstrap_service(&self) -> OperatorBootstrapService {
        OperatorBootstrapService::new(
            self.store.clone(),
            self.developers.clone(),
            Arc::clone(&self.audit),
        )
    }

    #[must_use]
    pub fn bootstrap_developer_admission_repair_service(
        &self,
    ) -> BootstrapDeveloperAdmissionRepairService {
        BootstrapDeveloperAdmissionRepairService::new(
            self.store.clone(),
            self.developers.clone(),
            Arc::clone(&self.audit),
        )
    }

    pub async fn sign_in(
        &self,
        email: &str,
        password: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<OperatorSessionGrant, OperatorPasswordAuthenticationError> {
        self.require_enabled()?;
        if !valid_text(source, 1, 512) || password.len() > 1_024 || email.len() > 320 {
            return Err(OperatorPasswordAuthenticationError::InvalidCredentials);
        }
        let source_digest = self
            .store
            .authentication_key
            .attempt_digest("source", source);
        let normalized_email = mako_identity::NormalizedEmail::parse(email).ok();
        let identity_attempt_value = normalized_email
            .as_ref()
            .map_or("[invalid]", mako_identity::NormalizedEmail::as_str);
        let identity_digest = self
            .store
            .authentication_key
            .attempt_digest("identity", identity_attempt_value);
        self.require_attempt_available(
            &source_digest,
            self.config.source_attempts_per_window,
            now_unix_seconds,
        )
        .await?;
        self.require_attempt_available(
            &identity_digest,
            self.config.identity_attempts_per_window,
            now_unix_seconds,
        )
        .await?;

        let identity = self
            .developers
            .verify_password_authentication_identity(email, password, now_unix_seconds)
            .await
            .map_err(OperatorPasswordAuthenticationError::Developer)?;
        let eligible = match identity {
            Some(identity)
                if identity.security_status() == AuthenticationSecurityStatus::Active
                    && identity.email_verified_at_unix_seconds().is_some() =>
            {
                let entitlement = self.store.get_entitlement(identity.id()).await?;
                entitlement.map(|entitlement| (identity, entitlement))
            }
            _ => None,
        };
        let Some((identity, entitlement)) = eligible else {
            self.record_failures(&source_digest, &identity_digest, now_unix_seconds)
                .await?;
            self.audit.record(OperatorAuthenticationAuditEvent {
                action: OperatorAuthenticationAuditAction::SignIn,
                outcome: OperatorAuthenticationAuditOutcome::Denied,
                operator_id: None,
                classification: "invalid_or_ineligible",
                at_unix_seconds: now_unix_seconds,
            });
            return Err(OperatorPasswordAuthenticationError::InvalidCredentials);
        };
        if entitlement.operator_id
            != self
                .store
                .authentication_key
                .stable_operator_id(identity.id())
            || entitlement.permissions.is_empty()
        {
            return Err(OperatorPasswordAuthenticationError::Store(
                OperatorAuthenticationStoreError::InvalidRecord,
            ));
        }
        self.clear_attempt(&source_digest).await?;
        self.clear_attempt(&identity_digest).await?;
        let credential = OperatorSessionCredential::generate();
        let digest = self.store.authentication_key.session_digest(&credential);
        let expires_at_unix_seconds = now_unix_seconds
            .checked_add(self.config.session_lifetime_seconds)
            .ok_or(OperatorPasswordAuthenticationError::InvalidConfiguration)?;
        let session = OperatorSessionRecord::new(
            digest,
            identity.id().clone(),
            entitlement.operator_id.clone(),
            identity.credential_epoch(),
            entitlement.operator_epoch,
            entitlement.permissions.iter().copied(),
            now_unix_seconds,
            expires_at_unix_seconds,
            "operator_password_signin",
        )?;
        self.store.create_session(&session).await?;
        let developer_status = self
            .developers
            .store()
            .get_developer_role(identity.id())
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .map(|role| role.status());
        let profile = profile(&identity, developer_status, &session);
        self.audit.record(OperatorAuthenticationAuditEvent {
            action: OperatorAuthenticationAuditAction::SignIn,
            outcome: OperatorAuthenticationAuditOutcome::Allowed,
            operator_id: Some(entitlement.operator_id.clone()),
            classification: "password",
            at_unix_seconds: now_unix_seconds,
        });
        Ok(OperatorSessionGrant {
            profile,
            set_cookie_header: format!(
                "{}={}; Path=/v1; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
                self.config.cookie_name,
                credential.expose_to_cookie(),
                self.config.session_lifetime_seconds
            ),
            credential,
        })
    }

    pub async fn authenticate(
        &self,
        encoded_credential: Option<&str>,
        now_unix_seconds: u64,
    ) -> Result<AuthenticatedOperatorSession, OperatorPasswordAuthenticationError> {
        self.require_enabled()?;
        let credential = OperatorSessionCredential::parse(
            encoded_credential.ok_or(OperatorPasswordAuthenticationError::Unauthenticated)?,
        )
        .map_err(|_| OperatorPasswordAuthenticationError::Unauthenticated)?;
        let digest = self.store.authentication_key.session_digest(&credential);
        let session = self
            .store
            .get_session(&digest)
            .await?
            .ok_or(OperatorPasswordAuthenticationError::Unauthenticated)?;
        if session.revoked_at_unix_seconds.is_some()
            || now_unix_seconds >= session.expires_at_unix_seconds
        {
            return Err(OperatorPasswordAuthenticationError::Unauthenticated);
        }
        let identity = self
            .developers
            .store()
            .get_authentication_identity(&session.developer_identity_id)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::Unauthenticated)?;
        let entitlement = self
            .store
            .get_entitlement(identity.id())
            .await?
            .ok_or(OperatorPasswordAuthenticationError::Unauthenticated)?;
        if identity.security_status() != AuthenticationSecurityStatus::Active
            || identity.email_verified_at_unix_seconds().is_none()
            || identity.credential_epoch() != session.credential_epoch
            || entitlement.operator_id != session.operator_id
            || entitlement.operator_epoch != session.operator_epoch
            || entitlement.permissions != session.permissions
        {
            return Err(OperatorPasswordAuthenticationError::Unauthenticated);
        }
        let developer_status = self
            .developers
            .store()
            .get_developer_role(identity.id())
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .map(|role| role.status());
        Ok(AuthenticatedOperatorSession {
            principal: OperatorPrincipal::authenticated(
                session.operator_id.clone(),
                identity.id().clone(),
                session.permissions.iter().copied(),
            ),
            profile: profile(&identity, developer_status, &session),
            digest,
        })
    }

    pub async fn sign_out(
        &self,
        encoded_credential: Option<&str>,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorPasswordAuthenticationError> {
        let Ok(authenticated) = self
            .authenticate(encoded_credential, now_unix_seconds)
            .await
        else {
            return Ok(());
        };
        let Some(previous) = self.store.get_session(&authenticated.digest).await? else {
            return Ok(());
        };
        let mut revoked = previous.clone();
        revoked.revoke(now_unix_seconds)?;
        match self.store.replace_session(&previous, &revoked).await {
            Ok(()) | Err(OperatorAuthenticationStoreError::Conflict) => {}
            Err(error) => return Err(error.into()),
        }
        self.audit.record(OperatorAuthenticationAuditEvent {
            action: OperatorAuthenticationAuditAction::SignOut,
            outcome: OperatorAuthenticationAuditOutcome::Allowed,
            operator_id: Some(authenticated.profile.operator_id),
            classification: "password_session",
            at_unix_seconds: now_unix_seconds,
        });
        Ok(())
    }

    pub async fn verify_current_password(
        &self,
        encoded_credential: Option<&str>,
        password: &str,
        source: &str,
        now_unix_seconds: u64,
    ) -> Result<OperatorSessionProfile, OperatorPasswordAuthenticationError> {
        let authenticated = self
            .authenticate(encoded_credential, now_unix_seconds)
            .await?;
        let identity = self
            .developers
            .store()
            .get_authentication_identity(&authenticated.profile.developer_identity_id)
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .ok_or(OperatorPasswordAuthenticationError::Unauthenticated)?;
        let identity_digest = self
            .store
            .authentication_key
            .attempt_digest("step-up-identity", identity.normalized_email().as_str());
        let source_digest = self
            .store
            .authentication_key
            .attempt_digest("step-up-source", source);
        self.require_attempt_available(
            &source_digest,
            self.config.source_attempts_per_window,
            now_unix_seconds,
        )
        .await?;
        self.require_attempt_available(
            &identity_digest,
            self.config.identity_attempts_per_window,
            now_unix_seconds,
        )
        .await?;
        let verified = self
            .developers
            .verify_password_authentication_identity(
                identity.normalized_email().as_str(),
                password,
                now_unix_seconds,
            )
            .await
            .map_err(OperatorPasswordAuthenticationError::Developer)?;
        if verified.as_ref().map(AuthenticationIdentityRecord::id) != Some(identity.id()) {
            self.record_failures(&source_digest, &identity_digest, now_unix_seconds)
                .await?;
            self.audit.record(OperatorAuthenticationAuditEvent {
                action: OperatorAuthenticationAuditAction::PasswordStepUp,
                outcome: OperatorAuthenticationAuditOutcome::Denied,
                operator_id: Some(authenticated.profile.operator_id),
                classification: "invalid_password",
                at_unix_seconds: now_unix_seconds,
            });
            return Err(OperatorPasswordAuthenticationError::InvalidCredentials);
        }
        let previous = self
            .store
            .get_session(&authenticated.digest)
            .await?
            .ok_or(OperatorPasswordAuthenticationError::Unauthenticated)?;
        let mut next = previous.clone();
        next.verify_password(now_unix_seconds)?;
        self.store.replace_session(&previous, &next).await?;
        self.clear_attempt(&source_digest).await?;
        self.clear_attempt(&identity_digest).await?;
        self.audit.record(OperatorAuthenticationAuditEvent {
            action: OperatorAuthenticationAuditAction::PasswordStepUp,
            outcome: OperatorAuthenticationAuditOutcome::Allowed,
            operator_id: Some(authenticated.profile.operator_id),
            classification: "password",
            at_unix_seconds: now_unix_seconds,
        });
        let developer_status = self
            .developers
            .store()
            .get_developer_role(identity.id())
            .await
            .map_err(DeveloperWorkflowError::from)
            .map_err(OperatorPasswordAuthenticationError::Developer)?
            .map(|role| role.status());
        Ok(profile(&identity, developer_status, &next))
    }

    pub fn require_mutation_freshness(
        &self,
        session: &AuthenticatedOperatorSession,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorPasswordAuthenticationError> {
        session
            .has_recent_password_verification(
                now_unix_seconds,
                self.config.mutation_freshness_seconds,
            )
            .then_some(())
            .ok_or(OperatorPasswordAuthenticationError::StepUpRequired)
    }

    pub async fn revoke_identity_sessions(
        &self,
        identity_id: &DeveloperIdentityId,
        now_unix_seconds: u64,
    ) -> Result<usize, OperatorPasswordAuthenticationError> {
        let revoked = self
            .store
            .revoke_identity_sessions(identity_id, now_unix_seconds)
            .await?;
        if revoked > 0 {
            self.audit.record(OperatorAuthenticationAuditEvent {
                action: OperatorAuthenticationAuditAction::SessionRevocation,
                outcome: OperatorAuthenticationAuditOutcome::Allowed,
                operator_id: None,
                classification: "identity_security_event",
                at_unix_seconds: now_unix_seconds,
            });
        }
        Ok(revoked)
    }

    #[must_use]
    pub fn expired_cookie_header(&self) -> String {
        format!(
            "{}=; Path=/v1; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
            self.config.cookie_name
        )
    }

    fn require_enabled(&self) -> Result<(), OperatorPasswordAuthenticationError> {
        self.config
            .enabled
            .then_some(())
            .ok_or(OperatorPasswordAuthenticationError::Unavailable)
    }

    async fn require_attempt_available(
        &self,
        digest: &str,
        maximum: u32,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorPasswordAuthenticationError> {
        if let Some(record) = self.store.get_attempt(digest).await?
            && now_unix_seconds < record.expires_at_unix_seconds
            && record.count >= maximum
            && now_unix_seconds < record.next_allowed_at_unix_seconds
        {
            self.audit.record(OperatorAuthenticationAuditEvent {
                action: OperatorAuthenticationAuditAction::SignIn,
                outcome: OperatorAuthenticationAuditOutcome::Throttled,
                operator_id: None,
                classification: "attempt_budget",
                at_unix_seconds: now_unix_seconds,
            });
            return Err(OperatorPasswordAuthenticationError::RateLimited {
                retry_after_seconds: record
                    .next_allowed_at_unix_seconds
                    .saturating_sub(now_unix_seconds)
                    .max(1),
            });
        }
        Ok(())
    }

    async fn record_failures(
        &self,
        source_digest: &str,
        identity_digest: &str,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorPasswordAuthenticationError> {
        self.record_failure(
            source_digest,
            OperatorAttemptClass::Source,
            now_unix_seconds,
        )
        .await?;
        self.record_failure(
            identity_digest,
            OperatorAttemptClass::Identity,
            now_unix_seconds,
        )
        .await
    }

    async fn record_failure(
        &self,
        digest: &str,
        class: OperatorAttemptClass,
        now_unix_seconds: u64,
    ) -> Result<(), OperatorPasswordAuthenticationError> {
        let expires_at = now_unix_seconds
            .checked_add(self.config.attempt_window_seconds)
            .ok_or(OperatorPasswordAuthenticationError::InvalidConfiguration)?;
        for _ in 0..8 {
            match self.store.get_attempt(digest).await? {
                None => {
                    let next =
                        OperatorAttemptRecord::new(digest, class, now_unix_seconds, expires_at)?;
                    match self.store.create_attempt(&next).await {
                        Ok(()) => return Ok(()),
                        Err(OperatorAuthenticationStoreError::Conflict) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                Some(previous) if now_unix_seconds >= previous.expires_at_unix_seconds => {
                    let next =
                        OperatorAttemptRecord::new(digest, class, now_unix_seconds, expires_at)?;
                    match self.store.replace_attempt(&previous, &next).await {
                        Ok(()) => return Ok(()),
                        Err(OperatorAuthenticationStoreError::Conflict) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                Some(previous) => {
                    let mut next = previous.clone();
                    next.record_failure(
                        now_unix_seconds,
                        self.config.base_backoff_seconds,
                        self.config.maximum_backoff_seconds,
                    )?;
                    match self.store.replace_attempt(&previous, &next).await {
                        Ok(()) => return Ok(()),
                        Err(OperatorAuthenticationStoreError::Conflict) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        Err(OperatorPasswordAuthenticationError::RateLimited {
            retry_after_seconds: self.config.base_backoff_seconds,
        })
    }

    async fn clear_attempt(&self, digest: &str) -> Result<(), OperatorPasswordAuthenticationError> {
        for _ in 0..4 {
            let Some(record) = self.store.get_attempt(digest).await? else {
                return Ok(());
            };
            match self.store.clear_attempt(&record).await {
                Ok(()) => return Ok(()),
                Err(OperatorAuthenticationStoreError::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

fn profile(
    identity: &AuthenticationIdentityRecord,
    developer_status: Option<DeveloperIdentityStatus>,
    session: &OperatorSessionRecord,
) -> OperatorSessionProfile {
    OperatorSessionProfile {
        operator_id: session.operator_id.clone(),
        developer_identity_id: identity.id().clone(),
        email: identity.normalized_email().as_str().to_owned(),
        display_name: identity.display_name().to_owned(),
        developer_status,
        permissions: session.permissions.clone(),
        password_verified_at_unix_seconds: session.password_verified_at_unix_seconds,
        expires_at_unix_seconds: session.expires_at_unix_seconds,
    }
}

#[derive(Debug)]
pub enum OperatorPasswordAuthenticationError {
    InvalidConfiguration,
    InvalidCredentials,
    Unauthenticated,
    StepUpRequired,
    RateLimited { retry_after_seconds: u64 },
    Unavailable,
    NotFound,
    Ineligible,
    Conflict,
    ConfirmationMismatch,
    LastRecoverableAdministrator,
    Developer(DeveloperWorkflowError),
    Store(OperatorAuthenticationStoreError),
}

impl fmt::Display for OperatorPasswordAuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "operator authentication configuration is invalid",
            Self::InvalidCredentials => "operator credentials were not accepted",
            Self::Unauthenticated => "operator session is not authenticated",
            Self::StepUpRequired => "operator password verification is required",
            Self::RateLimited { .. } => "operator authentication is rate limited",
            Self::Unavailable => "operator password authentication is unavailable",
            Self::NotFound => "operator entitlement target was not found",
            Self::Ineligible => "operator entitlement target is not eligible",
            Self::Conflict => "operator entitlement state changed concurrently",
            Self::ConfirmationMismatch => "operator bootstrap confirmation does not match",
            Self::LastRecoverableAdministrator => {
                "operator entitlement change would remove the last recoverable administrator"
            }
            Self::Developer(_) | Self::Store(_) => "operator authentication dependency failed",
        })
    }
}

impl Error for OperatorPasswordAuthenticationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Developer(error) => Some(error),
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<OperatorAuthenticationStoreError> for OperatorPasswordAuthenticationError {
    fn from(value: OperatorAuthenticationStoreError) -> Self {
        Self::Store(value)
    }
}

fn encode_record<T: Serialize>(value: &T) -> Result<Vec<u8>, OperatorAuthenticationStoreError> {
    let encoded = serde_json::to_vec(value)?;
    if encoded.len() > MAX_RECORD_BYTES {
        return Err(OperatorAuthenticationStoreError::LimitExceeded);
    }
    Ok(encoded)
}

fn decode_record<T: DeserializeOwned>(value: &[u8]) -> Result<T, OperatorAuthenticationStoreError> {
    if value.len() > MAX_RECORD_BYTES {
        return Err(OperatorAuthenticationStoreError::LimitExceeded);
    }
    serde_json::from_slice(value).map_err(OperatorAuthenticationStoreError::from)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_text(value: &str, minimum: usize, maximum: usize) -> bool {
    (minimum..=maximum).contains(&value.len())
        && !value.trim().is_empty()
        && !value.chars().any(char::is_control)
}

#[derive(Debug)]
pub enum OperatorAuthenticationStoreError {
    InvalidCredential,
    InvalidSession,
    InvalidRecord,
    ScopeMismatch,
    Conflict,
    Expired,
    LimitExceeded,
    UnsupportedDurability(Durability),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for OperatorAuthenticationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCredential => formatter.write_str("operator credential is invalid"),
            Self::InvalidSession => formatter.write_str("operator session is invalid"),
            Self::InvalidRecord => formatter.write_str("operator authentication record is invalid"),
            Self::ScopeMismatch => {
                formatter.write_str("operator authentication scope does not match")
            }
            Self::Conflict => {
                formatter.write_str("operator authentication state changed concurrently")
            }
            Self::Expired => formatter.write_str("operator authentication record expired"),
            Self::LimitExceeded => {
                formatter.write_str("operator authentication limit was exceeded")
            }
            Self::UnsupportedDurability(value) => write!(
                formatter,
                "operator authentication store does not support {value:?} durability"
            ),
            Self::Keyspace(_) => {
                formatter.write_str("operator authentication storage key is invalid")
            }
            Self::Storage(_) => {
                formatter.write_str("operator authentication storage operation failed")
            }
            Self::Json(_) => formatter.write_str("operator authentication record encoding failed"),
        }
    }
}

impl Error for OperatorAuthenticationStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ControlKeyspaceError> for OperatorAuthenticationStoreError {
    fn from(value: ControlKeyspaceError) -> Self {
        Self::Keyspace(value)
    }
}

impl From<StorageError> for OperatorAuthenticationStoreError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<serde_json::Error> for OperatorAuthenticationStoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::executor::block_on;
    use mako_identity::{Argon2idParameters, PasswordPolicy, PasswordService};
    use mako_storage::{
        Durability, FailurePoint, KvAdapter, MemoryAdapter, RocksDbAdapter, RocksDbConfig,
        StorageError, StorageErrorKind, WriteBatch,
    };

    use super::*;

    const NOW: u64 = 1_786_400_000;

    fn identity() -> DeveloperIdentityId {
        DeveloperIdentityId::parse("dev_operator01").expect("identity")
    }

    fn store(adapter: MemoryAdapter) -> OperatorAuthenticationStore {
        OperatorAuthenticationStore::new(
            Arc::new(adapter),
            Durability::Memory,
            OperatorAuthenticationKey::derive(b"test operator auth key"),
        )
        .expect("store")
    }

    fn entitlement(store: &OperatorAuthenticationStore) -> OperatorEntitlementRecord {
        OperatorEntitlementRecord::new(
            identity(),
            store.authentication_key().stable_operator_id(&identity()),
            1,
            [OperatorPermission::WaitlistReview],
            "initial protected operator grant",
            "audit_entitlement01",
            NOW,
        )
        .expect("entitlement")
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<OperatorAuthenticationAuditEvent>>);

    impl OperatorAuthenticationAuditSink for Audit {
        fn record(&self, event: OperatorAuthenticationAuditEvent) {
            self.0.lock().expect("audit lock").push(event);
        }
    }

    fn authentication_service(
        status: DeveloperIdentityStatus,
        entitled: bool,
        configure: impl FnOnce(&mut OperatorAuthenticationConfig),
    ) -> (OperatorAuthenticationService, MemoryAdapter, Arc<Audit>) {
        let adapter = MemoryAdapter::new();
        let lookup = crate::DeveloperLookupKey::derive(b"operator developer lookup key");
        let passwords =
            PasswordService::new(PasswordPolicy::default(), Argon2idParameters::default());
        let hash = passwords
            .hash("correct horse battery staple")
            .expect("hash");
        let mut account = crate::DeveloperAccount::new_unverified(
            identity(),
            mako_identity::NormalizedEmail::parse("operator@example.test").expect("email"),
            "Test Operator",
            hash.encoded(),
            NOW - 10,
        )
        .expect("account");
        if status != DeveloperIdentityStatus::Unverified {
            account
                .transition(DeveloperIdentityStatus::Waitlisted, NOW - 9)
                .expect("verify");
        }
        match status {
            DeveloperIdentityStatus::Active => account
                .transition(DeveloperIdentityStatus::Active, NOW - 8)
                .expect("activate"),
            DeveloperIdentityStatus::Rejected => account
                .transition(DeveloperIdentityStatus::Rejected, NOW - 8)
                .expect("reject"),
            DeveloperIdentityStatus::Disabled => {
                account
                    .transition(DeveloperIdentityStatus::Active, NOW - 8)
                    .expect("activate before disable");
                account
                    .transition(DeveloperIdentityStatus::Disabled, NOW - 7)
                    .expect("disable");
            }
            DeveloperIdentityStatus::Deleted => account
                .transition(DeveloperIdentityStatus::Deleted, NOW - 8)
                .expect("delete"),
            DeveloperIdentityStatus::Unverified | DeveloperIdentityStatus::Waitlisted => {}
        }
        let identity_key =
            ControlKeyspace::authentication_identity_key(account.id()).expect("identity key");
        let role_key = ControlKeyspace::developer_role_key(account.id()).expect("role key");
        let email_key =
            ControlKeyspace::developer_email_key(&lookup.email_digest(account.normalized_email()))
                .expect("email key");
        let status_key = ControlKeyspace::developer_status_key(
            account.status(),
            account.created_at_unix_seconds(),
            account.id(),
        )
        .expect("status key");
        let mut seed = WriteBatch::new();
        seed.put(
            &identity_key,
            serde_json::to_vec(account.identity()).expect("identity json"),
        );
        seed.put(
            &role_key,
            serde_json::to_vec(account.role()).expect("role json"),
        );
        seed.put(
            &email_key,
            serde_json::to_vec(account.id()).expect("identity json"),
        );
        seed.put(
            &status_key,
            serde_json::to_vec(account.id()).expect("identity json"),
        );
        block_on(adapter.write(seed, Durability::Memory)).expect("seed account");

        let developer_store = crate::DeveloperRegistrationStore::new(
            Arc::new(adapter.clone()),
            Durability::Memory,
            lookup,
        )
        .expect("developer store");
        let developer_config = crate::DeveloperRegistrationConfig {
            enabled: true,
            public_origin: "https://cloud.example.test".to_owned(),
            issuer: "https://cloud.example.test/v1/developer-auth".to_owned(),
            ..crate::DeveloperRegistrationConfig::default()
        };
        let developers = crate::DeveloperRegistrationService::new(
            developer_store,
            developer_config,
            crate::DeveloperMailEncryptionKey::derive(b"operator test mail key"),
        )
        .expect("developer service");
        let operator_store = store(adapter.clone());
        if entitled {
            block_on(operator_store.create_entitlement(&entitlement(&operator_store)))
                .expect("entitlement");
        }
        let mut config = OperatorAuthenticationConfig {
            enabled: true,
            ..OperatorAuthenticationConfig::default()
        };
        configure(&mut config);
        let audit = Arc::new(Audit::default());
        let service =
            OperatorAuthenticationService::new(operator_store, developers, config, audit.clone())
                .expect("operator auth service");
        (service, adapter, audit)
    }

    #[test]
    fn entitlement_admin_preserves_the_last_recoverable_security_administrator() {
        let (service, _, _) =
            authentication_service(DeveloperIdentityStatus::Active, false, |_| {});
        let record = OperatorEntitlementRecord::new(
            identity(),
            service
                .store()
                .authentication_key()
                .stable_operator_id(&identity()),
            1,
            [OperatorPermission::SecurityManage],
            "initial recoverable security administrator",
            "audit_security_admin",
            NOW,
        )
        .expect("entitlement");
        block_on(service.store().create_entitlement(&record)).expect("create entitlement");
        let input = OperatorEntitlementAdminInput {
            kind: OperatorEntitlementChangeKind::Revoke,
            target_email: "operator@example.test".to_owned(),
            permissions: BTreeSet::new(),
            private_reason: "remove security administrator during qualification".to_owned(),
            environment_binding: "https://cloud.example.test".to_owned(),
            idempotency_key: "security-admin-revoke-0001".to_owned(),
        };
        assert!(matches!(
            block_on(service.entitlement_admin_service().plan(&input)),
            Err(OperatorPasswordAuthenticationError::LastRecoverableAdministrator)
        ));
        assert!(
            block_on(service.store().get_entitlement(&identity()))
                .expect("entitlement read")
                .is_some()
        );
    }

    #[test]
    fn credential_and_key_material_are_redacted_and_stable() {
        let key = OperatorAuthenticationKey::derive(b"test operator auth key");
        let credential = OperatorSessionCredential::generate();
        assert_eq!(format!("{key:?}"), "OperatorAuthenticationKey([REDACTED])");
        assert_eq!(
            format!("{credential:?}"),
            "OperatorSessionCredential([REDACTED])"
        );
        assert_eq!(
            key.stable_operator_id(&identity()),
            key.stable_operator_id(&identity())
        );
        assert_ne!(
            key.session_digest(&credential),
            credential.expose_to_cookie()
        );
    }

    #[test]
    fn records_enforce_bounds_epochs_and_permission_sets() {
        let adapter = MemoryAdapter::new();
        let store = store(adapter);
        assert!(
            OperatorEntitlementRecord::new(
                identity(),
                store.authentication_key().stable_operator_id(&identity()),
                0,
                [],
                "reason is long enough",
                "audit_invalid01",
                NOW,
            )
            .is_err()
        );
        let digest = "a".repeat(64);
        assert!(
            OperatorSessionRecord::new(
                digest,
                identity(),
                store.authentication_key().stable_operator_id(&identity()),
                1,
                1,
                [OperatorPermission::TenantRead],
                NOW,
                NOW + MAX_SESSION_SECONDS + 1,
                "audit_session001",
            )
            .is_err()
        );
    }

    #[test]
    fn additive_state_survives_restart_and_migration_replay() {
        let adapter = MemoryAdapter::new();
        let first = store(adapter.clone());
        assert!(block_on(first.migrate()).expect("migration").completed);
        let entitlement = entitlement(&first);
        block_on(first.create_entitlement(&entitlement)).expect("create entitlement");
        let credential = OperatorSessionCredential::generate();
        let digest = first.authentication_key().session_digest(&credential);
        let session = OperatorSessionRecord::new(
            digest.clone(),
            identity(),
            entitlement.operator_id().clone(),
            3,
            entitlement.operator_epoch(),
            entitlement.permissions().iter().copied(),
            NOW,
            NOW + 3_600,
            "audit_session001",
        )
        .expect("session");
        block_on(first.create_session(&session)).expect("create session");

        let reopened = store(adapter);
        assert!(
            block_on(reopened.migrate())
                .expect("migration replay")
                .completed
        );
        assert_eq!(
            block_on(reopened.get_entitlement(&identity()))
                .expect("read")
                .expect("entitlement"),
            entitlement
        );
        assert_eq!(
            block_on(reopened.get_session(&digest))
                .expect("read")
                .expect("session"),
            session
        );
    }

    #[test]
    fn rocksdb_restart_preserves_entitlement_and_never_resurrects_revoked_session() {
        let directory = tempfile::tempdir().expect("temporary RocksDB");
        let mut config = RocksDbConfig::new(directory.path());
        config.minimum_durability = Durability::Sync;
        let adapter = RocksDbAdapter::open(config.clone()).expect("open RocksDB");
        let persistent = OperatorAuthenticationStore::new(
            Arc::new(adapter.clone()),
            Durability::Sync,
            OperatorAuthenticationKey::derive(b"persistent operator test key"),
        )
        .expect("store");
        block_on(persistent.migrate()).expect("migration");
        let entitlement = entitlement(&persistent);
        block_on(persistent.create_entitlement(&entitlement)).expect("entitlement");
        let credential = OperatorSessionCredential::generate();
        let digest = persistent.authentication_key().session_digest(&credential);
        let session = OperatorSessionRecord::new(
            digest.clone(),
            identity(),
            entitlement.operator_id().clone(),
            3,
            entitlement.operator_epoch(),
            entitlement.permissions().iter().copied(),
            NOW,
            NOW + 3_600,
            "audit_persistent01",
        )
        .expect("session");
        block_on(persistent.create_session(&session)).expect("create session");
        assert_eq!(
            block_on(persistent.revoke_identity_sessions(&identity(), NOW + 1)).expect("revoke"),
            1
        );
        let revoked = block_on(persistent.get_session(&digest))
            .expect("revoked session read")
            .expect("revoked session");
        drop(persistent);
        drop(adapter);

        let reopened_adapter = RocksDbAdapter::open(config).expect("reopen RocksDB");
        let reopened = OperatorAuthenticationStore::new(
            Arc::new(reopened_adapter),
            Durability::Sync,
            OperatorAuthenticationKey::derive(b"persistent operator test key"),
        )
        .expect("reopened store");
        block_on(reopened.migrate()).expect("migration replay");
        assert_eq!(
            block_on(reopened.get_entitlement(&identity()))
                .expect("entitlement read")
                .expect("entitlement"),
            entitlement
        );
        assert_eq!(
            block_on(reopened.get_session(&digest))
                .expect("session read")
                .expect("session"),
            revoked
        );
        assert!(
            block_on(reopened.cleanup_expired(NOW + 3_601, NonZeroUsize::new(10).expect("limit"),))
                .expect("cleanup")
                .sessions
                > 0
        );
        assert!(
            block_on(reopened.get_session(&digest))
                .expect("session read")
                .is_none()
        );
    }

    #[test]
    fn malformed_and_oversized_records_fail_closed_without_touching_prior_state() {
        let adapter = MemoryAdapter::new();
        let store = store(adapter.clone());
        let unrelated_key = ControlKeyspace::developer_key(&identity()).expect("key");
        let malformed_key = ControlKeyspace::operator_entitlement_key(&identity()).expect("key");
        let mut seed = WriteBatch::new();
        seed.put(&unrelated_key, b"prior-release-value");
        seed.put(&malformed_key, b"{not-json");
        block_on(adapter.write(seed, Durability::Memory)).expect("seed");
        assert!(block_on(store.get_entitlement(&identity())).is_err());
        assert_eq!(
            block_on(adapter.get(&unrelated_key)).expect("prior state"),
            Some(b"prior-release-value".to_vec())
        );
    }

    #[test]
    fn epoch_replacement_is_monotonic_and_cleanup_is_bounded() {
        let adapter = MemoryAdapter::new();
        let store = store(adapter);
        let first = entitlement(&store);
        block_on(store.create_entitlement(&first)).expect("create");
        let next = first
            .replacement(
                [
                    OperatorPermission::TenantRead,
                    OperatorPermission::WaitlistReview,
                ],
                "expanded only after protected review",
                "audit_entitlement02",
                NOW + 1,
            )
            .expect("replacement");
        block_on(store.replace_entitlement(&first, &next)).expect("replace");
        assert_eq!(next.operator_epoch(), 2);

        let attempt = OperatorAttemptRecord::new(
            "b".repeat(64),
            OperatorAttemptClass::Identity,
            NOW,
            NOW + 60,
        )
        .expect("attempt");
        block_on(store.create_attempt(&attempt)).expect("attempt create");
        let report =
            block_on(store.cleanup_expired(NOW + 61, NonZeroUsize::new(10).expect("limit")))
                .expect("cleanup");
        assert_eq!(report.attempts, 1);
    }

    #[test]
    fn eligible_identity_gets_isolated_digest_backed_session_and_signout_revokes_it() {
        let (service, adapter, audit) =
            authentication_service(DeveloperIdentityStatus::Active, true, |_| {});
        let grant = block_on(service.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.10",
            NOW,
        ))
        .expect("sign in");
        assert!(
            grant
                .set_cookie_header()
                .contains("Secure; HttpOnly; SameSite=Strict")
        );
        assert!(grant.set_cookie_header().contains("Path=/v1"));
        assert!(!grant.set_cookie_header().contains("Domain="));
        let authenticated =
            block_on(service.authenticate(Some(grant.credential().expose_to_cookie()), NOW + 1))
                .expect("authenticate");
        assert!(
            authenticated
                .principal
                .allows(OperatorPermission::WaitlistReview)
        );
        assert!(
            service
                .require_mutation_freshness(&authenticated, NOW + 300)
                .is_ok()
        );
        assert!(matches!(
            service.require_mutation_freshness(&authenticated, NOW + 301),
            Err(OperatorPasswordAuthenticationError::StepUpRequired)
        ));

        let values = block_on(adapter.scan(ScanRequest::new(
            ControlKeyspace::operator_sessions_range().expect("range"),
            ScanDirection::Forward,
            NonZeroUsize::new(10).expect("limit"),
        )))
        .expect("scan");
        assert_eq!(values.len(), 1);
        assert!(
            !String::from_utf8_lossy(&values[0].value)
                .contains(grant.credential().expose_to_cookie())
        );

        block_on(service.sign_out(Some(grant.credential().expose_to_cookie()), NOW + 2))
            .expect("sign out");
        assert!(
            block_on(service.authenticate(Some(grant.credential().expose_to_cookie()), NOW + 3,))
                .is_err()
        );
        assert_eq!(
            audit
                .0
                .lock()
                .expect("audit")
                .iter()
                .filter(|event| event.outcome == OperatorAuthenticationAuditOutcome::Allowed)
                .count(),
            2
        );
    }

    #[test]
    fn unknown_wrong_password_and_unentitled_fail_identically() {
        for (status, entitled, email, password) in [
            (
                DeveloperIdentityStatus::Active,
                true,
                "unknown@example.test",
                "correct horse battery staple",
            ),
            (
                DeveloperIdentityStatus::Active,
                true,
                "operator@example.test",
                "wrong horse battery staple",
            ),
            (
                DeveloperIdentityStatus::Active,
                false,
                "operator@example.test",
                "correct horse battery staple",
            ),
        ] {
            let (service, _, _) = authentication_service(status, entitled, |_| {});
            assert!(matches!(
                block_on(service.sign_in(email, password, "198.51.100.11", NOW)),
                Err(OperatorPasswordAuthenticationError::InvalidCredentials)
            ));
        }
    }

    #[test]
    fn operator_access_ignores_every_developer_role_state_and_absent_role() {
        for status in [
            DeveloperIdentityStatus::Waitlisted,
            DeveloperIdentityStatus::Active,
            DeveloperIdentityStatus::Rejected,
            DeveloperIdentityStatus::Disabled,
        ] {
            let (service, _, _) = authentication_service(status, true, |_| {});
            let grant = block_on(service.sign_in(
                "operator@example.test",
                "correct horse battery staple",
                "198.51.100.30",
                NOW,
            ))
            .expect("entitled verified identity signs in");
            assert_eq!(grant.profile.developer_status, Some(status));
        }

        let (service, adapter, _) =
            authentication_service(DeveloperIdentityStatus::Active, true, |_| {});
        let account = block_on(service.developers.store().get_account(&identity()))
            .expect("account read")
            .expect("account");
        let role_key = ControlKeyspace::developer_role_key(account.id()).expect("role key");
        let status_key = ControlKeyspace::developer_status_key(
            account.status(),
            account.created_at_unix_seconds(),
            account.id(),
        )
        .expect("status key");
        let mut remove_role = WriteBatch::new();
        remove_role.delete(&role_key);
        remove_role.delete(&status_key);
        block_on(adapter.write(remove_role, Durability::Memory)).expect("remove developer role");
        let grant = block_on(service.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.31",
            NOW,
        ))
        .expect("operator-only identity signs in");
        assert_eq!(grant.profile.developer_status, None);
    }

    #[test]
    fn developer_transition_preserves_operator_session_but_account_suspension_revokes_it() {
        let (service, _, _) =
            authentication_service(DeveloperIdentityStatus::Waitlisted, true, |_| {});
        let grant = block_on(service.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.32",
            NOW,
        ))
        .expect("sign in");
        let previous = block_on(service.developers.store().get_account(&identity()))
            .expect("account read")
            .expect("account");
        let mut approved = previous.clone();
        approved
            .transition(DeveloperIdentityStatus::Active, NOW + 1)
            .expect("approve developer");
        block_on(
            service
                .developers
                .store()
                .replace_account(&previous, &approved),
        )
        .expect("commit developer transition");
        let authenticated =
            block_on(service.authenticate(Some(grant.credential().expose_to_cookie()), NOW + 2))
                .expect("developer change preserves operator session");
        assert_eq!(
            authenticated.profile.developer_status,
            Some(DeveloperIdentityStatus::Active)
        );

        let previous_identity = block_on(
            service
                .developers
                .store()
                .get_authentication_identity(&identity()),
        )
        .expect("identity read")
        .expect("identity");
        let mut suspended = previous_identity.clone();
        suspended
            .transition_security(AuthenticationSecurityStatus::Suspended, NOW + 3)
            .expect("suspend account");
        block_on(
            service
                .developers
                .store()
                .replace_authentication_identity(&previous_identity, &suspended),
        )
        .expect("commit suspension");
        assert!(matches!(
            block_on(service.authenticate(Some(grant.credential().expose_to_cookie()), NOW + 4,)),
            Err(OperatorPasswordAuthenticationError::Unauthenticated)
        ));
    }

    #[test]
    fn operator_self_rejection_changes_only_developer_state_and_preserves_session() {
        let (service, _, _) =
            authentication_service(DeveloperIdentityStatus::Waitlisted, true, |_| {});
        let grant = block_on(service.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.33",
            NOW,
        ))
        .expect("operator sign in");
        let authenticated =
            block_on(service.authenticate(Some(grant.credential().expose_to_cookie()), NOW + 1))
                .expect("operator session");
        let entitlement_before = block_on(service.store().get_entitlement(&identity()))
            .expect("entitlement read")
            .expect("entitlement");
        let rejected = block_on(service.developers.decide(
            &authenticated.principal,
            &identity(),
            crate::DeveloperDecisionKind::Reject,
            "operator deliberately rejected their own developer application",
            "request_self_reject01",
            "idempotency_self_reject01",
            NOW + 2,
        ))
        .expect("self rejection");
        assert_eq!(rejected.status(), DeveloperIdentityStatus::Rejected);
        assert_eq!(
            block_on(service.store().get_entitlement(&identity()))
                .expect("entitlement read")
                .expect("entitlement"),
            entitlement_before
        );
        block_on(service.authenticate(Some(grant.credential().expose_to_cookie()), NOW + 3))
            .expect("operator session survives self rejection");
        let decision_digest = service
            .developers
            .store()
            .lookup_key()
            .secret_digest("waitlist-decision-idempotency", "idempotency_self_reject01");
        assert!(
            block_on(service.developers.store().get_decision(&decision_digest))
                .expect("decision read")
                .expect("decision")
                .self_review()
        );
    }

    #[test]
    fn attempt_budgets_throttle_and_do_not_store_attempt_keys_in_cleartext() {
        let (service, adapter, audit) =
            authentication_service(DeveloperIdentityStatus::Active, true, |config| {
                config.source_attempts_per_window = 2;
                config.identity_attempts_per_window = 2;
            });
        for second in 0..2 {
            assert!(matches!(
                block_on(service.sign_in(
                    "operator@example.test",
                    "wrong horse battery staple",
                    "198.51.100.12",
                    NOW + second,
                )),
                Err(OperatorPasswordAuthenticationError::InvalidCredentials)
            ));
        }
        assert!(matches!(
            block_on(service.sign_in(
                "operator@example.test",
                "wrong horse battery staple",
                "198.51.100.12",
                NOW + 2,
            )),
            Err(OperatorPasswordAuthenticationError::RateLimited { .. })
        ));
        let values = block_on(adapter.scan(ScanRequest::new(
            ControlKeyspace::operator_attempts_range().expect("range"),
            ScanDirection::Forward,
            NonZeroUsize::new(10).expect("limit"),
        )))
        .expect("scan");
        let persisted = values
            .iter()
            .flat_map(|value| value.key.iter().chain(value.value.iter()))
            .copied()
            .collect::<Vec<_>>();
        let persisted = String::from_utf8_lossy(&persisted);
        assert!(!persisted.contains("operator@example.test"));
        assert!(!persisted.contains("198.51.100.12"));
        assert!(
            audit
                .0
                .lock()
                .expect("audit")
                .iter()
                .any(|event| event.outcome == OperatorAuthenticationAuditOutcome::Throttled)
        );
    }

    #[test]
    fn step_up_is_same_identity_and_cross_audience_values_are_rejected() {
        let (service, _, _) = authentication_service(DeveloperIdentityStatus::Active, true, |_| {});
        assert!(matches!(
            block_on(service.authenticate(Some("developer-session-token"), NOW)),
            Err(OperatorPasswordAuthenticationError::Unauthenticated)
        ));
        let grant = block_on(service.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.13",
            NOW,
        ))
        .expect("sign in");
        assert!(matches!(
            block_on(service.verify_current_password(
                Some(grant.credential().expose_to_cookie()),
                "wrong horse battery staple",
                "198.51.100.13",
                NOW + 301,
            )),
            Err(OperatorPasswordAuthenticationError::InvalidCredentials)
        ));
        let profile = block_on(service.verify_current_password(
            Some(grant.credential().expose_to_cookie()),
            "correct horse battery staple",
            "198.51.100.13",
            NOW + 302,
        ))
        .expect("step up");
        assert_eq!(profile.password_verified_at_unix_seconds, NOW + 302);
    }

    #[test]
    fn entitlement_grant_replacement_and_revocation_are_idempotent_and_revoke_sessions() {
        let (service, _, _) =
            authentication_service(DeveloperIdentityStatus::Active, false, |_| {});
        let entitlements = service.entitlement_service();
        let granted = block_on(entitlements.apply_change(
            OperatorEntitlementChangeKind::Grant,
            &identity(),
            [
                OperatorPermission::TenantRead,
                OperatorPermission::WaitlistReview,
            ],
            "initial protected operator entitlement",
            "request_grant001",
            "idempotency-grant-0001",
            NOW,
            86_400,
        ))
        .expect("grant");
        assert_eq!(granted.resulting_operator_epoch, 1);
        let replay = block_on(entitlements.apply_change(
            OperatorEntitlementChangeKind::Grant,
            &identity(),
            [
                OperatorPermission::TenantRead,
                OperatorPermission::WaitlistReview,
            ],
            "initial protected operator entitlement",
            "request_grant001",
            "idempotency-grant-0001",
            NOW,
            86_400,
        ))
        .expect("replay");
        assert!(replay.replayed);

        let first = block_on(service.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.20",
            NOW + 1,
        ))
        .expect("first session");
        let replaced = block_on(entitlements.apply_change(
            OperatorEntitlementChangeKind::Replace,
            &identity(),
            [OperatorPermission::TenantRead],
            "least privilege entitlement reduction",
            "request_replace01",
            "idempotency-replace-01",
            NOW + 2,
            86_400,
        ))
        .expect("replace");
        assert_eq!(replaced.resulting_operator_epoch, 2);
        assert_eq!(replaced.revoked_sessions, 1);
        assert!(
            block_on(service.authenticate(Some(first.credential().expose_to_cookie()), NOW + 3,))
                .is_err()
        );

        let second = block_on(service.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.20",
            NOW + 4,
        ))
        .expect("second session");
        assert!(
            !second
                .profile
                .permissions
                .contains(&OperatorPermission::WaitlistReview)
        );
        let revoked = block_on(entitlements.apply_change(
            OperatorEntitlementChangeKind::Revoke,
            &identity(),
            [],
            "remove operator access after protected review",
            "request_revoke001",
            "idempotency-revoke-001",
            NOW + 5,
            86_400,
        ))
        .expect("revoke");
        assert!(revoked.entitlement.is_none());
        assert_eq!(revoked.resulting_operator_epoch, 3);
        assert_eq!(revoked.revoked_sessions, 1);
        assert!(
            block_on(service.authenticate(Some(second.credential().expose_to_cookie()), NOW + 6,))
                .is_err()
        );
    }

    fn bootstrap_input() -> OperatorBootstrapInput {
        OperatorBootstrapInput {
            target_email: "operator@example.test".to_owned(),
            permissions: BTreeSet::from([
                OperatorPermission::TenantRead,
                OperatorPermission::ProvisioningRepair,
                OperatorPermission::QuotaOverride,
                OperatorPermission::AbuseResponse,
                OperatorPermission::SupportAccess,
                OperatorPermission::WaitlistReview,
            ]),
            private_reason: "initial public beta operator bootstrap".to_owned(),
            environment_binding: "https://cloud.example.test".to_owned(),
            idempotency_key: "operator-bootstrap-idempotency-0001".to_owned(),
            activate_waitlisted: false,
        }
    }

    #[test]
    fn bootstrap_requires_exact_confirmation_and_is_atomic_and_replay_safe() {
        let (authentication, adapter, audit) =
            authentication_service(DeveloperIdentityStatus::Waitlisted, false, |_| {});
        let bootstrap = authentication.bootstrap_service();
        let input = bootstrap_input();
        let plan = block_on(bootstrap.plan(&input)).expect("plan");
        assert_eq!(plan.lifecycle_result, "unchanged");
        assert_eq!(
            plan.developer_status_before,
            Some(DeveloperIdentityStatus::Waitlisted)
        );
        assert_eq!(
            plan.developer_status_after,
            Some(DeveloperIdentityStatus::Waitlisted)
        );
        assert!(!format!("{input:?}").contains("operator@example.test"));
        assert!(!format!("{input:?}").contains(&input.private_reason));
        assert!(matches!(
            block_on(bootstrap.apply(&input, "wrong confirmation", NOW)),
            Err(OperatorPasswordAuthenticationError::ConfirmationMismatch)
        ));
        assert!(
            block_on(authentication.store().get_entitlement(&identity()))
                .expect("entitlement read")
                .is_none()
        );

        adapter
            .fail_next(
                FailurePoint::BeforeCommit,
                StorageError::new(
                    StorageErrorKind::Io,
                    "bootstrap-test",
                    true,
                    "injected failure",
                ),
            )
            .expect("inject failure");
        assert!(block_on(bootstrap.apply(&input, &plan.typed_confirmation, NOW)).is_err());
        let unchanged = block_on(authentication.developers.store().get_account(&identity()))
            .expect("account read")
            .expect("account");
        assert_eq!(unchanged.status(), DeveloperIdentityStatus::Waitlisted);
        assert!(
            block_on(authentication.store().get_entitlement(&identity()))
                .expect("entitlement read")
                .is_none()
        );

        let applied = block_on(bootstrap.apply(&input, &plan.typed_confirmation, NOW))
            .expect("bootstrap apply");
        assert_eq!(applied.lifecycle_result, "unchanged");
        assert_eq!(applied.operator_epoch, 1);
        assert!(!applied.replayed);
        let replay = block_on(bootstrap.apply(&input, &plan.typed_confirmation, NOW + 1))
            .expect("bootstrap replay");
        assert!(replay.replayed);
        let still_waitlisted = block_on(authentication.developers.store().get_account(&identity()))
            .expect("account read")
            .expect("account");
        assert_eq!(
            still_waitlisted.status(),
            DeveloperIdentityStatus::Waitlisted
        );
        let entitlement = block_on(authentication.store().get_entitlement(&identity()))
            .expect("entitlement read")
            .expect("entitlement");
        assert_eq!(entitlement.permissions(), &input.permissions);
        let events = audit.0.lock().expect("audit");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.classification == "initial_bootstrap")
                .count(),
            1
        );
        assert!(!format!("{events:?}").contains("operator@example.test"));
        assert!(!format!("{events:?}").contains(&input.private_reason));
    }

    #[test]
    fn bootstrap_rejects_unverified_and_permission_mismatched_plans() {
        let (unverified, _, _) =
            authentication_service(DeveloperIdentityStatus::Unverified, false, |_| {});
        assert!(matches!(
            block_on(unverified.bootstrap_service().plan(&bootstrap_input())),
            Err(OperatorPasswordAuthenticationError::Ineligible)
        ));

        let (authentication, _, _) =
            authentication_service(DeveloperIdentityStatus::Waitlisted, false, |_| {});
        let bootstrap = authentication.bootstrap_service();
        let input = bootstrap_input();
        let plan = block_on(bootstrap.plan(&input)).expect("plan");
        let mut changed_permissions = input.clone();
        changed_permissions
            .permissions
            .remove(&OperatorPermission::SupportAccess);
        assert!(matches!(
            block_on(bootstrap.apply(&changed_permissions, &plan.typed_confirmation, NOW)),
            Err(OperatorPasswordAuthenticationError::ConfirmationMismatch)
        ));

        let previous = block_on(authentication.developers.store().get_account(&identity()))
            .expect("account read")
            .expect("account");
        let mut active = previous.clone();
        active
            .transition(DeveloperIdentityStatus::Active, NOW)
            .expect("activation");
        block_on(
            authentication
                .developers
                .store()
                .replace_account(&previous, &active),
        )
        .expect("external lifecycle change");
        let applied = block_on(bootstrap.apply(&input, &plan.typed_confirmation, NOW + 1))
            .expect("developer lifecycle does not stale operator bootstrap");
        assert_eq!(
            applied.developer_status_before,
            Some(DeveloperIdentityStatus::Active)
        );
        assert!(
            block_on(authentication.store().get_entitlement(&identity()))
                .expect("entitlement read")
                .is_some()
        );
    }

    fn seed_combined_bootstrap_provenance(
        service: &OperatorAuthenticationService,
        idempotency_key: &str,
    ) -> OperatorIdempotencyRecord {
        let record = OperatorIdempotencyRecord::new(
            service
                .store()
                .authentication_key()
                .idempotency_digest(idempotency_key),
            "a".repeat(64),
            "b".repeat(64),
            identity(),
            1,
            "operator_bootstrap",
            NOW - 5,
            NOW + 365 * 24 * 60 * 60,
        )
        .expect("bootstrap provenance");
        block_on(service.store().create_idempotency(&record)).expect("seed provenance");
        record
    }

    fn bootstrap_developer_admission_repair_input() -> BootstrapDeveloperAdmissionRepairInput {
        BootstrapDeveloperAdmissionRepairInput {
            target_email: "operator@example.test".to_owned(),
            private_reason: "undo only the former combined bootstrap admission side effect"
                .to_owned(),
            environment_binding: "https://cloud.example.test".to_owned(),
            idempotency_key: "developer-admission-repair-0001".to_owned(),
            prior_bootstrap_idempotency_key: "combined-bootstrap-provenance-0001".to_owned(),
        }
    }

    #[test]
    fn provenance_bound_repair_changes_only_developer_role_and_replays_once() {
        let (authentication, adapter, audit) =
            authentication_service(DeveloperIdentityStatus::Active, true, |_| {});
        let input = bootstrap_developer_admission_repair_input();
        seed_combined_bootstrap_provenance(&authentication, &input.prior_bootstrap_idempotency_key);
        let grant = block_on(authentication.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.40",
            NOW,
        ))
        .expect("operator sign in");
        let session_digest = authentication
            .store()
            .authentication_key()
            .session_digest(grant.credential());
        let session_key =
            ControlKeyspace::operator_session_key(&session_digest).expect("session key");
        let session_before = block_on(adapter.get(&session_key))
            .expect("session read")
            .expect("session");
        let identity_before = block_on(
            authentication
                .developers
                .store()
                .get_authentication_identity(&identity()),
        )
        .expect("identity read")
        .expect("identity");
        let entitlement_before = block_on(authentication.store().get_entitlement(&identity()))
            .expect("entitlement read")
            .expect("entitlement");
        let service = authentication.bootstrap_developer_admission_repair_service();
        let plan = block_on(service.plan(&input)).expect("repair plan");
        assert_eq!(
            plan.developer_status_before,
            DeveloperIdentityStatus::Active
        );
        assert_eq!(
            plan.developer_status_after,
            DeveloperIdentityStatus::Waitlisted
        );
        assert_eq!(plan.operator_status_before, "active");
        assert_eq!(plan.operator_status_after, "active");
        assert!(matches!(
            block_on(service.apply(&input, "wrong confirmation", NOW + 1)),
            Err(OperatorPasswordAuthenticationError::ConfirmationMismatch)
        ));
        assert_eq!(
            block_on(authentication.developers.store().get_account(&identity()))
                .expect("account read")
                .expect("account")
                .status(),
            DeveloperIdentityStatus::Active
        );

        let applied = block_on(service.apply(&input, &plan.typed_confirmation, NOW + 1))
            .expect("repair apply");
        assert!(!applied.replayed);
        let repaired = block_on(authentication.developers.store().get_account(&identity()))
            .expect("account read")
            .expect("account");
        assert_eq!(repaired.status(), DeveloperIdentityStatus::Waitlisted);
        assert_eq!(repaired.review(), None);
        assert_eq!(
            repaired.credential_epoch(),
            identity_before.credential_epoch()
        );
        assert_eq!(
            block_on(authentication.store().get_entitlement(&identity()))
                .expect("entitlement read")
                .expect("entitlement"),
            entitlement_before
        );
        assert_eq!(
            block_on(adapter.get(&session_key))
                .expect("session read")
                .expect("session"),
            session_before
        );
        block_on(authentication.authenticate(Some(grant.credential().expose_to_cookie()), NOW + 2))
            .expect("operator session survives repair");
        let replay = block_on(service.apply(&input, &plan.typed_confirmation, NOW + 3))
            .expect("repair replay");
        assert!(replay.replayed);
        assert_eq!(
            audit
                .0
                .lock()
                .expect("audit")
                .iter()
                .filter(|event| {
                    event.classification == "combined_bootstrap_developer_admission_repair"
                })
                .count(),
            1
        );
    }

    #[test]
    fn repair_refuses_missing_provenance_and_subsequently_reviewed_developer() {
        let (missing, _, _) = authentication_service(DeveloperIdentityStatus::Active, true, |_| {});
        let input = bootstrap_developer_admission_repair_input();
        assert!(matches!(
            block_on(
                missing
                    .bootstrap_developer_admission_repair_service()
                    .plan(&input)
            ),
            Err(OperatorPasswordAuthenticationError::Ineligible)
        ));

        let (reviewed, _, _) =
            authentication_service(DeveloperIdentityStatus::Waitlisted, true, |_| {});
        seed_combined_bootstrap_provenance(&reviewed, &input.prior_bootstrap_idempotency_key);
        let previous = block_on(reviewed.developers.store().get_account(&identity()))
            .expect("account read")
            .expect("account");
        let mut next = previous.clone();
        let decision = crate::DeveloperDecisionRecord::new(
            identity(),
            crate::DeveloperDecisionKind::Approve,
            "opr_reviewer01",
            "approved through normal review",
            "request_reviewed01",
            "c".repeat(64),
            "d".repeat(64),
            previous.authorization_epoch() + 1,
            NOW + 1,
            NOW + 100,
        )
        .expect("decision");
        next.record_decision(&decision).expect("record decision");
        block_on(
            reviewed
                .developers
                .store()
                .replace_account(&previous, &next),
        )
        .expect("commit review");
        assert!(matches!(
            block_on(
                reviewed
                    .bootstrap_developer_admission_repair_service()
                    .plan(&input)
            ),
            Err(OperatorPasswordAuthenticationError::Ineligible)
        ));
    }

    #[test]
    fn protected_entitlement_admin_plans_exact_grants_replacements_and_revocation() {
        let (authentication, _, _) =
            authentication_service(DeveloperIdentityStatus::Active, false, |_| {});
        let service = authentication.entitlement_admin_service();
        let grant = OperatorEntitlementAdminInput {
            kind: OperatorEntitlementChangeKind::Grant,
            target_email: "operator@example.test".to_owned(),
            permissions: BTreeSet::from([
                OperatorPermission::TenantRead,
                OperatorPermission::WaitlistReview,
            ]),
            private_reason: "protected initial least privilege grant".to_owned(),
            environment_binding: "https://cloud.example.test".to_owned(),
            idempotency_key: "entitlement-admin-grant-0001".to_owned(),
        };
        let plan = block_on(service.plan(&grant)).expect("grant plan");
        assert!(matches!(
            block_on(service.apply(&grant, "stale", "request_admin01", NOW)),
            Err(OperatorPasswordAuthenticationError::ConfirmationMismatch)
        ));
        let applied =
            block_on(service.apply(&grant, &plan.typed_confirmation, "request_admin01", NOW))
                .expect("grant apply");
        assert_eq!(applied.operator_epoch, 1);
        assert!(!applied.replayed);
        let replay =
            block_on(service.apply(&grant, &plan.typed_confirmation, "request_admin02", NOW + 1))
                .expect("grant replay");
        assert!(replay.replayed);

        let replace = OperatorEntitlementAdminInput {
            kind: OperatorEntitlementChangeKind::Replace,
            permissions: BTreeSet::from([OperatorPermission::TenantRead]),
            idempotency_key: "entitlement-admin-replace-01".to_owned(),
            private_reason: "protected least privilege reduction".to_owned(),
            ..grant.clone()
        };
        let replace_plan = block_on(service.plan(&replace)).expect("replace plan");
        let replaced = block_on(service.apply(
            &replace,
            &replace_plan.typed_confirmation,
            "request_admin03",
            NOW + 2,
        ))
        .expect("replace apply");
        assert_eq!(replaced.operator_epoch, 2);

        let revoke = OperatorEntitlementAdminInput {
            kind: OperatorEntitlementChangeKind::Revoke,
            permissions: BTreeSet::new(),
            idempotency_key: "entitlement-admin-revoke-001".to_owned(),
            private_reason: "protected complete entitlement revocation".to_owned(),
            ..grant
        };
        let revoke_plan = block_on(service.plan(&revoke)).expect("revoke plan");
        let revoked = block_on(service.apply(
            &revoke,
            &revoke_plan.typed_confirmation,
            "request_admin04",
            NOW + 3,
        ))
        .expect("revoke apply");
        assert_eq!(revoked.operator_epoch, 3);
        assert!(
            block_on(authentication.store().get_entitlement(&identity()))
                .expect("entitlement read")
                .is_none()
        );
    }

    #[test]
    fn concurrent_bootstrap_apply_commits_one_atomic_entitlement() {
        let (authentication, _, audit) =
            authentication_service(DeveloperIdentityStatus::Waitlisted, false, |_| {});
        let bootstrap = authentication.bootstrap_service();
        let input = bootstrap_input();
        let plan = block_on(bootstrap.plan(&input)).expect("plan");
        let (left, right) = block_on(async {
            futures::join!(
                bootstrap.apply(&input, &plan.typed_confirmation, NOW),
                bootstrap.apply(&input, &plan.typed_confirmation, NOW),
            )
        });
        assert!(left.is_ok() || right.is_ok());
        assert!(
            left.is_err()
                || right.is_err()
                || left.as_ref().is_ok_and(|value| value.replayed)
                || right.as_ref().is_ok_and(|value| value.replayed)
        );
        let account = block_on(authentication.developers.store().get_account(&identity()))
            .expect("account read")
            .expect("account");
        assert_eq!(account.status(), DeveloperIdentityStatus::Waitlisted);
        let entitlement = block_on(authentication.store().get_entitlement(&identity()))
            .expect("entitlement read")
            .expect("entitlement");
        assert_eq!(entitlement.operator_epoch(), 1);
        assert_eq!(
            audit
                .0
                .lock()
                .expect("audit")
                .iter()
                .filter(|event| event.classification == "initial_bootstrap")
                .count(),
            1
        );
    }

    #[test]
    fn concurrent_attempt_updates_preserve_the_bounded_counter() {
        let (service, _, _) =
            authentication_service(DeveloperIdentityStatus::Active, true, |config| {
                config.source_attempts_per_window = 100;
                config.identity_attempts_per_window = 100;
            });
        let attempts = (0..4).map(|_| {
            service.sign_in(
                "operator@example.test",
                "wrong horse battery staple",
                "198.51.100.21",
                NOW,
            )
        });
        let results = block_on(futures::future::join_all(attempts));
        assert!(results.iter().all(|result| matches!(
            result,
            Err(OperatorPasswordAuthenticationError::InvalidCredentials)
                | Err(OperatorPasswordAuthenticationError::RateLimited { .. })
        )));
        let source_digest = service
            .store()
            .authentication_key()
            .attempt_digest("source", "198.51.100.21");
        let identity_digest = service
            .store()
            .authentication_key()
            .attempt_digest("identity", "operator@example.test");
        assert_eq!(
            block_on(service.store().get_attempt(&source_digest))
                .expect("source attempt")
                .expect("source record")
                .count(),
            4
        );
        assert_eq!(
            block_on(service.store().get_attempt(&identity_digest))
                .expect("identity attempt")
                .expect("identity record")
                .count(),
            4
        );
    }

    #[test]
    fn developer_security_epoch_change_invalidates_operator_session_atomically() {
        let (service, _, _) = authentication_service(DeveloperIdentityStatus::Active, true, |_| {});
        let grant = block_on(service.sign_in(
            "operator@example.test",
            "correct horse battery staple",
            "198.51.100.22",
            NOW,
        ))
        .expect("sign in");
        let previous = block_on(service.developers.store().get_account(&identity()))
            .expect("read")
            .expect("account");
        let mut changed = previous.clone();
        changed
            .replace_password_hash(previous.password_hash().expect("hash"), NOW + 1)
            .expect("security epoch");
        block_on(
            service
                .developers
                .store()
                .replace_account(&previous, &changed),
        )
        .expect("commit epoch");
        assert!(
            block_on(service.authenticate(Some(grant.credential().expose_to_cookie()), NOW + 2,))
                .is_err()
        );
    }
}
