//! Derived logical byte totals. The adapter operation lock fences cursor changes
//! against mutations; ordinary mutations retain shared-lock concurrency.
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use super::*;
use crate::TenantKeyspace;
use crate::key_codec::decode_tenant_span;

const REGISTRY: &[u8] = b"storage-accounting-v1/registry";
const COUNTERS: &[u8] = b"storage-accounting-v1/counters";
const FENCE: &[u8] = b"storage-accounting-v1/fence";
const SHARDS: u8 = 64;

pub(super) fn shard_for(key: &[u8]) -> usize {
    usize::from(blake3::hash(key).as_bytes()[0] % SHARDS)
}
const MAX_RANGES: usize = 4096;
const STEP_INTERVAL: Duration = Duration::from_secs(2);
const STEP_BYTES: u64 = 2 * 1024 * 1024;
const STEP_ENTRIES: usize = 8192;
const STEP_TIME: Duration = Duration::from_millis(20);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RangeAccounting {
    start: Vec<u8>,
    end: Vec<u8>,
    cursor: Option<Vec<u8>>,
    base: u64,
    ready: bool,
}

impl RangeAccounting {
    fn new(range: &KeyRange) -> Self {
        Self {
            start: range.start_inclusive.clone(),
            end: range.end_exclusive.clone(),
            cursor: None,
            base: 0,
            ready: false,
        }
    }

    fn registry_key(&self) -> Vec<u8> {
        system_key(REGISTRY, blake3::hash(&self.start).as_bytes())
    }

    fn counter_key(&self, shard: u8) -> Vec<u8> {
        let mut id = blake3::hash(&self.start).as_bytes().to_vec();
        id.push(shard);
        system_key(COUNTERS, &id)
    }

    pub(super) fn counter_for(&self, key: &[u8]) -> Option<Vec<u8>> {
        if key < self.start.as_slice() || key >= self.end.as_slice() {
            return None;
        }
        if !self.ready
            && !self
                .cursor
                .as_ref()
                .is_some_and(|cursor| key <= cursor.as_slice())
        {
            return None;
        }
        Some(self.counter_key(shard_for(key) as u8))
    }
}

pub(super) struct Accounting {
    db: Arc<OptimisticTransactionDB>,
    pub(super) ranges: RwLock<BTreeMap<Vec<u8>, RangeAccounting>>,
    last_step: Mutex<Option<Instant>>,
    pub(super) write_shards: [Mutex<()>; SHARDS as usize],
    active: AtomicBool,
}

