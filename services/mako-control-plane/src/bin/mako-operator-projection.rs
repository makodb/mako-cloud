//! Rebuilds and validates operator read projections against an offline RocksDB snapshot.

#![forbid(unsafe_code)]

use std::{collections::BTreeMap, env, num::NonZeroUsize, path::PathBuf, process::ExitCode};

use futures::executor::block_on;
use mako_control_plane::{ActivityRecord, ControlKeyspace, LifecycleState, ProjectRecord};
use mako_storage::{
    Durability, KeyRange, KvAdapter, RocksDbAdapter, RocksDbConfig, ScanDirection, ScanRequest,
    WriteBatch,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const PAGE_SIZE: usize = 100;
const MAXIMUM_RECORDS: usize = 100_000;
const CONFIRMATION: &str = "QUALIFY_OPERATOR_PROJECTIONS_ON_OFFLINE_SNAPSHOT";

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("operator projection qualification failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<String, String> {
    let options = parse_options(arguments)?;
    let snapshot_path = PathBuf::from(required(&options, "snapshot-path")?);
    if !snapshot_path.is_absolute()
        || snapshot_path.parent().is_none()
        || !snapshot_path.join("CURRENT").is_file()
    {
        return Err("--snapshot-path must be an absolute offline RocksDB snapshot".to_owned());
    }
    let snapshot_id = required(&options, "snapshot-id")?;
    if snapshot_id.len() < 8
        || snapshot_id.len() > 128
        || !snapshot_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err("--snapshot-id is invalid".to_owned());
    }
    if required(&options, "confirm")? != CONFIRMATION {
        return Err(format!("--confirm must equal {CONFIRMATION}"));
    }
    let mut config = RocksDbConfig::new(snapshot_path);
    config.create_if_missing = false;
    let adapter = RocksDbAdapter::open(config).map_err(|_| "snapshot could not be opened")?;
    let evidence = block_on(qualify(&adapter, snapshot_id))?;
    serde_json::to_string_pretty(&evidence).map_err(|_| "evidence could not be encoded".to_owned())
}

async fn qualify(adapter: &RocksDbAdapter, snapshot_id: &str) -> Result<Evidence, String> {
    let projects = scan_records::<ProjectRecord>(adapter, ControlKeyspace::projects_range())
        .await
        .map_err(|_| "project source scan failed")?;
    let mut source_hasher = blake3::Hasher::new();
    for (_, project, bytes) in &projects {
        source_hasher.update(bytes);
        let index = TenantSearchIndexRecord {
            schema_version: 1,
            project_id: project.id().as_str().to_owned(),
            organization_id: project.organization_id().as_str().to_owned(),
            normalized_project_name: project.name().to_ascii_lowercase(),
            region: project.region().to_owned(),
            lifecycle: project.lifecycle(),
            source_updated_at_unix_seconds: project.updated_at_unix_seconds(),
        };
        let mut batch = WriteBatch::new();
        batch.put(
            &ControlKeyspace::operator_tenant_search_key(project.id())
                .map_err(|_| "tenant-search index key is invalid")?,
            serde_json::to_vec(&index).map_err(|_| "tenant-search index is invalid")?,
        );
        adapter
            .write(batch, Durability::Sync)
            .await
            .map_err(|_| "tenant-search index write failed")?;
    }
    let projected = scan_records::<TenantSearchIndexRecord>(
        adapter,
        ControlKeyspace::operator_tenant_search_range(),
    )
    .await
    .map_err(|_| "tenant-search projection scan failed")?;
    if projected.len() != projects.len() {
        return Err("tenant-search source and projection counts differ".to_owned());
    }
    let mut projected_hasher = blake3::Hasher::new();
    for (_, _, bytes) in &projected {
        projected_hasher.update(bytes);
    }

    let activity =
        scan_records::<ActivityRecord>(adapter, ControlKeyspace::operator_activity_range())
            .await
            .map_err(|_| "activity projection scan failed")?;
    if activity
        .iter()
        .any(|(_, record, _)| !record.integrity_valid())
    {
        return Err("activity projection contains an integrity gap".to_owned());
    }
    let mut activity_hasher = blake3::Hasher::new();
    for (_, _, bytes) in &activity {
        activity_hasher.update(bytes);
    }

    Ok(Evidence {
        schema_version: 1,
        snapshot_id: snapshot_id.to_owned(),
        tenant_search: ProjectionResult {
            source_count: projects.len(),
            projected_count: projected.len(),
            source_checksum: source_hasher.finalize().to_hex().to_string(),
            projected_checksum: projected_hasher.finalize().to_hex().to_string(),
            complete: true,
        },
        activity: ActivityResult {
            projected_count: activity.len(),
            checksum: activity_hasher.finalize().to_hex().to_string(),
            integrity_gaps: 0,
            complete: true,
        },
    })
}

