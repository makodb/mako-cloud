use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::Duration,
};

use futures::executor::block_on;
use mako_storage::{
    DATABASE_FORMAT_VERSION, Durability, KvAdapter, MARKER_FILE_NAME, ProductionRocksDb,
    ProductionRocksDbConfig, ProductionVolumeIdentity, RocksDbAdapter, RocksDbConfig,
    SequencerKeyKind, StorageErrorKind, TenantKeyspace, VolumeState, WriteBatch,
    provision_production_volume, read_volume_marker,
};
use tempfile::TempDir;

fn identity(service: &str) -> ProductionVolumeIdentity {
    ProductionVolumeIdentity::new(service, format!("{service}-us-east-1")).expect("identity")
}

fn config(path: impl Into<PathBuf>, identity: ProductionVolumeIdentity) -> ProductionRocksDbConfig {
    ProductionRocksDbConfig {
        database_path: path.into(),
        identity,
        maximum_batch_operations: NonZeroUsize::new(10_000).expect("constant"),
        maximum_scan_items: NonZeroUsize::new(10_000).expect("constant"),
        transaction_lock_timeout: Duration::from_millis(100),
        transaction_expiration: Duration::from_secs(5),
        disk_warning_free_bytes: 2,
        disk_critical_free_bytes: 1,
    }
}

fn empty_volume() -> (TempDir, PathBuf) {
    let directory = TempDir::new().expect("temporary root");
    let path = directory.path().join("volume");
    fs::create_dir(&path).expect("volume directory");
    (directory, path)
}

#[test]
fn provisioned_volume_opens_sync_and_becomes_active() {
    block_on(async {
        let (_directory, path) = empty_volume();
        let owner = identity("mako-data-plane");
        let marker = provision_production_volume(&path, owner.clone()).expect("provision");
        assert_eq!(marker.state, VolumeState::Provisioned);

        let storage = ProductionRocksDb::open(config(&path, owner))
            .await
            .expect("production open");
        assert!(storage.readiness().is_ready());
        assert!(storage.storage_health().database_open);
        assert!(storage.storage_health().lock_held);
        assert!(storage.storage_health().synchronous_durability);
        assert!(storage.storage_health().wal_verification);
        assert!(storage.storage_health().paranoid_checks);
        assert!(storage.storage_health().sequencer_recovery_complete);
        assert!(!storage.storage_health().write_stopped);
        assert_eq!(storage.storage_health().background_errors, 0);
        assert_eq!(storage.storage_health().io_errors, 0);
        assert_eq!(storage.storage_health().corruption_signals, 0);
        assert_eq!(storage.marker().state, VolumeState::Active);

        let mut batch = WriteBatch::new();
        batch.put(b"acknowledged", b"value");
        storage
            .adapter()
            .write(batch, Durability::Memory)
            .await
            .expect("weaker request is upgraded");
        storage.graceful_shutdown().await.expect("shutdown");

        let reopened = ProductionRocksDb::open(config(&path, identity("mako-data-plane")))
            .await
            .expect("same-volume restart");
        assert_eq!(
            reopened.adapter().get(b"acknowledged").await.expect("get"),
            Some(b"value".to_vec())
        );
    });
}

#[test]
fn configured_disk_exhaustion_fails_before_database_open() {
    block_on(async {
        let (_directory, path) = empty_volume();
        let owner = identity("mako-data-plane");
        provision_production_volume(&path, owner.clone()).expect("provision");
        let available = fs2::available_space(&path).expect("capacity");
        let mut exhausted = config(&path, owner);
        exhausted.disk_critical_free_bytes = available.saturating_add(1);
        exhausted.disk_warning_free_bytes = available.saturating_add(2);
        let error = ProductionRocksDb::open(exhausted)
            .await
            .expect_err("critical reserve must fail");
        assert_eq!(error.operation, "capacity_check");
        assert!(!has_rocksdb_current(&path));
    });
}

#[test]
fn format_compatible_previous_binary_rollback_uses_the_same_nonempty_volume() {
    block_on(async {
        let (directory, path) = empty_volume();
        let owner = identity("mako-data-plane");
        provision_production_volume(&path, owner.clone()).expect("provision");
        let current = ProductionRocksDb::open(config(&path, owner.clone()))
            .await
            .expect("current binary");
        let mut batch = WriteBatch::new();
        batch.put(b"rollback/current", b"preserved");
        current
            .adapter()
            .write(batch, Durability::Sync)
            .await
            .expect("current write");
        current.graceful_shutdown().await.expect("shutdown");

        let mut previous_config = RocksDbConfig::new(&path);
        previous_config.create_if_missing = false;
        previous_config.minimum_durability = Durability::Sync;
        let previous = RocksDbAdapter::open(previous_config).expect("previous compatible binary");
        assert_eq!(
            previous.get(b"rollback/current").await.expect("read"),
            Some(b"preserved".to_vec())
        );
        let mut batch = WriteBatch::new();
        batch.put(b"rollback/previous", b"also-preserved");
        previous
            .write(batch, Durability::Sync)
            .await
            .expect("previous write");
        drop(previous);

        let resumed = ProductionRocksDb::open(config(&path, owner))
            .await
            .expect("resume current binary");
        assert_eq!(
            resumed
                .adapter()
                .get(b"rollback/previous")
                .await
                .expect("read after rollback"),
            Some(b"also-preserved".to_vec())
        );

        let missing = directory.path().join("rollback-empty");
        let mut no_fallback = RocksDbConfig::new(&missing);
        no_fallback.create_if_missing = false;
        assert!(RocksDbAdapter::open(no_fallback).is_err());
        assert!(!missing.join("CURRENT").exists());
    });
}

