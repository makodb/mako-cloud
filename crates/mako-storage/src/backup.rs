use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    fs::OpenOptions,
    io::{self, Read, Write},
    num::NonZeroU32,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    DATABASE_FORMAT_VERSION, MARKER_VERSION, ProductionRocksDb, ProductionVolumeIdentity,
    RocksDbAdapter, RocksDbConfig, SequencerRecoveryReport, StorageError, StorageErrorKind,
    StorageResult, VolumeMarker, VolumeState,
    production::{recover_all_sequencers, write_volume_marker},
};

pub const BACKUP_MANIFEST_VERSION: u32 = 1;
const ENVELOPE_FILE_NAME: &str = "manifest-envelope.json";
const CHECKPOINT_DIRECTORY_NAME: &str = "checkpoint";
/// A checkpoint being staged, and any checkpoint a finished backup left behind.
const STAGING_SUFFIX: &str = ".incomplete";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackupId(String);

impl BackupId {
    pub fn new(value: impl Into<String>) -> StorageResult<Self> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(StorageError::invalid(
                "backup_id",
                "backup identifier must be a 1-128 character safe identifier",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupFileInventory {
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantBackupInventory {
    pub project: Vec<u8>,
    pub environment: Vec<u8>,
    pub acknowledged_high_water: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub manifest_version: u32,
    pub backup_id: BackupId,
    pub identity: ProductionVolumeIdentity,
    pub database_format_version: u32,
    pub created_at_unix_seconds: u64,
    pub files: Vec<BackupFileInventory>,
    pub tenants: Vec<TenantBackupInventory>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifestEnvelope {
    pub manifest: BackupManifest,
    pub authentication_tag: String,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ManifestSigningKey([u8; 32]);

impl ManifestSigningKey {
    pub fn new(material: impl AsRef<[u8]>) -> StorageResult<Self> {
        let material = material.as_ref();
        if material.len() < 32 || material.len() > 64 * 1024 {
            return Err(StorageError::invalid(
                "manifest_signing_key",
                "signing material must contain between 32 bytes and 64 KiB",
            ));
        }
        Ok(Self(*blake3::hash(material).as_bytes()))
    }

    fn authenticate(&self, manifest: &BackupManifest) -> StorageResult<String> {
        let encoded = serde_json::to_vec(manifest).map_err(|_| internal_backup_error())?;
        Ok(encode_hex(blake3::keyed_hash(&self.0, &encoded).as_bytes()))
    }
}

impl fmt::Debug for ManifestSigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ManifestSigningKey([REDACTED])")
    }
}

#[derive(Clone, Debug)]
pub struct BackupRequest {
    pub backup_id: BackupId,
    pub created_at_unix_seconds: u64,
    pub staging_root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct BackupArtifact {
    pub root: PathBuf,
    pub envelope: BackupManifestEnvelope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupMetrics {
    pub backup_id: BackupId,
    pub created_at_unix_seconds: u64,
    pub completed_at_unix_seconds: u64,
    pub file_count: usize,
    pub total_bytes: u64,
    pub duration: Duration,
    pub verified_after_upload: bool,
    pub retained_backups: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackupTelemetry {
    pub successful_backups: u64,
    pub failed_backups: u64,
    pub last_success_unix_seconds: Option<u64>,
    pub last_failure_unix_seconds: Option<u64>,
    pub latest_backup_age_seconds: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct FilesystemBackupTransport {
    root: PathBuf,
    telemetry: Arc<Mutex<BackupTelemetry>>,
}

impl FilesystemBackupTransport {
    pub fn new(root: impl Into<PathBuf>) -> StorageResult<Self> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(StorageError::invalid(
                "backup_transport",
                "backup destination must be absolute",
            ));
        }
        fs::create_dir_all(&root).map_err(|_| io_backup_error("backup destination unavailable"))?;
        Ok(Self {
            root,
            telemetry: Arc::new(Mutex::new(BackupTelemetry::default())),
        })
    }

    pub fn publish(
        &self,
        artifact: &BackupArtifact,
        signing_key: &ManifestSigningKey,
        retention_count: NonZeroU32,
        completed_at_unix_seconds: u64,
    ) -> StorageResult<BackupMetrics> {
        let result = self.publish_inner(
            artifact,
            signing_key,
            retention_count,
            completed_at_unix_seconds,
        );
        if let Ok(mut telemetry) = self.telemetry.lock() {
            match &result {
                Ok(metrics) => {
                    telemetry.successful_backups += 1;
                    telemetry.last_success_unix_seconds = Some(metrics.completed_at_unix_seconds);
                    telemetry.latest_backup_age_seconds = Some(0);
                }
                Err(_) => {
                    telemetry.failed_backups += 1;
                    telemetry.last_failure_unix_seconds = Some(completed_at_unix_seconds);
                }
            }
        }
        result
    }

    fn publish_inner(
        &self,
        artifact: &BackupArtifact,
        signing_key: &ManifestSigningKey,
        retention_count: NonZeroU32,
        completed_at_unix_seconds: u64,
    ) -> StorageResult<BackupMetrics> {
        let started = Instant::now();
        let target = self
            .root
            .join(artifact.envelope.manifest.backup_id.as_str());
        let staging = self.root.join(format!(
            ".upload-{}",
            artifact.envelope.manifest.backup_id.as_str()
        ));
        if target.exists() || staging.exists() {
            return Err(StorageError::new(
                StorageErrorKind::Conflict,
                "backup_transport",
                false,
                "immutable backup identifier already exists",
            ));
        }
        inspect_backup(&artifact.root, signing_key)?;
        create_private_directory(&staging)?;
        if let Err(error) = copy_tree_contents(&artifact.root, &staging) {
            let _ignored = fs::remove_dir_all(&staging);
            return Err(error);
        }
        sync_tree(&staging)?;
        fs::rename(&staging, &target)
            .map_err(|_| io_backup_error("backup upload cannot be committed"))?;
        sync_directory(&self.root)?;
        let verified = inspect_backup(&target, signing_key)?;
        if verified != artifact.envelope.manifest {
            return Err(corrupt_backup("uploaded backup manifest changed"));
        }
        let retained_backups = self.apply_retention(retention_count, signing_key)?;
        remove_staging_artifact(&artifact.root)?;
        Ok(BackupMetrics {
            backup_id: verified.backup_id,
            created_at_unix_seconds: verified.created_at_unix_seconds,
            completed_at_unix_seconds,
            file_count: verified.files.len(),
            total_bytes: verified.files.iter().map(|file| file.size_bytes).sum(),
            duration: started.elapsed(),
            verified_after_upload: true,
            retained_backups,
        })
    }

    pub fn backup_path(&self, backup_id: &BackupId) -> PathBuf {
        self.root.join(backup_id.as_str())
    }

    pub fn telemetry(&self, now_unix_seconds: u64) -> BackupTelemetry {
        let mut telemetry = self.telemetry.lock().map_or_else(
            |_| BackupTelemetry::default(),
            |telemetry| telemetry.clone(),
        );
        telemetry.latest_backup_age_seconds = telemetry
            .last_success_unix_seconds
            .and_then(|last| now_unix_seconds.checked_sub(last));
        telemetry
    }

    fn apply_retention(
        &self,
        retention_count: NonZeroU32,
        signing_key: &ManifestSigningKey,
    ) -> StorageResult<usize> {
        let mut eligible = Vec::new();
        for entry in fs::read_dir(&self.root)
            .map_err(|_| io_backup_error("backup destination cannot be listed"))?
        {
            let entry = entry.map_err(|_| io_backup_error("backup entry cannot be inspected"))?;
            if !entry
                .file_type()
                .map_err(|_| io_backup_error("backup entry cannot be inspected"))?
                .is_dir()
                || entry.file_name().to_string_lossy().starts_with('.')
            {
                continue;
            }
            let manifest = inspect_backup(&entry.path(), signing_key)?;
            eligible.push((
                manifest.created_at_unix_seconds,
                manifest.backup_id,
                entry.path(),
            ));
        }
        eligible.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
        let retain = retention_count.get() as usize;
        let remove_count = eligible.len().saturating_sub(retain);
        for (_, _, path) in eligible.iter().take(remove_count) {
            if path.parent() != Some(self.root.as_path()) {
                return Err(internal_backup_error());
            }
            fs::remove_dir_all(path)
                .map_err(|_| io_backup_error("expired backup cannot be removed"))?;
        }
        sync_directory(&self.root)?;
        Ok(eligible.len() - remove_count)
    }
}

impl ProductionRocksDb {
    pub async fn create_backup(
        &self,
        request: BackupRequest,
        signing_key: &ManifestSigningKey,
    ) -> StorageResult<BackupArtifact> {
        if !request.staging_root.is_absolute() {
            return Err(StorageError::invalid(
                "backup_create",
                "backup staging root must be absolute",
            ));
        }
        fs::create_dir_all(&request.staging_root)
            .map_err(|_| io_backup_error("backup staging root is unavailable"))?;
        sweep_stale_staging(&request.staging_root)?;
        let artifact_root = request
            .staging_root
            .join(format!("{}{STAGING_SUFFIX}", request.backup_id.as_str()));
        create_private_directory(&artifact_root)?;
        let checkpoint_path = artifact_root.join(CHECKPOINT_DIRECTORY_NAME);
        if let Err(error) = self.adapter().create_checkpoint(&checkpoint_path) {
            let _ignored = fs::remove_dir_all(&artifact_root);
            return Err(error);
        }

        let inspection = inspect_checkpoint_recovery(&checkpoint_path).await;
        let recovery = match inspection {
            Ok(recovery) => recovery,
            Err(error) => {
                let _ignored = fs::remove_dir_all(&artifact_root);
                return Err(error);
            }
        };
        let files = inventory_files(&checkpoint_path)?;
        if files.is_empty() {
            let _ignored = fs::remove_dir_all(&artifact_root);
            return Err(corrupt_backup("checkpoint contains no files"));
        }
        let tenants = recovery
            .tenants
            .into_iter()
            .map(|tenant| TenantBackupInventory {
                project: tenant.project,
                environment: tenant.environment,
                acknowledged_high_water: tenant.acknowledged_high_water,
            })
            .collect();
        let manifest = BackupManifest {
            manifest_version: BACKUP_MANIFEST_VERSION,
            backup_id: request.backup_id,
            identity: self.marker().identity.clone(),
            database_format_version: self.marker().database_format_version,
            created_at_unix_seconds: request.created_at_unix_seconds,
            files,
            tenants,
        };
        let envelope = BackupManifestEnvelope {
            authentication_tag: signing_key.authenticate(&manifest)?,
            manifest,
        };
        write_json_file(&artifact_root.join(ENVELOPE_FILE_NAME), &envelope, true)?;
        sync_tree(&artifact_root)?;
        Ok(BackupArtifact {
            root: artifact_root,
            envelope,
        })
    }
}

#[derive(Clone, Debug)]
pub struct RestorePolicy {
    pub expected_identity: ProductionVolumeIdentity,
    pub now_unix_seconds: u64,
    pub maximum_backup_age: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreReport {
    pub backup_id: BackupId,
    pub target: PathBuf,
    pub tenants_verified: usize,
    pub files_verified: usize,
    pub acknowledged_high_waters: BTreeMap<(Vec<u8>, Vec<u8>), u64>,
    pub state: VolumeState,
}

pub fn inspect_backup(
    artifact_root: &Path,
    signing_key: &ManifestSigningKey,
) -> StorageResult<BackupManifest> {
    let envelope = read_envelope(artifact_root)?;
    verify_envelope(&envelope, signing_key)?;
    verify_manifest_files(artifact_root, &envelope.manifest)?;
    Ok(envelope.manifest)
}

pub async fn restore_backup(
    artifact_root: &Path,
    target: &Path,
    policy: &RestorePolicy,
    signing_key: &ManifestSigningKey,
) -> StorageResult<RestoreReport> {
    if !target.is_absolute() {
        return Err(StorageError::invalid(
            "restore",
            "restore target must be absolute",
        ));
    }
    ensure_empty_directory(target)?;
    let manifest = inspect_backup(artifact_root, signing_key)?;
    validate_restore_manifest(&manifest, policy)?;

    let parent = target.parent().ok_or_else(|| {
        StorageError::invalid("restore", "restore target must have a parent directory")
    })?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| StorageError::invalid("restore", "restore target name is invalid"))?;
    let staging = parent.join(format!(
        ".{name}.restore-{}.incomplete",
        manifest.backup_id.as_str()
    ));
    if staging.exists() {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "restore",
            false,
            "restore staging path already exists",
        ));
    }
    create_private_directory(&staging)?;
    if let Err(error) = copy_tree_contents(&artifact_root.join(CHECKPOINT_DIRECTORY_NAME), &staging)
    {
        let _ignored = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let verification = verify_restored_database(&staging, &manifest).await;
    let recovered = match verification {
        Ok(recovered) => recovered,
        Err(error) => {
            let _ignored = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    sync_tree(&staging)?;
    fs::remove_dir(target)
        .map_err(|_| io_restore_error("empty restore target cannot be replaced"))?;
    if let Err(error) = fs::rename(&staging, target) {
        let _ignored = fs::create_dir(target);
        let _ignored = fs::remove_dir_all(&staging);
        return Err(StorageError::new(
            StorageErrorKind::Io,
            "restore",
            false,
            format!("restore staging cannot be promoted: {}", error.kind()),
        ));
    }
    let marker = VolumeMarker {
        marker_version: MARKER_VERSION,
        database_format_version: DATABASE_FORMAT_VERSION,
        identity: policy.expected_identity.clone(),
        state: VolumeState::Promotable,
    };
    write_volume_marker(target, &marker, true)?;
    sync_directory(parent)?;

    Ok(RestoreReport {
        backup_id: manifest.backup_id,
        target: target.to_path_buf(),
        tenants_verified: recovered.tenants.len(),
        files_verified: manifest.files.len(),
        acknowledged_high_waters: recovered
            .tenants
            .into_iter()
            .map(|tenant| {
                (
                    (tenant.project, tenant.environment),
                    tenant.acknowledged_high_water,
                )
            })
            .collect(),
        state: VolumeState::Promotable,
    })
}

async fn inspect_checkpoint_recovery(path: &Path) -> StorageResult<SequencerRecoveryReport> {
    let mut config = RocksDbConfig::new(path);
    config.create_if_missing = false;
    let page_size = config.maximum_scan_items;
    let adapter = RocksDbAdapter::open(config)?;
    let report = recover_all_sequencers(&adapter, page_size).await?;
    drop(adapter);
    Ok(report)
}

async fn verify_restored_database(
    path: &Path,
    manifest: &BackupManifest,
) -> StorageResult<SequencerRecoveryReport> {
    let mut config = RocksDbConfig::new(path);
    config.create_if_missing = false;
    let page_size = config.maximum_scan_items;
    let adapter = RocksDbAdapter::open(config)?;
    let recovery = recover_all_sequencers(&adapter, page_size).await?;
    verify_recovery_against_manifest(&recovery, manifest)?;
    drop(adapter);
    Ok(recovery)
}

fn verify_recovery_against_manifest(
    recovery: &SequencerRecoveryReport,
    manifest: &BackupManifest,
) -> StorageResult<()> {
    let expected = manifest
        .tenants
        .iter()
        .map(|tenant| {
            (
                (tenant.project.clone(), tenant.environment.clone()),
                tenant.acknowledged_high_water,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let actual = recovery
        .tenants
        .iter()
        .map(|tenant| {
            (
                (tenant.project.clone(), tenant.environment.clone()),
                tenant.acknowledged_high_water,
            )
        })
        .collect::<BTreeMap<_, _>>();
    if actual.keys().collect::<BTreeSet<_>>() != expected.keys().collect::<BTreeSet<_>>()
        || expected
            .iter()
            .any(|(tenant, high_water)| actual.get(tenant).is_none_or(|actual| actual < high_water))
    {
        return Err(corrupt_backup(
            "restored tenant inventory or acknowledged high water does not match the manifest",
        ));
    }
    Ok(())
}

fn validate_restore_manifest(
    manifest: &BackupManifest,
    policy: &RestorePolicy,
) -> StorageResult<()> {
    if manifest.manifest_version != BACKUP_MANIFEST_VERSION
        || manifest.database_format_version != DATABASE_FORMAT_VERSION
    {
        return Err(StorageError::new(
            StorageErrorKind::Unsupported,
            "restore",
            false,
            "backup manifest or database format version is unsupported",
        ));
    }
    if manifest.identity != policy.expected_identity {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "restore",
            false,
            "backup belongs to a different service or database",
        ));
    }
    let age = policy
        .now_unix_seconds
        .checked_sub(manifest.created_at_unix_seconds)
        .ok_or_else(|| corrupt_backup("backup creation time is in the future"))?;
    if age > policy.maximum_backup_age.as_secs() {
        return Err(StorageError::new(
            StorageErrorKind::Unavailable,
            "restore",
            false,
            "backup exceeds the allowed recovery age",
        ));
    }
    Ok(())
}

fn verify_envelope(
    envelope: &BackupManifestEnvelope,
    signing_key: &ManifestSigningKey,
) -> StorageResult<()> {
    let expected = signing_key.authenticate(&envelope.manifest)?;
    if !constant_time_equal(expected.as_bytes(), envelope.authentication_tag.as_bytes()) {
        return Err(corrupt_backup("backup manifest authentication failed"));
    }
    Ok(())
}

fn verify_manifest_files(root: &Path, manifest: &BackupManifest) -> StorageResult<()> {
    if manifest.files.is_empty() {
        return Err(corrupt_backup(
            "backup manifest contains no checkpoint files",
        ));
    }
    let expected_paths = manifest
        .files
        .iter()
        .map(|file| file.relative_path.clone())
        .collect::<BTreeSet<_>>();
    if expected_paths.len() != manifest.files.len() {
        return Err(corrupt_backup("backup manifest contains duplicate files"));
    }
    let checkpoint = root.join(CHECKPOINT_DIRECTORY_NAME);
    let actual_paths = inventory_files(&checkpoint)?
        .into_iter()
        .map(|file| file.relative_path)
        .collect::<BTreeSet<_>>();
    if actual_paths != expected_paths {
        return Err(corrupt_backup("backup checkpoint file inventory changed"));
    }
    for file in &manifest.files {
        validate_relative_path(&file.relative_path)?;
        let path = checkpoint.join(&file.relative_path);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| corrupt_backup("backup checkpoint file is missing"))?;
        if !metadata.file_type().is_file() || metadata.len() != file.size_bytes {
            return Err(corrupt_backup("backup checkpoint file metadata changed"));
        }
        if hash_file(&path)? != file.sha256 {
            return Err(corrupt_backup("backup checkpoint digest validation failed"));
        }
    }
    Ok(())
}

fn inventory_files(root: &Path) -> StorageResult<Vec<BackupFileInventory>> {
    let mut pending = vec![(root.to_path_buf(), PathBuf::new())];
    let mut files = Vec::new();
    while let Some((directory, relative_directory)) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|_| io_backup_error("checkpoint directory cannot be read"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| io_backup_error("checkpoint entry cannot be read"))?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let relative = relative_directory.join(entry.file_name());
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|_| io_backup_error("checkpoint entry cannot be inspected"))?;
            if metadata.file_type().is_symlink() {
                return Err(corrupt_backup("checkpoint cannot contain symbolic links"));
            }
            if metadata.is_dir() {
                pending.push((entry.path(), relative));
            } else if metadata.is_file() {
                let relative_path = portable_relative_path(&relative)?;
                files.push(BackupFileInventory {
                    relative_path,
                    size_bytes: metadata.len(),
                    sha256: hash_file(&entry.path())?,
                });
            } else {
                return Err(corrupt_backup(
                    "checkpoint contains an unsupported file type",
                ));
            }
        }
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn read_envelope(root: &Path) -> StorageResult<BackupManifestEnvelope> {
    let bytes = fs::read(root.join(ENVELOPE_FILE_NAME))
        .map_err(|_| corrupt_backup("backup manifest envelope is missing"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| corrupt_backup("backup manifest envelope is invalid"))
}

fn write_json_file<T: Serialize>(path: &Path, value: &T, create_new: bool) -> StorageResult<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| internal_backup_error())?;
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(create_new)
        .create(!create_new);
    let mut file = options
        .open(path)
        .map_err(|_| io_backup_error("backup metadata file cannot be created"))?;
    file.write_all(&bytes)
        .map_err(|_| io_backup_error("backup metadata file cannot be written"))?;
    file.sync_all()
        .map_err(|_| io_backup_error("backup metadata file cannot be synchronized"))
}

fn hash_file(path: &Path) -> StorageResult<String> {
    let mut file =
        fs::File::open(path).map_err(|_| io_backup_error("checkpoint file cannot be opened"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| io_backup_error("checkpoint file cannot be hashed"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(encode_hex(&hasher.finalize()))
}

fn copy_tree_contents(source: &Path, target: &Path) -> StorageResult<()> {
    let mut pending = vec![(source.to_path_buf(), target.to_path_buf())];
    while let Some((source_directory, target_directory)) = pending.pop() {
        let mut entries = fs::read_dir(&source_directory)
            .map_err(|_| io_backup_error("backup source cannot be read"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| io_backup_error("backup source entry cannot be read"))?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|_| io_backup_error("backup source entry cannot be inspected"))?;
            let destination = target_directory.join(entry.file_name());
            if metadata.file_type().is_symlink() {
                return Err(corrupt_backup(
                    "backup source cannot contain symbolic links",
                ));
            }
            if metadata.is_dir() {
                create_private_directory(&destination)?;
                pending.push((entry.path(), destination));
            } else if metadata.is_file() {
                copy_file_immutable(&entry.path(), &destination)?;
            } else {
                return Err(corrupt_backup(
                    "backup source contains an unsupported file type",
                ));
            }
        }
    }
    Ok(())
}

fn copy_file_immutable(source: &Path, target: &Path) -> StorageResult<()> {
    let mut input = fs::File::open(source)
        .map_err(|_| io_backup_error("backup source file cannot be opened"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|_| io_backup_error("backup destination file cannot be created"))?;
    std::io::copy(&mut input, &mut output)
        .map_err(|_| io_backup_error("backup file cannot be copied"))?;
    output
        .sync_all()
        .map_err(|_| io_backup_error("backup file cannot be synchronized"))
}

fn sync_tree(root: &Path) -> StorageResult<()> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        for entry in fs::read_dir(&directories[index])
            .map_err(|_| io_backup_error("backup directory cannot be synchronized"))?
        {
            let entry =
                entry.map_err(|_| io_backup_error("backup entry cannot be synchronized"))?;
            if entry
                .file_type()
                .map_err(|_| io_backup_error("backup entry cannot be synchronized"))?
                .is_dir()
            {
                directories.push(entry.path());
            }
        }
        index += 1;
    }
    for directory in directories.into_iter().rev() {
        sync_directory(&directory)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> StorageResult<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| io_backup_error("backup directory cannot be synchronized"))
}

/// The staging copy exists only to be published. Once the destination holds
/// the verified artifact it is redundant, and leaving it behind is how a
/// staging root fills a disk one checkpoint at a time.
fn remove_staging_artifact(root: &Path) -> StorageResult<()> {
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(io_backup_error(
            "published backup staging copy cannot be removed",
        )),
    }
}

/// Removes the `*.incomplete` checkpoints earlier backups left in the staging
/// root. A backup holds the database exclusively, so nothing else can be
/// staging a checkpoint here; whatever is present is a leftover, and sweeping
/// it bounds the staging root even when a publication could not remove its
/// own copy. Other entries -- an orchestrator's verification scratch, say --
/// are not touched.
fn sweep_stale_staging(staging_root: &Path) -> StorageResult<()> {
    for entry in fs::read_dir(staging_root)
        .map_err(|_| io_backup_error("backup staging root cannot be listed"))?
    {
        let entry =
            entry.map_err(|_| io_backup_error("backup staging entry cannot be inspected"))?;
        let stale_checkpoint = entry.file_type().is_ok_and(|kind| kind.is_dir())
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(STAGING_SUFFIX));
        if stale_checkpoint {
            fs::remove_dir_all(entry.path())
                .map_err(|_| io_backup_error("stale backup staging cannot be removed"))?;
        }
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> StorageResult<()> {
    fs::create_dir(path)
        .map_err(|_| io_backup_error("private staging directory cannot be created"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| io_backup_error("private staging permissions cannot be applied"))?;
    }
    Ok(())
}

fn ensure_empty_directory(path: &Path) -> StorageResult<()> {
    let metadata = fs::metadata(path).map_err(|_| {
        io_restore_error("restore target must already exist as an empty offline directory")
    })?;
    if !metadata.is_dir() {
        return Err(StorageError::invalid(
            "restore",
            "restore target must be a directory",
        ));
    }
    if fs::read_dir(path)
        .map_err(|_| io_restore_error("restore target cannot be inspected"))?
        .next()
        .transpose()
        .map_err(|_| io_restore_error("restore target cannot be inspected"))?
        .is_some()
    {
        return Err(StorageError::new(
            StorageErrorKind::Conflict,
            "restore",
            false,
            "restore target is not empty",
        ));
    }
    Ok(())
}

fn portable_relative_path(path: &Path) -> StorageResult<String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(corrupt_backup("checkpoint relative path is invalid"));
    }
    Ok(path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn validate_relative_path(path: &str) -> StorageResult<()> {
    portable_relative_path(Path::new(path)).map(|_| ())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn corrupt_backup(message: &'static str) -> StorageError {
    StorageError::new(
        StorageErrorKind::Corruption,
        "backup_verify",
        false,
        message,
    )
}

fn io_backup_error(message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Io, "backup", true, message)
}

fn io_restore_error(message: &'static str) -> StorageError {
    StorageError::new(StorageErrorKind::Io, "restore", false, message)
}

fn internal_backup_error() -> StorageError {
    StorageError::new(
        StorageErrorKind::Internal,
        "backup",
        false,
        "backup metadata operation failed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_manifest_format_is_rejected_even_when_other_fields_match() {
        let identity =
            ProductionVolumeIdentity::new("mako-data-plane", "database").expect("identity");
        let manifest = BackupManifest {
            manifest_version: BACKUP_MANIFEST_VERSION,
            backup_id: BackupId::new("backup").expect("backup id"),
            identity: identity.clone(),
            database_format_version: DATABASE_FORMAT_VERSION + 1,
            created_at_unix_seconds: 10,
            files: Vec::new(),
            tenants: Vec::new(),
        };
        let error = validate_restore_manifest(
            &manifest,
            &RestorePolicy {
                expected_identity: identity,
                now_unix_seconds: 10,
                maximum_backup_age: Duration::from_secs(1),
            },
        )
        .expect_err("wrong format must fail");
        assert_eq!(error.kind, StorageErrorKind::Unsupported);
    }

    #[test]
    fn cross_tenant_restore_inventory_is_rejected() {
        let identity =
            ProductionVolumeIdentity::new("mako-data-plane", "database").expect("identity");
        let manifest = BackupManifest {
            manifest_version: BACKUP_MANIFEST_VERSION,
            backup_id: BackupId::new("backup").expect("backup id"),
            identity,
            database_format_version: DATABASE_FORMAT_VERSION,
            created_at_unix_seconds: 10,
            files: Vec::new(),
            tenants: vec![TenantBackupInventory {
                project: b"project-a".to_vec(),
                environment: b"production".to_vec(),
                acknowledged_high_water: 4,
            }],
        };
        let recovery = SequencerRecoveryReport {
            tenants: vec![
                crate::TenantRecoveryEvidence {
                    project: b"project-a".to_vec(),
                    environment: b"production".to_vec(),
                    acknowledged_high_water: 4,
                },
                crate::TenantRecoveryEvidence {
                    project: b"project-b".to_vec(),
                    environment: b"production".to_vec(),
                    acknowledged_high_water: 1,
                },
            ],
        };
        let error = verify_recovery_against_manifest(&recovery, &manifest)
            .expect_err("undeclared tenant must fail restore");
        assert_eq!(error.kind, StorageErrorKind::Corruption);
    }
}
