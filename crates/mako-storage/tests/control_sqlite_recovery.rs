use std::{io::Write, num::NonZeroU32, path::Path, process::Command, time::Duration};

use futures::executor::block_on;
use mako_storage::{
    CONTROL_MIGRATION_FORMAT_VERSION, ControlBackupSigningKey, ControlMigrationPlan, Durability,
    KvAdapter, RocksDbAdapter, RocksDbConfig, SequencerKeyKind, SqliteAdapter, SqliteConfig,
    StorageErrorKind, TenantKeyspace, WriteBatch, create_control_sqlite_backup,
    inspect_control_sqlite, inspect_control_sqlite_backup, migrate_control_rocks_to_sqlite,
    promote_control_sqlite_restore, restore_control_sqlite_backup, write_control_checkpoint_fence,
};
use tempfile::TempDir;

const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[test]
fn operations_cli_accepts_shell_safe_equals_arguments_and_writes_a_bound_plan() {
    let directory = TempDir::new().expect("temporary directory");
    let output = directory.path().join("plan.json");
    let arguments = [
        format!(
            "--source-checkpoint={}",
            directory.path().join("source").display()
        ),
        format!(
            "--temporary-target={}",
            directory.path().join("temporary.sqlite3").display()
        ),
        format!(
            "--final-target={}",
            directory.path().join("live.sqlite3").display()
        ),
        format!(
            "--lock-path={}",
            directory.path().join("migration.lock").display()
        ),
        format!(
            "--receipt={}",
            directory.path().join("receipt.json").display()
        ),
        "--identity=mako-control-cli-test".to_owned(),
        format!("--release-sha256={DIGEST_A}"),
        format!("--configuration-sha256={DIGEST_B}"),
        format!("--source-sha256={DIGEST_C}"),
        format!("--output={}", output.display()),
    ];
    let status = Command::new(env!("CARGO_BIN_EXE_mako-control-storage-ops"))
        .arg("plan-migration")
        .args(arguments)
        .status()
        .expect("run operations CLI");
    assert!(status.success());
    let plan: ControlMigrationPlan =
        serde_json::from_slice(&std::fs::read(output).expect("read plan")).expect("parse plan");
    assert_eq!(plan.database_identity, "mako-control-cli-test");
    assert_eq!(plan.expected_release_sha256, DIGEST_A);
    assert_eq!(plan.expected_configuration_sha256, DIGEST_B);
    assert_eq!(plan.source_checkpoint_sha256, DIGEST_C);
}

