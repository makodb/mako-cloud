use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::{
    AdapterCapabilities, AtomicWrite, Capability, CompareAndWriteResult, Durability, HealthReport,
    HealthStatus, KeyCondition, KeyRange, KeyValue, KvAdapter, KvSnapshot, KvTransaction,
    ScanDirection, ScanRequest, SnapshotId, StorageError, StorageErrorKind, StorageResult,
    TransactionMode, WriteBatch, WriteOperation,
};

pub const CONTROL_SQLITE_FORMAT_VERSION: u32 = 1;
const SQLITE_APPLICATION_ID: i32 = 0x4d41_4b4f;
const DEFAULT_MAXIMUM_BATCH_OPERATIONS: usize = 10_000;
const DEFAULT_MAXIMUM_SCAN_ITEMS: usize = 10_000;

/// Limits and identity required to open the local control-plane SQLite database.
#[derive(Clone, Debug)]
pub struct SqliteConfig {
    pub database_path: PathBuf,
    pub lock_path: PathBuf,
    pub database_identity: String,
    pub create_if_missing: bool,
    pub minimum_durability: Durability,
    pub maximum_batch_operations: NonZeroUsize,
    pub maximum_scan_items: NonZeroUsize,
    pub busy_timeout: Duration,
    pub transaction_expiration: Duration,
    pub shutdown_timeout: Duration,
    pub wal_autocheckpoint_pages: NonZeroUsize,
    pub maximum_wal_bytes: u64,
    pub disk_warning_free_bytes: u64,
    pub disk_critical_free_bytes: u64,
}

impl SqliteConfig {
    #[must_use]
    pub fn new(database_path: impl Into<PathBuf>, database_identity: impl Into<String>) -> Self {
        let database_path = database_path.into();
        let lock_path = database_path.with_extension("sqlite3.lock");
        Self {
            database_path,
            lock_path,
            database_identity: database_identity.into(),
            create_if_missing: true,
            minimum_durability: Durability::Sync,
            maximum_batch_operations: NonZeroUsize::new(DEFAULT_MAXIMUM_BATCH_OPERATIONS)
                .expect("non-zero constant"),
            maximum_scan_items: NonZeroUsize::new(DEFAULT_MAXIMUM_SCAN_ITEMS)
                .expect("non-zero constant"),
            busy_timeout: Duration::from_secs(2),
            transaction_expiration: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(10),
            wal_autocheckpoint_pages: NonZeroUsize::new(1_000).expect("non-zero constant"),
            maximum_wal_bytes: 256 * 1024 * 1024,
            disk_warning_free_bytes: 2 * 1024 * 1024 * 1024,
            disk_critical_free_bytes: 512 * 1024 * 1024,
        }
    }

