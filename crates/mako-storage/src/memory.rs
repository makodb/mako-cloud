use std::{
    collections::{BTreeMap, VecDeque},
    num::NonZeroUsize,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use async_trait::async_trait;

use crate::{
    AdapterCapabilities, AtomicWrite, Capability, CompareAndWriteResult, Durability, HealthReport,
    HealthStatus, KeyCondition, KeyValue, KvAdapter, KvSnapshot, KvTransaction, ScanDirection,
    ScanRequest, SnapshotId, StorageError, StorageErrorKind, StorageResult, TransactionMode,
    WriteBatch, WriteOperation,
};

const DEFAULT_MAXIMUM_BATCH_OPERATIONS: usize = 10_000;
const DEFAULT_MAXIMUM_SCAN_ITEMS: usize = 10_000;

/// Named deterministic failure locations shared by model, crash, and retry tests.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FailurePoint {
    BeforeGet,
    BeforeScan,
    BeforeSnapshot,
    BeforeWrite,
    BeforeConditionCheck,
    AfterConditionCheck,
    BeforeBatchStage,
    AfterBatchStage,
    BeforeCommit,
    AfterCommit,
    BeforeHealth,
}

#[derive(Debug, Default)]
struct MemoryState {
    version: u64,
    values: BTreeMap<Vec<u8>, Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct MemoryAdapter {
    state: Arc<RwLock<MemoryState>>,
    failures: Arc<Mutex<BTreeMap<FailurePoint, VecDeque<StorageError>>>>,
    maximum_batch_operations: NonZeroUsize,
    maximum_scan_items: NonZeroUsize,
}

impl Default for MemoryAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(
            NonZeroUsize::new(DEFAULT_MAXIMUM_BATCH_OPERATIONS).expect("non-zero constant"),
            NonZeroUsize::new(DEFAULT_MAXIMUM_SCAN_ITEMS).expect("non-zero constant"),
        )
    }

    #[must_use]
    pub fn with_limits(
        maximum_batch_operations: NonZeroUsize,
        maximum_scan_items: NonZeroUsize,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(MemoryState::default())),
            failures: Arc::new(Mutex::new(BTreeMap::new())),
            maximum_batch_operations,
            maximum_scan_items,
        }
    }

    /// Causes the next visit to `point` to return `error`.
    pub fn fail_next(&self, point: FailurePoint, error: StorageError) -> StorageResult<()> {
        let mut failures = self.failures.lock().map_err(|_| lock_error("fail_next"))?;
        failures.entry(point).or_default().push_back(error);
        Ok(())
    }

    pub fn clear_failures(&self) -> StorageResult<()> {
        self.failures
            .lock()
            .map_err(|_| lock_error("clear_failures"))?
            .clear();
        Ok(())
    }

    pub fn dump(&self) -> StorageResult<BTreeMap<Vec<u8>, Vec<u8>>> {
        Ok(self
            .state
            .read()
            .map_err(|_| lock_error("dump"))?
            .values
            .clone())
    }

    fn maybe_fail(&self, point: FailurePoint) -> StorageResult<()> {
        let mut failures = self.failures.lock().map_err(|_| lock_error("failure"))?;
        let Some(queue) = failures.get_mut(&point) else {
            return Ok(());
        };
        let failure = queue.pop_front();
        if queue.is_empty() {
            failures.remove(&point);
        }
        failure.map_or(Ok(()), Err)
    }

    fn validate_batch(&self, batch: &WriteBatch) -> StorageResult<()> {
        if batch.is_empty() {
            return Err(StorageError::invalid(
                "write",
                "write batch cannot be empty",
            ));
        }
        if batch.len() > self.maximum_batch_operations.get() {
            return Err(StorageError::invalid(
                "write",
                "write batch exceeds adapter limit",
            ));
        }
        if batch
            .operations()
            .iter()
            .any(|operation| operation.key().is_empty())
        {
            return Err(StorageError::invalid("write", "keys cannot be empty"));
        }
        Ok(())
    }

    fn validate_scan(&self, request: &ScanRequest) -> StorageResult<()> {
        if request.limit.get() > self.maximum_scan_items.get() {
            return Err(StorageError::invalid(
                "scan",
                "scan limit exceeds adapter limit",
            ));
        }
        Ok(())
    }

    fn require_memory_durability(durability: Durability) -> StorageResult<()> {
        if durability == Durability::Memory {
            Ok(())
        } else {
            Err(StorageError::unsupported(
                "durability",
                "in-memory adapter cannot acknowledge WAL or restart durability",
            ))
        }
    }

    fn apply_atomic(&self, request: AtomicWrite) -> StorageResult<CompareAndWriteResult> {
        request.validate()?;
        self.validate_batch(&request.batch)?;
        Self::require_memory_durability(request.durability)?;
        self.maybe_fail(FailurePoint::BeforeConditionCheck)?;

        let mut state = self
            .state
            .write()
            .map_err(|_| lock_error("compare_and_write"))?;
        for (index, condition) in request.conditions.iter().enumerate() {
            let actual = state.values.get(condition.key());
            let matches = match condition {
                KeyCondition::Missing { .. } => actual.is_none(),
                KeyCondition::Present { .. } => actual.is_some(),
                KeyCondition::ValueEquals { value, .. } => actual == Some(value),
            };
            if !matches {
                return Ok(CompareAndWriteResult::Conflict {
                    failed_condition: index,
                    actual_value: actual.cloned(),
                });
            }
        }

        self.maybe_fail(FailurePoint::AfterConditionCheck)?;
        self.maybe_fail(FailurePoint::BeforeBatchStage)?;
        let mut next = state.values.clone();
        apply_batch(&mut next, &request.batch);
        self.maybe_fail(FailurePoint::AfterBatchStage)?;
        self.maybe_fail(FailurePoint::BeforeCommit)?;
        state.values = next;
        state.version = state.version.checked_add(1).ok_or_else(|| {
            StorageError::new(
                StorageErrorKind::Internal,
                "commit",
                false,
                "in-memory version exhausted",
            )
        })?;
        drop(state);
        self.maybe_fail(FailurePoint::AfterCommit)?;
        Ok(CompareAndWriteResult::Applied)
    }
}

