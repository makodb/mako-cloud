use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use futures::executor::block_on;
use rocksdb::{DB, IteratorMode, Options};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter,
    ScanDirection, ScanRequest, SqliteAdapter, SqliteConfig, StorageError, StorageErrorKind,
    StorageResult, TenantKeyspace, TransactionMode, WriteBatch,
};

pub const CONTROL_MIGRATION_FORMAT_VERSION: u32 = 1;
const FENCE_MARKER: &str = ".mako-control-checkpoint-fenced.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KvInventory {
    pub record_count: u64,
    pub framed_blake3: String,
    pub prefix_counts: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlMigrationPlan {
    pub format_version: u32,
    pub source_checkpoint: PathBuf,
    pub temporary_target: PathBuf,
    pub final_target: PathBuf,
    pub lock_path: PathBuf,
    pub receipt_path: PathBuf,
    pub database_identity: String,
    pub expected_release_sha256: String,
    pub expected_configuration_sha256: String,
    pub source_checkpoint_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlMigrationReceipt {
    pub format_version: u32,
    pub database_format_version: u32,
    pub database_identity: String,
    pub expected_release_sha256: String,
    pub expected_configuration_sha256: String,
    pub source_checkpoint_sha256: String,
    pub source: KvInventory,
    pub target: KvInventory,
    pub started_at_unix_seconds: u64,
    pub completed_at_unix_seconds: u64,
    pub integrity_verified: bool,
    pub byte_exact_verified: bool,
    pub control_domain_verified: bool,
    pub adapter_conformance_verified: bool,
    pub receipt_blake3: String,
}

pub fn inspect_control_rocks_checkpoint(path: &Path) -> StorageResult<KvInventory> {
    require_absolute(path, "migration_inspect")?;
    if !path.join(FENCE_MARKER).is_file() {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "migration_inspect",
            false,
            "source checkpoint is not fenced for offline migration",
        ));
    }
    let mut options = Options::default();
    options.create_if_missing(false);
    let database = DB::open_for_read_only(&options, path, false).map_err(|_| {
        StorageError::new(
            StorageErrorKind::Unavailable,
            "migration_inspect",
            false,
            "fenced source checkpoint could not be opened",
        )
    })?;
    let mut accumulator = InventoryAccumulator::default();
    for row in database.iterator(IteratorMode::Start) {
        let (key, value) = row.map_err(|_| {
            StorageError::new(
                StorageErrorKind::Corruption,
                "migration_inspect",
                false,
                "source checkpoint scan failed",
            )
        })?;
        accumulator.push(&key, &value)?;
    }
    Ok(accumulator.finish())
}

pub fn inspect_control_sqlite(path: &Path, expected_identity: &str) -> StorageResult<KvInventory> {
    require_absolute(path, "sqlite_inspect")?;
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| {
        unavailable(
            "sqlite_inspect",
            "control SQLite target could not be opened",
        )
    })?;
    let application_id: i32 = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(|_| corrupt("sqlite_inspect", "control SQLite metadata is invalid"))?;
    let schema: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| corrupt("sqlite_inspect", "control SQLite schema is invalid"))?;
    if application_id != 0x4d41_4b4f || schema != crate::CONTROL_SQLITE_FORMAT_VERSION {
        return Err(StorageError::new(
            StorageErrorKind::Unsupported,
            "sqlite_inspect",
            false,
            "control SQLite identity or schema is unsupported",
        ));
    }
    let identity: Vec<u8> = connection
        .query_row(
            "SELECT value FROM mako_metadata WHERE key='database_identity'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| corrupt("sqlite_inspect", "control SQLite identity is missing"))?;
    if identity != expected_identity.as_bytes() {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "sqlite_inspect",
            false,
            "control SQLite identity does not match the migration plan",
        ));
    }
    let integrity: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|_| corrupt("sqlite_inspect", "control SQLite integrity check failed"))?;
    if integrity != "ok" {
        return Err(corrupt(
            "sqlite_inspect",
            "control SQLite integrity is invalid",
        ));
    }
    let mut statement = connection
        .prepare("SELECT key, value FROM mako_kv ORDER BY key ASC")
        .map_err(|_| corrupt("sqlite_inspect", "control SQLite keyspace is invalid"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|_| corrupt("sqlite_inspect", "control SQLite keyspace scan failed"))?;
    let mut accumulator = InventoryAccumulator::default();
    for row in rows {
        let (key, value) =
            row.map_err(|_| corrupt("sqlite_inspect", "control SQLite keyspace scan failed"))?;
        accumulator.push(&key, &value)?;
    }
    Ok(accumulator.finish())
}

