use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use async_trait::async_trait;
use mako_api::TenantScope;
use mako_object_store::{ObjectAddress, ObjectStore, ObjectStoreError};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, CredentialAdminError, CredentialAdminService, DeveloperPrincipal,
    FunctionBundleFormat, FunctionBundleRecord, FunctionBundleUpload, FunctionBundleUploadOutcome,
    FunctionSecretName, FunctionSecretValue, OrganizationId, OrganizationStore,
    OrganizationStoreError, ProjectStore, ProjectStoreError,
    function_bundle::{BuiltFunctionBundle, build_function_bundle, digest_bytes},
};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FunctionName(String);

impl FunctionName {
    pub fn parse(value: impl Into<String>) -> Result<Self, FunctionAdminError> {
        let value = value.into();
        let valid = value.len() <= 63
            && value
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !valid {
            return Err(FunctionAdminError::InvalidFunction);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for FunctionName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionState {
    Active,
    Deleting,
    Deleted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionVersionState {
    Healthy,
    Failed,
    Deleting,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionLimits {
    pub cpu_milliseconds: u64,
    pub wall_milliseconds: u64,
    pub memory_bytes: u64,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub concurrency: u32,
}

impl FunctionLimits {
    fn validate(&self) -> Result<(), FunctionAdminError> {
        if self.cpu_milliseconds == 0
            || self.wall_milliseconds == 0
            || self.memory_bytes == 0
            || self.request_bytes == 0
            || self.response_bytes == 0
            || self.concurrency == 0
        {
            return Err(FunctionAdminError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionConfiguration {
    pub verify_jwt: bool,
    pub regions: Vec<String>,
    pub secret_names: Vec<FunctionSecretName>,
    pub limits: FunctionLimits,
    /// External hosts this function may reach over HTTPS, beyond the platform
    /// API origin every worker gets. Empty means egress stays denied. Stored
    /// records predate this field, so it must keep a serde default — without
    /// one, every function written before it existed fails to deserialize —
    /// and an empty list must serialize to *nothing*: the promote and deploy
    /// paths compare-and-write against the re-serialized stored record, and a
    /// field that appears on the way out but not in the stored bytes makes
    /// that comparison fail forever (finding #41).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}

/// The most egress hosts one function may declare. A documented constant, not
/// a config flag: raising it is a deliberate change.
pub const MAX_ALLOWED_HOSTS: usize = 8;

impl FunctionConfiguration {
    fn validate(&mut self) -> Result<(), FunctionAdminError> {
        self.limits.validate()?;
        self.regions.sort();
        self.regions.dedup();
        self.secret_names.sort();
        self.secret_names.dedup();
        self.allowed_hosts.sort();
        self.allowed_hosts.dedup();
        if self.regions.is_empty()
            || self.regions.len() > 16
            || self.secret_names.len() > 64
            || self.regions.iter().any(|region| {
                region.is_empty() || region.len() > 64 || region.chars().any(char::is_control)
            })
        {
            return Err(FunctionAdminError::InvalidConfiguration);
        }
        if self.allowed_hosts.len() > MAX_ALLOWED_HOSTS {
            return Err(FunctionAdminError::InvalidAllowedHost(format!(
                "at most {MAX_ALLOWED_HOSTS} hosts may be declared"
            )));
        }
        for host in &self.allowed_hosts {
            validate_allowed_host(host)?;
        }
        Ok(())
    }
}

/// One declared egress host: a public DNS name and nothing else. IP literals,
/// ports, wildcards, and the name families that reach the platform's own
/// machinery from inside a worker's network namespace -- `localhost`,
/// `metadata`, and everything under `.internal` (cloud metadata,
/// `host.docker.internal`), `.local`, `.localhost`, and `.arpa` -- are refused
/// here, before any version exists, naming the entry that failed.
fn validate_allowed_host(host: &str) -> Result<(), FunctionAdminError> {
    let refuse = |reason: &str| {
        let shown: String = host.chars().take(64).filter(|c| !c.is_control()).collect();
        Err(FunctionAdminError::InvalidAllowedHost(format!(
            "{reason}: {shown}"
        )))
    };
    let labels_are_valid = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    });
    if host.is_empty() || host.len() > 253 || !labels_are_valid {
        return refuse("host must be a lowercase DNS name without port or wildcard");
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return refuse("host must be a DNS name, not an address");
    }
    if matches!(host, "localhost" | "metadata")
        || [".internal", ".local", ".localhost", ".arpa"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
    {
        return refuse("host resolves inside the platform, not on the public internet");
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionRecord {
    tenant: TenantScope,
    name: FunctionName,
    state: FunctionState,
    active_version: Option<u64>,
    configuration: FunctionConfiguration,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
    /// True from creation until a deployment is first handed to the runtime,
    /// so deleting the function or reading its logs need not ask a runtime
    /// that holds nothing for it, and works while none is reachable. It is
    /// cleared before the first deploy call, never after it: the runtime can
    /// accept a deployment the control plane then fails to record, and that
    /// still has to be retired. Records written before this field read as
    /// `false`, the safe answer, and like `allowed_hosts` it serializes to
    /// nothing when false, so a round-trip reproduces their stored bytes.
    #[serde(default, skip_serializing_if = "is_false")]
    runtime_untouched: bool,
    /// The highest version number any deployment of this function has used,
    /// deleted ones included. A number is never handed out twice: logs,
    /// metrics, and audit that say "version 2" must mean one bundle. Records
    /// written before it read as 0, and their next deploy raises it from the
    /// versions still stored; like `runtime_untouched` it serializes to
    /// nothing at 0, so a round-trip reproduces their stored bytes.
    #[serde(default, skip_serializing_if = "is_zero")]
    highest_version: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
const fn is_false(value: &bool) -> bool {
    !*value
}

/// How many stored deployments a legacy record's first deploy reads to find
/// the highest version it has used.
const MAXIMUM_VERSION_SCAN: usize = 10_000;

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
const fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl FunctionRecord {
    #[must_use]
    pub const fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn name(&self) -> &FunctionName {
        &self.name
    }

    #[must_use]
    pub const fn state(&self) -> FunctionState {
        self.state
    }

    #[must_use]
    pub const fn active_version(&self) -> Option<u64> {
        self.active_version
    }

    /// The version number the next deployment must use, once one has been
    /// recorded; `None` before the first deploy of a record that predates it.
    #[must_use]
    pub const fn next_version(&self) -> Option<u64> {
        if self.highest_version == 0 {
            None
        } else {
            Some(self.highest_version.saturating_add(1))
        }
    }

    #[must_use]
    pub fn configuration(&self) -> &FunctionConfiguration {
        &self.configuration
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionVersionRecord {
    tenant: TenantScope,
    function_name: FunctionName,
    version: u64,
    bundle_digest: String,
    entrypoint: String,
    runtime_version: String,
    bundle_format: FunctionBundleFormat,
    bundle_size_bytes: u64,
    configuration: FunctionConfiguration,
    secret_versions: Vec<FunctionSecretVersionReference>,
    state: FunctionVersionState,
    diagnostic: Option<String>,
    created_at_unix_seconds: u64,
}

impl FunctionVersionRecord {
    #[must_use]
    pub const fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn function_name(&self) -> &FunctionName {
        &self.function_name
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn state(&self) -> FunctionVersionState {
        self.state
    }

    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    #[must_use]
    pub fn bundle_digest(&self) -> &str {
        &self.bundle_digest
    }

    #[must_use]
    pub fn entrypoint(&self) -> &str {
        &self.entrypoint
    }

    #[must_use]
    pub fn runtime_version(&self) -> &str {
        &self.runtime_version
    }

    #[must_use]
    pub const fn bundle_format(&self) -> FunctionBundleFormat {
        self.bundle_format
    }

    #[must_use]
    pub const fn bundle_size_bytes(&self) -> u64 {
        self.bundle_size_bytes
    }

    #[must_use]
    pub fn configuration(&self) -> &FunctionConfiguration {
        &self.configuration
    }

    #[must_use]
    pub fn secret_versions(&self) -> &[FunctionSecretVersionReference] {
        &self.secret_versions
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionSecretVersionReference {
    pub name: FunctionSecretName,
    pub version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDeploymentSecret {
    pub reference: FunctionSecretVersionReference,
    value: FunctionSecretValue,
}

impl FunctionDeploymentSecret {
    #[must_use]
    pub fn new(reference: FunctionSecretVersionReference, value: FunctionSecretValue) -> Self {
        Self { reference, value }
    }

    #[must_use]
    pub fn expose_to_runtime_adapter(&self) -> &str {
        self.value.expose_for_runtime()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewFunction {
    pub tenant: TenantScope,
    pub name: FunctionName,
    pub configuration: FunctionConfiguration,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewFunctionVersion {
    pub tenant: TenantScope,
    pub function_name: FunctionName,
    pub version: u64,
    pub bundle_digest: String,
    pub entrypoint: String,
    pub runtime_version: String,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDeploymentSpec {
    pub tenant: TenantScope,
    pub function_name: FunctionName,
    pub version: u64,
    pub bundle_digest: String,
    pub entrypoint: String,
    pub runtime_version: String,
    pub configuration: FunctionConfiguration,
    pub bundle_format: FunctionBundleFormat,
    pub bundle: Arc<[u8]>,
    pub secrets: Vec<FunctionDeploymentSecret>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDeploymentResult {
    pub healthy: bool,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionTestRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionTestResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub correlation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionLogQuery {
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionLogEntry {
    pub timestamp_unix_milliseconds: u64,
    pub level: String,
    pub message: String,
    pub correlation_id: String,
    pub version: u64,
    pub region: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionLogPage {
    pub items: Vec<FunctionLogEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionBackendError {
    safe_message: String,
}

impl FunctionBackendError {
    pub fn new(safe_message: impl Into<String>) -> Result<Self, FunctionAdminError> {
        let safe_message = safe_message.into();
        if safe_message.is_empty()
            || safe_message.len() > 500
            || safe_message.chars().any(char::is_control)
        {
            return Err(FunctionAdminError::InvalidBackendDiagnostic);
        }
        Ok(Self { safe_message })
    }
}

impl fmt::Display for FunctionBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.safe_message)
    }
}

impl Error for FunctionBackendError {}

#[async_trait]
pub trait FunctionDeploymentBackend: Send + Sync {
    async fn deploy(
        &self,
        spec: &FunctionDeploymentSpec,
    ) -> Result<FunctionDeploymentResult, FunctionBackendError>;

    async fn health(
        &self,
        spec: &FunctionDeploymentSpec,
    ) -> Result<FunctionDeploymentResult, FunctionBackendError>;

    async fn test(
        &self,
        spec: &FunctionDeploymentSpec,
        request: &FunctionTestRequest,
    ) -> Result<FunctionTestResponse, FunctionBackendError>;

    async fn logs(
        &self,
        tenant: &TenantScope,
        function_name: &FunctionName,
        query: &FunctionLogQuery,
    ) -> Result<FunctionLogPage, FunctionBackendError>;

    async fn delete_version(
        &self,
        tenant: &TenantScope,
        function_name: &FunctionName,
        version: u64,
    ) -> Result<(), FunctionBackendError>;

    async fn delete_function(
        &self,
        tenant: &TenantScope,
        function_name: &FunctionName,
    ) -> Result<(), FunctionBackendError>;
}

#[derive(Clone)]
pub struct FunctionAdminService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
    credentials: CredentialAdminService,
    objects: Arc<dyn ObjectStore>,
    backend: Arc<dyn FunctionDeploymentBackend>,
}

impl fmt::Debug for FunctionAdminService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionAdminService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl FunctionAdminService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
        credentials: CredentialAdminService,
        objects: Arc<dyn ObjectStore>,
        backend: Arc<dyn FunctionDeploymentBackend>,
    ) -> Result<Self, FunctionAdminError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(FunctionAdminError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            audit,
            credentials,
            objects,
            backend,
        })
    }

    pub async fn create_function(
        &self,
        actor: &DeveloperPrincipal,
        mut input: NewFunction,
    ) -> Result<FunctionRecord, FunctionAdminError> {
        let organization = self
            .authorize(actor, &input.tenant, true, input.now_unix_seconds)
            .await?;
        input.configuration.validate()?;
        let record = FunctionRecord {
            tenant: input.tenant.clone(),
            name: input.name,
            state: FunctionState::Active,
            active_version: None,
            configuration: input.configuration,
            created_at_unix_seconds: input.now_unix_seconds,
            updated_at_unix_seconds: input.now_unix_seconds,
            runtime_untouched: true,
            highest_version: 0,
        };
        let key = function_key(&record)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        self.create(key, batch).await?;
        self.audit(
            actor,
            &organization,
            &input.tenant,
            ControlAuditAction::FunctionCreate,
            record.name.as_str(),
            input.now_unix_seconds,
        );
        Ok(record)
    }

    pub async fn get_function(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        now: u64,
    ) -> Result<FunctionRecord, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, false, now).await?;
        let record = self.function(tenant, name).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionRead,
            name.as_str(),
            now,
        );
        Ok(record)
    }

    pub async fn list_functions(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        limit: NonZeroUsize,
        now: u64,
    ) -> Result<Vec<FunctionRecord>, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, false, now).await?;
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::functions_range(tenant.project_id(), tenant.environment_id())?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let records = entries
            .into_iter()
            .map(|entry| serde_json::from_slice(&entry.value))
            .collect::<Result<Vec<FunctionRecord>, _>>()?;
        if records.iter().any(|record| record.tenant != *tenant) {
            return Err(FunctionAdminError::CorruptRecord);
        }
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionRead,
            "functions",
            now,
        );
        Ok(records)
    }

    pub async fn configure(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        mut configuration: FunctionConfiguration,
        now: u64,
    ) -> Result<FunctionRecord, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, true, now).await?;
        configuration.validate()?;
        let previous = self.function(tenant, name).await?;
        let mut next = previous.clone();
        next.configuration = configuration;
        next.updated_at_unix_seconds = now;
        self.replace_function(&previous, &next).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionConfigure,
            name.as_str(),
            now,
        );
        Ok(next)
    }

    pub async fn upload_bundle(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        upload: FunctionBundleUpload,
        now: u64,
    ) -> Result<FunctionBundleUploadOutcome, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, true, now).await?;
        let built = match build_function_bundle(tenant.clone(), upload, now) {
            Ok(built) => built,
            Err(diagnostics) => {
                self.audit(
                    actor,
                    &organization,
                    tenant,
                    ControlAuditAction::FunctionBundleUpload,
                    "bundle-rejected",
                    now,
                );
                return Ok(FunctionBundleUploadOutcome {
                    artifact: None,
                    diagnostics,
                });
            }
        };
        let record = self.store_bundle(built).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionBundleUpload,
            record.digest(),
            now,
        );
        Ok(FunctionBundleUploadOutcome {
            artifact: Some(record),
            diagnostics: Vec::new(),
        })
    }

    pub async fn deploy_version(
        &self,
        actor: &DeveloperPrincipal,
        input: NewFunctionVersion,
    ) -> Result<FunctionVersionRecord, FunctionAdminError> {
        let organization = self
            .authorize(actor, &input.tenant, true, input.now_unix_seconds)
            .await?;
        if input.version == 0
            || !valid_digest(&input.bundle_digest)
            || !valid_text(&input.entrypoint, 512)
            || !valid_text(&input.runtime_version, 128)
        {
            return Err(FunctionAdminError::InvalidDeployment);
        }
        let function = self.function(&input.tenant, &input.function_name).await?;
        if function.state != FunctionState::Active {
            return Err(FunctionAdminError::InvalidState);
        }
        let (bundle, bundle_bytes) = self.bundle(&input.tenant, &input.bundle_digest).await?;
        if bundle.entrypoint() != input.entrypoint {
            return Err(FunctionAdminError::InvalidDeployment);
        }
        let highest = if function.highest_version > 0 {
            function.highest_version
        } else {
            self.highest_stored_version(&input.tenant, &input.function_name)
                .await?
        };
        if input.version <= highest {
            return Err(FunctionAdminError::VersionAlreadyUsed {
                version: input.version,
                next: highest.saturating_add(1),
            });
        }
        let key = ControlKeyspace::function_version_key(
            input.tenant.project_id(),
            input.tenant.environment_id(),
            &input.function_name,
            input.version,
        )?;
        if self.adapter.get(&key).await?.is_some() {
            return Err(FunctionAdminError::Conflict);
        }
        let secrets = self
            .resolve_active_secrets(&input.tenant, &function.configuration.secret_names)
            .await?;
        let secret_versions = secrets
            .iter()
            .map(|secret| secret.reference.clone())
            .collect::<Vec<_>>();
        let spec = FunctionDeploymentSpec {
            tenant: input.tenant.clone(),
            function_name: input.function_name.clone(),
            version: input.version,
            bundle_digest: input.bundle_digest.clone(),
            entrypoint: input.entrypoint.clone(),
            runtime_version: input.runtime_version.clone(),
            configuration: function.configuration.clone(),
            bundle_format: bundle.format(),
            bundle: bundle_bytes,
            secrets,
        };
        // Record that the runtime may hold a deployment before asking it to
        // take one: a delete that races this deploy, or follows one the
        // runtime accepted but this service never recorded, must retire it.
        // The version number is spent here too, so a deploy that then fails
        // still never hands its number out again.
        let mut handed_over = function.clone();
        handed_over.runtime_untouched = false;
        handed_over.highest_version = input.version;
        self.replace_function(&function, &handed_over).await?;
        let result = self.backend.deploy(&spec).await?;
        validate_deployment_result(&result)?;
        let record = FunctionVersionRecord {
            tenant: input.tenant.clone(),
            function_name: input.function_name,
            version: input.version,
            bundle_digest: input.bundle_digest,
            entrypoint: input.entrypoint,
            runtime_version: input.runtime_version,
            bundle_format: bundle.format(),
            bundle_size_bytes: bundle.size_bytes(),
            configuration: function.configuration,
            secret_versions,
            state: if result.healthy {
                FunctionVersionState::Healthy
            } else {
                FunctionVersionState::Failed
            },
            diagnostic: result.diagnostic,
            created_at_unix_seconds: input.now_unix_seconds,
        };
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        self.create(key, batch).await?;
        self.audit(
            actor,
            &organization,
            &input.tenant,
            ControlAuditAction::FunctionDeploy,
            record.function_name.as_str(),
            input.now_unix_seconds,
        );
        Ok(record)
    }

    pub async fn list_versions(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        limit: NonZeroUsize,
        now: u64,
    ) -> Result<Vec<FunctionVersionRecord>, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, false, now).await?;
        self.function(tenant, name).await?;
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::function_versions_range(
                    tenant.project_id(),
                    tenant.environment_id(),
                    name,
                )?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let records = entries
            .into_iter()
            .map(|entry| serde_json::from_slice(&entry.value))
            .collect::<Result<Vec<FunctionVersionRecord>, _>>()?;
        if records
            .iter()
            .any(|record| record.tenant != *tenant || record.function_name != *name)
        {
            return Err(FunctionAdminError::CorruptRecord);
        }
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionRead,
            name.as_str(),
            now,
        );
        Ok(records)
    }

    pub async fn get_version(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        version: u64,
        now: u64,
    ) -> Result<FunctionVersionRecord, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, false, now).await?;
        self.function(tenant, name).await?;
        let record = self.version(tenant, name, version).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionRead,
            name.as_str(),
            now,
        );
        Ok(record)
    }

    pub async fn check_version_health(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        version: u64,
        now: u64,
    ) -> Result<FunctionVersionRecord, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, true, now).await?;
        let function = self.function(tenant, name).await?;
        let previous = self.version(tenant, name, version).await?;
        if previous.state == FunctionVersionState::Deleting {
            return Err(FunctionAdminError::InvalidState);
        }
        let (_, bundle) = self.bundle(tenant, &previous.bundle_digest).await?;
        let spec = self.deployment_spec(&function, &previous, bundle).await?;
        let result = self.backend.health(&spec).await?;
        validate_deployment_result(&result)?;
        let mut next = previous.clone();
        next.state = if result.healthy {
            FunctionVersionState::Healthy
        } else {
            FunctionVersionState::Failed
        };
        next.diagnostic = result.diagnostic;
        self.replace_version(&previous, &next).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionHealthCheck,
            name.as_str(),
            now,
        );
        Ok(next)
    }

    pub async fn promote(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        version: u64,
        now: u64,
    ) -> Result<FunctionRecord, FunctionAdminError> {
        self.switch_version(actor, tenant, name, version, now, false)
            .await
    }

    pub async fn rollback(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        version: u64,
        now: u64,
    ) -> Result<FunctionRecord, FunctionAdminError> {
        self.switch_version(actor, tenant, name, version, now, true)
            .await
    }

    async fn switch_version(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        version: u64,
        now: u64,
        rollback: bool,
    ) -> Result<FunctionRecord, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, true, now).await?;
        let deployment = self.version(tenant, name, version).await?;
        if deployment.state != FunctionVersionState::Healthy {
            return Err(FunctionAdminError::DeploymentUnhealthy);
        }
        let previous = self.function(tenant, name).await?;
        let mut next = previous.clone();
        next.active_version = Some(version);
        next.updated_at_unix_seconds = now;
        self.replace_function_for_version(&previous, &deployment, &next)
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            if rollback {
                ControlAuditAction::FunctionRollback
            } else {
                ControlAuditAction::FunctionPromote
            },
            name.as_str(),
            now,
        );
        Ok(next)
    }

    pub async fn test_invoke(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        version: Option<u64>,
        request: &FunctionTestRequest,
        now: u64,
    ) -> Result<FunctionTestResponse, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, true, now).await?;
        if request.body.len() > 10 * 1024 * 1024
            || !valid_text(&request.method, 16)
            || !valid_text(&request.path, 2048)
        {
            return Err(FunctionAdminError::InvalidTestRequest);
        }
        let function = self.function(tenant, name).await?;
        let version = version
            .or(function.active_version)
            .ok_or(FunctionAdminError::NoActiveVersion)?;
        let deployment = self.version(tenant, name, version).await?;
        if deployment.state != FunctionVersionState::Healthy {
            return Err(FunctionAdminError::DeploymentUnhealthy);
        }
        let (_, bundle) = self.bundle(tenant, &deployment.bundle_digest).await?;
        let spec = self.deployment_spec(&function, &deployment, bundle).await?;
        let response = self.backend.test(&spec, request).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionTest,
            name.as_str(),
            now,
        );
        Ok(response)
    }

    pub async fn logs(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        query: &FunctionLogQuery,
        now: u64,
    ) -> Result<FunctionLogPage, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, false, now).await?;
        if query.limit == 0 || query.limit > 1000 {
            return Err(FunctionAdminError::InvalidLogQuery);
        }
        let function = self.function(tenant, name).await?;
        // Nothing has run, so there is nothing to read, and a runtime that is
        // down must not fail the read.
        let mut page = if function.runtime_untouched {
            FunctionLogPage {
                items: Vec::new(),
                next_cursor: None,
            }
        } else {
            self.backend.logs(tenant, name, query).await?
        };
        if page.items.len() > query.limit {
            return Err(FunctionAdminError::InvalidBackendResponse);
        }
        self.redact_log_page(tenant, name, &mut page).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionLogsRead,
            name.as_str(),
            now,
        );
        Ok(page)
    }

    pub async fn delete_version(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        version: u64,
        now: u64,
    ) -> Result<(), FunctionAdminError> {
        let organization = self.authorize(actor, tenant, true, now).await?;
        let function = self.function(tenant, name).await?;
        if function.active_version == Some(version) {
            return Err(FunctionAdminError::ActiveVersionCannotBeDeleted);
        }
        let record = self.version(tenant, name, version).await?;
        let deleting = if record.state == FunctionVersionState::Deleting {
            record
        } else {
            let mut deleting = record.clone();
            deleting.state = FunctionVersionState::Deleting;
            self.mark_version_deleting(&function, &record, &deleting)
                .await?;
            deleting
        };
        self.backend.delete_version(tenant, name, version).await?;
        self.delete_record(version_key(&deleting)?, serde_json::to_vec(&deleting)?)
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionDelete,
            name.as_str(),
            now,
        );
        Ok(())
    }

    pub async fn delete_function(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionName,
        now: u64,
    ) -> Result<FunctionRecord, FunctionAdminError> {
        let organization = self.authorize(actor, tenant, true, now).await?;
        let previous = self.function(tenant, name).await?;
        // A function never handed to the runtime holds nothing there to
        // retire, so its delete does not wait on one. A deploy that starts
        // meanwhile clears the flag first, and the write below then fails its
        // comparison instead of deleting a function the runtime now serves.
        if !previous.runtime_untouched {
            self.backend.delete_function(tenant, name).await?;
        }
        let mut next = previous.clone();
        next.state = FunctionState::Deleted;
        next.active_version = None;
        next.updated_at_unix_seconds = now;
        self.replace_function(&previous, &next).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionDelete,
            name.as_str(),
            now,
        );
        Ok(next)
    }

    async fn function(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
    ) -> Result<FunctionRecord, FunctionAdminError> {
        let key =
            ControlKeyspace::function_key(tenant.project_id(), tenant.environment_id(), name)?;
        let record = self
            .adapter
            .get(&key)
            .await?
            .ok_or(FunctionAdminError::NotFound)
            .and_then(|bytes| {
                serde_json::from_slice::<FunctionRecord>(&bytes).map_err(Into::into)
            })?;
        if record.tenant != *tenant || record.name != *name {
            return Err(FunctionAdminError::CorruptRecord);
        }
        Ok(record)
    }

    async fn version(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
        version: u64,
    ) -> Result<FunctionVersionRecord, FunctionAdminError> {
        let key = ControlKeyspace::function_version_key(
            tenant.project_id(),
            tenant.environment_id(),
            name,
            version,
        )?;
        let record = self
            .adapter
            .get(&key)
            .await?
            .ok_or(FunctionAdminError::NotFound)
            .and_then(|bytes| {
                serde_json::from_slice::<FunctionVersionRecord>(&bytes).map_err(Into::into)
            })?;
        if record.tenant != *tenant || record.function_name != *name || record.version != version {
            return Err(FunctionAdminError::CorruptRecord);
        }
        Ok(record)
    }

    async fn store_bundle(
        &self,
        built: BuiltFunctionBundle,
    ) -> Result<FunctionBundleRecord, FunctionAdminError> {
        let tenant = &built.record.tenant;
        let metadata_key = ControlKeyspace::function_bundle_key(
            tenant.project_id(),
            tenant.environment_id(),
            built.record.digest(),
        )?;
        let object_address = ObjectAddress::function_bundle(tenant.clone(), built.record.digest())?;
        self.objects
            .put_immutable(tenant, &object_address, Arc::from(built.bytes.clone()))
            .await?;
        if self.adapter.get(&metadata_key).await?.is_some() {
            return self.existing_bundle(&built).await;
        }
        let mut batch = WriteBatch::new();
        batch.put(&metadata_key, serde_json::to_vec(&built.record)?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::Missing { key: metadata_key }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(built.record),
            CompareAndWriteResult::Conflict { .. } => self.existing_bundle(&built).await,
        }
    }

    async fn existing_bundle(
        &self,
        built: &BuiltFunctionBundle,
    ) -> Result<FunctionBundleRecord, FunctionAdminError> {
        let (record, bytes) = self
            .bundle(&built.record.tenant, built.record.digest())
            .await?;
        if bytes.as_ref() != built.bytes
            || record.format != built.record.format
            || record.entrypoint != built.record.entrypoint
            || record.module_count != built.record.module_count
        {
            return Err(FunctionAdminError::CorruptBundle);
        }
        Ok(record)
    }

    async fn bundle(
        &self,
        tenant: &TenantScope,
        digest: &str,
    ) -> Result<(FunctionBundleRecord, Arc<[u8]>), FunctionAdminError> {
        if !valid_digest(digest) {
            return Err(FunctionAdminError::InvalidDeployment);
        }
        let metadata_key = ControlKeyspace::function_bundle_key(
            tenant.project_id(),
            tenant.environment_id(),
            digest,
        )?;
        let object_address = ObjectAddress::function_bundle(tenant.clone(), digest)?;
        let record = self
            .adapter
            .get(&metadata_key)
            .await?
            .ok_or(FunctionAdminError::BundleNotFound)
            .and_then(|bytes| {
                serde_json::from_slice::<FunctionBundleRecord>(&bytes).map_err(Into::into)
            })?;
        let bytes = self
            .objects
            .get(tenant, &object_address)
            .await?
            .ok_or(FunctionAdminError::CorruptBundle)?;
        if record.tenant != *tenant
            || record.digest != digest
            || usize::try_from(record.size_bytes).ok() != Some(bytes.len())
            || digest_bytes(bytes.as_ref()) != digest
        {
            return Err(FunctionAdminError::CorruptBundle);
        }
        Ok((record, bytes))
    }

    async fn resolve_active_secrets(
        &self,
        tenant: &TenantScope,
        names: &[FunctionSecretName],
    ) -> Result<Vec<FunctionDeploymentSecret>, FunctionAdminError> {
        let mut secrets = Vec::with_capacity(names.len());
        for name in names {
            let resolved = self
                .credentials
                .resolve_active_function_secret(tenant, name)
                .await?;
            secrets.push(FunctionDeploymentSecret {
                reference: FunctionSecretVersionReference {
                    name: resolved.metadata.name().clone(),
                    version: resolved.metadata.version(),
                },
                value: resolved.value,
            });
        }
        Ok(secrets)
    }

    async fn resolve_version_secrets(
        &self,
        tenant: &TenantScope,
        references: &[FunctionSecretVersionReference],
    ) -> Result<Vec<FunctionDeploymentSecret>, FunctionAdminError> {
        let mut secrets = Vec::with_capacity(references.len());
        for reference in references {
            let resolved = self
                .credentials
                .resolve_function_secret_version(tenant, &reference.name, reference.version)
                .await?;
            secrets.push(FunctionDeploymentSecret {
                reference: reference.clone(),
                value: resolved.value,
            });
        }
        Ok(secrets)
    }

    async fn deployment_spec(
        &self,
        function: &FunctionRecord,
        version: &FunctionVersionRecord,
        bundle: Arc<[u8]>,
    ) -> Result<FunctionDeploymentSpec, FunctionAdminError> {
        Ok(FunctionDeploymentSpec {
            tenant: function.tenant.clone(),
            function_name: function.name.clone(),
            version: version.version,
            bundle_digest: version.bundle_digest.clone(),
            entrypoint: version.entrypoint.clone(),
            runtime_version: version.runtime_version.clone(),
            configuration: version.configuration.clone(),
            bundle_format: version.bundle_format,
            bundle,
            secrets: self
                .resolve_version_secrets(&function.tenant, &version.secret_versions)
                .await?,
        })
    }

    /// Read one function's supervisor logs with the same per-version
    /// secret-value redaction the member-facing endpoint applies, without an
    /// actor: this is for the platform's own log collector, which must never
    /// store a line the live view would have masked. Not audited -- it is a
    /// scheduled platform pass, not a member reading logs.
    pub async fn collect_logs(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
        query: &FunctionLogQuery,
    ) -> Result<FunctionLogPage, FunctionAdminError> {
        if query.limit == 0 || query.limit > 1000 {
            return Err(FunctionAdminError::InvalidLogQuery);
        }
        let mut page = self.backend.logs(tenant, name, query).await?;
        if page.items.len() > query.limit {
            return Err(FunctionAdminError::InvalidBackendResponse);
        }
        self.redact_log_page(tenant, name, &mut page).await?;
        Ok(page)
    }

    async fn redact_log_page(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
        page: &mut FunctionLogPage,
    ) -> Result<(), FunctionAdminError> {
        let mut values_by_version = BTreeMap::<u64, Vec<FunctionSecretValue>>::new();
        for item in &page.items {
            if values_by_version.contains_key(&item.version) {
                continue;
            }
            let deployment = self.version(tenant, name, item.version).await?;
            let values = self
                .resolve_version_secrets(tenant, &deployment.secret_versions)
                .await?
                .into_iter()
                .map(|secret| secret.value)
                .collect();
            values_by_version.insert(item.version, values);
        }
        for item in &mut page.items {
            if let Some(values) = values_by_version.get(&item.version) {
                for value in values {
                    item.message = item
                        .message
                        .replace(value.expose_for_redaction(), "[REDACTED]");
                }
            }
        }
        Ok(())
    }

    /// The highest version among the deployments still stored, for a record
    /// written before `highest_version` existed.
    async fn highest_stored_version(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
    ) -> Result<u64, FunctionAdminError> {
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::function_versions_range(
                    tenant.project_id(),
                    tenant.environment_id(),
                    name,
                )?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_VERSION_SCAN).expect("constant is positive"),
            ))
            .await?;
        entries.into_iter().try_fold(0, |highest, entry| {
            let record: FunctionVersionRecord = serde_json::from_slice(&entry.value)?;
            Ok(highest.max(record.version))
        })
    }

    async fn replace_function(
        &self,
        previous: &FunctionRecord,
        next: &FunctionRecord,
    ) -> Result<(), FunctionAdminError> {
        let key = function_key(previous)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn replace_function_for_version(
        &self,
        previous: &FunctionRecord,
        deployment: &FunctionVersionRecord,
        next: &FunctionRecord,
    ) -> Result<(), FunctionAdminError> {
        let function_key = function_key(previous)?;
        let deployment_key = version_key(deployment)?;
        let mut batch = WriteBatch::new();
        batch.put(&function_key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: function_key,
                    value: serde_json::to_vec(previous)?,
                },
                KeyCondition::ValueEquals {
                    key: deployment_key,
                    value: serde_json::to_vec(deployment)?,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn replace_version(
        &self,
        previous: &FunctionVersionRecord,
        next: &FunctionVersionRecord,
    ) -> Result<(), FunctionAdminError> {
        let key = version_key(previous)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn mark_version_deleting(
        &self,
        function: &FunctionRecord,
        previous: &FunctionVersionRecord,
        deleting: &FunctionVersionRecord,
    ) -> Result<(), FunctionAdminError> {
        let function_key = function_key(function)?;
        let deployment_key = version_key(previous)?;
        let mut batch = WriteBatch::new();
        batch.put(&deployment_key, serde_json::to_vec(deleting)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: function_key,
                    value: serde_json::to_vec(function)?,
                },
                KeyCondition::ValueEquals {
                    key: deployment_key,
                    value: serde_json::to_vec(previous)?,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn create(&self, key: Vec<u8>, batch: WriteBatch) -> Result<(), FunctionAdminError> {
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn delete_record(
        &self,
        key: Vec<u8>,
        previous: Vec<u8>,
    ) -> Result<(), FunctionAdminError> {
        let mut batch = WriteBatch::new();
        batch.delete(&key);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: previous,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), FunctionAdminError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(FunctionAdminError::Conflict),
        }
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        mutation: bool,
        now: u64,
    ) -> Result<OrganizationId, FunctionAdminError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(FunctionAdminError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(FunctionAdminError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?;
        if membership.is_none()
            || (mutation
                && membership
                    .as_ref()
                    .is_none_or(|member| !member.role().can_mutate_projects()))
        {
            self.audit(
                actor,
                project.organization_id(),
                tenant,
                ControlAuditAction::FunctionRead,
                "authorization",
                now,
            );
            return Err(FunctionAdminError::Forbidden);
        }
        Ok(project.organization_id().clone())
    }

    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization: &OrganizationId,
        tenant: &TenantScope,
        action: ControlAuditAction,
        target: &str,
        at: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: format!(
                "{}/{}/{}",
                tenant.project_id().as_str(),
                tenant.environment_id().as_str(),
                target
            ),
            outcome: if target == "authorization" {
                ControlAuditOutcome::Denied
            } else {
                ControlAuditOutcome::Allowed
            },
            at_unix_seconds: at,
        });
    }
}

fn function_key(record: &FunctionRecord) -> Result<Vec<u8>, FunctionAdminError> {
    ControlKeyspace::function_key(
        record.tenant.project_id(),
        record.tenant.environment_id(),
        &record.name,
    )
    .map_err(Into::into)
}
fn version_key(record: &FunctionVersionRecord) -> Result<Vec<u8>, FunctionAdminError> {
    ControlKeyspace::function_version_key(
        record.tenant.project_id(),
        record.tenant.environment_id(),
        &record.function_name,
        record.version,
    )
    .map_err(Into::into)
}
fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn validate_deployment_result(result: &FunctionDeploymentResult) -> Result<(), FunctionAdminError> {
    if result
        .diagnostic
        .as_ref()
        .is_some_and(|value| !valid_text(value, 500))
    {
        Err(FunctionAdminError::InvalidBackendDiagnostic)
    } else {
        Ok(())
    }
}
#[derive(Debug)]
pub enum FunctionAdminError {
    UnsupportedDurability,
    NotFound,
    Forbidden,
    Conflict,
    CorruptRecord,
    InvalidFunction,
    InvalidConfiguration,
    InvalidAllowedHost(String),
    InvalidDeployment,
    BundleNotFound,
    CorruptBundle,
    InvalidBackendDiagnostic,
    InvalidBackendResponse,
    InvalidTestRequest,
    InvalidLogQuery,
    InvalidState,
    DeploymentUnhealthy,
    NoActiveVersion,
    ActiveVersionCannotBeDeleted,
    /// A deployment asked for a version number the function has used before.
    VersionAlreadyUsed {
        version: u64,
        next: u64,
    },
    Backend(FunctionBackendError),
    Credential(CredentialAdminError),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    ObjectStore(ObjectStoreError),
    Json(serde_json::Error),
}

impl fmt::Display for FunctionAdminError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::InvalidAllowedHost(detail) = self {
            return write!(formatter, "function allowed host is invalid: {detail}");
        }
        if let Self::VersionAlreadyUsed { version, next } = self {
            return write!(
                formatter,
                "version {version} of this function was used before and versions are never reused; deploy it as version {next}"
            );
        }
        formatter.write_str(match self {
            Self::UnsupportedDurability => "function durability is unsupported",
            Self::NotFound => "function resource was not found",
            Self::Forbidden => "function action is forbidden",
            Self::Conflict => "function resource changed concurrently",
            Self::CorruptRecord => "function record is corrupt",
            Self::InvalidFunction => "function name is invalid",
            Self::InvalidConfiguration => "function configuration is invalid",
            Self::InvalidAllowedHost(_) => "function allowed host is invalid",
            Self::InvalidDeployment => "function deployment is invalid",
            Self::BundleNotFound => "function bundle was not found",
            Self::CorruptBundle => "function bundle is corrupt",
            Self::InvalidBackendDiagnostic => "function backend diagnostic is invalid",
            Self::InvalidBackendResponse => "function backend response is invalid",
            Self::InvalidTestRequest => "function test request is invalid",
            Self::InvalidLogQuery => "function log query is invalid",
            Self::InvalidState => "function state is invalid",
            Self::DeploymentUnhealthy => "function deployment is not healthy",
            Self::NoActiveVersion => "function has no active version",
            Self::ActiveVersionCannotBeDeleted => "active function version cannot be deleted",
            Self::VersionAlreadyUsed { .. } => "function version was used before",
            Self::Backend(_) => "function backend operation failed",
            Self::Credential(_) => "function secret resolution failed",
            Self::Project(_) => "function project lookup failed",
            Self::Organization(_) => "function organization lookup failed",
            Self::Keyspace(_) => "function key is invalid",
            Self::Storage(_) => "function storage operation failed",
            Self::ObjectStore(_) => "function object storage operation failed",
            Self::Json(_) => "function record is invalid",
        })
    }
}
impl Error for FunctionAdminError {}
macro_rules! error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for FunctionAdminError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}
error_from!(FunctionBackendError, Backend);
error_from!(CredentialAdminError, Credential);
error_from!(ProjectStoreError, Project);
error_from!(OrganizationStoreError, Organization);
error_from!(ControlKeyspaceError, Keyspace);
error_from!(StorageError, Storage);
error_from!(ObjectStoreError, ObjectStore);
error_from!(serde_json::Error, Json);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DeveloperIdentityId, EnvironmentRecord, FunctionSecretEncryptionKey, MembershipRecord,
        OrganizationRecord, OrganizationRole, ProjectRecord,
    };
    use mako_api::{EnvironmentId, ProjectId};
    use mako_identity::KeyEncryptionKey;
    use mako_object_store::MemoryObjectStore;
    use mako_storage::MemoryAdapter;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Audit;
    impl ControlAuditSink for Audit {
        fn record(&self, _: ControlAuditEvent) {}
    }
    type CapturedSecret = (String, u64, String);

    #[derive(Default)]
    struct Backend {
        deleted: Mutex<Vec<String>>,
        deployed_secrets: Mutex<BTreeMap<u64, Vec<CapturedSecret>>>,
        tested_secrets: Mutex<Vec<CapturedSecret>>,
    }
    #[async_trait]
    impl FunctionDeploymentBackend for Backend {
        async fn deploy(
            &self,
            spec: &FunctionDeploymentSpec,
        ) -> Result<FunctionDeploymentResult, FunctionBackendError> {
            self.deployed_secrets
                .lock()
                .expect("deployed secrets")
                .insert(
                    spec.version,
                    spec.secrets
                        .iter()
                        .map(|secret| {
                            (
                                secret.reference.name.as_str().to_owned(),
                                secret.reference.version,
                                secret.expose_to_runtime_adapter().to_owned(),
                            )
                        })
                        .collect(),
                );
            Ok(FunctionDeploymentResult {
                healthy: spec.version != 3,
                diagnostic: (spec.version == 3).then(|| "health check failed".to_owned()),
            })
        }
        async fn health(
            &self,
            spec: &FunctionDeploymentSpec,
        ) -> Result<FunctionDeploymentResult, FunctionBackendError> {
            Ok(FunctionDeploymentResult {
                healthy: spec.version != 2,
                diagnostic: (spec.version == 2).then(|| "health probe failed".to_owned()),
            })
        }
        async fn test(
            &self,
            spec: &FunctionDeploymentSpec,
            _: &FunctionTestRequest,
        ) -> Result<FunctionTestResponse, FunctionBackendError> {
            *self.tested_secrets.lock().expect("tested secrets") = spec
                .secrets
                .iter()
                .map(|secret| {
                    (
                        secret.reference.name.as_str().to_owned(),
                        secret.reference.version,
                        secret.expose_to_runtime_adapter().to_owned(),
                    )
                })
                .collect();
            Ok(FunctionTestResponse {
                status: 200,
                headers: vec![],
                body: spec.version.to_string().into_bytes(),
                correlation_id: "corr_example00".to_owned(),
            })
        }
        async fn logs(
            &self,
            _: &TenantScope,
            _: &FunctionName,
            _: &FunctionLogQuery,
        ) -> Result<FunctionLogPage, FunctionBackendError> {
            let value = self
                .deployed_secrets
                .lock()
                .expect("deployed secrets")
                .get(&1)
                .and_then(|secrets| secrets.first())
                .map(|secret| secret.2.clone())
                .expect("version one secret");
            Ok(FunctionLogPage {
                items: vec![FunctionLogEntry {
                    timestamp_unix_milliseconds: 1_000,
                    level: "info".to_owned(),
                    message: format!("configured token={value}"),
                    correlation_id: "corr_example00".to_owned(),
                    version: 1,
                    region: "local".to_owned(),
                }],
                next_cursor: None,
            })
        }
        async fn delete_version(
            &self,
            _: &TenantScope,
            name: &FunctionName,
            version: u64,
        ) -> Result<(), FunctionBackendError> {
            self.deleted
                .lock()
                .expect("deleted")
                .push(format!("{}@{version}", name.as_str()));
            Ok(())
        }
        async fn delete_function(
            &self,
            _: &TenantScope,
            name: &FunctionName,
        ) -> Result<(), FunctionBackendError> {
            self.deleted
                .lock()
                .expect("deleted")
                .push(name.as_str().to_owned());
            Ok(())
        }
    }
    fn configuration(secret_names: Vec<FunctionSecretName>) -> FunctionConfiguration {
        FunctionConfiguration {
            verify_jwt: true,
            regions: vec!["local".to_owned()],
            secret_names,
            allowed_hosts: vec![],
            limits: FunctionLimits {
                cpu_milliseconds: 100,
                wall_milliseconds: 1000,
                memory_bytes: 1_000_000,
                request_bytes: 1000,
                response_bytes: 1000,
                concurrency: 2,
            },
        }
    }
    #[test]
    fn allowed_hosts_accept_public_dns_names_and_refuse_everything_else() {
        let mut valid = configuration(vec![]);
        valid.allowed_hosts = vec![
            "sandbox.plaid.com".to_owned(),
            "api.example.com".to_owned(),
            "api.example.com".to_owned(),
        ];
        valid.validate().expect("public DNS names are declarable");
        assert_eq!(
            valid.allowed_hosts,
            vec!["api.example.com".to_owned(), "sandbox.plaid.com".to_owned()],
            "hosts are sorted and deduplicated like every other list"
        );

        for (host, why) in [
            ("127.0.0.1", "an IPv4 literal"),
            ("2606:4700::1111", "an IPv6 literal"),
            ("sandbox.plaid.com:8443", "a port"),
            ("*.plaid.com", "a wildcard"),
            ("Sandbox.Plaid.com", "uppercase"),
            ("", "an empty host"),
            ("plaid..com", "an empty label"),
            ("-bad.example.com", "a leading hyphen"),
            ("localhost", "the loopback name"),
            ("metadata", "the bare metadata name"),
            ("metadata.google.internal", "cloud metadata"),
            ("host.docker.internal", "the container's route to the host"),
            ("printer.local", "an mDNS name"),
            ("1.0.0.127.in-addr.arpa", "reverse DNS"),
        ] {
            let mut config = configuration(vec![]);
            config.allowed_hosts = vec![host.to_owned()];
            let error = config.validate().expect_err(why);
            let FunctionAdminError::InvalidAllowedHost(detail) = &error else {
                panic!("{why} must be refused as an allowed-host error, got {error:?}");
            };
            assert!(
                host.is_empty() || detail.contains(host.trim_matches('*')) || host.contains(':'),
                "the refusal names the entry: {detail}"
            );
        }

        let mut oversized = configuration(vec![]);
        oversized.allowed_hosts = (0..9)
            .map(|index| format!("h{index}.example.com"))
            .collect();
        assert!(matches!(
            oversized.validate(),
            Err(FunctionAdminError::InvalidAllowedHost(_))
        ));
    }

    #[test]
    fn a_function_record_stored_before_allowed_hosts_existed_still_deserializes() {
        // The exact JSON shape `create_function` wrote before this field: a
        // record in `mako_kv` is a blob, so an old row must load as deny-all
        // rather than fail `deny_unknown_fields`.
        let stored = r#"{
            "tenant": {"projectId": "prj_example00", "environmentId": "env_example00"},
            "name": "hello-world",
            "state": "active",
            "activeVersion": 3,
            "configuration": {
                "verifyJwt": true,
                "regions": ["local"],
                "secretNames": [],
                "limits": {
                    "cpuMilliseconds": 100,
                    "wallMilliseconds": 1000,
                    "memoryBytes": 1000000,
                    "requestBytes": 1000,
                    "responseBytes": 1000,
                    "concurrency": 2
                }
            },
            "createdAtUnixSeconds": 1,
            "updatedAtUnixSeconds": 2
        }"#;
        let record: FunctionRecord = serde_json::from_str(stored).expect("old record loads");
        assert!(
            record.configuration().allowed_hosts.is_empty(),
            "a record from before the field defaults to declaring nothing"
        );
        // The promote and deploy paths compare-and-write against the
        // re-serialized stored record, so a round-trip of an old record must
        // reproduce its bytes' shape: no `allowedHosts` key may appear on the
        // way out when none was stored (finding #41 -- with it, no function
        // written before the field could ever be promoted again).
        let round_tripped = serde_json::to_value(&record).expect("record serializes");
        assert!(
            round_tripped["configuration"].get("allowedHosts").is_none(),
            "an empty declaration must serialize to nothing: {round_tripped}"
        );
        assert_eq!(
            serde_json::to_value(&record).expect("record serializes"),
            serde_json::from_str::<serde_json::Value>(stored).expect("stored json"),
            "a round-trip of a pre-field record is value-identical"
        );
    }

    #[test]
    fn deploy_promote_rollback_test_logs_and_delete_follow_safe_lifecycle() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let organizations =
                OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
            let projects =
                ProjectStore::new(adapter.clone(), Durability::Memory).expect("projects");
            let organization = OrganizationId::parse("org_example00").expect("organization");
            let developer = DeveloperIdentityId::parse("dev_example00").expect("developer");
            organizations
                .create_organization(
                    &OrganizationRecord::new(organization.clone(), "Example", 1)
                        .expect("organization"),
                    &MembershipRecord::new(
                        organization.clone(),
                        developer.clone(),
                        OrganizationRole::Owner,
                        1,
                    ),
                )
                .await
                .expect("organization");
            let project = ProjectId::parse("prj_example00").expect("project");
            let environment = EnvironmentId::parse("env_example00").expect("environment");
            projects
                .create_project(
                    &ProjectRecord::new(project.clone(), organization, "Mako", "local", 1)
                        .expect("project"),
                )
                .await
                .expect("project");
            projects
                .create_environment(
                    &EnvironmentRecord::new(environment.clone(), project.clone(), "Development", 1)
                        .expect("environment"),
                )
                .await
                .expect("environment");
            let tenant = TenantScope::new(project, environment);
            let actor = DeveloperPrincipal::for_test(developer, "owner@example.test");
            let audit: Arc<dyn ControlAuditSink> = Arc::new(Audit);
            let credentials = CredentialAdminService::new(
                adapter.clone(),
                Durability::Memory,
                projects.clone(),
                organizations.clone(),
                audit.clone(),
                KeyEncryptionKey::generate(),
                FunctionSecretEncryptionKey::generate(),
            )
            .expect("credentials");
            let secret_name = FunctionSecretName::parse("API_TOKEN").expect("secret name");
            let initial_secret = credentials
                .create_function_secret(&actor, &tenant, secret_name.clone(), 2)
                .await
                .expect("create secret")
                .value
                .expose_once()
                .to_owned();
            let backend = Arc::new(Backend::default());
            let service = FunctionAdminService::new(
                adapter,
                Durability::Memory,
                projects,
                organizations,
                audit,
                credentials.clone(),
                Arc::new(MemoryObjectStore::default()),
                backend.clone(),
            )
            .expect("service");
            let name = FunctionName::parse("hello-world").expect("name");
            service
                .create_function(
                    &actor,
                    NewFunction {
                        tenant: tenant.clone(),
                        name: name.clone(),
                        configuration: configuration(vec![secret_name.clone()]),
                        now_unix_seconds: 2,
                    },
                )
                .await
                .expect("function");
            let bundle = service
                .upload_bundle(
                    &actor,
                    &tenant,
                    FunctionBundleUpload::Source {
                        entrypoint: "index.ts".to_owned(),
                        files: vec![crate::FunctionSourceFile {
                            path: "index.ts".to_owned(),
                            contents: b"export default () => new Response('ok');\n".to_vec(),
                        }],
                        dependencies: std::collections::BTreeMap::new(),
                    },
                    2,
                )
                .await
                .expect("upload bundle")
                .artifact
                .expect("valid artifact");
            for version in [1, 2, 3] {
                let record = service
                    .deploy_version(
                        &actor,
                        NewFunctionVersion {
                            tenant: tenant.clone(),
                            function_name: name.clone(),
                            version,
                            bundle_digest: bundle.digest().to_owned(),
                            entrypoint: "index.ts".to_owned(),
                            runtime_version: "deno-test".to_owned(),
                            now_unix_seconds: 2 + version,
                        },
                    )
                    .await
                    .expect("deploy");
                assert_eq!(
                    record.state(),
                    if version == 3 {
                        FunctionVersionState::Failed
                    } else {
                        FunctionVersionState::Healthy
                    }
                );
                assert_eq!(record.secret_versions().len(), 1);
                assert_eq!(record.secret_versions()[0].name, secret_name);
                assert_eq!(
                    record.secret_versions()[0].version,
                    if version == 1 { 1 } else { 2 }
                );
                let encoded = serde_json::to_string(&record).expect("serialize deployment");
                assert!(!encoded.contains(&initial_secret));
                if version == 1 {
                    credentials
                        .rotate_function_secret(&actor, &tenant, &secret_name, 4)
                        .await
                        .expect("rotate secret");
                }
            }
            assert_eq!(
                service
                    .promote(&actor, &tenant, &name, 1, 6)
                    .await
                    .expect("promote")
                    .active_version(),
                Some(1)
            );
            assert!(service.promote(&actor, &tenant, &name, 3, 7).await.is_err());
            service
                .promote(&actor, &tenant, &name, 2, 8)
                .await
                .expect("promote two");
            assert_eq!(
                service
                    .rollback(&actor, &tenant, &name, 1, 9)
                    .await
                    .expect("rollback")
                    .active_version(),
                Some(1)
            );
            let health = service
                .check_version_health(&actor, &tenant, &name, 2, 9)
                .await
                .expect("health check");
            assert_eq!(health.state(), FunctionVersionState::Failed);
            assert_eq!(health.diagnostic(), Some("health probe failed"));
            assert!(service.promote(&actor, &tenant, &name, 2, 9).await.is_err());
            let response = service
                .test_invoke(
                    &actor,
                    &tenant,
                    &name,
                    None,
                    &FunctionTestRequest {
                        method: "POST".to_owned(),
                        path: "/".to_owned(),
                        headers: vec![],
                        body: vec![],
                    },
                    10,
                )
                .await
                .expect("test");
            assert_eq!(response.body, b"1");
            assert_eq!(
                backend
                    .tested_secrets
                    .lock()
                    .expect("tested secrets")
                    .as_slice(),
                &[("API_TOKEN".to_owned(), 1, initial_secret.clone())]
            );
            let logs = service
                .logs(
                    &actor,
                    &tenant,
                    &name,
                    &FunctionLogQuery {
                        cursor: None,
                        limit: 10,
                    },
                    11,
                )
                .await
                .expect("logs");
            assert_eq!(logs.items.len(), 1);
            assert_eq!(logs.items[0].message, "configured token=[REDACTED]");
            assert!(!logs.items[0].message.contains(&initial_secret));
            service
                .delete_version(&actor, &tenant, &name, 2, 12)
                .await
                .expect("delete version");
            // A deleted version's number is spent: deploying it again would
            // make "version 2" in logs and audit mean two bundles.
            let reused = service
                .deploy_version(
                    &actor,
                    NewFunctionVersion {
                        tenant: tenant.clone(),
                        function_name: name.clone(),
                        version: 2,
                        bundle_digest: bundle.digest().to_owned(),
                        entrypoint: "index.ts".to_owned(),
                        runtime_version: "deno-test".to_owned(),
                        now_unix_seconds: 12,
                    },
                )
                .await
                .expect_err("a spent version number");
            assert!(matches!(
                reused,
                FunctionAdminError::VersionAlreadyUsed {
                    version: 2,
                    next: 4
                }
            ));
            assert!(reused.to_string().contains("deploy it as version 4"));
            assert_eq!(
                service
                    .get_function(&actor, &tenant, &name, 12)
                    .await
                    .expect("function")
                    .next_version(),
                Some(4)
            );
            assert_eq!(
                service
                    .delete_function(&actor, &tenant, &name, 13)
                    .await
                    .expect("delete function")
                    .state(),
                FunctionState::Deleted
            );
        });
    }

    /// A backend with no runtime behind it: every call fails, the way the
    /// HTTP backend does when the supervisor cannot be reached.
    #[derive(Default)]
    struct Unreachable {
        calls: Mutex<Vec<&'static str>>,
    }
    impl Unreachable {
        fn refuse<T>(&self, call: &'static str) -> Result<T, FunctionBackendError> {
            self.calls.lock().expect("calls").push(call);
            Err(
                FunctionBackendError::new("the runtime supervisor could not be reached")
                    .expect("safe message"),
            )
        }
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().expect("calls").clone()
        }
    }
    #[async_trait]
    impl FunctionDeploymentBackend for Unreachable {
        async fn deploy(
            &self,
            _: &FunctionDeploymentSpec,
        ) -> Result<FunctionDeploymentResult, FunctionBackendError> {
            self.refuse("deploy")
        }
        async fn health(
            &self,
            _: &FunctionDeploymentSpec,
        ) -> Result<FunctionDeploymentResult, FunctionBackendError> {
            self.refuse("health")
        }
        async fn test(
            &self,
            _: &FunctionDeploymentSpec,
            _: &FunctionTestRequest,
        ) -> Result<FunctionTestResponse, FunctionBackendError> {
            self.refuse("test")
        }
        async fn logs(
            &self,
            _: &TenantScope,
            _: &FunctionName,
            _: &FunctionLogQuery,
        ) -> Result<FunctionLogPage, FunctionBackendError> {
            self.refuse("logs")
        }
        async fn delete_version(
            &self,
            _: &TenantScope,
            _: &FunctionName,
            _: u64,
        ) -> Result<(), FunctionBackendError> {
            self.refuse("delete_version")
        }
        async fn delete_function(
            &self,
            _: &TenantScope,
            _: &FunctionName,
        ) -> Result<(), FunctionBackendError> {
            self.refuse("delete_function")
        }
    }

    async fn service_without_runtime(
        backend: Arc<Unreachable>,
    ) -> (FunctionAdminService, DeveloperPrincipal, TenantScope) {
        let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
        let organizations =
            OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
        let projects = ProjectStore::new(adapter.clone(), Durability::Memory).expect("projects");
        let organization = OrganizationId::parse("org_example00").expect("organization");
        let developer = DeveloperIdentityId::parse("dev_example00").expect("developer");
        organizations
            .create_organization(
                &OrganizationRecord::new(organization.clone(), "Example", 1).expect("organization"),
                &MembershipRecord::new(
                    organization.clone(),
                    developer.clone(),
                    OrganizationRole::Owner,
                    1,
                ),
            )
            .await
            .expect("organization");
        let project = ProjectId::parse("prj_example00").expect("project");
        let environment = EnvironmentId::parse("env_example00").expect("environment");
        projects
            .create_project(
                &ProjectRecord::new(project.clone(), organization, "Mako", "local", 1)
                    .expect("project"),
            )
            .await
            .expect("project");
        projects
            .create_environment(
                &EnvironmentRecord::new(environment.clone(), project.clone(), "Development", 1)
                    .expect("environment"),
            )
            .await
            .expect("environment");
        let audit: Arc<dyn ControlAuditSink> = Arc::new(Audit);
        let credentials = CredentialAdminService::new(
            adapter.clone(),
            Durability::Memory,
            projects.clone(),
            organizations.clone(),
            audit.clone(),
            KeyEncryptionKey::generate(),
            FunctionSecretEncryptionKey::generate(),
        )
        .expect("credentials");
        let service = FunctionAdminService::new(
            adapter,
            Durability::Memory,
            projects,
            organizations,
            audit,
            credentials,
            Arc::new(MemoryObjectStore::default()),
            backend,
        )
        .expect("service");
        (
            service,
            DeveloperPrincipal::for_test(developer, "owner@example.test"),
            TenantScope::new(project, environment),
        )
    }

    /// Deleting a function, or reading its logs, used to ask the runtime
    /// first, so neither worked while no runtime was reachable -- not even
    /// for a function that had never been deployed and held nothing there.
    #[test]
    fn a_function_never_handed_to_the_runtime_is_deleted_without_one() {
        futures::executor::block_on(async {
            let backend = Arc::new(Unreachable::default());
            let (service, actor, tenant) = service_without_runtime(backend.clone()).await;
            let create = |name: &'static str| {
                let service = &service;
                let actor = &actor;
                let tenant = tenant.clone();
                async move {
                    service
                        .create_function(
                            actor,
                            NewFunction {
                                tenant,
                                name: FunctionName::parse(name).expect("name"),
                                configuration: configuration(vec![]),
                                now_unix_seconds: 2,
                            },
                        )
                        .await
                        .expect("create function")
                }
            };
            let query = FunctionLogQuery {
                cursor: None,
                limit: 10,
            };

            // Never deployed: no logs to read and nothing to retire.
            let fresh = create("never-deployed").await;
            assert_eq!(
                serde_json::to_value(&fresh).expect("record")["runtimeUntouched"],
                true
            );
            let logs = service
                .logs(&actor, &tenant, fresh.name(), &query, 3)
                .await
                .expect("logs of a function that never ran");
            assert!(logs.items.is_empty() && logs.next_cursor.is_none());
            let deleted = service
                .delete_function(&actor, &tenant, fresh.name(), 4)
                .await
                .expect("delete without a runtime");
            assert_eq!(deleted.state(), FunctionState::Deleted);
            assert!(
                backend.calls().is_empty(),
                "the runtime was asked: {:?}",
                backend.calls()
            );

            // A deploy that reached for the runtime, even one that failed, may
            // have left a deployment there, so its delete still retires.
            let attempted = create("deploy-attempted").await;
            let bundle = service
                .upload_bundle(
                    &actor,
                    &tenant,
                    FunctionBundleUpload::Source {
                        entrypoint: "index.ts".to_owned(),
                        files: vec![crate::FunctionSourceFile {
                            path: "index.ts".to_owned(),
                            contents: b"export default () => new Response('ok');\n".to_vec(),
                        }],
                        dependencies: std::collections::BTreeMap::new(),
                    },
                    5,
                )
                .await
                .expect("upload bundle")
                .artifact
                .expect("valid artifact");
            assert!(
                service
                    .deploy_version(
                        &actor,
                        NewFunctionVersion {
                            tenant: tenant.clone(),
                            function_name: attempted.name().clone(),
                            version: 1,
                            bundle_digest: bundle.digest().to_owned(),
                            entrypoint: "index.ts".to_owned(),
                            runtime_version: "deno-test".to_owned(),
                            now_unix_seconds: 6,
                        },
                    )
                    .await
                    .is_err()
            );
            assert!(
                service
                    .delete_function(&actor, &tenant, attempted.name(), 7)
                    .await
                    .is_err()
            );
            assert!(
                service
                    .logs(&actor, &tenant, attempted.name(), &query, 7)
                    .await
                    .is_err()
            );
            assert_eq!(backend.calls(), ["deploy", "delete_function", "logs"]);

            // A record stored before the flag existed may have been deployed,
            // so it is treated like one that was.
            let older = create("from-before").await;
            let mut stored_before = older.clone();
            stored_before.runtime_untouched = false;
            service
                .replace_function(&older, &stored_before)
                .await
                .expect("store the older shape");
            assert!(
                serde_json::to_value(&stored_before)
                    .expect("record")
                    .get("runtimeUntouched")
                    .is_none()
            );
            assert!(
                service
                    .delete_function(&actor, &tenant, older.name(), 8)
                    .await
                    .is_err()
            );
            assert_eq!(backend.calls().last(), Some(&"delete_function"));
        });
    }
}
