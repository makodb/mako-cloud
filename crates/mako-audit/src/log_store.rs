use std::{error::Error, fmt, sync::Arc};

use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter,
    ScanDirection, ScanRequest, StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{
    DetailCompactionReport, ObservabilityEvent, RedactionError, RetentionMode, SignalScope,
    StructuredLogEvent, TelemetryRedactor,
};

const DETAIL_DOMAIN_PREFIX: &str = "mako:log:detail:v1";
const ID_DOMAIN_PREFIX: &str = "mako:log:id:v1";

#[derive(Clone, Copy, Debug)]
pub struct LogStoreConfig {
    pub durability: Durability,
    pub retention_milliseconds: u64,
}

#[derive(Clone)]
pub struct LogStore {
    adapter: Arc<dyn KvAdapter>,
    config: LogStoreConfig,
}

impl LogStore {
    pub fn new(adapter: Arc<dyn KvAdapter>, config: LogStoreConfig) -> Result<Self, LogStoreError> {
        if config.retention_milliseconds == 0
            || adapter.capabilities().maximum_batch_operations.get() < 2
            || adapter.capabilities().strongest_durability < config.durability
        {
            return Err(LogStoreError::InvalidConfiguration);
        }
        Ok(Self { adapter, config })
    }

    pub async fn append(
        &self,
        tenant: &TenantScope,
        log_id: &str,
        event: StructuredLogEvent,
        redactor: &TelemetryRedactor,
    ) -> Result<LogAppendOutcome, LogStoreError> {
        validate_log_id(log_id)?;
        let mut wrapped = ObservabilityEvent::Log(event);
        redactor.sanitize_event(&mut wrapped)?;
        let ObservabilityEvent::Log(event) = wrapped else {
            return Err(LogStoreError::InvalidEvent);
        };
        validate_event_for(&event, tenant)?;
        let record = StoredLogRecord {
            log_id: log_id.to_owned(),
            event,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| LogStoreError::Serialization)?;
        let digest = blake3::hash(&bytes).as_bytes().to_vec();
        let detail_key = detail_key(tenant, &record)?;
        let guard_key = id_key(tenant, log_id)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&detail_key, &bytes).put(&guard_key, &digest);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::Missing {
                        key: guard_key.clone(),
                    },
                    KeyCondition::Missing { key: detail_key },
                ],
                batch,
                durability: self.config.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(LogAppendOutcome::Created),
            CompareAndWriteResult::Conflict {
                failed_condition: 0,
                actual_value: Some(actual),
            } if constant_time_eq(&actual, &digest) => Ok(LogAppendOutcome::AlreadyPresent),
            CompareAndWriteResult::Conflict {
                failed_condition: 0,
                ..
            } => Err(LogStoreError::LogIdConflict),
            CompareAndWriteResult::Conflict { .. } => Err(LogStoreError::CorruptStore),
        }
    }

    pub async fn compact_expired(
        &self,
        tenant: &TenantScope,
        now_unix_milliseconds: u64,
        mode: RetentionMode,
    ) -> Result<DetailCompactionReport, LogStoreError> {
        if now_unix_milliseconds == 0 {
            return Err(LogStoreError::InvalidQuery);
        }
        let retained_from =
            now_unix_milliseconds.saturating_sub(self.config.retention_milliseconds);
        let domain = detail_domain(tenant);
        let domain_range = TenantKeyspace::system_domain_range(&domain)
            .map_err(|_| LogStoreError::InvalidQuery)?;
        let end = TenantKeyspace::system_key(&domain, format!("{retained_from:020}/").as_bytes())
            .map_err(|_| LogStoreError::InvalidQuery)?;
        if end <= domain_range.start_inclusive {
            return Ok(report(retained_from, mode, 0, 0));
        }
        let mut range = KeyRange::new(domain_range.start_inclusive, end)
            .map_err(|_| LogStoreError::InvalidQuery)?;
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
                let record: StoredLogRecord = serde_json::from_slice(&entry.value)
                    .map_err(|_| LogStoreError::CorruptStore)?;
                validate_log_id(&record.log_id)?;
                validate_event_for(&record.event, tenant)?;
                if record.event.occurred_at_unix_milliseconds >= retained_from {
                    return Err(LogStoreError::CorruptStore);
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
                .ok_or(LogStoreError::CorruptStore)?
                .key
                .clone();
            next.push(0);
            if !full_page || next >= range.end_exclusive {
                break;
            }
            range.start_inclusive = next;
        }
        Ok(report(retained_from, mode, eligible, removed))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredLogRecord {
    log_id: String,
    event: StructuredLogEvent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogAppendOutcome {
    Created,
    AlreadyPresent,
}

#[derive(Debug)]
pub enum LogStoreError {
    InvalidConfiguration,
    InvalidEvent,
    TenantScopeRequired,
    TenantMismatch,
    LogIdConflict,
    InvalidQuery,
    Serialization,
    CorruptStore,
    Redaction(RedactionError),
    Storage(StorageError),
}

impl fmt::Display for LogStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "log store configuration is invalid",
            Self::InvalidEvent => "structured log event is invalid",
            Self::TenantScopeRequired => "tenant log scope is required",
            Self::TenantMismatch => "log event tenant does not match the storage tenant",
            Self::LogIdConflict => "log identifier already has different content",
            Self::InvalidQuery => "log retention query is invalid",
            Self::Serialization => "log record serialization failed",
            Self::CorruptStore => "log storage contains an invalid record",
            Self::Redaction(_) => "log event redaction failed",
            Self::Storage(_) => "log storage operation failed",
        })
    }
}

