use std::{
    fs,
    num::NonZeroU32,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::executor::block_on;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, ControlBackupSigningKey, Durability, KeyCondition,
    KvAdapter, ScanDirection, ScanRequest, SqliteAdapter, SqliteConfig, TenantKeyspace, WriteBatch,
    create_control_sqlite_backup, restore_control_sqlite_backup,
};
use serde_json::json;

const RELEASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!("mako-control-sqlite-benchmark-{nonce}"));
    fs::create_dir_all(&root)?;
    let database = root.join("live/control.sqlite3");
    let identity = "mako-control-benchmark";
    let adapter = Arc::new(open(&database, identity)?);
    let domains = [
        "control/authentication-identities/v1",
        "control/developer-roles/v1",
        "control/operator-auth/v1/entitlements",
        "control/organizations",
        "control/projects",
        "control/audit",
    ];

    let mut write_micros = Vec::with_capacity(1_200);
    for index in 0..1_200_u64 {
        let key = TenantKeyspace::system_key(
            domains[index as usize % domains.len()],
            format!("record-{index:08}"),
        )?;
        let mut batch = WriteBatch::new();
        batch.put(
            key,
            format!("{{\"version\":1,\"sequence\":{index}}}").as_bytes(),
        );
        let started = Instant::now();
        adapter.write(batch, Durability::Sync).await?;
        write_micros.push(started.elapsed().as_micros() as u64);
    }

    let range = TenantKeyspace::system_range()?;
    let scan_started = Instant::now();
    let scanned = adapter
        .scan(ScanRequest::new(
            range,
            ScanDirection::Forward,
            std::num::NonZeroUsize::new(2_000).expect("non-zero constant"),
        ))
        .await?;
    let scan_micros = scan_started.elapsed().as_micros() as u64;

    let contention_key = TenantKeyspace::system_key("control/idempotency", "contended")?;
    let mut seed = WriteBatch::new();
    seed.put(&contention_key, b"pending");
    adapter.write(seed, Durability::Sync).await?;
    let contention_started = Instant::now();
    let mut workers = Vec::new();
    for candidate in 0..32_u8 {
        let adapter = Arc::clone(&adapter);
        let key = contention_key.clone();
        workers.push(std::thread::spawn(move || {
            let mut batch = WriteBatch::new();
            batch.put(&key, [candidate]);
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
        .map(|worker| worker.join().expect("contention worker panicked"))
        .collect::<Vec<_>>();
    let contention_micros = contention_started.elapsed().as_micros() as u64;
    let contention_applied = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Ok(CompareAndWriteResult::Applied)))
        .count();
    let contention_conflicts = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Ok(CompareAndWriteResult::Conflict { .. })))
        .count();
    let contention_busy = outcomes.iter().filter(|outcome| outcome.is_err()).count();

    let health = adapter.health_signals()?;
    let checkpoint_started = Instant::now();
    adapter.shutdown()?;
    let checkpoint_micros = checkpoint_started.elapsed().as_micros() as u64;
    let reopened = open(&database, identity)?;
    let backup_started = Instant::now();
    let signing_key = ControlBackupSigningKey::new([42_u8; 32])?;
    let manifest = create_control_sqlite_backup(
        &database,
        identity,
        &root.join("backup-staging"),
        &root.join("backup-publish"),
        "benchmark-backup",
        RELEASE,
        100,
        NonZeroU32::new(2).expect("non-zero constant"),
        &signing_key,
    )?;
    let backup_micros = backup_started.elapsed().as_micros() as u64;
    let restore_started = Instant::now();
    let restored = root.join("restore/control.sqlite3");
    let report = restore_control_sqlite_backup(
        &root.join("backup-publish/benchmark-backup"),
        &restored,
        identity,
        RELEASE,
        110,
        Duration::from_secs(60),
        &signing_key,
    )?;
    let restore_micros = restore_started.elapsed().as_micros() as u64;
    reopened.shutdown()?;

    write_micros.sort_unstable();
    let p50 = percentile(&write_micros, 50);
    let p95 = percentile(&write_micros, 95);
    let p99 = percentile(&write_micros, 99);
    let evidence = json!({
        "schemaVersion": 1,
        "engine": "sqlite",
        "databaseFormat": 1,
        "workload": {
            "records": scanned.len(),
            "controlDomains": domains.len(),
            "synchronousWrites": write_micros.len(),
            "contentionWorkers": outcomes.len()
        },
        "latencyMicroseconds": {
            "synchronousWriteP50": p50,
            "synchronousWriteP95": p95,
            "synchronousWriteP99": p99,
            "boundedFullControlScan": scan_micros,
            "contentionRound": contention_micros,
            "checkpointAndShutdown": checkpoint_micros,
            "verifiedBackup": backup_micros,
            "verifiedEmptyTargetRestore": restore_micros
        },
        "contention": {
            "applied": contention_applied,
            "conflicts": contention_conflicts,
            "busyOrErrors": contention_busy,
            "adapterBusyTotal": health.busy_failures
        },
        "recovery": {
            "backupRecords": manifest.inventory.record_count,
            "restoredRecords": report.inventory.record_count,
            "inventoryEqual": manifest.inventory == report.inventory,
            "integrityVerified": report.integrity_verified
        },
        "capacity": {
            "databaseBytes": health.database_bytes,
            "walBytesBeforeCheckpoint": health.wal_bytes,
            "availableBytes": health.available_bytes,
            "criticalReserveBytes": health.disk_critical_free_bytes
        }
    });
    println!("{}", serde_json::to_string_pretty(&evidence)?);
    fs::remove_dir_all(&root)?;
    Ok(())
}

fn open(database: &PathBuf, identity: &str) -> Result<SqliteAdapter, mako_storage::StorageError> {
    let mut config = SqliteConfig::new(database, identity);
    config.disk_warning_free_bytes = 2;
    config.disk_critical_free_bytes = 1;
    SqliteAdapter::open(config)
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let index = (sorted.len().saturating_sub(1) * percentile) / 100;
    sorted[index]
}
