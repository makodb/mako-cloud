use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, MAIN_DB, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CONTROL_SQLITE_FORMAT_VERSION, KvInventory, StorageError, StorageErrorKind, StorageResult,
    inspect_control_sqlite,
};

pub const CONTROL_BACKUP_FORMAT_VERSION: u32 = 1;
const DATABASE_FILE: &str = "control.sqlite3";
const MANIFEST_FILE: &str = "control-backup-manifest.json";

#[derive(Clone, Eq, PartialEq)]
pub struct ControlBackupSigningKey([u8; 32]);

impl ControlBackupSigningKey {
    pub fn new(material: impl AsRef<[u8]>) -> StorageResult<Self> {
        let material = material.as_ref();
        if !(32..=64 * 1024).contains(&material.len()) {
            return Err(StorageError::invalid(
                "control_backup_key",
                "backup signing material must contain between 32 bytes and 64 KiB",
            ));
        }
        Ok(Self(*blake3::hash(material).as_bytes()))
    }

    fn authenticate(&self, manifest: &ControlBackupManifest) -> StorageResult<String> {
        let mut copy = manifest.clone();
        copy.authentication_tag.clear();
        let encoded =
            serde_json::to_vec(&copy).map_err(|_| internal("backup manifest encoding failed"))?;
        Ok(blake3::keyed_hash(&self.0, &encoded).to_hex().to_string())
    }
}