#[test]
fn fenced_rocks_checkpoint_migrates_byte_exactly_and_publishes_once() {
    let directory = TempDir::new().expect("temporary directory");
    let source = directory.path().join("source-rocks");
    let rocks = RocksDbAdapter::open(RocksDbConfig::new(&source)).expect("open source");
    let entries = [
        (
            "control/authentication-identities/v1",
            "dev_01",
            br#"{"passwordHash":"argon2id-preserved","credentialEpoch":7}"#.as_slice(),
        ),
        (
            "control/developer-roles/v1",
            "dev_01",
            br#"{"status":"waitlisted","developerEpoch":3}"#.as_slice(),
        ),
        (
            "control/operator-auth/v1/entitlements",
            "dev_01",
            br#"{"operatorEpoch":4,"permissions":["tenant_read"]}"#.as_slice(),
        ),
        (
            "control/developer-refresh-sessions",
            "session_revoked",
            br#"{"revokedAt":1234}"#.as_slice(),
        ),
        (
            "control/developer-review-decisions",
            "idempotency_01",
            br#"{"outcome":"approved"}"#.as_slice(),
        ),
        (
            "control/developer-mail-outbox",
            "mail_01",
            br#"{"state":"pending"}"#.as_slice(),
        ),
        (
            "control/organizations",
            "org_01",
            br#"{"name":"Migration fixture"}"#.as_slice(),
        ),
        (
            "control/projects",
            "prj_01",
            br#"{"organizationId":"org_01"}"#.as_slice(),
        ),
        (
            "control/environments/prj_01",
            "env_01",
            br#"{"state":"active"}"#.as_slice(),
        ),
        (
            "control/provisioning",
            "workflow_01",
            br#"{"step":"created"}"#.as_slice(),
        ),
        (
            "control/operator-control-center/v1/incidents",
            "incident_01",
            br#"{"status":"open"}"#.as_slice(),
        ),
        (
            "control/operator-control-center/v1/activity",
            "activity_01",
            br#"{"action":"review"}"#.as_slice(),
        ),
        (
            "control/functions/prj_01/env_01",
            "function_01",
            br#"{"activeVersion":"v1"}"#.as_slice(),
        ),
        (
            "control/audit",
            "event_01",
            br#"{"sequence":42}"#.as_slice(),
        ),
    ];
    let mut batch = WriteBatch::new();
    for (domain, item, value) in entries {
        batch.put(
            TenantKeyspace::system_key(domain, item).expect("control key"),
            value,
        );
    }
    let tenant = TenantKeyspace::new("prj_control_fixture", "env_control_fixture")
        .expect("control tenant keyspace");
    let tenant_entries = [
        (
            tenant
                .collection_key("control_collection")
                .expect("collection metadata key"),
            br#"{"schemaVersion":1}"#.as_slice(),
        ),
        (
            tenant
                .internal_rpc_response_key("control-plane", "request_01")
                .expect("internal response key"),
            br#"{"status":"committed"}"#.as_slice(),
        ),
        (
            tenant.sequencer_key(SequencerKeyKind::HighWater),
            b"\0\0\0\0\0\0\0\x2a".as_slice(),
        ),
    ];
    for (key, value) in &tenant_entries {
        batch.put(key, *value);
    }
    block_on(rocks.write(batch, Durability::Sync)).expect("seed source");
    drop(rocks);
    write_control_checkpoint_fence(&source, DIGEST_C).expect("fence checkpoint");

    let plan = ControlMigrationPlan {
        format_version: CONTROL_MIGRATION_FORMAT_VERSION,
        source_checkpoint: source,
        temporary_target: directory
            .path()
            .join("migration/control.incomplete.sqlite3"),
        final_target: directory.path().join("live/control.sqlite3"),
        lock_path: directory.path().join("lock/migration.lock"),
        receipt_path: directory.path().join("migration/receipt.json"),
        database_identity: "mako-control-migration-test".into(),
        expected_release_sha256: DIGEST_A.into(),
        expected_configuration_sha256: DIGEST_B.into(),
        source_checkpoint_sha256: DIGEST_C.into(),
    };
    std::fs::create_dir_all(plan.temporary_target.parent().expect("temporary parent"))
        .expect("temporary parent");
    std::fs::create_dir_all(plan.final_target.parent().expect("final parent"))
        .expect("final parent");
    let receipt = migrate_control_rocks_to_sqlite(&plan).expect("migrate");
    assert!(receipt.integrity_verified);
    assert!(receipt.byte_exact_verified);
    assert_eq!(receipt.source, receipt.target);
    assert_eq!(
        receipt.source.record_count,
        (entries.len() + tenant_entries.len()) as u64
    );
    assert!(receipt.source.prefix_counts.len() >= 8);
    assert_eq!(
        receipt.source.prefix_counts.get("tenant:internal-rpc"),
        Some(&1)
    );
    assert!(receipt.control_domain_verified);
    assert!(receipt.adapter_conformance_verified);
    assert!(plan.final_target.is_file());
    assert!(!plan.temporary_target.exists());
    assert_eq!(
        inspect_control_sqlite(&plan.final_target, &plan.database_identity)
            .expect("inspect target"),
        receipt.target
    );
    let repeated =
        migrate_control_rocks_to_sqlite(&plan).expect("matching migration is idempotent");
    assert_eq!(repeated, receipt);
}

#[test]
fn unfenced_or_empty_source_is_never_selected() {
    let directory = TempDir::new().expect("temporary directory");
    let source = directory.path().join("source-rocks");
    drop(RocksDbAdapter::open(RocksDbConfig::new(&source)).expect("empty source"));
    let plan = ControlMigrationPlan {
        format_version: CONTROL_MIGRATION_FORMAT_VERSION,
        source_checkpoint: source.clone(),
        temporary_target: directory.path().join("temporary.sqlite3"),
        final_target: directory.path().join("live.sqlite3"),
        lock_path: directory.path().join("migration.lock"),
        receipt_path: directory.path().join("receipt.json"),
        database_identity: "mako-control-empty-test".into(),
        expected_release_sha256: DIGEST_A.into(),
        expected_configuration_sha256: DIGEST_B.into(),
        source_checkpoint_sha256: DIGEST_C.into(),
    };
    let error = migrate_control_rocks_to_sqlite(&plan).expect_err("unfenced source");
    assert_eq!(error.kind, StorageErrorKind::Conflict);
    write_control_checkpoint_fence(&source, DIGEST_C).expect("fence empty source");
    let error = migrate_control_rocks_to_sqlite(&plan).expect_err("empty source");
    assert_eq!(error.kind, StorageErrorKind::Corruption);
    assert!(!plan.final_target.exists());
}

