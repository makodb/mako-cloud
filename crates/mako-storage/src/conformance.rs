use std::{num::NonZeroUsize, sync::Arc};

use async_trait::async_trait;
use futures::future;

use crate::{
    AtomicWrite, Capability, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter,
    ScanDirection, ScanRequest, StorageError, StorageErrorKind, StorageResult, TenantKeyspace,
    WriteBatch,
};

#[async_trait]
pub trait ConformanceFactory: Send + Sync {
    async fn open(&self) -> StorageResult<Arc<dyn KvAdapter>>;

    /// Drops the suite's last adapter handle before reopening the same logical store.
    async fn restart(&self, adapter: Arc<dyn KvAdapter>) -> StorageResult<Arc<dyn KvAdapter>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConformanceCheck {
    TenantIsolation,
    OrderedScans,
    AtomicBatch,
    StableSnapshot,
    ConditionalConflict,
    ConditionalRace,
    RestartDurability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConformanceOutcome {
    Passed,
    Skipped { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConformanceReport {
    pub adapter_capabilities: crate::AdapterCapabilities,
    pub checks: Vec<(ConformanceCheck, ConformanceOutcome)>,
}

impl ConformanceReport {
    #[must_use]
    pub fn passed(&self, check: ConformanceCheck) -> bool {
        self.checks.iter().any(|(candidate, outcome)| {
            *candidate == check && *outcome == ConformanceOutcome::Passed
        })
    }

    #[must_use]
    pub fn skipped(&self, check: ConformanceCheck) -> bool {
        self.checks.iter().any(|(candidate, outcome)| {
            *candidate == check && matches!(outcome, ConformanceOutcome::Skipped { .. })
        })
    }
}

/// Runs semantic checks against a fresh logical store from `factory`.
pub async fn run_adapter_conformance(
    factory: &dyn ConformanceFactory,
) -> StorageResult<ConformanceReport> {
    let adapter = factory.open().await?;
    let capabilities = adapter.capabilities();
    let required = [
        Capability::PointReads,
        Capability::OrderedBoundedScans,
        Capability::StableSnapshots,
        Capability::AtomicBatches,
        Capability::AtomicConditionalWrites,
    ];
    if let Some(missing) = required
        .into_iter()
        .find(|capability| !capabilities.supports(*capability))
    {
        return Err(conformance_error(format!(
            "adapter does not claim required conformance capability {missing:?}"
        )));
    }

    let first = TenantKeyspace::new(b"conformance-project-a".as_slice(), b"environment")
        .map_err(|_| conformance_error("could not encode first tenant"))?;
    let second = TenantKeyspace::new(b"conformance-project-b".as_slice(), b"environment")
        .map_err(|_| conformance_error("could not encode second tenant"))?;
    check_tenant_isolation(adapter.as_ref(), &first, &second).await?;
    check_ordering(adapter.as_ref(), &first).await?;
    check_atomic_batch(adapter.as_ref(), &first).await?;
    check_snapshot(adapter.as_ref(), &first).await?;
    check_conditional_conflict(adapter.as_ref(), &first).await?;
    check_conditional_race(Arc::clone(&adapter), &first).await?;

    let mut checks = vec![
        (
            ConformanceCheck::TenantIsolation,
            ConformanceOutcome::Passed,
        ),
        (ConformanceCheck::OrderedScans, ConformanceOutcome::Passed),
        (ConformanceCheck::AtomicBatch, ConformanceOutcome::Passed),
        (ConformanceCheck::StableSnapshot, ConformanceOutcome::Passed),
        (
            ConformanceCheck::ConditionalConflict,
            ConformanceOutcome::Passed,
        ),
        (
            ConformanceCheck::ConditionalRace,
            ConformanceOutcome::Passed,
        ),
    ];

    if capabilities.supports(Capability::DurableRestart) {
        let restart_key = first
            .idempotency_key(b"conformance", b"restart-durability")
            .map_err(|_| conformance_error("could not encode restart key"))?;
        adapter
            .write(single_put(&restart_key, b"acknowledged"), Durability::Sync)
            .await?;
        let restarted = factory.restart(adapter).await?;
        require_equal(
            restarted.get(&restart_key).await?,
            Some(b"acknowledged".to_vec()),
            "sync acknowledgement did not survive restart",
        )?;
        checks.push((
            ConformanceCheck::RestartDurability,
            ConformanceOutcome::Passed,
        ));
    } else {
        checks.push((
            ConformanceCheck::RestartDurability,
            ConformanceOutcome::Skipped {
                reason: "adapter does not claim durable restart".into(),
            },
        ));
    }

    Ok(ConformanceReport {
        adapter_capabilities: capabilities,
        checks,
    })
}

async fn check_tenant_isolation(
    adapter: &dyn KvAdapter,
    first: &TenantKeyspace,
    second: &TenantKeyspace,
) -> StorageResult<()> {
    let first_key = first
        .document_key(b"items", b"first")
        .map_err(|_| conformance_error("could not encode isolation key"))?;
    let second_key = second
        .document_key(b"items", b"second")
        .map_err(|_| conformance_error("could not encode isolation key"))?;
    let mut batch = WriteBatch::new();
    batch.put(&first_key, b"first-tenant");
    batch.put(&second_key, b"second-tenant");
    adapter.write(batch, Durability::Memory).await?;

    let entries = adapter
        .scan(scan(
            first
                .environment_range()
                .map_err(|_| conformance_error("could not encode environment range"))?,
            ScanDirection::Forward,
            100,
        ))
        .await?;
    require(
        entries.iter().any(|entry| entry.key == first_key),
        "tenant scan omitted its own key",
    )?;
    require(
        entries.iter().all(|entry| entry.key != second_key),
        "tenant scan crossed into another project range",
    )
}

async fn check_ordering(adapter: &dyn KvAdapter, tenant: &TenantKeyspace) -> StorageResult<()> {
    let collection = b"ordered";
    let keys = [
        tenant
            .document_key(collection, b"c")
            .map_err(|_| conformance_error("could not encode ordering key"))?,
        tenant
            .document_key(collection, b"a")
            .map_err(|_| conformance_error("could not encode ordering key"))?,
        tenant
            .document_key(collection, b"b")
            .map_err(|_| conformance_error("could not encode ordering key"))?,
    ];
    let mut batch = WriteBatch::new();
    for key in &keys {
        batch.put(key, key);
    }
    adapter.write(batch, Durability::Memory).await?;
    let range = tenant
        .documents_range(collection)
        .map_err(|_| conformance_error("could not encode ordering range"))?;
    let forward = adapter
        .scan(scan(range.clone(), ScanDirection::Forward, 3))
        .await?;
    let reverse = adapter.scan(scan(range, ScanDirection::Reverse, 3)).await?;
    let mut expected = keys.to_vec();
    expected.sort();
    require_equal(
        forward
            .iter()
            .map(|entry| entry.key.clone())
            .collect::<Vec<_>>(),
        expected.clone(),
        "forward scan order differs from byte-key order",
    )?;
    expected.reverse();
    require_equal(
        reverse
            .iter()
            .map(|entry| entry.key.clone())
            .collect::<Vec<_>>(),
        expected,
        "reverse scan order differs from byte-key order",
    )
}

async fn check_atomic_batch(adapter: &dyn KvAdapter, tenant: &TenantKeyspace) -> StorageResult<()> {
    let first = tenant
        .idempotency_key(b"atomic", b"one")
        .map_err(|_| conformance_error("could not encode atomic key"))?;
    let second = tenant
        .idempotency_key(b"atomic", b"two")
        .map_err(|_| conformance_error("could not encode atomic key"))?;
    let mut batch = WriteBatch::new();
    batch.put(&first, b"one");
    batch.put(&second, b"two");
    adapter.write(batch, Durability::Memory).await?;
    require_equal(
        adapter.get(&first).await?,
        Some(b"one".to_vec()),
        "atomic batch lost first key",
    )?;
    require_equal(
        adapter.get(&second).await?,
        Some(b"two".to_vec()),
        "atomic batch lost second key",
    )
}

async fn check_snapshot(adapter: &dyn KvAdapter, tenant: &TenantKeyspace) -> StorageResult<()> {
    let key = tenant
        .idempotency_key(b"snapshot", b"stable")
        .map_err(|_| conformance_error("could not encode snapshot key"))?;
    adapter
        .write(single_put(&key, b"before"), Durability::Memory)
        .await?;
    let snapshot = adapter.snapshot().await?;
    adapter
        .write(single_put(&key, b"after"), Durability::Memory)
        .await?;
    require_equal(
        snapshot.get(&key).await?,
        Some(b"before".to_vec()),
        "snapshot observed a later write",
    )?;
    require_equal(
        adapter.get(&key).await?,
        Some(b"after".to_vec()),
        "current view omitted a later write",
    )
}

async fn check_conditional_conflict(
    adapter: &dyn KvAdapter,
    tenant: &TenantKeyspace,
) -> StorageResult<()> {
    let guard = tenant
        .idempotency_key(b"condition", b"guard")
        .map_err(|_| conformance_error("could not encode condition key"))?;
    let derived = tenant
        .idempotency_key(b"condition", b"derived")
        .map_err(|_| conformance_error("could not encode condition key"))?;
    adapter
        .write(single_put(&guard, b"current"), Durability::Memory)
        .await?;
    let mut batch = WriteBatch::new();
    batch.put(&guard, b"incorrect");
    batch.put(&derived, b"must-not-exist");
    let result = adapter
        .compare_and_write(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key: guard.clone(),
                value: b"stale".to_vec(),
            }],
            batch,
            durability: Durability::Memory,
        })
        .await?;
    require(
        matches!(result, CompareAndWriteResult::Conflict { .. }),
        "stale condition unexpectedly committed",
    )?;
    require_equal(
        adapter.get(&guard).await?,
        Some(b"current".to_vec()),
        "conflict changed guard",
    )?;
    require_equal(
        adapter.get(&derived).await?,
        None,
        "conflict partially applied batch",
    )
}

async fn check_conditional_race(
    adapter: Arc<dyn KvAdapter>,
    tenant: &TenantKeyspace,
) -> StorageResult<()> {
    let guard = tenant
        .idempotency_key(b"race", b"guard")
        .map_err(|_| conformance_error("could not encode race key"))?;
    let first_side = tenant
        .idempotency_key(b"race", b"first")
        .map_err(|_| conformance_error("could not encode race key"))?;
    let second_side = tenant
        .idempotency_key(b"race", b"second")
        .map_err(|_| conformance_error("could not encode race key"))?;
    adapter
        .write(single_put(&guard, b"base"), Durability::Memory)
        .await?;
    let first_request = conditional_update(&guard, &first_side, b"first");
    let second_request = conditional_update(&guard, &second_side, b"second");
    let first_adapter = Arc::clone(&adapter);
    let second_adapter = Arc::clone(&adapter);
    let (first_result, second_result) = future::join(
        async move { first_adapter.compare_and_write(first_request).await },
        async move { second_adapter.compare_and_write(second_request).await },
    )
    .await;
    let results = [first_result?, second_result?];
    let applied = results
        .iter()
        .filter(|result| **result == CompareAndWriteResult::Applied)
        .count();
    let conflicts = results
        .iter()
        .filter(|result| matches!(result, CompareAndWriteResult::Conflict { .. }))
        .count();
    require(
        applied == 1 && conflicts == 1,
        "conditional race did not produce exactly one winner",
    )?;
    let side_values = [
        adapter.get(&first_side).await?,
        adapter.get(&second_side).await?,
    ];
    require(
        side_values.iter().filter(|value| value.is_some()).count() == 1,
        "conditional race applied zero or multiple derived writes",
    )
}

fn conditional_update(guard: &[u8], side: &[u8], winner: &[u8]) -> AtomicWrite {
    let mut batch = WriteBatch::new();
    batch.put(guard, winner);
    batch.put(side, b"applied");
    AtomicWrite {
        conditions: vec![KeyCondition::ValueEquals {
            key: guard.to_vec(),
            value: b"base".to_vec(),
        }],
        batch,
        durability: Durability::Memory,
    }
}

fn single_put(key: &[u8], value: &[u8]) -> WriteBatch {
    let mut batch = WriteBatch::new();
    batch.put(key, value);
    batch
}

fn scan(range: KeyRange, direction: ScanDirection, limit: usize) -> ScanRequest {
    ScanRequest::new(
        range,
        direction,
        NonZeroUsize::new(limit).expect("conformance limits are non-zero"),
    )
}

fn require(condition: bool, message: &'static str) -> StorageResult<()> {
    if condition {
        Ok(())
    } else {
        Err(conformance_error(message))
    }
}

fn require_equal<T: Eq>(actual: T, expected: T, message: &'static str) -> StorageResult<()> {
    require(actual == expected, message)
}

fn conformance_error(message: impl Into<String>) -> StorageError {
    StorageError::new(
        StorageErrorKind::Corruption,
        "adapter_conformance",
        false,
        message,
    )
}
