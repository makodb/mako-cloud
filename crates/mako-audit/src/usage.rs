use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    num::NonZeroUsize,
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter, KvSnapshot,
    ScanDirection, ScanRequest, StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{
    DetailCompactionReport, ObservabilityEvent, RedactionError, RetentionMode, SignalScope,
    TelemetryRedactor, UsageEvent, UsageResource,
};

const DETAIL_DOMAIN_PREFIX: &str = "mako:usage:detail:v1";
const ID_DOMAIN_PREFIX: &str = "mako:usage:id:v1";
const AGGREGATE_DOMAIN_PREFIX: &str = "mako:usage:aggregate:v1";
const USAGE_RESOURCES: usize = 10;

#[derive(Clone, Debug)]
pub struct UsageStoreConfig {
    pub durability: Durability,
    pub detail_retention_milliseconds: u64,
    pub bucket_width_milliseconds: NonZeroUsize,
    pub maximum_aggregation_buckets: NonZeroUsize,
    pub maximum_conflict_retries: NonZeroUsize,
}

impl UsageStoreConfig {
    fn validate(&self) -> Result<(), UsageStoreError> {
        if self.detail_retention_milliseconds == 0 {
            return Err(UsageStoreError::InvalidConfiguration);
        }
        let bucket_width = u64::try_from(self.bucket_width_milliseconds.get())
            .map_err(|_| UsageStoreError::InvalidConfiguration)?;
        let bucket_count = self.maximum_aggregation_buckets.get();
        bucket_width
            .checked_mul(
                u64::try_from(bucket_count).map_err(|_| UsageStoreError::InvalidConfiguration)?,
            )
            .ok_or(UsageStoreError::InvalidConfiguration)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct UsageStore {
    adapter: Arc<dyn KvAdapter>,
    config: UsageStoreConfig,
}

impl UsageStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        config: UsageStoreConfig,
    ) -> Result<Self, UsageStoreError> {
        config.validate()?;
        if adapter.capabilities().maximum_batch_operations.get() < 3
            || adapter.capabilities().strongest_durability < config.durability
        {
            return Err(UsageStoreError::InvalidConfiguration);
        }
        Ok(Self { adapter, config })
    }