#[test]
fn migration_refuses_non_control_keys_incomplete_targets_and_receipt_drift() {
    let directory = TempDir::new().expect("temporary directory");
    let source = directory.path().join("source-rocks");
    let rocks = RocksDbAdapter::open(RocksDbConfig::new(&source)).expect("open source");
    let mut batch = WriteBatch::new();
    batch.put(b"not-a-control-key", b"must-not-migrate");
    block_on(rocks.write(batch, Durability::Sync)).expect("seed invalid source");
    drop(rocks);
    write_control_checkpoint_fence(&source, DIGEST_C).expect("fence checkpoint");
    let mut plan = ControlMigrationPlan {
        format_version: CONTROL_MIGRATION_FORMAT_VERSION,
        source_checkpoint: source,
        temporary_target: directory.path().join("migration/temporary.sqlite3"),
        final_target: directory.path().join("live/control.sqlite3"),
        lock_path: directory.path().join("lock/migration.lock"),
        receipt_path: directory.path().join("migration/receipt.json"),
        database_identity: "mako-control-negative-migration".into(),
        expected_release_sha256: DIGEST_A.into(),
        expected_configuration_sha256: DIGEST_B.into(),
        source_checkpoint_sha256: DIGEST_C.into(),
    };
    let error = migrate_control_rocks_to_sqlite(&plan).expect_err("non-control source key");
    assert_eq!(error.kind, StorageErrorKind::Corruption);
    assert!(!plan.final_target.exists());

    let source = directory.path().join("valid-source-rocks");
    let rocks = RocksDbAdapter::open(RocksDbConfig::new(&source)).expect("open valid source");
    let mut batch = WriteBatch::new();
    batch.put(
        TenantKeyspace::system_key("control/projects", "prj_01").expect("control key"),
        br#"{"state":"active"}"#,
    );
    block_on(rocks.write(batch, Durability::Sync)).expect("seed valid source");
    drop(rocks);
    write_control_checkpoint_fence(&source, DIGEST_C).expect("fence valid checkpoint");
    plan.source_checkpoint = source;
    plan.temporary_target = directory.path().join("migration/temporary-valid.sqlite3");
    plan.lock_path = directory.path().join("lock/migration-valid.lock");
    std::fs::create_dir_all(plan.temporary_target.parent().expect("temporary parent"))
        .expect("temporary parent");
    std::fs::create_dir_all(plan.final_target.parent().expect("final parent"))
        .expect("final parent");
    std::fs::write(&plan.temporary_target, b"interrupted").expect("incomplete target");
    let error = migrate_control_rocks_to_sqlite(&plan).expect_err("incomplete target");
    assert_eq!(error.kind, StorageErrorKind::Conflict);
    std::fs::remove_file(&plan.temporary_target).expect("remove incomplete target");

    migrate_control_rocks_to_sqlite(&plan).expect("complete migration");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&plan.receipt_path).expect("read receipt"))
            .expect("parse receipt");
    receipt["expected_release_sha256"] = serde_json::Value::String(DIGEST_B.into());
    std::fs::write(
        &plan.receipt_path,
        serde_json::to_vec_pretty(&receipt).expect("encode tampered receipt"),
    )
    .expect("tamper receipt");
    let error = migrate_control_rocks_to_sqlite(&plan).expect_err("receipt drift");
    assert_eq!(error.kind, StorageErrorKind::Conflict);
}

