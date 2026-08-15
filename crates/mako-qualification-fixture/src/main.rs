//! Offline, idempotent data-plane fixture for hosted beta qualification.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    io::{self, Read},
    num::NonZeroUsize,
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use futures::executor::block_on;
use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
use mako_documents::{
    CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, DocumentEngine,
    PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion,
};
use mako_identity::{
    CredentialDigest, IdentityStore, ProjectCredentialId, VerifiedProjectCredential,
};
use mako_policy::{
    DocumentOperation, PolicyCompiler, PolicyEffect, PolicyRule, PolicyRuleId, PolicySet,
    PolicyState, PolicyStore, PolicyVersion,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ProductionRocksDb,
    ProductionRocksDbConfig, ProductionVolumeIdentity, TenantKeyspace, WriteBatch,
};
use serde_json::{Value, json};

fn main() -> ExitCode {
    match run(std::env::args().skip(1)) {
        Ok((tenant, collection)) => {
            println!(
                "installed idempotent qualification collection {}/{}/{}",
                tenant.project_id(),
                tenant.environment_id(),
                collection
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("qualification fixture was not installed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<(TenantScope, CollectionId), String> {
    let options = parse_options(arguments)?;
    let project = ProjectId::parse(required(&options, "project")?)
        .map_err(|_| "project is invalid".to_owned())?;
    let environment = EnvironmentId::parse(required(&options, "environment")?)
        .map_err(|_| "environment is invalid".to_owned())?;
    let collection = CollectionId::parse(required(&options, "collection")?)
        .map_err(|_| "collection is invalid".to_owned())?;
    let credential_id = ProjectCredentialId::parse(required(&options, "public-credential-id")?)
        .map_err(|_| "public credential id is invalid".to_owned())?;
    let public_credential = read_public_credential(&credential_id)?;
    let tenant = TenantScope::new(project, environment);
    let database_path = PathBuf::from(required(&options, "database-path")?);
    if !database_path.is_absolute() {
        return Err("database-path must be absolute".to_owned());
    }
    let identity =
        ProductionVolumeIdentity::new("mako-data-plane", required(&options, "database-id")?)
            .map_err(safe_error)?;
    let disk_warning_free_bytes = parse_u64(&options, "disk-warning-free-bytes")?;
    let disk_critical_free_bytes = parse_u64(&options, "disk-critical-free-bytes")?;
    if disk_warning_free_bytes <= disk_critical_free_bytes {
        return Err("disk warning reserve must exceed the critical reserve".to_owned());
    }
    let storage = block_on(ProductionRocksDb::open(ProductionRocksDbConfig {
        database_path,
        identity,
        maximum_batch_operations: NonZeroUsize::new(10_000).expect("positive batch bound"),
        maximum_scan_items: NonZeroUsize::new(10_000).expect("positive scan bound"),
        transaction_lock_timeout: Duration::from_secs(2),
        transaction_expiration: Duration::from_secs(30),
        disk_warning_free_bytes,
        disk_critical_free_bytes,
    }))
    .map_err(safe_error)?;
    let adapter: Arc<dyn KvAdapter> = Arc::new(storage.adapter().clone());
    block_on(install_public_credential(
        Arc::clone(&adapter),
        &tenant,
        &credential_id,
        &public_credential,
        Durability::Sync,
    ))?;
    block_on(install_collection_and_policy(
        Arc::clone(&adapter),
        &tenant,
        &collection,
        Durability::Sync,
    ))?;
    block_on(storage.graceful_shutdown()).map_err(safe_error)?;
    Ok((tenant, collection))
}

fn read_public_credential(credential_id: &ProjectCredentialId) -> Result<String, String> {
    let mut value = String::new();
    io::stdin()
        .take(513)
        .read_to_string(&mut value)
        .map_err(|_| "public credential could not be read from stdin".to_owned())?;
    if value.len() > 512 {
        return Err("public credential from stdin is too long".to_owned());
    }
    validate_public_credential(&value, credential_id)?;
    Ok(value)
}

fn validate_public_credential(
    value: &str,
    expected_id: &ProjectCredentialId,
) -> Result<(), String> {
    let mut parts = value.split('.');
    if parts.next() != Some("mako_pk")
        || parts.next() != Some(expected_id.as_str())
        || !parts.next().is_some_and(|random| {
            random.len() == 64 && random.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || parts.next().is_some()
    {
        return Err("public credential from stdin is invalid".to_owned());
    }
    Ok(())
}

async fn install_public_credential(
    adapter: Arc<dyn KvAdapter>,
    tenant: &TenantScope,
    credential_id: &ProjectCredentialId,
    credential: &str,
    durability: Durability,
) -> Result<(), String> {
    validate_public_credential(credential, credential_id)?;
    let keyspace = TenantKeyspace::new(
        tenant.project_id().as_str().as_bytes(),
        tenant.environment_id().as_str().as_bytes(),
    )
    .map_err(safe_error)?;
    let key = keyspace
        .project_credential_key(credential_id.as_str())
        .map_err(safe_error)?;
    let digest = CredentialDigest::new(blake3::hash(credential.as_bytes()).as_bytes().to_vec())
        .map_err(safe_error)?;
    let expected = serde_json::to_vec(&json!({
        "metadata": {
            "scope": tenant,
            "id": credential_id,
            "kind": "public",
            "serviceScope": null,
            "state": "active",
            "createdAtUnixSeconds": 1,
            "overlapEndsAtUnixSeconds": null,
            "retiredAtUnixSeconds": null
        },
        "digest": digest
    }))
    .map_err(safe_error)?;
    match adapter.get(&key).await.map_err(safe_error)? {
        Some(_) => {
            let identity = IdentityStore::new(Arc::clone(&adapter), tenant, tenant, durability)
                .map_err(safe_error)?;
            match identity
                .verify_project_credential(credential, 2)
                .await
                .map_err(safe_error)?
            {
                Some(VerifiedProjectCredential::Public(public))
                    if public.credential_id() == credential_id =>
                {
                    Ok(())
                }
                _ => Err("an incompatible public credential already exists".to_owned()),
            }
        }
        None => {
            let mut batch = WriteBatch::new();
            batch.put(&key, &expected);
            let result = adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::Missing { key }],
                    batch,
                    durability,
                })
                .await
                .map_err(safe_error)?;
            if result != CompareAndWriteResult::Applied {
                return Err("public credential changed concurrently".to_owned());
            }
            Ok(())
        }
    }
}

async fn install_collection_and_policy(
    adapter: Arc<dyn KvAdapter>,
    tenant: &TenantScope,
    collection: &CollectionId,
    durability: Durability,
) -> Result<(), String> {
    let scope = CollectionScope::new(tenant.clone(), collection.clone());
    let metadata = qualification_metadata(collection.clone())?;
    let scoped = DocumentEngine::new(Arc::clone(&adapter))
        .scope_collection(tenant, scope.clone())
        .map_err(safe_error)?;
    match scoped.collection_metadata().await.map_err(safe_error)? {
        Some(existing) if existing == metadata => {}
        Some(_) => return Err("an incompatible collection already exists".to_owned()),
        None => {
            let key = scoped.collection_metadata_key().map_err(safe_error)?;
            let mut batch = WriteBatch::new();
            batch.put(&key, metadata.encode().map_err(safe_error)?);
            let result = adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::Missing { key }],
                    batch,
                    durability,
                })
                .await
                .map_err(safe_error)?;
            if result != CompareAndWriteResult::Applied {
                return Err("collection metadata changed concurrently".to_owned());
            }
        }
    }

    let store =
        PolicyStore::new(Arc::clone(&adapter), tenant, scope, durability).map_err(safe_error)?;
    let version = PolicyVersion::new(1).map_err(safe_error)?;
    let expected = qualification_policy(tenant, collection)?;
    match store.policy_version(version).await.map_err(safe_error)? {
        Some(existing)
            if existing.encode().map_err(safe_error)?
                == expected.encode().map_err(safe_error)? => {}
        Some(existing) if existing.state() == PolicyState::Active => {
            let active = store.active_policy().await.map_err(safe_error)?;
            if active
                .as_ref()
                .map(|policy| policy.encode())
                .transpose()
                .map_err(safe_error)?
                != Some(existing.encode().map_err(safe_error)?)
            {
                return Err("an incompatible active policy already exists".to_owned());
            }
            return Ok(());
        }
        Some(_) => return Err("an incompatible qualification policy version exists".to_owned()),
        None => store.create_draft(&expected).await.map_err(safe_error)?,
    }
    store
        .activate(version, &qualification_schema(), &PolicyCompiler::default())
        .await
        .map_err(safe_error)?;
    Ok(())
}