#[async_trait]
impl KvAdapter for MemoryAdapter {
    async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        if key.is_empty() {
            return Err(StorageError::invalid("get", "key cannot be empty"));
        }
        self.maybe_fail(FailurePoint::BeforeGet)?;
        Ok(self
            .state
            .read()
            .map_err(|_| lock_error("get"))?
            .values
            .get(key)
            .cloned())
    }

    async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
        self.validate_scan(&request)?;
        self.maybe_fail(FailurePoint::BeforeScan)?;
        let state = self.state.read().map_err(|_| lock_error("scan"))?;
        Ok(scan_map(&state.values, &request))
    }

    async fn snapshot(&self) -> StorageResult<Box<dyn KvSnapshot>> {
        self.maybe_fail(FailurePoint::BeforeSnapshot)?;
        let state = self.state.read().map_err(|_| lock_error("snapshot"))?;
        Ok(Box::new(MemorySnapshot {
            id: SnapshotId(state.version),
            values: state.values.clone(),
            maximum_scan_items: self.maximum_scan_items,
        }))
    }

    async fn write(&self, batch: WriteBatch, durability: Durability) -> StorageResult<()> {
        self.maybe_fail(FailurePoint::BeforeWrite)?;
        match self.apply_atomic(AtomicWrite::unconditional(batch, durability))? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(StorageError::new(
                StorageErrorKind::Internal,
                "write",
                false,
                "unconditional batch reported a conflict",
            )),
        }
    }

    async fn compare_and_write(
        &self,
        request: AtomicWrite,
    ) -> StorageResult<CompareAndWriteResult> {
        self.maybe_fail(FailurePoint::BeforeWrite)?;
        self.apply_atomic(request)
    }

    async fn begin_transaction(
        &self,
        mode: TransactionMode,
    ) -> StorageResult<Box<dyn KvTransaction>> {
        let state = self
            .state
            .read()
            .map_err(|_| lock_error("begin_transaction"))?;
        Ok(Box::new(MemoryTransaction {
            adapter: self.clone(),
            base_version: state.version,
            working: state.values.clone(),
            batch: WriteBatch::new(),
            mode,
        }))
    }

    async fn health(&self) -> StorageResult<HealthReport> {
        self.maybe_fail(FailurePoint::BeforeHealth)?;
        let _guard = self.state.read().map_err(|_| lock_error("health"))?;
        Ok(HealthReport {
            status: HealthStatus::Healthy,
            latency: Duration::ZERO,
            durability_verified: false,
            safe_diagnostic: Some(
                "deterministic in-memory adapter; restart durability absent".into(),
            ),
        })
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::new(
            [
                Capability::PointReads,
                Capability::OrderedBoundedScans,
                Capability::StableSnapshots,
                Capability::AtomicBatches,
                Capability::AtomicConditionalWrites,
                Capability::SerializableTransactions,
            ],
            self.maximum_batch_operations,
            self.maximum_scan_items,
            Durability::Memory,
        )
    }
}

#[derive(Debug)]
struct MemorySnapshot {
    id: SnapshotId,
    values: BTreeMap<Vec<u8>, Vec<u8>>,
    maximum_scan_items: NonZeroUsize,
}

