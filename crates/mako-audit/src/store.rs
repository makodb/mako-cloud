use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter, KvSnapshot,
    ScanDirection, ScanRequest, StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{
    ActorIdentity, AuditEvent, AuditOutcome, ObservabilityEvent, RedactionError, RetentionMode,
    SignalScope, TelemetryRedactor,
};

const CURSOR_VERSION: u8 = 1;
const EVENT_DOMAIN_PREFIX: &str = "mako:audit:event:v1";
const ID_DOMAIN_PREFIX: &str = "mako:audit:id:v1";
const EXPORT_CONTENT_TYPE: &str = "application/x-ndjson";

/// Origin of an audit event. These are the security-sensitive producer classes
/// that share this append-only storage boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditCategory {
    Control,
    #[serde(rename = "auth")]
    Authentication,
    Policy,
    ServiceBypass,
    Function,
    Operator,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditActorKind {
    ApplicationUser,
    Developer,
    Service,
    Operator,
    System,
    Anonymous,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditFilter {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub categories: BTreeSet<AuditCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_kind: Option<AuditActorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AuditOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
}

impl AuditFilter {
    fn validate(&self) -> Result<(), AuditStoreError> {
        for value in [
            self.actor_id.as_deref(),
            self.action.as_deref(),
            self.resource_kind.as_deref(),
            self.resource_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
                return Err(AuditStoreError::InvalidQuery);
            }
        }
        Ok(())
    }

    fn matches(&self, record: &AuditRecord) -> bool {
        if !self.categories.is_empty() && !self.categories.contains(&record.category) {
            return false;
        }
        let (actor_kind, actor_id) = actor_parts(record.event.context.actor());
        if self
            .actor_kind
            .is_some_and(|expected| expected != actor_kind)
            || self
                .actor_id
                .as_deref()
                .is_some_and(|expected| actor_id != Some(expected))
            || self
                .action
                .as_deref()
                .is_some_and(|expected| record.event.action != expected)
            || self
                .outcome
                .is_some_and(|expected| record.event.outcome != expected)
            || self
                .resource_kind
                .as_deref()
                .is_some_and(|expected| record.event.context.resource().kind() != expected)
            || self
                .resource_id
                .as_deref()
                .is_some_and(|expected| record.event.context.resource().id() != expected)
        {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    pub category: AuditCategory,
    pub event: AuditEvent,
}

impl AuditRecord {
    fn validate_for(&self, tenant: &TenantScope) -> Result<(), AuditStoreError> {
        self.event
            .validate()
            .map_err(|_| AuditStoreError::InvalidEvent)?;
        match self.event.context.scope() {
            SignalScope::Tenant {
                tenant: event_tenant,
                organization_id: Some(_),
            } if event_tenant == tenant => Ok(()),
            SignalScope::Tenant {
                tenant: event_tenant,
                organization_id: None,
            } if event_tenant == tenant => Err(AuditStoreError::OrganizationScopeRequired),
            SignalScope::Tenant { .. } => Err(AuditStoreError::TenantMismatch),
            SignalScope::Organization { .. } | SignalScope::Platform => {
                Err(AuditStoreError::TenantScopeRequired)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    Created,
    AlreadyPresent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditPage {
    pub records: Vec<AuditRecord>,
    pub next_cursor: Option<String>,
    pub examined_records: usize,
    pub retained_from_unix_milliseconds: u64,
    pub observed_at_unix_milliseconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditExport {
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub record_count: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub retained_from_unix_milliseconds: u64,
    pub observed_at_unix_milliseconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditExportRequest {
    pub filter: AuditFilter,
    pub from_unix_milliseconds: u64,
    pub until_unix_milliseconds: Option<u64>,
    pub max_records: NonZeroUsize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditCompactionReport {
    pub retained_from_unix_milliseconds: u64,
    pub dry_run: bool,
    pub events_eligible: usize,
    pub events_removed: usize,
}

/// A key dedicated to signing opaque audit cursors. Debug formatting never
/// exposes the bytes.
#[derive(Clone)]
pub struct CursorSigningKey([u8; 32]);

impl CursorSigningKey {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for CursorSigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CursorSigningKey([REDACTED])")
    }
}

#[derive(Clone, Debug)]
pub struct AuditStoreConfig {
    pub durability: Durability,
    pub retention_milliseconds: u64,
    pub maximum_page_records: NonZeroUsize,
    pub maximum_export_records: NonZeroUsize,
    pub maximum_examined_records: NonZeroUsize,
    pub cursor_signing_key: CursorSigningKey,
}

impl AuditStoreConfig {
    fn validate(&self) -> Result<(), AuditStoreError> {
        if self.retention_milliseconds == 0
            || self.maximum_page_records > self.maximum_export_records
            || self.maximum_examined_records < self.maximum_page_records
        {
            return Err(AuditStoreError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AuditStore {
    adapter: Arc<dyn KvAdapter>,
    config: AuditStoreConfig,
}

impl AuditStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        config: AuditStoreConfig,
    ) -> Result<Self, AuditStoreError> {
        config.validate()?;
        if adapter.capabilities().maximum_batch_operations.get() < 2
            || adapter.capabilities().strongest_durability < config.durability
        {
            return Err(AuditStoreError::InvalidConfiguration);
        }
        Ok(Self { adapter, config })
    }

    /// Appends a sanitized event. An identical retry is idempotent; reusing an
    /// event ID for different bytes is rejected. No mutation or deletion API is
    /// provided by this store.
    pub async fn append(
        &self,
        tenant: &TenantScope,
        category: AuditCategory,
        event: AuditEvent,
        redactor: &TelemetryRedactor,
    ) -> Result<AppendOutcome, AuditStoreError> {
        let mut wrapped = ObservabilityEvent::Audit(event);
        redactor.sanitize_event(&mut wrapped)?;
        let ObservabilityEvent::Audit(event) = wrapped else {
            return Err(AuditStoreError::InvalidEvent);
        };
        let record = AuditRecord { category, event };
        record.validate_for(tenant)?;
        let bytes = serde_json::to_vec(&record).map_err(|_| AuditStoreError::Serialization)?;
        let digest = blake3::hash(&bytes).as_bytes().to_vec();
        let event_key = event_key(tenant, &record.event)?;
        let id_key = id_key(tenant, &record.event.event_id)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&event_key, &bytes).put(&id_key, &digest);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::Missing {
                        key: id_key.clone(),
                    },
                    KeyCondition::Missing {
                        key: event_key.clone(),
                    },
                ],
                batch,
                durability: self.config.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(AppendOutcome::Created),
            CompareAndWriteResult::Conflict {
                failed_condition: 0,
                actual_value: Some(actual),
            } if constant_time_eq(&actual, &digest) => Ok(AppendOutcome::AlreadyPresent),
            CompareAndWriteResult::Conflict {
                failed_condition: 0,
                ..
            } => Err(AuditStoreError::EventIdConflict),
            CompareAndWriteResult::Conflict { .. } => Err(AuditStoreError::CorruptStore),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn query(
        &self,
        tenant: &TenantScope,
        filter: &AuditFilter,
        from_unix_milliseconds: u64,
        until_unix_milliseconds: Option<u64>,
        limit: NonZeroUsize,
        cursor: Option<&str>,
        now_unix_milliseconds: u64,
    ) -> Result<AuditPage, AuditStoreError> {
        filter.validate()?;
        if limit > self.config.maximum_page_records {
            return Err(AuditStoreError::InvalidQuery);
        }
        let retained_from = self.validate_window(
            from_unix_milliseconds,
            until_unix_milliseconds,
            now_unix_milliseconds,
        )?;
        let fingerprint = query_fingerprint(
            tenant,
            filter,
            from_unix_milliseconds,
            until_unix_milliseconds,
        )?;
        let domain = event_domain(tenant);
        let range = TenantKeyspace::system_domain_range(&domain)
            .map_err(|_| AuditStoreError::InvalidQuery)?;
        let cursor_payload = cursor
            .map(|value| self.decode_cursor(value, tenant, &fingerprint, retained_from))
            .transpose()?;
        let start = cursor_payload.map_or_else(
            || {
                TenantKeyspace::system_key(
                    &domain,
                    format!("{from_unix_milliseconds:020}/").as_bytes(),
                )
                .map_err(|_| AuditStoreError::InvalidQuery)
            },
            |payload| {
                if payload.last_occurred_at_unix_milliseconds < retained_from {
                    return Err(AuditStoreError::CursorExpired);
                }
                let mut next = URL_SAFE_NO_PAD
                    .decode(payload.last_key)
                    .map_err(|_| AuditStoreError::InvalidCursor)?;
                if !range.contains(&next) {
                    return Err(AuditStoreError::InvalidCursor);
                }
                next.push(0);
                Ok(next)
            },
        )?;
        if start >= range.end_exclusive {
            return Ok(AuditPage {
                records: Vec::new(),
                next_cursor: None,
                examined_records: 0,
                retained_from_unix_milliseconds: retained_from,
                observed_at_unix_milliseconds: now_unix_milliseconds,
            });
        }
        let snapshot = self.adapter.snapshot().await?;
        self.scan_page(
            snapshot.as_ref(),
            tenant,
            filter,
            fingerprint,
            KeyRange::new(start, range.end_exclusive).map_err(|_| AuditStoreError::InvalidQuery)?,
            from_unix_milliseconds,
            Some(
                until_unix_milliseconds
                    .unwrap_or(now_unix_milliseconds)
                    .min(now_unix_milliseconds),
            ),
            limit,
            retained_from,
            now_unix_milliseconds,
        )
        .await
    }

    pub async fn export(
        &self,
        tenant: &TenantScope,
        request: &AuditExportRequest,
        now_unix_milliseconds: u64,
    ) -> Result<AuditExport, AuditStoreError> {
        if request.max_records > self.config.maximum_export_records {
            return Err(AuditStoreError::InvalidQuery);
        }
        let retained_from = self.validate_window(
            request.from_unix_milliseconds,
            request.until_unix_milliseconds,
            now_unix_milliseconds,
        )?;
        let mut cursor = request.cursor.clone();
        let mut records = Vec::new();
        let mut remaining = request.max_records.get();
        let mut examined = 0;
        let truncated = loop {
            let page_limit = remaining.min(self.config.maximum_page_records.get());
            let page = self
                .query(
                    tenant,
                    &request.filter,
                    request.from_unix_milliseconds,
                    request.until_unix_milliseconds,
                    NonZeroUsize::new(page_limit).ok_or(AuditStoreError::InvalidQuery)?,
                    cursor.as_deref(),
                    now_unix_milliseconds,
                )
                .await?;
            examined += page.examined_records;
            remaining -= page.records.len();
            records.extend(page.records);
            cursor = page.next_cursor;
            if cursor.is_none() || remaining == 0 {
                break cursor.is_some();
            }
            if examined >= self.config.maximum_examined_records.get() {
                break true;
            }
        };
        let mut body = Vec::new();
        for record in &records {
            serde_json::to_writer(&mut body, record).map_err(|_| AuditStoreError::Serialization)?;
            body.push(b'\n');
        }
        Ok(AuditExport {
            content_type: EXPORT_CONTENT_TYPE,
            body,
            record_count: records.len(),
            truncated,
            next_cursor: cursor,
            retained_from_unix_milliseconds: retained_from,
            observed_at_unix_milliseconds: now_unix_milliseconds,
        })
    }

    /// Removes expired audit detail while retaining event-ID guards so an old
    /// retry cannot recreate an event or reuse its identifier for new content.
    pub async fn compact_expired(
        &self,
        tenant: &TenantScope,
        now_unix_milliseconds: u64,
        mode: RetentionMode,
    ) -> Result<AuditCompactionReport, AuditStoreError> {
        if now_unix_milliseconds == 0 {
            return Err(AuditStoreError::InvalidQuery);
        }
        let retained_from =
            now_unix_milliseconds.saturating_sub(self.config.retention_milliseconds);
        let domain = event_domain(tenant);
        let domain_range = TenantKeyspace::system_domain_range(&domain)
            .map_err(|_| AuditStoreError::InvalidQuery)?;
        let end = TenantKeyspace::system_key(&domain, format!("{retained_from:020}/").as_bytes())
            .map_err(|_| AuditStoreError::InvalidQuery)?;
        if end <= domain_range.start_inclusive {
            return Ok(AuditCompactionReport {
                retained_from_unix_milliseconds: retained_from,
                dry_run: mode == RetentionMode::DryRun,
                events_eligible: 0,
                events_removed: 0,
            });
        }
        let mut range = KeyRange::new(domain_range.start_inclusive, end)
            .map_err(|_| AuditStoreError::InvalidQuery)?;
        let scan_limit = self.adapter.capabilities().maximum_scan_items;
        let maximum_batch = self.adapter.capabilities().maximum_batch_operations.get();
        let mut eligible = 0;
        let mut removed = 0;
        loop {
            let entries = self
                .adapter
                .scan(ScanRequest::new(
                    range.clone(),
                    ScanDirection::Forward,
                    scan_limit,
                ))
                .await?;
            if entries.is_empty() {
                break;
            }
            for entry in &entries {
                let record: AuditRecord = serde_json::from_slice(&entry.value)
                    .map_err(|_| AuditStoreError::CorruptStore)?;
                record.validate_for(tenant)?;
                if record.event.occurred_at_unix_milliseconds >= retained_from {
                    return Err(AuditStoreError::CorruptStore);
                }
            }
            eligible += entries.len();
            if mode == RetentionMode::Apply {
                for chunk in entries.chunks(maximum_batch) {
                    let mut batch = WriteBatch::with_capacity(chunk.len());
                    for entry in chunk {
                        batch.delete(&entry.key);
                    }
                    self.adapter.write(batch, self.config.durability).await?;
                    removed += chunk.len();
                }
            }
            let full_page = entries.len() == scan_limit.get();
            let mut next = entries
                .last()
                .ok_or(AuditStoreError::CorruptStore)?
                .key
                .clone();
            next.push(0);
            if !full_page || next >= range.end_exclusive {
                break;
            }
            range.start_inclusive = next;
        }
        Ok(AuditCompactionReport {
            retained_from_unix_milliseconds: retained_from,
            dry_run: mode == RetentionMode::DryRun,
            events_eligible: eligible,
            events_removed: removed,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn scan_page(
        &self,
        snapshot: &dyn KvSnapshot,
        tenant: &TenantScope,
        filter: &AuditFilter,
        fingerprint: String,
        mut range: KeyRange,
        from: u64,
        until: Option<u64>,
        limit: NonZeroUsize,
        retained_from: u64,
        now: u64,
    ) -> Result<AuditPage, AuditStoreError> {
        let adapter_limit = self.adapter.capabilities().maximum_scan_items.get();
        let chunk_size = adapter_limit.min(256);
        let mut examined = 0;
        let mut records = Vec::new();
        let mut record_keys = Vec::new();
        let mut last_examined: Option<(Vec<u8>, u64)> = None;
        let mut reached_end = false;
        while examined < self.config.maximum_examined_records.get() && records.len() <= limit.get()
        {
            let remaining = self.config.maximum_examined_records.get() - examined;
            let requested = chunk_size.min(remaining);
            let entries = snapshot
                .scan(ScanRequest::new(
                    range.clone(),
                    ScanDirection::Forward,
                    NonZeroUsize::new(requested).ok_or(AuditStoreError::InvalidConfiguration)?,
                ))
                .await?;
            if entries.is_empty() {
                reached_end = true;
                break;
            }
            examined += entries.len();
            for entry in &entries {
                let record: AuditRecord = serde_json::from_slice(&entry.value)
                    .map_err(|_| AuditStoreError::CorruptStore)?;
                record.validate_for(tenant)?;
                let timestamp = record.event.occurred_at_unix_milliseconds;
                last_examined = Some((entry.key.clone(), timestamp));
                if timestamp < from {
                    continue;
                }
                if until.is_some_and(|upper| timestamp > upper) {
                    reached_end = true;
                    break;
                }
                if filter.matches(&record) {
                    record_keys.push(entry.key.clone());
                    records.push(record);
                    if records.len() > limit.get() {
                        break;
                    }
                }
            }
            if reached_end || records.len() > limit.get() {
                break;
            }
            if entries.len() < requested {
                reached_end = true;
                break;
            }
            let mut next = entries
                .last()
                .ok_or(AuditStoreError::CorruptStore)?
                .key
                .clone();
            next.push(0);
            if next >= range.end_exclusive {
                reached_end = true;
                break;
            }
            range.start_inclusive = next;
        }

        let has_extra_match = records.len() > limit.get();
        if has_extra_match {
            records.truncate(limit.get());
            record_keys.truncate(limit.get());
        }
        let scan_capped = !reached_end && examined >= self.config.maximum_examined_records.get();
        let continuation = if has_extra_match {
            record_keys.last().cloned().zip(
                records
                    .last()
                    .map(|record| record.event.occurred_at_unix_milliseconds),
            )
        } else if scan_capped {
            last_examined
        } else {
            None
        };
        let next_cursor = continuation
            .map(|(key, timestamp)| {
                self.encode_cursor(tenant, &fingerprint, key, timestamp, retained_from)
            })
            .transpose()?;
        Ok(AuditPage {
            records,
            next_cursor,
            examined_records: examined,
            retained_from_unix_milliseconds: retained_from,
            observed_at_unix_milliseconds: now,
        })
    }

    fn validate_window(
        &self,
        from: u64,
        until: Option<u64>,
        now: u64,
    ) -> Result<u64, AuditStoreError> {
        if from == 0 || now == 0 || until.is_some_and(|upper| upper < from) {
            return Err(AuditStoreError::InvalidQuery);
        }
        let retained_from = now.saturating_sub(self.config.retention_milliseconds);
        if from < retained_from {
            return Err(AuditStoreError::RetentionWindowExpired);
        }
        Ok(retained_from)
    }

    fn encode_cursor(
        &self,
        tenant: &TenantScope,
        fingerprint: &str,
        last_key: Vec<u8>,
        last_occurred_at_unix_milliseconds: u64,
        retained_from_unix_milliseconds: u64,
    ) -> Result<String, AuditStoreError> {
        let payload = CursorPayload {
            version: CURSOR_VERSION,
            project_id: tenant.project_id().as_str().to_owned(),
            environment_id: tenant.environment_id().as_str().to_owned(),
            fingerprint: fingerprint.to_owned(),
            last_key: URL_SAFE_NO_PAD.encode(last_key),
            last_occurred_at_unix_milliseconds,
            retained_from_unix_milliseconds,
        };
        let bytes = serde_json::to_vec(&payload).map_err(|_| AuditStoreError::Serialization)?;
        let signature = blake3::keyed_hash(&self.config.cursor_signing_key.0, &bytes);
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(bytes),
            URL_SAFE_NO_PAD.encode(signature.as_bytes())
        ))
    }

    fn decode_cursor(
        &self,
        cursor: &str,
        tenant: &TenantScope,
        fingerprint: &str,
        retained_from: u64,
    ) -> Result<CursorPayload, AuditStoreError> {
        let (payload, signature) = cursor
            .split_once('.')
            .ok_or(AuditStoreError::InvalidCursor)?;
        if signature.contains('.') {
            return Err(AuditStoreError::InvalidCursor);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| AuditStoreError::InvalidCursor)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| AuditStoreError::InvalidCursor)?;
        let expected = blake3::keyed_hash(&self.config.cursor_signing_key.0, &bytes);
        if !constant_time_eq(&signature, expected.as_bytes()) {
            return Err(AuditStoreError::InvalidCursor);
        }
        let payload: CursorPayload =
            serde_json::from_slice(&bytes).map_err(|_| AuditStoreError::InvalidCursor)?;
        if payload.version != CURSOR_VERSION
            || payload.project_id != tenant.project_id().as_str()
            || payload.environment_id != tenant.environment_id().as_str()
            || payload.fingerprint != fingerprint
        {
            return Err(AuditStoreError::InvalidCursor);
        }
        if payload.retained_from_unix_milliseconds < retained_from
            && payload.last_occurred_at_unix_milliseconds < retained_from
        {
            return Err(AuditStoreError::CursorExpired);
        }
        Ok(payload)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorPayload {
    version: u8,
    project_id: String,
    environment_id: String,
    fingerprint: String,
    last_key: String,
    last_occurred_at_unix_milliseconds: u64,
    retained_from_unix_milliseconds: u64,
}

#[derive(Debug)]
pub enum AuditStoreError {
    InvalidConfiguration,
    InvalidEvent,
    TenantScopeRequired,
    OrganizationScopeRequired,
    TenantMismatch,
    EventIdConflict,
    InvalidQuery,
    InvalidCursor,
    CursorExpired,
    RetentionWindowExpired,
    Serialization,
    CorruptStore,
    Redaction(RedactionError),
    Storage(StorageError),
}

impl fmt::Display for AuditStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "audit store configuration is invalid",
            Self::InvalidEvent => "audit event is invalid",
            Self::TenantScopeRequired => "tenant audit scope is required",
            Self::OrganizationScopeRequired => "organization audit scope is required",
            Self::TenantMismatch => "audit event tenant does not match the storage tenant",
            Self::EventIdConflict => "audit event identifier already has different content",
            Self::InvalidQuery => "audit query is invalid",
            Self::InvalidCursor => "audit cursor is invalid",
            Self::CursorExpired => "audit cursor is outside the retained window",
            Self::RetentionWindowExpired => "audit query begins before the retained window",
            Self::Serialization => "audit record serialization failed",
            Self::CorruptStore => "audit storage contains an invalid record",
            Self::Redaction(_) => "audit event redaction failed",
            Self::Storage(_) => "audit storage operation failed",
        })
    }
}

impl Error for AuditStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Redaction(error) => Some(error),
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RedactionError> for AuditStoreError {
    fn from(value: RedactionError) -> Self {
        Self::Redaction(value)
    }
}

impl From<StorageError> for AuditStoreError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

fn actor_parts(actor: &ActorIdentity) -> (AuditActorKind, Option<&str>) {
    match actor {
        ActorIdentity::ApplicationUser { actor_id, .. } => {
            (AuditActorKind::ApplicationUser, Some(actor_id))
        }
        ActorIdentity::Developer { actor_id } => (AuditActorKind::Developer, Some(actor_id)),
        ActorIdentity::Service { actor_id } => (AuditActorKind::Service, Some(actor_id)),
        ActorIdentity::Operator { actor_id, .. } => (AuditActorKind::Operator, Some(actor_id)),
        ActorIdentity::System { component } => (AuditActorKind::System, Some(component)),
        ActorIdentity::Anonymous => (AuditActorKind::Anonymous, None),
    }
}

fn event_domain(tenant: &TenantScope) -> Vec<u8> {
    format!(
        "{EVENT_DOMAIN_PREFIX}:{}:{}",
        tenant.project_id(),
        tenant.environment_id()
    )
    .into_bytes()
}

fn id_domain(tenant: &TenantScope) -> Vec<u8> {
    format!(
        "{ID_DOMAIN_PREFIX}:{}:{}",
        tenant.project_id(),
        tenant.environment_id()
    )
    .into_bytes()
}

fn event_key(tenant: &TenantScope, event: &AuditEvent) -> Result<Vec<u8>, AuditStoreError> {
    TenantKeyspace::system_key(
        event_domain(tenant),
        format!(
            "{:020}/{}",
            event.occurred_at_unix_milliseconds, event.event_id
        ),
    )
    .map_err(|_| AuditStoreError::InvalidEvent)
}

fn id_key(tenant: &TenantScope, event_id: &str) -> Result<Vec<u8>, AuditStoreError> {
    TenantKeyspace::system_key(id_domain(tenant), event_id)
        .map_err(|_| AuditStoreError::InvalidEvent)
}

fn query_fingerprint(
    tenant: &TenantScope,
    filter: &AuditFilter,
    from: u64,
    until: Option<u64>,
) -> Result<String, AuditStoreError> {
    #[derive(Serialize)]
    struct Fingerprint<'a> {
        tenant: &'a TenantScope,
        filter: &'a AuditFilter,
        from: u64,
        until: Option<u64>,
    }
    let bytes = serde_json::to_vec(&Fingerprint {
        tenant,
        filter,
        from,
        until,
    })
    .map_err(|_| AuditStoreError::Serialization)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{
        ActorIdentity, AttributeValue, CorrelationId, RequestId, ResourceReference, SafeAttributes,
        SignalContext,
    };

    const NOW: u64 = 20_000;

    #[test]
    fn append_is_immutable_idempotent_and_tenant_scoped() {
        block_on(async {
            let store = store();
            let tenant = test_tenant("example00", "example00");
            let other = test_tenant("other000", "other000");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            let event = audit_event(&tenant, "aud_control00", 11_000, "project.update");

            assert_eq!(
                store
                    .append(&tenant, AuditCategory::Control, event.clone(), &redactor)
                    .await
                    .expect("append"),
                AppendOutcome::Created
            );
            assert_eq!(
                store
                    .append(&tenant, AuditCategory::Control, event.clone(), &redactor)
                    .await
                    .expect("retry"),
                AppendOutcome::AlreadyPresent
            );
            let mut changed = event;
            changed.action = "project.delete".to_owned();
            assert!(matches!(
                store
                    .append(&tenant, AuditCategory::Control, changed, &redactor)
                    .await,
                Err(AuditStoreError::EventIdConflict)
            ));
            assert!(matches!(
                store
                    .append(
                        &other,
                        AuditCategory::Control,
                        audit_event(&tenant, "aud_wrong000", 12_000, "project.read"),
                        &redactor,
                    )
                    .await,
                Err(AuditStoreError::TenantMismatch)
            ));
        });
    }

    #[test]
    fn filters_paginate_in_order_and_cursors_are_bound_and_signed() {
        block_on(async {
            let store = store();
            let tenant = test_tenant("example00", "example00");
            let other = test_tenant("other000", "other000");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            for (category, id, timestamp, action) in [
                (
                    AuditCategory::Control,
                    "aud_control00",
                    11_000,
                    "project.update",
                ),
                (
                    AuditCategory::Authentication,
                    "aud_auth00000",
                    12_000,
                    "session.create",
                ),
                (
                    AuditCategory::Control,
                    "aud_control01",
                    13_000,
                    "project.delete",
                ),
            ] {
                store
                    .append(
                        &tenant,
                        category,
                        audit_event(&tenant, id, timestamp, action),
                        &redactor,
                    )
                    .await
                    .expect("append");
            }
            store
                .append(
                    &other,
                    AuditCategory::Control,
                    audit_event(&other, "aud_other000", 11_500, "project.update"),
                    &redactor,
                )
                .await
                .expect("other append");

            let filter = AuditFilter {
                categories: BTreeSet::from([AuditCategory::Control]),
                ..AuditFilter::default()
            };
            let first = store
                .query(
                    &tenant,
                    &filter,
                    10_000,
                    None,
                    NonZeroUsize::new(1).expect("non-zero"),
                    None,
                    NOW,
                )
                .await
                .expect("first page");
            assert_eq!(first.records.len(), 1);
            assert_eq!(first.records[0].event.event_id, "aud_control00");
            let cursor = first.next_cursor.expect("continuation");

            let second = store
                .query(
                    &tenant,
                    &filter,
                    10_000,
                    None,
                    NonZeroUsize::new(1).expect("non-zero"),
                    Some(&cursor),
                    NOW,
                )
                .await
                .expect("second page");
            assert_eq!(second.records.len(), 1);
            assert_eq!(second.records[0].event.event_id, "aud_control01");
            assert!(second.next_cursor.is_none());

            let mut tampered = cursor.clone();
            tampered.push('x');
            for result in [
                store
                    .query(
                        &tenant,
                        &filter,
                        10_000,
                        None,
                        NonZeroUsize::new(1).expect("non-zero"),
                        Some(&tampered),
                        NOW,
                    )
                    .await,
                store
                    .query(
                        &other,
                        &filter,
                        10_000,
                        None,
                        NonZeroUsize::new(1).expect("non-zero"),
                        Some(&cursor),
                        NOW,
                    )
                    .await,
            ] {
                assert!(matches!(result, Err(AuditStoreError::InvalidCursor)));
            }
            let different_filter = AuditFilter {
                categories: BTreeSet::from([AuditCategory::Authentication]),
                ..AuditFilter::default()
            };
            assert!(matches!(
                store
                    .query(
                        &tenant,
                        &different_filter,
                        10_000,
                        None,
                        NonZeroUsize::new(1).expect("non-zero"),
                        Some(&cursor),
                        NOW,
                    )
                    .await,
                Err(AuditStoreError::InvalidCursor)
            ));
        });
    }

    #[test]
    fn retention_rejects_old_windows_and_expired_cursors() {
        block_on(async {
            let store = store();
            let tenant = test_tenant("example00", "example00");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            for (id, timestamp) in [("aud_old00000", 11_000), ("aud_new00000", 16_000)] {
                store
                    .append(
                        &tenant,
                        AuditCategory::Operator,
                        audit_event(&tenant, id, timestamp, "operator.inspect"),
                        &redactor,
                    )
                    .await
                    .expect("append");
            }
            assert!(matches!(
                store
                    .query(
                        &tenant,
                        &AuditFilter::default(),
                        9_999,
                        None,
                        NonZeroUsize::new(1).expect("non-zero"),
                        None,
                        NOW,
                    )
                    .await,
                Err(AuditStoreError::RetentionWindowExpired)
            ));
            let first = store
                .query(
                    &tenant,
                    &AuditFilter::default(),
                    10_000,
                    None,
                    NonZeroUsize::new(1).expect("non-zero"),
                    None,
                    NOW,
                )
                .await
                .expect("page");
            let cursor = first.next_cursor.expect("cursor");
            assert!(matches!(
                store
                    .query(
                        &tenant,
                        &AuditFilter::default(),
                        15_000,
                        None,
                        NonZeroUsize::new(1).expect("non-zero"),
                        Some(&cursor),
                        25_000,
                    )
                    .await,
                Err(AuditStoreError::InvalidCursor | AuditStoreError::CursorExpired)
            ));
        });
    }

    #[test]
    fn export_is_bounded_ndjson_and_redacted_before_storage() {
        block_on(async {
            let store = store();
            let tenant = test_tenant("example00", "example00");
            let secret = "function-secret-canary";
            let redactor = TelemetryRedactor::new([secret]).expect("redactor");
            for index in 0..3 {
                let mut audit = audit_event(
                    &tenant,
                    &format!("aud_export{index:02}"),
                    11_000 + index,
                    "function.invoke",
                );
                audit.details = SafeAttributes::try_from_iter([
                    (
                        "document.body".to_owned(),
                        AttributeValue::Text("private document".to_owned()),
                    ),
                    (
                        "error.message".to_owned(),
                        AttributeValue::Text(format!("failed with {secret}")),
                    ),
                ])
                .expect("details");
                store
                    .append(&tenant, AuditCategory::Function, audit, &redactor)
                    .await
                    .expect("append");
            }
            let export = store
                .export(
                    &tenant,
                    &AuditExportRequest {
                        filter: AuditFilter::default(),
                        from_unix_milliseconds: 10_000,
                        until_unix_milliseconds: None,
                        max_records: NonZeroUsize::new(2).expect("non-zero"),
                        cursor: None,
                    },
                    NOW,
                )
                .await
                .expect("export");
            assert_eq!(export.content_type, EXPORT_CONTENT_TYPE);
            assert_eq!(export.record_count, 2);
            assert!(export.truncated);
            assert!(export.next_cursor.is_some());
            assert_eq!(export.body.iter().filter(|byte| **byte == b'\n').count(), 2);
            let body = String::from_utf8(export.body).expect("utf-8");
            assert!(!body.contains(secret));
            assert!(!body.contains("private document"));
            assert!(body.contains("[REDACTED]"));
        });
    }

    #[test]
    fn every_required_producer_category_is_filterable() {
        block_on(async {
            let store = store();
            let tenant = test_tenant("example00", "example00");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            let categories = [
                AuditCategory::Control,
                AuditCategory::Authentication,
                AuditCategory::Policy,
                AuditCategory::ServiceBypass,
                AuditCategory::Function,
                AuditCategory::Operator,
            ];
            for (index, category) in categories.into_iter().enumerate() {
                store
                    .append(
                        &tenant,
                        category,
                        audit_event(
                            &tenant,
                            &format!("aud_category{index}"),
                            11_000 + u64::try_from(index).expect("small index"),
                            "audit.category",
                        ),
                        &redactor,
                    )
                    .await
                    .expect("append");
                let page = store
                    .query(
                        &tenant,
                        &AuditFilter {
                            categories: BTreeSet::from([category]),
                            ..AuditFilter::default()
                        },
                        10_000,
                        None,
                        NonZeroUsize::new(1).expect("non-zero"),
                        None,
                        NOW,
                    )
                    .await
                    .expect("query");
                assert_eq!(page.records.len(), 1);
                assert_eq!(page.records[0].category, category);
            }
        });
    }

    #[test]
    fn audit_compaction_reports_dry_run_and_preserves_idempotency_guards() {
        block_on(async {
            let store = store();
            let tenant = test_tenant("example00", "example00");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            let expired = audit_event(&tenant, "aud_expired00", 9_000, "project.update");
            store
                .append(&tenant, AuditCategory::Control, expired.clone(), &redactor)
                .await
                .expect("expired append");
            store
                .append(
                    &tenant,
                    AuditCategory::Control,
                    audit_event(&tenant, "aud_retained00", 11_000, "project.read"),
                    &redactor,
                )
                .await
                .expect("retained append");

            let preview = store
                .compact_expired(&tenant, NOW, RetentionMode::DryRun)
                .await
                .expect("preview");
            assert!(preview.dry_run);
            assert_eq!(preview.events_eligible, 1);
            assert_eq!(preview.events_removed, 0);
            let applied = store
                .compact_expired(&tenant, NOW, RetentionMode::Apply)
                .await
                .expect("compact");
            assert!(!applied.dry_run);
            assert_eq!(applied.events_eligible, 1);
            assert_eq!(applied.events_removed, 1);
            assert_eq!(
                store
                    .append(&tenant, AuditCategory::Control, expired, &redactor)
                    .await
                    .expect("old retry"),
                AppendOutcome::AlreadyPresent
            );
            let page = store
                .query(
                    &tenant,
                    &AuditFilter::default(),
                    10_000,
                    None,
                    NonZeroUsize::new(2).expect("non-zero"),
                    None,
                    NOW,
                )
                .await
                .expect("query");
            assert_eq!(page.records.len(), 1);
            assert_eq!(page.records[0].event.event_id, "aud_retained00");
        });
    }

    fn store() -> AuditStore {
        AuditStore::new(
            Arc::new(MemoryAdapter::new()),
            AuditStoreConfig {
                durability: Durability::Memory,
                retention_milliseconds: 10_000,
                maximum_page_records: NonZeroUsize::new(2).expect("non-zero"),
                maximum_export_records: NonZeroUsize::new(4).expect("non-zero"),
                maximum_examined_records: NonZeroUsize::new(20).expect("non-zero"),
                cursor_signing_key: CursorSigningKey::new([7; 32]),
            },
        )
        .expect("store")
    }

    fn test_tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(format!("prj_{project}")).expect("project"),
            EnvironmentId::parse(format!("env_{environment}")).expect("environment"),
        )
    }

    fn audit_event(tenant: &TenantScope, id: &str, timestamp: u64, action: &str) -> AuditEvent {
        AuditEvent {
            context: SignalContext::new(
                SignalScope::Tenant {
                    tenant: tenant.clone(),
                    organization_id: Some("org_example00".to_owned()),
                },
                ActorIdentity::Developer {
                    actor_id: "dev_example00".to_owned(),
                },
                ResourceReference::new("project", tenant.project_id().as_str()).expect("resource"),
                RequestId::parse("req_example00").expect("request"),
                CorrelationId::parse("cor_example00").expect("correlation"),
                None,
            )
            .expect("context"),
            event_id: id.to_owned(),
            occurred_at_unix_milliseconds: timestamp,
            action: action.to_owned(),
            outcome: AuditOutcome::Allowed,
            reason_code: "authorized".to_owned(),
            details: SafeAttributes::default(),
        }
    }
}
