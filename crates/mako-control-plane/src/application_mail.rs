//! Mail to application users: verification, recovery, invitation, and magic
//! link messages the data plane asks for.
//!
//! The data plane writes a *mail intent* (kind, recipient, variables) in the
//! environment's keyspace and hands it out on lease over an internal route.
//! The control plane's mail worker drains those intents, renders each with the
//! environment's template or the built-in default, seals the envelope into
//! its own outbox under the developer-mail key, and only then acknowledges
//! the intent so the data plane forgets it. A lease that expires unacknowledged
//! is drained again, so intake is at-least-once from the data plane and made
//! exactly-once here by keying the outbox record on the intent id.
//!
//! Delivery reuses the developer outbox's state machine -- lease, deliver,
//! retry with exponential backoff, dead-letter -- and the same transport, so
//! application mail has one durability story and one relay with developer
//! mail. The developer outbox itself is untouched: these records live in
//! their own key domain with their own associated data.

use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use mako_api::{EnvironmentId, ProjectId, TenantScope};
use mako_identity::NormalizedEmail;
use mako_internal_rpc::{
    ApplicationMailAcknowledgeRequest, ApplicationMailDrainRequest, ApplicationMailIntent,
    ControlToDataClient, InternalClientError,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{
    ControlKeyspace, ControlKeyspaceError, DeveloperMailCipher, DeveloperMailEnvelope,
    DeveloperMailFailureKind, DeveloperMailOutboxState, DeveloperMailTransport,
    DeveloperMailTransportError, DeveloperRegistrationConfig, DeveloperWorkflowError,
    EmailTemplateError, EmailTemplateKind, EmailTemplateService, EncryptedDeveloperMail,
    developer_workflow::outbox_backoff, render_template,
};

/// Intent ids are `aml_` followed by 32 lowercase hex digits.
const INTENT_ID_PREFIX: &str = "aml_";
const INTENT_ID_HEX_LENGTH: usize = 32;
/// Scans are bounded like the developer outbox; a saturated scan fails closed.
const MAXIMUM_OUTBOX_SCAN: usize = 10_000;
const MAXIMUM_BATCH: usize = 100;
const MAXIMUM_ERROR_CODE_BYTES: usize = 64;

/// The outbox record id: the data plane's intent id, verbatim, so the same
/// intent can never produce two records.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ApplicationMailOutboxId(String);

impl ApplicationMailOutboxId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ApplicationMailError> {
        let value = value.into();
        let Some(suffix) = value.strip_prefix(INTENT_ID_PREFIX) else {
            return Err(ApplicationMailError::InvalidId);
        };
        if suffix.len() != INTENT_ID_HEX_LENGTH
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ApplicationMailError::InvalidId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One application mail in the control plane's outbox.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApplicationMailOutboxRecord {
    id: ApplicationMailOutboxId,
    tenant: TenantScope,
    kind: EmailTemplateKind,
    /// Absent only on a record dead-lettered at intake, before an envelope
    /// could be built -- the record then exists to keep the intent's id from
    /// being taken again.
    encrypted_mail: Option<EncryptedDeveloperMail>,
    state: DeveloperMailOutboxState,
    attempts: u32,
    created_at_unix_seconds: u64,
    next_attempt_at_unix_seconds: u64,
    lease_expires_at_unix_seconds: Option<u64>,
    delivered_at_unix_seconds: Option<u64>,
    last_error_code: Option<String>,
}

impl ApplicationMailOutboxRecord {
    #[must_use]
    pub fn new(
        id: ApplicationMailOutboxId,
        tenant: TenantScope,
        kind: EmailTemplateKind,
        encrypted_mail: EncryptedDeveloperMail,
        now_unix_seconds: u64,
    ) -> Self {
        Self {
            id,
            tenant,
            kind,
            encrypted_mail: Some(encrypted_mail),
            state: DeveloperMailOutboxState::Pending,
            attempts: 0,
            created_at_unix_seconds: now_unix_seconds,
            next_attempt_at_unix_seconds: now_unix_seconds,
            lease_expires_at_unix_seconds: None,
            delivered_at_unix_seconds: None,
            last_error_code: None,
        }
    }

    /// A record for an intent that can never become mail: the environment
    /// is gone, the recipient is malformed, or the template will not render.
    pub fn dead_letter_at_intake(
        id: ApplicationMailOutboxId,
        tenant: TenantScope,
        kind: EmailTemplateKind,
        error_code: &str,
        now_unix_seconds: u64,
    ) -> Result<Self, ApplicationMailError> {
        validate_error_code(error_code)?;
        Ok(Self {
            id,
            tenant,
            kind,
            encrypted_mail: None,
            state: DeveloperMailOutboxState::DeadLetter,
            attempts: 0,
            created_at_unix_seconds: now_unix_seconds,
            next_attempt_at_unix_seconds: now_unix_seconds,
            lease_expires_at_unix_seconds: None,
            delivered_at_unix_seconds: None,
            last_error_code: Some(error_code.to_owned()),
        })
    }

    #[must_use]
    pub fn id(&self) -> &ApplicationMailOutboxId {
        &self.id
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub const fn kind(&self) -> EmailTemplateKind {
        self.kind
    }

    #[must_use]
    pub const fn encrypted_mail(&self) -> Option<&EncryptedDeveloperMail> {
        self.encrypted_mail.as_ref()
    }

    #[must_use]
    pub const fn state(&self) -> DeveloperMailOutboxState {
        self.state
    }

    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn next_attempt_at_unix_seconds(&self) -> u64 {
        self.next_attempt_at_unix_seconds
    }

    #[must_use]
    pub fn last_error_code(&self) -> Option<&str> {
        self.last_error_code.as_deref()
    }

    fn is_due(&self, now_unix_seconds: u64) -> bool {
        matches!(
            self.state,
            DeveloperMailOutboxState::Pending | DeveloperMailOutboxState::Leased
        ) && self.next_attempt_at_unix_seconds <= now_unix_seconds
            && self
                .lease_expires_at_unix_seconds
                .is_none_or(|expiry| expiry <= now_unix_seconds)
    }

    pub fn lease(
        &mut self,
        now_unix_seconds: u64,
        lease_seconds: u64,
    ) -> Result<(), ApplicationMailError> {
        if !self.is_due(now_unix_seconds) || lease_seconds == 0 || self.encrypted_mail.is_none() {
            return Err(ApplicationMailError::Conflict);
        }
        self.state = DeveloperMailOutboxState::Leased;
        self.attempts = self
            .attempts
            .checked_add(1)
            .ok_or(ApplicationMailError::CorruptRecord)?;
        self.lease_expires_at_unix_seconds = Some(
            now_unix_seconds
                .checked_add(lease_seconds)
                .ok_or(ApplicationMailError::CorruptRecord)?,
        );
        Ok(())
    }

    pub fn delivered(&mut self, now_unix_seconds: u64) -> Result<(), ApplicationMailError> {
        if self.state != DeveloperMailOutboxState::Leased {
            return Err(ApplicationMailError::Conflict);
        }
        self.state = DeveloperMailOutboxState::Delivered;
        self.delivered_at_unix_seconds = Some(now_unix_seconds);
        self.lease_expires_at_unix_seconds = None;
        self.last_error_code = None;
        Ok(())
    }

    pub fn retry(
        &mut self,
        now_unix_seconds: u64,
        retry_at_unix_seconds: u64,
        error_code: &str,
        maximum_attempts: u32,
    ) -> Result<(), ApplicationMailError> {
        validate_error_code(error_code)?;
        if self.state != DeveloperMailOutboxState::Leased
            || retry_at_unix_seconds <= now_unix_seconds
            || maximum_attempts == 0
        {
            return Err(ApplicationMailError::Conflict);
        }
        self.state = if self.attempts >= maximum_attempts {
            DeveloperMailOutboxState::DeadLetter
        } else {
            DeveloperMailOutboxState::Pending
        };
        self.next_attempt_at_unix_seconds = retry_at_unix_seconds;
        self.lease_expires_at_unix_seconds = None;
        self.last_error_code = Some(error_code.to_owned());
        Ok(())
    }
}

fn validate_error_code(code: &str) -> Result<(), ApplicationMailError> {
    if code.is_empty()
        || code.len() > MAXIMUM_ERROR_CODE_BYTES
        || code.chars().any(char::is_control)
    {
        return Err(ApplicationMailError::Conflict);
    }
    Ok(())
}

/// Associated data binding a sealed envelope to its record, tenant, and kind.
fn mail_aad(
    record_id: &ApplicationMailOutboxId,
    tenant: &TenantScope,
    kind: EmailTemplateKind,
) -> Vec<u8> {
    format!(
        "mako-application-mail-v1\0{}\0{}\0{}\0{}",
        record_id.as_str(),
        tenant.project_id().as_str(),
        tenant.environment_id().as_str(),
        kind.as_str()
    )
    .into_bytes()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnqueueOutcome {
    Stored,
    /// The intent was drained before and its record already exists.
    AlreadyStored,
}

/// Pending and dead-lettered counts for health reporting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApplicationMailDepth {
    pub pending: usize,
    pub dead_letter: usize,
}

/// The application mail outbox in the control store.
#[derive(Clone)]
pub struct ApplicationMailStore {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for ApplicationMailStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplicationMailStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl ApplicationMailStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
    ) -> Result<Self, ApplicationMailError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(ApplicationMailError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
        })
    }

    /// Stores a record unless one with the same id exists. Idempotent by
    /// construction: a re-drained intent conflicts here and is reported as
    /// already stored, which is still a reason to acknowledge it.
    pub async fn enqueue(
        &self,
        record: &ApplicationMailOutboxRecord,
    ) -> Result<EnqueueOutcome, ApplicationMailError> {
        let key = ControlKeyspace::application_mail_outbox_key(record.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(1);
        batch.put(&key, serde_json::to_vec(record)?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::Missing { key }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(EnqueueOutcome::Stored),
            CompareAndWriteResult::Conflict { .. } => Ok(EnqueueOutcome::AlreadyStored),
        }
    }

    pub async fn get(
        &self,
        id: &ApplicationMailOutboxId,
    ) -> Result<Option<ApplicationMailOutboxRecord>, ApplicationMailError> {
        self.adapter
            .get(&ControlKeyspace::application_mail_outbox_key(id.as_str())?)
            .await?
            .map(|value| serde_json::from_slice(&value).map_err(ApplicationMailError::from))
            .transpose()
    }

    pub async fn replace(
        &self,
        previous: &ApplicationMailOutboxRecord,
        next: &ApplicationMailOutboxRecord,
    ) -> Result<(), ApplicationMailError> {
        if previous.id != next.id || previous.tenant != next.tenant {
            return Err(ApplicationMailError::Conflict);
        }
        let key = ControlKeyspace::application_mail_outbox_key(previous.id.as_str())?;
        let mut batch = WriteBatch::with_capacity(1);
        batch.put(&key, serde_json::to_vec(next)?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals {
                    key,
                    value: serde_json::to_vec(previous)?,
                }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(ApplicationMailError::Conflict),
        }
    }

    /// Records due for a delivery attempt, oldest key first.
    pub async fn pending(
        &self,
        now_unix_seconds: u64,
        limit: NonZeroUsize,
    ) -> Result<Vec<ApplicationMailOutboxRecord>, ApplicationMailError> {
        if limit.get() > MAXIMUM_BATCH {
            return Err(ApplicationMailError::LimitExceeded);
        }
        Ok(self
            .scan_all()
            .await?
            .into_iter()
            .filter(|record| record.is_due(now_unix_seconds))
            .take(limit.get())
            .collect())
    }

    /// Removes delivered records older than the retention window and
    /// dead letters older than four times it, so the outbox stays bounded.
    pub async fn cleanup(
        &self,
        now_unix_seconds: u64,
        delivered_retention_seconds: u64,
        limit: NonZeroUsize,
    ) -> Result<usize, ApplicationMailError> {
        if delivered_retention_seconds == 0 || limit.get() > MAXIMUM_BATCH {
            return Err(ApplicationMailError::LimitExceeded);
        }
        let dead_letter_retention = delivered_retention_seconds.saturating_mul(4);
        let mut batch = WriteBatch::new();
        let mut removed = 0;
        for record in self.scan_all().await? {
            if removed >= limit.get() {
                break;
            }
            let expired = match record.state {
                DeveloperMailOutboxState::Delivered => {
                    record.delivered_at_unix_seconds.is_some_and(|at| {
                        at.saturating_add(delivered_retention_seconds) <= now_unix_seconds
                    })
                }
                DeveloperMailOutboxState::DeadLetter => {
                    record
                        .next_attempt_at_unix_seconds
                        .saturating_add(dead_letter_retention)
                        <= now_unix_seconds
                }
                DeveloperMailOutboxState::Pending | DeveloperMailOutboxState::Leased => false,
            };
            if expired {
                batch.delete(ControlKeyspace::application_mail_outbox_key(
                    record.id.as_str(),
                )?);
                removed += 1;
            }
        }
        if removed > 0 {
            self.adapter
                .compare_and_write(AtomicWrite {
                    conditions: Vec::new(),
                    batch,
                    durability: self.durability,
                })
                .await?;
        }
        Ok(removed)
    }

    pub async fn depth(&self) -> Result<ApplicationMailDepth, ApplicationMailError> {
        let mut depth = ApplicationMailDepth::default();
        for record in self.scan_all().await? {
            match record.state {
                DeveloperMailOutboxState::Pending | DeveloperMailOutboxState::Leased => {
                    depth.pending += 1;
                }
                DeveloperMailOutboxState::DeadLetter => depth.dead_letter += 1,
                DeveloperMailOutboxState::Delivered => {}
            }
        }
        Ok(depth)
    }

    async fn scan_all(&self) -> Result<Vec<ApplicationMailOutboxRecord>, ApplicationMailError> {
        let values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::application_mail_outbox_range()?,
                ScanDirection::Forward,
                NonZeroUsize::new(MAXIMUM_OUTBOX_SCAN).expect("constant is positive"),
            ))
            .await?;
        // Fail closed rather than act on a partial view of the outbox.
        if values.len() >= MAXIMUM_OUTBOX_SCAN {
            return Err(ApplicationMailError::LimitExceeded);
        }
        values
            .into_iter()
            .map(|value| serde_json::from_slice(&value.value).map_err(ApplicationMailError::from))
            .collect()
    }
}

