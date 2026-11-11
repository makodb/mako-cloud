use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use futures::channel::oneshot;
use rocksdb::{
    DBAccess, DBRawIteratorWithThreadMode, Error as RocksError, ErrorKind as RocksErrorKind,
    OptimisticTransactionDB, OptimisticTransactionOptions, Options, ReadOptions,
    SnapshotWithThreadMode, WriteBatchWithTransaction, WriteOptions, checkpoint::Checkpoint,
};

use crate::{
    AdapterCapabilities, AtomicWrite, Capability, CompareAndWriteResult, Durability, HealthReport,
    HealthStatus, KeyCondition, KeyRange, KeyValue, KvAdapter, KvSnapshot, KvTransaction,
    ScanDirection, ScanRequest, SnapshotId, StorageError, StorageErrorKind, StorageResult,
    TransactionMode, WriteBatch, WriteOperation,
};

const DEFAULT_MAXIMUM_BATCH_OPERATIONS: usize = 10_000;
const DEFAULT_MAXIMUM_SCAN_ITEMS: usize = 10_000;

/// Local RocksDB settings. A configured minimum upgrades weaker call-site requests.
#[derive(Clone, Debug)]
pub struct RocksDbConfig {
    pub path: PathBuf,
    pub create_if_missing: bool,
    pub minimum_durability: Durability,
    pub maximum_batch_operations: NonZeroUsize,
    pub maximum_scan_items: NonZeroUsize,
    pub transaction_lock_timeout: Duration,
    pub transaction_expiration: Duration,
}

impl RocksDbConfig {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            create_if_missing: true,
            minimum_durability: Durability::Sync,
            maximum_batch_operations: NonZeroUsize::new(DEFAULT_MAXIMUM_BATCH_OPERATIONS)
                .expect("non-zero constant"),
            maximum_scan_items: NonZeroUsize::new(DEFAULT_MAXIMUM_SCAN_ITEMS)
                .expect("non-zero constant"),
            transaction_lock_timeout: Duration::from_secs(2),
            transaction_expiration: Duration::from_secs(30),
        }
    }

    fn validate(&self) -> StorageResult<()> {
        if self.path.as_os_str().is_empty() {
            return Err(StorageError::invalid(
                "rocksdb_open",
                "database path cannot be empty",
            ));
        }
        duration_millis(self.transaction_lock_timeout, "transaction_lock_timeout")?;
        duration_millis(self.transaction_expiration, "transaction_expiration")?;
        Ok(())
    }
}

/// Local durable adapter backed by RocksDB's optimistic transactional database.
#[derive(Clone)]
pub struct RocksDbAdapter {
    db: Arc<OptimisticTransactionDB>,
    config: Arc<RocksDbConfig>,
    next_snapshot_id: Arc<AtomicU64>,
    error_counters: Arc<RocksErrorCounters>,
    operation_lock: Arc<RwLock<()>>,
}

#[derive(Debug, Default)]
struct RocksErrorCounters {
    io: AtomicU64,
    corruption: AtomicU64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RocksDbHealthSignals {
    pub write_stopped: bool,
    pub delayed_write_rate: u64,
    pub pending_compaction_bytes: u64,
    pub running_compactions: u64,
    pub running_flushes: u64,
    pub background_errors: u64,
    pub io_errors: u64,
    pub corruption_signals: u64,
}

impl std::fmt::Debug for RocksDbAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RocksDbAdapter")
            .field("path", &self.config.path)
            .field("minimum_durability", &self.config.minimum_durability)
            .finish_non_exhaustive()
    }
}

