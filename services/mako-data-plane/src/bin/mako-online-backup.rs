//! Back up the running data plane without stopping it.
//!
//! The data plane is the only process that can open its RocksDB database, so
//! this worker asks it, over the signed loopback protocol, for a linked
//! checkpoint of itself. It then finishes the backup from that checkpoint --
//! copy to staging, release the checkpoint, recover sequencers on the copy,
//! inventory, sign, publish and verify -- while the data plane keeps serving.
//! It never opens the live database.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    fs,
    io::Read as _,
    net::SocketAddr,
    num::NonZeroU32,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::executor::block_on;
use mako_api::{EnvironmentId, ProjectId, TenantScope};
use mako_internal_rpc::{
    BACKUP_WORKER_ENVIRONMENT_ID, BACKUP_WORKER_PROJECT_ID, DeploymentKey, InternalCaller,
    InternalClientError, InternalHttpClient, InternalHttpClientConfig, InternalRoute,
    StorageCheckpointRequest, StorageCheckpointResponse,
};
use mako_storage::{
    BackupId, BackupRequest, FilesystemBackupTransport, ManifestSigningKey,
    ProductionVolumeIdentity, StorageError, finalize_online_backup, online_checkpoint_root,
};

/// Flushing the memtable and linking the database can outlast the default
/// loopback timeout on a busy volume. A timed-out request leaves a checkpoint
/// the next backup sweeps.
const CHECKPOINT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAXIMUM_SECRET_BYTES: u64 = 64 * 1024;

