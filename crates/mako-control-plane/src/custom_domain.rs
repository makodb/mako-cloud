//! Custom domains: a project's API and functions served on the developer's
//! own hostname, proven by a DNS TXT record and gated end to end.
//!
//! A domain is registered `pending` with a verification record the developer
//! publishes at `_mako-verify.<hostname>`. A control-plane worker checks the
//! record every minute (and on request), marks the domain `verified` when the
//! value is present, and publishes each environment's verified hostnames to
//! the data plane, which is what the edge gateway's route resolution, the
//! data plane's request gate, and Caddy's on-demand TLS `ask` endpoint all
//! answer from. Nothing is served on a hostname before that.
//!
//! Serving is withdrawn when the proof goes away -- but only after **two**
//! consecutive checks that answered and did not find it, so a resolver
//! hiccup between two good checks never takes a production hostname down.
//! A check the resolver could not answer at all changes nothing. A domain
//! that fails re-verification becomes `verified` again as soon as the
//! record is back.
//!
//! A hostname belongs to at most one project on the node: a global owner
//! index is claimed in the same atomic write as the record.

use std::{
    collections::HashSet,
    error::Error,
    fmt,
    net::IpAddr,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use mako_api::{EnvironmentId, ProjectId, TenantScope};
use mako_internal_rpc::{
    ControlToDataClient, IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission,
    InstallCustomDomainsInput, InternalClientError,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, DeveloperPrincipal, OrganizationId, OrganizationStore,
    OrganizationStoreError, ProjectStore, ProjectStoreError,
    dns::{DnsError, TxtResolver},
};

/// The verification record lives under this label of the hostname.
pub const VERIFICATION_RECORD_PREFIX: &str = "_mako-verify.";
pub const VERIFICATION_RECORD_TYPE: &str = "TXT";
/// Every verification value starts with this; the rest is random.
pub const VERIFICATION_VALUE_PREFIX: &str = "mako-domain-verify=";
pub const MAXIMUM_HOSTNAME_BYTES: usize = 253;
pub const MAXIMUM_DOMAINS_PER_PROJECT: usize = 20;
/// A `verified` domain becomes `failed` only after this many consecutive
/// checks that answered and did not find the record.
pub const FAILURES_BEFORE_REVOCATION: u32 = 2;
/// Stable `lastError` values.
pub const ERROR_RECORD_MISSING: &str = "record_missing";
pub const ERROR_RECORD_MISMATCH: &str = "record_mismatch";
pub const ERROR_DNS_UNAVAILABLE: &str = "dns_unavailable";

const ID_SUFFIX_LENGTH: usize = 16;
const VERIFICATION_TOKEN_LENGTH: usize = 32;
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const MAXIMUM_LABEL_BYTES: usize = 63;
/// Every scan is bounded; a saturated scan fails closed.
const MAXIMUM_REGISTRY_SCAN: usize = 10_000;
const MAXIMUM_PROJECT_SCAN: usize = MAXIMUM_DOMAINS_PER_PROJECT + 100;
const WORKER_ACTOR_ID: &str = "system/custom-domain-verifier";

/// `^dom_[a-z0-9]{12,32}$`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CustomDomainId(String);

impl CustomDomainId {
    pub fn parse(value: impl Into<String>) -> Result<Self, CustomDomainError> {
        let value = value.into();
        let Some(suffix) = value.strip_prefix("dom_") else {
            return Err(CustomDomainError::InvalidRequest("domain id is invalid"));
        };
        if !(12..=32).contains(&suffix.len())
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        {
            return Err(CustomDomainError::InvalidRequest("domain id is invalid"));
        }
        Ok(Self(value))
    }

    /// `dom_` and sixteen random lowercase alphanumerics.
    #[must_use]
    pub fn generate() -> Self {
        Self(format!("dom_{}", random_alphanumerics(ID_SUFFIX_LENGTH)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CustomDomainId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Unbiased characters from the lowercase alphanumeric alphabet.
fn random_alphanumerics(length: usize) -> String {
    let mut output = String::with_capacity(length);
    let mut buffer = [0_u8; 64];
    while output.len() < length {
        OsRng.fill_bytes(&mut buffer);
        for byte in buffer {
            // 252 is the largest multiple of 36 below 256; rejecting the
            // rest keeps every character equally likely.
            if byte < 252 && output.len() < length {
                output.push(char::from(ALPHABET[usize::from(byte % 36)]));
            }
        }
    }
    output
}

fn random_token(prefix: &str) -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut token = String::with_capacity(prefix.len() + 32);
    token.push_str(prefix);
    for byte in bytes {
        use fmt::Write as _;
        let _ = write!(token, "{byte:02x}");
    }
    token
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomDomainState {
    Pending,
    Verified,
    Failed,
}

impl CustomDomainState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Verified => "verified",
            Self::Failed => "failed",
        }
    }
}

/// A registered hostname. Mirrors the public `CustomDomain`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CustomDomainRecord {
    id: CustomDomainId,
    project_id: ProjectId,
    environment_id: EnvironmentId,
    hostname: String,
    state: CustomDomainState,
    record_value: String,
    verified_at_unix_seconds: Option<u64>,
    last_checked_at_unix_seconds: Option<u64>,
    last_error: Option<String>,
    /// Checks in a row that answered without the record while the domain
    /// was `verified`; reset by a good check.
    consecutive_misses: u32,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

impl CustomDomainRecord {
    fn new(
        id: CustomDomainId,
        project_id: ProjectId,
        environment_id: EnvironmentId,
        hostname: String,
        now_unix_seconds: u64,
    ) -> Self {
        Self {
            id,
            project_id,
            environment_id,
            hostname,
            state: CustomDomainState::Pending,
            record_value: format!(
                "{VERIFICATION_VALUE_PREFIX}{}",
                random_alphanumerics(VERIFICATION_TOKEN_LENGTH)
            ),
            verified_at_unix_seconds: None,
            last_checked_at_unix_seconds: None,
            last_error: None,
            consecutive_misses: 0,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
        }
    }

    #[must_use]
    pub fn id(&self) -> &CustomDomainId {
        &self.id
    }

    #[must_use]
    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    #[must_use]
    pub fn environment_id(&self) -> &EnvironmentId {
        &self.environment_id
    }

    #[must_use]
    pub fn tenant(&self) -> TenantScope {
        TenantScope::new(self.project_id.clone(), self.environment_id.clone())
    }

    #[must_use]
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    #[must_use]
    pub const fn state(&self) -> CustomDomainState {
        self.state
    }

    /// `_mako-verify.<hostname>`.
    #[must_use]
    pub fn record_name(&self) -> String {
        verification_record_name(&self.hostname)
    }

    #[must_use]
    pub fn record_value(&self) -> &str {
        &self.record_value
    }

    #[must_use]
    pub const fn verified_at_unix_seconds(&self) -> Option<u64> {
        self.verified_at_unix_seconds
    }

    #[must_use]
    pub const fn last_checked_at_unix_seconds(&self) -> Option<u64> {
        self.last_checked_at_unix_seconds
    }

    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    #[must_use]
    pub const fn consecutive_misses(&self) -> u32 {
        self.consecutive_misses
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }
}

/// The name the verification TXT record is published at.
#[must_use]
pub fn verification_record_name(hostname: &str) -> String {
    format!("{VERIFICATION_RECORD_PREFIX}{hostname}")
}

/// `CustomDomainCreate`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewCustomDomain {
    pub hostname: String,
    pub environment_id: EnvironmentId,
}

/// Lowercase, without surrounding whitespace or one trailing dot.
#[must_use]
pub fn normalize_hostname(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_suffix('.')
        .unwrap_or(trimmed)
        .to_ascii_lowercase()
}

/// The hostname a project may claim: a lowercase DNS name of at least two
/// labels, not an address, not `localhost`, and neither the platform's own
/// public hostname nor anything under it.
pub fn validate_hostname(
    value: &str,
    platform_hostname: Option<&str>,
) -> Result<String, CustomDomainError> {
    let invalid = CustomDomainError::InvalidHostname;
    let hostname = normalize_hostname(value);
    if hostname.is_empty() || hostname.len() > MAXIMUM_HOSTNAME_BYTES {
        return Err(invalid("hostname must be between 1 and 253 characters"));
    }
    if hostname.parse::<IpAddr>().is_ok() || hostname.starts_with('[') {
        return Err(invalid("hostname must be a DNS name, not an IP address"));
    }
    let labels = hostname.split('.').collect::<Vec<_>>();
    if labels.len() < 2 {
        return Err(invalid("hostname must have at least two labels"));
    }
    for label in &labels {
        if label.is_empty()
            || label.len() > MAXIMUM_LABEL_BYTES
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || label.starts_with('-')
            || label.ends_with('-')
        {
            return Err(invalid(
                "hostname labels must be 1 to 63 lowercase letters, digits, or hyphens, not starting or ending with a hyphen",
            ));
        }
    }
    if labels
        .last()
        .is_some_and(|top| top.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid("hostname must be a DNS name, not an IP address"));
    }
    if hostname == "localhost" || hostname.ends_with(".localhost") {
        return Err(invalid("hostname must not be localhost"));
    }
    if let Some(platform) = platform_hostname.map(normalize_hostname)
        && !platform.is_empty()
        && (hostname == platform || hostname.ends_with(&format!(".{platform}")))
    {
        return Err(invalid(
            "hostname must not be the platform's own hostname or one of its subdomains",
        ));
    }
    Ok(hostname)
}