impl RocksDbAdapter {
    pub fn open(config: RocksDbConfig) -> StorageResult<Self> {
        config.validate()?;
        let mut options = Options::default();
        options.create_if_missing(config.create_if_missing);
        options.set_use_fsync(true);
        options.set_paranoid_checks(true);
        options.set_track_and_verify_wals_in_manifest(true);

        let db = OptimisticTransactionDB::open(&options, &config.path)
            .map_err(|error| map_rocks_error("rocksdb_open", error, None))?;

        Ok(Self {
            db: Arc::new(db),
            config: Arc::new(config),
            next_snapshot_id: Arc::new(AtomicU64::new(1)),
            error_counters: Arc::new(RocksErrorCounters::default()),
            operation_lock: Arc::new(RwLock::new(())),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.config.path
    }

    pub fn health_signals(&self) -> StorageResult<RocksDbHealthSignals> {
        Ok(RocksDbHealthSignals {
            write_stopped: self.property("rocksdb.is-write-stopped")? != 0,
            delayed_write_rate: self.property("rocksdb.actual-delayed-write-rate")?,
            pending_compaction_bytes: self.property("rocksdb.estimate-pending-compaction-bytes")?,
            running_compactions: self.property("rocksdb.num-running-compactions")?,
            running_flushes: self.property("rocksdb.num-running-flushes")?,
            background_errors: self.property("rocksdb.background-errors")?,
            io_errors: self.error_counters.io.load(Ordering::Relaxed),
            corruption_signals: self.error_counters.corruption.load(Ordering::Relaxed),
        })
    }

    pub(crate) fn create_checkpoint(&self, target: &Path) -> StorageResult<()> {
        let _exclusive = self.operation_lock.write().map_err(|_| {
            StorageError::new(
                StorageErrorKind::Internal,
                "checkpoint",
                false,
                "storage operation lock is unavailable",
            )
        })?;
        if target.exists() {
            return Err(StorageError::new(
                StorageErrorKind::Conflict,
                "checkpoint",
                false,
                "checkpoint target already exists",
            ));
        }
        let checkpoint = Checkpoint::new(self.db.as_ref())
            .map_err(|error| self.map_error("checkpoint", error))?;
        checkpoint
            .create_checkpoint(target)
            .map_err(|error| self.map_error("checkpoint", error))
    }

    fn property(&self, name: &'static str) -> StorageResult<u64> {
        self.db
            .property_int_value(name)
            .map(Option::unwrap_or_default)
            .map_err(|error| self.map_error("rocksdb_property", error))
    }

    fn map_error(&self, operation: &'static str, error: RocksError) -> StorageError {
        map_rocks_error(operation, error, Some(self.error_counters.as_ref()))
    }

    fn effective_durability(&self, requested: Durability) -> Durability {
        requested.max(self.config.minimum_durability)
    }

    fn validate_batch(&self, batch: &WriteBatch) -> StorageResult<()> {
        if batch.is_empty() {
            return Err(StorageError::invalid(
                "write",
                "write batch cannot be empty",
            ));
        }
        if batch.len() > self.config.maximum_batch_operations.get() {
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
        if request.limit.get() > self.config.maximum_scan_items.get() {
            return Err(StorageError::invalid(
                "scan",
                "scan limit exceeds adapter limit",
            ));
        }
        Ok(())
    }

    fn transaction_options(&self) -> StorageResult<OptimisticTransactionOptions> {
        duration_millis(
            self.config.transaction_lock_timeout,
            "transaction_lock_timeout",
        )?;
        duration_millis(self.config.transaction_expiration, "transaction_expiration")?;
        let mut options = OptimisticTransactionOptions::new();
        options.set_snapshot(true);
        Ok(options)
    }
}

#[async_trait]
impl KvAdapter for RocksDbAdapter {
    async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        if key.is_empty() {
            return Err(StorageError::invalid("get", "key cannot be empty"));
        }
        self.db
            .get(key)
            .map_err(|error| self.map_error("get", error))
    }

    async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
        self.validate_scan(&request)?;
        scan_db(self.db.as_ref(), &request, self.error_counters.as_ref())
    }

    async fn snapshot(&self) -> StorageResult<Box<dyn KvSnapshot>> {
        let id = SnapshotId(self.next_snapshot_id.fetch_add(1, Ordering::Relaxed));
        let (commands, receiver) = mpsc::channel();
        let (ready, readiness) = mpsc::sync_channel(0);
        let db = Arc::clone(&self.db);
        let error_counters = Arc::clone(&self.error_counters);
        let maximum_scan_items = self.config.maximum_scan_items;
        let worker = thread::Builder::new()
            .name(format!("mako-rocks-snapshot-{}", id.0))
            .spawn(move || serve_snapshot(db, receiver, ready, error_counters))
            .map_err(|_| {
                StorageError::new(
                    StorageErrorKind::Unavailable,
                    "snapshot",
                    true,
                    "snapshot worker could not start",
                )
            })?;
        readiness
            .recv()
            .map_err(|_| snapshot_worker_error("snapshot"))?;
        Ok(Box::new(RocksSnapshot {
            id,
            commands,
            maximum_scan_items,
            worker: Mutex::new(Some(worker)),
        }))
    }

    async fn write(&self, batch: WriteBatch, durability: Durability) -> StorageResult<()> {
        let _operation = self.operation_lock.read().map_err(|_| {
            StorageError::new(
                StorageErrorKind::Internal,
                "write",
                false,
                "storage operation lock is unavailable",
            )
        })?;
        self.validate_batch(&batch)?;
        let rocks_batch = build_rocks_batch(&batch);
        let options = write_options(self.effective_durability(durability));
        self.db
            .write_opt(rocks_batch, &options)
            .map_err(|error| self.map_error("write", error))
    }

    async fn compare_and_write(
        &self,
        request: AtomicWrite,
    ) -> StorageResult<CompareAndWriteResult> {
        let _operation = self.operation_lock.read().map_err(|_| {
            StorageError::new(
                StorageErrorKind::Internal,
                "compare_and_write",
                false,
                "storage operation lock is unavailable",
            )
        })?;
        request.validate()?;
        self.validate_batch(&request.batch)?;
        let write_options = write_options(self.effective_durability(request.durability));
        let transaction_options = self.transaction_options()?;
        for attempt in 0..=16 {
            let transaction = self
                .db
                .transaction_opt(&write_options, &transaction_options);

            for (index, condition) in request.conditions.iter().enumerate() {
                let actual = transaction
                    .get_for_update(condition.key(), true)
                    .map_err(|error| self.map_error("condition_read", error))?;
                let matches = match condition {
                    KeyCondition::Missing { .. } => actual.is_none(),
                    KeyCondition::Present { .. } => actual.is_some(),
                    KeyCondition::ValueEquals { value, .. } => actual.as_ref() == Some(value),
                };
                if !matches {
                    transaction
                        .rollback()
                        .map_err(|error| self.map_error("transaction_rollback", error))?;
                    return Ok(CompareAndWriteResult::Conflict {
                        failed_condition: index,
                        actual_value: actual,
                    });
                }
            }

            for operation in request.batch.operations() {
                match operation {
                    WriteOperation::Put { key, value } => transaction
                        .put(key, value)
                        .map_err(|error| self.map_error("transaction_put", error))?,
                    WriteOperation::Delete { key } => transaction
                        .delete(key)
                        .map_err(|error| self.map_error("transaction_delete", error))?,
                }
            }
            match transaction.commit() {
                Ok(()) => return Ok(CompareAndWriteResult::Applied),
                // Optimistic commit conflicts mean nothing was written. Start a
                // fresh transaction and re-evaluate every condition: a competing
                // writer normally makes this a logical CAS conflict. Do not retry
                // ambiguous I/O or other failures, nor reuse the stale transaction.
                Err(error)
                    if matches!(
                        error.kind(),
                        RocksErrorKind::Busy | RocksErrorKind::TryAgain
                    ) && attempt < 16 =>
                {
                    thread::yield_now();
                }
                Err(error) => return Err(self.map_error("transaction_commit", error)),
            }
        }
        unreachable!("the final attempt returns its commit result")
    }

    async fn begin_transaction(
        &self,
        _mode: TransactionMode,
    ) -> StorageResult<Box<dyn KvTransaction>> {
        Err(StorageError::unsupported(
            "begin_transaction",
            "local adapter exposes atomic compare-and-write transactions",
        ))
    }

    async fn stored_bytes(&self, range: KeyRange) -> StorageResult<u64> {
        // get_approximate_sizes was the obvious answer and is the wrong one: it
        // reads table statistics that exclude the memtable, so it reported zero
        // for thirty-two kilobytes of freshly written data. A tenant would be
        // measured as holding nothing until a flush happened to occur.
        //
        // So this walks the range instead. It is the reason this is a sampled
        // measurement rather than one taken per request.
        let mut options = ReadOptions::default();
        options.set_iterate_lower_bound(range.start_inclusive.clone());
        options.set_iterate_upper_bound(range.end_exclusive.clone());
        let mut iterator = self.db.raw_iterator_opt(options);
        iterator.seek(&range.start_inclusive);
        let mut total: u64 = 0;
        while iterator.valid() {
            if let (Some(key), Some(value)) = (iterator.key(), iterator.value()) {
                total = total
                    .saturating_add(u64::try_from(key.len() + value.len()).unwrap_or(u64::MAX));
            }
            iterator.next();
        }
        iterator.status().map_err(|_| {
            StorageError::new(
                StorageErrorKind::Unavailable,
                "stored_bytes",
                true,
                "stored size could not be read",
            )
        })?;
        Ok(total)
    }

    async fn count_keys(&self, range: KeyRange) -> StorageResult<u64> {
        let mut options = ReadOptions::default();
        options.set_iterate_lower_bound(range.start_inclusive.clone());
        options.set_iterate_upper_bound(range.end_exclusive.clone());
        let mut iterator = self.db.raw_iterator_opt(options);
        iterator.seek(&range.start_inclusive);
        let mut total: u64 = 0;
        while iterator.valid() {
            total = total.saturating_add(1);
            iterator.next();
        }
        iterator.status().map_err(|_| {
            StorageError::new(
                StorageErrorKind::Unavailable,
                "count_keys",
                true,
                "stored count could not be read",
            )
        })?;
        Ok(total)
    }

    async fn health(&self) -> StorageResult<HealthReport> {
        let started = Instant::now();
        self.db
            .get(b"\x01\x10mako-health\x00\x00")
            .map_err(|error| self.map_error("health", error))?;
        Ok(HealthReport {
            status: HealthStatus::Healthy,
            latency: started.elapsed(),
            durability_verified: true,
            safe_diagnostic: None,
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
                Capability::DurableRestart,
            ],
            self.config.maximum_batch_operations,
            self.config.maximum_scan_items,
            Durability::Sync,
        )
    }
}

enum SnapshotCommand {
    Get {
        key: Vec<u8>,
        reply: oneshot::Sender<StorageResult<Option<Vec<u8>>>>,
    },
    Scan {
        request: ScanRequest,
        reply: oneshot::Sender<StorageResult<Vec<KeyValue>>>,
    },
    Close,
}

struct RocksSnapshot {
    id: SnapshotId,
    commands: mpsc::Sender<SnapshotCommand>,
    maximum_scan_items: NonZeroUsize,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for RocksSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RocksSnapshot")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl KvSnapshot for RocksSnapshot {
    fn id(&self) -> SnapshotId {
        self.id
    }