pub fn migrate_control_rocks_to_sqlite(
    plan: &ControlMigrationPlan,
) -> StorageResult<ControlMigrationReceipt> {
    migrate_control_rocks_to_sqlite_with(plan, |_| Ok(()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MigrationStage {
    SourceScan,
    Capacity,
    Copy,
    Checkpoint,
    TargetVerification,
    FileSync,
    AtomicRename,
}

fn migrate_control_rocks_to_sqlite_with(
    plan: &ControlMigrationPlan,
    mut stage_hook: impl FnMut(MigrationStage) -> StorageResult<()>,
) -> StorageResult<ControlMigrationReceipt> {
    validate_plan(plan)?;
    if plan.final_target.exists() {
        return inspect_completed_migration(plan);
    }
    if plan.temporary_target.exists() {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "control_migrate",
            false,
            "incomplete migration target already exists and requires inspection",
        ));
    }
    let started = now()?;
    let source = inspect_control_rocks_checkpoint(&plan.source_checkpoint)?;
    stage_hook(MigrationStage::SourceScan)?;
    let fence: serde_json::Value = serde_json::from_slice(
        &fs::read(plan.source_checkpoint.join(FENCE_MARKER))
            .map_err(|_| corrupt("control_migrate", "source checkpoint fence is unreadable"))?,
    )
    .map_err(|_| corrupt("control_migrate", "source checkpoint fence is invalid"))?;
    if fence
        .get("source_checkpoint_sha256")
        .and_then(serde_json::Value::as_str)
        != Some(plan.source_checkpoint_sha256.as_str())
    {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "control_migrate",
            false,
            "source checkpoint fence does not match the migration plan",
        ));
    }
    if source.record_count == 0 {
        return Err(corrupt(
            "control_migrate",
            "fenced source checkpoint is empty",
        ));
    }

    let mut options = Options::default();
    options.create_if_missing(false);
    let source_db =
        DB::open_for_read_only(&options, &plan.source_checkpoint, false).map_err(|_| {
            unavailable(
                "control_migrate",
                "fenced source checkpoint could not be opened",
            )
        })?;
    let mut sqlite_config =
        SqliteConfig::new(&plan.temporary_target, plan.database_identity.clone());
    sqlite_config.lock_path = plan.lock_path.clone();
    sqlite_config.create_if_missing = true;
    sqlite_config.disk_warning_free_bytes = 2;
    sqlite_config.disk_critical_free_bytes = 1;
    stage_hook(MigrationStage::Capacity)?;
    let target = SqliteAdapter::open(sqlite_config)?;
    let mut batch = WriteBatch::with_capacity(1_000);
    for row in source_db.iterator(IteratorMode::Start) {
        let (key, value) = row.map_err(|_| {
            corrupt(
                "control_migrate",
                "source checkpoint changed or became unreadable during migration",
            )
        })?;
        validate_control_key(&key)?;
        batch.put(key, value);
        if batch.len() == 1_000 {
            block_on(target.write(batch, Durability::Sync))?;
            batch = WriteBatch::with_capacity(1_000);
        }
    }
    if !batch.is_empty() {
        block_on(target.write(batch, Durability::Sync))?;
    }
    stage_hook(MigrationStage::Copy)?;
    block_on(run_migration_adapter_conformance(&target))?;
    stage_hook(MigrationStage::Checkpoint)?;
    target.shutdown()?;
    drop(source_db);

    stage_hook(MigrationStage::TargetVerification)?;
    let target_inventory = inspect_control_sqlite(&plan.temporary_target, &plan.database_identity)?;
    if source != target_inventory {
        return Err(corrupt(
            "control_migrate",
            "source and target inventories or framed checksums differ",
        ));
    }
    stage_hook(MigrationStage::FileSync)?;
    sync_file(&plan.temporary_target)?;
    let parent = plan.final_target.parent().ok_or_else(|| {
        StorageError::invalid("control_migrate", "final target has no parent directory")
    })?;
    stage_hook(MigrationStage::AtomicRename)?;
    fs::rename(&plan.temporary_target, &plan.final_target).map_err(|_| {
        unavailable(
            "control_migrate",
            "verified SQLite target could not be published atomically",
        )
    })?;
    sync_directory(parent)?;
    let completed = now()?;
    let mut receipt = ControlMigrationReceipt {
        format_version: CONTROL_MIGRATION_FORMAT_VERSION,
        database_format_version: crate::CONTROL_SQLITE_FORMAT_VERSION,
        database_identity: plan.database_identity.clone(),
        expected_release_sha256: plan.expected_release_sha256.clone(),
        expected_configuration_sha256: plan.expected_configuration_sha256.clone(),
        source_checkpoint_sha256: plan.source_checkpoint_sha256.clone(),
        source,
        target: target_inventory,
        started_at_unix_seconds: started,
        completed_at_unix_seconds: completed,
        integrity_verified: true,
        byte_exact_verified: true,
        control_domain_verified: true,
        adapter_conformance_verified: true,
        receipt_blake3: String::new(),
    };
    receipt.receipt_blake3 = receipt_digest(&receipt)?;
    write_control_migration_receipt(&plan.receipt_path, &receipt)?;
    Ok(receipt)
}

