//! Seed a complete, usable local tenant into the two stores Mako Cloud owns.
//!
//! Hosted developer registration requires mail delivery over authenticated TLS
//! SMTP and an operator wait-list decision, none of which a local development
//! environment has. Without a way past that chain there is no way to reach a
//! working project locally, and so no way to exercise the application happy
//! path at all. This tool writes the tenant directly instead.
//!
//! It is a development tool, never a provisioning path: it refuses to run
//! unless the resolved deployment environment is local, and it writes only to
//! stores it exclusively owns while it runs, which means the services must be
//! stopped. Identifiers are fixed, so repeated runs are deterministic, and
//! every write is check-then-create, so a repeated run neither duplicates nor
//! corrupts existing records.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    process::ExitCode,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use futures::executor::block_on;
use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
use mako_config::{DeploymentEnvironment, ServiceConfig, ServiceKind};
use mako_control_plane::{
    AuthenticationIdentityRecord, ControlAuditEvent, ControlAuditSink, CredentialAdminService,
    DeveloperIdentityId, DeveloperLookupKey, DeveloperPrincipal, DeveloperRegistrationStore,
    DeveloperRoleRecord, EnvironmentRecord, FunctionAdminService, FunctionBundleUpload,
    FunctionConfiguration, FunctionLimits, FunctionName, FunctionSecretEncryptionKey,
    FunctionSourceFile, MembershipRecord, NewFunction, NewFunctionVersion, OrganizationId,
    OrganizationRecord, OrganizationRole, OrganizationStore, ProjectRecord, ProjectStore,
    RuntimeDeploymentClient, RuntimeDeploymentClientConfig, RuntimeSupervisorCredential,
};
use mako_data_plane_service::DataPlaneGraph;
use mako_documents::{
    CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, PrimaryKeyDefinition,
    SchemaCompatibility, SchemaVersion,
};
use mako_identity::{
    Argon2idParameters, KeyEncryptionKey, NormalizedEmail, PasswordPolicy, PasswordService,
    ProjectCredentialId, ProjectCredentialKind,
};
use mako_object_store::MemoryObjectStore;
use mako_policy::{
    DocumentOperation, PolicyCompiler, PolicyEffect, PolicyRule, PolicyRuleId, PolicySet,
    PolicyState, PolicyVersion,
};
use mako_storage::{Durability, KvAdapter, SqliteAdapter, SqliteConfig};
use serde_json::json;

/// Fixed identifiers, so a bootstrapped tenant is the same on every machine and
/// the smoke test can address it without discovery.
const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const DEVELOPER_PASSWORD: &str = "LocalBootstrap1!";
const ORGANIZATION_ID: &str = "org_localboot";
const PROJECT_ID: &str = "prj_localboot";
const ENVIRONMENT_ID: &str = "env_localboot";
const COLLECTION_ID: &str = "todos";
const PUBLIC_KEY_ID: &str = "key_localbootstrap";
const FUNCTION_NAME: &str = "hello";
const FUNCTION_ENTRYPOINT: &str = "index.ts";
const FUNCTION_REGION: &str = "local";
const RUNTIME_VERSION: &str = "v1.74.3";

/// The function body the bootstrapped tenant deploys. The edge test asserts on
/// this response, so it stays trivial and self-describing. The log line
/// carries deliberately sensitive-shaped values -- a fake address and
/// password -- because the edge test asserts they arrive at the retained log
/// store masked, which is the scrubbing contract demonstrated end to end.
const FUNCTION_SOURCE: &str = r#"export default {
  fetch(request: Request): Response {
    const url = new URL(request.url);
    console.log(`serving hello for caller@example.com password=hunter2 path=${url.pathname}`);
    return new Response(
      JSON.stringify({ ok: true, function: "hello", method: request.method, path: url.pathname }),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  },
};
"#;

