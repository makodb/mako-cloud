use std::{
    collections::BTreeMap,
    fs,
    num::{NonZeroU32, NonZeroUsize},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::executor::block_on;
use mako_storage::{
    BackupId, BackupRequest, DATABASE_FORMAT_VERSION, Durability, FilesystemBackupTransport,
    KvAdapter, ManifestSigningKey, ProductionRocksDb, ProductionRocksDbConfig,
    ProductionVolumeIdentity, RestorePolicy, SequencerKeyKind, TenantKeyspace, VolumeState,
    WriteBatch, activate_fenced_volume, fence_production_volume, inspect_backup,
    promote_restored_volume, provision_production_volume, read_volume_marker, restore_backup,
};

const QUALIFICATION_COMMIT_VALUE: &[u8] = b"mako-public-beta-qualification-v1";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("storage operation failed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let (command, arguments) = parse_arguments()?;
    match command.as_str() {
        "provision" => provision(&arguments),
        "backup" => backup(&arguments),
        "qualify-recovery" => qualify_recovery(&arguments),
        "inspect" | "verify" => inspect(&arguments),
        "restore" => restore(&arguments),
        "fence" => transition(&arguments, "FENCE", VolumeState::Fenced),
        "promote" => transition(&arguments, "PROMOTE", VolumeState::Active),
        "activate" => activate(&arguments),
        _ => Err(
            "expected provision, qualify-recovery, backup, inspect, verify, restore, fence, promote, or activate"
                .into(),
        ),
    }
}

fn qualify_recovery(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let path = absolute_path(arguments, "database-path")?;
    let identity = identity(arguments)?;
    let project = required(arguments, "project")?;
    let environment = required(arguments, "environment")?;
    let keyspace = TenantKeyspace::new(project, environment)
        .map_err(|_| "qualification tenant scope is invalid".to_string())?;
    if flag(arguments, "dry-run") {
        println!(
            "dry-run: qualify recovery path={} owner={} database={} expected_high_water=1",
            path.display(),
            identity.service,
            identity.database_id
        );
        return Ok(());
    }
    require_confirmation(arguments, "QUALIFY_RECOVERY")?;

    block_on(async {
        let storage = ProductionRocksDb::open(production_config(path.clone(), identity.clone()))
            .await
            .map_err(safe_storage_error)?;
        let lease_key = keyspace.sequencer_key(SequencerKeyKind::Lease);
        let committed_key = keyspace.sequencer_position_key(SequencerKeyKind::Committed, 1);
        let high_water_key = keyspace.sequencer_key(SequencerKeyKind::HighWater);
        let lease = storage
            .adapter()
            .get(&lease_key)
            .await
            .map_err(safe_storage_error)?;
        let committed = storage
            .adapter()
            .get(&committed_key)
            .await
            .map_err(safe_storage_error)?;
        let high_water = storage
            .adapter()
            .get(&high_water_key)
            .await
            .map_err(safe_storage_error)?;
        match (
            lease.as_deref(),
            committed.as_deref(),
            high_water.as_deref(),
        ) {
            (None, None, None) => {
                let mut batch = WriteBatch::new();
                batch.put(lease_key, 2_u64.to_be_bytes());
                batch.put(committed_key, QUALIFICATION_COMMIT_VALUE);
                storage
                    .adapter()
                    .write(batch, Durability::Sync)
                    .await
                    .map_err(safe_storage_error)?;
            }
            (Some(lease), Some(committed), Some(high_water))
                if lease == 2_u64.to_be_bytes()
                    && committed == QUALIFICATION_COMMIT_VALUE
                    && high_water == 1_u64.to_be_bytes() => {}
            _ => return Err("existing qualification sequencer state is inconsistent".into()),
        }
        storage
            .graceful_shutdown()
            .await
            .map_err(safe_storage_error)?;

        let recovered = ProductionRocksDb::open(production_config(path, identity))
            .await
            .map_err(safe_storage_error)?;
        let tenant = recovered
            .recovery_report()
            .tenants
            .iter()
            .find(|tenant| {
                tenant.project == project.as_bytes() && tenant.environment == environment.as_bytes()
            })
            .ok_or_else(|| "qualification tenant was not recovered".to_string())?;
        if tenant.acknowledged_high_water != 1
            || recovered
                .adapter()
                .get(&high_water_key)
                .await
                .map_err(safe_storage_error)?
                .as_deref()
                != Some(1_u64.to_be_bytes().as_slice())
            || recovered
                .adapter()
                .get(&keyspace.sequencer_position_key(SequencerKeyKind::Committed, 1))
                .await
                .map_err(safe_storage_error)?
                .as_deref()
                != Some(QUALIFICATION_COMMIT_VALUE)
        {
            return Err("acknowledged high-water recovery did not preserve the commit".into());
        }
        recovered
            .graceful_shutdown()
            .await
            .map_err(safe_storage_error)?;
        Ok::<(), String>(())
    })?;
    println!("recovery qualified: tenants=1 acknowledged_high_water=1 durability=sync");
    Ok(())
}

fn provision(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let path = absolute_path(arguments, "database-path")?;
    let identity = identity(arguments)?;
    if flag(arguments, "dry-run") {
        println!(
            "dry-run: provision empty volume path={} owner={} database={}",
            path.display(),
            identity.service,
            identity.database_id
        );
        return Ok(());
    }
    require_confirmation(arguments, "PROVISION")?;
    if flag(arguments, "accept-matching-marker") && path.join("mako-volume.json").exists() {
        let marker = read_volume_marker(&path).map_err(safe_storage_error)?;
        if marker.identity != identity || marker.database_format_version != DATABASE_FORMAT_VERSION
        {
            return Err("existing marker belongs to another owner or format".into());
        }
        println!("volume already provisioned with matching owner and format");
        return Ok(());
    }
    let marker = provision_production_volume(&path, identity).map_err(safe_storage_error)?;
    println!(
        "provisioned volume marker_version={} database_format={} state={:?}",
        marker.marker_version, marker.database_format_version, marker.state
    );
    Ok(())
}

fn backup(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let path = absolute_path(arguments, "database-path")?;
    let staging_root = absolute_path(arguments, "staging-root")?;
    let destination = absolute_path(arguments, "destination")?;
    let identity = identity(arguments)?;
    let backup_id = BackupId::new(required(arguments, "backup-id")?).map_err(safe_storage_error)?;
    let created_at = optional_u64(arguments, "created-at")?.unwrap_or_else(now_unix_seconds);
    let retention = NonZeroU32::new(optional_u32(arguments, "retention")?.unwrap_or(14))
        .ok_or_else(|| "--retention must be positive".to_string())?;
    if flag(arguments, "dry-run") {
        println!(
            "dry-run: checkpoint path={} backup={} staging={} destination={} retention={}",
            path.display(),
            backup_id.as_str(),
            staging_root.display(),
            destination.display(),
            retention
        );
        return Ok(());
    }
    let signing_key = signing_key(arguments)?;
    let storage = block_on(ProductionRocksDb::open(production_config(path, identity)))
        .map_err(safe_storage_error)?;
    let artifact = block_on(storage.create_backup(
        BackupRequest {
            backup_id,
            created_at_unix_seconds: created_at,
            staging_root,
        },
        &signing_key,
    ))
    .map_err(safe_storage_error)?;
    let transport = FilesystemBackupTransport::new(destination).map_err(safe_storage_error)?;
    let metrics = transport
        .publish(&artifact, &signing_key, retention, now_unix_seconds())
        .map_err(safe_storage_error)?;
    println!(
        "backup={} verified={} files={} bytes={} retained={}",
        metrics.backup_id.as_str(),
        metrics.verified_after_upload,
        metrics.file_count,
        metrics.total_bytes,
        metrics.retained_backups
    );
    Ok(())
}

fn inspect(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let artifact = absolute_path(arguments, "artifact")?;
    let signing_key = signing_key(arguments)?;
    let manifest = inspect_backup(&artifact, &signing_key).map_err(safe_storage_error)?;
    println!(
        "backup={} service={} database={} format={} created={} files={} tenants={}",
        manifest.backup_id.as_str(),
        manifest.identity.service,
        manifest.identity.database_id,
        manifest.database_format_version,
        manifest.created_at_unix_seconds,
        manifest.files.len(),
        manifest.tenants.len()
    );
    Ok(())
}

fn restore(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let artifact = absolute_path(arguments, "artifact")?;
    let target = absolute_path(arguments, "target")?;
    let identity = identity(arguments)?;
    let now = optional_u64(arguments, "now")?.unwrap_or_else(now_unix_seconds);
    let maximum_age = optional_u64(arguments, "maximum-age-seconds")?.unwrap_or(86_400);
    if flag(arguments, "dry-run") {
        println!(
            "dry-run: restore artifact={} empty_target={} owner={} database={} maximum_age_seconds={}",
            artifact.display(),
            target.display(),
            identity.service,
            identity.database_id,
            maximum_age
        );
        return Ok(());
    }
    require_confirmation(arguments, "RESTORE")?;
    let signing_key = signing_key(arguments)?;
    let report = block_on(restore_backup(
        &artifact,
        &target,
        &RestorePolicy {
            expected_identity: identity,
            now_unix_seconds: now,
            maximum_backup_age: Duration::from_secs(maximum_age),
        },
        &signing_key,
    ))
    .map_err(safe_storage_error)?;
    println!(
        "restored backup={} state={:?} files_verified={} tenants_verified={}; run promote after fencing the prior owner",
        report.backup_id.as_str(),
        report.state,
        report.files_verified,
        report.tenants_verified
    );
    Ok(())
}

fn transition(
    arguments: &BTreeMap<String, String>,
    confirmation: &str,
    target: VolumeState,
) -> Result<(), String> {
    let path = absolute_path(arguments, "database-path")?;
    let identity = identity(arguments)?;
    if flag(arguments, "dry-run") {
        println!(
            "dry-run: transition path={} owner={} database={} target={:?}",
            path.display(),
            identity.service,
            identity.database_id,
            target
        );
        return Ok(());
    }
    require_confirmation(arguments, confirmation)?;
    let marker = match target {
        VolumeState::Fenced => fence_production_volume(&path, &identity),
        VolumeState::Active => promote_restored_volume(&path, &identity),
        _ => unreachable!("only operator transitions are routed here"),
    }
    .map_err(safe_storage_error)?;
    println!("volume state={:?}", marker.state);
    Ok(())
}

fn activate(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let path = absolute_path(arguments, "database-path")?;
    let identity = identity(arguments)?;
    if flag(arguments, "dry-run") {
        println!(
            "dry-run: activate fenced nonempty volume path={} owner={} database={}",
            path.display(),
            identity.service,
            identity.database_id
        );
        return Ok(());
    }
    require_confirmation(arguments, "ACTIVATE")?;
    activate_fenced_volume(&path, &identity).map_err(safe_storage_error)?;
    let opened = block_on(ProductionRocksDb::open(production_config(
        path.clone(),
        identity.clone(),
    )));
    match opened {
        Ok(storage) => {
            block_on(storage.graceful_shutdown()).map_err(safe_storage_error)?;
            println!("volume state=Active recovery=verified nonempty=true");
            Ok(())
        }
        Err(error) => {
            let _ignored = fence_production_volume(&path, &identity);
            Err(safe_storage_error(error))
        }
    }
}

fn production_config(
    database_path: PathBuf,
    identity: ProductionVolumeIdentity,
) -> ProductionRocksDbConfig {
    ProductionRocksDbConfig {
        database_path,
        identity,
        maximum_batch_operations: NonZeroUsize::new(10_000).expect("constant"),
        maximum_scan_items: NonZeroUsize::new(10_000).expect("constant"),
        transaction_lock_timeout: Duration::from_secs(2),
        transaction_expiration: Duration::from_secs(30),
        disk_warning_free_bytes: 512 * 1024 * 1024,
        disk_critical_free_bytes: 256 * 1024 * 1024,
    }
}

fn identity(arguments: &BTreeMap<String, String>) -> Result<ProductionVolumeIdentity, String> {
    ProductionVolumeIdentity::new(
        required(arguments, "service")?,
        required(arguments, "database-id")?,
    )
    .map_err(safe_storage_error)
}

fn signing_key(arguments: &BTreeMap<String, String>) -> Result<ManifestSigningKey, String> {
    let path = absolute_path(arguments, "signing-key-file")?;
    let metadata =
        fs::metadata(&path).map_err(|_| "signing-key file is unavailable".to_string())?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err("signing-key file must be a regular file no larger than 64 KiB".into());
    }
    let material = fs::read(path).map_err(|_| "signing-key file is unavailable".to_string())?;
    ManifestSigningKey::new(material).map_err(safe_storage_error)
}