fn qualification_metadata(collection: CollectionId) -> Result<CollectionMetadata, String> {
    CollectionMetadata::new(
        collection,
        CollectionMetadataVersion::new(1).map_err(safe_error)?,
        SchemaVersion::new(1).map_err(safe_error)?,
        qualification_schema(),
        PrimaryKeyDefinition::field("id").map_err(safe_error)?,
        SchemaCompatibility::Compatible,
        CollectionLifecycle::Active,
    )
    .map_err(safe_error)
}

fn qualification_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "owner_id", "value"],
        "properties": {
            "id": {"type": "string", "minLength": 1, "maxLength": 128},
            "owner_id": {"type": "string", "minLength": 1, "maxLength": 128},
            "value": {"type": "string", "maxLength": 4096}
        },
        "additionalProperties": false
    })
}

fn qualification_policy(
    tenant: &TenantScope,
    collection: &CollectionId,
) -> Result<PolicySet, String> {
    let rule = |id, operations, expression| {
        PolicyRule::new(
            PolicyRuleId::parse(id).map_err(safe_error)?,
            PolicyEffect::Allow,
            operations,
            expression,
        )
        .map_err(safe_error)
    };
    PolicySet::new(
        CollectionScope::new(tenant.clone(), collection.clone()),
        PolicyVersion::new(1).map_err(safe_error)?,
        PolicyState::Draft,
        [
            rule(
                "owner-create",
                [DocumentOperation::Create],
                "new.owner_id == identity.user_id",
            )?,
            rule(
                "owner-read",
                [DocumentOperation::Read],
                "old.owner_id == identity.user_id",
            )?,
            rule(
                "owner-update",
                [DocumentOperation::Update],
                "old.owner_id == identity.user_id && new.owner_id == identity.user_id",
            )?,
            rule(
                "owner-delete",
                [DocumentOperation::Delete],
                "old.owner_id == identity.user_id",
            )?,
        ],
        [],
    )
    .map_err(safe_error)
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<BTreeMap<String, String>, String> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let mut chunks = arguments.chunks_exact(2);
    let mut options = BTreeMap::new();
    for pair in &mut chunks {
        let name = pair[0]
            .strip_prefix("--")
            .ok_or_else(|| "options must use --name value pairs".to_owned())?;
        if ![
            "collection",
            "database-id",
            "database-path",
            "disk-critical-free-bytes",
            "disk-warning-free-bytes",
            "environment",
            "project",
            "public-credential-id",
        ]
        .contains(&name)
            || options.insert(name.to_owned(), pair[1].clone()).is_some()
        {
            return Err("options are invalid or duplicated".to_owned());
        }
    }
    if !chunks.remainder().is_empty() {
        return Err("options must use --name value pairs".to_owned());
    }
    Ok(options)
}