fn inspect_completed_migration(
    plan: &ControlMigrationPlan,
) -> StorageResult<ControlMigrationReceipt> {
    let bytes = fs::read(&plan.receipt_path).map_err(|_| {
        StorageError::new(
            StorageErrorKind::Conflict,
            "control_migrate",
            false,
            "live target exists without its protected migration receipt",
        )
    })?;
    let receipt: ControlMigrationReceipt = serde_json::from_slice(&bytes)
        .map_err(|_| corrupt("control_migrate", "migration receipt is invalid"))?;
    if receipt.receipt_blake3 != receipt_digest(&receipt)?
        || receipt.database_identity != plan.database_identity
        || receipt.expected_release_sha256 != plan.expected_release_sha256
        || receipt.expected_configuration_sha256 != plan.expected_configuration_sha256
        || receipt.source_checkpoint_sha256 != plan.source_checkpoint_sha256
        || !receipt.integrity_verified
        || !receipt.byte_exact_verified
        || !receipt.control_domain_verified
        || !receipt.adapter_conformance_verified
    {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "control_migrate",
            false,
            "existing migration receipt does not match the plan",
        ));
    }
    let target = inspect_control_sqlite(&plan.final_target, &plan.database_identity)?;
    if target != receipt.target || receipt.source != receipt.target {
        return Err(corrupt(
            "control_migrate",
            "existing live target differs from its migration receipt",
        ));
    }
    Ok(receipt)
}

fn validate_control_key(key: &[u8]) -> StorageResult<()> {
    let system_key = TenantKeyspace::system_range()
        .map(|range| range.contains(key) && is_complete_system_key(key))
        .map_err(|_| {
            StorageError::new(
                StorageErrorKind::Internal,
                "control_migrate",
                false,
                "control keyspace range is unavailable",
            )
        })?;
    let structured_tenant_key =
        structured_tenant_category(key).is_some_and(|category| !category.starts_with("unknown-"));
    if !system_key && !structured_tenant_key {
        return Err(corrupt(
            "control_migrate",
            "source contains a malformed or unsupported control-plane key namespace",
        ));
    }
    Ok(())
}

fn is_complete_system_key(key: &[u8]) -> bool {
    let Some(after_domain) = skip_encoded_segment(key, 2) else {
        return false;
    };
    skip_encoded_segment(key, after_domain) == Some(key.len())
}

