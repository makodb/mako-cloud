use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use futures::executor::block_on;
use mako_storage::{
    AtomicWrite, Capability, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter,
    ScanDirection, ScanRequest, SqliteAdapter, SqliteConfig, StorageErrorKind, TransactionMode,
    WriteBatch,
};
use proptest::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn config(directory: &TempDir) -> SqliteConfig {
    let mut config = SqliteConfig::new(
        directory.path().join("control.sqlite3"),
        "mako-control-test",
    );
    config.disk_warning_free_bytes = 2;
    config.disk_critical_free_bytes = 1;
    config
}

#[test]
fn serializable_transactions_commit_and_rollback() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let adapter = SqliteAdapter::open(config(&directory)).expect("open SQLite");
        assert!(
            adapter
                .capabilities()
                .supports(Capability::SerializableTransactions)
        );

        let mut committed = adapter
            .begin_transaction(TransactionMode::Serializable)
            .await
            .expect("begin transaction");
        assert_eq!(
            committed
                .get_for_update(b"control/identity")
                .await
                .expect("guarded read"),
            None
        );
        committed
            .put(b"control/identity".to_vec(), b"active".to_vec())
            .expect("transaction put");
        committed
            .commit(Durability::Sync)
            .await
            .expect("commit transaction");
        assert_eq!(
            adapter.get(b"control/identity").await.expect("read"),
            Some(b"active".to_vec())
        );

        let mut rolled_back = adapter
            .begin_transaction(TransactionMode::Serializable)
            .await
            .expect("begin transaction");
        rolled_back
            .put(b"control/identity".to_vec(), b"disabled".to_vec())
            .expect("transaction put");
        rolled_back.rollback().await.expect("rollback transaction");
        assert_eq!(
            adapter.get(b"control/identity").await.expect("read"),
            Some(b"active".to_vec())
        );
    });
}

#[test]
fn transaction_lifetime_and_mutation_bounds_fail_closed() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let mut settings = config(&directory);
        settings.transaction_expiration = Duration::from_millis(1);
        settings.maximum_batch_operations = NonZeroUsize::new(1).expect("non-zero");
        let adapter = SqliteAdapter::open(settings).expect("open SQLite");

        let mut bounded = adapter
            .begin_transaction(TransactionMode::Serializable)
            .await
            .expect("begin transaction");
        bounded
            .put(b"a".to_vec(), b"1".to_vec())
            .expect("first put");
        let error = bounded
            .put(b"b".to_vec(), b"2".to_vec())
            .expect_err("mutation limit");
        assert_eq!(error.kind, StorageErrorKind::InvalidRequest);
        bounded.rollback().await.expect("rollback");

        let mut expired = adapter
            .begin_transaction(TransactionMode::Serializable)
            .await
            .expect("begin transaction");
        std::thread::sleep(Duration::from_millis(5));
        let error = expired
            .get_for_update(b"a")
            .await
            .expect_err("expired transaction");
        assert_eq!(error.kind, StorageErrorKind::Timeout);
    });
}

#[test]
fn exclusive_lock_identity_and_shutdown_are_enforced() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let settings = config(&directory);
        let adapter = SqliteAdapter::open(settings.clone()).expect("open SQLite");
        let second = SqliteAdapter::open(settings.clone()).expect_err("exclusive process lock");
        assert_eq!(second.kind, StorageErrorKind::Conflict);

        adapter.shutdown().expect("graceful shutdown");
        let error = adapter.get(b"key").await.expect_err("work after shutdown");
        assert_eq!(error.kind, StorageErrorKind::Unavailable);

        let reopened = SqliteAdapter::open(settings.clone()).expect("lock released");
        reopened.shutdown().expect("shutdown reopened adapter");

        let mut wrong_identity = settings;
        wrong_identity.database_identity = "another-control-authority".into();
        let error = SqliteAdapter::open(wrong_identity).expect_err("identity mismatch");
        assert_eq!(error.kind, StorageErrorKind::Corruption);
        assert!(
            !error
                .message
                .contains(directory.path().to_string_lossy().as_ref())
        );
    });
}