fn parse_arguments() -> Result<(String, BTreeMap<String, String>), String> {
    let mut values = std::env::args().skip(1);
    let command = values.next().ok_or_else(|| "missing command".to_string())?;
    let mut arguments = BTreeMap::new();
    for value in values {
        let Some(flag) = value.strip_prefix("--") else {
            return Err("arguments must use --name=value or --flag".into());
        };
        let (name, value) = flag.split_once('=').unwrap_or((flag, "true"));
        if name.is_empty()
            || arguments
                .insert(name.to_owned(), value.to_owned())
                .is_some()
        {
            return Err("argument names must be non-empty and unique".into());
        }
    }
    Ok((command, arguments))
}

fn required<'a>(arguments: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    arguments
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing --{name}"))
}

fn flag(arguments: &BTreeMap<String, String>, name: &str) -> bool {
    arguments.get(name).is_some_and(|value| value == "true")
}

fn absolute_path(arguments: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required(arguments, name)?);
    if !path.is_absolute() || path == Path::new("/") {
        return Err(format!("--{name} must be an absolute non-root path"));
    }
    Ok(path)
}

fn require_confirmation(
    arguments: &BTreeMap<String, String>,
    expected: &str,
) -> Result<(), String> {
    if arguments
        .get("confirm")
        .is_none_or(|actual| actual != expected)
    {
        return Err(format!("operation requires --confirm={expected}"));
    }
    Ok(())
}

