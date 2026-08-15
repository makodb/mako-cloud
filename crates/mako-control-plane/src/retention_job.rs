use std::{collections::BTreeSet, error::Error, fmt};

use mako_api::TenantScope;
use mako_audit::{
    AuditCompactionReport, AuditStore, AuditStoreError, DetailCompactionReport, LogStore,
    LogStoreError, RetentionMode, UsageStore, UsageStoreError,
};
use mako_documents::{CompactionMode, CompactionReport, RetentionError, ScopedCollectionEngine};
use mako_storage::Durability;

#[derive(Clone)]
pub struct CollectionRetentionTarget {
    pub engine: ScopedCollectionEngine,
    pub compact_through_position: u64,
}

#[derive(Clone)]
pub struct RetentionJob {
    tenant: TenantScope,
    audit: AuditStore,
    usage: UsageStore,
    logs: LogStore,
    collections: Vec<CollectionRetentionTarget>,
    durability: Durability,
}

impl RetentionJob {
    pub fn new(
        tenant: TenantScope,
        audit: AuditStore,
        usage: UsageStore,
        logs: LogStore,
        collections: Vec<CollectionRetentionTarget>,
        durability: Durability,
    ) -> Result<Self, RetentionJobError> {
        let mut collection_ids = BTreeSet::new();
        for target in &collections {
            if target.engine.scope().tenant() != &tenant {
                return Err(RetentionJobError::TenantMismatch);
            }
            if !collection_ids.insert(target.engine.scope().collection_id().as_str().to_owned()) {
                return Err(RetentionJobError::DuplicateCollection);
            }
        }
        Ok(Self {
            tenant,
            audit,
            usage,
            logs,
            collections,
            durability,
        })
    }