impl Accounting {
    pub(super) fn open(db: Arc<OptimisticTransactionDB>) -> StorageResult<Self> {
        let range = TenantKeyspace::system_domain_range(REGISTRY).map_err(|_| invalid())?;
        let mut options = ReadOptions::default();
        options.fill_cache(false);
        options.set_iterate_lower_bound(range.start_inclusive.clone());
        options.set_iterate_upper_bound(range.end_exclusive.clone());
        let mut iterator = db.raw_iterator_opt(options);
        iterator.seek(&range.start_inclusive);
        let mut ranges = BTreeMap::new();
        while iterator.valid() {
            if ranges.len() >= MAX_RANGES {
                return Err(invalid());
            }
            let (key, value) = iterator.item().ok_or_else(invalid)?;
            let record: RangeAccounting = serde_json::from_slice(value).map_err(|_| invalid())?;
            canonical_range(&KeyRange::new(record.start.clone(), record.end.clone())?)?;
            if key != record.registry_key()
                || record
                    .cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor < &record.start || cursor >= &record.end)
            {
                return Err(invalid());
            }
            ranges.insert(record.start.clone(), record);
            iterator.next();
        }
        iterator
            .status()
            .map_err(|e| map_rocks_error("accounting_open", e, None))?;
        drop(iterator);
        let active = !ranges.is_empty();
        if active {
            let fence = db
                .get(system_key(FENCE, b"clean"))
                .map_err(|e| map_rocks_error("accounting_fence", e, None))?;
            let trusted = fence
                .as_deref()
                .and_then(|v| <[u8; 8]>::try_from(v).ok())
                .map(u64::from_be_bytes)
                == Some(db.latest_sequence_number());
            // Delete first: an interrupted reset must never leave a trusted fence.
            db.delete_opt(
                system_key(FENCE, b"clean"),
                &write_options(Durability::Sync),
            )
            .map_err(|e| map_rocks_error("accounting_fence", e, None))?;
            if !trusted {
                eprintln!(
                    "storage_accounting: reconciling derived totals after an untrusted close"
                );
                // Only derived metadata is reset. A crash or an old binary may
                // have bypassed accounting; do not trust totals in that case.
                for record in ranges.values_mut() {
                    record.cursor = None;
                    record.base = 0;
                    record.ready = false;
                    let mut batch = WriteBatchWithTransaction::<true>::default();
                    for shard in 0..SHARDS {
                        batch.delete(record.counter_key(shard));
                    }
                    batch.put(record.registry_key(), encode(record)?);
                    db.write_opt(batch, &write_options(Durability::Sync))
                        .map_err(|e| map_rocks_error("accounting_reset", e, None))?;
                }
            }
        }
        Ok(Self {
            db,
            ranges: RwLock::new(ranges),
            last_step: Mutex::new(None),
            write_shards: std::array::from_fn(|_| Mutex::new(())),
            active: AtomicBool::new(active),
        })
    }

    /// Called with the adapter operation lock exclusively held. One globally
    /// budgeted page per interval; no snapshot pins old SST files during bootstrap.
    pub(super) fn sample(&self, range: &KeyRange) -> StorageResult<Option<u64>> {
        canonical_range(range)?;
        let mut ranges = self.ranges.write().map_err(|_| invalid())?;
        if !ranges.contains_key(&range.start_inclusive) {
            if ranges.len() >= MAX_RANGES {
                return Err(StorageError::invalid(
                    "accounting",
                    "too many tracked tenants",
                ));
            }
            let record = RangeAccounting::new(range);
            // No old clean fence may survive an active accounting session.
            let mut batch = WriteBatchWithTransaction::<true>::default();
            batch.delete(system_key(FENCE, b"clean"));
            batch.put(record.registry_key(), encode(&record)?);
            self.db
                .write_opt(batch, &write_options(Durability::Sync))
                .map_err(|e| map_rocks_error("accounting_register", e, None))?;
            eprintln!("storage_accounting: registered bounded tenant reconciliation");
            self.active.store(true, Ordering::Release);
            ranges.insert(record.start.clone(), record);
        }
        let record = ranges.get_mut(&range.start_inclusive).ok_or_else(invalid)?;
        if !record.ready {
            let mut last = self.last_step.lock().map_err(|_| invalid())?;
            if last.is_some_and(|at| at.elapsed() < STEP_INTERVAL) {
                return Ok(None);
            }
            // Reserve the interval even on failure, so repeated I/O errors do
            // not create an unbounded retry loop.
            *last = Some(Instant::now());
            let next = self.advance(record, STEP_ENTRIES, STEP_BYTES, STEP_TIME)?;
            if next.ready {
                eprintln!(
                    "storage_accounting: tenant reconciliation complete; incremental totals active"
                );
            }
            *record = next;
        }
        if !record.ready {
            return Ok(None);
        }
        // The exclusive operation lock makes the 64 point reads coherent with
        // mutations, without a scan or a long-lived snapshot.
        let mut total = i128::from(record.base);
        for shard in 0..SHARDS {
            let value = self
                .db
                .get(record.counter_key(shard))
                .map_err(|e| map_rocks_error("accounting_total", e, None))?;
            total = total
                .checked_add(decode_counter(value.as_deref())?)
                .ok_or_else(invalid)?;
        }
        u64::try_from(total).map(Some).map_err(|_| invalid())
    }

    fn advance(
        &self,
        record: &RangeAccounting,
        entries: usize,
        bytes: u64,
        time: Duration,
    ) -> StorageResult<RangeAccounting> {
        let started = Instant::now();
        let mut next = record.clone();
        let mut options = ReadOptions::default();
        options.fill_cache(false);
        options.set_auto_readahead_size(false);
        options.set_readahead_size(0);
        options.set_iterate_lower_bound(record.start.clone());
        options.set_iterate_upper_bound(record.end.clone());
        let mut iterator = self.db.raw_iterator_opt(options);
        if let Some(cursor) = &record.cursor {
            let mut after = cursor.clone();
            after.push(0);
            iterator.seek(&after);
        } else {
            iterator.seek(&record.start);
        }
        let mut measured = 0_u64;
        let mut count = 0;
        while iterator.valid() {
            let (key, value) = iterator.item().ok_or_else(invalid)?;
            measured = measured
                .checked_add((key.len() + value.len()) as u64)
                .ok_or_else(invalid)?;
            next.cursor = Some(key.to_vec());
            count += 1;
            if count >= entries || measured >= bytes || started.elapsed() >= time {
                break;
            }
            iterator.next();
        }
        next.ready = !iterator.valid();
        iterator
            .status()
            .map_err(|e| map_rocks_error("accounting_reconcile", e, None))?;
        next.base = next.base.checked_add(measured).ok_or_else(invalid)?;
        self.db
            .put_opt(
                next.registry_key(),
                encode(&next)?,
                &write_options(Durability::Sync),
            )
            .map_err(|e| map_rocks_error("accounting_checkpoint", e, None))?;
        Ok(next)
    }
}