#[test]
fn missing_unprovisioned_wrong_owner_and_wrong_format_fail_closed() {
    block_on(async {
        let directory = TempDir::new().expect("temporary root");
        let missing = directory.path().join("missing");
        let error = ProductionRocksDb::open(config(&missing, identity("mako-data-plane")))
            .await
            .expect_err("missing volume must fail");
        assert_eq!(error.operation, "volume_marker");
        assert!(!missing.exists());

        let unprovisioned = directory.path().join("unprovisioned");
        fs::create_dir(&unprovisioned).expect("empty path");
        ProductionRocksDb::open(config(&unprovisioned, identity("mako-data-plane")))
            .await
            .expect_err("empty fallback must fail");
        assert_eq!(
            fs::read_dir(&unprovisioned)
                .expect("read directory")
                .count(),
            0
        );

        let wrong_owner = directory.path().join("wrong-owner");
        fs::create_dir(&wrong_owner).expect("volume");
        provision_production_volume(&wrong_owner, identity("mako-control-plane"))
            .expect("provision");
        let error = ProductionRocksDb::open(config(&wrong_owner, identity("mako-data-plane")))
            .await
            .expect_err("wrong owner must fail");
        assert_eq!(error.kind, StorageErrorKind::Conflict);

        let wrong_format = directory.path().join("wrong-format");
        fs::create_dir(&wrong_format).expect("volume");
        provision_production_volume(&wrong_format, identity("mako-data-plane")).expect("provision");
        let mut marker = read_volume_marker(&wrong_format).expect("marker");
        marker.database_format_version = DATABASE_FORMAT_VERSION + 1;
        fs::write(
            wrong_format.join(MARKER_FILE_NAME),
            serde_json::to_vec(&marker).expect("encode"),
        )
        .expect("modify marker");
        let error = ProductionRocksDb::open(config(&wrong_format, identity("mako-data-plane")))
            .await
            .expect_err("wrong format must fail");
        assert_eq!(error.kind, StorageErrorKind::Unsupported);
    });
}

#[test]
fn lock_contention_and_corrupt_database_fail_closed() {
    block_on(async {
        let (_directory, path) = empty_volume();
        let owner = identity("mako-data-plane");
        provision_production_volume(&path, owner.clone()).expect("provision");
        let first = ProductionRocksDb::open(config(&path, owner.clone()))
            .await
            .expect("first owner");
        let error = ProductionRocksDb::open(config(&path, owner.clone()))
            .await
            .expect_err("second owner must fail");
        assert!(matches!(
            error.kind,
            StorageErrorKind::Io | StorageErrorKind::Conflict
        ));
        drop(first);

        fs::write(path.join("CURRENT"), b"MANIFEST-does-not-exist\n").expect("corrupt CURRENT");
        let error = ProductionRocksDb::open(config(&path, owner))
            .await
            .expect_err("corrupt database must fail");
        assert!(matches!(
            error.kind,
            StorageErrorKind::Corruption
                | StorageErrorKind::Io
                | StorageErrorKind::Unsupported
                | StorageErrorKind::Internal
        ));
    });
}

#[test]
fn same_volume_restart_recovers_sequencers_before_readiness() {
    block_on(async {
        let (_directory, path) = empty_volume();
        let owner = identity("mako-data-plane");
        provision_production_volume(&path, owner.clone()).expect("provision");
        let storage = ProductionRocksDb::open(config(&path, owner.clone()))
            .await
            .expect("first open");
        let keyspace = TenantKeyspace::new("project-a", "production").expect("keyspace");
        let mut batch = WriteBatch::new();
        batch.put(
            keyspace.sequencer_key(SequencerKeyKind::Lease),
            4_u64.to_be_bytes(),
        );
        batch.put(
            keyspace.sequencer_position_key(SequencerKeyKind::Committed, 1),
            b"v1",
        );
        batch.put(
            keyspace.sequencer_position_key(SequencerKeyKind::Aborted, 2),
            b"v1",
        );
        batch.put(
            keyspace.sequencer_position_key(SequencerKeyKind::Committed, 3),
            b"v1",
        );
        storage
            .adapter()
            .write(batch, Durability::Sync)
            .await
            .expect("sequencer state");
        storage.graceful_shutdown().await.expect("shutdown");

        let reopened = ProductionRocksDb::open(config(&path, owner))
            .await
            .expect("restart and recover");
        assert!(reopened.readiness().is_ready());
        assert!(reopened.storage_health().sequencer_recovery_complete);
        assert_eq!(reopened.recovery_report().tenants.len(), 1);
        assert_eq!(
            reopened.recovery_report().tenants[0].acknowledged_high_water,
            3
        );
        assert_eq!(
            reopened
                .adapter()
                .get(&keyspace.sequencer_key(SequencerKeyKind::HighWater))
                .await
                .expect("high water"),
            Some(3_u64.to_be_bytes().to_vec())
        );
    });
}

#[cfg(unix)]
#[test]
fn read_only_provisioned_path_does_not_create_a_database() {
    use std::os::unix::fs::PermissionsExt;

    block_on(async {
        let (_directory, path) = empty_volume();
        let owner = identity("mako-data-plane");
        provision_production_volume(&path, owner.clone()).expect("provision");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).expect("make read-only");
        let result = ProductionRocksDb::open(config(&path, owner)).await;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("restore permissions");
        let error = result.expect_err("read-only volume must fail");
        assert!(matches!(
            error.kind,
            StorageErrorKind::Io | StorageErrorKind::Unavailable | StorageErrorKind::Unsupported
        ));
        assert!(!has_rocksdb_current(&path));
    });
}

fn has_rocksdb_current(path: &Path) -> bool {
    path.join("CURRENT").exists()
}