async fn run_migration_adapter_conformance(target: &SqliteAdapter) -> StorageResult<()> {
    let probe = b"\xffmako-control-migration-probe";
    let companion = b"\xffmako-control-migration-probe-2";
    let mut seed = WriteBatch::new();
    seed.put(probe, b"v1");
    target.write(seed, Durability::Sync).await?;
    if target.get(probe).await?.as_deref() != Some(b"v1") {
        return Err(corrupt(
            "control_migrate",
            "target adapter point-read conformance failed",
        ));
    }
    let snapshot = target.snapshot().await?;
    let mut update = WriteBatch::new();
    update.put(probe, b"v2");
    update.put(companion, b"scan");
    let outcome = target
        .compare_and_write(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key: probe.to_vec(),
                value: b"v1".to_vec(),
            }],
            batch: update,
            durability: Durability::Sync,
        })
        .await?;
    if outcome != CompareAndWriteResult::Applied
        || snapshot.get(probe).await?.as_deref() != Some(b"v1")
    {
        return Err(corrupt(
            "control_migrate",
            "target adapter conditional or snapshot conformance failed",
        ));
    }
    drop(snapshot);
    let range = KeyRange::new(vec![0xff], vec![0xff, 0xff])?;
    let scan = target
        .scan(ScanRequest::new(
            range,
            ScanDirection::Forward,
            std::num::NonZeroUsize::new(8).expect("non-zero constant"),
        ))
        .await?;
    if scan.len() != 2 {
        return Err(corrupt(
            "control_migrate",
            "target adapter bounded-scan conformance failed",
        ));
    }
    let mut transaction = target
        .begin_transaction(TransactionMode::Serializable)
        .await?;
    if transaction.get_for_update(probe).await?.as_deref() != Some(b"v2") {
        return Err(corrupt(
            "control_migrate",
            "target adapter transaction-read conformance failed",
        ));
    }
    transaction.delete(probe.to_vec())?;
    transaction.delete(companion.to_vec())?;
    transaction.commit(Durability::Sync).await?;
    if target.get(probe).await?.is_some() || target.get(companion).await?.is_some() {
        return Err(corrupt(
            "control_migrate",
            "target adapter transaction cleanup failed",
        ));
    }
    Ok(())
}

pub fn write_control_checkpoint_fence(
    checkpoint: &Path,
    source_checkpoint_sha256: &str,
) -> StorageResult<()> {
    require_absolute(checkpoint, "migration_fence")?;
    validate_digest(source_checkpoint_sha256, "source checkpoint digest")?;
    let path = checkpoint.join(FENCE_MARKER);
    let body = serde_json::json!({
        "format_version": CONTROL_MIGRATION_FORMAT_VERSION,
        "source_checkpoint_sha256": source_checkpoint_sha256,
        "fenced_at_unix_seconds": now()?,
    });
    write_json_new(&path, &body)
}

pub fn write_control_migration_receipt(
    path: &Path,
    receipt: &ControlMigrationReceipt,
) -> StorageResult<()> {
    if receipt.receipt_blake3 != receipt_digest(receipt)? {
        return Err(corrupt(
            "migration_receipt",
            "migration receipt digest is invalid",
        ));
    }
    write_json_new(path, receipt)
}

fn validate_plan(plan: &ControlMigrationPlan) -> StorageResult<()> {
    if plan.format_version != CONTROL_MIGRATION_FORMAT_VERSION {
        return Err(StorageError::unsupported(
            "control_migrate",
            "migration plan format is unsupported",
        ));
    }
    for path in [
        &plan.source_checkpoint,
        &plan.temporary_target,
        &plan.final_target,
        &plan.lock_path,
        &plan.receipt_path,
    ] {
        require_absolute(path, "control_migrate")?;
    }
    if plan.temporary_target == plan.final_target
        || plan.lock_path == plan.final_target
        || plan.lock_path == plan.temporary_target
        || plan.receipt_path == plan.final_target
        || plan.receipt_path == plan.temporary_target
    {
        return Err(StorageError::invalid(
            "control_migrate",
            "source, temporary, live, and lock paths must be distinct",
        ));
    }
    for digest in [
        &plan.expected_release_sha256,
        &plan.expected_configuration_sha256,
        &plan.source_checkpoint_sha256,
    ] {
        validate_digest(digest, "migration binding digest")?;
    }
    if plan.database_identity.is_empty() || plan.database_identity.len() > 128 {
        return Err(StorageError::invalid(
            "control_migrate",
            "database identity is invalid",
        ));
    }
    Ok(())
}