impl Drop for Accounting {
    fn drop(&mut self) {
        if self.active.load(Ordering::Acquire) && self.db.flush().is_ok() {
            // This Arc is shared by all adapter clones. No adapter mutation can
            // remain when its last owner drops. Flush also covers callers that used
            // Memory durability; a persisted fence must not outrun their data.
            // A one-key put consumes one seqno.
            if let Some(sequence) = self.db.latest_sequence_number().checked_add(1) {
                let _result = self.db.put_opt(
                    system_key(FENCE, b"clean"),
                    sequence.to_be_bytes(),
                    &write_options(Durability::Sync),
                );
            }
        }
    }
}

pub(super) fn decode_counter(value: Option<&[u8]>) -> StorageResult<i128> {
    value.map_or(Ok(0), |v| {
        <[u8; 16]>::try_from(v)
            .map(i128::from_be_bytes)
            .map_err(|_| invalid())
    })
}

fn canonical_range(range: &KeyRange) -> StorageResult<()> {
    let Some(((project, environment), _)) =
        decode_tenant_span(&range.start_inclusive).map_err(|_| invalid())?
    else {
        return Err(StorageError::invalid(
            "accounting",
            "expected a complete tenant range",
        ));
    };
    let canonical = TenantKeyspace::new(project, environment)
        .and_then(|t| t.tenant_range())
        .map_err(|_| invalid())?;
    if canonical != *range {
        return Err(StorageError::invalid(
            "accounting",
            "expected a complete tenant range",
        ));
    }
    Ok(())
}