    fn validate(&self) -> StorageResult<()> {
        for (field, path) in [
            ("database_path", self.database_path.as_path()),
            ("lock_path", self.lock_path.as_path()),
        ] {
            if !path.is_absolute() {
                return Err(StorageError::invalid(
                    "sqlite_config",
                    format!("{field} must be absolute"),
                ));
            }
            if path.file_name().is_none() {
                return Err(StorageError::invalid(
                    "sqlite_config",
                    format!("{field} must identify a file"),
                ));
            }
        }
        if self.database_path == self.lock_path {
            return Err(StorageError::invalid(
                "sqlite_config",
                "database and lock paths must be different",
            ));
        }
        if self.database_identity.is_empty()
            || self.database_identity.len() > 128
            || !self
                .database_identity
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(StorageError::invalid(
                "sqlite_config",
                "database identity is missing or malformed",
            ));
        }
        for (field, duration) in [
            ("busy_timeout", self.busy_timeout),
            ("transaction_expiration", self.transaction_expiration),
            ("shutdown_timeout", self.shutdown_timeout),
        ] {
            if duration.is_zero() || i64::try_from(duration.as_millis()).is_err() {
                return Err(StorageError::invalid(
                    "sqlite_config",
                    format!("{field} is out of range"),
                ));
            }
        }
        if self.maximum_wal_bytes == 0 {
            return Err(StorageError::invalid(
                "sqlite_config",
                "maximum_wal_bytes must be non-zero",
            ));
        }
        if self.disk_critical_free_bytes == 0
            || self.disk_warning_free_bytes <= self.disk_critical_free_bytes
        {
            return Err(StorageError::invalid(
                "sqlite_config",
                "disk thresholds must be non-zero and ordered",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SqliteHealthSignals {
    pub schema_version: u32,
    pub integrity_verified: bool,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub available_bytes: u64,
    pub active_operations: usize,
    pub active_transactions: usize,
    pub oldest_transaction_seconds: u64,
    pub shutting_down: bool,
    pub disk_warning_free_bytes: u64,
    pub disk_critical_free_bytes: u64,
    pub busy_failures: u64,
    pub checkpoint_failures: u64,
}

#[derive(Debug)]
struct Lifecycle {
    shutting_down: bool,
    active_operations: usize,
    active_transactions: BTreeMap<u64, Instant>,
}

struct SqliteInner {
    config: SqliteConfig,
    lock_file: Mutex<Option<File>>,
    lifecycle: Mutex<Lifecycle>,
    drained: Condvar,
    next_snapshot_id: AtomicU64,
    next_transaction_id: AtomicU64,
    busy_failures: AtomicU64,
    checkpoint_failures: AtomicU64,
}

/// Local durable adapter used exclusively for control-plane state.
#[derive(Clone)]
pub struct SqliteAdapter {
    inner: Arc<SqliteInner>,
}

impl std::fmt::Debug for SqliteAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteAdapter")
            .field("database_identity", &self.inner.config.database_identity)
            .field("minimum_durability", &self.inner.config.minimum_durability)
            .finish_non_exhaustive()
    }
}

impl SqliteAdapter {
    pub fn open(mut config: SqliteConfig) -> StorageResult<Self> {
        config.validate()?;
        config.database_path = normalize_file_path(&config.database_path, "database_path")?;
        config.lock_path = normalize_file_path(&config.lock_path, "lock_path")?;
        if config.database_path == config.lock_path {
            return Err(StorageError::invalid(
                "sqlite_open",
                "normalized database and lock paths must be different",
            ));
        }

        let existed = config.database_path.exists();
        if !existed && !config.create_if_missing {
            return Err(StorageError::new(
                StorageErrorKind::Unavailable,
                "sqlite_open",
                false,
                "control database does not exist",
            ));
        }

        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&config.lock_path)
            .map_err(|_| {
                sqlite_io_error("sqlite_lock", "control storage lock could not be opened")
            })?;
        FileExt::try_lock_exclusive(&lock_file).map_err(|_| {
            StorageError::new(
                StorageErrorKind::Conflict,
                "sqlite_lock",
                true,
                "control storage is already owned by another process",
            )
        })?;

        let adapter = Self {
            inner: Arc::new(SqliteInner {
                config,
                lock_file: Mutex::new(Some(lock_file)),
                lifecycle: Mutex::new(Lifecycle {
                    shutting_down: false,
                    active_operations: 0,
                    active_transactions: BTreeMap::new(),
                }),
                drained: Condvar::new(),
                next_snapshot_id: AtomicU64::new(1),
                next_transaction_id: AtomicU64::new(1),
                busy_failures: AtomicU64::new(0),
                checkpoint_failures: AtomicU64::new(0),
            }),
        };

        let connection = adapter.open_connection(!existed)?;
        adapter.initialize_or_verify(&connection, existed)?;
        if !existed {
            adapter.checkpoint(&connection, "sqlite_checkpoint")?;
            sync_file_and_parent(&adapter.inner.config.database_path)?;
        }
        Ok(adapter)
    }

    #[must_use]
    pub fn database_identity(&self) -> &str {
        &self.inner.config.database_identity
    }

    pub fn health_signals(&self) -> StorageResult<SqliteHealthSignals> {
        let _operation = self.begin_operation("sqlite_health")?;
        let connection = self.open_connection(false)?;
        let integrity_verified = integrity_ok(&connection)?;
        let schema_version = read_schema_version(&connection)?;
        let parent = parent_of(&self.inner.config.database_path)?;
        let available_bytes = fs2::available_space(parent).map_err(|_| {
            sqlite_io_error("sqlite_health", "control storage capacity is unavailable")
        })?;
        let database_bytes = file_size(&self.inner.config.database_path)?;
        let wal_bytes = file_size(&wal_path(&self.inner.config.database_path))?;
        let lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| lifecycle_error("sqlite_health"))?;
        Ok(SqliteHealthSignals {
            schema_version,
            integrity_verified,
            database_bytes,
            wal_bytes,
            available_bytes,
            active_operations: lifecycle.active_operations,
            active_transactions: lifecycle.active_transactions.len(),
            oldest_transaction_seconds: lifecycle
                .active_transactions
                .values()
                .map(Instant::elapsed)
                .max()
                .unwrap_or_default()
                .as_secs(),
            shutting_down: lifecycle.shutting_down,
            disk_warning_free_bytes: self.inner.config.disk_warning_free_bytes,
            disk_critical_free_bytes: self.inner.config.disk_critical_free_bytes,
            busy_failures: self.inner.busy_failures.load(Ordering::Relaxed),
            checkpoint_failures: self.inner.checkpoint_failures.load(Ordering::Relaxed),
        })
    }