fn parse_u64(options: &BTreeMap<String, String>, name: &str) -> Result<u64, String> {
    required(options, name)?
        .parse::<u64>()
        .map_err(|_| format!("--{name} must be an integer"))
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("--{name} is required"))
}

fn safe_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_storage::MemoryAdapter;

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_qualification").expect("project"),
            EnvironmentId::parse("env_qualification").expect("environment"),
        )
    }

    #[test]
    fn fixture_is_idempotent_and_installs_an_active_owner_policy() {
        block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let tenant = tenant();
            let collection = CollectionId::parse("documents").expect("collection");
            let credential_id = ProjectCredentialId::parse("qualification_public").expect("id");
            let credential = format!("mako_pk.qualification_public.{}", "a".repeat(64));
            install_public_credential(
                Arc::clone(&adapter),
                &tenant,
                &credential_id,
                &credential,
                Durability::Memory,
            )
            .await
            .expect("first credential install");
            install_public_credential(
                Arc::clone(&adapter),
                &tenant,
                &credential_id,
                &credential,
                Durability::Memory,
            )
            .await
            .expect("second credential install");
            let identity =
                IdentityStore::new(Arc::clone(&adapter), &tenant, &tenant, Durability::Memory)
                    .expect("identity store");
            assert!(
                identity
                    .verify_project_credential(&credential, 2)
                    .await
                    .expect("verify credential")
                    .is_some()
            );
            install_collection_and_policy(
                Arc::clone(&adapter),
                &tenant,
                &collection,
                Durability::Memory,
            )
            .await
            .expect("first install");
            install_collection_and_policy(
                Arc::clone(&adapter),
                &tenant,
                &collection,
                Durability::Memory,
            )
            .await
            .expect("second install");
            let scope = CollectionScope::new(tenant.clone(), collection);
            let store = PolicyStore::new(adapter, &tenant, scope, Durability::Memory)
                .expect("policy store");
            assert_eq!(
                store
                    .active_policy()
                    .await
                    .expect("active policy")
                    .expect("policy")
                    .state(),
                PolicyState::Active
            );
        });
    }
}
