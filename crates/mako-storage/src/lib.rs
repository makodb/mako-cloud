//! Semantic ordered key-value storage boundary and adapters.
//!
//! Production exposes no remote connector or backend-selection surface:
//!
//! ```compile_fail
//! use mako_storage::{AdapterConnectionConfig, DistributedAdapter};
//! ```

#![forbid(unsafe_code)]

mod backup;
mod conformance;
mod contract;
mod control_migration;
mod control_sqlite_backup;
mod error;
mod key_codec;
mod memory;
mod production;
mod readiness;
mod rocks;
mod sqlite;

pub use backup::{
    BACKUP_MANIFEST_VERSION, BackupArtifact, BackupFileInventory, BackupId, BackupManifest,
    BackupManifestEnvelope, BackupMetrics, BackupRequest, BackupTelemetry,
    FilesystemBackupTransport, ManifestSigningKey, RestorePolicy, RestoreReport,
    TenantBackupInventory, inspect_backup, restore_backup,
};
pub use contract::{
    AdapterCapabilities, AtomicWrite, Capability, CompareAndWriteResult, ConditionalPut,
    Durability, HealthReport, HealthStatus, KeyCondition, KeyRange, KeyValue, KvAdapter,
    KvSnapshot, KvTransaction, ScanDirection, ScanRequest, SnapshotId, TransactionMode, WriteBatch,
    WriteOperation,
};
pub use control_migration::{
    CONTROL_MIGRATION_FORMAT_VERSION, ControlMigrationPlan, ControlMigrationReceipt, KvInventory,
    inspect_control_rocks_checkpoint, inspect_control_sqlite, migrate_control_rocks_to_sqlite,
    write_control_checkpoint_fence, write_control_migration_receipt,
};
pub use control_sqlite_backup::{
    CONTROL_BACKUP_FORMAT_VERSION, ControlBackupManifest, ControlBackupSigningKey,
    ControlRestoreReport, create_control_sqlite_backup, inspect_control_sqlite_backup,
    promote_control_sqlite_restore, restore_control_sqlite_backup,
};
pub use error::{StorageError, StorageErrorKind, StorageResult};
pub use key_codec::{
    DecodedChangeKey, DecodedIndexKey, DecodedRevisionKey, KeyCodecError, SequencerKeyKind,
    TenantKeyspace,
};
pub use memory::{FailurePoint, MemoryAdapter};
pub use production::{
    DATABASE_FORMAT_VERSION, MARKER_FILE_NAME, MARKER_VERSION, ProductionRocksDb,
    ProductionRocksDbConfig, ProductionStorageHealth, ProductionVolumeIdentity,
    SequencerRecoveryReport, TenantRecoveryEvidence, VolumeMarker, VolumeState,
    activate_fenced_volume, fence_production_volume, promote_restored_volume,
    provision_production_volume, read_volume_marker,
};
pub use readiness::{ReadinessFailure, ReadinessStatus, StorageReadiness, check_storage_readiness};
pub use rocks::{RocksDbAdapter, RocksDbConfig, RocksDbHealthSignals};
pub use sqlite::{CONTROL_SQLITE_FORMAT_VERSION, SqliteAdapter, SqliteConfig, SqliteHealthSignals};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "storage";
pub use conformance::{
    ConformanceCheck, ConformanceFactory, ConformanceOutcome, ConformanceReport,
    run_adapter_conformance,
};
