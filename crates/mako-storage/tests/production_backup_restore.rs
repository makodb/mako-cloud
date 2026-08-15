use std::{
    fs,
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
    time::Duration,
};

use futures::executor::block_on;
use mako_storage::{
    BackupId, BackupRequest, Durability, FilesystemBackupTransport, KvAdapter, ManifestSigningKey,
    ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity, RestorePolicy,
    SequencerKeyKind, StorageErrorKind, TenantKeyspace, VolumeState, WriteBatch,
    fence_production_volume, inspect_backup, promote_restored_volume, provision_production_volume,
    read_volume_marker, restore_backup,
};
use tempfile::TempDir;

fn identity() -> ProductionVolumeIdentity {
    ProductionVolumeIdentity::new("mako-data-plane", "mako-data-plane-backup-test")
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

fn directories() -> (TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
    let root = TempDir::new().expect("temporary root");
    let database = root.path().join("database");
    let staging = root.path().join("staging");
    let destination = root.path().join("destination");
    let restore = root.path().join("restore");
    for path in [&database, &staging, &destination, &restore] {
        fs::create_dir(path).expect("directory");
    }
    (root, database, staging, destination, restore)
}

async fn seed(storage: &ProductionRocksDb) {
    let first = TenantKeyspace::new("project-a", "production").expect("tenant");
    let second = TenantKeyspace::new("project-b", "production").expect("tenant");
    let mut batch = WriteBatch::new();
    batch.put(
        first.document_key("todos", "one").expect("document key"),
        b"protected-document-value",
    );
    batch.put(
        first.sequencer_key(SequencerKeyKind::Lease),
        3_u64.to_be_bytes(),
    );
    batch.put(
        first.sequencer_position_key(SequencerKeyKind::Committed, 1),
        b"v1",
    );
    batch.put(
        first.sequencer_position_key(SequencerKeyKind::Aborted, 2),
        b"v1",
    );
    batch.put(
        first.sequencer_key(SequencerKeyKind::HighWater),
        2_u64.to_be_bytes(),
    );
    batch.put(
        second.sequencer_key(SequencerKeyKind::Lease),
        2_u64.to_be_bytes(),
    );
    batch.put(
        second.sequencer_position_key(SequencerKeyKind::Committed, 1),
        b"v1",
    );
    batch.put(
        second.sequencer_key(SequencerKeyKind::HighWater),
        1_u64.to_be_bytes(),
    );
    for (key, value) in operator_state_fixture() {
        batch.put(key, value);
    }
    storage
        .adapter()
        .write(batch, Durability::Sync)
        .await
        .expect("seed");
}

#[test]
fn checkpoint_publish_and_empty_target_restore_are_verified() {
    block_on(async {
        let (_root, database, staging, destination, restore) = directories();
        provision_production_volume(&database, identity()).expect("provision");
        let storage = ProductionRocksDb::open(config(&database))
            .await
            .expect("open");
        seed(&storage).await;
        let signing_key = key();
        let artifact = storage
            .create_backup(
                BackupRequest {
                    backup_id: BackupId::new("backup-0001").expect("id"),
                    created_at_unix_seconds: 1_000,
                    staging_root: staging.clone(),
                },
                &signing_key,
            )
            .await
            .expect("checkpoint backup");
        assert_eq!(artifact.envelope.manifest.tenants.len(), 2);
        assert!(!format!("{:?}", artifact.envelope).contains("protected-document-value"));
        assert_eq!(format!("{signing_key:?}"), "ManifestSigningKey([REDACTED])");

        let transport = FilesystemBackupTransport::new(&destination).expect("transport");
        let metrics = transport
            .publish(
                &artifact,
                &signing_key,
                NonZeroU32::new(2).expect("retention"),
                1_005,
            )
            .expect("publish and verify");
        assert!(metrics.verified_after_upload);
        assert_eq!(metrics.retained_backups, 1);

        let published = transport.backup_path(&metrics.backup_id);
        let report = restore_backup(
            &published,
            &restore,
            &RestorePolicy {
                expected_identity: identity(),
                now_unix_seconds: 1_050,
                maximum_backup_age: Duration::from_secs(100),
            },
            &signing_key,
        )
        .await
        .expect("restore");
        assert_eq!(report.state, VolumeState::Promotable);
        assert_eq!(report.tenants_verified, 2);
        assert_eq!(
            report
                .acknowledged_high_waters
                .get(&(b"project-a".to_vec(), b"production".to_vec())),
            Some(&2)
        );
        assert_eq!(
            read_volume_marker(&restore).expect("marker").state,
            VolumeState::Promotable
        );
        let error = ProductionRocksDb::open(config(&restore))
            .await
            .expect_err("restore needs explicit promotion");
        assert_eq!(error.kind, StorageErrorKind::Unavailable);
        promote_restored_volume(&restore, &identity()).expect("explicit promotion");
        let promoted = ProductionRocksDb::open(config(&restore))
            .await
            .expect("promoted restore opens");
        let tenant = TenantKeyspace::new("project-a", "production").expect("tenant");
        assert_eq!(
            promoted
                .adapter()
                .get(&tenant.document_key("todos", "one").expect("key"))
                .await
                .expect("restored get"),
            Some(b"protected-document-value".to_vec())
        );
        for (key, value) in operator_state_fixture() {
            assert_eq!(
                promoted.adapter().get(&key).await.expect("operator state"),
                Some(value),
                "operator entitlement, epoch, session revocation, attempt, idempotency, and migration state must survive checkpoint restore",
            );
        }
        promoted.graceful_shutdown().await.expect("shutdown");
        fence_production_volume(&restore, &identity()).expect("fence");
        assert!(ProductionRocksDb::open(config(&restore)).await.is_err());

        let second = storage
            .create_backup(
                BackupRequest {
                    backup_id: BackupId::new("backup-0002").expect("id"),
                    created_at_unix_seconds: 1_100,
                    staging_root: staging,
                },
                &signing_key,
            )
            .await
            .expect("second backup");
        let second_metrics = transport
            .publish(
                &second,
                &signing_key,
                NonZeroU32::new(1).expect("retention"),
                1_105,
            )
            .expect("publish second backup");
        assert_eq!(second_metrics.retained_backups, 1);
        assert!(!published.exists());
        assert_eq!(transport.telemetry(1_125).successful_backups, 2);
        assert_eq!(
            transport.telemetry(1_125).latest_backup_age_seconds,
            Some(20)
        );
        transport
            .publish(
                &second,
                &signing_key,
                NonZeroU32::new(1).expect("retention"),
                1_130,
            )
            .expect_err("immutable backup cannot be replaced");
        assert_eq!(transport.telemetry(1_130).failed_backups, 1);
    });
}

fn operator_state_fixture() -> Vec<(Vec<u8>, Vec<u8>)> {
    [
        (
            "control/operator-auth/v1/entitlements",
            "dev_restore_operator",
            br#"{"schemaVersion":1,"operatorEpoch":4,"state":"active"}"#.as_slice(),
        ),
        (
            "control/operator-auth/v1/sessions",
            "session-digest",
            br#"{"schemaVersion":1,"operatorEpoch":4,"revokedAtUnixSeconds":1001}"#.as_slice(),
        ),
        (
            "control/operator-auth/v1/attempts",
            "attempt-digest",
            br#"{"schemaVersion":1,"attempts":2,"blockedUntilUnixSeconds":1010}"#.as_slice(),
        ),
        (
            "control/operator-auth/v1/idempotency",
            "idempotency-digest",
            br#"{"schemaVersion":1,"outcome":"committed"}"#.as_slice(),
        ),
        (
            "control/operator-auth/migrations",
            "0000000001",
            br#"{"schemaVersion":1,"completed":true}"#.as_slice(),
        ),
    ]
    .into_iter()
    .map(|(domain, item, value)| {
        (
            TenantKeyspace::system_key(domain, item).expect("operator system key"),
            value.to_vec(),
        )
    })
    .collect()
}

#[test]
fn modified_missing_forged_stale_wrong_owner_and_nonempty_restore_fail() {
    block_on(async {
        let (_root, database, staging, destination, restore) = directories();
        provision_production_volume(&database, identity()).expect("provision");
        let storage = ProductionRocksDb::open(config(&database))
            .await
            .expect("open");
        seed(&storage).await;
        let signing_key = key();
        let artifact = storage
            .create_backup(
                BackupRequest {
                    backup_id: BackupId::new("backup-errors").expect("id"),
                    created_at_unix_seconds: 1_000,
                    staging_root: staging,
                },
                &signing_key,
            )
            .await
            .expect("backup");
        let transport = FilesystemBackupTransport::new(&destination).expect("transport");
        transport
            .publish(
                &artifact,
                &signing_key,
                NonZeroU32::new(2).expect("retention"),
                1_005,
            )
            .expect("publish");
        let published = transport.backup_path(&BackupId::new("backup-errors").expect("id"));

        let stale = restore_backup(
            &published,
            &restore,
            &RestorePolicy {
                expected_identity: identity(),
                now_unix_seconds: 2_000,
                maximum_backup_age: Duration::from_secs(100),
            },
            &signing_key,
        )
        .await
        .expect_err("stale backup");
        assert_eq!(stale.kind, StorageErrorKind::Unavailable);

        let wrong =
            ProductionVolumeIdentity::new("mako-control-plane", "wrong").expect("wrong identity");
        let wrong_owner = restore_backup(
            &published,
            &restore,
            &RestorePolicy {
                expected_identity: wrong,
                now_unix_seconds: 1_010,
                maximum_backup_age: Duration::from_secs(100),
            },
            &signing_key,
        )
        .await
        .expect_err("wrong owner");
        assert_eq!(wrong_owner.kind, StorageErrorKind::Conflict);

        fs::write(restore.join("existing"), b"do-not-overwrite").expect("non-empty target");
        let nonempty = restore_backup(
            &published,
            &restore,
            &RestorePolicy {
                expected_identity: identity(),
                now_unix_seconds: 1_010,
                maximum_backup_age: Duration::from_secs(100),
            },
            &signing_key,
        )
        .await
        .expect_err("non-empty target");
        assert_eq!(nonempty.kind, StorageErrorKind::Conflict);
        fs::remove_file(restore.join("existing")).expect("empty target");

        let interrupted = restore
            .parent()
            .expect("parent")
            .join(".restore.restore-backup-errors.incomplete");
        fs::create_dir(&interrupted).expect("interrupted staging");
        let error = restore_backup(
            &published,
            &restore,
            &RestorePolicy {
                expected_identity: identity(),
                now_unix_seconds: 1_010,
                maximum_backup_age: Duration::from_secs(100),
            },
            &signing_key,
        )
        .await
        .expect_err("interrupted staging requires operator cleanup");
        assert_eq!(error.kind, StorageErrorKind::Conflict);
        assert_eq!(
            fs::read_dir(&restore)
                .expect("target remains empty")
                .count(),
            0
        );
        fs::remove_dir(&interrupted).expect("cleanup staging");

        let envelope_path = published.join("manifest-envelope.json");
        let original_envelope = fs::read(&envelope_path).expect("envelope");
        let mut envelope: serde_json::Value =
            serde_json::from_slice(&original_envelope).expect("JSON");
        envelope["authentication_tag"] = serde_json::Value::String("00".repeat(32));
        fs::write(
            &envelope_path,
            serde_json::to_vec(&envelope).expect("encode"),
        )
        .expect("forge envelope");
        assert!(inspect_backup(&published, &signing_key).is_err());
        fs::write(&envelope_path, original_envelope).expect("restore envelope");

        let first_file = artifact.envelope.manifest.files[0].relative_path.clone();
        let checkpoint_file = published.join("checkpoint").join(first_file);
        let original_file = fs::read(&checkpoint_file).expect("checkpoint file");
        fs::write(&checkpoint_file, b"modified").expect("modify file");
        assert!(inspect_backup(&published, &signing_key).is_err());
        fs::write(&checkpoint_file, &original_file).expect("restore file");
        fs::remove_file(&checkpoint_file).expect("remove file");
        assert!(inspect_backup(&published, &signing_key).is_err());
    });
}

#[test]
fn malformed_tenant_keys_and_missing_acknowledged_positions_block_backup() {
    block_on(async {
        let root = TempDir::new().expect("root");
        for (name, batch) in [
            ("malformed", malformed_tenant_batch()),
            ("missing-position", missing_position_batch()),
        ] {
            let database = root.path().join(format!("{name}-database"));
            let staging = root.path().join(format!("{name}-staging"));
            fs::create_dir(&database).expect("database");
            fs::create_dir(&staging).expect("staging");
            provision_production_volume(&database, identity()).expect("provision");
            let storage = ProductionRocksDb::open(config(&database))
                .await
                .expect("open");
            storage
                .adapter()
                .write(batch, Durability::Sync)
                .await
                .expect("seed invalid state");
            let error = storage
                .create_backup(
                    BackupRequest {
                        backup_id: BackupId::new(format!("backup-{name}")).expect("id"),
                        created_at_unix_seconds: 1_000,
                        staging_root: staging,
                    },
                    &key(),
                )
                .await
                .expect_err("invalid backup must fail");
            assert_eq!(error.kind, StorageErrorKind::Corruption);
        }
    });
}

fn malformed_tenant_batch() -> WriteBatch {
    let mut batch = WriteBatch::new();
    batch.put(vec![1, 0x20, b'p'], b"invalid");
    batch
}

fn missing_position_batch() -> WriteBatch {
    let keyspace = TenantKeyspace::new("project-a", "production").expect("tenant");
    let mut batch = WriteBatch::new();
    batch.put(
        keyspace.sequencer_key(SequencerKeyKind::Lease),
        3_u64.to_be_bytes(),
    );
    batch.put(
        keyspace.sequencer_position_key(SequencerKeyKind::Committed, 1),
        b"v1",
    );
    batch.put(
        keyspace.sequencer_key(SequencerKeyKind::HighWater),
        2_u64.to_be_bytes(),
    );
    batch
}