/// The global owner index entry for a hostname.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HostnameOwner {
    project_id: ProjectId,
    environment_id: EnvironmentId,
    domain_id: CustomDomainId,
}

/// The domain records and the hostname owner index in the control store.
#[derive(Clone)]
pub struct CustomDomainStore {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for CustomDomainStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CustomDomainStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl CustomDomainStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
    ) -> Result<Self, CustomDomainError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(CustomDomainError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
        })
    }

    async fn apply(&self, write: AtomicWrite) -> Result<bool, CustomDomainError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(true),
            CompareAndWriteResult::Conflict { .. } => Ok(false),
        }
    }

    pub async fn get(
        &self,
        project_id: &ProjectId,
        id: &CustomDomainId,
    ) -> Result<Option<CustomDomainRecord>, CustomDomainError> {
        let key = ControlKeyspace::custom_domain_key(project_id, id.as_str())?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: CustomDomainRecord = serde_json::from_slice(&value)?;
        if record.project_id() != project_id || record.id() != id {
            return Err(CustomDomainError::CorruptRecord);
        }
        Ok(Some(record))
    }

    /// A project's domains, oldest first.
    pub async fn list(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<CustomDomainRecord>, CustomDomainError> {
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::custom_domains_range(project_id)?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_PROJECT_SCAN).ok_or(CustomDomainError::LimitExceeded)?,
            ))
            .await?;
        if entries.len() >= MAXIMUM_PROJECT_SCAN {
            return Err(CustomDomainError::LimitExceeded);
        }
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            let record: CustomDomainRecord = serde_json::from_slice(&entry.value)?;
            if record.project_id() != project_id {
                return Err(CustomDomainError::CorruptRecord);
            }
            records.push(record);
        }
        records.sort_by_key(|record| (record.created_at_unix_seconds, record.id.clone()));
        Ok(records)
    }

    /// Every registered domain on the node, by walking the hostname index.
    pub async fn registry(&self) -> Result<Vec<(ProjectId, CustomDomainId)>, CustomDomainError> {
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::custom_domain_hostnames_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_REGISTRY_SCAN).ok_or(CustomDomainError::LimitExceeded)?,
            ))
            .await?;
        if entries.len() >= MAXIMUM_REGISTRY_SCAN {
            return Err(CustomDomainError::LimitExceeded);
        }
        let mut registry = Vec::with_capacity(entries.len());
        for entry in entries {
            let owner: HostnameOwner = serde_json::from_slice(&entry.value)?;
            registry.push((owner.project_id, owner.domain_id));
        }
        Ok(registry)
    }

    /// The record that claimed a (normalized) hostname, if any.
    pub async fn by_hostname(
        &self,
        hostname: &str,
    ) -> Result<Option<CustomDomainRecord>, CustomDomainError> {
        let key = ControlKeyspace::custom_domain_hostname_key(hostname)?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let owner: HostnameOwner = serde_json::from_slice(&value)?;
        let record = self.get(&owner.project_id, &owner.domain_id).await?;
        if record
            .as_ref()
            .is_some_and(|record| record.hostname() != hostname)
        {
            return Err(CustomDomainError::CorruptRecord);
        }
        Ok(record)
    }

    /// The verified hostnames an environment is served on.
    pub async fn verified_hostnames(
        &self,
        tenant: &TenantScope,
    ) -> Result<Vec<String>, CustomDomainError> {
        Ok(self
            .list(tenant.project_id())
            .await?
            .into_iter()
            .filter(|record| {
                record.environment_id() == tenant.environment_id()
                    && record.state() == CustomDomainState::Verified
            })
            .map(|record| record.hostname)
            .collect())
    }

    /// Claims the hostname and writes the record in one decision; `false`
    /// when the hostname (or, improbably, the id) is already taken.
    async fn insert(&self, record: &CustomDomainRecord) -> Result<bool, CustomDomainError> {
        let record_key =
            ControlKeyspace::custom_domain_key(&record.project_id, record.id.as_str())?;
        let hostname_key = ControlKeyspace::custom_domain_hostname_key(&record.hostname)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&record_key, serde_json::to_vec(record)?);
        batch.put(
            &hostname_key,
            serde_json::to_vec(&HostnameOwner {
                project_id: record.project_id.clone(),
                environment_id: record.environment_id.clone(),
                domain_id: record.id.clone(),
            })?,
        );
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::Missing { key: hostname_key },
                KeyCondition::Missing { key: record_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    /// Replaces `previous` with `next` only if the stored record is still
    /// `previous`; `false` when someone else changed it first.
    async fn replace(
        &self,
        previous: &CustomDomainRecord,
        next: &CustomDomainRecord,
    ) -> Result<bool, CustomDomainError> {
        let key = ControlKeyspace::custom_domain_key(&previous.project_id, previous.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(1);
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

    async fn remove(&self, record: &CustomDomainRecord) -> Result<(), CustomDomainError> {
        let mut batch = WriteBatch::with_capacity(2);
        batch.delete(ControlKeyspace::custom_domain_key(
            &record.project_id,
            record.id.as_str(),
        )?);
        batch.delete(ControlKeyspace::custom_domain_hostname_key(
            &record.hostname,
        )?);
        self.adapter
            .compare_and_write(AtomicWrite::unconditional(batch, self.durability))
            .await?;
        Ok(())
    }
}

/// Where the verified list goes: the data plane, in production.
pub trait CustomDomainPublisher: Send + Sync {
    /// Installs the complete verified list for one environment; an empty
    /// list withdraws serving on custom domains for it.
    fn publish(
        &self,
        tenant: &TenantScope,
        hostnames: &[String],
    ) -> Result<(), CustomDomainPublishError>;
}

impl CustomDomainPublisher for ControlToDataClient {
    fn publish(
        &self,
        tenant: &TenantScope,
        hostnames: &[String],
    ) -> Result<(), CustomDomainPublishError> {
        let command = IdentityAdminCommand {
            operation: IdentityAdminOperation::InstallCustomDomains,
            actor_id: WORKER_ACTOR_ID.to_owned(),
            permissions: std::iter::once(IdentityAdminPermission::ManageProjectCredentials)
                .collect(),
            input: serde_json::to_value(InstallCustomDomainsInput {
                hostnames: hostnames.to_vec(),
            })
            .map_err(|_| CustomDomainPublishError::Unavailable(None))?,
        };
        let request_id = random_token("req_");
        let idempotency_key = random_token("idem_");
        let _: Value = self
            .administer(tenant, &request_id, &idempotency_key, &command)
            .map_err(|error| CustomDomainPublishError::Unavailable(Some(error)))?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum CustomDomainPublishError {
    Unavailable(Option<InternalClientError>),
}

impl fmt::Display for CustomDomainPublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("custom domain list could not be published to the data plane")
    }
}

impl Error for CustomDomainPublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable(error) => error.as_ref().map(|error| error as &dyn Error),
        }
    }
}