impl std::fmt::Debug for ControlBackupSigningKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ControlBackupSigningKey([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlBackupManifest {
    pub format_version: u32,
    pub backup_id: String,
    pub database_identity: String,
    pub database_format_version: u32,
    pub release_sha256: String,
    pub created_at_unix_seconds: u64,
    pub database_size_bytes: u64,
    pub database_sha256: String,
    pub inventory: KvInventory,
    pub integrity_verified: bool,
    pub durable_high_water: u64,
    pub authentication_tag: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlRestoreReport {
    pub backup_id: String,
    pub restored_path: PathBuf,
    pub database_identity: String,
    pub inventory: KvInventory,
    pub integrity_verified: bool,
    pub promotable: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn create_control_sqlite_backup(
    live_database: &Path,
    expected_identity: &str,
    staging_root: &Path,
    publish_root: &Path,
    backup_id: &str,
    release_sha256: &str,
    created_at_unix_seconds: u64,
    retention: NonZeroU32,
    signing_key: &ControlBackupSigningKey,
) -> StorageResult<ControlBackupManifest> {
    validate_safe_id(backup_id)?;
    validate_digest(release_sha256)?;
    for path in [live_database, staging_root, publish_root] {
        require_absolute(path, "control_backup")?;
    }
    // Validate the live identity before invoking SQLite's consistent online backup API.
    inspect_control_sqlite(live_database, expected_identity)?;
    let incomplete = staging_root.join(format!("{backup_id}.incomplete"));
    let published = publish_root.join(backup_id);
    if incomplete.exists() || published.exists() {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "control_backup",
            false,
            "backup identifier already exists",
        ));
    }
    create_private_directory(staging_root)?;
    create_private_directory(publish_root)?;
    create_private_directory(&incomplete)?;
    let backup_database = incomplete.join(DATABASE_FILE);
    let source = Connection::open_with_flags(
        live_database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| {
        unavailable(
            "control_backup",
            "live control database could not be opened",
        )
    })?;
    source
        .backup(MAIN_DB, &backup_database, None)
        .map_err(|_| unavailable("control_backup", "transactional SQLite backup failed"))?;
    let backup_inventory = inspect_control_sqlite(&backup_database, expected_identity)?;
    let durable_high_water = backup_inventory.record_count;
    sync_file(&backup_database)?;
    let database_size_bytes = fs::metadata(&backup_database)
        .map_err(|_| unavailable("control_backup", "backup metadata is unavailable"))?
        .len();
    let mut manifest = ControlBackupManifest {
        format_version: CONTROL_BACKUP_FORMAT_VERSION,
        backup_id: backup_id.to_owned(),
        database_identity: expected_identity.to_owned(),
        database_format_version: CONTROL_SQLITE_FORMAT_VERSION,
        release_sha256: release_sha256.to_owned(),
        created_at_unix_seconds,
        database_size_bytes,
        database_sha256: sha256_file(&backup_database)?,
        inventory: backup_inventory,
        integrity_verified: true,
        durable_high_water,
        authentication_tag: String::new(),
    };
    manifest.authentication_tag = signing_key.authenticate(&manifest)?;
    write_json_new(&incomplete.join(MANIFEST_FILE), &manifest)?;
    sync_directory(&incomplete)?;
    fs::rename(&incomplete, &published).map_err(|_| {
        unavailable(
            "control_backup",
            "verified backup could not be published atomically",
        )
    })?;
    sync_directory(publish_root)?;
    let verified = inspect_control_sqlite_backup(&published, signing_key)?;
    apply_retention(publish_root, retention, signing_key)?;
    Ok(verified)
}

pub fn inspect_control_sqlite_backup(
    artifact: &Path,
    signing_key: &ControlBackupSigningKey,
) -> StorageResult<ControlBackupManifest> {
    require_absolute(artifact, "control_backup_inspect")?;
    let bytes = fs::read(artifact.join(MANIFEST_FILE))
        .map_err(|_| corrupt("control_backup_inspect", "backup manifest is missing"))?;
    let manifest: ControlBackupManifest = serde_json::from_slice(&bytes)
        .map_err(|_| corrupt("control_backup_inspect", "backup manifest is invalid"))?;
    if manifest.format_version != CONTROL_BACKUP_FORMAT_VERSION
        || manifest.database_format_version != CONTROL_SQLITE_FORMAT_VERSION
    {
        return Err(StorageError::unsupported(
            "control_backup_inspect",
            "backup or control database format is unsupported",
        ));
    }
    if signing_key.authenticate(&manifest)? != manifest.authentication_tag {
        return Err(corrupt(
            "control_backup_inspect",
            "backup manifest authentication failed",
        ));
    }
    let database = artifact.join(DATABASE_FILE);
    let metadata = fs::symlink_metadata(&database)
        .map_err(|_| corrupt("control_backup_inspect", "backup database is missing"))?;
    if !metadata.file_type().is_file()
        || metadata.len() != manifest.database_size_bytes
        || sha256_file(&database)? != manifest.database_sha256
    {
        return Err(corrupt(
            "control_backup_inspect",
            "backup database digest or metadata changed",
        ));
    }
    let inventory = inspect_control_sqlite(&database, &manifest.database_identity)?;
    if inventory != manifest.inventory || inventory.record_count < manifest.durable_high_water {
        return Err(corrupt(
            "control_backup_inspect",
            "backup inventory or durable high water is invalid",
        ));
    }
    Ok(manifest)
}

#[allow(clippy::too_many_arguments)]
pub fn restore_control_sqlite_backup(
    artifact: &Path,
    offline_target: &Path,
    expected_identity: &str,
    expected_release_sha256: &str,
    now_unix_seconds: u64,
    maximum_age: Duration,
    signing_key: &ControlBackupSigningKey,
) -> StorageResult<ControlRestoreReport> {
    require_absolute(offline_target, "control_restore")?;
    if offline_target.exists() {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "control_restore",
            false,
            "restore target must not already exist",
        ));
    }
    let manifest = inspect_control_sqlite_backup(artifact, signing_key)?;
    if manifest.database_identity != expected_identity
        || manifest.release_sha256 != expected_release_sha256
    {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "control_restore",
            false,
            "backup identity or release binding does not match restore policy",
        ));
    }
    let age = now_unix_seconds
        .checked_sub(manifest.created_at_unix_seconds)
        .ok_or_else(|| corrupt("control_restore", "backup timestamp is in the future"))?;
    if age > maximum_age.as_secs() {
        return Err(StorageError::new(
            StorageErrorKind::Unavailable,
            "control_restore",
            false,
            "backup exceeds the maximum permitted recovery age",
        ));
    }
    let parent = offline_target.parent().ok_or_else(|| {
        StorageError::invalid("control_restore", "restore target has no parent directory")
    })?;
    fs::create_dir_all(parent)
        .map_err(|_| unavailable("control_restore", "restore directory is unavailable"))?;
    let temporary = parent.join(format!(
        ".{}.restore.incomplete",
        offline_target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| StorageError::invalid(
                "control_restore",
                "restore target name is invalid"
            ))?
    ));
    if temporary.exists() {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "control_restore",
            false,
            "incomplete restore target requires inspection",
        ));
    }
    copy_file_new(&artifact.join(DATABASE_FILE), &temporary)?;
    let inventory = inspect_control_sqlite(&temporary, expected_identity)?;
    if inventory != manifest.inventory {
        return Err(corrupt(
            "control_restore",
            "restored database inventory differs from the manifest",
        ));
    }
    sync_file(&temporary)?;
    fs::rename(&temporary, offline_target)
        .map_err(|_| unavailable("control_restore", "verified restore could not be published"))?;
    sync_directory(parent)?;
    Ok(ControlRestoreReport {
        backup_id: manifest.backup_id,
        restored_path: offline_target.to_path_buf(),
        database_identity: expected_identity.to_owned(),
        inventory,
        integrity_verified: true,
        promotable: true,
    })
}