    /// Stop admitting work, drain bounded operations, checkpoint, verify, and release ownership.
    pub fn shutdown(&self) -> StorageResult<()> {
        let deadline = Instant::now() + self.inner.config.shutdown_timeout;
        let mut lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| lifecycle_error("sqlite_shutdown"))?;
        lifecycle.shutting_down = true;
        while lifecycle.active_operations > 0 {
            let now = Instant::now();
            if now >= deadline {
                return Err(StorageError::new(
                    StorageErrorKind::Timeout,
                    "sqlite_shutdown",
                    true,
                    "control storage did not drain before the shutdown deadline",
                ));
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next, timeout) = self
                .inner
                .drained
                .wait_timeout(lifecycle, remaining)
                .map_err(|_| lifecycle_error("sqlite_shutdown"))?;
            lifecycle = next;
            if timeout.timed_out() && lifecycle.active_operations > 0 {
                return Err(StorageError::new(
                    StorageErrorKind::Timeout,
                    "sqlite_shutdown",
                    true,
                    "control storage did not drain before the shutdown deadline",
                ));
            }
        }
        drop(lifecycle);

        let connection = self.open_connection_for_shutdown()?;
        self.checkpoint(&connection, "sqlite_shutdown")?;
        if !integrity_ok(&connection)? {
            return Err(StorageError::new(
                StorageErrorKind::Corruption,
                "sqlite_shutdown",
                false,
                "control database integrity verification failed",
            ));
        }
        sync_file_and_parent(&self.inner.config.database_path)?;
        let mut lock = self
            .inner
            .lock_file
            .lock()
            .map_err(|_| lifecycle_error("sqlite_shutdown"))?;
        if let Some(file) = lock.take() {
            FileExt::unlock(&file).map_err(|_| {
                sqlite_io_error(
                    "sqlite_shutdown",
                    "control storage lock could not be released",
                )
            })?;
        }
        Ok(())
    }

    fn initialize_or_verify(&self, connection: &Connection, existed: bool) -> StorageResult<()> {
        let application_id: i32 = connection
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(|error| map_sqlite_error("sqlite_identity", &error))?;
        let schema_version = read_schema_version(connection)?;

        if !existed {
            connection
                .execute_batch(&format!(
                    "BEGIN IMMEDIATE;
                     PRAGMA application_id = {SQLITE_APPLICATION_ID};
                     CREATE TABLE mako_metadata (
                         key TEXT PRIMARY KEY,
                         value BLOB NOT NULL
                     ) WITHOUT ROWID;
                     CREATE TABLE mako_kv (
                         key BLOB PRIMARY KEY,
                         value BLOB NOT NULL
                     ) WITHOUT ROWID;
                     CREATE TABLE mako_migrations (
                         version INTEGER PRIMARY KEY,
                         applied_unix_ms INTEGER NOT NULL,
                         checksum TEXT NOT NULL
                     );
                     PRAGMA user_version = {CONTROL_SQLITE_FORMAT_VERSION};"
                ))
                .map_err(|error| map_sqlite_error("sqlite_schema", &error))?;
            if let Err(error) = connection.execute(
                "INSERT INTO mako_metadata(key, value) VALUES ('database_identity', ?1)",
                params![self.inner.config.database_identity.as_bytes()],
            ) {
                rollback_quietly(connection);
                return Err(map_sqlite_error("sqlite_identity", &error));
            }
            connection
                .execute_batch("COMMIT;")
                .map_err(|error| map_sqlite_error("sqlite_schema", &error))?;
            return Ok(());
        }

        if application_id != SQLITE_APPLICATION_ID {
            return Err(StorageError::new(
                StorageErrorKind::Corruption,
                "sqlite_identity",
                false,
                "control database application identity is unexpected",
            ));
        }
        if schema_version != CONTROL_SQLITE_FORMAT_VERSION {
            let message = if schema_version > CONTROL_SQLITE_FORMAT_VERSION {
                "control database schema is newer than this release"
            } else {
                "control database schema requires an explicit migration"
            };
            return Err(StorageError::new(
                StorageErrorKind::Unsupported,
                "sqlite_schema",
                false,
                message,
            ));
        }
        let actual_identity: Option<Vec<u8>> = connection
            .query_row(
                "SELECT value FROM mako_metadata WHERE key = 'database_identity'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| map_sqlite_error("sqlite_identity", &error))?;
        if actual_identity.as_deref() != Some(self.inner.config.database_identity.as_bytes()) {
            return Err(StorageError::new(
                StorageErrorKind::Corruption,
                "sqlite_identity",
                false,
                "control database identity does not match configuration",
            ));
        }
        Ok(())
    }

    fn begin_operation(&self, operation: &'static str) -> StorageResult<OperationGuard> {
        let mut lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| lifecycle_error(operation))?;
        if lifecycle.shutting_down {
            return Err(StorageError::new(
                StorageErrorKind::Unavailable,
                operation,
                true,
                "control storage is shutting down",
            ));
        }
        lifecycle.active_operations = lifecycle.active_operations.saturating_add(1);
        drop(lifecycle);
        Ok(OperationGuard {
            inner: Arc::clone(&self.inner),
        })
    }

    fn register_transaction(&self) -> StorageResult<TransactionGuard> {
        let id = self
            .inner
            .next_transaction_id
            .fetch_add(1, Ordering::Relaxed);
        let mut lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| lifecycle_error("begin_transaction"))?;
        lifecycle.active_transactions.insert(id, Instant::now());
        drop(lifecycle);
        Ok(TransactionGuard {
            id,
            inner: Arc::clone(&self.inner),
        })
    }

    fn map_error(&self, operation: &'static str, error: &rusqlite::Error) -> StorageError {
        let mapped = map_sqlite_error(operation, error);
        if mapped.kind == StorageErrorKind::Timeout {
            self.inner.busy_failures.fetch_add(1, Ordering::Relaxed);
        }
        mapped
    }

    fn checkpoint(&self, connection: &Connection, operation: &'static str) -> StorageResult<()> {
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|error| {
                self.inner
                    .checkpoint_failures
                    .fetch_add(1, Ordering::Relaxed);
                self.map_error(operation, &error)
            })
    }

    fn open_connection(&self, create: bool) -> StorageResult<Connection> {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let connection = Connection::open_with_flags(&self.inner.config.database_path, flags)
            .map_err(|error| map_sqlite_error("sqlite_open", &error))?;
        configure_connection(&connection, &self.inner.config)?;
        Ok(connection)
    }

    fn open_connection_for_shutdown(&self) -> StorageResult<Connection> {
        let connection = Connection::open_with_flags(
            &self.inner.config.database_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| map_sqlite_error("sqlite_shutdown", &error))?;
        configure_connection(&connection, &self.inner.config)?;
        Ok(connection)
    }

    fn validate_scan(&self, request: &ScanRequest) -> StorageResult<()> {
        validate_scan(request, self.inner.config.maximum_scan_items)
    }

    fn validate_batch(&self, batch: &WriteBatch) -> StorageResult<()> {
        validate_batch(batch, self.inner.config.maximum_batch_operations)
    }

    fn ensure_write_capacity(&self) -> StorageResult<()> {
        let parent = parent_of(&self.inner.config.database_path)?;
        let available = fs2::available_space(parent).map_err(|_| {
            sqlite_io_error("sqlite_capacity", "control storage capacity is unavailable")
        })?;
        if available <= self.inner.config.disk_critical_free_bytes {
            return Err(StorageError::new(
                StorageErrorKind::Unavailable,
                "sqlite_capacity",
                true,
                "control storage has reached its critical free-space reserve",
            ));
        }
        let wal = wal_path(&self.inner.config.database_path);
        if file_size(&wal)? > self.inner.config.maximum_wal_bytes {
            let connection = self.open_connection(false)?;
            self.checkpoint(&connection, "sqlite_checkpoint")?;
            if file_size(&wal)? > self.inner.config.maximum_wal_bytes {
                return Err(StorageError::new(
                    StorageErrorKind::Unavailable,
                    "sqlite_checkpoint",
                    true,
                    "control storage WAL exceeds its configured limit",
                ));
            }
        }
        Ok(())
    }
}