#[derive(Default)]
struct InventoryAccumulator {
    count: u64,
    hasher: blake3::Hasher,
    prefixes: BTreeMap<String, u64>,
}

impl InventoryAccumulator {
    fn push(&mut self, key: &[u8], value: &[u8]) -> StorageResult<()> {
        let key_len = u64::try_from(key.len())
            .map_err(|_| StorageError::invalid("inventory", "key is too large"))?;
        let value_len = u64::try_from(value.len())
            .map_err(|_| StorageError::invalid("inventory", "value is too large"))?;
        self.hasher.update(&key_len.to_be_bytes());
        self.hasher.update(key);
        self.hasher.update(&value_len.to_be_bytes());
        self.hasher.update(value);
        self.count = self.count.checked_add(1).ok_or_else(|| {
            StorageError::new(
                StorageErrorKind::Internal,
                "inventory",
                false,
                "record count overflow",
            )
        })?;
        *self.prefixes.entry(safe_prefix(key)).or_default() += 1;
        Ok(())
    }

    fn finish(self) -> KvInventory {
        KvInventory {
            record_count: self.count,
            framed_blake3: self.hasher.finalize().to_hex().to_string(),
            prefix_counts: self.prefixes,
        }
    }
}

fn safe_prefix(key: &[u8]) -> String {
    if let Some(category) = structured_tenant_category(key) {
        return format!("tenant:{category}");
    }
    let Some(domain) = decode_system_domain(key) else {
        return format!(
            "namespace:{:02x}{:02x}",
            key.first().copied().unwrap_or_default(),
            key.get(1).copied().unwrap_or_default()
        );
    };
    let category = [
        (
            b"control/authentication-identities".as_slice(),
            "authentication-identities",
        ),
        (b"control/developer-".as_slice(), "developer-identity"),
        (b"control/developers".as_slice(), "developer-identity"),
        (
            b"control/operator-auth".as_slice(),
            "operator-authentication",
        ),
        (
            b"control/operator-control-center".as_slice(),
            "operator-control-center",
        ),
        (b"control/organizations".as_slice(), "organizations"),
        (b"control/memberships".as_slice(), "organizations"),
        (b"control/invitations".as_slice(), "organizations"),
        (b"control/projects".as_slice(), "projects"),
        (b"control/environments".as_slice(), "projects"),
        (b"control/quotas".as_slice(), "projects"),
        (b"control/automation-tokens".as_slice(), "projects"),
        (b"control/provision".as_slice(), "provisioning"),
        (b"control/functions".as_slice(), "functions"),
        (b"control/function-".as_slice(), "functions"),
        (b"control/schema-migrations".as_slice(), "functions"),
        (
            b"control/developer-workspace".as_slice(),
            "developer-workspace",
        ),
        (b"control/audit".as_slice(), "audit"),
        (b"audit/".as_slice(), "audit"),
    ]
    .into_iter()
    .find_map(|(prefix, category)| domain.starts_with(prefix).then_some(category))
    .unwrap_or("system-other");
    format!("system:{category}")
}

fn structured_tenant_category(key: &[u8]) -> Option<&'static str> {
    if !key.starts_with(&[1, 0x20]) {
        return None;
    }
    let after_project = skip_encoded_segment(key, 2)?;
    match key.get(after_project).copied()? {
        0x21 => Some("project-metadata"),
        0x30 => {
            let after_environment = skip_encoded_segment(key, after_project + 1)?;
            Some(match key.get(after_environment).copied()? {
                0x31 => "environment-metadata",
                0x40 => "collection-metadata",
                0x41 => "documents",
                0x42 => "indexes",
                0x43 => "change-log",
                0x44 => "idempotency",
                0x45 => "sequencer",
                0x46 => "revisions",
                0x47 => "index-metadata",
                0x48 => "policies",
                0x49 => "authorization",
                0x4a => "application-identity",
                0x4b => "internal-rpc",
                _ => "unknown-environment-namespace",
            })
        }
        _ => Some("unknown-project-namespace"),
    }
}