fn main() -> ExitCode {
    match run() {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("local bootstrap failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let data_config = ServiceConfig::load_from_process(ServiceKind::DataPlane)
        .map_err(|error| format!("data-plane configuration is invalid: {error}"))?;
    let control_config = ServiceConfig::load_from_process(ServiceKind::ControlPlane)
        .map_err(|error| format!("control-plane configuration is invalid: {error}"))?;

    ensure_local(data_config.environment)?;
    ensure_local(control_config.environment)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the unix epoch".to_owned())?
        .as_secs();

    let tenant = TenantScope::new(
        ProjectId::parse(PROJECT_ID).map_err(|_| "project id is invalid".to_owned())?,
        EnvironmentId::parse(ENVIRONMENT_ID).map_err(|_| "environment id is invalid".to_owned())?,
    );
    let collection_id =
        CollectionId::parse(COLLECTION_ID).map_err(|_| "collection id is invalid".to_owned())?;

    let public_key = seed_data_plane(&data_config, &tenant, &collection_id, now)?;
    seed_control_plane(&control_config, &tenant, &collection_id, now)?;

    let summary = json!({
        "developerId": DEVELOPER_ID,
        "developerEmail": DEVELOPER_EMAIL,
        "developerPassword": DEVELOPER_PASSWORD,
        "teamId": ORGANIZATION_ID,
        "projectId": PROJECT_ID,
        "environmentId": ENVIRONMENT_ID,
        "collectionId": COLLECTION_ID,
        "publicProjectKey": public_key,
        "functionName": FUNCTION_NAME,
        "functionEntrypoint": FUNCTION_ENTRYPOINT,
        "functionRegion": FUNCTION_REGION,
    });
    serde_json::to_string_pretty(&summary).map_err(|_| "summary could not be encoded".to_owned())
}

/// Fail closed outside local. This tool bypasses controls that a hosted
/// environment enforces for good reasons, so it must never be reachable there.
fn ensure_local(environment: DeploymentEnvironment) -> Result<(), String> {
    if environment == DeploymentEnvironment::Local {
        return Ok(());
    }
    Err(format!(
        "refusing to run: resolved environment is {environment:?}, and the local bootstrap is \
         only permitted in a local environment"
    ))
}

/// Seed the store the data plane serves from: the public project key clients
/// present, the signing key that issues their sessions, and the collection
/// metadata without which every document operation is rejected as not found.
fn seed_data_plane(
    config: &ServiceConfig,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    now: u64,
) -> Result<String, String> {
    let graph = DataPlaneGraph::open(config).map_err(|error| {
        format!("data-plane storage could not be opened (is the data plane running?): {error}")
    })?;

    block_on(async {
        let store = graph
            .identity_store(tenant, tenant)
            .map_err(|_| "identity authority is unavailable".to_owned())?;
        let credential_id = ProjectCredentialId::parse(PUBLIC_KEY_ID)
            .map_err(|_| "public key id is invalid".to_owned())?;

        // A public key's secret is shown once at creation and is unrecoverable
        // afterwards, so a repeated run cannot report the original key. Rather
        // than duplicating credentials or leaving the caller without a usable
        // one, an existing public credential is rotated so the tenant keeps its
        // identity and the caller always gets a working key. A credential left
        // in a state that cannot be rotated is superseded by a fresh one.
        let existing = store
            .list_project_credential_metadata(
                NonZeroUsize::new(100).expect("credential limit is positive"),
            )
            .await
            .map_err(|error| format!("project credentials could not be listed: {error:?}"))?
            .into_iter()
            .find(|metadata| metadata.kind() == ProjectCredentialKind::Public);

        let issued = match existing {
            None => store
                .create_public_project_key(credential_id, now)
                .await
                .map_err(|error| format!("public project key could not be created: {error:?}"))?,
            Some(current) => {
                let replacement = ProjectCredentialId::parse(format!("{PUBLIC_KEY_ID}_{now}"))
                    .map_err(|_| "replacement public key id is invalid".to_owned())?;
                match store
                    .rotate_project_credential(current.id(), replacement.clone(), 0, now)
                    .await
                {
                    Ok(issued) => issued,
                    Err(_) => store
                        .create_public_project_key(replacement, now)
                        .await
                        .map_err(|error| {
                            format!("public project key could not be created: {error:?}")
                        })?,
                }
            }
        };
        let public_key = issued.credential.expose_once().to_owned();

        // Sessions cannot be issued without a signing key.
        let has_signing_key = graph
            .signing_key_ring(tenant)
            .await
            .is_ok_and(|ring| !ring.records().is_empty());
        if !has_signing_key {
            graph
                .initialize_signing_key(tenant, now)
                .await
                .map_err(|_| "signing key could not be initialized".to_owned())?;
        }

        install_collection(
            graph.document_engine(),
            tenant,
            collection_id,
            Durability::Sync,
        )
        .await?;
        install_development_policy(&graph, tenant, collection_id).await?;
        Ok(public_key)
    })
}

/// Seed the control store so the tenant is coherent for management surfaces:
/// an active developer, an organization they own, and the project and
/// environment the data-plane state belongs to.
fn seed_control_plane(
    config: &ServiceConfig,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    now: u64,
) -> Result<(), String> {
    let settings = config
        .control_sqlite
        .as_ref()
        .ok_or_else(|| "control-plane SQLite configuration is required".to_owned())?;
    let mut sqlite = SqliteConfig::new(&settings.database_path, settings.database_identity.clone());
    sqlite.lock_path = settings.lock_path.clone();
    sqlite.create_if_missing = true;
    sqlite.minimum_durability = Durability::Sync;
    sqlite.maximum_batch_operations = settings.maximum_batch_operations;
    sqlite.maximum_scan_items = settings.maximum_scan_items;
    sqlite.busy_timeout = settings.busy_timeout;
    sqlite.transaction_expiration = settings.transaction_expiration;
    sqlite.shutdown_timeout = settings.shutdown_timeout;
    let adapter = SqliteAdapter::open(sqlite).map_err(|error| {
        format!("control storage could not be opened (is the control plane running?): {error}")
    })?;
    let adapter: Arc<dyn KvAdapter> = Arc::new(adapter);

    let secret = config
        .internal_auth_secret
        .as_ref()
        .ok_or_else(|| "internal auth secret reference is required".to_owned())?;

    block_on(async {
        let developer_id = DeveloperIdentityId::parse(DEVELOPER_ID)
            .map_err(|_| "developer id is invalid".to_owned())?;
        let registrations = DeveloperRegistrationStore::new(
            Arc::clone(&adapter),
            Durability::Sync,
            DeveloperLookupKey::derive(secret.expose_secret().as_bytes()),
        )
        .map_err(|_| "developer registration store is unavailable".to_owned())?;

        if registrations
            .get_authentication_identity(&developer_id)
            .await
            .map_err(|_| "developer identity could not be read".to_owned())?
            .is_none()
        {
            let password = PasswordService::new(
                PasswordPolicy::new(8, 4096, true, true, true, true)
                    .map_err(|_| "password policy is invalid".to_owned())?,
                Argon2idParameters::new(19 * 1024, 2, 1, 32)
                    .map_err(|_| "password parameters are invalid".to_owned())?,
            )
            .hash(DEVELOPER_PASSWORD)
            .map_err(|_| "developer password could not be hashed".to_owned())?;
            let identity = AuthenticationIdentityRecord::new_verified(
                developer_id.clone(),
                NormalizedEmail::parse(DEVELOPER_EMAIL)
                    .map_err(|_| "developer email is invalid".to_owned())?,
                "Local Developer",
                password.encoded(),
                now,
            )
            .map_err(|_| "developer identity is invalid".to_owned())?;
            registrations
                .create_authentication_identity(&identity)
                .await
                .map_err(|_| "developer identity could not be created".to_owned())?;
            let role =
                DeveloperRoleRecord::new_active_for_local_bootstrap(developer_id.clone(), now)
                    .map_err(|_| "developer role is invalid".to_owned())?;
            registrations
                .create_developer_role(&identity, &role)
                .await
                .map_err(|_| "developer role could not be created".to_owned())?;
        }

        let organization_id = OrganizationId::parse(ORGANIZATION_ID)
            .map_err(|_| "organization id is invalid".to_owned())?;
        let organizations = OrganizationStore::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| "organization store is unavailable".to_owned())?;
        if organizations
            .get_organization(&organization_id)
            .await
            .map_err(|_| "organization could not be read".to_owned())?
            .is_none()
        {
            organizations
                .create_organization(
                    &OrganizationRecord::new(organization_id.clone(), "Local", now)
                        .map_err(|_| "organization is invalid".to_owned())?,
                    &MembershipRecord::new(
                        organization_id.clone(),
                        developer_id.clone(),
                        OrganizationRole::Owner,
                        now,
                    ),
                )
                .await
                .map_err(|_| "organization could not be created".to_owned())?;
        }

        let projects = ProjectStore::new(Arc::clone(&adapter), Durability::Sync)
            .map_err(|_| "project store is unavailable".to_owned())?;
        if projects
            .get_project(tenant.project_id())
            .await
            .map_err(|_| "project could not be read".to_owned())?
            .is_none()
        {
            projects
                .create_project(
                    &ProjectRecord::new(
                        tenant.project_id().clone(),
                        organization_id,
                        "Local",
                        "local",
                        now,
                    )
                    .map_err(|_| "project is invalid".to_owned())?,
                )
                .await
                .map_err(|_| "project could not be created".to_owned())?;
        }
        if projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await
            .map_err(|_| "environment could not be read".to_owned())?
            .is_none()
        {
            projects
                .create_environment(
                    &EnvironmentRecord::new(
                        tenant.environment_id().clone(),
                        tenant.project_id().clone(),
                        "Development",
                        now,
                    )
                    .map_err(|_| "environment is invalid".to_owned())?,
                )
                .await
                .map_err(|_| "environment could not be created".to_owned())?;
        }

        // The control store keeps its own copy of the collection record, so
        // management surfaces list the same collection the data plane serves.
        install_collection(
            &mako_documents::DocumentEngine::new(Arc::clone(&adapter)),
            tenant,
            collection_id,
            Durability::Sync,
        )
        .await?;

        deploy_function(FunctionDeployment {
            adapter,
            projects: &projects,
            organizations: &organizations,
            secret: secret.expose_secret(),
            supervisor: config.runtime_supervisor_address,
            region: &config.region,
            actor: &DeveloperPrincipal::for_local_bootstrap(developer_id, DEVELOPER_EMAIL),
            tenant,
            now,
        })
        .await
    })
}

/// Deploy a function through the real administrative path: create, upload a
/// bundle, deploy an immutable version, health-check it, then promote.
///
/// Going through `FunctionAdminService` rather than writing records directly is
/// what makes the edge test meaningful: what the gateway resolves at invocation
/// time is exactly what a genuine deployment produces.
/// What deploying the sample function needs from the surrounding bootstrap.
struct FunctionDeployment<'a> {
    adapter: Arc<dyn KvAdapter>,
    projects: &'a ProjectStore,
    organizations: &'a OrganizationStore,
    secret: &'a str,
    supervisor: std::net::SocketAddr,
    region: &'a str,
    actor: &'a DeveloperPrincipal,
    tenant: &'a TenantScope,
    now: u64,
}