struct OperationGuard {
    inner: Arc<SqliteInner>,
}

struct TransactionGuard {
    id: u64,
    inner: Arc<SqliteInner>,
}

impl Drop for TransactionGuard {
    fn drop(&mut self) {
        if let Ok(mut lifecycle) = self.inner.lifecycle.lock() {
            lifecycle.active_transactions.remove(&self.id);
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if let Ok(mut lifecycle) = self.inner.lifecycle.lock() {
            lifecycle.active_operations = lifecycle.active_operations.saturating_sub(1);
            if lifecycle.active_operations == 0 {
                self.inner.drained.notify_all();
            }
        }
    }
}

#[async_trait]
impl KvAdapter for SqliteAdapter {
    async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        validate_key(key, "get")?;
        let _operation = self.begin_operation("get")?;
        let connection = self.open_connection(false)?;
        read_value(&connection, key, "get")
    }

    async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
        self.validate_scan(&request)?;
        let _operation = self.begin_operation("scan")?;
        let connection = self.open_connection(false)?;
        scan_connection(&connection, &request, "scan")
    }

    async fn snapshot(&self) -> StorageResult<Box<dyn KvSnapshot>> {
        let operation = self.begin_operation("snapshot")?;
        let connection = self.open_connection(false)?;
        connection
            .execute_batch("BEGIN DEFERRED;")
            .map_err(|error| map_sqlite_error("snapshot", &error))?;
        connection
            .query_row("SELECT key FROM mako_kv ORDER BY key LIMIT 1", [], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .optional()
            .map_err(|error| map_sqlite_error("snapshot", &error))?;
        let id = SnapshotId(self.inner.next_snapshot_id.fetch_add(1, Ordering::Relaxed));
        Ok(Box::new(SqliteSnapshot {
            id,
            connection: Mutex::new(Some(connection)),
            maximum_scan_items: self.inner.config.maximum_scan_items,
            _operation: operation,
        }))
    }

    async fn write(&self, batch: WriteBatch, durability: Durability) -> StorageResult<()> {
        self.validate_batch(&batch)?;
        let _operation = self.begin_operation("write")?;
        self.ensure_write_capacity()?;
        let connection = self.open_connection(false)?;
        configure_durability(
            &connection,
            durability.max(self.inner.config.minimum_durability),
        )?;
        execute_batch_transaction(&connection, &batch, "write", &self.inner.busy_failures)
    }

    async fn compare_and_write(
        &self,
        request: AtomicWrite,
    ) -> StorageResult<CompareAndWriteResult> {
        request.validate()?;
        self.validate_batch(&request.batch)?;
        let _operation = self.begin_operation("compare_and_write")?;
        self.ensure_write_capacity()?;
        let connection = self.open_connection(false)?;
        configure_durability(
            &connection,
            request.durability.max(self.inner.config.minimum_durability),
        )?;
        connection
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(|error| self.map_error("compare_and_write", &error))?;
        for (index, condition) in request.conditions.iter().enumerate() {
            let actual = match read_value(&connection, condition.key(), "condition_read") {
                Ok(actual) => actual,
                Err(error) => {
                    rollback_quietly(&connection);
                    return Err(error);
                }
            };
            let matches = match condition {
                KeyCondition::Missing { .. } => actual.is_none(),
                KeyCondition::Present { .. } => actual.is_some(),
                KeyCondition::ValueEquals { value, .. } => actual.as_ref() == Some(value),
            };
            if !matches {
                rollback_quietly(&connection);
                return Ok(CompareAndWriteResult::Conflict {
                    failed_condition: index,
                    actual_value: actual,
                });
            }
        }
        if let Err(error) = apply_operations(&connection, &request.batch, "compare_and_write") {
            rollback_quietly(&connection);
            return Err(error);
        }
        connection
            .execute_batch("COMMIT;")
            .map_err(|error| self.map_error("compare_and_write", &error))?;
        Ok(CompareAndWriteResult::Applied)
    }

    async fn begin_transaction(
        &self,
        _mode: TransactionMode,
    ) -> StorageResult<Box<dyn KvTransaction>> {
        let operation = self.begin_operation("begin_transaction")?;
        self.ensure_write_capacity()?;
        let connection = self.open_connection(false)?;
        configure_durability(&connection, Durability::Sync)?;
        connection
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(|error| self.map_error("begin_transaction", &error))?;
        let transaction_guard = self.register_transaction()?;
        Ok(Box::new(SqliteTransaction {
            connection: Some(connection),
            started: Instant::now(),
            expiration: self.inner.config.transaction_expiration,
            maximum_batch_operations: self.inner.config.maximum_batch_operations,
            maximum_scan_items: self.inner.config.maximum_scan_items,
            mutation_count: 0,
            _transaction_guard: transaction_guard,
            _operation: operation,
        }))
    }

    async fn stored_bytes(&self, range: KeyRange) -> StorageResult<u64> {
        // SQLite has no size estimate for a key range, so this sums the stored
        // lengths. Bounded by the range the caller asks for, which is one
        // tenant rather than the whole database.
        let _operation = self.begin_operation("stored_bytes")?;
        let connection = self.open_connection(false)?;
        let mut statement = connection
            .prepare(
                "SELECT COALESCE(SUM(LENGTH(key) + LENGTH(value)), 0) FROM mako_kv \
                 WHERE key >= ?1 AND key < ?2",
            )
            .map_err(|_| sqlite_io_error("stored_bytes", "stored size could not be read"))?;
        let total: i64 = statement
            .query_row((&range.start_inclusive, &range.end_exclusive), |row| {
                row.get(0)
            })
            .map_err(|_| sqlite_io_error("stored_bytes", "stored size could not be read"))?;
        Ok(u64::try_from(total).unwrap_or(0))
    }

    async fn health(&self) -> StorageResult<HealthReport> {
        let started = Instant::now();
        let signals = self.health_signals()?;
        let degraded = signals.available_bytes <= self.inner.config.disk_warning_free_bytes
            || signals.wal_bytes > self.inner.config.maximum_wal_bytes;
        Ok(HealthReport {
            status: if signals.integrity_verified && !degraded {
                HealthStatus::Healthy
            } else if signals.integrity_verified {
                HealthStatus::Degraded
            } else {
                HealthStatus::Unavailable
            },
            latency: started.elapsed(),
            durability_verified: signals.integrity_verified
                && signals.schema_version == CONTROL_SQLITE_FORMAT_VERSION,
            safe_diagnostic: degraded.then(|| "control storage capacity requires attention".into()),
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
                Capability::DurableRestart,
            ],
            self.inner.config.maximum_batch_operations,
            self.inner.config.maximum_scan_items,
            Durability::Sync,
        )
    }
}