    /// Runs one bounded tenant retention pass. Dry-run mode is side-effect
    /// free across every store and reports the exact same candidate classes as
    /// apply mode.
    pub async fn run(
        &self,
        now_unix_milliseconds: u64,
        mode: RetentionMode,
    ) -> Result<RetentionJobReport, RetentionJobError> {
        if now_unix_milliseconds == 0 {
            return Err(RetentionJobError::InvalidTimestamp);
        }
        let logs = self
            .logs
            .compact_expired(&self.tenant, now_unix_milliseconds, mode)
            .await?;
        let audit = self
            .audit
            .compact_expired(&self.tenant, now_unix_milliseconds, mode)
            .await?;
        let usage_detail = self
            .usage
            .compact_expired_detail(&self.tenant, now_unix_milliseconds, mode)
            .await?;
        let document_mode = match mode {
            RetentionMode::DryRun => CompactionMode::DryRun,
            RetentionMode::Apply => CompactionMode::Apply,
        };
        let mut collections = Vec::with_capacity(self.collections.len());
        for target in &self.collections {
            let report = target
                .engine
                .compact_through_with_mode(
                    target.compact_through_position,
                    self.durability,
                    document_mode,
                )
                .await?;
            collections.push(CollectionRetentionReport {
                collection_id: target.engine.scope().collection_id().as_str().to_owned(),
                report,
            });
        }
        Ok(RetentionJobReport {
            tenant: self.tenant.clone(),
            mode,
            observed_at_unix_milliseconds: now_unix_milliseconds,
            logs,
            audit,
            usage_detail,
            collections,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionJobReport {
    pub tenant: TenantScope,
    pub mode: RetentionMode,
    pub observed_at_unix_milliseconds: u64,
    pub logs: DetailCompactionReport,
    pub audit: AuditCompactionReport,
    pub usage_detail: DetailCompactionReport,
    pub collections: Vec<CollectionRetentionReport>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionRetentionReport {
    pub collection_id: String,
    pub report: CompactionReport,
}

#[derive(Debug)]
pub enum RetentionJobError {
    InvalidTimestamp,
    TenantMismatch,
    DuplicateCollection,
    Logs(LogStoreError),
    Audit(AuditStoreError),
    Usage(UsageStoreError),
    Documents(RetentionError),
}

impl fmt::Display for RetentionJobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTimestamp => "retention job timestamp is invalid",
            Self::TenantMismatch => "retention target tenant does not match the job tenant",
            Self::DuplicateCollection => "retention job contains a duplicate collection",
            Self::Logs(_) => "log retention failed",
            Self::Audit(_) => "audit retention failed",
            Self::Usage(_) => "usage-detail retention failed",
            Self::Documents(_) => "document-history retention failed",
        })
    }
}

impl Error for RetentionJobError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Logs(error) => Some(error),
            Self::Audit(error) => Some(error),
            Self::Usage(error) => Some(error),
            Self::Documents(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LogStoreError> for RetentionJobError {
    fn from(value: LogStoreError) -> Self {
        Self::Logs(value)
    }
}

impl From<AuditStoreError> for RetentionJobError {
    fn from(value: AuditStoreError) -> Self {
        Self::Audit(value)
    }
}

impl From<UsageStoreError> for RetentionJobError {
    fn from(value: UsageStoreError) -> Self {
        Self::Usage(value)
    }
}

impl From<RetentionError> for RetentionJobError {
    fn from(value: RetentionError) -> Self {
        Self::Documents(value)
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, sync::Arc};

    use futures::executor::block_on;
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId};
    use mako_audit::{AuditStoreConfig, CursorSigningKey, LogStoreConfig, UsageStoreConfig};
    use mako_documents::DocumentEngine;
    use mako_storage::MemoryAdapter;

    use super::*;

    #[test]
    fn one_job_reports_every_retention_class_without_mutating_in_dry_run() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let tenant = test_tenant("example00", "example00");
            let target = target(adapter.clone(), &tenant, "todos");
            let job = job(adapter, tenant.clone(), vec![target]).expect("job");

            let preview = job
                .run(20_000, RetentionMode::DryRun)
                .await
                .expect("preview");
            assert_eq!(preview.tenant, tenant);
            assert_eq!(preview.mode, RetentionMode::DryRun);
            assert!(preview.logs.dry_run);
            assert!(preview.audit.dry_run);
            assert!(preview.usage_detail.dry_run);
            assert_eq!(preview.collections.len(), 1);
            assert!(preview.collections[0].report.dry_run);
            assert_eq!(preview.collections[0].report.watermark, 0);

            let applied = job.run(20_000, RetentionMode::Apply).await.expect("apply");
            assert_eq!(applied.mode, RetentionMode::Apply);
            assert!(!applied.logs.dry_run);
            assert!(!applied.audit.dry_run);
            assert!(!applied.usage_detail.dry_run);
            assert!(!applied.collections[0].report.dry_run);
        });
    }

    #[test]
    fn job_rejects_cross_tenant_and_duplicate_collection_targets() {
        let adapter = Arc::new(MemoryAdapter::new());
        let tenant = test_tenant("example00", "example00");
        let other = test_tenant("other000", "other000");
        let other_target = target(adapter.clone(), &other, "todos");
        assert!(matches!(
            job(adapter.clone(), tenant.clone(), vec![other_target]),
            Err(RetentionJobError::TenantMismatch)
        ));
        let duplicate = target(adapter.clone(), &tenant, "todos");
        assert!(matches!(
            job(
                adapter.clone(),
                tenant.clone(),
                vec![duplicate.clone(), duplicate]
            ),
            Err(RetentionJobError::DuplicateCollection)
        ));
    }

    fn job(
        adapter: Arc<MemoryAdapter>,
        tenant: TenantScope,
        collections: Vec<CollectionRetentionTarget>,
    ) -> Result<RetentionJob, RetentionJobError> {
        let audit = AuditStore::new(
            adapter.clone(),
            AuditStoreConfig {
                durability: Durability::Memory,
                retention_milliseconds: 10_000,
                maximum_page_records: NonZeroUsize::new(10).expect("non-zero"),
                maximum_export_records: NonZeroUsize::new(20).expect("non-zero"),
                maximum_examined_records: NonZeroUsize::new(100).expect("non-zero"),
                cursor_signing_key: CursorSigningKey::new([3; 32]),
            },
        )
        .expect("audit");
        let usage = UsageStore::new(
            adapter.clone(),
            UsageStoreConfig {
                durability: Durability::Memory,
                detail_retention_milliseconds: 10_000,
                bucket_width_milliseconds: NonZeroUsize::new(1_000).expect("non-zero"),
                maximum_aggregation_buckets: NonZeroUsize::new(10).expect("non-zero"),
                maximum_conflict_retries: NonZeroUsize::new(10).expect("non-zero"),
            },
        )
        .expect("usage");
        let logs = LogStore::new(
            adapter,
            LogStoreConfig {
                durability: Durability::Memory,
                retention_milliseconds: 10_000,
            },
        )
        .expect("logs");
        RetentionJob::new(tenant, audit, usage, logs, collections, Durability::Memory)
    }

    fn target(
        adapter: Arc<MemoryAdapter>,
        tenant: &TenantScope,
        collection: &str,
    ) -> CollectionRetentionTarget {
        let engine = DocumentEngine::new(adapter)
            .scope_collection(
                tenant,
                CollectionScope::new(
                    tenant.clone(),
                    CollectionId::parse(collection).expect("collection"),
                ),
            )
            .expect("scope");
        CollectionRetentionTarget {
            engine,
            compact_through_position: 0,
        }
    }

    fn test_tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(format!("prj_{project}")).expect("project"),
            EnvironmentId::parse(format!("env_{environment}")).expect("environment"),
        )
    }
}