#[test]
fn newer_schema_missing_database_and_capacity_pressure_fail_closed() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let settings = config(&directory);
        let adapter = SqliteAdapter::open(settings.clone()).expect("open SQLite");
        adapter.shutdown().expect("shutdown");
        let connection = Connection::open(&settings.database_path).expect("open raw SQLite");
        connection
            .execute_batch("PRAGMA user_version = 999;")
            .expect("set newer schema");
        drop(connection);
        let error = SqliteAdapter::open(settings).expect_err("newer schema");
        assert_eq!(error.kind, StorageErrorKind::Unsupported);

        let missing_directory = TempDir::new().expect("temporary directory");
        let mut missing = config(&missing_directory);
        missing.create_if_missing = false;
        let error = SqliteAdapter::open(missing).expect_err("missing database");
        assert_eq!(error.kind, StorageErrorKind::Unavailable);

        let pressure_directory = TempDir::new().expect("temporary directory");
        let mut pressure = config(&pressure_directory);
        pressure.disk_warning_free_bytes = u64::MAX;
        pressure.disk_critical_free_bytes = u64::MAX - 1;
        let pressured = SqliteAdapter::open(pressure).expect("open pressured SQLite");
        let mut batch = WriteBatch::new();
        batch.put(b"control", b"value");
        let error = pressured
            .write(batch, Durability::Sync)
            .await
            .expect_err("critical reserve");
        assert_eq!(error.kind, StorageErrorKind::Unavailable);
    });
}

#[test]
fn conditional_outcomes_are_deterministic_and_atomic() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let adapter = Arc::new(SqliteAdapter::open(config(&directory)).expect("open SQLite"));
        let mut seed = WriteBatch::new();
        seed.put(b"waitlist/applicant", b"pending");
        adapter.write(seed, Durability::Sync).await.expect("seed");

        let decide = |decision: &'static [u8]| AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key: b"waitlist/applicant".to_vec(),
                value: b"pending".to_vec(),
            }],
            batch: {
                let mut batch = WriteBatch::new();
                batch.put(b"waitlist/applicant", decision);
                batch.put(b"audit/decision", decision);
                batch
            },
            durability: Durability::Sync,
        };
        let left = Arc::clone(&adapter);
        let right = Arc::clone(&adapter);
        let left_thread =
            std::thread::spawn(move || block_on(left.compare_and_write(decide(b"approved"))));
        let right_thread =
            std::thread::spawn(move || block_on(right.compare_and_write(decide(b"denied"))));
        let outcomes = [
            left_thread
                .join()
                .expect("left thread")
                .expect("left result"),
            right_thread
                .join()
                .expect("right thread")
                .expect("right result"),
        ];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CompareAndWriteResult::Applied))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CompareAndWriteResult::Conflict { .. }))
                .count(),
            1
        );
        let decision = adapter
            .get(b"waitlist/applicant")
            .await
            .expect("read decision")
            .expect("decision exists");
        assert_eq!(
            adapter.get(b"audit/decision").await.expect("read audit"),
            Some(decision)
        );
    });
}

#[test]
fn busy_contention_is_bounded_counted_and_recovers() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let mut settings = config(&directory);
        settings.busy_timeout = Duration::from_millis(10);
        let adapter = SqliteAdapter::open(settings).expect("open SQLite");
        let mut owner = adapter
            .begin_transaction(TransactionMode::Serializable)
            .await
            .expect("begin owner transaction");
        owner
            .put(b"control/contention".to_vec(), b"owner".to_vec())
            .expect("owner mutation");

        let mut blocked = WriteBatch::new();
        blocked.put(b"control/contention", b"contender");
        let error = adapter
            .write(blocked, Durability::Sync)
            .await
            .expect_err("bounded busy timeout");
        assert_eq!(error.kind, StorageErrorKind::Timeout);
        let health = adapter.health_signals().expect("health during transaction");
        assert_eq!(health.active_transactions, 1);
        assert!(health.busy_failures >= 1);

        owner.commit(Durability::Sync).await.expect("commit owner");
        assert_eq!(
            adapter
                .health_signals()
                .expect("health after transaction")
                .active_transactions,
            0
        );
        let mut recovered = WriteBatch::new();
        recovered.put(b"control/contention", b"after");
        adapter
            .write(recovered, Durability::Sync)
            .await
            .expect("write after contention");
    });
}

