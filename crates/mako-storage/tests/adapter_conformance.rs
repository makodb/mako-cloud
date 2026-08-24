use std::{fs, num::NonZeroUsize, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures::executor::block_on;
use mako_storage::{
    ConformanceCheck, ConformanceFactory, Durability, KeyRange, KvAdapter, MemoryAdapter,
    ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity, RocksDbAdapter,
    RocksDbConfig, SqliteAdapter, SqliteConfig, StorageResult, WriteBatch,
    provision_production_volume, run_adapter_conformance,
};
use tempfile::TempDir;

struct MemoryFactory;

#[async_trait]
impl ConformanceFactory for MemoryFactory {
    async fn open(&self) -> StorageResult<Arc<dyn KvAdapter>> {
        Ok(Arc::new(MemoryAdapter::new()))
    }

    async fn restart(&self, adapter: Arc<dyn KvAdapter>) -> StorageResult<Arc<dyn KvAdapter>> {
        drop(adapter);
        self.open().await
    }
}

struct RocksFactory {
    path: PathBuf,
}

struct ProductionRocksFactory {
    config: ProductionRocksDbConfig,
}

struct SqliteFactory {
    path: PathBuf,
}

#[async_trait]
impl ConformanceFactory for ProductionRocksFactory {
    async fn open(&self) -> StorageResult<Arc<dyn KvAdapter>> {
        let storage = ProductionRocksDb::open(self.config.clone()).await?;
        Ok(Arc::new(storage.adapter().clone()))
    }

    async fn restart(&self, adapter: Arc<dyn KvAdapter>) -> StorageResult<Arc<dyn KvAdapter>> {
        drop(adapter);
        self.open().await
    }
}

#[async_trait]
impl ConformanceFactory for RocksFactory {
    async fn open(&self) -> StorageResult<Arc<dyn KvAdapter>> {
        let mut config = RocksDbConfig::new(&self.path);
        config.minimum_durability = Durability::Memory;
        Ok(Arc::new(RocksDbAdapter::open(config)?))
    }

    async fn restart(&self, adapter: Arc<dyn KvAdapter>) -> StorageResult<Arc<dyn KvAdapter>> {
        drop(adapter);
        self.open().await
    }
}

#[async_trait]
impl ConformanceFactory for SqliteFactory {
    async fn open(&self) -> StorageResult<Arc<dyn KvAdapter>> {
        let mut config = SqliteConfig::new(&self.path, "mako-control-conformance");
        config.minimum_durability = Durability::Memory;
        config.disk_warning_free_bytes = 2;
        config.disk_critical_free_bytes = 1;
        Ok(Arc::new(SqliteAdapter::open(config)?))
    }

    async fn restart(&self, adapter: Arc<dyn KvAdapter>) -> StorageResult<Arc<dyn KvAdapter>> {
        drop(adapter);
        self.open().await
    }
}

#[test]
fn memory_adapter_passes_supported_conformance_checks() {
    let report = block_on(run_adapter_conformance(&MemoryFactory)).expect("memory conformance");

    assert!(report.passed(ConformanceCheck::TenantIsolation));
    assert!(report.passed(ConformanceCheck::OrderedScans));
    assert!(report.passed(ConformanceCheck::AtomicBatch));
    assert!(report.passed(ConformanceCheck::StableSnapshot));
    assert!(report.passed(ConformanceCheck::ConditionalConflict));
    assert!(report.passed(ConformanceCheck::ConditionalRace));
    assert!(report.skipped(ConformanceCheck::RestartDurability));
}

#[test]
fn rocksdb_adapter_passes_every_conformance_check() {
    let directory = TempDir::new().expect("temporary directory");
    let factory = RocksFactory {
        path: directory.path().join("rocksdb"),
    };
    let report = block_on(run_adapter_conformance(&factory)).expect("RocksDB conformance");

    for check in [
        ConformanceCheck::TenantIsolation,
        ConformanceCheck::OrderedScans,
        ConformanceCheck::AtomicBatch,
        ConformanceCheck::StableSnapshot,
        ConformanceCheck::ConditionalConflict,
        ConformanceCheck::ConditionalRace,
        ConformanceCheck::RestartDurability,
    ] {
        assert!(report.passed(check), "RocksDB did not pass {check:?}");
    }
}

#[test]
fn sqlite_adapter_passes_every_conformance_check() {
    let directory = TempDir::new().expect("temporary directory");
    let factory = SqliteFactory {
        path: directory.path().join("control.sqlite3"),
    };
    let report = block_on(run_adapter_conformance(&factory)).expect("SQLite conformance");

    for check in [
        ConformanceCheck::TenantIsolation,
        ConformanceCheck::OrderedScans,
        ConformanceCheck::AtomicBatch,
        ConformanceCheck::StableSnapshot,
        ConformanceCheck::ConditionalConflict,
        ConformanceCheck::ConditionalRace,
        ConformanceCheck::RestartDurability,
    ] {
        assert!(report.passed(check), "SQLite did not pass {check:?}");
    }
}

#[test]
fn production_rocksdb_configuration_passes_every_conformance_check() {
    let directory = TempDir::new().expect("temporary directory");
    let path = directory.path().join("production-rocksdb");
    fs::create_dir(&path).expect("volume");
    let identity = ProductionVolumeIdentity::new("mako-data-plane", "conformance-us-east-1")
        .expect("identity");
    provision_production_volume(&path, identity.clone()).expect("provision");
    let factory = ProductionRocksFactory {
        config: ProductionRocksDbConfig {
            database_path: path,
            identity,
            maximum_batch_operations: NonZeroUsize::new(10_000).expect("constant"),
            maximum_scan_items: NonZeroUsize::new(10_000).expect("constant"),
            transaction_lock_timeout: Duration::from_secs(2),
            transaction_expiration: Duration::from_secs(30),
            disk_warning_free_bytes: 2,
            disk_critical_free_bytes: 1,
        },
    };
    let report = block_on(run_adapter_conformance(&factory)).expect("production conformance");

    for check in [
        ConformanceCheck::TenantIsolation,
        ConformanceCheck::OrderedScans,
        ConformanceCheck::AtomicBatch,
        ConformanceCheck::StableSnapshot,
        ConformanceCheck::ConditionalConflict,
        ConformanceCheck::ConditionalRace,
        ConformanceCheck::RestartDurability,
    ] {
        assert!(
            report.passed(check),
            "production RocksDB did not pass {check:?}"
        );
    }
    assert_eq!(
        report.adapter_capabilities.strongest_durability,
        Durability::Sync
    );
}

/// Stored size has to mean the same thing on every engine, because a tenant
/// billed for it must not owe a different amount depending on which one served
/// them. RocksDB's own size estimate does not satisfy that -- it reads table
/// statistics that exclude the memtable and reported zero for the data below --
/// so every adapter sums the range and every adapter must agree.
#[test]
fn every_adapter_scopes_stored_size_to_the_requested_range() {
    block_on(async {
        let memory = TempDir::new().expect("temporary directory");
        let rocks = TempDir::new().expect("temporary directory");
        let sqlite = TempDir::new().expect("temporary directory");
        let factories: Vec<(&str, Box<dyn ConformanceFactory>)> = vec![
            ("memory", Box::new(MemoryFactory)),
            (
                "rocksdb",
                Box::new(RocksFactory {
                    path: rocks.path().join("db"),
                }),
            ),
            (
                "sqlite",
                Box::new(SqliteFactory {
                    path: sqlite.path().join("control.sqlite3"),
                }),
            ),
        ];
        let _ = memory;

        for (name, factory) in factories {
            let adapter = factory.open().await.expect("adapter opens");
            let mut batch = WriteBatch::new();
            for index in 0..64 {
                batch.put(format!("tenant-a/{index:04}").into_bytes(), vec![b'x'; 512]);
            }
            adapter
                .write(batch, Durability::Memory)
                .await
                .expect("write");

            let occupied = adapter
                .stored_bytes(
                    KeyRange::new(b"tenant-a/".to_vec(), b"tenant-a0".to_vec()).expect("range"),
                )
                .await
                .unwrap_or_else(|_| panic!("{name} could not report stored size"));
            let elsewhere = adapter
                .stored_bytes(
                    KeyRange::new(b"tenant-z/".to_vec(), b"tenant-z0".to_vec()).expect("range"),
                )
                .await
                .unwrap_or_else(|_| panic!("{name} could not report stored size"));

            assert_eq!(
                elsewhere, 0,
                "{name} reported stored bytes for a range holding nothing"
            );
            let expected: u64 = (0..64)
                .map(|index| (format!("tenant-a/{index:04}").len() + 512) as u64)
                .sum();
            assert_eq!(
                occupied, expected,
                "{name} disagrees with the other engines about stored size"
            );
        }
    });
}