fn system_key(domain: &[u8], id: &[u8]) -> Vec<u8> {
    TenantKeyspace::system_key(domain, id).expect("bounded accounting identifiers")
}
fn encode(record: &RangeAccounting) -> StorageResult<Vec<u8>> {
    serde_json::to_vec(record).map_err(|_| invalid())
}
fn invalid() -> StorageError {
    StorageError::new(
        StorageErrorKind::Corruption,
        "storage_accounting",
        false,
        "storage accounting metadata is invalid",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use tempfile::TempDir;

    fn tenant(id: &[u8]) -> KeyRange {
        TenantKeyspace::new(b"project", id)
            .unwrap()
            .tenant_range()
            .unwrap()
    }
    fn key(range: &KeyRange, suffix: &[u8]) -> Vec<u8> {
        [range.start_inclusive.as_slice(), suffix].concat()
    }
    fn put(adapter: &RocksDbAdapter, range: &KeyRange, suffix: &[u8], value: &[u8]) {
        let mut batch = WriteBatch::new();
        batch.put(key(range, suffix), value);
        block_on(adapter.write(batch, Durability::Sync)).unwrap();
    }
    fn open(path: &Path) -> RocksDbAdapter {
        RocksDbAdapter::open(RocksDbConfig::new(path)).unwrap()
    }
    fn sample(adapter: &RocksDbAdapter, range: &KeyRange) -> Option<u64> {
        block_on(adapter.sample_stored_bytes(range.clone())).unwrap()
    }
    fn finish(adapter: &RocksDbAdapter, range: &KeyRange) -> u64 {
        for _ in 0..100 {
            *adapter.accounting.last_step.lock().unwrap() = None;
            if let Some(total) = sample(adapter, range) {
                return total;
            }
        }
        panic!("bounded reconciliation did not finish");
    }
    fn exact(adapter: &RocksDbAdapter, range: &KeyRange) -> u64 {
        block_on(adapter.stored_bytes(range.clone())).unwrap()
    }
    fn partial(adapter: &RocksDbAdapter, range: &KeyRange) {
        let _exclusive = adapter.operation_lock.write().unwrap();
        let mut ranges = adapter.accounting.ranges.write().unwrap();
        let record = RangeAccounting::new(range);
        let next = adapter
            .accounting
            .advance(&record, 1, u64::MAX, Duration::from_secs(1))
            .unwrap();
        ranges.insert(range.start_inclusive.clone(), next);
        adapter.accounting.active.store(true, Ordering::Release);
    }

    #[test]
    fn reconciliation_tracks_writes_on_both_sides_of_cursor() {
        let dir = TempDir::new().unwrap();
        let adapter = open(dir.path());
        let range = tenant(b"one");
        put(&adapter, &range, b"b", b"old");
        put(&adapter, &range, b"d", b"later");
        partial(&adapter, &range);
        put(&adapter, &range, b"a", b"insert-behind");
        put(&adapter, &range, b"b", b"replace-counted");
        put(&adapter, &range, b"c", b"insert-ahead");
        let mut batch = WriteBatch::new();
        batch.delete(key(&range, b"d"));
        block_on(adapter.write(batch, Durability::Sync)).unwrap();
        assert_eq!(finish(&adapter, &range), exact(&adapter, &range));
        let mut batch = WriteBatch::new();
        batch.delete(key(&range, b"a"));
        block_on(adapter.write(batch, Durability::Sync)).unwrap();
        assert_eq!(sample(&adapter, &range), Some(exact(&adapter, &range)));
    }

    #[test]
    fn atomic_accounting_handles_failed_cas_duplicate_keys_and_tenants() {
        let dir = TempDir::new().unwrap();
        let adapter = open(dir.path());
        let a = tenant(b"one");
        let b = tenant(b"two");
        finish(&adapter, &a);
        finish(&adapter, &b);
        let mut batch = WriteBatch::new();
        batch
            .put(key(&a, b"k"), b"first")
            .put(key(&a, b"k"), b"second")
            .delete(key(&a, b"k"))
            .put(key(&a, b"k"), b"final")
            .put(key(&b, b"k"), b"other");
        block_on(adapter.write(batch, Durability::Sync)).unwrap();
        let mut batch = WriteBatch::new();
        batch.delete(key(&a, b"k"));
        let result = block_on(adapter.compare_and_write(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key: key(&a, b"k") }],
            batch,
            durability: Durability::Sync,
        }))
        .unwrap();
        assert!(matches!(result, CompareAndWriteResult::Conflict { .. }));
        assert_eq!(sample(&adapter, &a), Some(exact(&adapter, &a)));
        assert_eq!(sample(&adapter, &b), Some(exact(&adapter, &b)));
    }

    #[test]
    fn concurrent_accounting_matches_exact_committed_data() {
        let dir = TempDir::new().unwrap();
        let adapter = open(dir.path());
        let range = tenant(b"one");
        finish(&adapter, &range);
        let barrier = Arc::new(std::sync::Barrier::new(8));
        thread::scope(|scope| {
            for worker in 0..8 {
                let adapter = adapter.clone();
                let range = range.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    for i in 0..40 {
                        put(
                            &adapter,
                            &range,
                            format!("{worker}-{i}").as_bytes(),
                            b"value",
                        );
                        put(
                            &adapter,
                            &range,
                            b"shared",
                            format!("{worker}-{i}").as_bytes(),
                        );
                    }
                });
            }
        });
        assert_eq!(sample(&adapter, &range), Some(exact(&adapter, &range)));
    }

    #[test]
    fn clean_restart_resumes_and_old_writer_invalidates_derived_totals() {
        let dir = TempDir::new().unwrap();
        let range = tenant(b"one");
        {
            let adapter = open(dir.path());
            put(&adapter, &range, b"b", b"before");
            put(&adapter, &range, b"z", b"after");
            partial(&adapter, &range);
        }
        {
            let adapter = open(dir.path());
            assert!(
                adapter.accounting.ranges.read().unwrap()[&range.start_inclusive]
                    .cursor
                    .is_some()
            );
            assert_eq!(finish(&adapter, &range), exact(&adapter, &range));
        }
        {
            let adapter = open(dir.path());
            assert!(adapter.accounting.ranges.read().unwrap()[&range.start_inclusive].ready);
            assert_eq!(sample(&adapter, &range), Some(exact(&adapter, &range)));
        }
        // Simulate rollback to an older adapter that knows nothing of counters.
        {
            let db: OptimisticTransactionDB =
                OptimisticTransactionDB::open_default(dir.path()).unwrap();
            db.put(key(&range, b"new"), b"older-binary-write").unwrap();
        }
        {
            let adapter = open(dir.path());
            assert!(!adapter.accounting.ranges.read().unwrap()[&range.start_inclusive].ready);
            assert_eq!(finish(&adapter, &range), exact(&adapter, &range));
        }
    }

    #[test]
    fn missing_clean_fence_rebuilds_without_modifying_tenant_data() {
        let dir = TempDir::new().unwrap();
        let range = tenant(b"one");
        {
            let adapter = open(dir.path());
            put(&adapter, &range, b"x", b"kept");
            finish(&adapter, &range);
        }
        {
            let db: OptimisticTransactionDB =
                OptimisticTransactionDB::open_default(dir.path()).unwrap();
            db.delete(system_key(FENCE, b"clean")).unwrap();
        }
        let adapter = open(dir.path());
        assert!(!adapter.accounting.ranges.read().unwrap()[&range.start_inclusive].ready);
        assert_eq!(finish(&adapter, &range), exact(&adapter, &range));
        assert_eq!(
            block_on(adapter.get(&key(&range, b"x"))).unwrap(),
            Some(b"kept".to_vec())
        );
    }

    #[test]
    fn reconciliation_is_bounded_throttled_and_steady_samples_do_not_advance() {
        let dir = TempDir::new().unwrap();
        let adapter = open(dir.path());
        let range = tenant(b"one");
        for i in 0..4 {
            put(&adapter, &range, &[i], &vec![0; 1024 * 1024]);
        }
        assert_eq!(sample(&adapter, &range), None);
        let sequence = adapter.db.latest_sequence_number();
        assert_eq!(sample(&adapter, &range), None);
        assert_eq!(
            sequence,
            adapter.db.latest_sequence_number(),
            "cooldown must do no checkpoint writes"
        );
        assert_eq!(finish(&adapter, &range), exact(&adapter, &range));
        let sequence = adapter.db.latest_sequence_number();
        for _ in 0..10 {
            assert_eq!(sample(&adapter, &range), Some(exact(&adapter, &range)));
        }
        assert_eq!(
            sequence,
            adapter.db.latest_sequence_number(),
            "steady sampling must not rescan/checkpoint"
        );
        let _read = adapter.operation_lock.read().unwrap();
        assert_eq!(
            sample(&adapter, &range),
            None,
            "sampling yields to an active writer"
        );
    }
}