fn skip_encoded_segment(key: &[u8], mut offset: usize) -> Option<usize> {
    let mut length = 0_usize;
    while offset < key.len() {
        let byte = key[offset];
        offset += 1;
        if byte != 0 {
            length = length.checked_add(1)?;
            continue;
        }
        match key.get(offset).copied() {
            Some(0xff) => {
                length = length.checked_add(1)?;
                offset += 1;
            }
            Some(0) if length > 0 => return Some(offset + 1),
            _ => return None,
        }
    }
    None
}

fn decode_system_domain(key: &[u8]) -> Option<Vec<u8>> {
    if !key.starts_with(&[1, 0x10]) {
        return None;
    }
    let mut offset = 2;
    let mut decoded = Vec::new();
    while offset < key.len() {
        let byte = key[offset];
        offset += 1;
        if byte != 0 {
            decoded.push(byte);
            continue;
        }
        match key.get(offset).copied() {
            Some(0xff) => {
                decoded.push(0);
                offset += 1;
            }
            Some(0) => return (!decoded.is_empty()).then_some(decoded),
            _ => return None,
        }
    }
    None
}

fn receipt_digest(receipt: &ControlMigrationReceipt) -> StorageResult<String> {
    let mut copy = receipt.clone();
    copy.receipt_blake3.clear();
    serde_json::to_vec(&copy)
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .map_err(|_| {
            StorageError::new(
                StorageErrorKind::Internal,
                "migration_receipt",
                false,
                "receipt encoding failed",
            )
        })
}

fn validate_digest(value: &str, field: &'static str) -> StorageResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StorageError::invalid(
            "control_migrate",
            format!("{field} must be a SHA-256 hex digest"),
        ));
    }
    Ok(())
}

fn require_absolute(path: &Path, operation: &'static str) -> StorageResult<()> {
    if !path.is_absolute() {
        return Err(StorageError::invalid(operation, "path must be absolute"));
    }
    Ok(())
}

fn now() -> StorageResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| {
            StorageError::new(
                StorageErrorKind::Internal,
                "clock",
                false,
                "system clock is invalid",
            )
        })
}

fn write_json_new<T: Serialize>(path: &Path, value: &T) -> StorageResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| StorageError::invalid("migration_receipt", "path has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|_| unavailable("migration_receipt", "receipt directory is unavailable"))?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| {
        StorageError::new(
            StorageErrorKind::Internal,
            "migration_receipt",
            false,
            "receipt encoding failed",
        )
    })?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| {
            StorageError::new(
                StorageErrorKind::Conflict,
                "migration_receipt",
                false,
                "receipt already exists or cannot be created",
            )
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| unavailable("migration_receipt", "receipt could not be synchronized"))?;
    sync_directory(parent)
}

fn sync_file(path: &Path) -> StorageResult<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable("control_migrate", "SQLite target could not be synchronized"))
}

fn sync_directory(path: &Path) -> StorageResult<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| {
            unavailable(
                "control_migrate",
                "migration directory could not be synchronized",
            )
        })
}

fn unavailable(operation: &'static str, message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Unavailable, operation, true, message)
}