    async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        if key.is_empty() {
            return Err(StorageError::invalid("snapshot_get", "key cannot be empty"));
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(SnapshotCommand::Get {
                key: key.to_vec(),
                reply,
            })
            .map_err(|_| snapshot_worker_error("snapshot_get"))?;
        response
            .await
            .map_err(|_| snapshot_worker_error("snapshot_get"))?
    }

    async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
        if request.limit.get() > self.maximum_scan_items.get() {
            return Err(StorageError::invalid(
                "snapshot_scan",
                "scan limit exceeds adapter limit",
            ));
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(SnapshotCommand::Scan { request, reply })
            .map_err(|_| snapshot_worker_error("snapshot_scan"))?;
        response
            .await
            .map_err(|_| snapshot_worker_error("snapshot_scan"))?
    }
}

impl Drop for RocksSnapshot {
    fn drop(&mut self) {
        let _ignored = self.commands.send(SnapshotCommand::Close);
        if let Ok(worker) = self.worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ignored = worker.join();
        }
    }
}

fn serve_snapshot(
    db: Arc<OptimisticTransactionDB>,
    receiver: mpsc::Receiver<SnapshotCommand>,
    ready: mpsc::SyncSender<()>,
    error_counters: Arc<RocksErrorCounters>,
) {
    let snapshot = db.snapshot();
    if ready.send(()).is_err() {
        return;
    }
    while let Ok(command) = receiver.recv() {
        match command {
            SnapshotCommand::Get { key, reply } => {
                let result = snapshot.get(key).map_err(|error| {
                    map_rocks_error("snapshot_get", error, Some(error_counters.as_ref()))
                });
                let _ignored = reply.send(result);
            }
            SnapshotCommand::Scan { request, reply } => {
                let result = scan_snapshot(&snapshot, &request, error_counters.as_ref());
                let _ignored = reply.send(result);
            }
            SnapshotCommand::Close => return,
        }
    }
}