async fn scan_records<T: DeserializeOwned>(
    adapter: &RocksDbAdapter,
    range: Result<KeyRange, impl std::fmt::Display>,
) -> Result<Vec<(Vec<u8>, T, Vec<u8>)>, ()> {
    let mut range = range.map_err(|_| ())?;
    let end = range.end_exclusive.clone();
    let mut records = Vec::new();
    loop {
        let page = adapter
            .scan(ScanRequest::new(
                range.clone(),
                ScanDirection::Forward,
                NonZeroUsize::new(PAGE_SIZE).expect("positive page size"),
            ))
            .await
            .map_err(|_| ())?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        let last_key = page.last().map(|value| value.key.clone()).ok_or(())?;
        for value in page {
            let record = serde_json::from_slice(&value.value).map_err(|_| ())?;
            records.push((value.key, record, value.value));
        }
        if records.len() > MAXIMUM_RECORDS {
            return Err(());
        }
        if page_len < PAGE_SIZE {
            break;
        }
        let mut next = last_key;
        next.push(0);
        range = KeyRange::new(next, end.clone()).map_err(|_| ())?;
    }
    Ok(records)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TenantSearchIndexRecord {
    schema_version: u8,
    project_id: String,
    organization_id: String,
    normalized_project_name: String,
    region: String,
    lifecycle: LifecycleState,
    source_updated_at_unix_seconds: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Evidence {
    schema_version: u8,
    snapshot_id: String,
    tenant_search: ProjectionResult,
    activity: ActivityResult,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectionResult {
    source_count: usize,
    projected_count: usize,
    source_checksum: String,
    projected_checksum: String,
    complete: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityResult {
    projected_count: usize,
    checksum: String,
    integrity_gaps: usize,
    complete: bool,
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<BTreeMap<String, String>, String> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let mut options = BTreeMap::new();
    let mut chunks = arguments.chunks_exact(2);
    for pair in &mut chunks {
        let name = pair[0]
            .strip_prefix("--")
            .ok_or_else(|| "options must use --name value pairs".to_owned())?;
        if name.is_empty() || options.insert(name.to_owned(), pair[1].clone()).is_some() {
            return Err("options must be unique --name value pairs".to_owned());
        }
    }
    if !chunks.remainder().is_empty() {
        return Err("options must use --name value pairs".to_owned());
    }
    let expected = ["confirm", "snapshot-id", "snapshot-path"];
    if options.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err("an unsupported option was provided".to_owned());
    }
    Ok(options)
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("--{name} is required"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_explicit_and_confirmation_is_not_optional() {
        let options = parse_options([
            "--snapshot-path".to_owned(),
            "/tmp/operator-snapshot".to_owned(),
            "--snapshot-id".to_owned(),
            "snapshot-example".to_owned(),
            "--confirm".to_owned(),
            CONFIRMATION.to_owned(),
        ])
        .expect("options");
        assert_eq!(required(&options, "confirm"), Ok(CONFIRMATION));
        assert!(parse_options(["--database-path".to_owned(), "/live".to_owned()]).is_err());
    }
}
