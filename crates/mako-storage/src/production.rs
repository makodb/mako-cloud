use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter,
    RocksDbAdapter, RocksDbConfig, ScanDirection, ScanRequest, SequencerKeyKind, StorageError,
    StorageErrorKind, StorageReadiness, StorageResult, TenantKeyspace, WriteBatch,
    check_storage_readiness,
    key_codec::{TenantIdentity, decode_tenant_span, tenant_namespace_range},
};

pub const MARKER_VERSION: u32 = 1;
pub const DATABASE_FORMAT_VERSION: u32 = 1;
pub const MARKER_FILE_NAME: &str = "mako-volume.json";
const MARKER_TEMP_FILE_NAME: &str = ".mako-volume.json.tmp";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionVolumeIdentity {
    pub service: String,
    pub database_id: String,
}

impl ProductionVolumeIdentity {
    pub fn new(service: impl Into<String>, database_id: impl Into<String>) -> StorageResult<Self> {
        let identity = Self {
            service: service.into(),
            database_id: database_id.into(),
        };
        for (field, value) in [
            ("service identity", identity.service.as_str()),
            ("database identity", identity.database_id.as_str()),
        ] {
            if value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            {
                return Err(StorageError::invalid(
                    "production_identity",
                    format!("{field} must be a 1-128 character safe identifier"),
                ));
            }
        }
        Ok(identity)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeState {
    Provisioned,
    Active,
    Fenced,
    Promotable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeMarker {
    pub marker_version: u32,
    pub database_format_version: u32,
    pub identity: ProductionVolumeIdentity,
    pub state: VolumeState,
}

#[derive(Clone, Debug)]
pub struct ProductionRocksDbConfig {
    pub database_path: PathBuf,
    pub identity: ProductionVolumeIdentity,
    pub maximum_batch_operations: NonZeroUsize,
    pub maximum_scan_items: NonZeroUsize,
    pub transaction_lock_timeout: Duration,
    pub transaction_expiration: Duration,
    pub disk_warning_free_bytes: u64,
    pub disk_critical_free_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionStorageHealth {
    pub database_open: bool,
    pub lock_held: bool,
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub below_warning_threshold: bool,
    pub below_critical_threshold: bool,
    pub synchronous_durability: bool,
    pub wal_verification: bool,
    pub paranoid_checks: bool,
    pub sequencer_recovery_complete: bool,
    pub write_stopped: bool,
    pub delayed_write_rate: u64,
    pub pending_compaction_bytes: u64,
    pub running_compactions: u64,
    pub running_flushes: u64,
    pub background_errors: u64,
    pub io_errors: u64,
    pub corruption_signals: u64,
    pub recovery_duration: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TenantRecoveryEvidence {
    pub project: Vec<u8>,
    pub environment: Vec<u8>,
    pub acknowledged_high_water: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SequencerRecoveryReport {
    pub tenants: Vec<TenantRecoveryEvidence>,
}

#[derive(Debug)]
pub struct ProductionRocksDb {
    adapter: RocksDbAdapter,
    marker: VolumeMarker,
    readiness: StorageReadiness,
    health: ProductionStorageHealth,
    recovery: SequencerRecoveryReport,
}

impl ProductionRocksDb {
    pub async fn open(config: ProductionRocksDbConfig) -> StorageResult<Self> {
        validate_production_config(&config)?;
        let mut marker = read_volume_marker(&config.database_path)?;
        validate_marker(&marker, &config.identity)?;
        if !matches!(marker.state, VolumeState::Provisioned | VolumeState::Active) {
            return Err(StorageError::new(
                StorageErrorKind::Unavailable,
                "production_open",
                false,
                "volume is fenced or awaiting explicit promotion",
            ));
        }

        let available_bytes = fs2::available_space(&config.database_path)
            .map_err(|_| safe_io("capacity_check", true, "filesystem capacity is unavailable"))?;
        let total_bytes = fs2::total_space(&config.database_path)
            .map_err(|_| safe_io("capacity_check", true, "filesystem capacity is unavailable"))?;
        if available_bytes < config.disk_critical_free_bytes {
            return Err(StorageError::new(
                StorageErrorKind::Unavailable,
                "capacity_check",
                true,
                "filesystem is below the critical free-space threshold",
            ));
        }

        let started = Instant::now();
        let mut rocks_config = RocksDbConfig::new(&config.database_path);
        rocks_config.create_if_missing = marker.state == VolumeState::Provisioned;
        rocks_config.minimum_durability = Durability::Sync;
        rocks_config.maximum_batch_operations = config.maximum_batch_operations;
        rocks_config.maximum_scan_items = config.maximum_scan_items;
        rocks_config.transaction_lock_timeout = config.transaction_lock_timeout;
        rocks_config.transaction_expiration = config.transaction_expiration;
        let adapter = RocksDbAdapter::open(rocks_config)?;
        let recovery = recover_all_sequencers(&adapter).await?;
        let readiness = check_storage_readiness(&adapter, Durability::Sync).await;
        if !readiness.is_ready() {
            return Err(StorageError::new(
                StorageErrorKind::Unavailable,
                "production_readiness",
                true,
                readiness.to_string(),
            ));
        }
        if marker.state == VolumeState::Provisioned {
            marker.state = VolumeState::Active;
            write_volume_marker(&config.database_path, &marker, false)?;
        }
        let signals = adapter.health_signals()?;
        let health = ProductionStorageHealth {
            database_open: true,
            lock_held: true,
            available_bytes,
            total_bytes,
            below_warning_threshold: available_bytes < config.disk_warning_free_bytes,
            below_critical_threshold: false,
            synchronous_durability: true,
            wal_verification: true,
            paranoid_checks: true,
            sequencer_recovery_complete: true,
            write_stopped: signals.write_stopped,
            delayed_write_rate: signals.delayed_write_rate,
            pending_compaction_bytes: signals.pending_compaction_bytes,
            running_compactions: signals.running_compactions,
            running_flushes: signals.running_flushes,
            background_errors: signals.background_errors,
            io_errors: signals.io_errors,
            corruption_signals: signals.corruption_signals,
            recovery_duration: started.elapsed(),
        };
        Ok(Self {
            adapter,
            marker,
            readiness,
            health,
            recovery,
        })
    }

    #[must_use]
    pub const fn adapter(&self) -> &RocksDbAdapter {
        &self.adapter
    }

    #[must_use]
    pub const fn marker(&self) -> &VolumeMarker {
        &self.marker
    }

    #[must_use]
    pub const fn readiness(&self) -> &StorageReadiness {
        &self.readiness
    }

    #[must_use]
    pub const fn storage_health(&self) -> &ProductionStorageHealth {
        &self.health
    }

    #[must_use]
    pub const fn recovery_report(&self) -> &SequencerRecoveryReport {
        &self.recovery
    }

    pub async fn graceful_shutdown(self) -> StorageResult<()> {
        self.adapter.health().await?;
        drop(self);
        Ok(())
    }
}

pub(crate) async fn recover_all_sequencers(
    adapter: &RocksDbAdapter,
) -> StorageResult<SequencerRecoveryReport> {
    let tenants = discover_tenants(adapter).await?;
    let mut report = SequencerRecoveryReport::default();
    for (project, environment) in tenants {
        let keyspace = TenantKeyspace::new(project.clone(), environment.clone()).map_err(|_| {
            StorageError::new(
                StorageErrorKind::Corruption,
                "sequencer_recovery",
                false,
                "tenant key prefix is invalid",
            )
        })?;
        let acknowledged_high_water = recover_tenant_sequencer(adapter, &keyspace).await?;
        report.tenants.push(TenantRecoveryEvidence {
            project,
            environment,
            acknowledged_high_water,
        });
    }
    Ok(report)
}

/// Every tenant that owns at least one key, found by reading one key per
/// tenant.
///
/// The keys are the only record of which tenants exist. This used to read
/// every key and value in the store to collect them, on every open, so opening
/// the beta's 8.5 GB data plane took about 90 seconds and outlasted the
/// release operation's readiness wait. All of a tenant's keys share its
/// encoded prefix, so the walk now seeks past that prefix as soon as it meets
/// it. It still decodes every key that is not inside an already-found tenant,
/// so a corrupt tenant prefix is refused exactly as before.
async fn discover_tenants(adapter: &dyn KvAdapter) -> StorageResult<BTreeSet<TenantIdentity>> {
    let namespace = tenant_namespace_range().map_err(|_| corrupt_tenant_prefix())?;
    let end = namespace.end_exclusive;
    let mut start = namespace.start_inclusive;
    let mut tenants = BTreeSet::new();
    while start < end {
        let entries = adapter
            .scan(ScanRequest::new(
                KeyRange::new(start, end.clone())?,
                ScanDirection::Forward,
                NonZeroUsize::MIN,
            ))
            .await?;
        let Some(entry) = entries.into_iter().next() else {
            break;
        };
        start = match decode_tenant_span(&entry.key).map_err(|_| corrupt_tenant_prefix())? {
            Some((identity, after)) => {
                tenants.insert(identity);
                after
            }
            None => {
                let mut next = entry.key;
                next.push(0);
                next
            }
        };
    }
    Ok(tenants)
}

fn corrupt_tenant_prefix() -> StorageError {
    corrupt_recovery("tenant key prefix is corrupt")
}

async fn recover_tenant_sequencer(
    adapter: &RocksDbAdapter,
    keyspace: &TenantKeyspace,
) -> StorageResult<u64> {
    let lease_key = keyspace.sequencer_key(SequencerKeyKind::Lease);
    let high_water_key = keyspace.sequencer_key(SequencerKeyKind::HighWater);
    let Some(lease) = adapter.get(&lease_key).await? else {
        return Ok(0);
    };
    let next_unallocated = decode_u64(&lease, "sequencer lease")?;
    if next_unallocated == 0 {
        return Err(corrupt_recovery("sequencer lease is invalid"));
    }
    let highest_allocated = next_unallocated - 1;

    loop {
        let stored = adapter.get(&high_water_key).await?;
        let current = stored
            .as_deref()
            .map(|value| decode_u64(value, "acknowledged high water"))
            .transpose()?
            .unwrap_or_default();
        if current > highest_allocated {
            return Err(corrupt_recovery(
                "acknowledged high water exceeds allocated positions",
            ));
        }
        for position in 1..=current {
            let committed = adapter
                .get(&keyspace.sequencer_position_key(SequencerKeyKind::Committed, position))
                .await?
                .is_some();
            let aborted = adapter
                .get(&keyspace.sequencer_position_key(SequencerKeyKind::Aborted, position))
                .await?
                .is_some();
            if !matches!((committed, aborted), (true, false) | (false, true)) {
                return Err(corrupt_recovery(
                    "acknowledged sequencer position lacks one terminal state",
                ));
            }
        }
        let mut advanced = current;
        while advanced < highest_allocated {
            let candidate = advanced + 1;
            let committed = adapter
                .get(&keyspace.sequencer_position_key(SequencerKeyKind::Committed, candidate))
                .await?
                .is_some();
            let aborted = adapter
                .get(&keyspace.sequencer_position_key(SequencerKeyKind::Aborted, candidate))
                .await?
                .is_some();
            match (committed, aborted) {
                (true, false) | (false, true) => advanced = candidate,
                (false, false) => break,
                (true, true) => {
                    return Err(corrupt_recovery(
                        "sequencer position has conflicting terminal states",
                    ));
                }
            }
        }
        if advanced == current {
            return Ok(current);
        }
        let condition = match &stored {
            Some(value) => KeyCondition::ValueEquals {
                key: high_water_key.clone(),
                value: value.clone(),
            },
            None => KeyCondition::Missing {
                key: high_water_key.clone(),
            },
        };
        let mut batch = WriteBatch::new();
        batch.put(&high_water_key, advanced.to_be_bytes());
        if adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![condition],
                batch,
                durability: Durability::Sync,
            })
            .await?
            == CompareAndWriteResult::Applied
        {
            return Ok(advanced);
        }
    }
}

fn decode_u64(value: &[u8], label: &'static str) -> StorageResult<u64> {
    value
        .try_into()
        .map(u64::from_be_bytes)
        .map_err(|_| corrupt_recovery(label))
}

fn corrupt_recovery(message: &'static str) -> StorageError {
    StorageError::new(
        StorageErrorKind::Corruption,
        "sequencer_recovery",
        false,
        message,
    )
}

pub fn provision_production_volume(
    database_path: &Path,
    identity: ProductionVolumeIdentity,
) -> StorageResult<VolumeMarker> {
    if !database_path.is_absolute() {
        return Err(StorageError::invalid(
            "volume_provision",
            "production database path must be absolute",
        ));
    }
    let metadata = fs::metadata(database_path).map_err(|_| {
        safe_io(
            "volume_provision",
            false,
            "database volume must already exist",
        )
    })?;
    if !metadata.is_dir() {
        return Err(StorageError::invalid(
            "volume_provision",
            "database volume must be a directory",
        ));
    }
    let mut entries = fs::read_dir(database_path).map_err(|_| {
        safe_io(
            "volume_provision",
            false,
            "database volume cannot be inspected",
        )
    })?;
    if entries
        .next()
        .transpose()
        .map_err(|_| {
            safe_io(
                "volume_provision",
                false,
                "database volume cannot be inspected",
            )
        })?
        .is_some()
    {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "volume_provision",
            false,
            "database volume is not empty",
        ));
    }

    let marker = VolumeMarker {
        marker_version: MARKER_VERSION,
        database_format_version: DATABASE_FORMAT_VERSION,
        identity,
        state: VolumeState::Provisioned,
    };
    write_volume_marker(database_path, &marker, true)?;
    Ok(marker)
}

pub fn read_volume_marker(database_path: &Path) -> StorageResult<VolumeMarker> {
    let bytes = fs::read(database_path.join(MARKER_FILE_NAME)).map_err(|_| {
        safe_io(
            "volume_marker",
            false,
            "provisioned volume marker is missing or unreadable",
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        StorageError::new(
            StorageErrorKind::Corruption,
            "volume_marker",
            false,
            "provisioned volume marker is invalid",
        )
    })
}

pub fn fence_production_volume(
    database_path: &Path,
    expected_identity: &ProductionVolumeIdentity,
) -> StorageResult<VolumeMarker> {
    transition_volume_state(
        database_path,
        expected_identity,
        &[VolumeState::Provisioned, VolumeState::Active],
        VolumeState::Fenced,
    )
}

pub fn promote_restored_volume(
    database_path: &Path,
    expected_identity: &ProductionVolumeIdentity,
) -> StorageResult<VolumeMarker> {
    transition_volume_state(
        database_path,
        expected_identity,
        &[VolumeState::Promotable],
        VolumeState::Active,
    )
}

pub fn activate_fenced_volume(
    database_path: &Path,
    expected_identity: &ProductionVolumeIdentity,
) -> StorageResult<VolumeMarker> {
    transition_volume_state(
        database_path,
        expected_identity,
        &[VolumeState::Fenced],
        VolumeState::Active,
    )
}

fn transition_volume_state(
    database_path: &Path,
    expected_identity: &ProductionVolumeIdentity,
    allowed: &[VolumeState],
    target: VolumeState,
) -> StorageResult<VolumeMarker> {
    let mut marker = read_volume_marker(database_path)?;
    validate_marker(&marker, expected_identity)?;
    if !allowed.contains(&marker.state) {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "volume_state",
            false,
            "volume state does not permit the requested transition",
        ));
    }
    marker.state = target;
    write_volume_marker(database_path, &marker, false)?;
    Ok(marker)
}

pub(crate) fn write_volume_marker(
    database_path: &Path,
    marker: &VolumeMarker,
    create_new: bool,
) -> StorageResult<()> {
    let bytes = serde_json::to_vec_pretty(marker).map_err(|_| {
        StorageError::new(
            StorageErrorKind::Internal,
            "volume_marker",
            false,
            "volume marker could not be encoded",
        )
    })?;
    let marker_path = database_path.join(MARKER_FILE_NAME);
    let temporary_path = database_path.join(MARKER_TEMP_FILE_NAME);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&temporary_path).map_err(|_| {
        safe_io(
            "volume_marker",
            false,
            "volume marker staging file cannot be created",
        )
    })?;
    file.write_all(&bytes)
        .map_err(|_| safe_io("volume_marker", false, "volume marker cannot be written"))?;
    file.sync_all().map_err(|_| {
        safe_io(
            "volume_marker",
            false,
            "volume marker cannot be synchronized",
        )
    })?;
    drop(file);
    if create_new && marker_path.exists() {
        let _ignored = fs::remove_file(&temporary_path);
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "volume_marker",
            false,
            "volume marker already exists",
        ));
    }
    fs::rename(&temporary_path, &marker_path).map_err(|_| {
        let _ignored = fs::remove_file(&temporary_path);
        safe_io("volume_marker", false, "volume marker cannot be installed")
    })?;
    sync_directory(database_path)?;
    Ok(())
}