impl Error for LogStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Redaction(error) => Some(error),
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RedactionError> for LogStoreError {
    fn from(value: RedactionError) -> Self {
        Self::Redaction(value)
    }
}

impl From<StorageError> for LogStoreError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

fn validate_event_for(
    event: &StructuredLogEvent,
    tenant: &TenantScope,
) -> Result<(), LogStoreError> {
    event.validate().map_err(|_| LogStoreError::InvalidEvent)?;
    match event.context.scope() {
        SignalScope::Tenant {
            tenant: event_tenant,
            ..
        } if event_tenant == tenant => Ok(()),
        SignalScope::Tenant { .. } => Err(LogStoreError::TenantMismatch),
        SignalScope::Organization { .. } | SignalScope::Platform => {
            Err(LogStoreError::TenantScopeRequired)
        }
    }
}

fn validate_log_id(log_id: &str) -> Result<(), LogStoreError> {
    if (8..=128).contains(&log_id.len())
        && log_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        Ok(())
    } else {
        Err(LogStoreError::InvalidEvent)
    }
}

fn detail_key(tenant: &TenantScope, record: &StoredLogRecord) -> Result<Vec<u8>, LogStoreError> {
    TenantKeyspace::system_key(
        detail_domain(tenant),
        format!(
            "{:020}/{}",
            record.event.occurred_at_unix_milliseconds, record.log_id
        ),
    )
    .map_err(|_| LogStoreError::InvalidEvent)
}

fn id_key(tenant: &TenantScope, log_id: &str) -> Result<Vec<u8>, LogStoreError> {
    TenantKeyspace::system_key(id_domain(tenant), log_id).map_err(|_| LogStoreError::InvalidEvent)
}

fn detail_domain(tenant: &TenantScope) -> Vec<u8> {
    tenant_domain(DETAIL_DOMAIN_PREFIX, tenant)
}

fn id_domain(tenant: &TenantScope) -> Vec<u8> {
    tenant_domain(ID_DOMAIN_PREFIX, tenant)
}

fn tenant_domain(prefix: &str, tenant: &TenantScope) -> Vec<u8> {
    format!(
        "{prefix}:{}:{}",
        tenant.project_id(),
        tenant.environment_id()
    )
    .into_bytes()
}