fn optional_u64(arguments: &BTreeMap<String, String>, name: &str) -> Result<Option<u64>, String> {
    arguments
        .get(name)
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("--{name} must be an unsigned integer"))
        })
        .transpose()
}

fn optional_u32(arguments: &BTreeMap<String, String>, name: &str) -> Result<Option<u32>, String> {
    arguments
        .get(name)
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("--{name} must be an unsigned integer"))
        })
        .transpose()
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn safe_storage_error(error: mako_storage::StorageError) -> String {
    format!(
        "kind={:?} operation={} retryable={} message={}",
        error.kind, error.operation, error.retryable, error.message
    )
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn recovery_qualification_is_nonzero_and_idempotent() {
        let directory = tempdir().expect("temporary directory");
        let database = directory.path().join("database");
        fs::create_dir(&database).expect("database directory");
        let identity =
            ProductionVolumeIdentity::new("mako-data-plane", "qualification").expect("identity");
        provision_production_volume(&database, identity).expect("provision volume");
        let arguments = BTreeMap::from([
            (
                "database-path".to_owned(),
                database.to_string_lossy().into_owned(),
            ),
            ("service".to_owned(), "mako-data-plane".to_owned()),
            ("database-id".to_owned(), "qualification".to_owned()),
            ("project".to_owned(), "prj_qualification".to_owned()),
            ("environment".to_owned(), "env_qualification".to_owned()),
            ("confirm".to_owned(), "QUALIFY_RECOVERY".to_owned()),
        ]);

        qualify_recovery(&arguments).expect("first qualification");
        qualify_recovery(&arguments).expect("idempotent qualification");
    }
}
