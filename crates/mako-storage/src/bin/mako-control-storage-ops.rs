use std::{
    collections::BTreeMap,
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mako_storage::{
    CONTROL_MIGRATION_FORMAT_VERSION, ControlBackupSigningKey, ControlMigrationPlan,
    create_control_sqlite_backup, inspect_control_rocks_checkpoint, inspect_control_sqlite,
    inspect_control_sqlite_backup, migrate_control_rocks_to_sqlite, promote_control_sqlite_restore,
    restore_control_sqlite_backup, write_control_checkpoint_fence,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("control storage operation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let (command, arguments) = parse_arguments()?;
    match command.as_str() {
        "plan-migration" => plan_migration(&arguments),
        "fence-checkpoint" => fence_checkpoint(&arguments),
        "inspect-rocks" => inspect_rocks(&arguments),
        "inspect-sqlite" => inspect_sqlite(&arguments),
        "migrate" => migrate(&arguments),
        "backup" => backup(&arguments),
        "verify-backup" => verify_backup(&arguments),
        "restore" => restore(&arguments),
        "promote" => promote(&arguments),
        _ => Err("expected plan-migration, fence-checkpoint, inspect-rocks, inspect-sqlite, migrate, backup, verify-backup, restore, or promote".into()),
    }
}

fn plan_migration(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let plan = migration_plan(arguments)?;
    let output = absolute(arguments, "output")?;
    write_json_new(&output, &plan)?;
    println!(
        "migration plan={} format={} source={} temporary={} target={} identity={} release={} configuration={}",
        output.display(),
        plan.format_version,
        plan.source_checkpoint.display(),
        plan.temporary_target.display(),
        plan.final_target.display(),
        plan.database_identity,
        plan.expected_release_sha256,
        plan.expected_configuration_sha256,
    );
    Ok(())
}

fn fence_checkpoint(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    require_confirmation(arguments, "FENCE_CONTROL_CHECKPOINT")?;
    let source = absolute(arguments, "source-checkpoint")?;
    let digest = required(arguments, "source-sha256")?;
    write_control_checkpoint_fence(&source, digest).map_err(safe_error)?;
    println!(
        "control checkpoint fenced: source={} digest={digest}",
        source.display()
    );
    Ok(())
}

fn inspect_rocks(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let source = absolute(arguments, "source-checkpoint")?;
    let inventory = inspect_control_rocks_checkpoint(&source).map_err(safe_error)?;
    print_inventory("rocks-checkpoint", &inventory);
    Ok(())
}

fn inspect_sqlite(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let database = absolute(arguments, "database")?;
    let identity = required(arguments, "identity")?;
    let inventory = inspect_control_sqlite(&database, identity).map_err(safe_error)?;
    print_inventory("control-sqlite", &inventory);
    Ok(())
}

fn migrate(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    require_confirmation(arguments, "MIGRATE_CONTROL_TO_SQLITE")?;
    let plan_path = absolute(arguments, "plan")?;
    let plan: ControlMigrationPlan = read_json(&plan_path)?;
    let receipt = migrate_control_rocks_to_sqlite(&plan).map_err(safe_error)?;
    println!(
        "migration complete: records={} checksum={} schema={} receipt={} target={}",
        receipt.target.record_count,
        receipt.target.framed_blake3,
        receipt.database_format_version,
        plan.receipt_path.display(),
        plan.final_target.display(),
    );
    Ok(())
}

fn backup(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let database = absolute(arguments, "database")?;
    let staging = absolute(arguments, "staging")?;
    let publish = absolute(arguments, "publish")?;
    let identity = required(arguments, "identity")?;
    let backup_id = required(arguments, "backup-id")?;
    let release = required(arguments, "release-sha256")?;
    let created = optional_u64(arguments, "created-at")?.unwrap_or_else(now);
    let retention = NonZeroU32::new(optional_u32(arguments, "retention")?.unwrap_or(14))
        .ok_or_else(|| "--retention must be positive".to_string())?;
    let key = signing_key(arguments)?;
    let manifest = create_control_sqlite_backup(
        &database, identity, &staging, &publish, backup_id, release, created, retention, &key,
    )
    .map_err(safe_error)?;
    println!(
        "control backup complete: backup={} records={} bytes={} checksum={} integrity={}",
        manifest.backup_id,
        manifest.inventory.record_count,
        manifest.database_size_bytes,
        manifest.inventory.framed_blake3,
        manifest.integrity_verified,
    );
    Ok(())
}

fn verify_backup(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    let artifact = absolute(arguments, "artifact")?;
    let key = signing_key(arguments)?;
    let manifest = inspect_control_sqlite_backup(&artifact, &key).map_err(safe_error)?;
    println!(
        "control backup verified: backup={} identity={} schema={} records={} high_water={} release={}",
        manifest.backup_id,
        manifest.database_identity,
        manifest.database_format_version,
        manifest.inventory.record_count,
        manifest.durable_high_water,
        manifest.release_sha256,
    );
    Ok(())
}

fn restore(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    require_confirmation(arguments, "RESTORE_CONTROL_SQLITE")?;
    let artifact = absolute(arguments, "artifact")?;
    let target = absolute(arguments, "target")?;
    let identity = required(arguments, "identity")?;
    let release = required(arguments, "release-sha256")?;
    let now_value = optional_u64(arguments, "now")?.unwrap_or_else(now);
    let maximum_age = optional_u64(arguments, "maximum-age-seconds")?.unwrap_or(86_400);
    let key = signing_key(arguments)?;
    let report = restore_control_sqlite_backup(
        &artifact,
        &target,
        identity,
        release,
        now_value,
        Duration::from_secs(maximum_age),
        &key,
    )
    .map_err(safe_error)?;
    println!(
        "control restore verified: backup={} records={} promotable={} target={}",
        report.backup_id,
        report.inventory.record_count,
        report.promotable,
        report.restored_path.display(),
    );
    Ok(())
}

fn promote(arguments: &BTreeMap<String, String>) -> Result<(), String> {
    require_confirmation(arguments, "PROMOTE_CONTROL_SQLITE")?;
    let restored = absolute(arguments, "restored")?;
    let live = absolute(arguments, "live")?;
    let identity = required(arguments, "identity")?;
    let inventory =
        promote_control_sqlite_restore(&restored, &live, identity).map_err(safe_error)?;
    println!(
        "control restore promoted: records={} checksum={} live={}",
        inventory.record_count,
        inventory.framed_blake3,
        live.display(),
    );
    Ok(())
}

fn migration_plan(arguments: &BTreeMap<String, String>) -> Result<ControlMigrationPlan, String> {
    Ok(ControlMigrationPlan {
        format_version: CONTROL_MIGRATION_FORMAT_VERSION,
        source_checkpoint: absolute(arguments, "source-checkpoint")?,
        temporary_target: absolute(arguments, "temporary-target")?,
        final_target: absolute(arguments, "final-target")?,
        lock_path: absolute(arguments, "lock-path")?,
        receipt_path: absolute(arguments, "receipt")?,
        database_identity: required(arguments, "identity")?.to_owned(),
        expected_release_sha256: required(arguments, "release-sha256")?.to_owned(),
        expected_configuration_sha256: required(arguments, "configuration-sha256")?.to_owned(),
        source_checkpoint_sha256: required(arguments, "source-sha256")?.to_owned(),
    })
}

fn parse_arguments() -> Result<(String, BTreeMap<String, String>), String> {
    let mut values = std::env::args().skip(1);
    let command = values
        .next()
        .ok_or_else(|| "a command is required".to_string())?;
    let mut arguments = BTreeMap::new();
    while let Some(argument) = values.next() {
        let raw = argument
            .strip_prefix("--")
            .ok_or_else(|| "arguments must use --name value".to_string())?;
        if let Some((name, value)) = raw.split_once('=') {
            if name.is_empty() || value.is_empty() {
                return Err("arguments must use --name=value with non-empty values".to_string());
            }
            if arguments
                .insert(name.to_owned(), value.to_owned())
                .is_some()
            {
                return Err(format!("--{name} cannot be repeated"));
            }
            continue;
        }
        let name = raw;
        if name == "dry-run" {
            arguments.insert(name.to_owned(), "true".into());
            continue;
        }
        let value = values
            .next()
            .ok_or_else(|| format!("--{name} requires a value"))?;
        if arguments.insert(name.to_owned(), value).is_some() {
            return Err(format!("--{name} cannot be repeated"));
        }
    }
    Ok((command, arguments))
}

fn required<'a>(arguments: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    arguments
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("--{name} is required"))
}