struct SqliteSnapshot {
    id: SnapshotId,
    connection: Mutex<Option<Connection>>,
    maximum_scan_items: NonZeroUsize,
    _operation: OperationGuard,
}

#[async_trait]
impl KvSnapshot for SqliteSnapshot {
    fn id(&self) -> SnapshotId {
        self.id
    }

    async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        validate_key(key, "snapshot_get")?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| lifecycle_error("snapshot_get"))?;
        let connection = connection.as_ref().ok_or_else(|| {
            StorageError::new(
                StorageErrorKind::Unavailable,
                "snapshot_get",
                false,
                "snapshot is closed",
            )
        })?;
        read_value(connection, key, "snapshot_get")
    }

    async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
        validate_scan(&request, self.maximum_scan_items)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| lifecycle_error("snapshot_scan"))?;
        let connection = connection.as_ref().ok_or_else(|| {
            StorageError::new(
                StorageErrorKind::Unavailable,
                "snapshot_scan",
                false,
                "snapshot is closed",
            )
        })?;
        scan_connection(connection, &request, "snapshot_scan")
    }
}

impl Drop for SqliteSnapshot {
    fn drop(&mut self) {
        if let Ok(connection) = self.connection.get_mut()
            && let Some(connection) = connection.take()
        {
            rollback_quietly(&connection);
        }
    }
}