#[test]
fn online_backup_restore_and_explicit_promotion_preserve_control_state() {
    block_on(async {
        let directory = TempDir::new().expect("temporary directory");
        let live = directory.path().join("live/control.sqlite3");
        let identity = "mako-control-backup-test";
        let adapter = open_sqlite(&live, identity);
        let mut batch = WriteBatch::new();
        let recovery_fixture = [
            (
                "control/authentication-identities/v1",
                "pending",
                br#"{"state":"security-active"}"#.as_slice(),
            ),
            (
                "control/developer-roles/v1",
                "pending",
                br#"{"state":"waitlisted"}"#.as_slice(),
            ),
            (
                "control/authentication-identities/v1",
                "active",
                br#"{"state":"security-active"}"#.as_slice(),
            ),
            (
                "control/developer-roles/v1",
                "active",
                br#"{"state":"active"}"#.as_slice(),
            ),
            (
                "control/operator-auth/v1/entitlements",
                "active",
                br#"{"epoch":9}"#.as_slice(),
            ),
            (
                "control/operator-auth/v1/sessions",
                "revoked",
                br#"{"revoked":true}"#.as_slice(),
            ),
            (
                "control/organizations",
                "org_01",
                br#"{"state":"active"}"#.as_slice(),
            ),
            (
                "control/projects",
                "prj_01",
                br#"{"state":"active"}"#.as_slice(),
            ),
            (
                "control/provisioning",
                "workflow_01",
                br#"{"state":"running"}"#.as_slice(),
            ),
            (
                "control/audit",
                "00000000000000000042",
                br#"{"sequence":42}"#.as_slice(),
            ),
            (
                "control/developer-mail-outbox",
                "mail_01",
                br#"{"state":"pending"}"#.as_slice(),
            ),
        ];
        for (domain, item, value) in recovery_fixture {
            batch.put(
                TenantKeyspace::system_key(domain, item).expect("control key"),
                value,
            );
        }
        adapter
            .write(batch, Durability::Sync)
            .await
            .expect("seed control");

        let key = ControlBackupSigningKey::new([7_u8; 32]).expect("signing key");
        let manifest = create_control_sqlite_backup(
            &live,
            identity,
            &directory.path().join("staging"),
            &directory.path().join("published"),
            "backup-0001",
            DIGEST_A,
            100,
            NonZeroU32::new(3).expect("non-zero"),
            &key,
        )
        .expect("create backup");
        assert!(manifest.integrity_verified);
        assert_eq!(
            manifest.inventory.record_count,
            recovery_fixture.len() as u64
        );
        let artifact = directory.path().join("published/backup-0001");
        assert_eq!(
            inspect_control_sqlite_backup(&artifact, &key)
                .expect("inspect backup")
                .inventory,
            manifest.inventory
        );

        let restored = directory.path().join("restore/control.sqlite3");
        let report = restore_control_sqlite_backup(
            &artifact,
            &restored,
            identity,
            DIGEST_A,
            120,
            Duration::from_secs(60),
            &key,
        )
        .expect("restore backup");
        assert!(report.promotable);
        assert_eq!(report.inventory, manifest.inventory);
        let promoted = directory.path().join("promoted/control.sqlite3");
        let inventory = promote_control_sqlite_restore(&restored, &promoted, identity)
            .expect("promote restore");
        assert_eq!(inventory, manifest.inventory);
        assert!(!restored.exists());
        assert!(promoted.exists());
        let recovered = open_sqlite(&promoted, identity);
        for (domain, item, value) in recovery_fixture {
            let key = TenantKeyspace::system_key(domain, item).expect("control key");
            assert_eq!(
                recovered.get(&key).await.expect("read recovered record"),
                Some(value.to_vec())
            );
        }
        recovered.shutdown().expect("shutdown recovered database");
        adapter.shutdown().expect("shutdown live");
    });
}