/// What one check found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CustomDomainCheckOutcome {
    Verified,
    RecordMissing,
    RecordMismatch,
    DnsUnavailable,
}

impl CustomDomainCheckOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::RecordMissing => ERROR_RECORD_MISSING,
            Self::RecordMismatch => ERROR_RECORD_MISMATCH,
            Self::DnsUnavailable => ERROR_DNS_UNAVAILABLE,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CustomDomainVerifierReport {
    pub checked: usize,
    pub verified: usize,
    pub record_missing: usize,
    pub record_mismatch: usize,
    pub dns_unavailable: usize,
    /// Domains that went from `verified` to `failed` this pass.
    pub revoked: usize,
    /// Environments whose list was published this pass.
    pub published: usize,
    pub publish_failures: usize,
}

/// Publication bookkeeping shared by the worker and the on-request check.
#[derive(Default)]
struct PublicationState {
    /// The first pass republishes every environment, so a data plane that
    /// missed an install while the control plane was down catches up.
    synced_once: bool,
    /// Environments whose last publication failed; retried every pass.
    pending: HashSet<TenantScope>,
}

/// Checks records, moves domains between states, and publishes the lists.
#[derive(Clone)]
pub struct CustomDomainVerifier {
    store: CustomDomainStore,
    resolver: Arc<dyn TxtResolver>,
    publisher: Arc<dyn CustomDomainPublisher>,
    publication: Arc<Mutex<PublicationState>>,
}

impl fmt::Debug for CustomDomainVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CustomDomainVerifier")
            .finish_non_exhaustive()
    }
}

impl CustomDomainVerifier {
    #[must_use]
    pub fn new(
        store: CustomDomainStore,
        resolver: Arc<dyn TxtResolver>,
        publisher: Arc<dyn CustomDomainPublisher>,
    ) -> Self {
        Self {
            store,
            resolver,
            publisher,
            publication: Arc::new(Mutex::new(PublicationState::default())),
        }
    }

    #[must_use]
    pub fn store(&self) -> &CustomDomainStore {
        &self.store
    }

    /// One pass: every registered domain is checked, state changes are
    /// stored, and every environment whose list changed -- plus any whose
    /// last publication failed, and all of them on the first pass -- is
    /// published. DNS lookups run one after another on the calling thread.
    pub async fn run_once(
        &self,
        now_unix_seconds: u64,
    ) -> Result<CustomDomainVerifierReport, CustomDomainError> {
        let mut report = CustomDomainVerifierReport::default();
        let mut to_publish = HashSet::new();
        let mut all_tenants = HashSet::new();
        for (project_id, domain_id) in self.store.registry().await? {
            let Some(record) = self.store.get(&project_id, &domain_id).await? else {
                continue;
            };
            all_tenants.insert(record.tenant());
            let (next, outcome) = self.check(&record, now_unix_seconds);
            report.checked += 1;
            match outcome {
                CustomDomainCheckOutcome::Verified => report.verified += 1,
                CustomDomainCheckOutcome::RecordMissing => report.record_missing += 1,
                CustomDomainCheckOutcome::RecordMismatch => report.record_mismatch += 1,
                CustomDomainCheckOutcome::DnsUnavailable => report.dns_unavailable += 1,
            }
            if !self.store.replace(&record, &next).await? {
                // Changed under us (a delete or an on-request check); the
                // other writer publishes for it.
                continue;
            }
            if next.state() != record.state() {
                if next.state() == CustomDomainState::Failed {
                    report.revoked += 1;
                }
                to_publish.insert(record.tenant());
            }
        }
        {
            let mut publication = self
                .publication
                .lock()
                .map_err(|_| CustomDomainError::Poisoned)?;
            if !publication.synced_once {
                to_publish.extend(all_tenants);
            }
            to_publish.extend(publication.pending.drain());
            publication.synced_once = true;
        }
        for tenant in to_publish {
            match self.publish_tenant(&tenant).await {
                Ok(()) => report.published += 1,
                Err(_) => report.publish_failures += 1,
            }
        }
        Ok(report)
    }

    /// One check of one domain, applied and published: what
    /// `actions/verify` runs. Returns the stored record afterwards.
    pub async fn verify_now(
        &self,
        record: &CustomDomainRecord,
        now_unix_seconds: u64,
    ) -> Result<(CustomDomainRecord, CustomDomainCheckOutcome), CustomDomainError> {
        let (next, outcome) = self.check(record, now_unix_seconds);
        let stored = if self.store.replace(record, &next).await? {
            next
        } else {
            self.store
                .get(record.project_id(), record.id())
                .await?
                .ok_or(CustomDomainError::DomainNotFound)?
        };
        if stored.state() != record.state() {
            // A failed publication is retried by the worker; the state is
            // already what the developer sees.
            let _ = self.publish_tenant(&record.tenant()).await;
        }
        Ok((stored, outcome))
    }