async fn deploy_function(deployment: FunctionDeployment<'_>) -> Result<(), String> {
    let FunctionDeployment {
        adapter,
        projects,
        organizations,
        secret,
        supervisor,
        region,
        actor,
        tenant,
        now,
    } = deployment;
    // The supervisor is what actually holds a deployment. Without it the
    // control plane would record a version that no runtime can serve, so the
    // function is only deployed when a supervisor is listening.
    let runtime = RuntimeDeploymentClient::new(
        RuntimeDeploymentClientConfig::loopback(supervisor, region.to_owned()),
        RuntimeSupervisorCredential::new(secret)
            .map_err(|_| "runtime supervisor credential is invalid".to_owned())?,
    )
    .map_err(|_| "runtime supervisor client is invalid".to_owned())?;
    if !runtime.dependency_ready() {
        eprintln!(
            "no runtime supervisor at {supervisor}; skipping function deployment. See \
             docs/edge-functions.md for how to run one locally. `mako functions serve` is not \
             suitable: it generates a random supervisor credential, so the control plane cannot \
             authenticate to it."
        );
        return Ok(());
    }
    let audit: Arc<dyn ControlAuditSink> = Arc::new(DiscardedAudit);
    let credentials = CredentialAdminService::new(
        Arc::clone(&adapter),
        Durability::Sync,
        projects.clone(),
        organizations.clone(),
        Arc::clone(&audit),
        KeyEncryptionKey::from_bytes(blake3::derive_key(
            "mako/control-plane/unused-local-signing-key-encryption/v1",
            secret.as_bytes(),
        )),
        FunctionSecretEncryptionKey::from_bytes(blake3::derive_key(
            "mako/control-plane/function-secret-encryption/v1",
            secret.as_bytes(),
        )),
    )
    .map_err(|_| "function secret service is unavailable".to_owned())?;
    let functions = FunctionAdminService::new(
        adapter,
        Durability::Sync,
        projects.clone(),
        organizations.clone(),
        audit,
        credentials,
        // Bundles are only read when a hosted runtime fetches one. Local
        // function serving mounts the source directory instead, so the artifact
        // does not need to outlive this process.
        Arc::new(MemoryObjectStore::default()),
        Arc::new(runtime),
    )
    .map_err(|_| "function administration is unavailable".to_owned())?;

    let name =
        FunctionName::parse(FUNCTION_NAME).map_err(|_| "function name is invalid".to_owned())?;
    if functions
        .get_function(actor, tenant, &name, now)
        .await
        .is_ok()
    {
        return Ok(());
    }

    functions
        .create_function(
            actor,
            NewFunction {
                tenant: tenant.clone(),
                name: name.clone(),
                configuration: FunctionConfiguration {
                    verify_jwt: false,
                    regions: vec![FUNCTION_REGION.to_owned()],
                    secret_names: Vec::new(),
                    limits: FunctionLimits {
                        cpu_milliseconds: 1_000,
                        wall_milliseconds: 10_000,
                        memory_bytes: 128 * 1024 * 1024,
                        request_bytes: 1024 * 1024,
                        response_bytes: 1024 * 1024,
                        concurrency: 4,
                    },
                },
                now_unix_seconds: now,
            },
        )
        .await
        .map_err(|error| format!("function could not be created: {error:?}"))?;

    let outcome = functions
        .upload_bundle(
            actor,
            tenant,
            FunctionBundleUpload::Source {
                entrypoint: FUNCTION_ENTRYPOINT.to_owned(),
                files: vec![FunctionSourceFile {
                    path: FUNCTION_ENTRYPOINT.to_owned(),
                    contents: FUNCTION_SOURCE.as_bytes().to_vec(),
                }],
                dependencies: BTreeMap::new(),
            },
            now,
        )
        .await
        .map_err(|error| format!("function bundle could not be uploaded: {error:?}"))?;
    let artifact = outcome
        .artifact
        .ok_or_else(|| format!("function bundle was rejected: {:?}", outcome.diagnostics))?;

    functions
        .deploy_version(
            actor,
            NewFunctionVersion {
                tenant: tenant.clone(),
                function_name: name.clone(),
                version: 1,
                bundle_digest: artifact.digest().to_owned(),
                entrypoint: FUNCTION_ENTRYPOINT.to_owned(),
                runtime_version: RUNTIME_VERSION.to_owned(),
                now_unix_seconds: now,
            },
        )
        .await
        .map_err(|error| format!("function version could not be deployed: {error:?}"))?;
    functions
        .check_version_health(actor, tenant, &name, 1, now)
        .await
        .map_err(|error| format!("function version health check failed: {error:?}"))?;
    functions
        .promote(actor, tenant, &name, 1, now)
        .await
        .map_err(|error| format!("function version could not be promoted: {error:?}"))?;
    Ok(())
}