fn scan_db(
    db: &OptimisticTransactionDB,
    request: &ScanRequest,
    error_counters: &RocksErrorCounters,
) -> StorageResult<Vec<KeyValue>> {
    let options = bounded_read_options(request);
    let iterator = db.raw_iterator_opt(options);
    collect_iterator(iterator, request, error_counters)
}

fn scan_snapshot(
    snapshot: &SnapshotWithThreadMode<'_, OptimisticTransactionDB>,
    request: &ScanRequest,
    error_counters: &RocksErrorCounters,
) -> StorageResult<Vec<KeyValue>> {
    let options = bounded_read_options(request);
    let iterator = snapshot.raw_iterator_opt(options);
    collect_iterator(iterator, request, error_counters)
}

fn bounded_read_options(request: &ScanRequest) -> ReadOptions {
    let mut options = ReadOptions::default();
    options.set_iterate_lower_bound(request.range.start_inclusive.clone());
    options.set_iterate_upper_bound(request.range.end_exclusive.clone());
    options
}

fn collect_iterator<D: DBAccess>(
    mut iterator: DBRawIteratorWithThreadMode<'_, D>,
    request: &ScanRequest,
    error_counters: &RocksErrorCounters,
) -> StorageResult<Vec<KeyValue>> {
    match request.direction {
        ScanDirection::Forward => iterator.seek(&request.range.start_inclusive),
        ScanDirection::Reverse => iterator.seek_for_prev(&request.range.end_exclusive),
    }
    let mut entries = Vec::with_capacity(request.limit.get());
    while iterator.valid() && entries.len() < request.limit.get() {
        let (key, value) = iterator.item().ok_or_else(|| {
            StorageError::new(
                StorageErrorKind::Internal,
                "scan",
                false,
                "RocksDB iterator was valid without an item",
            )
        })?;
        if !request.range.contains(key) {
            match request.direction {
                ScanDirection::Forward => iterator.next(),
                ScanDirection::Reverse => iterator.prev(),
            }
            continue;
        }
        entries.push(KeyValue {
            key: key.to_vec(),
            value: value.to_vec(),
        });
        match request.direction {
            ScanDirection::Forward => iterator.next(),
            ScanDirection::Reverse => iterator.prev(),
        }
    }
    iterator
        .status()
        .map_err(|error| map_rocks_error("scan", error, Some(error_counters)))?;
    Ok(entries)
}