    /// Publishes an environment's current verified list, remembering a
    /// failure for the next pass.
    pub async fn publish_tenant(&self, tenant: &TenantScope) -> Result<(), CustomDomainError> {
        let hostnames = self.store.verified_hostnames(tenant).await?;
        match self.publisher.publish(tenant, &hostnames) {
            Ok(()) => {
                if let Ok(mut publication) = self.publication.lock() {
                    publication.pending.remove(tenant);
                }
                Ok(())
            }
            Err(error) => {
                if let Ok(mut publication) = self.publication.lock() {
                    publication.pending.insert(tenant.clone());
                }
                Err(CustomDomainError::Publish(error))
            }
        }
    }

    /// The state machine, on one record, from one lookup.
    fn check(
        &self,
        record: &CustomDomainRecord,
        now_unix_seconds: u64,
    ) -> (CustomDomainRecord, CustomDomainCheckOutcome) {
        let mut next = record.clone();
        next.last_checked_at_unix_seconds = Some(now_unix_seconds);
        next.updated_at_unix_seconds = now_unix_seconds;
        let outcome = match self.resolver.lookup_txt(&record.record_name()) {
            Ok(values) => {
                if values
                    .iter()
                    .any(|value| value.trim() == record.record_value())
                {
                    CustomDomainCheckOutcome::Verified
                } else if values.is_empty() {
                    CustomDomainCheckOutcome::RecordMissing
                } else {
                    CustomDomainCheckOutcome::RecordMismatch
                }
            }
            Err(DnsError::InvalidName) => CustomDomainCheckOutcome::RecordMissing,
            Err(_) => CustomDomainCheckOutcome::DnsUnavailable,
        };
        match outcome {
            CustomDomainCheckOutcome::Verified => {
                next.state = CustomDomainState::Verified;
                if next.verified_at_unix_seconds.is_none() {
                    next.verified_at_unix_seconds = Some(now_unix_seconds);
                }
                next.last_error = None;
                next.consecutive_misses = 0;
            }
            CustomDomainCheckOutcome::RecordMissing | CustomDomainCheckOutcome::RecordMismatch => {
                next.last_error = Some(outcome.as_str().to_owned());
                if record.state() == CustomDomainState::Verified {
                    next.consecutive_misses = record.consecutive_misses.saturating_add(1);
                    if next.consecutive_misses >= FAILURES_BEFORE_REVOCATION {
                        next.state = CustomDomainState::Failed;
                    }
                }
            }
            // A lookup that could not be answered says nothing about the
            // record: no state change, no strike.
            CustomDomainCheckOutcome::DnsUnavailable => {
                next.last_error = Some(ERROR_DNS_UNAVAILABLE.to_owned());
            }
        }
        (next, outcome)
    }
}

/// Registers, reads, removes, and checks domains with the membership rules
/// project settings use: any member reads, a member who may mutate projects
/// writes.
#[derive(Clone)]
pub struct CustomDomainService {
    store: CustomDomainStore,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
    verifier: CustomDomainVerifier,
    platform_hostname: Option<String>,
}

impl fmt::Debug for CustomDomainService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CustomDomainService")
            .field("platform_hostname", &self.platform_hostname)
            .finish_non_exhaustive()
    }
}

impl CustomDomainService {
    #[must_use]
    pub fn new(
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
        verifier: CustomDomainVerifier,
        platform_hostname: Option<String>,
    ) -> Self {
        Self {
            store: verifier.store().clone(),
            projects,
            organizations,
            audit,
            verifier,
            platform_hostname: platform_hostname.map(|hostname| normalize_hostname(&hostname)),
        }
    }

    #[must_use]
    pub fn store(&self) -> &CustomDomainStore {
        &self.store
    }

    pub async fn list(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        now_unix_seconds: u64,
    ) -> Result<Vec<CustomDomainRecord>, CustomDomainError> {
        self.authorize(
            actor,
            project_id,
            ControlAuditAction::CustomDomainRead,
            false,
            now_unix_seconds,
        )
        .await?;
        self.store.list(project_id).await
    }

    pub async fn get(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        id: &CustomDomainId,
        now_unix_seconds: u64,
    ) -> Result<CustomDomainRecord, CustomDomainError> {
        self.authorize(
            actor,
            project_id,
            ControlAuditAction::CustomDomainRead,
            false,
            now_unix_seconds,
        )
        .await?;
        self.store
            .get(project_id, id)
            .await?
            .ok_or(CustomDomainError::DomainNotFound)
    }

    pub async fn create(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        input: NewCustomDomain,
        now_unix_seconds: u64,
    ) -> Result<CustomDomainRecord, CustomDomainError> {
        let organization = self
            .authorize(
                actor,
                project_id,
                ControlAuditAction::CustomDomainCreate,
                true,
                now_unix_seconds,
            )
            .await?;
        let hostname = validate_hostname(&input.hostname, self.platform_hostname.as_deref())?;
        self.projects
            .get_environment(project_id, &input.environment_id)
            .await?
            .ok_or(CustomDomainError::EnvironmentNotFound)?;
        if self.store.list(project_id).await?.len() >= MAXIMUM_DOMAINS_PER_PROJECT {
            return Err(CustomDomainError::Conflict(
                "project has reached its custom domain limit",
            ));
        }
        let record = CustomDomainRecord::new(
            CustomDomainId::generate(),
            project_id.clone(),
            input.environment_id,
            hostname,
            now_unix_seconds,
        );
        if !self.store.insert(&record).await? {
            return Err(CustomDomainError::Conflict(
                "hostname is already claimed by a project",
            ));
        }
        self.audit(
            actor,
            &organization,
            project_id,
            ControlAuditAction::CustomDomainCreate,
            record.hostname(),
            now_unix_seconds,
        );
        Ok(record)
    }

    /// Removes the domain; the environment's list is republished without
    /// it, so serving stops.
    pub async fn delete(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        id: &CustomDomainId,
        now_unix_seconds: u64,
    ) -> Result<(), CustomDomainError> {
        let organization = self
            .authorize(
                actor,
                project_id,
                ControlAuditAction::CustomDomainDelete,
                true,
                now_unix_seconds,
            )
            .await?;
        let record = self
            .store
            .get(project_id, id)
            .await?
            .ok_or(CustomDomainError::DomainNotFound)?;
        self.store.remove(&record).await?;
        self.audit(
            actor,
            &organization,
            project_id,
            ControlAuditAction::CustomDomainDelete,
            record.hostname(),
            now_unix_seconds,
        );
        // Only a verified domain was being served; a failed publication is
        // retried by the worker.
        if record.state() == CustomDomainState::Verified {
            let _ = self.verifier.publish_tenant(&record.tenant()).await;
        }
        Ok(())
    }