fn report(
    retained_from: u64,
    mode: RetentionMode,
    eligible: usize,
    removed: usize,
) -> DetailCompactionReport {
    DetailCompactionReport {
        retained_from_unix_milliseconds: retained_from,
        dry_run: mode == RetentionMode::DryRun,
        records_eligible: eligible,
        records_removed: removed,
    }
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
    use proptest::prelude::*;

    use super::*;
    use crate::{
        ActorIdentity, AttributeValue, CorrelationId, LogLevel, RequestId, ResourceReference,
        SafeAttributes, SignalContext,
    };

    #[test]
    fn logs_are_redacted_and_compacted_with_dry_run_reporting() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let store = LogStore::new(
                adapter.clone(),
                LogStoreConfig {
                    durability: Durability::Memory,
                    retention_milliseconds: 10_000,
                },
            )
            .expect("store");
            let tenant = test_tenant("example00", "example00");
            let secret = "log-secret-canary";
            let redactor = TelemetryRedactor::new([secret]).expect("redactor");
            let expired = event(&tenant, 9_000, format!("failed with {secret}"));
            assert_eq!(
                store
                    .append(&tenant, "log_expired00", expired.clone(), &redactor)
                    .await
                    .expect("append"),
                LogAppendOutcome::Created
            );
            store
                .append(
                    &tenant,
                    "log_retained00",
                    event(&tenant, 11_000, "retained".to_owned()),
                    &redactor,
                )
                .await
                .expect("append retained");
            let raw = adapter
                .dump()
                .expect("dump")
                .into_values()
                .flatten()
                .collect::<Vec<_>>();
            assert!(!String::from_utf8_lossy(&raw).contains(secret));

            let preview = store
                .compact_expired(&tenant, 20_000, RetentionMode::DryRun)
                .await
                .expect("preview");
            assert!(preview.dry_run);
            assert_eq!(preview.records_eligible, 1);
            assert_eq!(preview.records_removed, 0);
            let applied = store
                .compact_expired(&tenant, 20_000, RetentionMode::Apply)
                .await
                .expect("apply");
            assert_eq!(applied.records_eligible, 1);
            assert_eq!(applied.records_removed, 1);
            assert_eq!(
                store
                    .append(&tenant, "log_expired00", expired.clone(), &redactor)
                    .await
                    .expect("retry"),
                LogAppendOutcome::AlreadyPresent
            );
            let mut changed = expired;
            changed.event = "log.changed".to_owned();
            assert!(matches!(
                store
                    .append(&tenant, "log_expired00", changed, &redactor)
                    .await,
                Err(LogStoreError::LogIdConflict)
            ));
        });
    }

    #[test]
    fn log_scope_mismatch_is_rejected() {
        block_on(async {
            let store = LogStore::new(
                Arc::new(MemoryAdapter::new()),
                LogStoreConfig {
                    durability: Durability::Memory,
                    retention_milliseconds: 10_000,
                },
            )
            .expect("store");
            let tenant = test_tenant("example00", "example00");
            let other = test_tenant("other000", "other000");
            let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
            assert!(matches!(
                store
                    .append(
                        &other,
                        "log_mismatch0",
                        event(&tenant, 9_000, "safe".to_owned()),
                        &redactor,
                    )
                    .await,
                Err(LogStoreError::TenantMismatch)
            ));
        });
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arbitrary_log_producers_cannot_cross_tenant_storage_boundaries(
            owner_project in "[a-z0-9_-]{8,24}",
            owner_environment in "[a-z0-9_-]{8,24}",
            attacker_project in "[a-z0-9_-]{8,24}",
            attacker_environment in "[a-z0-9_-]{8,24}",
        ) {
            prop_assume!(
                owner_project != attacker_project || owner_environment != attacker_environment
            );
            block_on(async {
                let store = LogStore::new(
                    Arc::new(MemoryAdapter::new()),
                    LogStoreConfig {
                        durability: Durability::Memory,
                        retention_milliseconds: 10_000,
                    },
                )
                .expect("store");
                let owner = test_tenant(&owner_project, &owner_environment);
                let attacker = test_tenant(&attacker_project, &attacker_environment);
                let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
                prop_assert!(matches!(
                    store
                        .append(
                            &attacker,
                            "log_boundary0",
                            event(&owner, 9_000, "safe".to_owned()),
                            &redactor,
                        )
                        .await,
                    Err(LogStoreError::TenantMismatch)
                ));
                prop_assert_eq!(
                    store
                        .append(
                            &owner,
                            "log_boundary0",
                            event(&owner, 9_000, "safe".to_owned()),
                            &redactor,
                        )
                        .await
                        .expect("owner append"),
                    LogAppendOutcome::Created,
                );
                Ok(())
            })?;
        }
    }

    fn test_tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(format!("prj_{project}")).expect("project"),
            EnvironmentId::parse(format!("env_{environment}")).expect("environment"),
        )
    }

    fn event(tenant: &TenantScope, timestamp: u64, message: String) -> StructuredLogEvent {
        StructuredLogEvent {
            context: SignalContext::new(
                SignalScope::Tenant {
                    tenant: tenant.clone(),
                    organization_id: Some("org_example00".to_owned()),
                },
                ActorIdentity::System {
                    component: "log-producer".to_owned(),
                },
                ResourceReference::new("environment", tenant.environment_id().as_str())
                    .expect("resource"),
                RequestId::parse("req_logstore00").expect("request"),
                CorrelationId::parse("cor_logstore00").expect("correlation"),
                None,
            )
            .expect("context"),
            occurred_at_unix_milliseconds: timestamp,
            service: "data-plane".to_owned(),
            region: "local".to_owned(),
            level: LogLevel::Error,
            event: "log.failed".to_owned(),
            message,
            attributes: SafeAttributes::try_from_iter([(
                "response.body".to_owned(),
                AttributeValue::Text("private body".to_owned()),
            )])
            .expect("attributes"),
        }
    }
}