fn corrupt(operation: &'static str, message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Corruption, operation, false, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RocksDbAdapter, RocksDbConfig};
    use tempfile::TempDir;

    const SOURCE_DIGEST: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const RELEASE_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CONFIG_DIGEST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn migration_fixture(directory: &TempDir, suffix: &str) -> ControlMigrationPlan {
        let source = directory.path().join(format!("source-{suffix}"));
        let rocks = RocksDbAdapter::open(RocksDbConfig::new(&source)).expect("open source");
        let mut batch = WriteBatch::new();
        batch.put(
            TenantKeyspace::system_key("control/projects", "prj_fault").expect("control key"),
            br#"{"state":"active"}"#,
        );
        block_on(rocks.write(batch, Durability::Sync)).expect("seed source");
        drop(rocks);
        write_control_checkpoint_fence(&source, SOURCE_DIGEST).expect("fence source");
        let root = directory.path().join(suffix);
        fs::create_dir_all(root.join("migration")).expect("migration directory");
        fs::create_dir_all(root.join("live")).expect("live directory");
        fs::create_dir_all(root.join("lock")).expect("lock directory");
        ControlMigrationPlan {
            format_version: CONTROL_MIGRATION_FORMAT_VERSION,
            source_checkpoint: source,
            temporary_target: root.join("migration/control.incomplete.sqlite3"),
            final_target: root.join("live/control.sqlite3"),
            lock_path: root.join("lock/migration.lock"),
            receipt_path: root.join("migration/receipt.json"),
            database_identity: format!("mako-control-fault-{suffix}"),
            expected_release_sha256: RELEASE_DIGEST.into(),
            expected_configuration_sha256: CONFIG_DIGEST.into(),
            source_checkpoint_sha256: SOURCE_DIGEST.into(),
        }
    }

    #[test]
    fn every_interrupted_migration_stage_fails_without_publishing() {
        let directory = TempDir::new().expect("temporary directory");
        for (index, fault) in [
            MigrationStage::SourceScan,
            MigrationStage::Capacity,
            MigrationStage::Copy,
            MigrationStage::Checkpoint,
            MigrationStage::FileSync,
            MigrationStage::AtomicRename,
        ]
        .into_iter()
        .enumerate()
        {
            let plan = migration_fixture(&directory, &format!("stage-{index}"));
            let error = migrate_control_rocks_to_sqlite_with(&plan, |stage| {
                if stage == fault {
                    Err(unavailable(
                        "control_migrate",
                        "injected migration interruption",
                    ))
                } else {
                    Ok(())
                }
            })
            .expect_err("injected stage must fail");
            assert_eq!(error.kind, StorageErrorKind::Unavailable);
            assert!(!plan.final_target.exists(), "{fault:?} published a target");
            assert!(!plan.receipt_path.exists(), "{fault:?} published a receipt");
        }
    }

    #[test]
    fn checksum_inventory_corruption_and_format_failures_never_publish() {
        let directory = TempDir::new().expect("temporary directory");

        let checksum_plan = migration_fixture(&directory, "checksum");
        let error = migrate_control_rocks_to_sqlite_with(&checksum_plan, |stage| {
            if stage == MigrationStage::TargetVerification {
                let connection = Connection::open(&checksum_plan.temporary_target)
                    .expect("open temporary target");
                connection
                    .execute(
                        "UPDATE mako_kv SET value = X'00' WHERE key = (SELECT key FROM mako_kv ORDER BY key LIMIT 1)",
                        [],
                    )
                    .expect("alter value without changing inventory count");
            }
            Ok(())
        })
        .expect_err("checksum mismatch");
        assert_eq!(error.kind, StorageErrorKind::Corruption);
        assert!(!checksum_plan.final_target.exists());

        let inventory_plan = migration_fixture(&directory, "inventory");
        let error = migrate_control_rocks_to_sqlite_with(&inventory_plan, |stage| {
            if stage == MigrationStage::TargetVerification {
                let connection = Connection::open(&inventory_plan.temporary_target)
                    .expect("open temporary target");
                connection
                    .execute(
                        "DELETE FROM mako_kv WHERE key = (SELECT key FROM mako_kv ORDER BY key LIMIT 1)",
                        [],
                    )
                    .expect("alter target inventory");
            }
            Ok(())
        })
        .expect_err("inventory mismatch");
        assert_eq!(error.kind, StorageErrorKind::Corruption);
        assert!(!inventory_plan.final_target.exists());

        let corrupt_source_plan = migration_fixture(&directory, "corrupt-source");
        fs::write(
            corrupt_source_plan.source_checkpoint.join("CURRENT"),
            b"MANIFEST-does-not-exist\n",
        )
        .expect("corrupt source checkpoint");
        assert!(migrate_control_rocks_to_sqlite(&corrupt_source_plan).is_err());
        assert!(!corrupt_source_plan.final_target.exists());

        let mut unsupported_plan = migration_fixture(&directory, "unsupported-format");
        unsupported_plan.format_version = CONTROL_MIGRATION_FORMAT_VERSION + 1;
        let error = migrate_control_rocks_to_sqlite(&unsupported_plan)
            .expect_err("unsupported migration format");
        assert_eq!(error.kind, StorageErrorKind::Unsupported);
        assert!(!unsupported_plan.final_target.exists());
    }
}
