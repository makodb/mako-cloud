use std::{collections::BTreeSet, num::NonZeroUsize, time::Duration};

use async_trait::async_trait;

use crate::{StorageError, StorageResult};

/// A half-open lexicographic byte-key range. Unbounded scans are intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRange {
    pub start_inclusive: Vec<u8>,
    pub end_exclusive: Vec<u8>,
}

impl KeyRange {
    pub fn new(
        start_inclusive: impl Into<Vec<u8>>,
        end_exclusive: impl Into<Vec<u8>>,
    ) -> StorageResult<Self> {
        let range = Self {
            start_inclusive: start_inclusive.into(),
            end_exclusive: end_exclusive.into(),
        };
        if range.start_inclusive >= range.end_exclusive {
            return Err(StorageError::invalid(
                "range",
                "start key must sort before the exclusive end key",
            ));
        }
        Ok(range)
    }

    #[must_use]
    pub fn contains(&self, key: &[u8]) -> bool {
        self.start_inclusive.as_slice() <= key && key < self.end_exclusive.as_slice()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScanDirection {
    #[default]
    Forward,
    Reverse,
}

/// A bounded scan request. Callers must opt into a finite result limit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanRequest {
    pub range: KeyRange,
    pub direction: ScanDirection,
    pub limit: NonZeroUsize,
}

impl ScanRequest {
    #[must_use]
    pub const fn new(range: KeyRange, direction: ScanDirection, limit: NonZeroUsize) -> Self {
        Self {
            range,
            direction,
            limit,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyValue {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteOperation {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

impl WriteOperation {
    #[must_use]
    pub fn key(&self) -> &[u8] {
        match self {
            Self::Put { key, .. } | Self::Delete { key } => key,
        }
    }
}

/// An ordered group committed entirely or not at all.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WriteBatch {
    operations: Vec<WriteOperation>,
}

impl WriteBatch {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            operations: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            operations: Vec::with_capacity(capacity),
        }
    }

    pub fn put(&mut self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> &mut Self {
        self.operations.push(WriteOperation::Put {
            key: key.as_ref().to_vec(),
            value: value.as_ref().to_vec(),
        });
        self
    }

    pub fn delete(&mut self, key: impl AsRef<[u8]>) -> &mut Self {
        self.operations.push(WriteOperation::Delete {
            key: key.as_ref().to_vec(),
        });
        self
    }

    #[must_use]
    pub fn operations(&self) -> &[WriteOperation] {
        &self.operations
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.operations.len()
    }
}

/// A condition evaluated atomically with the associated batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyCondition {
    Missing { key: Vec<u8> },
    Present { key: Vec<u8> },
    ValueEquals { key: Vec<u8>, value: Vec<u8> },
}

/// One value that must be absent and is inserted in the same atomic decision
/// as an owning domain mutation. This is used for durable side records whose
/// existence must never get ahead of, or lag behind, that mutation.
#[derive(Clone, Eq, PartialEq)]
pub struct ConditionalPut {
    key: Vec<u8>,
    value: Vec<u8>,
}

impl ConditionalPut {
    #[must_use]
    pub fn new(key: Vec<u8>, value: Vec<u8>) -> Self {
        Self { key, value }
    }

    pub fn append_to(self, conditions: &mut Vec<KeyCondition>, batch: &mut WriteBatch) {
        conditions.push(KeyCondition::Missing {
            key: self.key.clone(),
        });
        batch.put(self.key, self.value);
    }
}

impl std::fmt::Debug for ConditionalPut {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConditionalPut")
            .field("key", &self.key)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

impl KeyCondition {
    #[must_use]
    pub fn key(&self) -> &[u8] {
        match self {
            Self::Missing { key } | Self::Present { key } | Self::ValueEquals { key, .. } => key,
        }
    }
}

/// Conditions and writes that share one atomic commit decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AtomicWrite {
    pub conditions: Vec<KeyCondition>,
    pub batch: WriteBatch,
    pub durability: Durability,
}

impl AtomicWrite {
    #[must_use]
    pub fn unconditional(batch: WriteBatch, durability: Durability) -> Self {
        Self {
            conditions: Vec::new(),
            batch,
            durability,
        }
    }

    pub fn validate(&self) -> StorageResult<()> {
        if self.batch.is_empty() {
            return Err(StorageError::invalid(
                "atomic_write",
                "write batch cannot be empty",
            ));
        }
        if self
            .conditions
            .iter()
            .any(|condition| condition.key().is_empty())
            || self
                .batch
                .operations()
                .iter()
                .any(|operation| operation.key().is_empty())
        {
            return Err(StorageError::invalid(
                "atomic_write",
                "keys cannot be empty",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompareAndWriteResult {
    Applied,
    Conflict {
        failed_condition: usize,
        actual_value: Option<Vec<u8>>,
    },
}

/// Requested acknowledgement strength.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Durability {
    Memory,
    Wal,
    #[default]
    Sync,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TransactionMode {
    CompareAndWrite,
    Serializable,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    PointReads,
    OrderedBoundedScans,
    StableSnapshots,
    AtomicBatches,
    AtomicConditionalWrites,
    SerializableTransactions,
    DurableRestart,
}

/// Semantic claims made by an adapter. Claims are verified by conformance tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterCapabilities {
    supported: BTreeSet<Capability>,
    pub maximum_batch_operations: NonZeroUsize,
    pub maximum_scan_items: NonZeroUsize,
    pub strongest_durability: Durability,
}

impl AdapterCapabilities {
    #[must_use]
    pub fn new(
        supported: impl IntoIterator<Item = Capability>,
        maximum_batch_operations: NonZeroUsize,
        maximum_scan_items: NonZeroUsize,
        strongest_durability: Durability,
    ) -> Self {
        Self {
            supported: supported.into_iter().collect(),
            maximum_batch_operations,
            maximum_scan_items,
            strongest_durability,
        }
    }

    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.supported.contains(&capability)
    }

    #[must_use]
    pub fn supported(&self) -> &BTreeSet<Capability> {
        &self.supported
    }

    #[must_use]
    pub fn missing_document_engine_requirements(&self) -> Vec<Capability> {
        const REQUIRED: [Capability; 6] = [
            Capability::PointReads,
            Capability::OrderedBoundedScans,
            Capability::StableSnapshots,
            Capability::AtomicBatches,
            Capability::AtomicConditionalWrites,
            Capability::DurableRestart,
        ];
        REQUIRED
            .into_iter()
            .filter(|capability| !self.supports(*capability))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SnapshotId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthReport {
    pub status: HealthStatus,
    pub latency: Duration,
    pub durability_verified: bool,
    pub safe_diagnostic: Option<String>,
}

/// A stable read view that cannot observe writes committed after its creation.
#[async_trait]
pub trait KvSnapshot: Send + Sync {
    fn id(&self) -> SnapshotId;

    async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>>;

    async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>>;
}

/// Optional serializable transaction support. Compare-and-write remains mandatory.
#[async_trait]
pub trait KvTransaction: Send {
    async fn get_for_update(&mut self, key: &[u8]) -> StorageResult<Option<Vec<u8>>>;

    async fn scan_for_update(&mut self, request: ScanRequest) -> StorageResult<Vec<KeyValue>>;

    fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> StorageResult<()>;

    fn delete(&mut self, key: Vec<u8>) -> StorageResult<()>;

    async fn commit(self: Box<Self>, durability: Durability) -> StorageResult<()>;

    async fn rollback(self: Box<Self>) -> StorageResult<()>;
}

/// Vendor-neutral semantic ordered transactional key-value boundary.
#[async_trait]
pub trait KvAdapter: Send + Sync {
    async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>>;

    async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>>;

    async fn snapshot(&self) -> StorageResult<Box<dyn KvSnapshot>>;

    async fn write(&self, batch: WriteBatch, durability: Durability) -> StorageResult<()>;

    async fn compare_and_write(&self, request: AtomicWrite)
    -> StorageResult<CompareAndWriteResult>;

    async fn begin_transaction(
        &self,
        mode: TransactionMode,
    ) -> StorageResult<Box<dyn KvTransaction>>;

    /// Stored bytes held in a half-open key range.
    ///
    /// Sums the stored length of every entry in the range, so it walks it. That
    /// is the reason this is a sampled measurement rather than one taken per
    /// request, and the reason the range should be one tenant rather than a
    /// whole database.
    ///
    /// RocksDB's own size estimate was tried first and is not usable for this:
    /// it reads table statistics that exclude the memtable, so it reported zero
    /// for thirty-two kilobytes of freshly written data. Every engine sums, so
    /// every engine agrees -- which matters when the number is what a tenant is
    /// billed on.
    async fn stored_bytes(&self, range: KeyRange) -> StorageResult<u64>;

    /// How many entries a half-open key range holds.
    ///
    /// Walks the range for the same reason `stored_bytes` does, and carries the
    /// same expectation: ask it about one tenant on a schedule, not on a
    /// request path.
    async fn count_keys(&self, range: KeyRange) -> StorageResult<u64>;

    async fn health(&self) -> StorageResult<HealthReport>;

    fn capabilities(&self) -> AdapterCapabilities;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_half_open_and_bounded() {
        let range = KeyRange::new(b"a".to_vec(), b"c".to_vec()).expect("valid range");
        assert!(range.contains(b"a"));
        assert!(range.contains(b"b"));
        assert!(!range.contains(b"c"));
        assert!(KeyRange::new(b"x".to_vec(), b"x".to_vec()).is_err());
    }

    #[test]
    fn required_capabilities_are_explicit() {
        let capabilities = AdapterCapabilities::new(
            [Capability::PointReads, Capability::AtomicBatches],
            NonZeroUsize::new(100).expect("non-zero"),
            NonZeroUsize::new(100).expect("non-zero"),
            Durability::Wal,
        );
        let missing = capabilities.missing_document_engine_requirements();
        assert!(missing.contains(&Capability::StableSnapshots));
        assert!(missing.contains(&Capability::AtomicConditionalWrites));
        assert!(missing.contains(&Capability::DurableRestart));
    }

    #[test]
    fn empty_atomic_writes_and_keys_are_rejected() {
        let empty = AtomicWrite::unconditional(WriteBatch::new(), Durability::Sync);
        assert!(empty.validate().is_err());

        let mut batch = WriteBatch::new();
        batch.put(Vec::new(), b"value");
        let invalid = AtomicWrite::unconditional(batch, Durability::Sync);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn adapter_trait_is_object_safe() {
        fn accepts_adapter(_: &dyn KvAdapter) {}
        let _ = accepts_adapter;
    }
}