    /// Checks the record now instead of waiting for the worker.
    pub async fn verify(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        id: &CustomDomainId,
        now_unix_seconds: u64,
    ) -> Result<CustomDomainRecord, CustomDomainError> {
        let organization = self
            .authorize(
                actor,
                project_id,
                ControlAuditAction::CustomDomainVerify,
                true,
                now_unix_seconds,
            )
            .await?;
        let record = self
            .store
            .get(project_id, id)
            .await?
            .ok_or(CustomDomainError::DomainNotFound)?;
        let (checked, outcome) = self.verifier.verify_now(&record, now_unix_seconds).await?;
        self.audit(
            actor,
            &organization,
            project_id,
            ControlAuditAction::CustomDomainVerify,
            &format!("{}/{}", record.hostname(), outcome.as_str()),
            now_unix_seconds,
        );
        Ok(checked)
    }

    /// The verified hostnames an environment is served on.
    pub async fn verified_hostnames(
        &self,
        tenant: &TenantScope,
    ) -> Result<Vec<String>, CustomDomainError> {
        self.store.verified_hostnames(tenant).await
    }

    /// The verified domain a hostname resolves to, for the on-demand TLS
    /// `ask` endpoint and the gateway: `None` unless verified.
    pub async fn lookup_verified(
        &self,
        hostname: &str,
    ) -> Result<Option<CustomDomainRecord>, CustomDomainError> {
        let hostname = normalize_hostname(hostname);
        if hostname.is_empty() || hostname.len() > MAXIMUM_HOSTNAME_BYTES {
            return Ok(None);
        }
        Ok(self
            .store
            .by_hostname(&hostname)
            .await?
            .filter(|record| record.state() == CustomDomainState::Verified))
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &ProjectId,
        action: ControlAuditAction,
        write: bool,
        now_unix_seconds: u64,
    ) -> Result<OrganizationId, CustomDomainError> {
        let project = self
            .projects
            .get_project(project_id)
            .await?
            .ok_or(CustomDomainError::ProjectNotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?;
        let allowed =
            membership.is_some_and(|membership| !write || membership.role().can_mutate_projects());
        if !allowed {
            self.audit.record(ControlAuditEvent {
                organization_id: project.organization_id().clone(),
                actor_id: actor.identity_id().clone(),
                action,
                target: audit_target(project_id, "authorization"),
                outcome: ControlAuditOutcome::Denied,
                at_unix_seconds: now_unix_seconds,
            });
            return Err(CustomDomainError::Forbidden);
        }
        Ok(project.organization_id().clone())
    }

    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization: &OrganizationId,
        project_id: &ProjectId,
        action: ControlAuditAction,
        target: &str,
        at_unix_seconds: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: audit_target(project_id, target),
            outcome: ControlAuditOutcome::Allowed,
            at_unix_seconds,
        });
    }
}

fn audit_target(project_id: &ProjectId, target: &str) -> String {
    format!("{}/custom-domains/{target}", project_id.as_str())
}