#[test]
fn concurrent_control_domain_mutations_have_one_serializable_winner() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let adapter = Arc::new(SqliteAdapter::open(config(&directory)).expect("open SQLite"));
        for domain in [
            "wait-list",
            "entitlement",
            "idempotency",
            "provisioning",
            "audit",
            "session",
        ] {
            let key = format!("control/concurrency/{domain}").into_bytes();
            let mut seed = WriteBatch::new();
            seed.put(&key, b"pending");
            adapter.write(seed, Durability::Sync).await.expect("seed");
            let mut workers = Vec::new();
            for winner in 0_u8..12 {
                let adapter = Arc::clone(&adapter);
                let key = key.clone();
                workers.push(std::thread::spawn(move || {
                    let value = vec![winner];
                    let mut batch = WriteBatch::new();
                    batch.put(&key, &value);
                    block_on(adapter.compare_and_write(AtomicWrite {
                        conditions: vec![KeyCondition::ValueEquals {
                            key,
                            value: b"pending".to_vec(),
                        }],
                        batch,
                        durability: Durability::Sync,
                    }))
                }));
            }
            let outcomes = workers
                .into_iter()
                .map(|worker| worker.join().expect("worker did not panic"))
                .collect::<Vec<_>>();
            let applied = outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Ok(CompareAndWriteResult::Applied)))
                .count();
            let conflicts = outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Ok(CompareAndWriteResult::Conflict { .. })))
                .count();
            let busy = outcomes
                .iter()
                .filter(|outcome| {
                    matches!(outcome, Err(error) if error.kind == StorageErrorKind::Timeout)
                })
                .count();
            assert_eq!(applied, 1, "{domain} must have one winner");
            assert_eq!(applied + conflicts + busy, outcomes.len());
            assert!(busy < outcomes.len(), "busy outcomes must be bounded");
        }
    });
}

#[test]
fn overgrown_wal_fails_closed_while_snapshot_blocks_checkpoint() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let mut settings = config(&directory);
        settings.maximum_wal_bytes = 1;
        settings.wal_autocheckpoint_pages = NonZeroUsize::new(1).expect("non-zero");
        let adapter = SqliteAdapter::open(settings).expect("open SQLite");
        let snapshot = adapter.snapshot().await.expect("stable snapshot");

        let mut growth = WriteBatch::new();
        growth.put(b"control/wal-growth", vec![7_u8; 32 * 1024]);
        adapter
            .write(growth, Durability::Sync)
            .await
            .expect("first WAL write");
        let mut guarded = WriteBatch::new();
        guarded.put(b"control/guarded", b"must-not-commit");
        let error = adapter
            .write(guarded, Durability::Sync)
            .await
            .expect_err("WAL limit while snapshot pins pages");
        assert_eq!(error.kind, StorageErrorKind::Unavailable);
        assert_eq!(
            adapter.get(b"control/guarded").await.expect("guarded read"),
            None
        );
        drop(snapshot);
        adapter
            .shutdown()
            .expect("checkpoint after snapshot release");
    });
}

