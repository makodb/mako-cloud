use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use futures::executor::block_on;
use mako_storage::{
    KvAdapter, ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity,
    provision_production_volume,
};
use tempfile::TempDir;

fn identity() -> ProductionVolumeIdentity {
    ProductionVolumeIdentity::new("mako-data-plane", "mako-data-plane-crash-test")
        .expect("identity")
}

fn config(path: impl Into<PathBuf>) -> ProductionRocksDbConfig {
    ProductionRocksDbConfig {
        database_path: path.into(),
        identity: identity(),
        maximum_batch_operations: NonZeroUsize::new(10_000).expect("constant"),
        maximum_scan_items: NonZeroUsize::new(10_000).expect("constant"),
        transaction_lock_timeout: Duration::from_millis(100),
        transaction_expiration: Duration::from_secs(5),
        disk_warning_free_bytes: 2,
        disk_critical_free_bytes: 1,
    }
}

fn volume() -> (TempDir, PathBuf) {
    let root = TempDir::new().expect("temporary root");
    let path = root.path().join("volume");
    fs::create_dir(&path).expect("volume");
    provision_production_volume(&path, identity()).expect("provision");
    (root, path)
}

#[test]
fn acknowledged_sync_write_survives_process_exit_without_destructors() {
    let (_root, path) = volume();
    run_helper("ack-crash", &path);
    assert_sentinel(&path);
}

#[test]
fn database_recovers_after_process_exit_during_compaction_pressure() {
    let (_root, path) = volume();
    run_helper("compaction-crash", &path);
    assert_sentinel(&path);
}

fn run_helper(mode: &str, path: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_mako-storage-crash-helper"))
        .arg(mode)
        .arg(path)
        .status()
        .expect("run crash helper");
    assert!(status.success(), "crash helper failed: {status}");
}

fn assert_sentinel(path: &Path) {
    block_on(async {
        let storage = ProductionRocksDb::open(config(path))
            .await
            .expect("recover production database");
        assert!(storage.readiness().is_ready());
        assert_eq!(
            storage
                .adapter()
                .get(b"crash/sentinel")
                .await
                .expect("read sentinel"),
            Some(b"acknowledged".to_vec())
        );
    });
}
