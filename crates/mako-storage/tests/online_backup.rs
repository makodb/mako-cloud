//! Online backups: the owner checkpoints itself while serving, and a worker
//! finishes the backup from the linked checkpoint without opening the live
//! database.

use std::{
    fs,
    num::{NonZeroU32, NonZeroUsize},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use futures::executor::block_on;
use mako_storage::{
    BackupId, BackupRequest, Durability, FilesystemBackupTransport, KvAdapter, ManifestSigningKey,
    OnlineCheckpointRequest, ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity,
    RestorePolicy, SequencerKeyKind, StorageErrorKind, TenantKeyspace, WriteBatch,
    finalize_online_backup, online_checkpoint_root, provision_production_volume, restore_backup,
    sweep_online_checkpoints,
};
use tempfile::TempDir;

fn identity() -> ProductionVolumeIdentity {
    ProductionVolumeIdentity::new("mako-data-plane", "mako-data-plane-online-test")
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

fn key() -> ManifestSigningKey {
    ManifestSigningKey::new(b"deterministic-test-signing-material-32-bytes").expect("signing key")
}

struct Layout {
    _root: TempDir,
    database: PathBuf,
    checkpoints: PathBuf,
    staging: PathBuf,
    destination: PathBuf,
    restore: PathBuf,
}

fn layout() -> Layout {
    let root = TempDir::new().expect("temporary root");
    let path = |name: &str| root.path().join(name);
    let layout = Layout {
        database: path("live"),
        checkpoints: online_checkpoint_root(&path("live")),
        staging: path("staging"),
        destination: path("destination"),
        restore: path("restore"),
        _root: root,
    };
    for directory in [
        &layout.database,
        &layout.staging,
        &layout.destination,
        &layout.restore,
    ] {
        fs::create_dir(directory).expect("directory");
    }
    layout
}

fn request(id: &str, drain: Duration) -> OnlineCheckpointRequest {
    OnlineCheckpointRequest {
        backup_id: BackupId::new(id).expect("id"),
        drain_bound: drain,
    }
}

async fn open_seeded(database: &Path) -> ProductionRocksDb {
    provision_production_volume(database, identity()).expect("provision");
    let storage = ProductionRocksDb::open(config(database))
        .await
        .expect("open");
    let tenant = TenantKeyspace::new("project-a", "production").expect("tenant");
    let mut batch = WriteBatch::new();
    batch.put(
        tenant.document_key("todos", "one").expect("document key"),
        b"protected-document-value",
    );
    batch.put(
        tenant.sequencer_key(SequencerKeyKind::Lease),
        2_u64.to_be_bytes(),
    );
    batch.put(
        tenant.sequencer_position_key(SequencerKeyKind::Committed, 1),
        b"v1",
    );
    batch.put(
        tenant.sequencer_key(SequencerKeyKind::HighWater),
        1_u64.to_be_bytes(),
    );
    storage
        .adapter()
        .write(batch, Durability::Sync)
        .await
        .expect("seed");
    storage
}

#[test]
fn online_backup_restores_while_writes_continue() {
    let layout = layout();
    let storage = block_on(open_seeded(&layout.database));
    let adapter = storage.adapter().clone();
    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let writer = {
        let stop = Arc::clone(&stop);
        let written = Arc::clone(&written);
        thread::spawn(move || {
            let tenant = TenantKeyspace::new("project-busy", "production").expect("tenant");
            let mut index = 0_u64;
            while !stop.load(Ordering::Relaxed) {
                let mut batch = WriteBatch::new();
                batch.put(
                    tenant
                        .document_key("events", format!("event-{index}"))
                        .expect("key"),
                    index.to_be_bytes(),
                );
                block_on(adapter.write(batch, Durability::Sync)).expect("write during backup");
                written.fetch_add(1, Ordering::Relaxed);
                index += 1;
            }
        })
    };
    while written.load(Ordering::Relaxed) < 5 {
        thread::yield_now();
    }

    let checkpoint = storage
        .create_online_checkpoint(&request("online-0001", Duration::from_secs(2)))
        .expect("online checkpoint");
    assert!(checkpoint.path.starts_with(&layout.checkpoints));
    let after_checkpoint = written.load(Ordering::Relaxed);
    while written.load(Ordering::Relaxed) < after_checkpoint + 5 {
        thread::yield_now();
    }
    stop.store(true, Ordering::Relaxed);
    writer
        .join()
        .expect("writes never failed during the backup");

    let signing_key = key();
    let artifact = block_on(finalize_online_backup(
        &layout.database,
        &identity(),
        &checkpoint.path,
        BackupRequest {
            backup_id: BackupId::new("online-0001").expect("id"),
            created_at_unix_seconds: 2_000,
            staging_root: layout.staging.clone(),
        },
        &signing_key,
    ))
    .expect("finalize");
    assert!(
        !checkpoint.path.exists(),
        "the linked checkpoint is released once staged"
    );
    let transport = FilesystemBackupTransport::new(&layout.destination).expect("transport");
    let metrics = transport
        .publish(
            &artifact,
            &signing_key,
            NonZeroU32::new(2).expect("retention"),
            2_005,
        )
        .expect("publish");
    let report = block_on(restore_backup(
        &transport.backup_path(&metrics.backup_id),
        &layout.restore,
        &RestorePolicy {
            expected_identity: identity(),
            now_unix_seconds: 2_010,
            maximum_backup_age: Duration::from_secs(100),
        },
        &signing_key,
    ))
    .expect("an online backup restores with the usual proofs");
    assert_eq!(
        report
            .acknowledged_high_waters
            .get(&(b"project-a".to_vec(), b"production".to_vec())),
        Some(&1)
    );
}

#[test]
fn a_checkpoint_waits_for_an_open_commit_group() {
    let layout = layout();
    let storage = block_on(open_seeded(&layout.database));
    let member = storage.adapter().enter_commit_group().expect("enter");
    let leaver = thread::spawn(move || {
        thread::sleep(Duration::from_millis(60));
        drop(member);
    });
    let checkpoint = storage
        .create_online_checkpoint(&request("online-wait", Duration::from_secs(5)))
        .expect("checkpoint after the group leaves");
    leaver.join().expect("leaver");
    assert!(checkpoint.timing.barrier_wait >= Duration::from_millis(40));
    assert_eq!(storage.adapter().commit_group_barrier().open_groups(), 0);
}

#[test]
fn a_group_past_the_bound_abandons_the_checkpoint_without_holding_writes() {
    let layout = layout();
    let storage = block_on(open_seeded(&layout.database));
    let member = storage.adapter().enter_commit_group().expect("enter");
    let error = storage
        .create_online_checkpoint(&request("online-abandoned", Duration::from_millis(30)))
        .expect_err("open group outlasts the bound");
    assert_eq!(error.kind, StorageErrorKind::Unavailable);
    assert!(error.retryable);
    assert_eq!(
        fs::read_dir(&layout.checkpoints).map_or(0, Iterator::count),
        0,
        "an abandoned checkpoint leaves nothing behind"
    );
    let _writer = storage
        .adapter()
        .enter_commit_group()
        .expect("writers enter again at once");
    drop(member);
}

#[test]
fn leftover_checkpoints_are_swept_before_the_next_one() {
    let layout = layout();
    let storage = block_on(open_seeded(&layout.database));
    fs::create_dir(&layout.checkpoints).expect("checkpoint root");
    let leftover = layout.checkpoints.join("interrupted.checkpoint");
    fs::create_dir(&leftover).expect("leftover");
    fs::write(leftover.join("000001.sst"), b"pinned").expect("pinned file");
    let unrelated = layout.checkpoints.join("operator-note.txt");
    fs::write(&unrelated, b"keep").expect("unrelated");

    let checkpoint = storage
        .create_online_checkpoint(&request("online-after-crash", Duration::from_secs(1)))
        .expect("checkpoint");
    assert!(!leftover.exists());
    assert!(unrelated.exists());
    assert!(checkpoint.path.exists());
    assert_eq!(
        sweep_online_checkpoints(&layout.checkpoints).expect("sweep"),
        1
    );
    assert!(!checkpoint.path.exists());
}

#[test]
fn finalize_refuses_a_live_marker_for_another_database() {
    let layout = layout();
    let storage = block_on(open_seeded(&layout.database));
    let checkpoint = storage
        .create_online_checkpoint(&request("online-mismatch", Duration::from_secs(1)))
        .expect("checkpoint");
    let other =
        ProductionVolumeIdentity::new("mako-data-plane", "another-database").expect("identity");
    let error = block_on(finalize_online_backup(
        &layout.database,
        &other,
        &checkpoint.path,
        BackupRequest {
            backup_id: BackupId::new("online-mismatch").expect("id"),
            created_at_unix_seconds: 3_000,
            staging_root: layout.staging.clone(),
        },
        &key(),
    ))
    .expect_err("identity mismatch");
    assert_eq!(error.kind, StorageErrorKind::Conflict);
}

/// The checkpoint root lives inside the live database directory. RocksDB must
/// keep ignoring it -- across writes, flushes, a restart, and an offline
/// checkpoint, which must not carry it into a backup.
#[test]
fn the_database_ignores_its_checkpoint_directory() {
    let layout = layout();
    let storage = block_on(open_seeded(&layout.database));
    let leftover = storage
        .create_online_checkpoint(&request("online-leftover", Duration::from_secs(1)))
        .expect("checkpoint left behind");
    let tenant = TenantKeyspace::new("project-a", "production").expect("tenant");
    for index in 0..200_u32 {
        let mut batch = WriteBatch::new();
        batch.put(
            tenant
                .document_key("todos", format!("after-{index}"))
                .expect("key"),
            index.to_be_bytes(),
        );
        block_on(storage.adapter().write(batch, Durability::Sync)).expect("write");
    }
    block_on(storage.graceful_shutdown()).expect("shutdown");

    let reopened = block_on(ProductionRocksDb::open(config(&layout.database)))
        .expect("reopens with a checkpoint directory inside");
    assert!(leftover.path.join("CURRENT").is_file(), "left untouched");
    assert_eq!(
        block_on(
            reopened
                .adapter()
                .get(&tenant.document_key("todos", "after-199").expect("key"))
        )
        .expect("read"),
        Some(199_u32.to_be_bytes().to_vec())
    );
    let offline = block_on(reopened.create_backup(
        BackupRequest {
            backup_id: BackupId::new("offline-after-online").expect("id"),
            created_at_unix_seconds: 4_000,
            staging_root: layout.staging.clone(),
        },
        &key(),
    ))
    .expect("offline checkpoint");
    assert!(
        offline
            .envelope
            .manifest
            .files
            .iter()
            .all(|file| !file.relative_path.contains(".online-checkpoints")),
        "a checkpoint never copies the checkpoint directory"
    );
    reopened
        .create_online_checkpoint(&request("online-next", Duration::from_secs(1)))
        .expect("next checkpoint");
    assert!(!leftover.path.exists(), "swept by the next checkpoint");
}