fn validate_marker(
    marker: &VolumeMarker,
    expected_identity: &ProductionVolumeIdentity,
) -> StorageResult<()> {
    if marker.marker_version != MARKER_VERSION
        || marker.database_format_version != DATABASE_FORMAT_VERSION
    {
        return Err(StorageError::new(
            StorageErrorKind::Unsupported,
            "volume_marker",
            false,
            "volume marker or database format version is unsupported",
        ));
    }
    if &marker.identity != expected_identity {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "volume_marker",
            false,
            "volume is owned by a different service or database",
        ));
    }
    Ok(())
}

fn validate_production_config(config: &ProductionRocksDbConfig) -> StorageResult<()> {
    if !config.database_path.is_absolute() {
        return Err(StorageError::invalid(
            "production_config",
            "database path must be absolute",
        ));
    }
    if config.transaction_lock_timeout.is_zero()
        || config.transaction_expiration < config.transaction_lock_timeout
        || config.disk_critical_free_bytes == 0
        || config.disk_warning_free_bytes <= config.disk_critical_free_bytes
    {
        return Err(StorageError::invalid(
            "production_config",
            "production storage limits are unsafe",
        ));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> StorageResult<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| {
            safe_io(
                "volume_marker",
                false,
                "volume directory cannot be synchronized",
            )
        })
}