#[cfg(unix)]
#[test]
fn symlinked_and_read_only_database_paths_fail_closed_without_path_disclosure() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = TempDir::new().expect("temporary directory");
    let settings = config(&directory);
    SqliteAdapter::open(settings.clone())
        .expect("create SQLite")
        .shutdown()
        .expect("shutdown");

    let linked = directory.path().join("linked.sqlite3");
    symlink(&settings.database_path, &linked).expect("create symlink");
    let linked_config = SqliteConfig::new(&linked, "mako-control-test");
    let error = SqliteAdapter::open(linked_config).expect_err("symlink rejected");
    assert_eq!(error.kind, StorageErrorKind::InvalidRequest);

    let protected_paths = [&settings.database_path, &settings.lock_path];
    let original_modes = protected_paths.map(|path| {
        std::fs::metadata(path)
            .expect("control file metadata")
            .permissions()
            .mode()
    });
    for path in protected_paths {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))
            .expect("make control file read only");
    }
    let error = SqliteAdapter::open(settings.clone()).expect_err("read-only database rejected");
    assert!(matches!(
        error.kind,
        StorageErrorKind::Io | StorageErrorKind::Internal
    ));
    assert!(
        !error
            .message
            .contains(directory.path().to_string_lossy().as_ref())
    );
    for (path, mode) in protected_paths.into_iter().zip(original_modes) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("restore control file mode");
    }
}

proptest! {
    #[test]
    fn arbitrary_nonempty_byte_keys_round_trip_and_sort(
        mut entries in prop::collection::vec(
            (prop::collection::vec(any::<u8>(), 1..32), prop::collection::vec(any::<u8>(), 0..64)),
            1..64,
        )
    ) {
        let directory = TempDir::new().expect("temporary directory");
        let adapter = SqliteAdapter::open(config(&directory)).expect("open SQLite");
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries.dedup_by(|left, right| left.0 == right.0);
        let mut batch = WriteBatch::new();
        for (key, value) in &entries {
            batch.put(key, value);
        }
        block_on(adapter.write(batch, Durability::Sync)).expect("write arbitrary bytes");

        let range = KeyRange::new(vec![0], vec![255, 255, 255, 255]).expect("bounded range");
        let request = ScanRequest::new(
            range,
            ScanDirection::Forward,
            NonZeroUsize::new(10_000).expect("non-zero"),
        );
        let scanned = block_on(adapter.scan(request)).expect("scan arbitrary bytes");
        let expected = entries
            .iter()
            .filter(|(key, _)| key.as_slice() >= [0].as_slice() && key.as_slice() < [255, 255, 255, 255].as_slice())
            .cloned()
            .collect::<Vec<_>>();
        prop_assert_eq!(
            scanned.into_iter().map(|entry| (entry.key, entry.value)).collect::<Vec<_>>(),
            expected,
        );
    }
}

/// The control database answers stored size by summing the range, because
/// SQLite has no estimate to ask for. What matters is that it is scoped to the
/// range and not the whole database.
#[test]
fn stored_bytes_is_scoped_to_the_requested_range() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let adapter = SqliteAdapter::open(config(&directory)).expect("open SQLite");

        let mut batch = WriteBatch::new();
        batch.put(b"tenant-a/one", b"payload");
        batch.put(b"tenant-b/one", b"much larger payload");
        adapter.write(batch, Durability::Sync).await.expect("write");

        let a = adapter
            .stored_bytes(
                KeyRange::new(b"tenant-a/".to_vec(), b"tenant-a0".to_vec()).expect("range"),
            )
            .await
            .expect("size");
        let b = adapter
            .stored_bytes(
                KeyRange::new(b"tenant-b/".to_vec(), b"tenant-b0".to_vec()).expect("range"),
            )
            .await
            .expect("size");
        let empty = adapter
            .stored_bytes(
                KeyRange::new(b"tenant-c/".to_vec(), b"tenant-c0".to_vec()).expect("range"),
            )
            .await
            .expect("size");

        assert_eq!(a, (b"tenant-a/one".len() + b"payload".len()) as u64);
        assert_eq!(
            b,
            (b"tenant-b/one".len() + b"much larger payload".len()) as u64
        );
        assert_eq!(empty, 0, "a range holding nothing reported stored bytes");
    });
}