struct SqliteTransaction {
    connection: Option<Connection>,
    started: Instant,
    expiration: Duration,
    maximum_batch_operations: NonZeroUsize,
    maximum_scan_items: NonZeroUsize,
    mutation_count: usize,
    _transaction_guard: TransactionGuard,
    _operation: OperationGuard,
}

impl SqliteTransaction {
    fn connection(&self, operation: &'static str) -> StorageResult<&Connection> {
        if self.started.elapsed() > self.expiration {
            return Err(StorageError::new(
                StorageErrorKind::Timeout,
                operation,
                false,
                "control storage transaction expired",
            ));
        }
        self.connection.as_ref().ok_or_else(|| {
            StorageError::new(
                StorageErrorKind::Unavailable,
                operation,
                false,
                "control storage transaction is closed",
            )
        })
    }

    fn admit_mutation(&mut self, key: &[u8], operation: &'static str) -> StorageResult<()> {
        validate_key(key, operation)?;
        if self.mutation_count >= self.maximum_batch_operations.get() {
            return Err(StorageError::invalid(
                operation,
                "transaction mutation limit exceeded",
            ));
        }
        self.connection(operation)?;
        self.mutation_count += 1;
        Ok(())
    }
}

#[async_trait]
impl KvTransaction for SqliteTransaction {
    async fn get_for_update(&mut self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        validate_key(key, "transaction_get")?;
        read_value(self.connection("transaction_get")?, key, "transaction_get")
    }

    async fn scan_for_update(&mut self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
        validate_scan(&request, self.maximum_scan_items)?;
        scan_connection(
            self.connection("transaction_scan")?,
            &request,
            "transaction_scan",
        )
    }

    fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> StorageResult<()> {
        self.admit_mutation(&key, "transaction_put")?;
        self.connection("transaction_put")?
            .execute(
                "INSERT INTO mako_kv(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| map_sqlite_error("transaction_put", &error))?;
        Ok(())
    }

    fn delete(&mut self, key: Vec<u8>) -> StorageResult<()> {
        self.admit_mutation(&key, "transaction_delete")?;
        self.connection("transaction_delete")?
            .execute("DELETE FROM mako_kv WHERE key = ?1", params![key])
            .map_err(|error| map_sqlite_error("transaction_delete", &error))?;
        Ok(())
    }

    async fn commit(mut self: Box<Self>, _durability: Durability) -> StorageResult<()> {
        self.connection("transaction_commit")?
            .execute_batch("COMMIT;")
            .map_err(|error| map_sqlite_error("transaction_commit", &error))?;
        self.connection.take();
        Ok(())
    }

    async fn rollback(mut self: Box<Self>) -> StorageResult<()> {
        let connection = self.connection.take().ok_or_else(|| {
            StorageError::new(
                StorageErrorKind::Unavailable,
                "transaction_rollback",
                false,
                "control storage transaction is closed",
            )
        })?;
        connection
            .execute_batch("ROLLBACK;")
            .map_err(|error| map_sqlite_error("transaction_rollback", &error))
    }
}

impl Drop for SqliteTransaction {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            rollback_quietly(&connection);
        }
    }
}

