use std::{collections::BTreeMap, process::Command};

use futures::executor::block_on;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, RocksDbAdapter,
    RocksDbConfig, SqliteAdapter, SqliteConfig, StorageErrorKind, WriteBatch,
};
use proptest::prelude::*;
use tempfile::TempDir;

#[derive(Clone, Debug)]
enum TraceOperation {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
    Compare(Vec<u8>, Option<Vec<u8>>, Vec<u8>),
}

proptest! {
    #[test]
    fn rocksdb_and_sqlite_have_identical_control_trace_outcomes(
        raw in prop::collection::vec(
            (
                0_u8..3,
                prop::collection::vec(any::<u8>(), 1..24),
                prop::collection::vec(any::<u8>(), 0..48),
                prop::option::of(prop::collection::vec(any::<u8>(), 0..48)),
            ),
            1..100,
        )
    ) {
        let trace = raw.into_iter().map(|(kind, key, value, expected)| match kind {
            0 => TraceOperation::Put(key, value),
            1 => TraceOperation::Delete(key),
            _ => TraceOperation::Compare(key, expected, value),
        }).collect::<Vec<_>>();
        let directory = TempDir::new().expect("temporary directory");
        let rocks = RocksDbAdapter::open(RocksDbConfig::new(directory.path().join("rocks")))
            .expect("RocksDB");
        let mut sqlite_config = SqliteConfig::new(
            directory.path().join("control.sqlite3"),
            "mako-control-differential",
        );
        sqlite_config.disk_warning_free_bytes = 2;
        sqlite_config.disk_critical_free_bytes = 1;
        let sqlite = SqliteAdapter::open(sqlite_config).expect("SQLite");
        let mut known_keys = BTreeMap::new();

        for operation in trace {
            match operation {
                TraceOperation::Put(key, value) => {
                    let mut batch = WriteBatch::new();
                    batch.put(&key, &value);
                    block_on(rocks.write(batch.clone(), Durability::Sync)).expect("RocksDB put");
                    block_on(sqlite.write(batch, Durability::Sync)).expect("SQLite put");
                    known_keys.insert(key, ());
                }
                TraceOperation::Delete(key) => {
                    let mut batch = WriteBatch::new();
                    batch.delete(&key);
                    block_on(rocks.write(batch.clone(), Durability::Sync)).expect("RocksDB delete");
                    block_on(sqlite.write(batch, Durability::Sync)).expect("SQLite delete");
                    known_keys.insert(key, ());
                }
                TraceOperation::Compare(key, expected, value) => {
                    let condition = match expected {
                        Some(expected) => KeyCondition::ValueEquals { key: key.clone(), value: expected },
                        None => KeyCondition::Missing { key: key.clone() },
                    };
                    let mut batch = WriteBatch::new();
                    batch.put(&key, &value);
                    let request = AtomicWrite {
                        conditions: vec![condition],
                        batch,
                        durability: Durability::Sync,
                    };
                    let rocks_outcome = block_on(rocks.compare_and_write(request.clone())).expect("RocksDB compare");
                    let sqlite_outcome = block_on(sqlite.compare_and_write(request)).expect("SQLite compare");
                    prop_assert_eq!(normalize(&rocks_outcome), normalize(&sqlite_outcome));
                    known_keys.insert(key, ());
                }
            }
        }
        for key in known_keys.keys() {
            prop_assert_eq!(
                block_on(rocks.get(key)).expect("RocksDB get"),
                block_on(sqlite.get(key)).expect("SQLite get"),
            );
        }
    }
}

#[test]
fn acknowledged_write_survives_abrupt_process_exit_and_wal_recovery() {
    let directory = TempDir::new().expect("temporary directory");
    let database = directory.path().join("control.sqlite3");
    let identity = "mako-control-crash-test";
    let status = Command::new(env!("CARGO_BIN_EXE_mako-sqlite-crash-helper"))
        .arg(&database)
        .arg(identity)
        .status()
        .expect("run crash helper");
    assert!(status.success());

    let mut config = SqliteConfig::new(&database, identity);
    config.disk_warning_free_bytes = 2;
    config.disk_critical_free_bytes = 1;
    let adapter = SqliteAdapter::open(config).expect("recover after abrupt exit");
    assert_eq!(
        block_on(adapter.get(b"crash/acknowledged")).expect("read acknowledged write"),
        Some(b"durable".to_vec())
    );
    adapter.shutdown().expect("shutdown recovered database");
}

#[test]
fn truncated_database_is_classified_without_vendor_details() {
    let directory = TempDir::new().expect("temporary directory");
    let database = directory.path().join("control.sqlite3");
    let identity = "mako-control-corrupt-test";
    let mut config = SqliteConfig::new(&database, identity);
    config.disk_warning_free_bytes = 2;
    config.disk_critical_free_bytes = 1;
    SqliteAdapter::open(config.clone())
        .expect("open SQLite")
        .shutdown()
        .expect("shutdown");
    std::fs::write(&database, b"truncated-not-a-database").expect("truncate database");
    let error = SqliteAdapter::open(config).expect_err("corruption must fail");
    assert!(matches!(
        error.kind,
        StorageErrorKind::Corruption | StorageErrorKind::Internal
    ));
    assert!(!error.message.contains("truncated-not-a-database"));
    assert!(!error.message.contains(database.to_string_lossy().as_ref()));
}

fn normalize(outcome: &CompareAndWriteResult) -> (bool, Option<usize>, Option<Vec<u8>>) {
    match outcome {
        CompareAndWriteResult::Applied => (true, None, None),
        CompareAndWriteResult::Conflict {
            failed_condition,
            actual_value,
        } => (false, Some(*failed_condition), actual_value.clone()),
    }
}