fn safe_io(operation: &'static str, retryable: bool, message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Io, operation, retryable, message)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use futures::executor::block_on;

    use super::*;
    use crate::{
        AdapterCapabilities, HealthReport, KeyValue, KvSnapshot, KvTransaction, MemoryAdapter,
        TransactionMode,
    };

    /// A memory adapter that counts the entries its scans return.
    struct CountingAdapter {
        inner: MemoryAdapter,
        scanned: AtomicUsize,
    }

    #[async_trait]
    impl KvAdapter for CountingAdapter {
        async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
            self.inner.get(key).await
        }

        async fn scan(&self, request: ScanRequest) -> StorageResult<Vec<KeyValue>> {
            let entries = self.inner.scan(request).await?;
            self.scanned.fetch_add(entries.len(), Ordering::Relaxed);
            Ok(entries)
        }

        async fn snapshot(&self) -> StorageResult<Box<dyn KvSnapshot>> {
            self.inner.snapshot().await
        }

        async fn write(&self, batch: WriteBatch, durability: Durability) -> StorageResult<()> {
            self.inner.write(batch, durability).await
        }

        async fn compare_and_write(
            &self,
            request: AtomicWrite,
        ) -> StorageResult<CompareAndWriteResult> {
            self.inner.compare_and_write(request).await
        }

        async fn begin_transaction(
            &self,
            mode: TransactionMode,
        ) -> StorageResult<Box<dyn KvTransaction>> {
            self.inner.begin_transaction(mode).await
        }

        async fn stored_bytes(&self, range: KeyRange) -> StorageResult<u64> {
            self.inner.stored_bytes(range).await
        }

        async fn count_keys(&self, range: KeyRange) -> StorageResult<u64> {
            self.inner.count_keys(range).await
        }

        async fn health(&self) -> StorageResult<HealthReport> {
            self.inner.health().await
        }

        fn capabilities(&self) -> AdapterCapabilities {
            self.inner.capabilities()
        }
    }

    fn store(keys: impl IntoIterator<Item = Vec<u8>>) -> CountingAdapter {
        let inner = MemoryAdapter::with_limits(
            NonZeroUsize::new(1_000).expect("non-zero"),
            NonZeroUsize::new(1_000_000).expect("non-zero"),
        );
        let keys: Vec<Vec<u8>> = keys.into_iter().collect();
        for chunk in keys.chunks(1_000) {
            let mut batch = WriteBatch::new();
            for key in chunk {
                batch.put(key, b"value");
            }
            block_on(inner.write(batch, Durability::Memory)).expect("write keys");
        }
        CountingAdapter {
            inner,
            scanned: AtomicUsize::new(0),
        }
    }

    fn tenant(project: &[u8], environment: &[u8]) -> TenantKeyspace {
        TenantKeyspace::new(project.to_vec(), environment.to_vec()).expect("tenant keyspace")
    }

    /// The walk this replaced: decode every key in the store.
    fn decode_every_key(adapter: &CountingAdapter) -> Option<BTreeSet<TenantIdentity>> {
        let everything = block_on(adapter.inner.scan(ScanRequest::new(
            KeyRange::new(vec![0], vec![0xff]).expect("range"),
            ScanDirection::Forward,
            NonZeroUsize::new(1_000_000).expect("non-zero"),
        )));
        let mut tenants = BTreeSet::new();
        for entry in everything.expect("scan the whole store") {
            match decode_tenant_span(&entry.key) {
                Ok(Some((identity, _))) => {
                    tenants.insert(identity);
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        Some(tenants)
    }

    #[test]
    fn tenant_discovery_reads_one_key_per_tenant() {
        let large = tenant(b"prj_large", b"env_live");
        let sibling = tenant(b"prj_large", b"env_dev");
        // A zero byte is escaped inside a segment; this tenant sorts right
        // after one whose environment it extends, and must not be skipped.
        let escaped = tenant(b"prj_large", b"env_dev\0x");
        let other = tenant(b"prj_other", b"env_live");
        let mut keys = vec![
            TenantKeyspace::system_key(b"domain", b"item").expect("system key"),
            TenantKeyspace::project_metadata_key(b"prj_large").expect("project key"),
            TenantKeyspace::project_metadata_key(b"prj_other").expect("project key"),
            sibling.sequencer_key(SequencerKeyKind::Lease),
            escaped.environment_metadata_key(),
            other.sequencer_key(SequencerKeyKind::HighWater),
        ];
        keys.extend(
            (1..=5_000).map(|position| {
                large.sequencer_position_key(SequencerKeyKind::Committed, position)
            }),
        );
        let adapter = store(keys);

        let tenants = block_on(discover_tenants(&adapter)).expect("discover tenants");

        let expected: BTreeSet<TenantIdentity> = [&large, &sibling, &escaped, &other]
            .into_iter()
            .map(|keyspace| (keyspace.project().to_vec(), keyspace.environment().to_vec()))
            .collect();
        assert_eq!(tenants, expected);
        assert_eq!(decode_every_key(&adapter), Some(expected));
        // Four tenants and two project records: none of the 5,000 keys under
        // the large tenant after its first, and nothing outside the project
        // namespace.
        assert_eq!(adapter.scanned.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn tenant_discovery_agrees_with_decoding_every_key() {
        let identities: [(&[u8], &[u8]); 5] = [
            (b"p", b"e"),
            (b"p", b"e\0"),
            (b"p\0", b"e"),
            (b"p\xff", b"\xff\0\xff"),
            (b"q", b"e"),
        ];
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            state >> 33
        };
        let mut refused = 0;
        for _ in 0..300 {
            let count = 1 + next() % 40;
            let keys: Vec<Vec<u8>> = (0..count)
                .map(|_| match next() % 6 {
                    0..=2 => {
                        let (project, environment) = identities[(next() % 5) as usize];
                        tenant(project, environment)
                            .sequencer_position_key(SequencerKeyKind::Committed, next())
                    }
                    3 => TenantKeyspace::project_metadata_key(identities[(next() % 5) as usize].0)
                        .expect("project key"),
                    // Raw bytes in the project namespace: some decode, some are
                    // malformed and must be refused as corrupt.
                    4 => {
                        let mut key = vec![1, 0x20];
                        key.extend(
                            (0..next() % 8).map(|_| [0, 0x30, b'p', 0xff][(next() % 4) as usize]),
                        );
                        key
                    }
                    _ => (0..1 + next() % 4).map(|_| (next() % 256) as u8).collect(),
                })
                .collect();
            let adapter = store(keys);
            let discovered = block_on(discover_tenants(&adapter));
            match decode_every_key(&adapter) {
                Some(expected) => assert_eq!(discovered.expect("no corrupt key"), expected),
                None => {
                    let error = discovered.expect_err("a corrupt key is refused");
                    assert_eq!(error.kind, StorageErrorKind::Corruption);
                    refused += 1;
                }
            }
        }
        assert!(refused > 0, "the generator never produced a corrupt key");
    }
}