    /// Stores one usage event and updates its rollup in the same conditional
    /// commit. Exact retries are idempotent and never increment twice.
    pub async fn record(
        &self,
        tenant: &TenantScope,
        event: UsageEvent,
        redactor: &TelemetryRedactor,
    ) -> Result<UsageRecordOutcome, UsageStoreError> {
        let mut wrapped = ObservabilityEvent::Usage(event);
        redactor.sanitize_event(&mut wrapped)?;
        let ObservabilityEvent::Usage(event) = wrapped else {
            return Err(UsageStoreError::InvalidEvent);
        };
        validate_event_for(&event, tenant)?;
        let event_bytes = serde_json::to_vec(&event).map_err(|_| UsageStoreError::Serialization)?;
        let event_key = detail_key(tenant, &event)?;
        let id_key = id_key(tenant, &event.usage_id)?;
        let guard = UsageIdGuard {
            digest: URL_SAFE_NO_PAD.encode(blake3::hash(&event_bytes).as_bytes()),
            event_key: Some(URL_SAFE_NO_PAD.encode(&event_key)),
        };
        let guard_bytes = serde_json::to_vec(&guard).map_err(|_| UsageStoreError::Serialization)?;
        let bucket_start = self.bucket_start(event.occurred_at_unix_milliseconds)?;
        let aggregate_key = aggregate_key(tenant, bucket_start, event.resource)?;

        for _ in 0..self.config.maximum_conflict_retries.get() {
            let existing_aggregate = self.adapter.get(&aggregate_key).await?;
            let mut aggregate = existing_aggregate
                .as_deref()
                .map(|bytes| {
                    serde_json::from_slice::<StoredUsageAggregate>(bytes)
                        .map_err(|_| UsageStoreError::CorruptStore)
                })
                .transpose()?
                .unwrap_or_else(|| StoredUsageAggregate::new(bucket_start, event.resource));
            if existing_aggregate.is_some() {
                aggregate.validate(self.bucket_width()?)?;
                if aggregate.bucket_start_unix_milliseconds != bucket_start
                    || aggregate.resource != event.resource
                {
                    return Err(UsageStoreError::CorruptStore);
                }
            }
            aggregate.apply(&event)?;
            let aggregate_bytes =
                serde_json::to_vec(&aggregate).map_err(|_| UsageStoreError::Serialization)?;
            let aggregate_condition = existing_aggregate.map_or_else(
                || KeyCondition::Missing {
                    key: aggregate_key.clone(),
                },
                |value| KeyCondition::ValueEquals {
                    key: aggregate_key.clone(),
                    value,
                },
            );
            let mut batch = WriteBatch::with_capacity(3);
            batch
                .put(&id_key, &guard_bytes)
                .put(&event_key, &event_bytes)
                .put(&aggregate_key, &aggregate_bytes);
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
                        aggregate_condition,
                    ],
                    batch,
                    durability: self.config.durability,
                })
                .await?
            {
                CompareAndWriteResult::Applied => return Ok(UsageRecordOutcome::Created),
                CompareAndWriteResult::Conflict {
                    failed_condition: 0,
                    actual_value: Some(actual),
                } => {
                    return self.resolve_existing(&actual, &guard, &event_bytes).await;
                }
                CompareAndWriteResult::Conflict {
                    failed_condition: 1,
                    ..
                } => return Err(UsageStoreError::CorruptStore),
                CompareAndWriteResult::Conflict {
                    failed_condition: 2,
                    ..
                } => continue,
                CompareAndWriteResult::Conflict { .. } => {
                    return Err(UsageStoreError::CorruptStore);
                }
            }
        }
        Err(UsageStoreError::ConcurrentAggregation)
    }

    pub async fn usage_by_id(
        &self,
        tenant: &TenantScope,
        usage_id: &str,
    ) -> Result<Option<UsageEvent>, UsageStoreError> {
        if usage_id.is_empty() || usage_id.len() > 128 || usage_id.chars().any(char::is_control) {
            return Err(UsageStoreError::InvalidEvent);
        }
        let Some(guard_bytes) = self.adapter.get(&id_key(tenant, usage_id)?).await? else {
            return Ok(None);
        };
        let guard: UsageIdGuard =
            serde_json::from_slice(&guard_bytes).map_err(|_| UsageStoreError::CorruptStore)?;
        let Some(encoded_event_key) = guard.event_key else {
            return Ok(None);
        };
        let event_key = URL_SAFE_NO_PAD
            .decode(encoded_event_key)
            .map_err(|_| UsageStoreError::CorruptStore)?;
        let Some(event_bytes) = self.adapter.get(&event_key).await? else {
            return Err(UsageStoreError::CorruptStore);
        };
        if guard.digest != URL_SAFE_NO_PAD.encode(blake3::hash(&event_bytes).as_bytes()) {
            return Err(UsageStoreError::CorruptStore);
        }
        let event: UsageEvent =
            serde_json::from_slice(&event_bytes).map_err(|_| UsageStoreError::CorruptStore)?;
        validate_event_for(&event, tenant)?;
        Ok(Some(event))
    }

    pub async fn aggregate(
        &self,
        tenant: &TenantScope,
        query: &UsageAggregationQuery,
    ) -> Result<UsageAggregation, UsageStoreError> {
        query.validate(&self.config)?;
        let domain = aggregate_domain(tenant);
        let domain_range = TenantKeyspace::system_domain_range(&domain)
            .map_err(|_| UsageStoreError::InvalidQuery)?;
        let start = TenantKeyspace::system_key(
            &domain,
            format!("{:020}/", query.from_unix_milliseconds).as_bytes(),
        )
        .map_err(|_| UsageStoreError::InvalidQuery)?;
        let end = TenantKeyspace::system_key(
            &domain,
            format!("{:020}/", query.until_unix_milliseconds).as_bytes(),
        )
        .map_err(|_| UsageStoreError::InvalidQuery)?;
        let range = KeyRange::new(start.max(domain_range.start_inclusive), end)
            .map_err(|_| UsageStoreError::InvalidQuery)?;
        let snapshot = self.adapter.snapshot().await?;
        let stored = self.scan_aggregates(snapshot.as_ref(), range).await?;
        let mut resources = BTreeMap::new();
        for aggregate in stored {
            aggregate.validate(self.bucket_width()?)?;
            if aggregate.bucket_start_unix_milliseconds < query.from_unix_milliseconds
                || aggregate.bucket_start_unix_milliseconds >= query.until_unix_milliseconds
            {
                return Err(UsageStoreError::CorruptStore);
            }
            if !query.resources.is_empty() && !query.resources.contains(&aggregate.resource) {
                continue;
            }
            resources
                .entry(aggregate.resource)
                .or_insert_with(|| UsageAggregate::new(aggregate.resource))
                .merge(aggregate)?;
        }
        Ok(UsageAggregation {
            from_unix_milliseconds: query.from_unix_milliseconds,
            until_unix_milliseconds: query.until_unix_milliseconds,
            bucket_width_milliseconds: self.bucket_width()?,
            resources,
        })
    }

    /// Compacts raw usage detail but retains its digest guard permanently so
    /// retries cannot increment an aggregate after the detail expires.
    pub async fn compact_expired_detail(
        &self,
        tenant: &TenantScope,
        now_unix_milliseconds: u64,
        mode: RetentionMode,
    ) -> Result<DetailCompactionReport, UsageStoreError> {
        if now_unix_milliseconds == 0 {
            return Err(UsageStoreError::InvalidQuery);
        }
        let retained_from =
            now_unix_milliseconds.saturating_sub(self.config.detail_retention_milliseconds);
        let domain = detail_domain(tenant);
        let domain_range = TenantKeyspace::system_domain_range(&domain)
            .map_err(|_| UsageStoreError::InvalidQuery)?;
        let end = TenantKeyspace::system_key(&domain, format!("{retained_from:020}/").as_bytes())
            .map_err(|_| UsageStoreError::InvalidQuery)?;
        if end <= domain_range.start_inclusive {
            return Ok(DetailCompactionReport {
                retained_from_unix_milliseconds: retained_from,
                dry_run: mode == RetentionMode::DryRun,
                records_eligible: 0,
                records_removed: 0,
            });
        }
        let mut range = KeyRange::new(domain_range.start_inclusive, end)
            .map_err(|_| UsageStoreError::InvalidQuery)?;
        let scan_limit = self.adapter.capabilities().maximum_scan_items;
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
                let event: UsageEvent = serde_json::from_slice(&entry.value)
                    .map_err(|_| UsageStoreError::CorruptStore)?;
                validate_event_for(&event, tenant)?;
                if event.occurred_at_unix_milliseconds >= retained_from {
                    return Err(UsageStoreError::CorruptStore);
                }
                let guard_key = id_key(tenant, &event.usage_id)?;
                let guard_bytes = self
                    .adapter
                    .get(&guard_key)
                    .await?
                    .ok_or(UsageStoreError::CorruptStore)?;
                let guard: UsageIdGuard = serde_json::from_slice(&guard_bytes)
                    .map_err(|_| UsageStoreError::CorruptStore)?;
                let expected_digest = URL_SAFE_NO_PAD.encode(blake3::hash(&entry.value).as_bytes());
                let expected_key = URL_SAFE_NO_PAD.encode(&entry.key);
                if guard.digest != expected_digest
                    || guard.event_key.as_deref() != Some(expected_key.as_str())
                {
                    return Err(UsageStoreError::CorruptStore);
                }
                eligible += 1;
                if mode == RetentionMode::Apply {
                    let compacted_guard = UsageIdGuard {
                        digest: guard.digest,
                        event_key: None,
                    };
                    let compacted_bytes = serde_json::to_vec(&compacted_guard)
                        .map_err(|_| UsageStoreError::Serialization)?;
                    let mut batch = WriteBatch::with_capacity(2);
                    batch.delete(&entry.key).put(&guard_key, compacted_bytes);
                    if self
                        .adapter
                        .compare_and_write(AtomicWrite {
                            conditions: vec![
                                KeyCondition::ValueEquals {
                                    key: entry.key.clone(),
                                    value: entry.value.clone(),
                                },
                                KeyCondition::ValueEquals {
                                    key: guard_key,
                                    value: guard_bytes,
                                },
                            ],
                            batch,
                            durability: self.config.durability,
                        })
                        .await?
                        == CompareAndWriteResult::Applied
                    {
                        removed += 1;
                    }
                }
            }
            let full_page = entries.len() == scan_limit.get();
            let mut next = entries
                .last()
                .ok_or(UsageStoreError::CorruptStore)?
                .key
                .clone();
            next.push(0);
            if !full_page || next >= range.end_exclusive {
                break;
            }
            range.start_inclusive = next;
        }
        Ok(DetailCompactionReport {
            retained_from_unix_milliseconds: retained_from,
            dry_run: mode == RetentionMode::DryRun,
            records_eligible: eligible,
            records_removed: removed,
        })
    }

    async fn resolve_existing(
        &self,
        actual_guard: &[u8],
        expected_guard: &UsageIdGuard,
        expected_event: &[u8],
    ) -> Result<UsageRecordOutcome, UsageStoreError> {
        let actual: UsageIdGuard =
            serde_json::from_slice(actual_guard).map_err(|_| UsageStoreError::CorruptStore)?;
        if actual.digest != expected_guard.digest {
            return Err(UsageStoreError::UsageIdConflict);
        }
        let Some(event_key) = actual.event_key else {
            return Ok(UsageRecordOutcome::AlreadyPresent);
        };
        let key = URL_SAFE_NO_PAD
            .decode(event_key)
            .map_err(|_| UsageStoreError::CorruptStore)?;
        let stored = self
            .adapter
            .get(&key)
            .await?
            .ok_or(UsageStoreError::CorruptStore)?;
        if stored != expected_event {
            return Err(UsageStoreError::CorruptStore);
        }
        Ok(UsageRecordOutcome::AlreadyPresent)
    }

    async fn scan_aggregates(
        &self,
        snapshot: &dyn KvSnapshot,
        mut range: KeyRange,
    ) -> Result<Vec<StoredUsageAggregate>, UsageStoreError> {
        let maximum_records = self
            .config
            .maximum_aggregation_buckets
            .get()
            .checked_mul(USAGE_RESOURCES)
            .ok_or(UsageStoreError::InvalidConfiguration)?;
        let chunk = self
            .adapter
            .capabilities()
            .maximum_scan_items
            .get()
            .min(256);
        let mut records = Vec::new();
        while records.len() < maximum_records {
            let limit = chunk.min(maximum_records - records.len());
            let entries = snapshot
                .scan(ScanRequest::new(
                    range.clone(),
                    ScanDirection::Forward,
                    NonZeroUsize::new(limit).ok_or(UsageStoreError::InvalidConfiguration)?,
                ))
                .await?;
            if entries.is_empty() {
                break;
            }
            for entry in &entries {
                records.push(
                    serde_json::from_slice(&entry.value)
                        .map_err(|_| UsageStoreError::CorruptStore)?,
                );
            }
            if entries.len() < limit {
                break;
            }
            let mut next = entries
                .last()
                .ok_or(UsageStoreError::CorruptStore)?
                .key
                .clone();
            next.push(0);
            if next >= range.end_exclusive {
                break;
            }
            range.start_inclusive = next;
        }
        Ok(records)
    }

    fn bucket_width(&self) -> Result<u64, UsageStoreError> {
        u64::try_from(self.config.bucket_width_milliseconds.get())
            .map_err(|_| UsageStoreError::InvalidConfiguration)
    }

    fn bucket_start(&self, timestamp: u64) -> Result<u64, UsageStoreError> {
        let width = self.bucket_width()?;
        Ok(timestamp / width * width)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageAggregationQuery {
    pub from_unix_milliseconds: u64,
    pub until_unix_milliseconds: u64,
    pub resources: BTreeSet<UsageResource>,
}

impl UsageAggregationQuery {
    fn validate(&self, config: &UsageStoreConfig) -> Result<(), UsageStoreError> {
        let width = u64::try_from(config.bucket_width_milliseconds.get())
            .map_err(|_| UsageStoreError::InvalidConfiguration)?;
        if self.from_unix_milliseconds >= self.until_unix_milliseconds
            || !self.from_unix_milliseconds.is_multiple_of(width)
            || !self.until_unix_milliseconds.is_multiple_of(width)
        {
            return Err(UsageStoreError::InvalidQuery);
        }
        let buckets = (self.until_unix_milliseconds - self.from_unix_milliseconds) / width;
        if buckets
            > u64::try_from(config.maximum_aggregation_buckets.get())
                .map_err(|_| UsageStoreError::InvalidConfiguration)?
        {
            return Err(UsageStoreError::AggregationWindowTooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageAggregation {
    pub from_unix_milliseconds: u64,
    pub until_unix_milliseconds: u64,
    pub bucket_width_milliseconds: u64,
    pub resources: BTreeMap<UsageResource, UsageAggregate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageAggregate {
    pub resource: UsageResource,
    pub quantity: u64,
    pub unit: &'static str,
    pub record_count: u64,
    pub latest_occurred_at_unix_milliseconds: u64,
}

impl UsageAggregate {
    const fn new(resource: UsageResource) -> Self {
        Self {
            resource,
            quantity: 0,
            unit: resource.expected_unit(),
            record_count: 0,
            latest_occurred_at_unix_milliseconds: 0,
        }
    }

    fn merge(&mut self, stored: StoredUsageAggregate) -> Result<(), UsageStoreError> {
        if stored.resource != self.resource || stored.unit != self.unit {
            return Err(UsageStoreError::CorruptStore);
        }
        self.record_count = self
            .record_count
            .checked_add(stored.record_count)
            .ok_or(UsageStoreError::QuantityOverflow)?;
        if self.resource.is_gauge() {
            if (
                stored.latest_occurred_at_unix_milliseconds,
                stored.latest_usage_id.as_str(),
            ) > (self.latest_occurred_at_unix_milliseconds, "")
            {
                self.quantity = stored.quantity;
                self.latest_occurred_at_unix_milliseconds =
                    stored.latest_occurred_at_unix_milliseconds;
            }
        } else {
            self.quantity = self
                .quantity
                .checked_add(stored.quantity)
                .ok_or(UsageStoreError::QuantityOverflow)?;
            self.latest_occurred_at_unix_milliseconds = self
                .latest_occurred_at_unix_milliseconds
                .max(stored.latest_occurred_at_unix_milliseconds);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredUsageAggregate {
    bucket_start_unix_milliseconds: u64,
    resource: UsageResource,
    quantity: u64,
    unit: String,
    record_count: u64,
    latest_occurred_at_unix_milliseconds: u64,
    latest_usage_id: String,
}

impl StoredUsageAggregate {
    fn new(bucket_start: u64, resource: UsageResource) -> Self {
        Self {
            bucket_start_unix_milliseconds: bucket_start,
            resource,
            quantity: 0,
            unit: resource.expected_unit().to_owned(),
            record_count: 0,
            latest_occurred_at_unix_milliseconds: 0,
            latest_usage_id: String::new(),
        }
    }

    fn apply(&mut self, event: &UsageEvent) -> Result<(), UsageStoreError> {
        if self.resource != event.resource || self.unit != event.unit {
            return Err(UsageStoreError::CorruptStore);
        }
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or(UsageStoreError::QuantityOverflow)?;
        if event.resource.is_gauge() {
            if (event.occurred_at_unix_milliseconds, event.usage_id.as_str())
                > (
                    self.latest_occurred_at_unix_milliseconds,
                    self.latest_usage_id.as_str(),
                )
            {
                self.quantity = event.quantity;
                self.latest_occurred_at_unix_milliseconds = event.occurred_at_unix_milliseconds;
                self.latest_usage_id.clone_from(&event.usage_id);
            }
        } else {
            self.quantity = self
                .quantity
                .checked_add(event.quantity)
                .ok_or(UsageStoreError::QuantityOverflow)?;
            if (event.occurred_at_unix_milliseconds, event.usage_id.as_str())
                > (
                    self.latest_occurred_at_unix_milliseconds,
                    self.latest_usage_id.as_str(),
                )
            {
                self.latest_occurred_at_unix_milliseconds = event.occurred_at_unix_milliseconds;
                self.latest_usage_id.clone_from(&event.usage_id);
            }
        }
        Ok(())
    }

    fn validate(&self, bucket_width: u64) -> Result<(), UsageStoreError> {
        let bucket_end = self
            .bucket_start_unix_milliseconds
            .checked_add(bucket_width)
            .ok_or(UsageStoreError::CorruptStore)?;
        if !self
            .bucket_start_unix_milliseconds
            .is_multiple_of(bucket_width)
            || self.unit != self.resource.expected_unit()
            || self.record_count == 0
            || self.latest_usage_id.is_empty()
            || self.latest_occurred_at_unix_milliseconds < self.bucket_start_unix_milliseconds
            || self.latest_occurred_at_unix_milliseconds >= bucket_end
        {
            return Err(UsageStoreError::CorruptStore);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageIdGuard {
    digest: String,
    event_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageRecordOutcome {
    Created,
    AlreadyPresent,
}

#[derive(Debug)]
pub enum UsageStoreError {
    InvalidConfiguration,
    InvalidEvent,
    TenantScopeRequired,
    TenantMismatch,
    UsageIdConflict,
    ConcurrentAggregation,
    QuantityOverflow,
    InvalidQuery,
    AggregationWindowTooLarge,
    Serialization,
    CorruptStore,
    Redaction(RedactionError),
    Storage(StorageError),
}

impl fmt::Display for UsageStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "usage store configuration is invalid",
            Self::InvalidEvent => "usage event is invalid",
            Self::TenantScopeRequired => "tenant usage scope is required",
            Self::TenantMismatch => "usage event tenant does not match the storage tenant",
            Self::UsageIdConflict => "usage identifier already has different content",
            Self::ConcurrentAggregation => "usage aggregation changed concurrently",
            Self::QuantityOverflow => "usage quantity exceeds the supported range",
            Self::InvalidQuery => "usage aggregation query is invalid",
            Self::AggregationWindowTooLarge => "usage aggregation window is too large",
            Self::Serialization => "usage record serialization failed",
            Self::CorruptStore => "usage storage contains an invalid record",
            Self::Redaction(_) => "usage record redaction failed",
            Self::Storage(_) => "usage storage operation failed",
        })
    }
}

impl Error for UsageStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Redaction(error) => Some(error),
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RedactionError> for UsageStoreError {
    fn from(value: RedactionError) -> Self {
        Self::Redaction(value)
    }
}

impl From<StorageError> for UsageStoreError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

fn validate_event_for(event: &UsageEvent, tenant: &TenantScope) -> Result<(), UsageStoreError> {
    event
        .validate()
        .map_err(|_| UsageStoreError::InvalidEvent)?;
    match event.context.scope() {
        SignalScope::Tenant {
            tenant: event_tenant,
            ..
        } if event_tenant == tenant => Ok(()),
        SignalScope::Tenant { .. } => Err(UsageStoreError::TenantMismatch),
        SignalScope::Organization { .. } | SignalScope::Platform => {
            Err(UsageStoreError::TenantScopeRequired)
        }
    }
}

fn detail_domain(tenant: &TenantScope) -> Vec<u8> {
    tenant_domain(DETAIL_DOMAIN_PREFIX, tenant)
}

fn id_domain(tenant: &TenantScope) -> Vec<u8> {
    tenant_domain(ID_DOMAIN_PREFIX, tenant)
}

fn aggregate_domain(tenant: &TenantScope) -> Vec<u8> {
    tenant_domain(AGGREGATE_DOMAIN_PREFIX, tenant)
}

fn tenant_domain(prefix: &str, tenant: &TenantScope) -> Vec<u8> {
    format!(
        "{prefix}:{}:{}",
        tenant.project_id(),
        tenant.environment_id()
    )
    .into_bytes()
}

fn detail_key(tenant: &TenantScope, event: &UsageEvent) -> Result<Vec<u8>, UsageStoreError> {
    TenantKeyspace::system_key(
        detail_domain(tenant),
        format!(
            "{:020}/{}",
            event.occurred_at_unix_milliseconds, event.usage_id
        ),
    )
    .map_err(|_| UsageStoreError::InvalidEvent)
}

fn id_key(tenant: &TenantScope, usage_id: &str) -> Result<Vec<u8>, UsageStoreError> {
    TenantKeyspace::system_key(id_domain(tenant), usage_id)
        .map_err(|_| UsageStoreError::InvalidEvent)
}

fn aggregate_key(
    tenant: &TenantScope,
    bucket_start: u64,
    resource: UsageResource,
) -> Result<Vec<u8>, UsageStoreError> {
    let resource = serde_json::to_string(&resource).map_err(|_| UsageStoreError::Serialization)?;
    TenantKeyspace::system_key(
        aggregate_domain(tenant),
        format!("{bucket_start:020}/{}", resource.trim_matches('"')),
    )
    .map_err(|_| UsageStoreError::InvalidEvent)
}

#[cfg(test)]
mod tests {
    use futures::{executor::block_on, future::join_all};
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{
        ActorIdentity, AttributeValue, CorrelationId, RequestId, ResourceReference, SafeAttributes,
        SignalContext,
    };

    #[test]
    fn exact_retries_do_not_double_count_and_changed_ids_conflict() {
        block_on(async {
            let store = store(Arc::new(MemoryAdapter::new()));
            let tenant = test_tenant("example00", "example00");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            let event = usage_event(
                &tenant,
                "usg_request00",
                1_100,
                UsageResource::ReplicationRequests,
                3,
            );
            assert_eq!(
                store
                    .record(&tenant, event.clone(), &redactor)
                    .await
                    .expect("record"),
                UsageRecordOutcome::Created
            );
            assert_eq!(
                store
                    .record(&tenant, event.clone(), &redactor)
                    .await
                    .expect("retry"),
                UsageRecordOutcome::AlreadyPresent
            );
            let mut changed = event.clone();
            changed.quantity = 4;
            assert!(matches!(
                store.record(&tenant, changed, &redactor).await,
                Err(UsageStoreError::UsageIdConflict)
            ));
            assert_eq!(
                store
                    .usage_by_id(&tenant, &event.usage_id)
                    .await
                    .expect("lookup"),
                Some(event)
            );
            let aggregate = query_aggregate(&store, &tenant, BTreeSet::new())
                .await
                .expect("aggregate");
            assert_eq!(
                aggregate.resources[&UsageResource::ReplicationRequests].quantity,
                3
            );
            assert_eq!(
                aggregate.resources[&UsageResource::ReplicationRequests].record_count,
                1
            );
        });
    }

    #[test]
    fn every_mvp_resource_aggregates_with_storage_as_latest_gauge() {
        block_on(async {
            let store = store(Arc::new(MemoryAdapter::new()));
            let tenant = test_tenant("example00", "example00");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            let resources = [
                UsageResource::ReplicationRequests,
                UsageResource::ReplicationBytes,
                UsageResource::AuthenticationActivity,
                UsageResource::FunctionInvocations,
                UsageResource::FunctionCpuMilliseconds,
                UsageResource::FunctionWallMilliseconds,
                UsageResource::LogBytes,
                UsageResource::EgressRequests,
                UsageResource::EgressBytes,
            ];
            for (index, resource) in resources.into_iter().enumerate() {
                store
                    .record(
                        &tenant,
                        usage_event(
                            &tenant,
                            &format!("usg_resource{index}"),
                            1_100 + u64::try_from(index).expect("small index"),
                            resource,
                            2,
                        ),
                        &redactor,
                    )
                    .await
                    .expect("record");
            }
            for (id, timestamp, quantity) in [
                ("usg_storage00", 1_100, 100),
                ("usg_storage01", 1_500, 80),
                ("usg_storage02", 2_500, 120),
            ] {
                store
                    .record(
                        &tenant,
                        usage_event(
                            &tenant,
                            id,
                            timestamp,
                            UsageResource::StorageBytes,
                            quantity,
                        ),
                        &redactor,
                    )
                    .await
                    .expect("storage record");
            }

            let aggregate = query_aggregate(&store, &tenant, BTreeSet::new())
                .await
                .expect("aggregate");
            assert_eq!(aggregate.resources.len(), 10);
            for resource in resources {
                assert_eq!(aggregate.resources[&resource].quantity, 2);
                assert_eq!(
                    aggregate.resources[&resource].unit,
                    resource.expected_unit()
                );
            }
            let storage = &aggregate.resources[&UsageResource::StorageBytes];
            assert_eq!(storage.quantity, 120);
            assert_eq!(storage.record_count, 3);
            assert_eq!(storage.latest_occurred_at_unix_milliseconds, 2_500);

            let filtered = query_aggregate(
                &store,
                &tenant,
                BTreeSet::from([UsageResource::EgressBytes]),
            )
            .await
            .expect("filtered aggregate");
            assert_eq!(filtered.resources.len(), 1);
            assert_eq!(filtered.resources[&UsageResource::EgressBytes].quantity, 2);
        });
    }

    #[test]
    fn concurrent_distinct_records_are_never_lost() {
        block_on(async {
            let store = store(Arc::new(MemoryAdapter::new()));
            let tenant = test_tenant("example00", "example00");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            let results = join_all((0..20).map(|index| {
                let event = usage_event(
                    &tenant,
                    &format!("usg_concurrent{index:02}"),
                    1_100 + index,
                    UsageResource::FunctionInvocations,
                    1,
                );
                store.record(&tenant, event, &redactor)
            }))
            .await;
            assert!(results.into_iter().all(|result| result.is_ok()));
            let aggregate = query_aggregate(&store, &tenant, BTreeSet::new())
                .await
                .expect("aggregate");
            let functions = &aggregate.resources[&UsageResource::FunctionInvocations];
            assert_eq!(functions.quantity, 20);
            assert_eq!(functions.record_count, 20);
        });
    }

    #[test]
    fn redaction_and_tenant_boundaries_apply_to_usage_detail() {
        block_on(async {
            let store = store(Arc::new(MemoryAdapter::new()));
            let tenant = test_tenant("example00", "example00");
            let other = test_tenant("other000", "other000");
            let secret = "meter-secret-canary";
            let redactor = TelemetryRedactor::new([secret]).expect("redactor");
            let mut event = usage_event(
                &tenant,
                "usg_redacted00",
                1_100,
                UsageResource::LogBytes,
                10,
            );
            event.attributes = SafeAttributes::try_from_iter([
                (
                    "document.body".to_owned(),
                    AttributeValue::Text("private document".to_owned()),
                ),
                (
                    "error.message".to_owned(),
                    AttributeValue::Text(format!("failure {secret}")),
                ),
            ])
            .expect("attributes");
            store
                .record(&tenant, event, &redactor)
                .await
                .expect("record");
            let stored = store
                .usage_by_id(&tenant, "usg_redacted00")
                .await
                .expect("lookup")
                .expect("stored");
            assert!(stored.attributes.get("document.body").is_none());
            assert_eq!(
                stored.attributes.get("error.message"),
                Some(&AttributeValue::Text("failure [REDACTED]".to_owned()))
            );
            assert!(
                store
                    .usage_by_id(&other, "usg_redacted00")
                    .await
                    .expect("other lookup")
                    .is_none()
            );
            assert!(matches!(
                store
                    .record(
                        &other,
                        usage_event(&tenant, "usg_wrong0000", 1_200, UsageResource::LogBytes, 1,),
                        &redactor,
                    )
                    .await,
                Err(UsageStoreError::TenantMismatch)
            ));
        });
    }

    #[test]
    fn unit_and_bounded_aligned_window_validation_fail_closed() {
        block_on(async {
            let store = store(Arc::new(MemoryAdapter::new()));
            let tenant = test_tenant("example00", "example00");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            let mut invalid = usage_event(
                &tenant,
                "usg_invalid00",
                1_100,
                UsageResource::EgressBytes,
                1,
            );
            invalid.unit = "requests".to_owned();
            assert!(matches!(
                store.record(&tenant, invalid, &redactor).await,
                Err(UsageStoreError::Redaction(
                    RedactionError::InvalidSanitizedEvent
                ))
            ));
            for query in [
                UsageAggregationQuery {
                    from_unix_milliseconds: 1_001,
                    until_unix_milliseconds: 2_000,
                    resources: BTreeSet::new(),
                },
                UsageAggregationQuery {
                    from_unix_milliseconds: 0,
                    until_unix_milliseconds: 12_000,
                    resources: BTreeSet::new(),
                },
            ] {
                assert!(matches!(
                    store.aggregate(&tenant, &query).await,
                    Err(UsageStoreError::InvalidQuery | UsageStoreError::AggregationWindowTooLarge)
                ));
            }
        });
    }

    #[test]
    fn detail_compaction_keeps_rollups_and_idempotency_guards() {
        block_on(async {
            let store = store(Arc::new(MemoryAdapter::new()));
            let tenant = test_tenant("example00", "example00");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            let expired = usage_event(
                &tenant,
                "usg_expired00",
                9_000,
                UsageResource::ReplicationBytes,
                3,
            );
            let retained = usage_event(
                &tenant,
                "usg_retained00",
                11_000,
                UsageResource::ReplicationBytes,
                4,
            );
            store
                .record(&tenant, expired.clone(), &redactor)
                .await
                .expect("expired record");
            store
                .record(&tenant, retained.clone(), &redactor)
                .await
                .expect("retained record");
            let preview = store
                .compact_expired_detail(&tenant, 20_000, RetentionMode::DryRun)
                .await
                .expect("preview");
            assert!(preview.dry_run);
            assert_eq!(preview.records_eligible, 1);
            assert_eq!(preview.records_removed, 0);
            assert!(
                store
                    .usage_by_id(&tenant, &expired.usage_id)
                    .await
                    .expect("lookup")
                    .is_some()
            );
            let applied = store
                .compact_expired_detail(&tenant, 20_000, RetentionMode::Apply)
                .await
                .expect("apply");
            assert_eq!(applied.records_eligible, 1);
            assert_eq!(applied.records_removed, 1);
            assert!(
                store
                    .usage_by_id(&tenant, &expired.usage_id)
                    .await
                    .expect("compacted lookup")
                    .is_none()
            );
            assert_eq!(
                store
                    .record(&tenant, expired, &redactor)
                    .await
                    .expect("retry"),
                UsageRecordOutcome::AlreadyPresent
            );
            assert_eq!(
                store
                    .usage_by_id(&tenant, &retained.usage_id)
                    .await
                    .expect("retained lookup"),
                Some(retained)
            );
            let rollup = store
                .aggregate(
                    &tenant,
                    &UsageAggregationQuery {
                        from_unix_milliseconds: 9_000,
                        until_unix_milliseconds: 12_000,
                        resources: BTreeSet::from([UsageResource::ReplicationBytes]),
                    },
                )
                .await
                .expect("rollup");
            assert_eq!(
                rollup.resources[&UsageResource::ReplicationBytes].quantity,
                7
            );
        });
    }

    fn store(adapter: Arc<MemoryAdapter>) -> UsageStore {
        UsageStore::new(
            adapter,
            UsageStoreConfig {
                durability: Durability::Memory,
                detail_retention_milliseconds: 10_000,
                bucket_width_milliseconds: NonZeroUsize::new(1_000).expect("non-zero"),
                maximum_aggregation_buckets: NonZeroUsize::new(10).expect("non-zero"),
                maximum_conflict_retries: NonZeroUsize::new(64).expect("non-zero"),
            },
        )
        .expect("store")
    }

    async fn query_aggregate(
        store: &UsageStore,
        tenant: &TenantScope,
        resources: BTreeSet<UsageResource>,
    ) -> Result<UsageAggregation, UsageStoreError> {
        store
            .aggregate(
                tenant,
                &UsageAggregationQuery {
                    from_unix_milliseconds: 1_000,
                    until_unix_milliseconds: 3_000,
                    resources,
                },
            )
            .await
    }

    fn test_tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(format!("prj_{project}")).expect("project"),
            EnvironmentId::parse(format!("env_{environment}")).expect("environment"),
        )
    }

    fn usage_event(
        tenant: &TenantScope,
        usage_id: &str,
        timestamp: u64,
        resource: UsageResource,
        quantity: u64,
    ) -> UsageEvent {
        UsageEvent {
            context: SignalContext::new(
                SignalScope::Tenant {
                    tenant: tenant.clone(),
                    organization_id: Some("org_example00".to_owned()),
                },
                ActorIdentity::System {
                    component: "usage-meter".to_owned(),
                },
                ResourceReference::new("environment", tenant.environment_id().as_str())
                    .expect("resource"),
                RequestId::parse(format!("req_{usage_id}")).expect("request"),
                CorrelationId::parse(format!("cor_{usage_id}")).expect("correlation"),
                None,
            )
            .expect("context"),
            usage_id: usage_id.to_owned(),
            occurred_at_unix_milliseconds: timestamp,
            resource,
            quantity,
            unit: resource.expected_unit().to_owned(),
            attributes: SafeAttributes::default(),
        }
    }
}