fn configure_connection(connection: &Connection, config: &SqliteConfig) -> StorageResult<()> {
    connection
        .busy_timeout(config.busy_timeout)
        .map_err(|error| map_sqlite_error("sqlite_configure", &error))?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             PRAGMA temp_store = MEMORY;",
        )
        .map_err(|error| map_sqlite_error("sqlite_configure", &error))?;
    connection
        .pragma_update(
            None,
            "wal_autocheckpoint",
            i64::try_from(config.wal_autocheckpoint_pages.get()).map_err(|_| {
                StorageError::invalid(
                    "sqlite_configure",
                    "WAL checkpoint page limit is out of range",
                )
            })?,
        )
        .map_err(|error| map_sqlite_error("sqlite_configure", &error))?;
    configure_durability(connection, config.minimum_durability)
}

fn configure_durability(connection: &Connection, durability: Durability) -> StorageResult<()> {
    let synchronous = match durability {
        Durability::Memory => "OFF",
        Durability::Wal => "NORMAL",
        Durability::Sync => "FULL",
    };
    connection
        .pragma_update(None, "synchronous", synchronous)
        .map_err(|error| map_sqlite_error("sqlite_durability", &error))
}

fn read_value(
    connection: &Connection,
    key: &[u8],
    operation: &'static str,
) -> StorageResult<Option<Vec<u8>>> {
    connection
        .query_row(
            "SELECT value FROM mako_kv WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| map_sqlite_error(operation, &error))
}

fn scan_connection(
    connection: &Connection,
    request: &ScanRequest,
    operation: &'static str,
) -> StorageResult<Vec<KeyValue>> {
    let order = match request.direction {
        ScanDirection::Forward => "ASC",
        ScanDirection::Reverse => "DESC",
    };
    let sql = format!(
        "SELECT key, value FROM mako_kv
         WHERE key >= ?1 AND key < ?2
         ORDER BY key {order} LIMIT ?3"
    );
    let limit = i64::try_from(request.limit.get())
        .map_err(|_| StorageError::invalid(operation, "scan limit is out of range"))?;
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| map_sqlite_error(operation, &error))?;
    let rows = statement
        .query_map(
            params![
                &request.range.start_inclusive,
                &request.range.end_exclusive,
                limit
            ],
            |row| {
                Ok(KeyValue {
                    key: row.get(0)?,
                    value: row.get(1)?,
                })
            },
        )
        .map_err(|error| map_sqlite_error(operation, &error))?;
    rows.map(|row| row.map_err(|error| map_sqlite_error(operation, &error)))
        .collect()
}

fn execute_batch_transaction(
    connection: &Connection,
    batch: &WriteBatch,
    operation: &'static str,
    busy_failures: &AtomicU64,
) -> StorageResult<()> {
    connection
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(|error| tracked_sqlite_error(operation, &error, busy_failures))?;
    if let Err(error) = apply_operations(connection, batch, operation) {
        rollback_quietly(connection);
        return Err(error);
    }
    connection
        .execute_batch("COMMIT;")
        .map_err(|error| tracked_sqlite_error(operation, &error, busy_failures))
}

fn tracked_sqlite_error(
    operation: &'static str,
    error: &rusqlite::Error,
    busy_failures: &AtomicU64,
) -> StorageError {
    let mapped = map_sqlite_error(operation, error);
    if mapped.kind == StorageErrorKind::Timeout {
        busy_failures.fetch_add(1, Ordering::Relaxed);
    }
    mapped
}