/// Where intents come from: the data plane in production, a queue in tests.
/// Both calls are synchronous loopback requests.
pub trait ApplicationMailSource: Send + Sync {
    fn drain(
        &self,
        request_id: &str,
        lease_seconds: u64,
        limit: u64,
    ) -> Result<Vec<ApplicationMailIntent>, ApplicationMailSourceError>;

    fn acknowledge(
        &self,
        request_id: &str,
        ids: &[String],
    ) -> Result<u64, ApplicationMailSourceError>;
}

impl ApplicationMailSource for ControlToDataClient {
    fn drain(
        &self,
        request_id: &str,
        lease_seconds: u64,
        limit: u64,
    ) -> Result<Vec<ApplicationMailIntent>, ApplicationMailSourceError> {
        self.drain_application_mail(
            request_id,
            &ApplicationMailDrainRequest {
                lease_seconds,
                limit,
            },
        )
        .map(|response| response.intents)
        .map_err(ApplicationMailSourceError::Client)
    }

    fn acknowledge(
        &self,
        request_id: &str,
        ids: &[String],
    ) -> Result<u64, ApplicationMailSourceError> {
        self.acknowledge_application_mail(
            request_id,
            &ApplicationMailAcknowledgeRequest { ids: ids.to_vec() },
        )
        .map(|response| response.acknowledged)
        .map_err(ApplicationMailSourceError::Client)
    }
}