#[test]
fn backup_authentication_staleness_and_live_overwrite_fail_closed() {
    let directory = TempDir::new().expect("temporary directory");
    let live = directory.path().join("live/control.sqlite3");
    let identity = "mako-control-negative-backup-test";
    let adapter = open_sqlite(&live, identity);
    let key = ControlBackupSigningKey::new([8_u8; 32]).expect("key");
    create_control_sqlite_backup(
        &live,
        identity,
        &directory.path().join("staging"),
        &directory.path().join("published"),
        "backup-0001",
        DIGEST_A,
        100,
        NonZeroU32::new(3).expect("non-zero"),
        &key,
    )
    .expect("backup");
    let artifact = directory.path().join("published/backup-0001");
    let wrong_key = ControlBackupSigningKey::new([9_u8; 32]).expect("wrong key");
    let error = inspect_control_sqlite_backup(&artifact, &wrong_key).expect_err("wrong key");
    assert_eq!(error.kind, StorageErrorKind::Corruption);

    let error = restore_control_sqlite_backup(
        &artifact,
        &directory.path().join("restore/wrong-identity.sqlite3"),
        "another-control-authority",
        DIGEST_A,
        120,
        Duration::from_secs(60),
        &key,
    )
    .expect_err("wrong identity");
    assert_eq!(error.kind, StorageErrorKind::Conflict);
    let error = restore_control_sqlite_backup(
        &artifact,
        &directory.path().join("restore/wrong-release.sqlite3"),
        identity,
        DIGEST_B,
        120,
        Duration::from_secs(60),
        &key,
    )
    .expect_err("wrong release");
    assert_eq!(error.kind, StorageErrorKind::Conflict);

    let stale_target = directory.path().join("restore/stale.sqlite3");
    let error = restore_control_sqlite_backup(
        &artifact,
        &stale_target,
        identity,
        DIGEST_A,
        1_000,
        Duration::from_secs(60),
        &key,
    )
    .expect_err("stale backup");
    assert_eq!(error.kind, StorageErrorKind::Unavailable);

    let restored = directory.path().join("restore/good.sqlite3");
    restore_control_sqlite_backup(
        &artifact,
        &restored,
        identity,
        DIGEST_A,
        120,
        Duration::from_secs(60),
        &key,
    )
    .expect("restore");
    let existing_live = directory.path().join("existing/control.sqlite3");
    std::fs::create_dir_all(existing_live.parent().expect("parent")).expect("parent");
    std::fs::write(&existing_live, b"do-not-overwrite").expect("existing live");
    let error = promote_control_sqlite_restore(&restored, &existing_live, identity)
        .expect_err("live overwrite");
    assert_eq!(error.kind, StorageErrorKind::Conflict);
    assert_eq!(
        std::fs::read(&existing_live).expect("existing live"),
        b"do-not-overwrite"
    );
    let error = promote_control_sqlite_restore(&restored, &existing_live, identity)
        .expect_err("repeated promotion remains refused");
    assert_eq!(error.kind, StorageErrorKind::Conflict);

    let incomplete = directory.path().join("published/incomplete");
    std::fs::create_dir_all(&incomplete).expect("incomplete artifact");
    let error = inspect_control_sqlite_backup(&incomplete, &key).expect_err("missing manifest");
    assert_eq!(error.kind, StorageErrorKind::Corruption);

    let corrupt_artifact = directory.path().join("published/corrupt");
    copy_directory(&artifact, &corrupt_artifact);
    std::fs::OpenOptions::new()
        .append(true)
        .open(corrupt_artifact.join("control.sqlite3"))
        .expect("open backup database")
        .write_all(b"corruption")
        .expect("corrupt backup database");
    let error = inspect_control_sqlite_backup(&corrupt_artifact, &key)
        .expect_err("corrupt backup database");
    assert_eq!(error.kind, StorageErrorKind::Corruption);

    let interrupted_target = directory.path().join("restore/interrupted.sqlite3");
    let interrupted_staging = interrupted_target
        .parent()
        .expect("restore parent")
        .join(".interrupted.sqlite3.restore.incomplete");
    std::fs::write(&interrupted_staging, b"partial restore").expect("interrupted restore");
    let error = restore_control_sqlite_backup(
        &artifact,
        &interrupted_target,
        identity,
        DIGEST_A,
        120,
        Duration::from_secs(60),
        &key,
    )
    .expect_err("interrupted restore staging");
    assert_eq!(error.kind, StorageErrorKind::Conflict);
    adapter.shutdown().expect("shutdown");
}

fn copy_directory(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("copy target");
    for entry in std::fs::read_dir(source).expect("list source") {
        let entry = entry.expect("source entry");
        std::fs::copy(entry.path(), target.join(entry.file_name())).expect("copy artifact file");
    }
}

fn open_sqlite(path: &Path, identity: &str) -> SqliteAdapter {
    let mut config = SqliteConfig::new(path, identity);
    config.disk_warning_free_bytes = 2;
    config.disk_critical_free_bytes = 1;
    SqliteAdapter::open(config).expect("open SQLite")
}