fn build_rocks_batch(batch: &WriteBatch) -> WriteBatchWithTransaction<true> {
    let mut rocks_batch = WriteBatchWithTransaction::<true>::with_capacity_bytes(batch.len() * 64);
    for operation in batch.operations() {
        match operation {
            WriteOperation::Put { key, value } => rocks_batch.put(key, value),
            WriteOperation::Delete { key } => rocks_batch.delete(key),
        }
    }
    rocks_batch
}

fn write_options(durability: Durability) -> WriteOptions {
    let mut options = WriteOptions::default();
    match durability {
        Durability::Memory => {
            options.disable_wal(true);
            options.set_sync(false);
        }
        Durability::Wal => {
            options.disable_wal(false);
            options.set_sync(false);
        }
        Durability::Sync => {
            options.disable_wal(false);
            options.set_sync(true);
        }
    }
    options
}

fn duration_millis(duration: Duration, field: &'static str) -> StorageResult<i64> {
    i64::try_from(duration.as_millis())
        .ok()
        .filter(|millis| *millis > 0)
        .ok_or_else(|| StorageError::invalid("rocksdb_config", format!("{field} is out of range")))
}

fn snapshot_worker_error(operation: &'static str) -> StorageError {
    StorageError::new(
        StorageErrorKind::Unavailable,
        operation,
        true,
        "snapshot worker is unavailable",
    )
}