#[async_trait]
impl KvSnapshot for MemorySnapshot {
    fn id(&self) -> SnapshotId {
        self.id
    }

    async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        if key.is_empty() {
            return Err(StorageError::invalid("snapshot_get", "key cannot be empty"));
        }
        Ok(self.values.get(key).cloned())
    }

    async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
        if request.limit.get() > self.maximum_scan_items.get() {
            return Err(StorageError::invalid(
                "snapshot_scan",
                "scan limit exceeds adapter limit",
            ));
        }
        Ok(scan_map(&self.values, &request))
    }
}

#[derive(Debug)]
struct MemoryTransaction {
    adapter: MemoryAdapter,
    base_version: u64,
    working: BTreeMap<Vec<u8>, Vec<u8>>,
    batch: WriteBatch,
    mode: TransactionMode,
}

#[async_trait]
impl KvTransaction for MemoryTransaction {
    async fn get_for_update(&mut self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        if key.is_empty() {
            return Err(StorageError::invalid(
                "transaction_get",
                "key cannot be empty",
            ));
        }
        Ok(self.working.get(key).cloned())
    }

    async fn scan_for_update(&mut self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
        self.adapter.validate_scan(&request)?;
        Ok(scan_map(&self.working, &request))
    }

    fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> StorageResult<()> {
        if key.is_empty() {
            return Err(StorageError::invalid(
                "transaction_put",
                "key cannot be empty",
            ));
        }
        if self.batch.len() >= self.adapter.maximum_batch_operations.get() {
            return Err(StorageError::invalid(
                "transaction_put",
                "transaction exceeds adapter batch limit",
            ));
        }
        self.working.insert(key.clone(), value.clone());
        self.batch.put(key, value);
        Ok(())
    }

    fn delete(&mut self, key: Vec<u8>) -> StorageResult<()> {
        if key.is_empty() {
            return Err(StorageError::invalid(
                "transaction_delete",
                "key cannot be empty",
            ));
        }
        if self.batch.len() >= self.adapter.maximum_batch_operations.get() {
            return Err(StorageError::invalid(
                "transaction_delete",
                "transaction exceeds adapter batch limit",
            ));
        }
        self.working.remove(&key);
        self.batch.delete(key);
        Ok(())
    }

    async fn commit(self: Box<Self>, durability: Durability) -> StorageResult<()> {
        MemoryAdapter::require_memory_durability(durability)?;
        if self.batch.is_empty() {
            return Err(StorageError::invalid(
                "transaction_commit",
                "transaction has no writes",
            ));
        }
        self.adapter.maybe_fail(FailurePoint::BeforeCommit)?;
        let mut state = self
            .adapter
            .state
            .write()
            .map_err(|_| lock_error("transaction_commit"))?;
        if state.version != self.base_version {
            return Err(StorageError::new(
                StorageErrorKind::Conflict,
                "transaction_commit",
                true,
                match self.mode {
                    TransactionMode::CompareAndWrite => "transaction base changed",
                    TransactionMode::Serializable => "serializable transaction conflicted",
                },
            ));
        }
        state.values = self.working;
        state.version = state.version.checked_add(1).ok_or_else(|| {
            StorageError::new(
                StorageErrorKind::Internal,
                "transaction_commit",
                false,
                "in-memory version exhausted",
            )
        })?;
        drop(state);
        self.adapter.maybe_fail(FailurePoint::AfterCommit)
    }

    async fn rollback(self: Box<Self>) -> StorageResult<()> {
        Ok(())
    }
}

fn apply_batch(values: &mut BTreeMap<Vec<u8>, Vec<u8>>, batch: &WriteBatch) {
    for operation in batch.operations() {
        match operation {
            WriteOperation::Put { key, value } => {
                values.insert(key.clone(), value.clone());
            }
            WriteOperation::Delete { key } => {
                values.remove(key);
            }
        }
    }
}

fn scan_map(values: &BTreeMap<Vec<u8>, Vec<u8>>, request: &ScanRequest) -> Vec<KeyValue> {
    let range =
        values.range(request.range.start_inclusive.clone()..request.range.end_exclusive.clone());
    match request.direction {
        ScanDirection::Forward => range.take(request.limit.get()).map(to_key_value).collect(),
        ScanDirection::Reverse => range
            .rev()
            .take(request.limit.get())
            .map(to_key_value)
            .collect(),
    }
}

fn to_key_value((key, value): (&Vec<u8>, &Vec<u8>)) -> KeyValue {
    KeyValue {
        key: key.clone(),
        value: value.clone(),
    }
}