/// Audit output is not part of what the bootstrap reports, and the control
/// plane writes its own records once it is running.
struct DiscardedAudit;

impl ControlAuditSink for DiscardedAudit {
    fn record(&self, _event: ControlAuditEvent) {}
}

/// The schema the bootstrapped collection uses. Policy compilation validates
/// expressions against this same schema, so both must read it from one place.
fn collection_schema() -> serde_json::Value {
    // Matches the todo shape the reference application under examples/local-first
    // replicates, so a bootstrapped tenant is directly usable by it.
    json!({
        "type": "object",
        "required": ["id", "ownerId", "title", "updatedAt"],
        "properties": {
            "id": { "type": "string" },
            "ownerId": { "type": "string" },
            "title": { "type": "string" },
            "updatedAt": { "type": "integer" }
        },
        "additionalProperties": true
    })
}

/// Activate a permissive document policy for the bootstrapped collection.
///
/// Document authorization is default-deny, so without an active policy every
/// write is refused and the tenant is unusable. This grants full access to any
/// authenticated application user, which is appropriate for a throwaway local
/// tenant and for nothing else: it is why the bootstrap refuses to run outside
/// a local environment.
async fn install_development_policy(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    collection_id: &CollectionId,
) -> Result<(), String> {
    let scope = CollectionScope::new(tenant.clone(), collection_id.clone());
    let store = graph
        .policy_store(tenant, scope.clone())
        .map_err(|_| "policy store is unavailable".to_owned())?;
    if store
        .active_policy()
        .await
        .map_err(|_| "active policy could not be read".to_owned())?
        .is_some()
    {
        return Ok(());
    }

    let version = PolicyVersion::new(1).map_err(|_| "policy version is invalid".to_owned())?;
    let rule = PolicyRule::new(
        PolicyRuleId::parse("local-development-allow-all")
            .map_err(|_| "policy rule id is invalid".to_owned())?,
        PolicyEffect::Allow,
        [
            DocumentOperation::Create,
            DocumentOperation::Read,
            DocumentOperation::Update,
            DocumentOperation::Delete,
        ],
        "true",
    )
    .map_err(|_| "policy rule is invalid".to_owned())?;
    let policy = PolicySet::new(scope, version, PolicyState::Draft, [rule], [])
        .map_err(|_| "policy set is invalid".to_owned())?;

    match store.create_draft(&policy).await {
        Ok(()) => {}
        Err(error) => return Err(format!("policy draft could not be created: {error:?}")),
    }

    let schema = collection_schema();
    store
        .activate(version, &schema, &PolicyCompiler::default())
        .await
        .map(|_| ())
        .map_err(|error| format!("policy could not be activated: {error:?}"))
}