#[derive(Debug)]
pub enum CustomDomainError {
    UnsupportedDurability,
    ProjectNotFound,
    EnvironmentNotFound,
    DomainNotFound,
    Forbidden,
    Conflict(&'static str),
    InvalidRequest(&'static str),
    InvalidHostname(&'static str),
    CorruptRecord,
    LimitExceeded,
    Poisoned,
    Publish(CustomDomainPublishError),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for CustomDomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDurability => {
                formatter.write_str("custom domain durability is unsupported")
            }
            Self::ProjectNotFound => formatter.write_str("project was not found"),
            Self::EnvironmentNotFound => formatter.write_str("environment was not found"),
            Self::DomainNotFound => formatter.write_str("custom domain was not found"),
            Self::Forbidden => formatter.write_str("custom domain operation is forbidden"),
            Self::Conflict(message)
            | Self::InvalidRequest(message)
            | Self::InvalidHostname(message) => formatter.write_str(message),
            Self::CorruptRecord => formatter.write_str("custom domain record is corrupt"),
            Self::LimitExceeded => formatter.write_str("custom domain scan exceeded its bound"),
            Self::Poisoned => formatter.write_str("custom domain publication state is poisoned"),
            Self::Publish(error) => error.fmt(formatter),
            Self::Project(_) => formatter.write_str("custom domain project lookup failed"),
            Self::Organization(_) => {
                formatter.write_str("custom domain organization lookup failed")
            }
            Self::Keyspace(_) => formatter.write_str("custom domain key is invalid"),
            Self::Storage(_) => formatter.write_str("custom domain storage operation failed"),
            Self::Json(_) => formatter.write_str("custom domain record encoding failed"),
        }
    }
}

impl Error for CustomDomainError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Publish(error) => Some(error),
            Self::Project(error) => Some(error),
            Self::Organization(error) => Some(error),
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ProjectStoreError> for CustomDomainError {
    fn from(value: ProjectStoreError) -> Self {
        Self::Project(value)
    }
}

impl From<OrganizationStoreError> for CustomDomainError {
    fn from(value: OrganizationStoreError) -> Self {
        Self::Organization(value)
    }
}

impl From<ControlKeyspaceError> for CustomDomainError {
    fn from(value: ControlKeyspaceError) -> Self {
        Self::Keyspace(value)
    }
}

impl From<StorageError> for CustomDomainError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<serde_json::Error> for CustomDomainError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::atomic::{AtomicBool, Ordering},
    };

    use futures::executor::block_on;

    use super::*;
    use crate::{
        ProjectRecord,
        email_template::tests::{Fixture, NOW, fixture},
    };

    const MINUTE: u64 = 60;

    /// Answers each name from a table: a value, or unavailability.
    #[derive(Default)]
    struct StubResolver {
        answers: Mutex<BTreeMap<String, Result<Vec<String>, ()>>>,
        lookups: Mutex<Vec<String>>,
    }

    impl StubResolver {
        fn set_txt(&self, name: &str, values: &[&str]) {
            self.answers.lock().expect("lock").insert(
                name.to_owned(),
                Ok(values.iter().map(|value| (*value).to_owned()).collect()),
            );
        }

        fn set_unavailable(&self, name: &str) {
            self.answers
                .lock()
                .expect("lock")
                .insert(name.to_owned(), Err(()));
        }

        fn clear(&self, name: &str) {
            self.answers.lock().expect("lock").remove(name);
        }

        fn lookups(&self) -> Vec<String> {
            self.lookups.lock().expect("lock").clone()
        }
    }

    impl TxtResolver for StubResolver {
        fn lookup_txt(&self, name: &str) -> Result<Vec<String>, DnsError> {
            self.lookups.lock().expect("lock").push(name.to_owned());
            match self.answers.lock().expect("lock").get(name) {
                Some(Ok(values)) => Ok(values.clone()),
                Some(Err(())) => Err(DnsError::Timeout),
                None => Ok(Vec::new()),
            }
        }
    }

    #[derive(Default)]
    struct StubPublisher {
        published: Mutex<Vec<(TenantScope, Vec<String>)>>,
        failing: AtomicBool,
    }

    impl StubPublisher {
        fn published(&self) -> Vec<(TenantScope, Vec<String>)> {
            self.published.lock().expect("lock").clone()
        }

        fn take(&self) -> Vec<(TenantScope, Vec<String>)> {
            std::mem::take(&mut *self.published.lock().expect("lock"))
        }
    }

    impl CustomDomainPublisher for StubPublisher {
        fn publish(
            &self,
            tenant: &TenantScope,
            hostnames: &[String],
        ) -> Result<(), CustomDomainPublishError> {
            if self.failing.load(Ordering::SeqCst) {
                return Err(CustomDomainPublishError::Unavailable(None));
            }
            self.published
                .lock()
                .expect("lock")
                .push((tenant.clone(), hostnames.to_vec()));
            Ok(())
        }
    }

    struct Harness {
        fixture: Fixture,
        resolver: Arc<StubResolver>,
        publisher: Arc<StubPublisher>,
        service: CustomDomainService,
        verifier: CustomDomainVerifier,
    }

    fn harness() -> Harness {
        let fixture = fixture();
        let resolver = Arc::new(StubResolver::default());
        let publisher = Arc::new(StubPublisher::default());
        let kv: Arc<dyn KvAdapter> = fixture.adapter.clone();
        let store = CustomDomainStore::new(kv.clone(), Durability::Memory).expect("store");
        let verifier = CustomDomainVerifier::new(store, resolver.clone(), publisher.clone());
        let service = CustomDomainService::new(
            ProjectStore::new(kv.clone(), Durability::Memory).expect("projects"),
            OrganizationStore::new(kv, Durability::Memory).expect("organizations"),
            fixture.audit.clone(),
            verifier.clone(),
            Some("Cloud.Example.Test".to_owned()),
        );
        Harness {
            fixture,
            resolver,
            publisher,
            service,
            verifier,
        }
    }

    async fn create(harness: &Harness, hostname: &str) -> CustomDomainRecord {
        harness
            .service
            .create(
                &harness.fixture.owner,
                harness.fixture.tenant.project_id(),
                NewCustomDomain {
                    hostname: hostname.to_owned(),
                    environment_id: harness.fixture.tenant.environment_id().clone(),
                },
                NOW,
            )
            .await
            .expect("created")
    }

    #[test]
    fn hostnames_are_normalized_and_the_platform_and_addresses_are_refused() {
        let platform = Some("cloud.example.test");
        assert_eq!(
            validate_hostname(" API.Example.COM. ", platform).expect("valid"),
            "api.example.com"
        );
        assert_eq!(
            validate_hostname("a-1.b-2.example", platform).expect("valid"),
            "a-1.b-2.example"
        );
        for invalid in [
            "",
            ".",
            "example",
            "localhost",
            "api.localhost",
            "127.0.0.1",
            "10.0.0.1",
            "[::1]",
            "1.2.3.4.5",
            "-api.example.com",
            "api-.example.com",
            "api..example.com",
            "api_1.example.com",
            "api.example.com/path",
            "cloud.example.test",
            "api.cloud.example.test",
            "deep.api.cloud.example.test",
            &format!("{}.example.com", "a".repeat(64)),
            &format!("{}.com", "a.".repeat(126)),
        ] {
            assert!(
                matches!(
                    validate_hostname(invalid, platform),
                    Err(CustomDomainError::InvalidHostname(_))
                ),
                "{invalid:?} must be refused"
            );
        }
        assert_eq!(
            validate_hostname("notcloud.example.test", platform).expect("sibling"),
            "notcloud.example.test",
            "a sibling of the platform hostname is not under it"
        );
        assert_eq!(
            validate_hostname("cloud.example.test", None).expect("no platform hostname"),
            "cloud.example.test"
        );
    }

    #[test]
    fn a_new_domain_is_pending_with_a_random_verification_record() {
        let harness = harness();
        let record = block_on(create(&harness, "API.example.com."));
        assert_eq!(record.hostname(), "api.example.com");
        assert_eq!(record.state(), CustomDomainState::Pending);
        assert_eq!(record.record_name(), "_mako-verify.api.example.com");
        let value = record.record_value();
        assert!(value.starts_with(VERIFICATION_VALUE_PREFIX));
        let token = &value[VERIFICATION_VALUE_PREFIX.len()..];
        assert_eq!(token.len(), 32);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        );
        assert!(record.id().as_str().starts_with("dom_"));
        assert_eq!(record.id().as_str().len(), 4 + 16);
        CustomDomainId::parse(record.id().as_str()).expect("public pattern");
        assert!(record.verified_at_unix_seconds().is_none());
        assert!(record.last_checked_at_unix_seconds().is_none());
        assert!(record.last_error().is_none());
        assert_eq!(record.created_at_unix_seconds(), NOW);
        let again = block_on(create(&harness, "other.example.com"));
        assert_ne!(again.record_value(), record.record_value());
        assert_ne!(again.id(), record.id());
        for invalid in ["dom_short", "sch_abcdefghijklmnop", "dom_ABCDEFGHIJKLMNOP"] {
            assert!(CustomDomainId::parse(invalid).is_err());
        }
        // Nothing is served before verification, and nothing was published.
        assert!(harness.publisher.published().is_empty());
        assert!(
            block_on(harness.service.lookup_verified("api.example.com"))
                .expect("lookup")
                .is_none()
        );
    }

    #[test]
    fn a_hostname_belongs_to_at_most_one_project() {
        let harness = harness();
        let first = block_on(create(&harness, "api.example.com"));
        block_on(async {
            // The same project again.
            assert!(matches!(
                harness
                    .service
                    .create(
                        &harness.fixture.owner,
                        harness.fixture.tenant.project_id(),
                        NewCustomDomain {
                            hostname: "API.EXAMPLE.COM".to_owned(),
                            environment_id: harness.fixture.tenant.environment_id().clone(),
                        },
                        NOW,
                    )
                    .await
                    .expect_err("claimed"),
                CustomDomainError::Conflict(_)
            ));
            // Another project of the same team.
            let other_project = ProjectId::parse("prj_otherproject1").expect("project id");
            let project = harness
                .service
                .projects
                .get_project(harness.fixture.tenant.project_id())
                .await
                .expect("read")
                .expect("project");
            harness
                .service
                .projects
                .create_project(
                    &ProjectRecord::new(
                        other_project.clone(),
                        project.organization_id().clone(),
                        "Other",
                        "local",
                        NOW,
                    )
                    .expect("record"),
                )
                .await
                .expect("other project");
            let other_environment = EnvironmentId::parse("env_otherenviron1").expect("env id");
            harness
                .service
                .projects
                .create_environment(
                    &crate::EnvironmentRecord::new(
                        other_environment.clone(),
                        other_project.clone(),
                        "Production",
                        NOW,
                    )
                    .expect("environment"),
                )
                .await
                .expect("other environment");
            assert!(matches!(
                harness
                    .service
                    .create(
                        &harness.fixture.owner,
                        &other_project,
                        NewCustomDomain {
                            hostname: "api.example.com".to_owned(),
                            environment_id: other_environment.clone(),
                        },
                        NOW,
                    )
                    .await
                    .expect_err("claimed elsewhere"),
                CustomDomainError::Conflict(_)
            ));
            // The environment must belong to the project named in the path.
            assert!(matches!(
                harness
                    .service
                    .create(
                        &harness.fixture.owner,
                        harness.fixture.tenant.project_id(),
                        NewCustomDomain {
                            hostname: "second.example.com".to_owned(),
                            environment_id: other_environment,
                        },
                        NOW,
                    )
                    .await
                    .expect_err("foreign environment"),
                CustomDomainError::EnvironmentNotFound
            ));
            // Once deleted, the hostname is free again.
            harness
                .service
                .delete(
                    &harness.fixture.owner,
                    harness.fixture.tenant.project_id(),
                    first.id(),
                    NOW,
                )
                .await
                .expect("deleted");
            assert!(
                harness
                    .service
                    .store()
                    .by_hostname("api.example.com")
                    .await
                    .expect("index")
                    .is_none()
            );
            let reclaimed = harness
                .service
                .create(
                    &harness.fixture.owner,
                    &other_project,
                    NewCustomDomain {
                        hostname: "api.example.com".to_owned(),
                        environment_id: EnvironmentId::parse("env_otherenviron1").expect("id"),
                    },
                    NOW,
                )
                .await
                .expect("reclaimed");
            assert_eq!(reclaimed.project_id(), &other_project);
        });
    }

    #[test]
    fn the_verifier_moves_a_domain_through_its_states_and_publishes_each_change() {
        let harness = harness();
        let tenant = harness.fixture.tenant.clone();
        let record = block_on(create(&harness, "api.example.com"));
        let name = record.record_name();
        block_on(async {
            // First pass, record absent: pending with a reason, and the
            // startup sync publishes the (empty) list once.
            let report = harness.verifier.run_once(NOW + MINUTE).await.expect("pass");
            assert_eq!(report.checked, 1);
            assert_eq!(report.record_missing, 1);
            assert_eq!(report.published, 1);
            assert_eq!(harness.publisher.take(), vec![(tenant.clone(), Vec::new())]);
            let current = harness
                .service
                .get(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW,
                )
                .await
                .expect("read");
            assert_eq!(current.state(), CustomDomainState::Pending);
            assert_eq!(current.last_error(), Some(ERROR_RECORD_MISSING));
            assert_eq!(current.last_checked_at_unix_seconds(), Some(NOW + MINUTE));
            assert_eq!(harness.resolver.lookups(), vec![name.clone()]);

            // A different value at the name is a mismatch, still pending.
            harness
                .resolver
                .set_txt(&name, &["mako-domain-verify=somethingelse", "v=spf1"]);
            harness
                .verifier
                .run_once(NOW + 2 * MINUTE)
                .await
                .expect("pass");
            let current = harness
                .service
                .get(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW,
                )
                .await
                .expect("read");
            assert_eq!(current.state(), CustomDomainState::Pending);
            assert_eq!(current.last_error(), Some(ERROR_RECORD_MISMATCH));
            assert!(
                harness.publisher.take().is_empty(),
                "no state change, no publication"
            );

            // The value appears (alongside another record, with whitespace).
            harness
                .resolver
                .set_txt(&name, &["v=spf1", &format!(" {} ", record.record_value())]);
            let report = harness
                .verifier
                .run_once(NOW + 3 * MINUTE)
                .await
                .expect("pass");
            assert_eq!(report.verified, 1);
            assert_eq!(report.published, 1);
            assert_eq!(
                harness.publisher.take(),
                vec![(tenant.clone(), vec!["api.example.com".to_owned()])]
            );
            let verified = harness
                .service
                .get(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW,
                )
                .await
                .expect("read");
            assert_eq!(verified.state(), CustomDomainState::Verified);
            assert_eq!(verified.verified_at_unix_seconds(), Some(NOW + 3 * MINUTE));
            assert!(verified.last_error().is_none());
            assert_eq!(
                harness
                    .service
                    .lookup_verified("API.example.com.")
                    .await
                    .expect("lookup")
                    .map(|record| record.id().clone()),
                Some(record.id().clone())
            );
            assert_eq!(
                harness
                    .service
                    .verified_hostnames(&tenant)
                    .await
                    .expect("hostnames"),
                vec!["api.example.com".to_owned()]
            );

            // A resolver outage changes nothing but the reason.
            harness.resolver.set_unavailable(&name);
            let report = harness
                .verifier
                .run_once(NOW + 4 * MINUTE)
                .await
                .expect("pass");
            assert_eq!(report.dns_unavailable, 1);
            let current = harness
                .service
                .get(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW,
                )
                .await
                .expect("read");
            assert_eq!(current.state(), CustomDomainState::Verified);
            assert_eq!(current.last_error(), Some(ERROR_DNS_UNAVAILABLE));
            assert_eq!(current.consecutive_misses(), 0);
            assert!(harness.publisher.take().is_empty());

            // The record is withdrawn: one miss is a strike, not a revocation...
            harness.resolver.clear(&name);
            harness
                .verifier
                .run_once(NOW + 5 * MINUTE)
                .await
                .expect("pass");
            let current = harness
                .service
                .get(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW,
                )
                .await
                .expect("read");
            assert_eq!(current.state(), CustomDomainState::Verified);
            assert_eq!(current.consecutive_misses(), 1);
            assert_eq!(current.last_error(), Some(ERROR_RECORD_MISSING));
            assert!(
                harness.publisher.take().is_empty(),
                "still served after one miss"
            );
            // ...an outage in between does not count either way...
            harness.resolver.set_unavailable(&name);
            harness
                .verifier
                .run_once(NOW + 6 * MINUTE)
                .await
                .expect("pass");
            let current = harness
                .service
                .get(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW,
                )
                .await
                .expect("read");
            assert_eq!(current.state(), CustomDomainState::Verified);
            assert_eq!(current.consecutive_misses(), 1);
            // ...and the second miss revokes serving.
            harness.resolver.clear(&name);
            let report = harness
                .verifier
                .run_once(NOW + 7 * MINUTE)
                .await
                .expect("pass");
            assert_eq!(report.revoked, 1);
            assert_eq!(harness.publisher.take(), vec![(tenant.clone(), Vec::new())]);
            let failed = harness
                .service
                .get(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW,
                )
                .await
                .expect("read");
            assert_eq!(failed.state(), CustomDomainState::Failed);
            assert_eq!(failed.last_error(), Some(ERROR_RECORD_MISSING));
            assert_eq!(
                failed.verified_at_unix_seconds(),
                Some(NOW + 3 * MINUTE),
                "the record keeps when it was first proven"
            );
            assert!(
                harness
                    .service
                    .lookup_verified("api.example.com")
                    .await
                    .expect("lookup")
                    .is_none(),
                "a failed domain is not served"
            );

            // The record comes back: verified again, served again.
            harness.resolver.set_txt(&name, &[record.record_value()]);
            harness
                .verifier
                .run_once(NOW + 8 * MINUTE)
                .await
                .expect("pass");
            assert_eq!(
                harness.publisher.take(),
                vec![(tenant.clone(), vec!["api.example.com".to_owned()])]
            );
            let again = harness
                .service
                .get(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW,
                )
                .await
                .expect("read");
            assert_eq!(again.state(), CustomDomainState::Verified);
            assert_eq!(again.consecutive_misses(), 0);
            assert!(again.last_error().is_none());
        });
    }

    #[test]
    fn verify_now_runs_one_check_synchronously_and_delete_withdraws_serving() {
        let harness = harness();
        let tenant = harness.fixture.tenant.clone();
        let record = block_on(create(&harness, "api.example.com"));
        block_on(async {
            let checked = harness
                .service
                .verify(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW + 1,
                )
                .await
                .expect("checked");
            assert_eq!(checked.state(), CustomDomainState::Pending);
            assert_eq!(checked.last_error(), Some(ERROR_RECORD_MISSING));
            assert_eq!(checked.last_checked_at_unix_seconds(), Some(NOW + 1));
            harness
                .resolver
                .set_txt(&record.record_name(), &[record.record_value()]);
            let checked = harness
                .service
                .verify(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW + 2,
                )
                .await
                .expect("checked");
            assert_eq!(checked.state(), CustomDomainState::Verified);
            assert_eq!(checked.verified_at_unix_seconds(), Some(NOW + 2));
            assert_eq!(
                harness.publisher.take(),
                vec![(tenant.clone(), vec!["api.example.com".to_owned()])],
                "the on-request check publishes like the worker"
            );
            harness
                .service
                .delete(
                    &harness.fixture.owner,
                    tenant.project_id(),
                    record.id(),
                    NOW + 3,
                )
                .await
                .expect("deleted");
            assert_eq!(
                harness.publisher.take(),
                vec![(tenant.clone(), Vec::new())],
                "removal republishes the environment without the domain"
            );
            assert!(matches!(
                harness
                    .service
                    .get(
                        &harness.fixture.owner,
                        tenant.project_id(),
                        record.id(),
                        NOW
                    )
                    .await
                    .expect_err("gone"),
                CustomDomainError::DomainNotFound
            ));
            assert!(matches!(
                harness
                    .service
                    .delete(
                        &harness.fixture.owner,
                        tenant.project_id(),
                        record.id(),
                        NOW
                    )
                    .await
                    .expect_err("gone"),
                CustomDomainError::DomainNotFound
            ));
            assert!(
                harness
                    .service
                    .list(&harness.fixture.owner, tenant.project_id(), NOW)
                    .await
                    .expect("list")
                    .is_empty()
            );
            let actions = harness
                .fixture
                .audit
                .0
                .lock()
                .expect("audit")
                .iter()
                .map(|event| event.action.as_str())
                .collect::<Vec<_>>();
            for expected in [
                "custom_domain_create",
                "custom_domain_verify",
                "custom_domain_delete",
            ] {
                assert!(actions.contains(&expected), "{expected} is audited");
            }
        });
    }

    #[test]
    fn a_failed_publication_is_retried_on_the_next_pass() {
        let harness = harness();
        let tenant = harness.fixture.tenant.clone();
        let record = block_on(create(&harness, "api.example.com"));
        block_on(async {
            harness
                .resolver
                .set_txt(&record.record_name(), &[record.record_value()]);
            harness.publisher.failing.store(true, Ordering::SeqCst);
            let report = harness.verifier.run_once(NOW + MINUTE).await.expect("pass");
            assert_eq!(report.verified, 1);
            assert_eq!(report.publish_failures, 1);
            assert_eq!(report.published, 0);
            // The state changed regardless; the developer sees `verified`.
            assert_eq!(
                harness
                    .service
                    .get(
                        &harness.fixture.owner,
                        tenant.project_id(),
                        record.id(),
                        NOW
                    )
                    .await
                    .expect("read")
                    .state(),
                CustomDomainState::Verified
            );
            harness.publisher.failing.store(false, Ordering::SeqCst);
            let report = harness
                .verifier
                .run_once(NOW + 2 * MINUTE)
                .await
                .expect("pass");
            assert_eq!(report.published, 1, "retried without a state change");
            assert_eq!(
                harness.publisher.take(),
                vec![(tenant.clone(), vec!["api.example.com".to_owned()])]
            );
            let report = harness
                .verifier
                .run_once(NOW + 3 * MINUTE)
                .await
                .expect("pass");
            assert_eq!(report.published, 0, "nothing pending, nothing changed");
        });
    }

    #[test]
    fn membership_decides_who_reads_and_who_writes() {
        let harness = harness();
        let project_id = harness.fixture.tenant.project_id().clone();
        let record = block_on(create(&harness, "api.example.com"));
        block_on(async {
            assert_eq!(
                harness
                    .service
                    .list(&harness.fixture.viewer, &project_id, NOW)
                    .await
                    .expect("viewer reads")
                    .len(),
                1
            );
            harness
                .service
                .get(&harness.fixture.viewer, &project_id, record.id(), NOW)
                .await
                .expect("viewer reads one");
            for (label, result) in [
                (
                    "create",
                    harness
                        .service
                        .create(
                            &harness.fixture.viewer,
                            &project_id,
                            NewCustomDomain {
                                hostname: "viewer.example.com".to_owned(),
                                environment_id: harness.fixture.tenant.environment_id().clone(),
                            },
                            NOW,
                        )
                        .await
                        .map(|_| ()),
                ),
                (
                    "verify",
                    harness
                        .service
                        .verify(&harness.fixture.viewer, &project_id, record.id(), NOW)
                        .await
                        .map(|_| ()),
                ),
                (
                    "delete",
                    harness
                        .service
                        .delete(&harness.fixture.viewer, &project_id, record.id(), NOW)
                        .await,
                ),
            ] {
                assert!(
                    matches!(result, Err(CustomDomainError::Forbidden)),
                    "a viewer may not {label}"
                );
            }
            assert!(matches!(
                harness
                    .service
                    .list(&harness.fixture.stranger, &project_id, NOW)
                    .await
                    .expect_err("stranger"),
                CustomDomainError::Forbidden
            ));
            assert!(matches!(
                harness
                    .service
                    .list(
                        &harness.fixture.owner,
                        &ProjectId::parse("prj_doesnotexist1").expect("id"),
                        NOW
                    )
                    .await
                    .expect_err("unknown project"),
                CustomDomainError::ProjectNotFound
            ));
            let denied = harness
                .fixture
                .audit
                .0
                .lock()
                .expect("audit")
                .iter()
                .filter(|event| event.outcome == ControlAuditOutcome::Denied)
                .count();
            assert_eq!(denied, 4, "each refusal is audited");
        });
    }
}