pub fn promote_control_sqlite_restore(
    restored: &Path,
    live_target: &Path,
    expected_identity: &str,
) -> StorageResult<KvInventory> {
    for path in [restored, live_target] {
        require_absolute(path, "control_restore_promote")?;
    }
    if live_target.exists() {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "control_restore_promote",
            false,
            "live target already exists and will not be overwritten",
        ));
    }
    let inventory = inspect_control_sqlite(restored, expected_identity)?;
    let parent = live_target.parent().ok_or_else(|| {
        StorageError::invalid(
            "control_restore_promote",
            "live target has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)
        .map_err(|_| unavailable("control_restore_promote", "live directory is unavailable"))?;
    fs::rename(restored, live_target).map_err(|_| {
        unavailable(
            "control_restore_promote",
            "verified restore could not be promoted atomically",
        )
    })?;
    sync_directory(parent)?;
    Ok(inventory)
}

fn apply_retention(
    root: &Path,
    retention: NonZeroU32,
    key: &ControlBackupSigningKey,
) -> StorageResult<()> {
    let mut backups = fs::read_dir(root)
        .map_err(|_| unavailable("control_backup", "backup destination cannot be listed"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            inspect_control_sqlite_backup(&entry.path(), key)
                .ok()
                .map(|manifest| (manifest.created_at_unix_seconds, entry.path()))
        })
        .collect::<Vec<_>>();
    backups.sort();
    let remove = backups.len().saturating_sub(retention.get() as usize);
    for (_, path) in backups.into_iter().take(remove) {
        if path.parent() != Some(root) {
            return Err(internal("backup retention path escaped its root"));
        }
        fs::remove_dir_all(path)
            .map_err(|_| unavailable("control_backup", "expired backup could not be removed"))?;
    }
    sync_directory(root)
}

fn validate_safe_id(value: &str) -> StorageResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(StorageError::invalid(
            "control_backup",
            "backup identifier is invalid",
        ));
    }
    Ok(())
}

fn validate_digest(value: &str) -> StorageResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StorageError::invalid(
            "control_backup",
            "release binding must be a SHA-256 hex digest",
        ));
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> StorageResult<()> {
    fs::create_dir_all(path)
        .map_err(|_| unavailable("control_backup", "backup directory could not be created"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| unavailable("control_backup", "backup permissions could not be set"))?;
    }
    Ok(())
}

fn copy_file_new(source: &Path, target: &Path) -> StorageResult<()> {
    let mut input = File::open(source)
        .map_err(|_| unavailable("control_restore", "backup database could not be opened"))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .map_err(|_| unavailable("control_restore", "restore target could not be created"))?;
    std::io::copy(&mut input, &mut output)
        .and_then(|_| output.sync_all())
        .map_err(|_| {
            unavailable(
                "control_restore",
                "restore target could not be synchronized",
            )
        })?;
    Ok(())
}

fn write_json_new<T: Serialize>(path: &Path, value: &T) -> StorageResult<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| internal("backup manifest encoding failed"))?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| unavailable("control_backup", "backup manifest could not be created"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| {
            unavailable(
                "control_backup",
                "backup manifest could not be synchronized",
            )
        })
}

fn sha256_file(path: &Path) -> StorageResult<String> {
    let mut file = File::open(path)
        .map_err(|_| unavailable("control_backup", "backup database could not be hashed"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| unavailable("control_backup", "backup database could not be hashed"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn require_absolute(path: &Path, operation: &'static str) -> StorageResult<()> {
    if !path.is_absolute() {
        return Err(StorageError::invalid(operation, "path must be absolute"));
    }
    Ok(())
}

fn sync_file(path: &Path) -> StorageResult<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable("control_backup", "database file could not be synchronized"))
}

fn sync_directory(path: &Path) -> StorageResult<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable("control_backup", "directory could not be synchronized"))
}

fn unavailable(operation: &'static str, message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Unavailable, operation, true, message)
}

fn corrupt(operation: &'static str, message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Corruption, operation, false, message)
}

fn internal(message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Internal, "control_backup", false, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Durability, KvAdapter, SqliteAdapter, SqliteConfig, WriteBatch};
    use futures::executor::block_on;
    use tempfile::TempDir;

    const RELEASE_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn authenticated_manifest_with_insufficient_high_water_is_rejected() {
        let directory = TempDir::new().expect("temporary directory");
        let database = directory.path().join("live/control.sqlite3");
        let identity = "mako-control-high-water-test";
        let mut config = SqliteConfig::new(&database, identity);
        config.disk_warning_free_bytes = 2;
        config.disk_critical_free_bytes = 1;
        let adapter = SqliteAdapter::open(config).expect("open SQLite");
        let mut batch = WriteBatch::new();
        batch.put(b"control/high-water", b"committed");
        block_on(adapter.write(batch, Durability::Sync)).expect("seed control record");
        let key = ControlBackupSigningKey::new([7_u8; 32]).expect("signing key");
        create_control_sqlite_backup(
            &database,
            identity,
            &directory.path().join("staging"),
            &directory.path().join("published"),
            "backup-high-water",
            RELEASE_DIGEST,
            100,
            NonZeroU32::new(1).expect("retention"),
            &key,
        )
        .expect("create backup");
        let artifact = directory.path().join("published/backup-high-water");
        let manifest_path = artifact.join(MANIFEST_FILE);
        let mut manifest: ControlBackupManifest =
            serde_json::from_slice(&fs::read(&manifest_path).expect("read manifest"))
                .expect("parse manifest");
        manifest.durable_high_water = manifest.inventory.record_count + 1;
        manifest.authentication_tag = key.authenticate(&manifest).expect("authenticate manifest");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("encode manifest"),
        )
        .expect("write manifest");
        let error =
            inspect_control_sqlite_backup(&artifact, &key).expect_err("insufficient high water");
        assert_eq!(error.kind, StorageErrorKind::Corruption);
        adapter.shutdown().expect("shutdown");
    }
}