/// The collection every bootstrapped tenant gets: a minimal RxDB-shaped
/// document with a string primary key.
async fn install_collection(
    engine: &mako_documents::DocumentEngine,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    durability: Durability,
) -> Result<(), String> {
    let metadata = CollectionMetadata::new(
        collection_id.clone(),
        CollectionMetadataVersion::new(1).map_err(|_| "metadata version is invalid".to_owned())?,
        SchemaVersion::new(1).map_err(|_| "schema version is invalid".to_owned())?,
        collection_schema(),
        PrimaryKeyDefinition::field("id").map_err(|_| "primary key is invalid".to_owned())?,
        SchemaCompatibility::Compatible,
        CollectionLifecycle::Active,
    )
    .map_err(|_| "collection metadata is invalid".to_owned())?;
    let scoped = engine
        .scope_collection(
            tenant,
            CollectionScope::new(tenant.clone(), collection_id.clone()),
        )
        .map_err(|_| "collection scope is invalid".to_owned())?;
    scoped
        .install_collection_metadata(&metadata, durability)
        .await
        .map(|_| ())
        .map_err(|error| format!("collection metadata could not be installed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_is_refused_outside_a_local_environment() {
        assert!(ensure_local(DeploymentEnvironment::Local).is_ok());
        for environment in [
            DeploymentEnvironment::Development,
            DeploymentEnvironment::Staging,
            DeploymentEnvironment::Production,
        ] {
            let refusal = ensure_local(environment).expect_err("must refuse");
            assert!(refusal.contains("refusing to run"));
        }
    }

    #[test]
    fn seeded_identifiers_are_valid_and_fixed() {
        // Fixed identifiers are what make repeated runs deterministic and let
        // the smoke test address the tenant without discovery.
        ProjectId::parse(PROJECT_ID).expect("project id");
        EnvironmentId::parse(ENVIRONMENT_ID).expect("environment id");
        CollectionId::parse(COLLECTION_ID).expect("collection id");
        DeveloperIdentityId::parse(DEVELOPER_ID).expect("developer id");
        OrganizationId::parse(ORGANIZATION_ID).expect("organization id");
        ProjectCredentialId::parse(PUBLIC_KEY_ID).expect("credential id");
        NormalizedEmail::parse(DEVELOPER_EMAIL).expect("developer email");
    }

    #[test]
    fn seeded_collection_is_servable() {
        // A collection the data plane will not serve is useless to the smoke
        // test, so the seeded metadata must be active and compatible.
        let engine = mako_documents::DocumentEngine::new(std::sync::Arc::new(
            mako_storage::MemoryAdapter::new(),
        ));
        let tenant = TenantScope::new(
            ProjectId::parse(PROJECT_ID).expect("project"),
            EnvironmentId::parse(ENVIRONMENT_ID).expect("environment"),
        );
        let collection_id = CollectionId::parse(COLLECTION_ID).expect("collection");
        block_on(install_collection(
            &engine,
            &tenant,
            &collection_id,
            Durability::Memory,
        ))
        .expect("install");

        let scoped = engine
            .scope_collection(
                &tenant,
                CollectionScope::new(tenant.clone(), collection_id.clone()),
            )
            .expect("scope");
        let stored = block_on(scoped.collection_metadata())
            .expect("load")
            .expect("metadata present");
        assert_eq!(stored.lifecycle(), CollectionLifecycle::Active);
        assert_eq!(stored.compatibility(), SchemaCompatibility::Compatible);
        assert_eq!(stored.collection_id(), &collection_id);
    }
}