#[derive(Debug)]
pub enum ApplicationMailSourceError {
    Client(InternalClientError),
    Unavailable,
}

impl fmt::Display for ApplicationMailSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("application mail source is unreachable")
    }
}

impl Error for ApplicationMailSourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Unavailable => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationMailConfig {
    /// How long a drained intent stays invisible to the next drain.
    pub drain_lease_seconds: u64,
    pub drain_limit: u64,
    pub maximum_outbox_batch: usize,
    pub outbox_lease_seconds: u64,
    pub outbox_maximum_attempts: u32,
    pub outbox_maximum_backoff_seconds: u64,
    pub delivered_mail_retention_seconds: u64,
}

impl ApplicationMailConfig {
    /// The outbox limits developer mail already runs with, plus the fixed
    /// drain lease and batch the data-plane contract expects.
    #[must_use]
    pub fn from_registration(registration: &DeveloperRegistrationConfig) -> Self {
        Self {
            maximum_outbox_batch: registration.maximum_outbox_batch,
            outbox_lease_seconds: registration.outbox_lease_seconds,
            outbox_maximum_attempts: registration.outbox_maximum_attempts,
            outbox_maximum_backoff_seconds: registration.outbox_maximum_backoff_seconds,
            delivered_mail_retention_seconds: registration.delivered_mail_retention_seconds,
            ..Self::default()
        }
    }

    fn validate(&self) -> Result<(), ApplicationMailError> {
        if self.drain_lease_seconds == 0
            || self.drain_limit == 0
            || self.drain_limit > MAXIMUM_BATCH as u64
            || self.maximum_outbox_batch == 0
            || self.maximum_outbox_batch > MAXIMUM_BATCH
            || self.outbox_lease_seconds == 0
            || self.outbox_maximum_attempts == 0
            || self.outbox_maximum_backoff_seconds == 0
            || self.delivered_mail_retention_seconds == 0
        {
            return Err(ApplicationMailError::InvalidConfiguration);
        }
        Ok(())
    }
}