fn lock_error(operation: &'static str) -> StorageError {
    StorageError::new(
        StorageErrorKind::Internal,
        operation,
        false,
        "in-memory adapter lock is unavailable",
    )
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use futures::executor::block_on;

    use super::*;
    use crate::KeyRange;

    fn scan_request(direction: ScanDirection) -> ScanRequest {
        ScanRequest::new(
            KeyRange::new(b"a".to_vec(), b"z".to_vec()).expect("range"),
            direction,
            NonZeroUsize::new(10).expect("non-zero"),
        )
    }

    fn put(key: &[u8], value: &[u8]) -> WriteBatch {
        let mut batch = WriteBatch::new();
        batch.put(key, value);
        batch
    }

    #[test]
    fn scans_are_deterministic_in_both_directions() {
        block_on(async {
            let adapter = MemoryAdapter::new();
            let mut batch = WriteBatch::new();
            batch.put(b"b", b"2");
            batch.put(b"a", b"1");
            batch.put(b"c", b"3");
            adapter
                .write(batch, Durability::Memory)
                .await
                .expect("write");

            let forward = adapter
                .scan(scan_request(ScanDirection::Forward))
                .await
                .expect("scan");
            let reverse = adapter
                .scan(scan_request(ScanDirection::Reverse))
                .await
                .expect("scan");
            assert_eq!(
                forward
                    .iter()
                    .map(|entry| entry.key.as_slice())
                    .collect::<Vec<_>>(),
                [b"a", b"b", b"c"]
            );
            assert_eq!(
                reverse
                    .iter()
                    .map(|entry| entry.key.as_slice())
                    .collect::<Vec<_>>(),
                [b"c", b"b", b"a"]
            );
        });
    }

    #[test]
    fn snapshots_remain_stable_after_new_commits() {
        block_on(async {
            let adapter = MemoryAdapter::new();
            adapter
                .write(put(b"key", b"old"), Durability::Memory)
                .await
                .expect("write");
            let snapshot = adapter.snapshot().await.expect("snapshot");
            adapter
                .write(put(b"key", b"new"), Durability::Memory)
                .await
                .expect("write");

            assert_eq!(
                snapshot.get(b"key").await.expect("snapshot get"),
                Some(b"old".to_vec())
            );
            assert_eq!(
                adapter.get(b"key").await.expect("adapter get"),
                Some(b"new".to_vec())
            );
        });
    }

    #[test]
    fn failed_condition_leaves_the_entire_batch_unapplied() {
        block_on(async {
            let adapter = MemoryAdapter::new();
            adapter
                .write(put(b"guard", b"current"), Durability::Memory)
                .await
                .expect("write");
            let mut batch = WriteBatch::new();
            batch.put(b"guard", b"next");
            batch.put(b"derived", b"value");
            let result = adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::ValueEquals {
                        key: b"guard".to_vec(),
                        value: b"stale".to_vec(),
                    }],
                    batch,
                    durability: Durability::Memory,
                })
                .await
                .expect("compare result");

            assert!(matches!(result, CompareAndWriteResult::Conflict { .. }));
            assert_eq!(
                adapter.get(b"guard").await.expect("get"),
                Some(b"current".to_vec())
            );
            assert_eq!(adapter.get(b"derived").await.expect("get"), None);
        });
    }

    #[test]
    fn injected_precommit_failure_leaves_no_partial_state() {
        block_on(async {
            let adapter = MemoryAdapter::new();
            adapter
                .fail_next(
                    FailurePoint::BeforeCommit,
                    StorageError::new(
                        StorageErrorKind::Io,
                        "injected",
                        true,
                        "injected I/O failure",
                    ),
                )
                .expect("inject failure");
            let mut batch = WriteBatch::new();
            batch.put(b"one", b"1");
            batch.put(b"two", b"2");

            assert!(adapter.write(batch, Durability::Memory).await.is_err());
            assert!(adapter.dump().expect("dump").is_empty());
        });
    }

    #[test]
    fn optimistic_serializable_transactions_detect_races() {
        block_on(async {
            let adapter = MemoryAdapter::new();
            adapter
                .write(put(b"key", b"base"), Durability::Memory)
                .await
                .expect("seed");
            let mut first = adapter
                .begin_transaction(TransactionMode::Serializable)
                .await
                .expect("first");
            let mut second = adapter
                .begin_transaction(TransactionMode::Serializable)
                .await
                .expect("second");
            first.put(b"key".to_vec(), b"first".to_vec()).expect("put");
            second
                .put(b"key".to_vec(), b"second".to_vec())
                .expect("put");

            first
                .commit(Durability::Memory)
                .await
                .expect("first commit");
            let error = second
                .commit(Durability::Memory)
                .await
                .expect_err("second conflicts");
            assert_eq!(error.kind, StorageErrorKind::Conflict);
            assert_eq!(
                adapter.get(b"key").await.expect("get"),
                Some(b"first".to_vec())
            );
        });
    }
}
