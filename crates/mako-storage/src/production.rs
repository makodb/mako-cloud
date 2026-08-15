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
    check_storage_readiness, key_codec::decode_tenant_identity,
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
        let recovery = recover_all_sequencers(&adapter, config.maximum_scan_items).await?;
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
    page_size: NonZeroUsize,
) -> StorageResult<SequencerRecoveryReport> {
    let mut tenants = BTreeSet::new();
    let mut start = vec![0];
    let end = vec![0xff];
    loop {
        let entries = adapter
            .scan(ScanRequest::new(
                KeyRange::new(start.clone(), end.clone())?,
                ScanDirection::Forward,
                page_size,
            ))
            .await?;
        for entry in &entries {
            if let Some(identity) = decode_tenant_identity(&entry.key).map_err(|_| {
                StorageError::new(
                    StorageErrorKind::Corruption,
                    "sequencer_recovery",
                    false,
                    "tenant key prefix is corrupt",
                )
            })? {
                tenants.insert(identity);
            }
        }
        if entries.len() < page_size.get() {
            break;
        }
        let Some(last) = entries.last() else {
            break;
        };
        start = last.key.clone();
        start.push(0);
    }

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