impl Default for ApplicationMailConfig {
    fn default() -> Self {
        let registration = DeveloperRegistrationConfig::default();
        Self {
            drain_lease_seconds: 60,
            drain_limit: 32,
            maximum_outbox_batch: registration.maximum_outbox_batch,
            outbox_lease_seconds: registration.outbox_lease_seconds,
            outbox_maximum_attempts: registration.outbox_maximum_attempts,
            outbox_maximum_backoff_seconds: registration.outbox_maximum_backoff_seconds,
            delivered_mail_retention_seconds: registration.delivered_mail_retention_seconds,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApplicationMailWorkerReport {
    /// Intents the data plane handed out this pass.
    pub drained: usize,
    /// Intents that became new outbox records.
    pub stored: usize,
    /// Intents whose record already existed (a re-drain after a lost acknowledgement).
    pub duplicates: usize,
    /// Intents recorded as dead letters at intake: no environment, bad recipient, or a template that would not render.
    pub refused_at_intake: usize,
    /// Intents the control plane could not even record; they are left for the next drain.
    pub unusable: usize,
    pub acknowledged: usize,
    pub leased: usize,
    pub delivered: usize,
    pub retried: usize,
    pub dead_lettered: usize,
    pub conflicts: usize,
    pub cleaned: usize,
}

/// Drains intents into the outbox, then delivers what is due.
#[derive(Clone)]
pub struct ApplicationMailWorker {
    store: ApplicationMailStore,
    cipher: DeveloperMailCipher,
    transport: Arc<dyn DeveloperMailTransport>,
    source: Arc<dyn ApplicationMailSource>,
    templates: EmailTemplateService,
    config: ApplicationMailConfig,
}

impl fmt::Debug for ApplicationMailWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplicationMailWorker")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

enum Intake {
    Stored(ApplicationMailOutboxId),
    AlreadyStored(ApplicationMailOutboxId),
    Refused(ApplicationMailOutboxId),
    Unusable,
}

impl ApplicationMailWorker {
    pub fn new(
        store: ApplicationMailStore,
        cipher: DeveloperMailCipher,
        transport: Arc<dyn DeveloperMailTransport>,
        source: Arc<dyn ApplicationMailSource>,
        templates: EmailTemplateService,
        config: ApplicationMailConfig,
    ) -> Result<Self, ApplicationMailError> {
        config.validate()?;
        Ok(Self {
            store,
            cipher,
            transport,
            source,
            templates,
            config,
        })
    }

    #[must_use]
    pub fn store(&self) -> &ApplicationMailStore {
        &self.store
    }

    /// One pass: intake, then delivery, then cleanup. Intake failing to reach
    /// the data plane is reported as an error after whatever was stored has
    /// been acknowledged; delivery of what is already in the outbox does not
    /// depend on the data plane at all.
    pub async fn run_once(
        &self,
        now_unix_seconds: u64,
    ) -> Result<ApplicationMailWorkerReport, ApplicationMailError> {
        let mut report = ApplicationMailWorkerReport::default();
        let intake = self.intake(now_unix_seconds, &mut report).await;
        self.deliver(now_unix_seconds, &mut report).await?;
        report.cleaned = self
            .store
            .cleanup(
                now_unix_seconds,
                self.config.delivered_mail_retention_seconds,
                NonZeroUsize::new(self.config.maximum_outbox_batch)
                    .ok_or(ApplicationMailError::InvalidConfiguration)?,
            )
            .await?;
        intake.map(|()| report)
    }

    async fn intake(
        &self,
        now_unix_seconds: u64,
        report: &mut ApplicationMailWorkerReport,
    ) -> Result<(), ApplicationMailError> {
        let request_id = random_request_id();
        let intents = self
            .source
            .drain(
                &request_id,
                self.config.drain_lease_seconds,
                self.config.drain_limit,
            )
            .map_err(ApplicationMailError::Source)?;
        report.drained = intents.len();
        let mut taken = Vec::with_capacity(intents.len());
        let mut failure = None;
        for intent in intents {
            match self.take(intent, now_unix_seconds).await {
                Ok(Intake::Stored(id)) => {
                    report.stored += 1;
                    taken.push(id.0);
                }
                Ok(Intake::AlreadyStored(id)) => {
                    report.duplicates += 1;
                    taken.push(id.0);
                }
                Ok(Intake::Refused(id)) => {
                    report.refused_at_intake += 1;
                    taken.push(id.0);
                }
                Ok(Intake::Unusable) => report.unusable += 1,
                Err(error) => {
                    // A storage failure mid-batch: acknowledge what is durable
                    // and leave the rest for the next drain.
                    failure = Some(error);
                    break;
                }
            }
        }
        if !taken.is_empty() {
            match self.source.acknowledge(&random_request_id(), &taken) {
                Ok(acknowledged) => {
                    report.acknowledged = usize::try_from(acknowledged).unwrap_or(usize::MAX);
                }
                Err(error) => {
                    failure.get_or_insert(ApplicationMailError::Source(error));
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Turns one intent into an outbox record. Only a storage or template
    /// lookup failure is an error; every other problem with the intent is
    /// recorded on the intent itself so it is acknowledged and never retried.
    async fn take(
        &self,
        intent: ApplicationMailIntent,
        now_unix_seconds: u64,
    ) -> Result<Intake, ApplicationMailError> {
        let Ok(id) = ApplicationMailOutboxId::parse(intent.id.as_str()) else {
            return Ok(Intake::Unusable);
        };
        let (Ok(project_id), Ok(environment_id)) = (
            ProjectId::parse(intent.project_id.as_str()),
            EnvironmentId::parse(intent.environment_id.as_str()),
        ) else {
            return Ok(Intake::Unusable);
        };
        let Some(kind) = EmailTemplateKind::parse(&intent.kind) else {
            return Ok(Intake::Unusable);
        };
        let tenant = TenantScope::new(project_id, environment_id);
        let record = match self.render(&id, &tenant, kind, &intent).await {
            Ok(encrypted) => ApplicationMailOutboxRecord::new(
                id.clone(),
                tenant,
                kind,
                encrypted,
                now_unix_seconds,
            ),
            Err(RenderFailure::Refused(code)) => {
                ApplicationMailOutboxRecord::dead_letter_at_intake(
                    id.clone(),
                    tenant,
                    kind,
                    code,
                    now_unix_seconds,
                )?
            }
            Err(RenderFailure::Error(error)) => return Err(error),
        };
        let refused = record.state == DeveloperMailOutboxState::DeadLetter;
        Ok(match self.store.enqueue(&record).await? {
            EnqueueOutcome::Stored if refused => Intake::Refused(id),
            EnqueueOutcome::Stored => Intake::Stored(id),
            EnqueueOutcome::AlreadyStored => Intake::AlreadyStored(id),
        })
    }

    async fn render(
        &self,
        id: &ApplicationMailOutboxId,
        tenant: &TenantScope,
        kind: EmailTemplateKind,
        intent: &ApplicationMailIntent,
    ) -> Result<EncryptedDeveloperMail, RenderFailure> {
        let Ok(recipient) = NormalizedEmail::parse(&intent.recipient) else {
            return Err(RenderFailure::Refused("invalid_recipient"));
        };
        let (project_name, environment_name) = match self.templates.names(tenant).await {
            Ok(names) => names,
            Err(EmailTemplateError::NotFound) => {
                return Err(RenderFailure::Refused("environment_not_found"));
            }
            Err(error) => return Err(RenderFailure::Error(error.into())),
        };
        let template = self
            .templates
            .resolve(tenant, kind)
            .await
            .map_err(|error| RenderFailure::Error(error.into()))?;
        let mut variables: BTreeMap<String, String> = intent
            .variables
            .iter()
            .filter(|(name, _)| kind.variables().contains(&name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        variables.insert("project_name".to_owned(), project_name);
        variables.insert("environment_name".to_owned(), environment_name);
        // A customized template saved before a rule it now breaks -- a body
        // without {{link}}, say -- is sent as the built-in default instead:
        // refused, the mail would never go, and the reader would still have
        // nothing to act on.
        let rendered = match template.render(&variables) {
            Ok(rendered) => rendered,
            Err(EmailTemplateError::InvalidTemplate(_)) if !template.is_default => {
                match render_template(
                    kind,
                    kind.default_subject(),
                    kind.default_text_body(),
                    &variables,
                ) {
                    Ok(rendered) => rendered,
                    Err(_) => return Err(RenderFailure::Refused("invalid_template")),
                }
            }
            Err(EmailTemplateError::InvalidTemplate(_)) => {
                return Err(RenderFailure::Refused("invalid_template"));
            }
            Err(error) => return Err(RenderFailure::Error(error.into())),
        };
        let Ok(envelope) =
            DeveloperMailEnvelope::new(recipient, rendered.subject, rendered.text_body)
        else {
            return Err(RenderFailure::Refused("invalid_envelope"));
        };
        self.cipher
            .seal(&mail_aad(id, tenant, kind), &envelope)
            .map_err(|error| RenderFailure::Error(ApplicationMailError::Workflow(error)))
    }

    async fn deliver(
        &self,
        now_unix_seconds: u64,
        report: &mut ApplicationMailWorkerReport,
    ) -> Result<(), ApplicationMailError> {
        let records = self
            .store
            .pending(
                now_unix_seconds,
                NonZeroUsize::new(self.config.maximum_outbox_batch)
                    .ok_or(ApplicationMailError::InvalidConfiguration)?,
            )
            .await?;
        for previous in records {
            let mut leased = previous.clone();
            leased.lease(now_unix_seconds, self.config.outbox_lease_seconds)?;
            match self.store.replace(&previous, &leased).await {
                Ok(()) => report.leased += 1,
                Err(ApplicationMailError::Conflict) => {
                    report.conflicts += 1;
                    continue;
                }
                Err(error) => return Err(error),
            }
            let delivery = match leased
                .encrypted_mail()
                .ok_or(DeveloperWorkflowError::InvalidMail)
                .and_then(|encrypted| {
                    self.cipher.open(
                        &mail_aad(&leased.id, &leased.tenant, leased.kind),
                        encrypted,
                    )
                }) {
                Ok(envelope) => self.transport.deliver(leased.id.as_str(), &envelope).await,
                Err(_) => Err(DeveloperMailTransportError {
                    kind: DeveloperMailFailureKind::Permanent,
                    stable_code: "invalid_envelope",
                }),
            };
            let mut completed = leased.clone();
            match delivery {
                Ok(()) => {
                    completed.delivered(now_unix_seconds)?;
                    report.delivered += 1;
                }
                Err(error) => {
                    let maximum_attempts = if error.kind == DeveloperMailFailureKind::Permanent {
                        completed.attempts()
                    } else {
                        self.config.outbox_maximum_attempts
                    };
                    let backoff = outbox_backoff(
                        completed.attempts(),
                        self.config.outbox_maximum_backoff_seconds,
                    );
                    completed.retry(
                        now_unix_seconds,
                        now_unix_seconds
                            .checked_add(backoff)
                            .ok_or(ApplicationMailError::CorruptRecord)?,
                        error.stable_code,
                        maximum_attempts,
                    )?;
                    if completed.state() == DeveloperMailOutboxState::DeadLetter {
                        report.dead_lettered += 1;
                    } else {
                        report.retried += 1;
                    }
                }
            }
            match self.store.replace(&leased, &completed).await {
                Ok(()) => {}
                Err(ApplicationMailError::Conflict) => report.conflicts += 1,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

enum RenderFailure {
    /// The intent can never become mail; recorded as a dead letter.
    Refused(&'static str),
    /// A lookup failed for a reason that may clear; the intent stays leased.
    Error(ApplicationMailError),
}

fn random_request_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut hex = String::with_capacity(36);
    hex.push_str("req_");
    for byte in bytes {
        use fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[derive(Debug)]
pub enum ApplicationMailError {
    InvalidConfiguration,
    InvalidId,
    Conflict,
    CorruptRecord,
    LimitExceeded,
    UnsupportedDurability,
    Source(ApplicationMailSourceError),
    Template(EmailTemplateError),
    Workflow(DeveloperWorkflowError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for ApplicationMailError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "application mail configuration is invalid",
            Self::InvalidId => "application mail intent id is invalid",
            Self::Conflict => "application mail record changed concurrently",
            Self::CorruptRecord => "application mail record is corrupt",
            Self::LimitExceeded => "application mail outbox scan exceeded its bound",
            Self::UnsupportedDurability => "application mail durability is unsupported",
            Self::Source(_) => "application mail source is unreachable",
            Self::Template(_) => "application mail template lookup failed",
            Self::Workflow(_) => "application mail encryption failed",
            Self::Keyspace(_) => "application mail key is invalid",
            Self::Storage(_) => "application mail storage operation failed",
            Self::Json(_) => "application mail record encoding failed",
        })
    }
}

impl Error for ApplicationMailError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Template(error) => Some(error),
            Self::Workflow(error) => Some(error),
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<EmailTemplateError> for ApplicationMailError {
    fn from(value: EmailTemplateError) -> Self {
        Self::Template(value)
    }
}

impl From<ControlKeyspaceError> for ApplicationMailError {
    fn from(value: ControlKeyspaceError) -> Self {
        Self::Keyspace(value)
    }
}

impl From<StorageError> for ApplicationMailError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<serde_json::Error> for ApplicationMailError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures::executor::block_on;

    use super::*;
    use crate::{
        DeveloperMailEncryptionKey, EmailTemplateText,
        email_template::tests::{Fixture, NOW, fixture},
    };

    const INTENT_ID: &str = "aml_0123456789abcdef0123456789abcdef";

    /// Behaves like the data plane's intent store: hands out what it holds,
    /// forgets what is acknowledged, and counts both.
    #[derive(Default)]
    struct QueueSource {
        intents: Mutex<Vec<ApplicationMailIntent>>,
        drains: Mutex<usize>,
        acknowledged: Mutex<Vec<String>>,
        unreachable: Mutex<bool>,
    }

    impl ApplicationMailSource for QueueSource {
        fn drain(
            &self,
            request_id: &str,
            lease_seconds: u64,
            limit: u64,
        ) -> Result<Vec<ApplicationMailIntent>, ApplicationMailSourceError> {
            assert!(request_id.starts_with("req_") && request_id.len() == 36);
            assert_eq!(lease_seconds, 60);
            assert_eq!(limit, 32);
            if *self.unreachable.lock().expect("lock") {
                return Err(ApplicationMailSourceError::Unavailable);
            }
            *self.drains.lock().expect("lock") += 1;
            Ok(self.intents.lock().expect("lock").clone())
        }

        fn acknowledge(
            &self,
            _request_id: &str,
            ids: &[String],
        ) -> Result<u64, ApplicationMailSourceError> {
            if *self.unreachable.lock().expect("lock") {
                return Err(ApplicationMailSourceError::Unavailable);
            }
            let mut intents = self.intents.lock().expect("lock");
            let before = intents.len();
            intents.retain(|intent| !ids.contains(&intent.id));
            self.acknowledged
                .lock()
                .expect("lock")
                .extend(ids.iter().cloned());
            Ok((before - intents.len()) as u64)
        }
    }

    #[derive(Default)]
    struct CapturingTransport {
        delivered: Mutex<Vec<(String, String, String, String)>>,
        fail_with: Mutex<Option<DeveloperMailTransportError>>,
    }

    #[async_trait]
    impl DeveloperMailTransport for CapturingTransport {
        async fn readiness(&self) -> Result<(), DeveloperMailTransportError> {
            Ok(())
        }

        async fn deliver(
            &self,
            delivery_id: &str,
            envelope: &DeveloperMailEnvelope,
        ) -> Result<(), DeveloperMailTransportError> {
            if let Some(error) = self.fail_with.lock().expect("lock").clone() {
                return Err(error);
            }
            self.delivered.lock().expect("lock").push((
                delivery_id.to_owned(),
                envelope.recipient().as_str().to_owned(),
                envelope.subject().to_owned(),
                envelope.text_body().to_owned(),
            ));
            Ok(())
        }
    }

    struct Harness {
        fixture: Fixture,
        source: Arc<QueueSource>,
        transport: Arc<CapturingTransport>,
        worker: ApplicationMailWorker,
    }

    fn harness() -> Harness {
        let fixture = fixture();
        let source = Arc::new(QueueSource::default());
        let transport = Arc::new(CapturingTransport::default());
        let kv: Arc<dyn KvAdapter> = fixture.adapter.clone();
        let worker = ApplicationMailWorker::new(
            ApplicationMailStore::new(kv, Durability::Memory).expect("store"),
            DeveloperMailCipher::new(DeveloperMailEncryptionKey::derive(b"application mail key")),
            transport.clone(),
            source.clone(),
            fixture.service.clone(),
            ApplicationMailConfig::default(),
        )
        .expect("worker");
        Harness {
            fixture,
            source,
            transport,
            worker,
        }
    }

    fn intent(harness: &Harness, id: &str, kind: &str) -> ApplicationMailIntent {
        let mut variables = BTreeMap::new();
        variables.insert(
            "link".to_owned(),
            "https://notes.example.com/auth/magic?token=t0k3n".to_owned(),
        );
        variables.insert("expires_at".to_owned(), "2026-08-29T10:15:00Z".to_owned());
        variables.insert("email".to_owned(), "reader@example.test".to_owned());
        // A variable outside the allowlist never reaches a template.
        variables.insert("password".to_owned(), "hunter2".to_owned());
        ApplicationMailIntent {
            id: id.to_owned(),
            project_id: harness.fixture.tenant.project_id().as_str().to_owned(),
            environment_id: harness.fixture.tenant.environment_id().as_str().to_owned(),
            kind: kind.to_owned(),
            recipient: "reader@example.test".to_owned(),
            variables,
            created_at_unix_seconds: NOW,
        }
    }

    #[test]
    fn a_drained_magic_link_intent_becomes_one_delivered_mail_with_the_custom_subject() {
        let harness = harness();
        block_on(async {
            harness
                .fixture
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    EmailTemplateKind::MagicLink,
                    EmailTemplateText {
                        subject: "Sign in to {{project_name}} / {{environment_name}}".to_owned(),
                        text_body: "Open {{link}} before {{expires_at}}.\n".to_owned(),
                    },
                    NOW,
                )
                .await
                .expect("custom template");
            harness.source.intents.lock().expect("lock").push(intent(
                &harness,
                INTENT_ID,
                "magic_link",
            ));

            let first = harness.worker.run_once(NOW).await.expect("first pass");
            assert_eq!(first.drained, 1);
            assert_eq!(first.stored, 1);
            assert_eq!(first.acknowledged, 1);
            assert_eq!(first.delivered, 1);
            assert_eq!(first.duplicates, 0);
            let delivered = harness.transport.delivered.lock().expect("lock").clone();
            assert_eq!(delivered.len(), 1);
            let (delivery_id, recipient, subject, body) = &delivered[0];
            assert_eq!(delivery_id, INTENT_ID);
            assert_eq!(recipient, "reader@example.test");
            assert_eq!(subject, "Sign in to Field Notes / Staging");
            assert_eq!(
                body,
                "Open https://notes.example.com/auth/magic?token=t0k3n before 2026-08-29T10:15:00Z.\n"
            );
            assert!(!body.contains("hunter2"));
            assert!(harness.source.intents.lock().expect("lock").is_empty());

            // The record is delivered and stays that way; a second pass finds nothing to do.
            let record = harness
                .worker
                .store()
                .get(&ApplicationMailOutboxId::parse(INTENT_ID).expect("id"))
                .await
                .expect("read")
                .expect("record");
            assert_eq!(record.state(), DeveloperMailOutboxState::Delivered);
            assert_eq!(record.attempts(), 1);
            let second = harness.worker.run_once(NOW + 1).await.expect("second pass");
            assert_eq!(second.drained, 0);
            assert_eq!(second.delivered, 0);
            assert_eq!(harness.transport.delivered.lock().expect("lock").len(), 1);
        });
    }

    /// A template customized before bodies had to carry {{link}} is sent as
    /// the default: its reader still gets the link the mail exists for.
    #[test]
    fn a_stored_template_without_its_link_is_sent_as_the_default() {
        let harness = harness();
        block_on(async {
            harness
                .fixture
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    EmailTemplateKind::Verification,
                    EmailTemplateText {
                        subject: "Welcome".to_owned(),
                        text_body: "Open {{link}}.".to_owned(),
                    },
                    NOW,
                )
                .await
                .expect("custom template");
            // Rewrite the stored body the way the old rules let it be saved.
            let key = ControlKeyspace::email_template_key(
                harness.fixture.tenant.project_id(),
                harness.fixture.tenant.environment_id(),
                EmailTemplateKind::Verification,
            )
            .expect("key");
            let stored = harness
                .fixture
                .adapter
                .get(&key)
                .await
                .expect("read")
                .expect("stored");
            let mut record: serde_json::Value = serde_json::from_slice(&stored).expect("json");
            record["textBody"] = serde_json::Value::from("Welcome aboard. See you soon.");
            let mut batch = mako_storage::WriteBatch::new();
            batch.put(&key, serde_json::to_vec(&record).expect("json"));
            harness
                .fixture
                .adapter
                .write(batch, mako_storage::Durability::Memory)
                .await
                .expect("rewrite");
            harness.source.intents.lock().expect("lock").push(intent(
                &harness,
                INTENT_ID,
                "verification",
            ));
            let report = harness.worker.run_once(NOW).await.expect("pass");
            assert_eq!(report.delivered, 1);
            let (_, _, subject, body) = &harness.transport.delivered.lock().expect("lock")[0];
            assert_eq!(subject, "Verify your email for Field Notes");
            assert!(body.contains("https://notes.example.com/auth/magic?token=t0k3n"));
        });
    }

    #[test]
    fn a_redrained_intent_is_stored_once_and_acknowledged_again() {
        let harness = harness();
        block_on(async {
            // The default template applies; nothing was customized.
            harness.source.intents.lock().expect("lock").push(intent(
                &harness,
                INTENT_ID,
                "verification",
            ));
            // First drain stores the record, but the acknowledgement is lost.
            let first = harness.worker.run_once(NOW).await.expect("first pass");
            assert_eq!(first.stored, 1);
            assert_eq!(first.delivered, 1);
            // The data plane hands the same intent out again after its lease.
            harness.source.intents.lock().expect("lock").push(intent(
                &harness,
                INTENT_ID,
                "verification",
            ));
            let second = harness
                .worker
                .run_once(NOW + 61)
                .await
                .expect("second pass");
            assert_eq!(second.drained, 1);
            assert_eq!(second.stored, 0);
            assert_eq!(second.duplicates, 1);
            assert_eq!(second.acknowledged, 1, "a duplicate is still acknowledged");
            assert_eq!(second.delivered, 0, "the mail is not sent twice");
            assert_eq!(harness.transport.delivered.lock().expect("lock").len(), 1);
            let (_, _, subject, body) = &harness.transport.delivered.lock().expect("lock")[0];
            assert_eq!(subject, "Verify your email for Field Notes");
            assert!(body.contains("https://notes.example.com/auth/magic?token=t0k3n"));
            assert!(body.contains("Field Notes (Staging)"));
            assert!(harness.source.intents.lock().expect("lock").is_empty());
        });
    }

    #[test]
    fn intents_that_can_never_become_mail_are_dead_lettered_and_acknowledged() {
        let harness = harness();
        block_on(async {
            let mut missing_environment =
                intent(&harness, "aml_ffffffffffffffffffffffffffffffff", "recovery");
            missing_environment.environment_id = "env_doesnotexist1".to_owned();
            let mut bad_recipient = intent(
                &harness,
                "aml_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                "invitation",
            );
            bad_recipient.recipient = "not an address".to_owned();
            let unknown_kind = intent(
                &harness,
                "aml_dddddddddddddddddddddddddddddddd",
                "newsletter",
            );
            let bad_id = intent(&harness, "dmo_notanapplicationmailid", "recovery");
            harness.source.intents.lock().expect("lock").extend([
                missing_environment,
                bad_recipient,
                unknown_kind,
                bad_id,
            ]);
            let report = harness.worker.run_once(NOW).await.expect("pass");
            assert_eq!(report.drained, 4);
            assert_eq!(report.refused_at_intake, 2);
            assert_eq!(report.unusable, 2);
            assert_eq!(report.acknowledged, 2);
            assert_eq!(report.delivered, 0);
            assert!(harness.transport.delivered.lock().expect("lock").is_empty());
            let store = harness.worker.store();
            let environment = store
                .get(
                    &ApplicationMailOutboxId::parse("aml_ffffffffffffffffffffffffffffffff")
                        .expect("id"),
                )
                .await
                .expect("read")
                .expect("dead letter");
            assert_eq!(environment.state(), DeveloperMailOutboxState::DeadLetter);
            assert_eq!(environment.last_error_code(), Some("environment_not_found"));
            assert!(environment.encrypted_mail().is_none());
            let recipient = store
                .get(
                    &ApplicationMailOutboxId::parse("aml_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
                        .expect("id"),
                )
                .await
                .expect("read")
                .expect("dead letter");
            assert_eq!(recipient.last_error_code(), Some("invalid_recipient"));
            // The unusable intents were neither stored nor acknowledged.
            assert_eq!(harness.source.intents.lock().expect("lock").len(), 2);
            assert_eq!(
                store.depth().await.expect("depth"),
                ApplicationMailDepth {
                    pending: 0,
                    dead_letter: 2
                }
            );
        });
    }

    #[test]
    fn delivery_failures_back_off_and_an_unreachable_data_plane_is_an_error_not_a_loss() {
        let harness = harness();
        block_on(async {
            harness
                .source
                .intents
                .lock()
                .expect("lock")
                .push(intent(&harness, INTENT_ID, "recovery"));
            *harness.transport.fail_with.lock().expect("lock") =
                Some(DeveloperMailTransportError {
                    kind: DeveloperMailFailureKind::Transient,
                    stable_code: "smtp_transient",
                });
            let first = harness.worker.run_once(NOW).await.expect("first pass");
            assert_eq!(first.stored, 1);
            assert_eq!(first.retried, 1);
            assert_eq!(first.delivered, 0);
            let id = ApplicationMailOutboxId::parse(INTENT_ID).expect("id");
            let record = harness
                .worker
                .store()
                .get(&id)
                .await
                .expect("read")
                .expect("record");
            assert_eq!(record.state(), DeveloperMailOutboxState::Pending);
            assert_eq!(record.last_error_code(), Some("smtp_transient"));
            assert_eq!(record.next_attempt_at_unix_seconds(), NOW + 30);

            // Not due yet: nothing is attempted.
            let early = harness.worker.run_once(NOW + 10).await.expect("early pass");
            assert_eq!(early.leased, 0);

            // The data plane is down: delivery of what is already stored still proceeds.
            *harness.source.unreachable.lock().expect("lock") = true;
            *harness.transport.fail_with.lock().expect("lock") = None;
            let error = harness
                .worker
                .run_once(NOW + 30)
                .await
                .expect_err("drain failure is reported");
            assert!(matches!(error, ApplicationMailError::Source(_)));
            assert_eq!(harness.transport.delivered.lock().expect("lock").len(), 1);
            let record = harness
                .worker
                .store()
                .get(&id)
                .await
                .expect("read")
                .expect("record");
            assert_eq!(record.state(), DeveloperMailOutboxState::Delivered);
            assert_eq!(record.attempts(), 2);

            // A permanent failure dead-letters on the first attempt.
            *harness.source.unreachable.lock().expect("lock") = false;
            harness.source.intents.lock().expect("lock").push(intent(
                &harness,
                "aml_cccccccccccccccccccccccccccccccc",
                "invitation",
            ));
            *harness.transport.fail_with.lock().expect("lock") =
                Some(DeveloperMailTransportError {
                    kind: DeveloperMailFailureKind::Permanent,
                    stable_code: "smtp_permanent",
                });
            let report = harness.worker.run_once(NOW + 100).await.expect("pass");
            assert_eq!(report.dead_lettered, 1);
            assert_eq!(
                harness.worker.store().depth().await.expect("depth"),
                ApplicationMailDepth {
                    pending: 0,
                    dead_letter: 1
                }
            );

            // Delivered records age out after the retention window.
            let retention = ApplicationMailConfig::default().delivered_mail_retention_seconds;
            let cleaned = harness
                .worker
                .run_once(NOW + 30 + retention)
                .await
                .expect("cleanup pass");
            assert_eq!(cleaned.cleaned, 1);
            assert!(
                harness
                    .worker
                    .store()
                    .get(&id)
                    .await
                    .expect("read")
                    .is_none()
            );
        });
    }

    #[test]
    fn intent_ids_are_validated_and_records_are_sealed_to_their_tenant() {
        assert!(ApplicationMailOutboxId::parse(INTENT_ID).is_ok());
        for invalid in [
            "aml_0123456789ABCDEF0123456789ABCDEF",
            "aml_0123456789abcdef",
            "dmo_0123456789abcdef0123456789abcdef",
            "",
        ] {
            assert!(matches!(
                ApplicationMailOutboxId::parse(invalid),
                Err(ApplicationMailError::InvalidId)
            ));
        }
        let harness = harness();
        let cipher =
            DeveloperMailCipher::new(DeveloperMailEncryptionKey::derive(b"application mail key"));
        let id = ApplicationMailOutboxId::parse(INTENT_ID).expect("id");
        let envelope = DeveloperMailEnvelope::new(
            NormalizedEmail::parse("reader@example.test").expect("email"),
            "Subject",
            "Body",
        )
        .expect("envelope");
        let sealed = cipher
            .seal(
                &mail_aad(&id, &harness.fixture.tenant, EmailTemplateKind::Recovery),
                &envelope,
            )
            .expect("sealed");
        assert!(
            cipher
                .open(
                    &mail_aad(&id, &harness.fixture.tenant, EmailTemplateKind::Recovery),
                    &sealed
                )
                .is_ok()
        );
        assert!(
            cipher
                .open(
                    &mail_aad(&id, &harness.fixture.tenant, EmailTemplateKind::MagicLink),
                    &sealed
                )
                .is_err()
        );
        assert!(!format!("{sealed:?}").contains("Body"));
    }
}