fn apply_operations(
    connection: &Connection,
    batch: &WriteBatch,
    operation: &'static str,
) -> StorageResult<()> {
    for mutation in batch.operations() {
        match mutation {
            WriteOperation::Put { key, value } => connection
                .execute(
                    "INSERT INTO mako_kv(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map(|_| ())
                .map_err(|error| map_sqlite_error(operation, &error))?,
            WriteOperation::Delete { key } => connection
                .execute("DELETE FROM mako_kv WHERE key = ?1", params![key])
                .map(|_| ())
                .map_err(|error| map_sqlite_error(operation, &error))?,
        }
    }
    Ok(())
}

fn validate_key(key: &[u8], operation: &'static str) -> StorageResult<()> {
    if key.is_empty() {
        return Err(StorageError::invalid(operation, "key cannot be empty"));
    }
    Ok(())
}

fn validate_scan(request: &ScanRequest, maximum: NonZeroUsize) -> StorageResult<()> {
    if request.limit.get() > maximum.get() {
        return Err(StorageError::invalid(
            "scan",
            "scan limit exceeds adapter limit",
        ));
    }
    Ok(())
}

fn validate_batch(batch: &WriteBatch, maximum: NonZeroUsize) -> StorageResult<()> {
    if batch.is_empty() {
        return Err(StorageError::invalid(
            "write",
            "write batch cannot be empty",
        ));
    }
    if batch.len() > maximum.get() {
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

fn normalize_file_path(path: &Path, field: &'static str) -> StorageResult<PathBuf> {
    let parent = parent_of(path)?;
    fs::create_dir_all(parent).map_err(|_| {
        sqlite_io_error(
            "sqlite_path",
            "control storage directory could not be created",
        )
    })?;
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(StorageError::invalid(
            "sqlite_path",
            format!("{field} cannot be a symbolic link"),
        ));
    }
    let canonical_parent = parent.canonicalize().map_err(|_| {
        sqlite_io_error(
            "sqlite_path",
            "control storage directory could not be resolved",
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        StorageError::invalid("sqlite_path", format!("{field} must identify a file"))
    })?;
    Ok(canonical_parent.join(file_name))
}

fn parent_of(path: &Path) -> StorageResult<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| StorageError::invalid("sqlite_path", "storage path has no parent"))
}

fn wal_path(database: &Path) -> PathBuf {
    let mut path = database.as_os_str().to_owned();
    path.push("-wal");
    PathBuf::from(path)
}

fn file_size(path: &Path) -> StorageResult<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(_) => Err(sqlite_io_error(
            "sqlite_health",
            "control storage file metadata is unavailable",
        )),
    }
}

fn sync_file_and_parent(path: &Path) -> StorageResult<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| {
            sqlite_io_error("sqlite_sync", "control database could not be synchronized")
        })?;
    File::open(parent_of(path)?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| {
            sqlite_io_error(
                "sqlite_sync",
                "control storage directory could not be synchronized",
            )
        })
}

fn read_schema_version(connection: &Connection) -> StorageResult<u32> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| map_sqlite_error("sqlite_schema", &error))
}

fn integrity_ok(connection: &Connection) -> StorageResult<bool> {
    connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
        .map(|result| result == "ok")
        .map_err(|error| map_sqlite_error("sqlite_integrity", &error))
}

fn rollback_quietly(connection: &Connection) {
    let _ignored = connection.execute_batch("ROLLBACK;");
}

fn lifecycle_error(operation: &'static str) -> StorageError {
    StorageError::new(
        StorageErrorKind::Internal,
        operation,
        false,
        "control storage lifecycle state is unavailable",
    )
}

fn sqlite_io_error(operation: &'static str, message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Io, operation, true, message)
}

fn map_sqlite_error(operation: &'static str, error: &rusqlite::Error) -> StorageError {
    use rusqlite::ffi::ErrorCode;

    let code = error.sqlite_error_code();
    let (kind, retryable, message) = match code {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => (
            StorageErrorKind::Timeout,
            true,
            "control storage is temporarily busy",
        ),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => (
            StorageErrorKind::Corruption,
            false,
            "control database integrity is invalid",
        ),
        Some(
            ErrorCode::ReadOnly
            | ErrorCode::DiskFull
            | ErrorCode::SystemIoFailure
            | ErrorCode::CannotOpen
            | ErrorCode::PermissionDenied,
        ) => (
            StorageErrorKind::Io,
            true,
            "control storage I/O is unavailable",
        ),
        Some(ErrorCode::ConstraintViolation) => (
            StorageErrorKind::Conflict,
            false,
            "control storage constraint was not satisfied",
        ),
        Some(ErrorCode::TooBig | ErrorCode::ParameterOutOfRange | ErrorCode::TypeMismatch) => (
            StorageErrorKind::InvalidRequest,
            false,
            "control storage rejected an invalid value",
        ),
        Some(ErrorCode::OperationInterrupted | ErrorCode::OperationAborted) => (
            StorageErrorKind::Timeout,
            true,
            "control storage operation was interrupted",
        ),
        _ => (
            StorageErrorKind::Internal,
            false,
            "control storage reported an internal failure",
        ),
    };
    StorageError::new(kind, operation, retryable, message)
}