fn map_rocks_error(
    operation: &'static str,
    error: RocksError,
    counters: Option<&RocksErrorCounters>,
) -> StorageError {
    let (kind, retryable, message) = match error.kind() {
        RocksErrorKind::Corruption => (
            StorageErrorKind::Corruption,
            false,
            "RocksDB reported corruption",
        ),
        RocksErrorKind::NotSupported | RocksErrorKind::InvalidArgument => (
            StorageErrorKind::Unsupported,
            false,
            "RocksDB rejected the requested operation",
        ),
        RocksErrorKind::TimedOut => (
            StorageErrorKind::Timeout,
            true,
            "RocksDB operation timed out",
        ),
        RocksErrorKind::Busy
        | RocksErrorKind::TryAgain
        | RocksErrorKind::Expired
        | RocksErrorKind::Aborted => (
            StorageErrorKind::Conflict,
            true,
            "RocksDB transaction could not commit",
        ),
        RocksErrorKind::IOError
        | RocksErrorKind::Incomplete
        | RocksErrorKind::ShutdownInProgress => {
            (StorageErrorKind::Io, true, "RocksDB I/O is unavailable")
        }
        RocksErrorKind::NotFound => (
            StorageErrorKind::InvalidRequest,
            false,
            "RocksDB resource was not found",
        ),
        RocksErrorKind::MergeInProgress
        | RocksErrorKind::CompactionTooLarge
        | RocksErrorKind::ColumnFamilyDropped
        | RocksErrorKind::Unknown => (
            StorageErrorKind::Internal,
            false,
            "RocksDB reported an internal failure",
        ),
    };
    if let Some(counters) = counters {
        match kind {
            StorageErrorKind::Io => {
                counters.io.fetch_add(1, Ordering::Relaxed);
            }
            StorageErrorKind::Corruption => {
                counters.corruption.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
    StorageError::new(kind, operation, retryable, message)
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use tempfile::TempDir;

    use super::*;
    use crate::KeyRange;

    fn open(path: &Path) -> RocksDbAdapter {
        RocksDbAdapter::open(RocksDbConfig::new(path)).expect("open RocksDB")
    }

    fn put(key: &[u8], value: &[u8]) -> WriteBatch {
        let mut batch = WriteBatch::new();
        batch.put(key, value);
        batch
    }

    #[test]
    fn concurrent_conditional_writes_return_one_winner_and_logical_conflicts() {
        let directory = TempDir::new().expect("temporary directory");
        let adapter = open(&directory.path().join("rocksdb"));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let workers: Vec<_> = (0..16)
            .map(|worker| {
                let adapter = adapter.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut batch = WriteBatch::new();
                    let conditions = (0..128)
                        .map(|index| {
                            let key = format!("contended-{index:03}").into_bytes();
                            batch.put(&key, worker.to_string().as_bytes());
                            KeyCondition::Missing { key }
                        })
                        .collect();
                    barrier.wait();
                    block_on(adapter.compare_and_write(AtomicWrite {
                        conditions,
                        batch,
                        durability: Durability::Sync,
                    }))
                })
            })
            .collect();
        let outcomes: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker"))
            .collect();
        assert!(
            outcomes.iter().all(Result::is_ok),
            "contention must yield logical conflicts: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|result| matches!(result, Ok(CompareAndWriteResult::Applied)))
                .count(),
            1
        );
        let winner = block_on(adapter.get(b"contended-000"))
            .expect("read")
            .expect("winner");
        for index in 0..128 {
            assert_eq!(
                block_on(adapter.get(format!("contended-{index:03}").as_bytes())).expect("read"),
                Some(winner.clone())
            );
        }
    }

    #[test]
    fn sync_acknowledged_write_survives_reopen() {
        block_on(async {
            let directory = TempDir::new().expect("temporary directory");
            let path = directory.path().join("rocksdb");
            let adapter = open(&path);
            adapter
                .write(put(b"durable", b"value"), Durability::Sync)
                .await
                .expect("durable write");
            drop(adapter);

            let reopened = open(&path);
            assert_eq!(
                reopened.get(b"durable").await.expect("read after reopen"),
                Some(b"value".to_vec())
            );
        });
    }

    #[test]
    fn native_snapshot_remains_stable() {
        block_on(async {
            let directory = TempDir::new().expect("temporary directory");
            let adapter = open(&directory.path().join("rocksdb"));
            adapter
                .write(put(b"key", b"old"), Durability::Sync)
                .await
                .expect("seed");
            let snapshot = adapter.snapshot().await.expect("snapshot");
            adapter
                .write(put(b"key", b"new"), Durability::Sync)
                .await
                .expect("update");

            assert_eq!(
                snapshot.get(b"key").await.expect("snapshot get"),
                Some(b"old".to_vec())
            );
            assert_eq!(
                adapter.get(b"key").await.expect("current get"),
                Some(b"new".to_vec())
            );
        });
    }

    #[test]
    fn bounded_iterators_honor_order_and_limit() {
        block_on(async {
            let directory = TempDir::new().expect("temporary directory");
            let adapter = open(&directory.path().join("rocksdb"));
            let mut batch = WriteBatch::new();
            for key in [b"a", b"b", b"c", b"d"] {
                batch.put(key, key);
            }
            adapter.write(batch, Durability::Sync).await.expect("seed");
            let request = ScanRequest::new(
                KeyRange::new(b"b".to_vec(), b"e".to_vec()).expect("range"),
                ScanDirection::Reverse,
                NonZeroUsize::new(2).expect("non-zero"),
            );
            let entries = adapter.scan(request).await.expect("scan");

            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.key.as_slice())
                    .collect::<Vec<_>>(),
                [b"d", b"c"]
            );
        });
    }

    #[test]
    fn conditional_transaction_commits_all_or_conflicts() {
        block_on(async {
            let directory = TempDir::new().expect("temporary directory");
            let adapter = open(&directory.path().join("rocksdb"));
            adapter
                .write(put(b"revision", b"one"), Durability::Sync)
                .await
                .expect("seed");
            let mut winner_batch = WriteBatch::new();
            winner_batch.put(b"revision", b"two");
            winner_batch.put(b"derived", b"present");
            let winner = adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::ValueEquals {
                        key: b"revision".to_vec(),
                        value: b"one".to_vec(),
                    }],
                    batch: winner_batch,
                    durability: Durability::Sync,
                })
                .await
                .expect("winner");
            let mut loser_batch = WriteBatch::new();
            loser_batch.put(b"revision", b"three");
            loser_batch.delete(b"derived");
            let loser = adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::ValueEquals {
                        key: b"revision".to_vec(),
                        value: b"one".to_vec(),
                    }],
                    batch: loser_batch,
                    durability: Durability::Sync,
                })
                .await
                .expect("loser result");

            assert_eq!(winner, CompareAndWriteResult::Applied);
            assert!(matches!(loser, CompareAndWriteResult::Conflict { .. }));
            assert_eq!(
                adapter.get(b"revision").await.expect("revision"),
                Some(b"two".to_vec())
            );
            assert_eq!(
                adapter.get(b"derived").await.expect("derived"),
                Some(b"present".to_vec())
            );
        });
    }
}