fn main() -> ExitCode {
    match run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("online backup failed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<(), String> {
    let arguments = parse_arguments(arguments)?;
    let endpoint = required(&arguments, "endpoint")?
        .parse::<SocketAddr>()
        .map_err(|_| "--endpoint must be an IP address and port".to_owned())?;
    if !endpoint.ip().is_loopback() {
        return Err("--endpoint must be loopback".to_owned());
    }
    let database_path = absolute_path(&arguments, "database-path")?;
    let staging_root = absolute_path(&arguments, "staging-root")?;
    let destination = absolute_path(&arguments, "destination")?;
    let identity = ProductionVolumeIdentity::new(
        required(&arguments, "service")?,
        required(&arguments, "database-id")?,
    )
    .map_err(safe_storage_error)?;
    let backup_id =
        BackupId::new(required(&arguments, "backup-id")?).map_err(safe_storage_error)?;
    let created_at = optional_u64(&arguments, "created-at")?.unwrap_or_else(now_unix_seconds);
    let retention = NonZeroU32::new(optional_u32(&arguments, "retention")?.unwrap_or(14))
        .ok_or_else(|| "--retention must be positive".to_owned())?;
    if arguments
        .get("dry-run")
        .is_some_and(|value| value == "true")
    {
        println!(
            "dry-run: online checkpoint endpoint={endpoint} path={} backup={} staging={} destination={} retention={retention}",
            database_path.display(),
            backup_id.as_str(),
            staging_root.display(),
            destination.display(),
        );
        return Ok(());
    }
    let secret = read_owner_only(&absolute_path(&arguments, "secret-file")?, "secret")?;
    let secret = std::str::from_utf8(&secret)
        .map_err(|_| "secret file is invalid".to_owned())?
        .trim_end_matches(['\r', '\n']);
    let key = DeploymentKey::derive(secret).map_err(|_| "secret file is invalid".to_owned())?;
    let signing_key = ManifestSigningKey::new(read_owner_only(
        &absolute_path(&arguments, "signing-key-file")?,
        "signing-key",
    )?)
    .map_err(safe_storage_error)?;

    let started = Instant::now();
    let checkpoint = request_checkpoint(endpoint, key, &backup_id)?;
    let checkpoint_path = validated_checkpoint_path(&checkpoint, &backup_id, &database_path)?;
    let artifact = block_on(finalize_online_backup(
        &database_path,
        &identity,
        &checkpoint_path,
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
        "backup={} verified={} files={} bytes={} retained={} mode=online barrier_wait_ms={} checkpoint_ms={} duration_ms={}",
        metrics.backup_id.as_str(),
        metrics.verified_after_upload,
        metrics.file_count,
        metrics.total_bytes,
        metrics.retained_backups,
        checkpoint.barrier_wait_milliseconds,
        checkpoint.creation_milliseconds,
        started.elapsed().as_millis(),
    );
    Ok(())
}

fn request_checkpoint(
    endpoint: SocketAddr,
    key: DeploymentKey,
    backup_id: &BackupId,
) -> Result<StorageCheckpointResponse, String> {
    let mut config = InternalHttpClientConfig::loopback(endpoint);
    config.io_timeout = CHECKPOINT_REQUEST_TIMEOUT;
    let client = InternalHttpClient::new(config, key, InternalCaller::BackupWorker)
        .map_err(|_| "checkpoint client configuration is invalid".to_owned())?;
    let scope = TenantScope::new(
        ProjectId::parse(BACKUP_WORKER_PROJECT_ID)
            .map_err(|_| "backup worker scope is invalid".to_owned())?,
        EnvironmentId::parse(BACKUP_WORKER_ENVIRONMENT_ID)
            .map_err(|_| "backup worker scope is invalid".to_owned())?,
    );
    let response = client
        .call(
            InternalRoute::StorageCheckpoint,
            &scope,
            &format!("req_backup_{}", backup_id.as_str()),
            backup_id.as_str(),
            &StorageCheckpointRequest {
                backup_id: backup_id.as_str().to_owned(),
            },
        )
        .map_err(|error| match error {
            InternalClientError::Remote { .. } => {
                format!("the data plane refused the checkpoint (retryable): {error}")
            }
            _ => format!("checkpoint request failed: {error}"),
        })?;
    let checkpoint: StorageCheckpointResponse = serde_json::from_slice(&response.body)
        .map_err(|_| "checkpoint response is invalid".to_owned())?;
    if checkpoint.backup_id != backup_id.as_str() {
        return Err("checkpoint response names another backup".to_owned());
    }
    Ok(checkpoint)
}

/// The data plane names the checkpoint it made. Accept only the one directory
/// it makes for this backup: `<database>/.online-checkpoints/<id>.checkpoint`.
fn validated_checkpoint_path(
    checkpoint: &StorageCheckpointResponse,
    backup_id: &BackupId,
    database_path: &Path,
) -> Result<PathBuf, String> {
    let expected =
        online_checkpoint_root(database_path).join(format!("{}.checkpoint", backup_id.as_str()));
    if Path::new(&checkpoint.checkpoint_path) != expected {
        return Err("checkpoint response names an unexpected path".to_owned());
    }
    Ok(expected)
}

fn read_owner_only(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    let mut file = fs::File::open(path).map_err(|_| format!("{label} file could not be opened"))?;
    let metadata = file
        .metadata()
        .map_err(|_| format!("{label} file metadata is unavailable"))?;
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 || metadata.len() > MAXIMUM_SECRET_BYTES
    {
        return Err(format!(
            "{label} file must be a regular owner-only file no larger than 64 KiB"
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| format!("{label} file could not be read"))?;
    Ok(bytes)
}

fn parse_arguments(
    values: impl IntoIterator<Item = String>,
) -> Result<BTreeMap<String, String>, String> {
    const KNOWN: [&str; 13] = [
        "endpoint",
        "secret-file",
        "database-path",
        "service",
        "database-id",
        "backup-id",
        "staging-root",
        "destination",
        "retention",
        "signing-key-file",
        "created-at",
        "dry-run",
        "help",
    ];
    let mut arguments = BTreeMap::new();
    for value in values {
        let Some(flag) = value.strip_prefix("--") else {
            return Err("arguments must use --name=value or --flag".to_owned());
        };
        let (name, value) = flag.split_once('=').unwrap_or((flag, "true"));
        if !KNOWN.contains(&name)
            || arguments
                .insert(name.to_owned(), value.to_owned())
                .is_some()
        {
            return Err(format!("unsupported or repeated argument --{name}"));
        }
    }
    Ok(arguments)
}

fn required<'a>(arguments: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    arguments
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing --{name}"))
}

fn absolute_path(arguments: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required(arguments, name)?);
    if !path.is_absolute() || path == Path::new("/") {
        return Err(format!("--{name} must be an absolute non-root path"));
    }
    Ok(path)
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

fn safe_storage_error(error: StorageError) -> String {
    format!(
        "kind={:?} operation={} retryable={} message={}",
        error.kind, error.operation, error.retryable, error.message
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(path: &str) -> StorageCheckpointResponse {
        StorageCheckpointResponse {
            backup_id: "dp-1".into(),
            checkpoint_path: path.into(),
            barrier_wait_milliseconds: 0,
            creation_milliseconds: 0,
        }
    }

    #[test]
    fn only_the_checkpoint_made_for_this_backup_is_accepted() {
        let id = BackupId::new("dp-1").expect("id");
        let live = Path::new("/srv/mako-data/rocksdb/data-plane");
        assert!(
            validated_checkpoint_path(
                &response("/srv/mako-data/rocksdb/data-plane/.online-checkpoints/dp-1.checkpoint"),
                &id,
                live
            )
            .is_ok()
        );
        for path in [
            "relative/dp-1.checkpoint",
            "/srv/mako-data/rocksdb/data-plane/.online-checkpoints/dp-2.checkpoint",
            "/srv/mako-data/rocksdb/data-plane/dp-1.checkpoint",
            "/srv/mako-data/rocksdb/data-plane/.online-checkpoints/../dp-1.checkpoint",
            "/srv/mako-data/rocksdb/data-plane/.online-checkpoints/dp-1",
        ] {
            assert!(
                validated_checkpoint_path(&response(path), &id, live).is_err(),
                "{path}"
            );
        }
    }

    #[test]
    fn arguments_are_named_known_and_unique() {
        assert!(parse_arguments(["--endpoint=127.0.0.1:8080".to_owned()]).is_ok());
        assert!(parse_arguments(["endpoint".to_owned()]).is_err());
        assert!(parse_arguments(["--unknown=1".to_owned()]).is_err());
        assert!(parse_arguments(["--backup-id=a".to_owned(), "--backup-id=b".to_owned()]).is_err());
    }

    #[test]
    fn a_remote_endpoint_is_refused_before_any_secret_is_read() {
        let error = run([
            "--endpoint=192.0.2.10:8080".to_owned(),
            "--secret-file=/nonexistent".to_owned(),
        ])
        .expect_err("remote endpoint");
        assert_eq!(error, "--endpoint must be loopback");
    }
}