fn absolute(arguments: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required(arguments, name)?);
    if !path.is_absolute() {
        return Err(format!("--{name} must be absolute"));
    }
    Ok(path)
}

fn optional_u64(arguments: &BTreeMap<String, String>, name: &str) -> Result<Option<u64>, String> {
    arguments
        .get(name)
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("--{name} must be an integer"))
        })
        .transpose()
}

fn optional_u32(arguments: &BTreeMap<String, String>, name: &str) -> Result<Option<u32>, String> {
    arguments
        .get(name)
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("--{name} must be an integer"))
        })
        .transpose()
}

fn require_confirmation(arguments: &BTreeMap<String, String>, action: &str) -> Result<(), String> {
    if required(arguments, "confirm")? != action {
        return Err(format!("--confirm must equal {action}"));
    }
    Ok(())
}

fn signing_key(arguments: &BTreeMap<String, String>) -> Result<ControlBackupSigningKey, String> {
    let material = if let Some(name) = arguments.get("signing-key-env") {
        std::env::var(name)
            .map_err(|_| "backup signing-key environment variable is unavailable".to_string())?
            .into_bytes()
    } else if let Some(path) = arguments.get("signing-key-file") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("--signing-key-file must be absolute".into());
        }
        fs::read(path).map_err(|_| "backup signing-key file is unavailable".to_string())?
    } else {
        return Err("--signing-key-env or --signing-key-file is required".into());
    };
    ControlBackupSigningKey::new(material).map_err(safe_error)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|_| "JSON input could not be read".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "JSON input is invalid".to_string())
}

fn write_json_new<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| "output directory could not be created".to_string())?;
    }
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| "output could not be encoded".to_string())?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    use std::io::Write;
    let mut file = options
        .open(path)
        .map_err(|_| "output already exists or cannot be created".to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| "output could not be synchronized".to_string())
}

fn print_inventory(label: &str, inventory: &mako_storage::KvInventory) {
    let prefixes = serde_json::to_string(&inventory.prefix_counts)
        .unwrap_or_else(|_| "{\"inventory\":\"unavailable\"}".to_owned());
    println!(
        "inventory={} records={} checksum={} prefixes={}",
        label, inventory.record_count, inventory.framed_blake3, prefixes,
    );
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn safe_error(error: mako_storage::StorageError) -> String {
    format!(
        "class={:?} operation={} retryable={} detail={}",
        error.kind, error.operation, error.retryable, error.message
    )
}
