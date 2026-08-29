//! Database webhooks: signed HTTP deliveries on collection changes.
//!
//! A developer registers an endpoint for an environment, subscribed to the
//! insert, update, and delete events of chosen collections. The control
//! plane's webhook worker consumes each environment's committed change log
//! over the internal change feed -- positions, revisions, and event kinds,
//! never document fields -- and writes one *delivery* per subscribed change
//! into a durable outbox in the control store. A delivery loop then posts
//! each delivery with an HMAC-SHA256 signature, retries with exponential
//! backoff for a bounded window, keeps one document's deliveries in order,
//! and pauses an endpoint that keeps failing rather than dropping anything.
//!
//! The outbox is the at-least-once guarantee across restarts: a delivery is
//! durable before the cursor that produced it advances (both land in one
//! atomic write), and a delivery's state only moves forward after the
//! endpoint answered. Nothing here touches the data plane's write path; a
//! slow endpoint slows its own deliveries and nothing else.
//!
//! Signing secrets are generated here, shown once, and rest sealed under the
//! developer-mail key with associated data naming the endpoint and the
//! secret version. They are never listed, never logged, and never in `Debug`.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    net::IpAddr,
    num::NonZeroUsize,
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use mako_api::{CollectionId, TenantScope};
use mako_http_client::{HttpClient, HttpClientConfig, HttpClientError};
use mako_internal_rpc::{
    ChangeFeedEvent, ControlToDataClient, InternalClientError, ReadChangeFeedInput,
    ReadChangeFeedOutput,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KeyValue, KvAdapter,
    ScanDirection, ScanRequest, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, DeveloperMailCipher, DeveloperPrincipal, DeveloperWorkflowError,
    EncryptedDeveloperMail, OrganizationId, OrganizationStore, OrganizationStoreError,
    ProjectStore, ProjectStoreError,
};

/// An endpoint subscribes to at most this many collections.
pub const MAXIMUM_SUBSCRIPTIONS: usize = 32;
pub const MAXIMUM_URL_BYTES: usize = 2_048;
pub const MAXIMUM_DESCRIPTION_BYTES: usize = 256;
/// Changes read from the feed per page during intake.
pub const INTAKE_PAGE: u32 = 100;
/// Pages one endpoint-subscription may consume in one pass, so a burst on one
/// collection cannot starve the other endpoints of a pass.
const INTAKE_PAGES_PER_PASS: usize = 10;
/// Deliveries attempted per endpoint per pass.
pub const DELIVERIES_PER_ENDPOINT_PER_PASS: usize = 50;
/// Backoff after `n` failed attempts is `min(5 s * 2^n, 15 min)`, plus jitter.
pub const BASE_BACKOFF_SECONDS: u64 = 5;
pub const MAXIMUM_BACKOFF_SECONDS: u64 = 15 * 60;
/// A delivery is retried for this long after its first attempt, then failed.
pub const RETRY_WINDOW_SECONDS: u64 = 24 * 60 * 60;
/// Consecutive failed attempts across an endpoint's deliveries that pause it.
pub const PAUSE_THRESHOLD: u64 = 20;
/// Delivered and failed deliveries are kept this long.
pub const DELIVERY_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;
/// And never more than this many per endpoint, oldest terminal ones first.
pub const MAXIMUM_DELIVERIES_PER_ENDPOINT: usize = 2_000;
/// The endpoint has this long to answer a delivery.
pub const DELIVERY_TIMEOUT_SECONDS: u64 = 10;
pub const PAUSED_REASON_SUSTAINED_FAILURE: &str = "sustained_failure";
/// The `lastError` a pending delivery takes when its endpoint is disabled and
/// re-enabled: it was abandoned, not attempted.
pub const LAST_ERROR_DISABLED: &str = "disabled";
pub const USER_AGENT: &str = "mako-cloud-webhooks/1";
pub const SIGNATURE_HEADER: &str = "x-mako-signature";

const ID_SUFFIX_LENGTH: usize = 16;
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const SECRET_PREFIX: &str = "whsec_";
const SECRET_BYTES: usize = 32;
const WORKER_ACTOR_ID: &str = "system/webhook-worker";
/// Every scan is bounded; a saturated scan fails closed like the mail outbox.
const MAXIMUM_REGISTRY_SCAN: usize = 10_000;
const MAXIMUM_ENDPOINTS_PER_TENANT: usize = 1_000;
const MAXIMUM_PENDING_SCAN: usize = 5_000;
const MAXIMUM_DUE_SCAN: usize = 5_000;
/// The log scan behind a listing: the retention ceiling plus what one pass
/// may add before retention runs again.
const MAXIMUM_LOG_SCAN: usize = MAXIMUM_DELIVERIES_PER_ENDPOINT + 500;
const RETENTION_BATCH: usize = 200;
const REMOVAL_BATCH: usize = 500;
const MAXIMUM_REMOVAL_ROUNDS: usize = 40;
const MAXIMUM_DOCUMENT_ID_BYTES: usize = 512;
const MAXIMUM_REVISION_BYTES: usize = 128;
const HTTP_CONNECT_TIMEOUT_SECONDS: u64 = 5;
const HTTP_MAXIMUM_RESPONSE_BYTES: usize = 64 * 1024;

macro_rules! webhook_id {
    ($name:ident, $prefix:literal, $label:literal) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Accepts the public pattern `^` $prefix `[a-z0-9]{12,32}$`.
            pub fn parse(value: impl Into<String>) -> Result<Self, WebhookError> {
                let value = value.into();
                let Some(suffix) = value.strip_prefix($prefix) else {
                    return Err(WebhookError::InvalidRequest(concat!(
                        $label,
                        " id is invalid"
                    )));
                };
                if !(12..=32).contains(&suffix.len())
                    || !suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                {
                    return Err(WebhookError::InvalidRequest(concat!(
                        $label,
                        " id is invalid"
                    )));
                }
                Ok(Self(value))
            }

            /// A fresh id: the prefix and sixteen random lowercase alphanumerics.
            #[must_use]
            pub fn generate() -> Self {
                Self(format!("{}{}", $prefix, random_suffix()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

webhook_id!(WebhookEndpointId, "whk_", "webhook endpoint");
webhook_id!(WebhookDeliveryId, "whd_", "webhook delivery");

/// Sixteen unbiased characters from the lowercase alphanumeric alphabet.
fn random_suffix() -> String {
    let mut suffix = String::with_capacity(ID_SUFFIX_LENGTH);
    let mut buffer = [0_u8; 32];
    while suffix.len() < ID_SUFFIX_LENGTH {
        OsRng.fill_bytes(&mut buffer);
        for byte in buffer {
            // 252 is the largest multiple of 36 below 256; rejecting the
            // rest keeps every character equally likely.
            if byte < 252 && suffix.len() < ID_SUFFIX_LENGTH {
                suffix.push(char::from(ID_ALPHABET[usize::from(byte % 36)]));
            }
        }
    }
    suffix
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookEvent {
    Insert,
    Update,
    Delete,
}

impl WebhookEvent {
    pub const ALL: [Self; 3] = [Self::Insert, Self::Update, Self::Delete];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.as_str() == value)
    }
}

impl From<ChangeFeedEvent> for WebhookEvent {
    fn from(value: ChangeFeedEvent) -> Self {
        match value {
            ChangeFeedEvent::Insert => Self::Insert,
            ChangeFeedEvent::Update => Self::Update,
            ChangeFeedEvent::Delete => Self::Delete,
        }
    }
}

/// One collection and the events an endpoint wants from it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebhookSubscription {
    pub collection_id: CollectionId,
    pub events: Vec<WebhookEvent>,
}

impl WebhookSubscription {
    fn wants(&self, event: WebhookEvent) -> bool {
        self.events.contains(&event)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookEndpointState {
    Active,
    Paused,
    Disabled,
}

impl WebhookEndpointState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Disabled => "disabled",
        }
    }
}

/// A signing secret as it is shown once. Its `Debug` output is redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct WebhookSigningSecret(String);

impl WebhookSigningSecret {
    fn generate() -> Self {
        let mut bytes = [0_u8; SECRET_BYTES];
        OsRng.fill_bytes(&mut bytes);
        let mut secret = String::with_capacity(SECRET_PREFIX.len() + SECRET_BYTES * 2);
        secret.push_str(SECRET_PREFIX);
        for byte in bytes {
            use fmt::Write as _;
            let _ = write!(secret, "{byte:02x}");
        }
        Self(secret)
    }

    /// The secret text: the HMAC key is exactly these bytes.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for WebhookSigningSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebhookSigningSecret([REDACTED])")
    }
}

/// Associated data binding a sealed secret to its endpoint and version.
fn secret_aad(endpoint_id: &WebhookEndpointId, secret_version: u64) -> Vec<u8> {
    format!(
        "mako-webhook-secret-v1\0{}\0{secret_version}",
        endpoint_id.as_str()
    )
    .into_bytes()
}

/// A registered endpoint. Mirrors the public `WebhookEndpoint` plus the
/// sealed signing secret, which never leaves this record in the clear.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebhookEndpointRecord {
    id: WebhookEndpointId,
    tenant: TenantScope,
    url: String,
    description: String,
    subscriptions: Vec<WebhookSubscription>,
    enabled: bool,
    paused_reason: Option<String>,
    paused_at_unix_seconds: Option<u64>,
    consecutive_failures: u64,
    secret_version: u64,
    sealed_secret: EncryptedDeveloperMail,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

impl fmt::Debug for WebhookEndpointRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookEndpointRecord")
            .field("id", &self.id)
            .field("tenant", &self.tenant)
            .field("url", &self.url)
            .field("subscriptions", &self.subscriptions)
            .field("state", &self.state())
            .field("consecutive_failures", &self.consecutive_failures)
            .field("secret_version", &self.secret_version)
            .field("sealed_secret", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl WebhookEndpointRecord {
    #[must_use]
    pub fn id(&self) -> &WebhookEndpointId {
        &self.id
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub fn subscriptions(&self) -> &[WebhookSubscription] {
        &self.subscriptions
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn paused_reason(&self) -> Option<&str> {
        self.paused_reason.as_deref()
    }

    #[must_use]
    pub const fn paused_at_unix_seconds(&self) -> Option<u64> {
        self.paused_at_unix_seconds
    }

    #[must_use]
    pub const fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures
    }

    #[must_use]
    pub const fn secret_version(&self) -> u64 {
        self.secret_version
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    /// `disabled` was chosen by the developer; `paused` was applied by the
    /// platform after sustained failure; otherwise the endpoint delivers.
    #[must_use]
    pub const fn state(&self) -> WebhookEndpointState {
        if !self.enabled {
            WebhookEndpointState::Disabled
        } else if self.paused_at_unix_seconds.is_some() {
            WebhookEndpointState::Paused
        } else {
            WebhookEndpointState::Active
        }
    }

    /// Enabled and not paused: intake and delivery run.
    #[must_use]
    pub const fn delivers(&self) -> bool {
        matches!(self.state(), WebhookEndpointState::Active)
    }

    fn note_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Counts one failed attempt; pauses the endpoint at the threshold and
    /// says whether this call did.
    fn note_failure(&mut self, threshold: u64, now_unix_seconds: u64) -> bool {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= threshold && self.paused_at_unix_seconds.is_none() {
            self.paused_reason = Some(PAUSED_REASON_SUSTAINED_FAILURE.to_owned());
            self.paused_at_unix_seconds = Some(now_unix_seconds);
            self.updated_at_unix_seconds = now_unix_seconds;
            return true;
        }
        false
    }

    fn clear_pause(&mut self, now_unix_seconds: u64) {
        self.paused_reason = None;
        self.paused_at_unix_seconds = None;
        self.consecutive_failures = 0;
        self.updated_at_unix_seconds = now_unix_seconds;
    }
}

/// What a registration supplies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewWebhookEndpoint {
    pub url: String,
    pub description: Option<String>,
    pub subscriptions: Vec<WebhookSubscription>,
    pub enabled: bool,
}

/// Fields omitted keep their values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WebhookEndpointUpdate {
    pub url: Option<String>,
    pub description: Option<String>,
    pub subscriptions: Option<Vec<WebhookSubscription>>,
    pub enabled: Option<bool>,
}

impl WebhookEndpointUpdate {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.url.is_none()
            && self.description.is_none()
            && self.subscriptions.is_none()
            && self.enabled.is_none()
    }
}

/// An endpoint with the secret that is shown exactly once.
#[derive(Clone, Debug)]
pub struct WebhookEndpointCreated {
    pub endpoint: WebhookEndpointRecord,
    pub signing_secret: WebhookSigningSecret,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookDeliveryState {
    Pending,
    Delivered,
    Failed,
}

impl WebhookDeliveryState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Pending, Self::Delivered, Self::Failed]
            .into_iter()
            .find(|state| state.as_str() == value)
    }
}

/// One delivery in an endpoint's log. Mirrors the public `WebhookDelivery`
/// plus what the body needs (`previousRevision`, `occurredAt`) and the
/// bookkeeping the retry window needs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebhookDeliveryRecord {
    id: WebhookDeliveryId,
    endpoint_id: WebhookEndpointId,
    tenant: TenantScope,
    event: WebhookEvent,
    collection_id: CollectionId,
    document_id: String,
    revision: String,
    previous_revision: Option<String>,
    commit_position: u64,
    /// When the change was observed by intake; a redelivery keeps its
    /// original's.
    occurred_at_unix_seconds: u64,
    state: WebhookDeliveryState,
    attempts: u64,
    first_attempt_at_unix_seconds: Option<u64>,
    next_attempt_at_unix_seconds: Option<u64>,
    last_response_status: Option<u16>,
    last_error: Option<String>,
    redelivery_of: Option<WebhookDeliveryId>,
    created_at_unix_seconds: u64,
    delivered_at_unix_seconds: Option<u64>,
}

impl WebhookDeliveryRecord {
    #[must_use]
    pub fn id(&self) -> &WebhookDeliveryId {
        &self.id
    }

    #[must_use]
    pub fn endpoint_id(&self) -> &WebhookEndpointId {
        &self.endpoint_id
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub const fn event(&self) -> WebhookEvent {
        self.event
    }

    #[must_use]
    pub fn collection_id(&self) -> &CollectionId {
        &self.collection_id
    }

    #[must_use]
    pub fn document_id(&self) -> &str {
        &self.document_id
    }

    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    #[must_use]
    pub fn previous_revision(&self) -> Option<&str> {
        self.previous_revision.as_deref()
    }

    #[must_use]
    pub const fn commit_position(&self) -> u64 {
        self.commit_position
    }

    #[must_use]
    pub const fn occurred_at_unix_seconds(&self) -> u64 {
        self.occurred_at_unix_seconds
    }

    #[must_use]
    pub const fn state(&self) -> WebhookDeliveryState {
        self.state
    }

    #[must_use]
    pub const fn attempts(&self) -> u64 {
        self.attempts
    }

    #[must_use]
    pub const fn next_attempt_at_unix_seconds(&self) -> Option<u64> {
        self.next_attempt_at_unix_seconds
    }

    #[must_use]
    pub const fn last_response_status(&self) -> Option<u16> {
        self.last_response_status
    }

    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    #[must_use]
    pub const fn redelivery_of(&self) -> Option<&WebhookDeliveryId> {
        self.redelivery_of.as_ref()
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn delivered_at_unix_seconds(&self) -> Option<u64> {
        self.delivered_at_unix_seconds
    }

    /// The log entry name: reverse-timestamped so a forward scan is newest
    /// first, reverse commit position within a second, then the id so
    /// entries never collide.
    fn item(&self) -> String {
        delivery_item(self.created_at_unix_seconds, self.commit_position, &self.id)
    }

    /// The pending-index entry name: creation order, then commit order.
    fn pending_item(&self) -> String {
        format!(
            "{:020}/{:020}/{}",
            self.created_at_unix_seconds,
            self.commit_position,
            self.id.as_str()
        )
    }

    /// The due-index entry name while pending: next attempt time, then id.
    fn due_item(&self) -> Option<String> {
        if self.state != WebhookDeliveryState::Pending {
            return None;
        }
        self.next_attempt_at_unix_seconds
            .map(|at| due_item(at, &self.id))
    }

    fn redelivery(&self, now_unix_seconds: u64) -> Self {
        Self {
            id: WebhookDeliveryId::generate(),
            endpoint_id: self.endpoint_id.clone(),
            tenant: self.tenant.clone(),
            event: self.event,
            collection_id: self.collection_id.clone(),
            document_id: self.document_id.clone(),
            revision: self.revision.clone(),
            previous_revision: self.previous_revision.clone(),
            commit_position: self.commit_position,
            occurred_at_unix_seconds: self.occurred_at_unix_seconds,
            state: WebhookDeliveryState::Pending,
            attempts: 0,
            first_attempt_at_unix_seconds: None,
            next_attempt_at_unix_seconds: Some(now_unix_seconds),
            last_response_status: None,
            last_error: None,
            redelivery_of: Some(self.id.clone()),
            created_at_unix_seconds: now_unix_seconds,
            delivered_at_unix_seconds: None,
        }
    }

    fn delivered(&mut self, status: u16, now_unix_seconds: u64) {
        self.attempts = self.attempts.saturating_add(1);
        self.first_attempt_at_unix_seconds
            .get_or_insert(now_unix_seconds);
        self.state = WebhookDeliveryState::Delivered;
        self.next_attempt_at_unix_seconds = None;
        self.last_response_status = Some(status);
        self.last_error = None;
        self.delivered_at_unix_seconds = Some(now_unix_seconds);
    }

    /// Records a failed attempt: schedules the retry, or fails the delivery
    /// once the retry window since the first attempt has closed.
    fn failed_attempt(
        &mut self,
        status: Option<u16>,
        error_code: &str,
        now_unix_seconds: u64,
        config: &WebhookWorkerConfig,
    ) {
        self.attempts = self.attempts.saturating_add(1);
        let first = *self
            .first_attempt_at_unix_seconds
            .get_or_insert(now_unix_seconds);
        self.last_response_status = status;
        self.last_error = Some(error_code.to_owned());
        if now_unix_seconds.saturating_sub(first) >= config.retry_window_seconds {
            self.state = WebhookDeliveryState::Failed;
            self.next_attempt_at_unix_seconds = None;
        } else {
            let backoff = config.backoff(self.attempts);
            self.next_attempt_at_unix_seconds = Some(now_unix_seconds.saturating_add(backoff));
        }
    }

    fn abandoned(&mut self, reason: &str) {
        self.state = WebhookDeliveryState::Failed;
        self.next_attempt_at_unix_seconds = None;
        self.last_error = Some(reason.to_owned());
    }
}

fn due_item(next_attempt_at_unix_seconds: u64, id: &WebhookDeliveryId) -> String {
    format!("{next_attempt_at_unix_seconds:020}/{}", id.as_str())
}

fn delivery_item(
    created_at_unix_seconds: u64,
    commit_position: u64,
    id: &WebhookDeliveryId,
) -> String {
    format!(
        "{:020}/{:020}/{}",
        u64::MAX - created_at_unix_seconds,
        u64::MAX - commit_position,
        id.as_str()
    )
}

/// How a delivery log is read: optionally one state, from a cursor, at
/// most `limit` entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebhookDeliveryQuery {
    pub state: Option<WebhookDeliveryState>,
    pub cursor: Option<String>,
    pub limit: NonZeroUsize,
}

/// A page of an endpoint's log, newest first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WebhookDeliveryPage {
    pub items: Vec<WebhookDeliveryRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EndpointIndexEntry {
    tenant: TenantScope,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CursorRecord {
    position: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PendingEntry {
    delivery_id: WebhookDeliveryId,
    delivery_item: String,
    document_id: String,
    next_attempt_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DueEntry {
    tenant: TenantScope,
    endpoint_id: WebhookEndpointId,
    delivery_id: WebhookDeliveryId,
}

/// A pending-index entry with the key it was read under.
struct PendingIndexEntry {
    key: Vec<u8>,
    entry: PendingEntry,
}

/// One endpoint's due entries as the due index holds them.
struct DueGroup {
    endpoint_id: WebhookEndpointId,
    tenant: TenantScope,
    /// Due keys and the deliveries they name.
    entries: Vec<(Vec<u8>, WebhookDeliveryId)>,
}

/// Where the change feed comes from: the data plane in production, a queue
/// in tests. Calls are synchronous loopback requests.
pub trait WebhookChangeSource: Send + Sync {
    fn read_change_feed(
        &self,
        tenant: &TenantScope,
        input: &ReadChangeFeedInput,
    ) -> Result<ReadChangeFeedOutput, WebhookSourceError>;
}

impl WebhookChangeSource for ControlToDataClient {
    fn read_change_feed(
        &self,
        tenant: &TenantScope,
        input: &ReadChangeFeedInput,
    ) -> Result<ReadChangeFeedOutput, WebhookSourceError> {
        let request_id = random_token("req_");
        let idempotency_key = random_token("idem_");
        ControlToDataClient::read_change_feed(
            self,
            tenant,
            &request_id,
            &idempotency_key,
            WORKER_ACTOR_ID,
            input,
        )
        .map_err(|error| match error {
            InternalClientError::Remote { status: 404, .. } => {
                WebhookSourceError::CollectionNotFound
            }
            other => WebhookSourceError::Unavailable(Some(other)),
        })
    }
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

#[derive(Debug)]
pub enum WebhookSourceError {
    /// The collection has no metadata in the data plane.
    CollectionNotFound,
    Unavailable(Option<InternalClientError>),
}

impl fmt::Display for WebhookSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CollectionNotFound => "collection was not found in the data plane",
            Self::Unavailable(_) => "change feed is unreachable",
        })
    }
}

impl Error for WebhookSourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable(Some(error)) => Some(error),
            _ => None,
        }
    }
}

/// How a delivery is posted: the bounded HTTPS client in production, an
/// in-memory sink in tests. Returns the response status.
pub trait WebhookTransport: Send + Sync {
    fn post(
        &self,
        url: &Url,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<u16, WebhookTransportError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebhookTransportError {
    InvalidUrl,
    ConnectRefused,
    Timeout,
    Tls,
    /// The endpoint answered, but not with HTTP this client accepts.
    InvalidResponse,
}

impl WebhookTransportError {
    /// The stable, non-sensitive reason a delivery log shows.
    #[must_use]
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::ConnectRefused => "connect_refused",
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

impl fmt::Display for WebhookTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.stable_code())
    }
}

impl Error for WebhookTransportError {}

/// The production transport: one request, one connection, TLS through
/// rustls with the Mozilla roots, and plain HTTP to loopback only where the
/// deployment admits it.
#[derive(Clone, Debug)]
pub struct HttpWebhookTransport {
    client: HttpClient,
}

impl HttpWebhookTransport {
    #[must_use]
    pub fn new(allow_plain_http_loopback: bool) -> Self {
        Self {
            client: HttpClient::new(HttpClientConfig {
                connect_timeout: Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECONDS),
                io_timeout: Duration::from_secs(DELIVERY_TIMEOUT_SECONDS),
                maximum_response_bytes: HTTP_MAXIMUM_RESPONSE_BYTES,
                allow_plain_http_loopback,
            }),
        }
    }
}

impl WebhookTransport for HttpWebhookTransport {
    fn post(
        &self,
        url: &Url,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<u16, WebhookTransportError> {
        self.client
            .request("POST", url, headers, body)
            .map(|response| response.status)
            .map_err(|error| match error {
                HttpClientError::UnsupportedUrl => WebhookTransportError::InvalidUrl,
                HttpClientError::Connect => WebhookTransportError::ConnectRefused,
                HttpClientError::Tls => WebhookTransportError::Tls,
                HttpClientError::Io => WebhookTransportError::Timeout,
                HttpClientError::Response => WebhookTransportError::InvalidResponse,
            })
    }
}

/// `t=<unix seconds>,v1=<lowercase hex HMAC-SHA256(secret, "<t>.<body>")>`.
#[must_use]
pub fn webhook_signature(secret: &[u8], timestamp_unix_seconds: u64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(timestamp_unix_seconds.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut signature = String::with_capacity(8 + 20 + digest.len() * 2);
    signature.push_str("t=");
    signature.push_str(&timestamp_unix_seconds.to_string());
    signature.push_str(",v1=");
    for byte in digest {
        use fmt::Write as _;
        let _ = write!(signature, "{byte:02x}");
    }
    signature
}

/// The delivery body, in exactly this field order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryBody<'a> {
    id: &'a str,
    event: WebhookEvent,
    collection: &'a str,
    document_id: &'a str,
    revision: &'a str,
    previous_revision: Option<&'a str>,
    commit_position: u64,
    occurred_at: String,
    project_id: &'a str,
    environment_id: &'a str,
    redelivery_of: Option<&'a str>,
}

fn format_timestamp(unix_seconds: u64) -> Result<String, WebhookError> {
    i64::try_from(unix_seconds)
        .ok()
        .and_then(|timestamp| OffsetDateTime::from_unix_timestamp(timestamp).ok())
        .and_then(|time| time.format(&Rfc3339).ok())
        .ok_or(WebhookError::CorruptRecord)
}

/// An absolute `https` URL with a host and no credentials; `http` only to
/// loopback and only where the deployment admits it.
fn validate_url(raw: &str, allow_plain_http_loopback: bool) -> Result<Url, WebhookError> {
    if raw.is_empty() || raw.len() > MAXIMUM_URL_BYTES || raw.chars().any(char::is_control) {
        return Err(WebhookError::InvalidRequest(
            "url must be an absolute https URL of at most 2048 bytes",
        ));
    }
    let url = Url::parse(raw).map_err(|_| {
        WebhookError::InvalidRequest("url must be an absolute https URL of at most 2048 bytes")
    })?;
    let host = url
        .host_str()
        .ok_or(WebhookError::InvalidRequest("url must name a host"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(WebhookError::InvalidRequest(
            "url must not carry credentials",
        ));
    }
    match url.scheme() {
        "https" => {}
        "http" if allow_plain_http_loopback && is_loopback_host(host) => {}
        "http" => {
            return Err(WebhookError::InvalidRequest(
                "url must use https; http is admitted only to loopback outside production",
            ));
        }
        _ => {
            return Err(WebhookError::InvalidRequest("url must use https"));
        }
    }
    Ok(url)
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_description(description: &str) -> Result<(), WebhookError> {
    if description.len() > MAXIMUM_DESCRIPTION_BYTES || description.chars().any(char::is_control) {
        return Err(WebhookError::InvalidRequest(
            "description must be at most 256 bytes without control characters",
        ));
    }
    Ok(())
}

/// One to thirty-two subscriptions, each naming a distinct collection with
/// one to three distinct events.
fn validate_subscriptions(subscriptions: &[WebhookSubscription]) -> Result<(), WebhookError> {
    if subscriptions.is_empty() || subscriptions.len() > MAXIMUM_SUBSCRIPTIONS {
        return Err(WebhookError::InvalidRequest(
            "subscriptions must name between 1 and 32 collections",
        ));
    }
    let mut collections = BTreeSet::new();
    for subscription in subscriptions {
        if !collections.insert(subscription.collection_id.as_str()) {
            return Err(WebhookError::InvalidRequest(
                "subscriptions must not name a collection twice",
            ));
        }
        if subscription.events.is_empty() || subscription.events.len() > WebhookEvent::ALL.len() {
            return Err(WebhookError::InvalidRequest(
                "subscription events must list between 1 and 3 distinct events",
            ));
        }
        let distinct: BTreeSet<WebhookEvent> = subscription.events.iter().copied().collect();
        if distinct.len() != subscription.events.len() {
            return Err(WebhookError::InvalidRequest(
                "subscription events must be distinct",
            ));
        }
    }
    Ok(())
}

/// The webhook records in the control store: endpoints, cursors, the
/// delivery log, and the indexes the worker walks.
#[derive(Clone)]
pub struct WebhookStore {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for WebhookStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl WebhookStore {
    pub fn new(adapter: Arc<dyn KvAdapter>, durability: Durability) -> Result<Self, WebhookError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(WebhookError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
        })
    }

    async fn apply(&self, write: AtomicWrite) -> Result<bool, WebhookError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(true),
            CompareAndWriteResult::Conflict { .. } => Ok(false),
        }
    }

    async fn write_unconditionally(&self, batch: WriteBatch) -> Result<(), WebhookError> {
        self.adapter
            .compare_and_write(AtomicWrite::unconditional(batch, self.durability))
            .await?;
        Ok(())
    }

    async fn scan(
        &self,
        range: KeyRange,
        direction: ScanDirection,
        limit: usize,
    ) -> Result<Vec<KeyValue>, WebhookError> {
        Ok(self
            .adapter
            .scan(ScanRequest::new(
                range,
                direction,
                NonZeroUsize::new(limit).ok_or(WebhookError::LimitExceeded)?,
            ))
            .await?)
    }

    /// Every registered endpoint the worker should consider, with its tenant.
    async fn registry(&self) -> Result<Vec<(WebhookEndpointId, TenantScope)>, WebhookError> {
        let entries = self
            .scan(
                ControlKeyspace::webhook_endpoint_index_range()?,
                ScanDirection::Forward,
                MAXIMUM_REGISTRY_SCAN,
            )
            .await?;
        if entries.len() >= MAXIMUM_REGISTRY_SCAN {
            return Err(WebhookError::LimitExceeded);
        }
        let mut registry = Vec::with_capacity(entries.len());
        for entry in entries {
            let index: EndpointIndexEntry = serde_json::from_slice(&entry.value)?;
            let id = decode_index_item(&entry.key, "whk_")?;
            registry.push((WebhookEndpointId::parse(id)?, index.tenant));
        }
        Ok(registry)
    }

    pub async fn get_endpoint(
        &self,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
    ) -> Result<Option<WebhookEndpointRecord>, WebhookError> {
        let key = ControlKeyspace::webhook_endpoint_key(
            tenant.project_id(),
            tenant.environment_id(),
            id.as_str(),
        )?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: WebhookEndpointRecord = serde_json::from_slice(&value)?;
        if record.tenant != *tenant || record.id != *id {
            return Err(WebhookError::CorruptRecord);
        }
        Ok(Some(record))
    }

    pub async fn list_endpoints(
        &self,
        tenant: &TenantScope,
    ) -> Result<Vec<WebhookEndpointRecord>, WebhookError> {
        let entries = self
            .scan(
                ControlKeyspace::webhook_endpoints_range(
                    tenant.project_id(),
                    tenant.environment_id(),
                )?,
                ScanDirection::Forward,
                MAXIMUM_ENDPOINTS_PER_TENANT,
            )
            .await?;
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            let record: WebhookEndpointRecord = serde_json::from_slice(&entry.value)?;
            if record.tenant != *tenant {
                return Err(WebhookError::CorruptRecord);
            }
            records.push(record);
        }
        records.sort_by(|left, right| {
            left.created_at_unix_seconds
                .cmp(&right.created_at_unix_seconds)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(records)
    }

    async fn insert_endpoint(
        &self,
        record: &WebhookEndpointRecord,
        cursors: &BTreeMap<CollectionId, u64>,
    ) -> Result<(), WebhookError> {
        let key = ControlKeyspace::webhook_endpoint_key(
            record.tenant.project_id(),
            record.tenant.environment_id(),
            record.id.as_str(),
        )?;
        let mut batch = WriteBatch::with_capacity(2 + cursors.len());
        batch.put(&key, serde_json::to_vec(record)?);
        batch.put(
            ControlKeyspace::webhook_endpoint_index_key(record.id.as_str())?,
            serde_json::to_vec(&EndpointIndexEntry {
                tenant: record.tenant.clone(),
            })?,
        );
        for (collection_id, position) in cursors {
            batch.put(
                self.cursor_key(&record.tenant, &record.id, collection_id)?,
                serde_json::to_vec(&CursorRecord {
                    position: *position,
                })?,
            );
        }
        if self
            .apply(AtomicWrite {
                conditions: vec![KeyCondition::Missing { key }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            Ok(())
        } else {
            Err(WebhookError::Conflict("webhook endpoint already exists"))
        }
    }

    /// Replaces an endpoint record if it is still `previous`; `extra` rides
    /// in the same atomic write.
    async fn replace_endpoint(
        &self,
        previous: &WebhookEndpointRecord,
        next: &WebhookEndpointRecord,
        mut extra: WriteBatch,
    ) -> Result<bool, WebhookError> {
        if previous.id != next.id || previous.tenant != next.tenant {
            return Err(WebhookError::Conflict("webhook endpoint identity changed"));
        }
        let key = ControlKeyspace::webhook_endpoint_key(
            previous.tenant.project_id(),
            previous.tenant.environment_id(),
            previous.id.as_str(),
        )?;
        extra.put(&key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(previous)?,
            }],
            batch: extra,
            durability: self.durability,
        })
        .await
    }

    /// Reloads and rewrites an endpoint until the write lands, so a change
    /// the worker makes never clobbers one a developer made meanwhile.
    async fn update_endpoint_with<F>(
        &self,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
        mut change: F,
    ) -> Result<WebhookEndpointRecord, WebhookError>
    where
        F: FnMut(&mut WebhookEndpointRecord) -> Result<(), WebhookError>,
    {
        for _ in 0..4 {
            let previous = self
                .get_endpoint(tenant, id)
                .await?
                .ok_or(WebhookError::EndpointNotFound)?;
            let mut next = previous.clone();
            change(&mut next)?;
            if next == previous
                || self
                    .replace_endpoint(&previous, &next, WriteBatch::new())
                    .await?
            {
                return Ok(next);
            }
        }
        Err(WebhookError::Conflict(
            "webhook endpoint changed concurrently; try again",
        ))
    }

    /// Removes the endpoint, its registry entry, cursors, indexes, and log.
    async fn remove_endpoint(
        &self,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
    ) -> Result<(), WebhookError> {
        let (project_id, environment_id) = (tenant.project_id(), tenant.environment_id());
        // Due entries live in the node-wide index keyed by time; the pending
        // index says which ones belong to this endpoint. Each round removes
        // the pending entries it read, so the next round reads the rest.
        for _ in 0..MAXIMUM_REMOVAL_ROUNDS {
            let pending = self.pending(tenant, id).await?;
            if pending.is_empty() {
                break;
            }
            let mut batch = WriteBatch::with_capacity(pending.len() * 2);
            for PendingIndexEntry { key, entry } in &pending {
                batch.delete(ControlKeyspace::webhook_due_key(&due_item(
                    entry.next_attempt_at_unix_seconds,
                    &entry.delivery_id,
                ))?);
                batch.delete(key);
            }
            self.write_unconditionally(batch).await?;
        }
        for range in [
            ControlKeyspace::webhook_pending_range(project_id, environment_id, id.as_str())?,
            ControlKeyspace::webhook_delivery_index_range(project_id, environment_id, id.as_str())?,
            ControlKeyspace::webhook_deliveries_range(project_id, environment_id, id.as_str())?,
            ControlKeyspace::webhook_cursors_range(project_id, environment_id, id.as_str())?,
        ] {
            self.delete_range(range).await?;
        }
        let mut batch = WriteBatch::with_capacity(2);
        batch.delete(ControlKeyspace::webhook_endpoint_key(
            project_id,
            environment_id,
            id.as_str(),
        )?);
        batch.delete(ControlKeyspace::webhook_endpoint_index_key(id.as_str())?);
        self.write_unconditionally(batch).await
    }

    async fn delete_range(&self, range: KeyRange) -> Result<(), WebhookError> {
        for _ in 0..MAXIMUM_REMOVAL_ROUNDS {
            let entries = self
                .scan(range.clone(), ScanDirection::Forward, REMOVAL_BATCH)
                .await?;
            if entries.is_empty() {
                return Ok(());
            }
            let exhausted = entries.len() < REMOVAL_BATCH;
            let mut batch = WriteBatch::with_capacity(entries.len());
            for entry in entries {
                batch.delete(entry.key);
            }
            self.write_unconditionally(batch).await?;
            if exhausted {
                return Ok(());
            }
        }
        Err(WebhookError::LimitExceeded)
    }

    fn cursor_key(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        collection_id: &CollectionId,
    ) -> Result<Vec<u8>, WebhookError> {
        Ok(ControlKeyspace::webhook_cursor_key(
            tenant.project_id(),
            tenant.environment_id(),
            endpoint_id.as_str(),
            collection_id.as_str(),
        )?)
    }

    pub async fn cursor(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        collection_id: &CollectionId,
    ) -> Result<Option<u64>, WebhookError> {
        self.adapter
            .get(&self.cursor_key(tenant, endpoint_id, collection_id)?)
            .await?
            .map(|value| {
                serde_json::from_slice::<CursorRecord>(&value)
                    .map(|record| record.position)
                    .map_err(WebhookError::from)
            })
            .transpose()
    }

    async fn put_cursors(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        cursors: &BTreeMap<CollectionId, u64>,
        removed: &[CollectionId],
    ) -> Result<(), WebhookError> {
        let mut batch = WriteBatch::with_capacity(cursors.len() + removed.len());
        for (collection_id, position) in cursors {
            batch.put(
                self.cursor_key(tenant, endpoint_id, collection_id)?,
                serde_json::to_vec(&CursorRecord {
                    position: *position,
                })?,
            );
        }
        for collection_id in removed {
            batch.delete(self.cursor_key(tenant, endpoint_id, collection_id)?);
        }
        if cursors.is_empty() && removed.is_empty() {
            return Ok(());
        }
        self.write_unconditionally(batch).await
    }

    /// Stores a page of deliveries and advances the cursor that produced
    /// them in one atomic write, conditioned on the cursor not having moved.
    /// Intake is therefore idempotent per (endpoint, collection, commit
    /// position): a position is only ever read past the cursor once.
    async fn enqueue_page(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        collection_id: &CollectionId,
        previous_cursor: u64,
        next_cursor: u64,
        records: &[WebhookDeliveryRecord],
    ) -> Result<bool, WebhookError> {
        let cursor_key = self.cursor_key(tenant, endpoint_id, collection_id)?;
        let mut batch = WriteBatch::with_capacity(records.len() * 4 + 1);
        for record in records {
            self.put_delivery(&mut batch, None, record)?;
        }
        batch.put(
            &cursor_key,
            serde_json::to_vec(&CursorRecord {
                position: next_cursor,
            })?,
        );
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key: cursor_key,
                value: serde_json::to_vec(&CursorRecord {
                    position: previous_cursor,
                })?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    /// Writes a delivery record and keeps the id, pending, and due indexes
    /// in step with it.
    fn put_delivery(
        &self,
        batch: &mut WriteBatch,
        previous: Option<&WebhookDeliveryRecord>,
        record: &WebhookDeliveryRecord,
    ) -> Result<(), WebhookError> {
        let (project_id, environment_id) =
            (record.tenant.project_id(), record.tenant.environment_id());
        let endpoint_id = record.endpoint_id.as_str();
        let item = record.item();
        batch.put(
            ControlKeyspace::webhook_delivery_key(project_id, environment_id, endpoint_id, &item)?,
            serde_json::to_vec(record)?,
        );
        batch.put(
            ControlKeyspace::webhook_delivery_index_key(
                project_id,
                environment_id,
                endpoint_id,
                record.id.as_str(),
            )?,
            item.clone().into_bytes(),
        );
        if let Some(due) = previous.and_then(WebhookDeliveryRecord::due_item) {
            batch.delete(ControlKeyspace::webhook_due_key(&due)?);
        }
        let pending_key = ControlKeyspace::webhook_pending_key(
            project_id,
            environment_id,
            endpoint_id,
            &record.pending_item(),
        )?;
        match (record.state, record.next_attempt_at_unix_seconds) {
            (WebhookDeliveryState::Pending, Some(next_attempt)) => {
                batch.put(
                    pending_key,
                    serde_json::to_vec(&PendingEntry {
                        delivery_id: record.id.clone(),
                        delivery_item: item,
                        document_id: record.document_id.clone(),
                        next_attempt_at_unix_seconds: next_attempt,
                    })?,
                );
                batch.put(
                    ControlKeyspace::webhook_due_key(
                        &record.due_item().ok_or(WebhookError::CorruptRecord)?,
                    )?,
                    serde_json::to_vec(&DueEntry {
                        tenant: record.tenant.clone(),
                        endpoint_id: record.endpoint_id.clone(),
                        delivery_id: record.id.clone(),
                    })?,
                );
            }
            _ => {
                batch.delete(pending_key);
            }
        }
        Ok(())
    }

    async fn insert_delivery(&self, record: &WebhookDeliveryRecord) -> Result<(), WebhookError> {
        let key = ControlKeyspace::webhook_delivery_key(
            record.tenant.project_id(),
            record.tenant.environment_id(),
            record.endpoint_id.as_str(),
            &record.item(),
        )?;
        let mut batch = WriteBatch::with_capacity(4);
        self.put_delivery(&mut batch, None, record)?;
        if self
            .apply(AtomicWrite {
                conditions: vec![KeyCondition::Missing { key }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            Ok(())
        } else {
            Err(WebhookError::Conflict("webhook delivery already exists"))
        }
    }

    async fn replace_delivery(
        &self,
        previous: &WebhookDeliveryRecord,
        next: &WebhookDeliveryRecord,
    ) -> Result<bool, WebhookError> {
        if previous.id != next.id
            || previous.tenant != next.tenant
            || previous.endpoint_id != next.endpoint_id
            || previous.created_at_unix_seconds != next.created_at_unix_seconds
        {
            return Err(WebhookError::Conflict("webhook delivery identity changed"));
        }
        let key = ControlKeyspace::webhook_delivery_key(
            previous.tenant.project_id(),
            previous.tenant.environment_id(),
            previous.endpoint_id.as_str(),
            &previous.item(),
        )?;
        let mut batch = WriteBatch::with_capacity(4);
        self.put_delivery(&mut batch, Some(previous), next)?;
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

    async fn get_delivery_by_item(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        item: &str,
    ) -> Result<Option<WebhookDeliveryRecord>, WebhookError> {
        let key = ControlKeyspace::webhook_delivery_key(
            tenant.project_id(),
            tenant.environment_id(),
            endpoint_id.as_str(),
            item,
        )?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: WebhookDeliveryRecord = serde_json::from_slice(&value)?;
        if record.tenant != *tenant || record.endpoint_id != *endpoint_id {
            return Err(WebhookError::CorruptRecord);
        }
        Ok(Some(record))
    }

    pub async fn get_delivery(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        id: &WebhookDeliveryId,
    ) -> Result<Option<WebhookDeliveryRecord>, WebhookError> {
        let key = ControlKeyspace::webhook_delivery_index_key(
            tenant.project_id(),
            tenant.environment_id(),
            endpoint_id.as_str(),
            id.as_str(),
        )?;
        let Some(item) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let item = String::from_utf8(item).map_err(|_| WebhookError::CorruptRecord)?;
        let record = self
            .get_delivery_by_item(tenant, endpoint_id, &item)
            .await?;
        if record.as_ref().is_some_and(|record| record.id != *id) {
            return Err(WebhookError::CorruptRecord);
        }
        Ok(record)
    }

    /// An endpoint's oldest pending deliveries, in order, up to a bound. A
    /// backlog beyond the bound is simply drained over more passes.
    async fn pending(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
    ) -> Result<Vec<PendingIndexEntry>, WebhookError> {
        let entries = self
            .scan(
                ControlKeyspace::webhook_pending_range(
                    tenant.project_id(),
                    tenant.environment_id(),
                    endpoint_id.as_str(),
                )?,
                ScanDirection::Forward,
                MAXIMUM_PENDING_SCAN,
            )
            .await?;
        entries
            .into_iter()
            .map(|KeyValue { key, value }| {
                Ok(PendingIndexEntry {
                    key,
                    entry: serde_json::from_slice(&value)?,
                })
            })
            .collect()
    }

    /// Endpoints with at least one delivery due at `now`, in the order their
    /// earliest due delivery fell due, each with the due keys that named it.
    async fn due_groups(&self, now_unix_seconds: u64) -> Result<Vec<DueGroup>, WebhookError> {
        let entries = self
            .scan(
                ControlKeyspace::webhook_due_range()?,
                ScanDirection::Forward,
                MAXIMUM_DUE_SCAN,
            )
            .await?;
        let mut groups: Vec<DueGroup> = Vec::new();
        for entry in entries {
            let item = decode_index_item(&entry.key, "")?;
            let due_at: u64 = item
                .split('/')
                .next()
                .and_then(|prefix| prefix.parse().ok())
                .ok_or(WebhookError::CorruptRecord)?;
            if due_at > now_unix_seconds {
                break;
            }
            let due: DueEntry = serde_json::from_slice(&entry.value)?;
            if !item.ends_with(due.delivery_id.as_str()) {
                return Err(WebhookError::CorruptRecord);
            }
            match groups
                .iter_mut()
                .find(|group| group.endpoint_id == due.endpoint_id)
            {
                Some(group) => group.entries.push((entry.key, due.delivery_id)),
                None => groups.push(DueGroup {
                    endpoint_id: due.endpoint_id,
                    tenant: due.tenant,
                    entries: vec![(entry.key, due.delivery_id)],
                }),
            }
        }
        Ok(groups)
    }

    async fn delete_keys(
        &self,
        keys: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<(), WebhookError> {
        let mut batch = WriteBatch::new();
        for key in keys {
            batch.delete(key);
        }
        if batch.is_empty() {
            return Ok(());
        }
        self.write_unconditionally(batch).await
    }

    /// The log newest first, optionally narrowed to one state, from a cursor.
    async fn list_deliveries(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        state: Option<WebhookDeliveryState>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<WebhookDeliveryPage, WebhookError> {
        let (project_id, environment_id) = (tenant.project_id(), tenant.environment_id());
        let mut range = ControlKeyspace::webhook_deliveries_range(
            project_id,
            environment_id,
            endpoint_id.as_str(),
        )?;
        let cursor_key = cursor
            .map(|cursor| {
                let item = decode_cursor(cursor)?;
                ControlKeyspace::webhook_delivery_key(
                    project_id,
                    environment_id,
                    endpoint_id.as_str(),
                    &item,
                )
                .map_err(WebhookError::from)
            })
            .transpose()?;
        if let Some(cursor_key) = &cursor_key {
            if !range.contains(cursor_key) {
                return Err(WebhookError::InvalidRequest("cursor is invalid"));
            }
            range.start_inclusive.clone_from(cursor_key);
        }
        let entries = self
            .scan(range, ScanDirection::Forward, MAXIMUM_LOG_SCAN)
            .await?;
        let mut items = Vec::with_capacity(limit.min(entries.len()));
        let mut remaining = false;
        for entry in entries {
            if cursor_key.as_ref() == Some(&entry.key) {
                continue;
            }
            let record: WebhookDeliveryRecord = serde_json::from_slice(&entry.value)?;
            if record.tenant != *tenant || record.endpoint_id != *endpoint_id {
                return Err(WebhookError::CorruptRecord);
            }
            if state.is_some_and(|state| record.state != state) {
                continue;
            }
            if items.len() == limit {
                remaining = true;
                break;
            }
            items.push(record);
        }
        let next_cursor = if remaining {
            items.last().map(|record| encode_cursor(&record.item()))
        } else {
            None
        };
        Ok(WebhookDeliveryPage { items, next_cursor })
    }

    /// Marks every pending delivery of an endpoint failed with `reason`.
    async fn abandon_pending(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        reason: &str,
    ) -> Result<usize, WebhookError> {
        let mut abandoned = 0;
        for _ in 0..MAXIMUM_REMOVAL_ROUNDS {
            let pending = self.pending(tenant, endpoint_id).await?;
            if pending.is_empty() {
                break;
            }
            for PendingIndexEntry { key, entry } in pending {
                match self
                    .get_delivery_by_item(tenant, endpoint_id, &entry.delivery_item)
                    .await?
                {
                    Some(previous) if previous.state == WebhookDeliveryState::Pending => {
                        let mut next = previous.clone();
                        next.abandoned(reason);
                        if self.replace_delivery(&previous, &next).await? {
                            abandoned += 1;
                        }
                    }
                    // The index outlived its delivery; drop the entry so the
                    // next round sees what is left.
                    _ => self.repair_pending(&key, &entry).await?,
                }
            }
        }
        Ok(abandoned)
    }

    /// Makes every pending delivery of an endpoint due now, so a resumed
    /// endpoint picks up where it stopped whatever backoff each was in.
    async fn resume_pending(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        now_unix_seconds: u64,
    ) -> Result<usize, WebhookError> {
        let mut resumed = 0;
        for PendingIndexEntry { key, entry } in self.pending(tenant, endpoint_id).await? {
            match self
                .get_delivery_by_item(tenant, endpoint_id, &entry.delivery_item)
                .await?
            {
                Some(previous) if previous.state == WebhookDeliveryState::Pending => {
                    let mut next = previous.clone();
                    next.next_attempt_at_unix_seconds = Some(now_unix_seconds);
                    if next != previous && self.replace_delivery(&previous, &next).await? {
                        resumed += 1;
                    }
                }
                _ => self.repair_pending(&key, &entry).await?,
            }
        }
        Ok(resumed)
    }

    /// Drops a pending-index entry and its due twin: the delivery they name
    /// is gone or no longer pending.
    async fn repair_pending(&self, key: &[u8], entry: &PendingEntry) -> Result<(), WebhookError> {
        self.delete_keys([
            key.to_vec(),
            ControlKeyspace::webhook_due_key(&due_item(
                entry.next_attempt_at_unix_seconds,
                &entry.delivery_id,
            ))?,
        ])
        .await
    }

    /// Removes terminal deliveries older than the retention window and, when
    /// the log holds more than the ceiling, the oldest terminal ones beyond
    /// it. Bounded per call; the next pass continues.
    async fn cleanup_endpoint(
        &self,
        tenant: &TenantScope,
        endpoint_id: &WebhookEndpointId,
        now_unix_seconds: u64,
        config: &WebhookWorkerConfig,
    ) -> Result<usize, WebhookError> {
        let (project_id, environment_id) = (tenant.project_id(), tenant.environment_id());
        let range = ControlKeyspace::webhook_deliveries_range(
            project_id,
            environment_id,
            endpoint_id.as_str(),
        )?;
        let count =
            usize::try_from(self.adapter.count_keys(range.clone()).await?).unwrap_or(usize::MAX);
        let mut excess = count.saturating_sub(config.maximum_deliveries_per_endpoint);
        // Oldest first: the log is newest first, so read it backwards.
        let entries = self
            .scan(range, ScanDirection::Reverse, RETENTION_BATCH)
            .await?;
        let mut batch = WriteBatch::new();
        let mut removed = 0;
        for entry in entries {
            let record: WebhookDeliveryRecord = serde_json::from_slice(&entry.value)?;
            if record.state == WebhookDeliveryState::Pending {
                continue;
            }
            let expired = record
                .created_at_unix_seconds
                .saturating_add(config.retention_seconds)
                <= now_unix_seconds;
            if !expired && excess == 0 {
                continue;
            }
            batch.delete(entry.key);
            batch.delete(ControlKeyspace::webhook_delivery_index_key(
                project_id,
                environment_id,
                endpoint_id.as_str(),
                record.id.as_str(),
            )?);
            excess = excess.saturating_sub(1);
            removed += 1;
        }
        if removed > 0 {
            self.write_unconditionally(batch).await?;
        }
        Ok(removed)
    }
}

/// The item segment of a system key: everything after the last domain
/// separator, unescaped. Index keys carry ids and timestamps only, so the
/// segment is ASCII and the escape byte never occurs inside it.
fn decode_index_item(key: &[u8], expected_prefix: &str) -> Result<String, WebhookError> {
    // Layout: [version, namespace] domain \0\0 item \0\0
    let body = key
        .strip_suffix(&[0, 0])
        .ok_or(WebhookError::CorruptRecord)?;
    let separator = body
        .windows(2)
        .rposition(|window| window == [0, 0])
        .ok_or(WebhookError::CorruptRecord)?;
    let item =
        std::str::from_utf8(&body[separator + 2..]).map_err(|_| WebhookError::CorruptRecord)?;
    if !item.starts_with(expected_prefix) {
        return Err(WebhookError::CorruptRecord);
    }
    Ok(item.to_owned())
}

fn encode_cursor(item: &str) -> String {
    URL_SAFE_NO_PAD.encode(item.as_bytes())
}

fn decode_cursor(cursor: &str) -> Result<String, WebhookError> {
    let invalid = || WebhookError::InvalidRequest("cursor is invalid");
    if cursor.is_empty() || cursor.len() > 256 {
        return Err(invalid());
    }
    let decoded = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
    let item = String::from_utf8(decoded).map_err(|_| invalid())?;
    let mut parts = item.split('/');
    for _ in 0..2 {
        let stamp = parts.next().ok_or_else(invalid)?;
        if stamp.len() != 20 || !stamp.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
    }
    let id = parts.next().ok_or_else(invalid)?;
    if parts.next().is_some() {
        return Err(invalid());
    }
    WebhookDeliveryId::parse(id).map_err(|_| invalid())?;
    Ok(item)
}

/// Registers, reads, changes, and removes endpoints with the same
/// membership rules as the environment's other resources: any member may
/// read; a role that can change projects may write. Every mutation is
/// audited.
#[derive(Clone)]
pub struct WebhookService {
    store: WebhookStore,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
    cipher: DeveloperMailCipher,
    source: Arc<dyn WebhookChangeSource>,
    allow_plain_http_loopback: bool,
}

impl fmt::Debug for WebhookService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookService")
            .field("store", &self.store)
            .field("allow_plain_http_loopback", &self.allow_plain_http_loopback)
            .finish_non_exhaustive()
    }
}

impl WebhookService {
    pub fn new(
        store: WebhookStore,
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
        cipher: DeveloperMailCipher,
        source: Arc<dyn WebhookChangeSource>,
        allow_plain_http_loopback: bool,
    ) -> Self {
        Self {
            store,
            projects,
            organizations,
            audit,
            cipher,
            source,
            allow_plain_http_loopback,
        }
    }

    #[must_use]
    pub fn store(&self) -> &WebhookStore {
        &self.store
    }

    pub async fn list(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<Vec<WebhookEndpointRecord>, WebhookError> {
        self.authorize(
            actor,
            tenant,
            ControlAuditAction::WebhookEndpointRead,
            false,
            now_unix_seconds,
        )
        .await?;
        self.store.list_endpoints(tenant).await
    }

    pub async fn get(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
        now_unix_seconds: u64,
    ) -> Result<WebhookEndpointRecord, WebhookError> {
        self.authorize(
            actor,
            tenant,
            ControlAuditAction::WebhookEndpointRead,
            false,
            now_unix_seconds,
        )
        .await?;
        self.store
            .get_endpoint(tenant, id)
            .await?
            .ok_or(WebhookError::EndpointNotFound)
    }

    /// Registers an endpoint. Cursors start at each subscribed collection's
    /// current high water, so only changes committed after registration are
    /// delivered. The secret is returned exactly once.
    pub async fn create(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        input: NewWebhookEndpoint,
        now_unix_seconds: u64,
    ) -> Result<WebhookEndpointCreated, WebhookError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::WebhookEndpointCreate,
                true,
                now_unix_seconds,
            )
            .await?;
        validate_url(&input.url, self.allow_plain_http_loopback)?;
        let description = input.description.unwrap_or_default();
        validate_description(&description)?;
        validate_subscriptions(&input.subscriptions)?;
        let id = WebhookEndpointId::generate();
        let secret = WebhookSigningSecret::generate();
        let sealed_secret = self
            .cipher
            .seal_bytes(&secret_aad(&id, 1), secret.expose().as_bytes())?;
        let cursors =
            self.initial_cursors(tenant, input.subscriptions.iter().map(|s| &s.collection_id))?;
        let record = WebhookEndpointRecord {
            id: id.clone(),
            tenant: tenant.clone(),
            url: input.url,
            description,
            subscriptions: input.subscriptions,
            enabled: input.enabled,
            paused_reason: None,
            paused_at_unix_seconds: None,
            consecutive_failures: 0,
            secret_version: 1,
            sealed_secret,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
        };
        self.store.insert_endpoint(&record, &cursors).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::WebhookEndpointCreate,
            id.as_str(),
            now_unix_seconds,
        );
        Ok(WebhookEndpointCreated {
            endpoint: record,
            signing_secret: secret,
        })
    }

    /// Changes URL, subscriptions, description, or the enabled flag. New
    /// subscriptions start at the current high water; removed ones drop
    /// their cursors. Re-enabling resets every cursor and abandons pending
    /// deliveries as `failed` / `disabled`.
    pub async fn update(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
        update: WebhookEndpointUpdate,
        now_unix_seconds: u64,
    ) -> Result<WebhookEndpointRecord, WebhookError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::WebhookEndpointUpdate,
                true,
                now_unix_seconds,
            )
            .await?;
        if update.is_empty() {
            return Err(WebhookError::InvalidRequest("update changes nothing"));
        }
        if let Some(url) = &update.url {
            validate_url(url, self.allow_plain_http_loopback)?;
        }
        if let Some(description) = &update.description {
            validate_description(description)?;
        }
        if let Some(subscriptions) = &update.subscriptions {
            validate_subscriptions(subscriptions)?;
        }
        let previous = self
            .store
            .get_endpoint(tenant, id)
            .await?
            .ok_or(WebhookError::EndpointNotFound)?;
        let mut next = previous.clone();
        if let Some(url) = update.url {
            next.url = url;
        }
        if let Some(description) = update.description {
            next.description = description;
        }
        if let Some(subscriptions) = update.subscriptions {
            next.subscriptions = subscriptions;
        }
        if let Some(enabled) = update.enabled {
            next.enabled = enabled;
        }
        let re_enabled = !previous.enabled && next.enabled;
        let before: BTreeSet<&CollectionId> = previous
            .subscriptions
            .iter()
            .map(|s| &s.collection_id)
            .collect();
        let after: BTreeSet<&CollectionId> = next
            .subscriptions
            .iter()
            .map(|s| &s.collection_id)
            .collect();
        let removed: Vec<CollectionId> =
            before.difference(&after).map(|id| (*id).clone()).collect();
        // Re-enabling starts every subscription over; otherwise only the new
        // ones need a cursor.
        let fresh: Vec<&CollectionId> = if re_enabled {
            after.iter().copied().collect()
        } else {
            after.difference(&before).copied().collect()
        };
        let cursors = self.initial_cursors(tenant, fresh.into_iter())?;
        if re_enabled {
            next.clear_pause(now_unix_seconds);
            self.store
                .abandon_pending(tenant, id, LAST_ERROR_DISABLED)
                .await?;
        }
        next.updated_at_unix_seconds = now_unix_seconds;
        if next != previous
            && !self
                .store
                .replace_endpoint(&previous, &next, WriteBatch::new())
                .await?
        {
            return Err(WebhookError::Conflict(
                "webhook endpoint changed concurrently; read it again",
            ));
        }
        self.store
            .put_cursors(tenant, id, &cursors, &removed)
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::WebhookEndpointUpdate,
            id.as_str(),
            now_unix_seconds,
        );
        Ok(next)
    }

    /// Removes the endpoint, its subscriptions, and its delivery log.
    pub async fn delete(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
        now_unix_seconds: u64,
    ) -> Result<(), WebhookError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::WebhookEndpointDelete,
                true,
                now_unix_seconds,
            )
            .await?;
        self.store
            .get_endpoint(tenant, id)
            .await?
            .ok_or(WebhookError::EndpointNotFound)?;
        self.store.remove_endpoint(tenant, id).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::WebhookEndpointDelete,
            id.as_str(),
            now_unix_seconds,
        );
        Ok(())
    }

    /// A new secret, shown once; deliveries attempted after this are signed
    /// with it alone.
    pub async fn rotate_secret(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
        now_unix_seconds: u64,
    ) -> Result<WebhookEndpointCreated, WebhookError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::WebhookEndpointRotateSecret,
                true,
                now_unix_seconds,
            )
            .await?;
        let secret = WebhookSigningSecret::generate();
        let cipher = self.cipher.clone();
        let endpoint = self
            .store
            .update_endpoint_with(tenant, id, |record| {
                let version = record
                    .secret_version
                    .checked_add(1)
                    .ok_or(WebhookError::CorruptRecord)?;
                record.sealed_secret = cipher
                    .seal_bytes(&secret_aad(&record.id, version), secret.expose().as_bytes())?;
                record.secret_version = version;
                record.updated_at_unix_seconds = now_unix_seconds;
                Ok(())
            })
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::WebhookEndpointRotateSecret,
            id.as_str(),
            now_unix_seconds,
        );
        Ok(WebhookEndpointCreated {
            endpoint,
            signing_secret: secret,
        })
    }

    /// Clears a platform pause and the failure counter. Pending deliveries
    /// pick up where they stopped; nothing already failed is revived.
    pub async fn resume(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
        now_unix_seconds: u64,
    ) -> Result<WebhookEndpointRecord, WebhookError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::WebhookEndpointResume,
                true,
                now_unix_seconds,
            )
            .await?;
        let endpoint = self
            .store
            .update_endpoint_with(tenant, id, |record| {
                if !record.enabled {
                    return Err(WebhookError::Conflict(
                        "webhook endpoint is disabled; enable it instead",
                    ));
                }
                if record.paused_at_unix_seconds.is_some() || record.consecutive_failures > 0 {
                    record.clear_pause(now_unix_seconds);
                }
                Ok(())
            })
            .await?;
        self.store
            .resume_pending(tenant, id, now_unix_seconds)
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::WebhookEndpointResume,
            id.as_str(),
            now_unix_seconds,
        );
        Ok(endpoint)
    }

    pub async fn list_deliveries(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
        query: &WebhookDeliveryQuery,
        now_unix_seconds: u64,
    ) -> Result<WebhookDeliveryPage, WebhookError> {
        self.authorize(
            actor,
            tenant,
            ControlAuditAction::WebhookEndpointRead,
            false,
            now_unix_seconds,
        )
        .await?;
        self.store
            .get_endpoint(tenant, id)
            .await?
            .ok_or(WebhookError::EndpointNotFound)?;
        self.store
            .list_deliveries(
                tenant,
                id,
                query.state,
                query.cursor.as_deref(),
                query.limit.get(),
            )
            .await
    }

    /// Queues a new signed delivery of the same event, logged as a
    /// redelivery of the original, which is left as it was. Refused while
    /// the endpoint is disabled or paused.
    pub async fn redeliver(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &WebhookEndpointId,
        delivery_id: &WebhookDeliveryId,
        now_unix_seconds: u64,
    ) -> Result<WebhookDeliveryRecord, WebhookError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::WebhookEndpointRedeliver,
                true,
                now_unix_seconds,
            )
            .await?;
        let endpoint = self
            .store
            .get_endpoint(tenant, id)
            .await?
            .ok_or(WebhookError::EndpointNotFound)?;
        match endpoint.state() {
            WebhookEndpointState::Active => {}
            WebhookEndpointState::Paused => {
                return Err(WebhookError::Conflict(
                    "webhook endpoint is paused; resume it first",
                ));
            }
            WebhookEndpointState::Disabled => {
                return Err(WebhookError::Conflict(
                    "webhook endpoint is disabled; enable it first",
                ));
            }
        }
        let original = self
            .store
            .get_delivery(tenant, id, delivery_id)
            .await?
            .ok_or(WebhookError::DeliveryNotFound)?;
        let record = original.redelivery(now_unix_seconds);
        self.store.insert_delivery(&record).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::WebhookEndpointRedeliver,
            &format!("{}/{}", id.as_str(), delivery_id.as_str()),
            now_unix_seconds,
        );
        Ok(record)
    }

    /// Each collection's cursor at the environment's current high water.
    /// Reading the feed proves the collection exists; a missing one refuses
    /// the request rather than leaving a subscription that can never fire.
    fn initial_cursors<'a>(
        &self,
        tenant: &TenantScope,
        collections: impl Iterator<Item = &'a CollectionId>,
    ) -> Result<BTreeMap<CollectionId, u64>, WebhookError> {
        let mut cursors = BTreeMap::new();
        for collection_id in collections {
            let feed = self
                .source
                .read_change_feed(
                    tenant,
                    &ReadChangeFeedInput {
                        collection_id: collection_id.as_str().to_owned(),
                        after_position: 0,
                        limit: 1,
                    },
                )
                .map_err(|error| match error {
                    WebhookSourceError::CollectionNotFound => {
                        WebhookError::CollectionNotFound(collection_id.as_str().to_owned())
                    }
                    other => WebhookError::Source(other),
                })?;
            cursors.insert(collection_id.clone(), feed.high_water);
        }
        Ok(cursors)
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        action: ControlAuditAction,
        write: bool,
        now_unix_seconds: u64,
    ) -> Result<OrganizationId, WebhookError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(WebhookError::EnvironmentNotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(WebhookError::EnvironmentNotFound)?;
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
                target: webhook_target(tenant, "authorization"),
                outcome: ControlAuditOutcome::Denied,
                at_unix_seconds: now_unix_seconds,
            });
            return Err(WebhookError::Forbidden);
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
        at_unix_seconds: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: webhook_target(tenant, target),
            outcome: ControlAuditOutcome::Allowed,
            at_unix_seconds,
        });
    }
}

fn webhook_target(tenant: &TenantScope, target: &str) -> String {
    format!(
        "{}/{}/webhooks/{}",
        tenant.project_id().as_str(),
        tenant.environment_id().as_str(),
        target
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebhookWorkerConfig {
    pub intake_page: u32,
    pub deliveries_per_endpoint: usize,
    pub base_backoff_seconds: u64,
    pub maximum_backoff_seconds: u64,
    /// Up to a quarter of the backoff is added at random; off in tests.
    pub jitter: bool,
    pub retry_window_seconds: u64,
    pub pause_threshold: u64,
    pub retention_seconds: u64,
    pub maximum_deliveries_per_endpoint: usize,
}

impl Default for WebhookWorkerConfig {
    fn default() -> Self {
        Self {
            intake_page: INTAKE_PAGE,
            deliveries_per_endpoint: DELIVERIES_PER_ENDPOINT_PER_PASS,
            base_backoff_seconds: BASE_BACKOFF_SECONDS,
            maximum_backoff_seconds: MAXIMUM_BACKOFF_SECONDS,
            jitter: true,
            retry_window_seconds: RETRY_WINDOW_SECONDS,
            pause_threshold: PAUSE_THRESHOLD,
            retention_seconds: DELIVERY_RETENTION_SECONDS,
            maximum_deliveries_per_endpoint: MAXIMUM_DELIVERIES_PER_ENDPOINT,
        }
    }
}

impl WebhookWorkerConfig {
    fn validate(&self) -> Result<(), WebhookError> {
        if !(1..=500).contains(&self.intake_page)
            || self.deliveries_per_endpoint == 0
            || self.base_backoff_seconds == 0
            || self.maximum_backoff_seconds < self.base_backoff_seconds
            || self.retry_window_seconds == 0
            || self.pause_threshold == 0
            || self.retention_seconds == 0
            || self.maximum_deliveries_per_endpoint == 0
        {
            return Err(WebhookError::InvalidConfiguration);
        }
        Ok(())
    }

    /// `min(base * 2^attempts, maximum)`, plus up to a quarter of that when
    /// jitter is on, so a fleet of endpoints that failed together does not
    /// retry together.
    fn backoff(&self, attempts: u64) -> u64 {
        let exponent = attempts.min(20);
        let base = self
            .base_backoff_seconds
            .saturating_mul(1_u64 << exponent)
            .min(self.maximum_backoff_seconds);
        if !self.jitter || base < 4 {
            return base;
        }
        let jitter = OsRng.next_u64() % (base / 4 + 1);
        base.saturating_add(jitter)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WebhookWorkerReport {
    /// Endpoints considered for intake this pass.
    pub endpoints: usize,
    /// Change-feed pages read.
    pub feed_reads: usize,
    /// Deliveries queued from the feed.
    pub queued: usize,
    /// Endpoint-subscriptions whose intake failed (feed unreachable or the
    /// collection gone); they are retried next pass.
    pub intake_failures: usize,
    pub attempted: usize,
    pub delivered: usize,
    pub retried: usize,
    pub failed: usize,
    /// Deliveries that were due but waited on an earlier one for the same document.
    pub deferred: usize,
    pub paused: usize,
    pub cleaned: usize,
}

/// Consumes change feeds into the outbox, then delivers what is due.
#[derive(Clone)]
pub struct WebhookWorker {
    store: WebhookStore,
    cipher: DeveloperMailCipher,
    transport: Arc<dyn WebhookTransport>,
    source: Arc<dyn WebhookChangeSource>,
    config: WebhookWorkerConfig,
}

impl fmt::Debug for WebhookWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookWorker")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl WebhookWorker {
    pub fn new(
        store: WebhookStore,
        cipher: DeveloperMailCipher,
        transport: Arc<dyn WebhookTransport>,
        source: Arc<dyn WebhookChangeSource>,
        config: WebhookWorkerConfig,
    ) -> Result<Self, WebhookError> {
        config.validate()?;
        Ok(Self {
            store,
            cipher,
            transport,
            source,
            config,
        })
    }

    #[must_use]
    pub fn store(&self) -> &WebhookStore {
        &self.store
    }

    /// One pass: intake for every delivering endpoint, delivery of what is
    /// due, then retention. A feed that cannot be reached is counted and
    /// left for the next pass; only a storage failure is an error.
    pub async fn run_once(
        &self,
        now_unix_seconds: u64,
    ) -> Result<WebhookWorkerReport, WebhookError> {
        let mut report = WebhookWorkerReport::default();
        let registry = self.store.registry().await?;
        report.endpoints = registry.len();
        let mut high_water: BTreeMap<String, u64> = BTreeMap::new();
        for (id, tenant) in &registry {
            let Some(endpoint) = self.store.get_endpoint(tenant, id).await? else {
                continue;
            };
            if !endpoint.delivers() {
                continue;
            }
            self.intake(&endpoint, now_unix_seconds, &mut high_water, &mut report)
                .await?;
        }
        for group in self.store.due_groups(now_unix_seconds).await? {
            self.deliver(group, now_unix_seconds, &mut report).await?;
        }
        for (id, tenant) in &registry {
            report.cleaned += self
                .store
                .cleanup_endpoint(tenant, id, now_unix_seconds, &self.config)
                .await?;
        }
        Ok(report)
    }

    async fn intake(
        &self,
        endpoint: &WebhookEndpointRecord,
        now_unix_seconds: u64,
        high_water: &mut BTreeMap<String, u64>,
        report: &mut WebhookWorkerReport,
    ) -> Result<(), WebhookError> {
        let tenant = &endpoint.tenant;
        let tenant_key = format!(
            "{}/{}",
            tenant.project_id().as_str(),
            tenant.environment_id().as_str()
        );
        for subscription in &endpoint.subscriptions {
            let collection_id = &subscription.collection_id;
            let mut cursor = match self
                .store
                .cursor(tenant, &endpoint.id, collection_id)
                .await?
            {
                Some(cursor) => cursor,
                None => {
                    // A subscription without a cursor starts now, like a
                    // registration would have.
                    let feed = match self.read(tenant, collection_id, 0, 1) {
                        Ok(feed) => feed,
                        Err(_) => {
                            report.intake_failures += 1;
                            continue;
                        }
                    };
                    report.feed_reads += 1;
                    high_water.insert(tenant_key.clone(), feed.high_water);
                    self.store
                        .put_cursors(
                            tenant,
                            &endpoint.id,
                            &BTreeMap::from([(collection_id.clone(), feed.high_water)]),
                            &[],
                        )
                        .await?;
                    continue;
                }
            };
            // The high water is environment-wide: once one read in this pass
            // showed nothing beyond this cursor, no other read will either.
            if high_water
                .get(&tenant_key)
                .is_some_and(|known| *known <= cursor)
            {
                continue;
            }
            for _ in 0..INTAKE_PAGES_PER_PASS {
                let feed = match self.read(tenant, collection_id, cursor, self.config.intake_page) {
                    Ok(feed) => feed,
                    Err(_) => {
                        report.intake_failures += 1;
                        break;
                    }
                };
                report.feed_reads += 1;
                high_water.insert(tenant_key.clone(), feed.high_water);
                let mut records = Vec::with_capacity(feed.changes.len());
                for change in &feed.changes {
                    let event = WebhookEvent::from(change.event);
                    if !subscription.wants(event)
                        || change.document_id.len() > MAXIMUM_DOCUMENT_ID_BYTES
                        || change.revision.len() > MAXIMUM_REVISION_BYTES
                    {
                        continue;
                    }
                    records.push(WebhookDeliveryRecord {
                        id: WebhookDeliveryId::generate(),
                        endpoint_id: endpoint.id.clone(),
                        tenant: tenant.clone(),
                        event,
                        collection_id: collection_id.clone(),
                        document_id: change.document_id.clone(),
                        revision: change.revision.clone(),
                        previous_revision: change.previous_revision.clone(),
                        commit_position: change.commit_position,
                        occurred_at_unix_seconds: now_unix_seconds,
                        state: WebhookDeliveryState::Pending,
                        attempts: 0,
                        first_attempt_at_unix_seconds: None,
                        next_attempt_at_unix_seconds: Some(now_unix_seconds),
                        last_response_status: None,
                        last_error: None,
                        redelivery_of: None,
                        created_at_unix_seconds: now_unix_seconds,
                        delivered_at_unix_seconds: None,
                    });
                }
                if feed.scanned_through != cursor || !records.is_empty() {
                    if !self
                        .store
                        .enqueue_page(
                            tenant,
                            &endpoint.id,
                            collection_id,
                            cursor,
                            feed.scanned_through,
                            &records,
                        )
                        .await?
                    {
                        // The cursor moved under us (a concurrent update or
                        // re-enable); this subscription starts over next pass.
                        break;
                    }
                    report.queued += records.len();
                }
                cursor = feed.scanned_through;
                if feed.exhausted {
                    break;
                }
            }
        }
        Ok(())
    }

    fn read(
        &self,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        after_position: u64,
        limit: u32,
    ) -> Result<ReadChangeFeedOutput, WebhookSourceError> {
        self.source.read_change_feed(
            tenant,
            &ReadChangeFeedInput {
                collection_id: collection_id.as_str().to_owned(),
                after_position,
                limit,
            },
        )
    }

    /// Delivers an endpoint's due deliveries oldest first, holding back any
    /// whose document still has an earlier delivery pending, and at most
    /// `deliveries_per_endpoint` in one pass. Due entries that no longer
    /// name a pending delivery of a delivering endpoint are dropped here, so
    /// a paused or removed endpoint cannot crowd the due index.
    async fn deliver(
        &self,
        group: DueGroup,
        now_unix_seconds: u64,
        report: &mut WebhookWorkerReport,
    ) -> Result<(), WebhookError> {
        let DueGroup {
            endpoint_id,
            tenant,
            entries,
        } = group;
        let (tenant, endpoint_id) = (&tenant, &endpoint_id);
        let mut endpoint = match self.store.get_endpoint(tenant, endpoint_id).await? {
            Some(endpoint) if endpoint.delivers() => endpoint,
            _ => {
                // Gone, paused, or disabled: a resume or re-enable rebuilds
                // the due entries from the pending index.
                return self
                    .store
                    .delete_keys(entries.into_iter().map(|(key, _)| key))
                    .await;
            }
        };
        let secret = self.cipher.open_bytes(
            &secret_aad(&endpoint.id, endpoint.secret_version),
            &endpoint.sealed_secret,
        )?;
        let pending = self.store.pending(tenant, endpoint_id).await?;
        let pending_ids: BTreeSet<&WebhookDeliveryId> = pending
            .iter()
            .map(|indexed| &indexed.entry.delivery_id)
            .collect();
        let stale: Vec<Vec<u8>> = entries
            .into_iter()
            .filter(|(_, id)| !pending_ids.contains(id))
            .map(|(key, _)| key)
            .collect();
        self.store.delete_keys(stale).await?;
        let mut blocked: BTreeSet<String> = BTreeSet::new();
        let mut attempted = 0;
        for PendingIndexEntry { key, entry } in pending {
            if attempted >= self.config.deliveries_per_endpoint {
                break;
            }
            let due = entry.next_attempt_at_unix_seconds <= now_unix_seconds;
            if blocked.contains(&entry.document_id) {
                if due {
                    report.deferred += 1;
                }
                continue;
            }
            if !due {
                blocked.insert(entry.document_id.clone());
                continue;
            }
            let previous = match self
                .store
                .get_delivery_by_item(tenant, endpoint_id, &entry.delivery_item)
                .await?
            {
                Some(previous) if previous.state == WebhookDeliveryState::Pending => previous,
                _ => {
                    self.store.repair_pending(&key, &entry).await?;
                    continue;
                }
            };
            attempted += 1;
            report.attempted += 1;
            let outcome = self.attempt(&endpoint, &secret, &previous, now_unix_seconds)?;
            let mut next = previous.clone();
            let succeeded = match outcome {
                Ok(status) if (200..300).contains(&status) => {
                    next.delivered(status, now_unix_seconds);
                    true
                }
                Ok(status) => {
                    let code = format!("status_{status}");
                    next.failed_attempt(Some(status), &code, now_unix_seconds, &self.config);
                    false
                }
                Err(error) => {
                    next.failed_attempt(None, error.stable_code(), now_unix_seconds, &self.config);
                    false
                }
            };
            self.store.replace_delivery(&previous, &next).await?;
            if succeeded {
                report.delivered += 1;
                if endpoint.consecutive_failures > 0 {
                    endpoint = self
                        .store
                        .update_endpoint_with(tenant, endpoint_id, |record| {
                            record.note_success();
                            Ok(())
                        })
                        .await?;
                }
                continue;
            }
            blocked.insert(previous.document_id.clone());
            if next.state == WebhookDeliveryState::Failed {
                report.failed += 1;
            } else {
                report.retried += 1;
            }
            let threshold = self.config.pause_threshold;
            let mut paused_now = false;
            endpoint = self
                .store
                .update_endpoint_with(tenant, endpoint_id, |record| {
                    paused_now = record.note_failure(threshold, now_unix_seconds);
                    Ok(())
                })
                .await?;
            if paused_now {
                report.paused += 1;
            }
            if !endpoint.delivers() {
                break;
            }
        }
        Ok(())
    }

    /// Signs and posts one delivery. The outer error is the control plane's
    /// own (a record that cannot be encoded); the inner one is the endpoint's.
    fn attempt(
        &self,
        endpoint: &WebhookEndpointRecord,
        secret: &[u8],
        record: &WebhookDeliveryRecord,
        now_unix_seconds: u64,
    ) -> Result<Result<u16, WebhookTransportError>, WebhookError> {
        let Ok(url) = Url::parse(&endpoint.url) else {
            return Ok(Err(WebhookTransportError::InvalidUrl));
        };
        let occurred_at = format_timestamp(record.occurred_at_unix_seconds)?;
        let body = serde_json::to_vec(&DeliveryBody {
            id: record.id.as_str(),
            event: record.event,
            collection: record.collection_id.as_str(),
            document_id: &record.document_id,
            revision: &record.revision,
            previous_revision: record.previous_revision.as_deref(),
            commit_position: record.commit_position,
            occurred_at,
            project_id: record.tenant.project_id().as_str(),
            environment_id: record.tenant.environment_id().as_str(),
            redelivery_of: record.redelivery_of.as_ref().map(WebhookDeliveryId::as_str),
        })?;
        let signature = webhook_signature(secret, now_unix_seconds, &body);
        let secret_version = endpoint.secret_version.to_string();
        let headers = [
            ("content-type", "application/json"),
            ("user-agent", USER_AGENT),
            ("x-mako-webhook-id", endpoint.id.as_str()),
            ("x-mako-delivery-id", record.id.as_str()),
            ("x-mako-event", record.event.as_str()),
            ("x-mako-secret-version", secret_version.as_str()),
            (SIGNATURE_HEADER, signature.as_str()),
        ];
        Ok(self.transport.post(&url, &headers, &body))
    }
}

#[derive(Debug)]
pub enum WebhookError {
    InvalidConfiguration,
    UnsupportedDurability,
    /// The project or environment does not exist.
    EnvironmentNotFound,
    EndpointNotFound,
    DeliveryNotFound,
    /// A subscribed collection has no metadata in the data plane.
    CollectionNotFound(String),
    Forbidden,
    Conflict(&'static str),
    InvalidRequest(&'static str),
    CorruptRecord,
    LimitExceeded,
    Source(WebhookSourceError),
    Encryption(DeveloperWorkflowError),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for WebhookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => formatter.write_str("webhook configuration is invalid"),
            Self::UnsupportedDurability => formatter.write_str("webhook durability is unsupported"),
            Self::EnvironmentNotFound => formatter.write_str("environment was not found"),
            Self::EndpointNotFound => formatter.write_str("webhook endpoint was not found"),
            Self::DeliveryNotFound => formatter.write_str("webhook delivery was not found"),
            Self::CollectionNotFound(collection) => {
                write!(formatter, "collection {collection} was not found")
            }
            Self::Forbidden => formatter.write_str("webhook operation is forbidden"),
            Self::Conflict(message) | Self::InvalidRequest(message) => formatter.write_str(message),
            Self::CorruptRecord => formatter.write_str("webhook record is corrupt"),
            Self::LimitExceeded => formatter.write_str("webhook scan exceeded its bound"),
            Self::Source(_) => formatter.write_str("change feed is unavailable"),
            Self::Encryption(_) => formatter.write_str("webhook secret could not be sealed"),
            Self::Project(_) => formatter.write_str("webhook project lookup failed"),
            Self::Organization(_) => formatter.write_str("webhook organization lookup failed"),
            Self::Keyspace(_) => formatter.write_str("webhook key is invalid"),
            Self::Storage(_) => formatter.write_str("webhook storage operation failed"),
            Self::Json(_) => formatter.write_str("webhook record encoding failed"),
        }
    }
}

impl Error for WebhookError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Encryption(error) => Some(error),
            Self::Project(error) => Some(error),
            Self::Organization(error) => Some(error),
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DeveloperWorkflowError> for WebhookError {
    fn from(value: DeveloperWorkflowError) -> Self {
        Self::Encryption(value)
    }
}

impl From<ProjectStoreError> for WebhookError {
    fn from(value: ProjectStoreError) -> Self {
        Self::Project(value)
    }
}

impl From<OrganizationStoreError> for WebhookError {
    fn from(value: OrganizationStoreError) -> Self {
        Self::Organization(value)
    }
}

impl From<ControlKeyspaceError> for WebhookError {
    fn from(value: ControlKeyspaceError) -> Self {
        Self::Keyspace(value)
    }
}

impl From<StorageError> for WebhookError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<serde_json::Error> for WebhookError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::executor::block_on;
    use mako_internal_rpc::ChangeFeedEntry;
    use serde_json::Value;

    use super::*;
    use crate::{
        DeveloperMailEncryptionKey,
        email_template::tests::{Fixture, NOW, fixture},
    };

    /// An environment's change log: positions are environment-wide, entries
    /// name their collection, and the high water is the last position.
    #[derive(Default)]
    struct FeedSource {
        collections: Mutex<BTreeSet<String>>,
        entries: Mutex<Vec<(String, ChangeFeedEntry)>>,
        reads: Mutex<usize>,
        unreachable: Mutex<bool>,
    }

    impl FeedSource {
        fn collection(&self, name: &str) {
            self.collections
                .lock()
                .expect("lock")
                .insert(name.to_owned());
        }

        fn change(
            &self,
            collection: &str,
            document_id: &str,
            previous_revision: Option<&str>,
            deleted: bool,
        ) -> u64 {
            let mut entries = self.entries.lock().expect("lock");
            let position = entries.len() as u64 + 1;
            entries.push((
                collection.to_owned(),
                ChangeFeedEntry {
                    document_id: document_id.to_owned(),
                    revision: format!("{position}-rev"),
                    previous_revision: previous_revision.map(str::to_owned),
                    commit_position: position,
                    event: if deleted {
                        ChangeFeedEvent::Delete
                    } else if previous_revision.is_none() {
                        ChangeFeedEvent::Insert
                    } else {
                        ChangeFeedEvent::Update
                    },
                },
            ));
            position
        }
    }

    impl WebhookChangeSource for FeedSource {
        fn read_change_feed(
            &self,
            _tenant: &TenantScope,
            input: &ReadChangeFeedInput,
        ) -> Result<ReadChangeFeedOutput, WebhookSourceError> {
            if *self.unreachable.lock().expect("lock") {
                return Err(WebhookSourceError::Unavailable(None));
            }
            if !self
                .collections
                .lock()
                .expect("lock")
                .contains(&input.collection_id)
            {
                return Err(WebhookSourceError::CollectionNotFound);
            }
            *self.reads.lock().expect("lock") += 1;
            let entries = self.entries.lock().expect("lock");
            let high_water = entries.len() as u64;
            let changes: Vec<ChangeFeedEntry> = entries
                .iter()
                .filter(|(collection, entry)| {
                    *collection == input.collection_id
                        && entry.commit_position > input.after_position
                })
                .map(|(_, entry)| entry.clone())
                .take(input.limit as usize)
                .collect();
            let last = changes.last().map(|entry| entry.commit_position);
            let exhausted = changes.len() < input.limit as usize || last == Some(high_water);
            Ok(ReadChangeFeedOutput {
                scanned_through: if exhausted {
                    high_water
                } else {
                    last.unwrap_or(input.after_position)
                },
                high_water,
                exhausted,
                changes,
            })
        }
    }

    #[derive(Clone)]
    struct Posted {
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    /// Fails the next `fail_remaining` posts, and every post for a document
    /// in `fail_documents`, with `failure`; otherwise answers `status`.
    #[derive(Default)]
    struct SinkTransport {
        posted: Mutex<Vec<Posted>>,
        fail_remaining: Mutex<usize>,
        fail_documents: Mutex<BTreeSet<String>>,
        failure: Mutex<Option<WebhookTransportError>>,
        status: Mutex<Option<u16>>,
    }

    impl SinkTransport {
        fn fail_next(&self, count: usize, failure: WebhookTransportError) {
            *self.fail_remaining.lock().expect("lock") = count;
            *self.failure.lock().expect("lock") = Some(failure);
        }

        fn fail_document(&self, document_id: &str, failure: WebhookTransportError) {
            self.fail_documents
                .lock()
                .expect("lock")
                .insert(document_id.to_owned());
            *self.failure.lock().expect("lock") = Some(failure);
        }

        fn answer(&self, status: u16) {
            *self.status.lock().expect("lock") = Some(status);
        }

        fn posted(&self) -> usize {
            self.posted.lock().expect("lock").len()
        }

        /// A snapshot of everything posted so far, so no lock is held while
        /// a test awaits.
        fn posts(&self) -> Vec<Posted> {
            self.posted.lock().expect("lock").clone()
        }
    }

    impl WebhookTransport for SinkTransport {
        fn post(
            &self,
            url: &Url,
            headers: &[(&str, &str)],
            body: &[u8],
        ) -> Result<u16, WebhookTransportError> {
            let failure = self
                .failure
                .lock()
                .expect("lock")
                .unwrap_or(WebhookTransportError::Timeout);
            let mut remaining = self.fail_remaining.lock().expect("lock");
            if *remaining > 0 {
                *remaining -= 1;
                return Err(failure);
            }
            let document: Value = serde_json::from_slice(body).expect("json body");
            if self
                .fail_documents
                .lock()
                .expect("lock")
                .contains(document["documentId"].as_str().unwrap_or_default())
            {
                return Err(failure);
            }
            self.posted.lock().expect("lock").push(Posted {
                url: url.to_string(),
                headers: headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
                body: body.to_vec(),
            });
            Ok(self.status.lock().expect("lock").unwrap_or(200))
        }
    }

    struct Harness {
        fixture: Fixture,
        source: Arc<FeedSource>,
        transport: Arc<SinkTransport>,
        service: WebhookService,
        worker: WebhookWorker,
    }

    fn harness() -> Harness {
        let fixture = fixture();
        let source = Arc::new(FeedSource::default());
        source.collection("todos");
        source.collection("notes");
        let transport = Arc::new(SinkTransport::default());
        let kv: Arc<dyn KvAdapter> = fixture.adapter.clone();
        let store = WebhookStore::new(kv.clone(), Durability::Memory).expect("store");
        let cipher = DeveloperMailCipher::new(DeveloperMailEncryptionKey::derive(b"webhook key"));
        let service = WebhookService::new(
            store.clone(),
            ProjectStore::new(kv.clone(), Durability::Memory).expect("projects"),
            OrganizationStore::new(kv, Durability::Memory).expect("organizations"),
            fixture.audit.clone(),
            cipher.clone(),
            source.clone(),
            true,
        );
        let worker = WebhookWorker::new(
            store,
            cipher,
            transport.clone(),
            source.clone(),
            WebhookWorkerConfig {
                jitter: false,
                ..WebhookWorkerConfig::default()
            },
        )
        .expect("worker");
        Harness {
            fixture,
            source,
            transport,
            service,
            worker,
        }
    }

    fn subscription(collection: &str, events: &[WebhookEvent]) -> WebhookSubscription {
        WebhookSubscription {
            collection_id: CollectionId::parse(collection).expect("collection"),
            events: events.to_vec(),
        }
    }

    async fn register(harness: &Harness, url: &str) -> WebhookEndpointCreated {
        harness
            .service
            .create(
                &harness.fixture.owner,
                &harness.fixture.tenant,
                NewWebhookEndpoint {
                    url: url.to_owned(),
                    description: Some("orders sync".to_owned()),
                    subscriptions: vec![subscription("todos", &WebhookEvent::ALL)],
                    enabled: true,
                },
                NOW,
            )
            .await
            .expect("registered")
    }

    fn query(
        state: Option<WebhookDeliveryState>,
        cursor: Option<&str>,
        limit: usize,
    ) -> WebhookDeliveryQuery {
        WebhookDeliveryQuery {
            state,
            cursor: cursor.map(str::to_owned),
            limit: NonZeroUsize::new(limit).expect("limit"),
        }
    }

    fn verify(secret: &str, posted: &Posted) -> bool {
        let header = posted
            .headers
            .iter()
            .find(|(name, _)| name == SIGNATURE_HEADER)
            .map(|(_, value)| value.as_str())
            .expect("signature header");
        let (timestamp, provided) = header.split_once(",v1=").expect("t and v1");
        let timestamp: u64 = timestamp
            .strip_prefix("t=")
            .expect("t=")
            .parse()
            .expect("unix seconds");
        let expected = webhook_signature(secret.as_bytes(), timestamp, &posted.body);
        expected.ends_with(provided) && provided.len() == 64
    }

    async fn deliveries(harness: &Harness, id: &WebhookEndpointId) -> Vec<WebhookDeliveryRecord> {
        harness
            .service
            .list_deliveries(
                &harness.fixture.owner,
                &harness.fixture.tenant,
                id,
                &query(None, None, 200),
                NOW,
            )
            .await
            .expect("log")
            .items
    }

    #[test]
    fn a_change_is_delivered_once_with_a_valid_signature() {
        let harness = harness();
        block_on(async {
            // A change before registration is never delivered.
            harness.source.change("todos", "old", None, false);
            let created = register(&harness, "https://hooks.example.test/mako").await;
            let endpoint = created.endpoint.clone();
            assert_eq!(endpoint.state(), WebhookEndpointState::Active);
            assert_eq!(endpoint.secret_version(), 1);
            assert!(created.signing_secret.expose().starts_with("whsec_"));
            assert_eq!(created.signing_secret.expose().len(), 6 + 64);
            assert_eq!(
                harness
                    .worker
                    .store()
                    .cursor(
                        &harness.fixture.tenant,
                        endpoint.id(),
                        &CollectionId::parse("todos").expect("collection")
                    )
                    .await
                    .expect("cursor"),
                Some(1)
            );

            let idle = harness.worker.run_once(NOW + 1).await.expect("idle pass");
            assert_eq!(idle.queued, 0);
            assert_eq!(harness.transport.posted(), 0);

            harness.source.change("todos", "todo-1", None, false);
            harness.source.change("notes", "note-1", None, false);
            let pass = harness.worker.run_once(NOW + 2).await.expect("pass");
            assert_eq!(pass.queued, 1, "only the subscribed collection queues");
            assert_eq!(pass.attempted, 1);
            assert_eq!(pass.delivered, 1);

            let posted = harness.transport.posts();
            assert_eq!(posted.len(), 1);
            let post = &posted[0];
            assert_eq!(post.url, "https://hooks.example.test/mako");
            let header = |name: &str| {
                post.headers
                    .iter()
                    .find(|(header, _)| header == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default()
            };
            assert_eq!(header("content-type"), "application/json");
            assert_eq!(header("user-agent"), "mako-cloud-webhooks/1");
            assert_eq!(header("x-mako-webhook-id"), endpoint.id().as_str());
            assert_eq!(header("x-mako-event"), "insert");
            assert_eq!(header("x-mako-secret-version"), "1");
            assert!(verify(created.signing_secret.expose(), post));
            assert!(!verify("whsec_wrong", post));
            let body: Value = serde_json::from_slice(&post.body).expect("json body");
            assert_eq!(body["event"], "insert");
            assert_eq!(body["collection"], "todos");
            assert_eq!(body["documentId"], "todo-1");
            assert_eq!(body["revision"], "2-rev");
            assert_eq!(body["previousRevision"], Value::Null);
            assert_eq!(body["commitPosition"], 2);
            assert_eq!(body["occurredAt"], "2027-01-15T08:00:02Z");
            assert_eq!(
                body["projectId"],
                harness.fixture.tenant.project_id().as_str()
            );
            assert_eq!(body["redeliveryOf"], Value::Null);
            assert_eq!(
                body["id"].as_str(),
                Some(header("x-mako-delivery-id").as_str())
            );
            // The wire order is the documented order, whatever a parser does.
            let raw = String::from_utf8(post.body.clone()).expect("utf8");
            let positions: Vec<usize> = [
                "id",
                "event",
                "collection",
                "documentId",
                "revision",
                "previousRevision",
                "commitPosition",
                "occurredAt",
                "projectId",
                "environmentId",
                "redeliveryOf",
            ]
            .iter()
            .map(|name| raw.find(&format!("\"{name}\":")).expect(name))
            .collect();
            assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{raw}");
            assert!(raw.starts_with("{\"id\":\"whd_"));

            let log = deliveries(&harness, endpoint.id()).await;
            assert_eq!(log.len(), 1);
            assert_eq!(log[0].state(), WebhookDeliveryState::Delivered);
            assert_eq!(log[0].last_response_status(), Some(200));
            assert_eq!(log[0].attempts(), 1);
            assert_eq!(log[0].delivered_at_unix_seconds(), Some(NOW + 2));

            // Nothing is delivered twice.
            let again = harness.worker.run_once(NOW + 3).await.expect("again");
            assert_eq!(again.attempted, 0);
            assert_eq!(harness.transport.posted(), 1);
        });
    }

    #[test]
    fn an_endpoint_down_for_a_while_receives_every_change_after_recovery_in_order() {
        let harness = harness();
        block_on(async {
            let endpoint = register(&harness, "https://hooks.example.test/mako")
                .await
                .endpoint;
            harness
                .transport
                .fail_next(4, WebhookTransportError::ConnectRefused);
            harness.source.change("todos", "a", None, false);
            harness.source.change("todos", "b", None, false);
            harness.source.change("todos", "a", Some("1-rev"), false);
            let first = harness.worker.run_once(NOW + 10).await.expect("first");
            assert_eq!(first.queued, 3);
            // a#1 fails and blocks a#2; b fails too.
            assert_eq!(first.attempted, 2);
            assert_eq!(first.retried, 2);
            assert_eq!(first.deferred, 1);
            let log = deliveries(&harness, endpoint.id()).await;
            assert!(
                log.iter()
                    .all(|record| record.state() == WebhookDeliveryState::Pending)
            );
            let a1 = log.iter().find(|r| r.commit_position() == 1).expect("a#1");
            assert_eq!(a1.attempts(), 1);
            assert_eq!(a1.last_error(), Some("connect_refused"));
            assert_eq!(a1.next_attempt_at_unix_seconds(), Some(NOW + 10 + 10));
            let a2 = log.iter().find(|r| r.commit_position() == 3).expect("a#2");
            assert_eq!(a2.attempts(), 0, "waits on the earlier delivery");

            // Still down: both retries fail and back off further; a#2 still waits.
            let second = harness.worker.run_once(NOW + 25).await.expect("second");
            assert_eq!(second.attempted, 2);
            assert_eq!(second.retried, 2);
            assert_eq!(second.deferred, 1);
            let log = deliveries(&harness, endpoint.id()).await;
            let a1 = log.iter().find(|r| r.commit_position() == 1).expect("a#1");
            assert_eq!(a1.attempts(), 2);
            assert_eq!(a1.next_attempt_at_unix_seconds(), Some(NOW + 25 + 20));
            let early = harness.worker.run_once(NOW + 30).await.expect("early");
            assert_eq!(early.attempted, 0, "nothing is due yet");
            harness.source.change("todos", "a", Some("3-rev"), true);
            // Recovered: everything drains, per document in order.
            let third = harness.worker.run_once(NOW + 2_000).await.expect("third");
            assert_eq!(third.queued, 1);
            assert_eq!(third.delivered, 4);
            let posted = harness.transport.posts();
            let order: Vec<(String, u64)> = posted
                .iter()
                .map(|post| {
                    let body: Value = serde_json::from_slice(&post.body).expect("json");
                    (
                        body["documentId"].as_str().expect("document").to_owned(),
                        body["commitPosition"].as_u64().expect("position"),
                    )
                })
                .collect();
            let a_positions: Vec<u64> = order
                .iter()
                .filter(|(d, _)| d == "a")
                .map(|(_, p)| *p)
                .collect();
            assert_eq!(a_positions, [1, 3, 4]);
            assert!(order.iter().any(|(d, p)| d == "b" && *p == 2));
            let log = deliveries(&harness, endpoint.id()).await;
            assert_eq!(log.len(), 4);
            assert!(
                log.iter()
                    .all(|record| record.state() == WebhookDeliveryState::Delivered)
            );
            let a1 = log.iter().find(|r| r.commit_position() == 1).expect("a#1");
            assert_eq!(a1.attempts(), 3, "the log shows the retries");
            assert!(a1.last_error().is_none());
            let endpoint = harness
                .service
                .get(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    NOW,
                )
                .await
                .expect("endpoint");
            assert_eq!(endpoint.consecutive_failures(), 0);
        });
    }

    #[test]
    fn sustained_failure_pauses_the_endpoint_and_resume_continues() {
        let harness = harness();
        block_on(async {
            let endpoint = register(&harness, "https://hooks.example.test/mako")
                .await
                .endpoint;
            harness.transport.answer(503);
            for index in 0..25 {
                harness
                    .source
                    .change("todos", &format!("doc-{index}"), None, false);
            }
            let mut now = NOW + 10;
            let mut paused_at = None;
            for _ in 0..10 {
                let report = harness.worker.run_once(now).await.expect("pass");
                if report.paused > 0 {
                    paused_at = Some(now);
                    break;
                }
                now += 1_000;
            }
            let paused_at = paused_at.expect("the endpoint pauses");
            let record = harness
                .service
                .get(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    NOW,
                )
                .await
                .expect("endpoint");
            assert_eq!(record.state(), WebhookEndpointState::Paused);
            assert_eq!(record.paused_reason(), Some("sustained_failure"));
            assert_eq!(record.paused_at_unix_seconds(), Some(paused_at));
            assert_eq!(record.consecutive_failures(), 20);
            let log = deliveries(&harness, endpoint.id()).await;
            assert_eq!(log.len(), 25);
            assert!(
                log.iter()
                    .all(|r| r.state() == WebhookDeliveryState::Pending)
            );
            assert!(log.iter().any(|r| r.last_error() == Some("status_503")));

            // Paused: nothing is attempted, and redelivery is refused.
            let before = harness.transport.posted();
            let idle = harness.worker.run_once(now + 5_000).await.expect("idle");
            assert_eq!(idle.attempted, 0);
            assert_eq!(harness.transport.posted(), before);
            let failed_one = log.iter().find(|r| r.attempts() > 0).expect("attempted");
            assert!(matches!(
                harness
                    .service
                    .redeliver(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        endpoint.id(),
                        failed_one.id(),
                        now
                    )
                    .await,
                Err(WebhookError::Conflict(_))
            ));

            // Resume with a healthy endpoint: the pending deliveries continue.
            harness.transport.answer(200);
            let resumed = harness
                .service
                .resume(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    now + 6_000,
                )
                .await
                .expect("resumed");
            assert_eq!(resumed.state(), WebhookEndpointState::Active);
            assert_eq!(resumed.consecutive_failures(), 0);
            let report = harness
                .worker
                .run_once(now + 10_000)
                .await
                .expect("after resume");
            assert_eq!(report.delivered, 25);
            let log = deliveries(&harness, endpoint.id()).await;
            assert!(
                log.iter()
                    .all(|r| r.state() == WebhookDeliveryState::Delivered)
            );
        });
    }

    #[test]
    fn redelivery_logs_a_new_delivery_and_lists_newest_first() {
        let harness = harness();
        block_on(async {
            let endpoint = register(&harness, "https://hooks.example.test/mako")
                .await
                .endpoint;
            harness.source.change("todos", "x", None, false);
            harness.worker.run_once(NOW + 1).await.expect("deliver");
            let original = deliveries(&harness, endpoint.id()).await.remove(0);
            assert_eq!(original.state(), WebhookDeliveryState::Delivered);

            let redelivery = harness
                .service
                .redeliver(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    original.id(),
                    NOW + 50,
                )
                .await
                .expect("queued");
            assert_eq!(redelivery.state(), WebhookDeliveryState::Pending);
            assert_eq!(redelivery.redelivery_of(), Some(original.id()));
            assert_eq!(redelivery.commit_position(), original.commit_position());
            assert_eq!(
                redelivery.occurred_at_unix_seconds(),
                original.occurred_at_unix_seconds()
            );
            let report = harness.worker.run_once(NOW + 51).await.expect("redeliver");
            assert_eq!(report.delivered, 1);
            let posted = harness.transport.posts();
            let body: Value = serde_json::from_slice(&posted[1].body).expect("json");
            assert_eq!(body["redeliveryOf"], original.id().as_str());
            assert_eq!(body["id"], redelivery.id().as_str());

            let log = deliveries(&harness, endpoint.id()).await;
            assert_eq!(log.len(), 2);
            assert_eq!(log[0].id(), redelivery.id(), "newest first");
            assert_eq!(log[1].id(), original.id());
            assert_eq!(log[0].state(), WebhookDeliveryState::Delivered);

            // Paging and the state filter.
            let page = harness
                .service
                .list_deliveries(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    &query(None, None, 1),
                    NOW,
                )
                .await
                .expect("page");
            assert_eq!(page.items.len(), 1);
            let cursor = page.next_cursor.expect("more");
            let rest = harness
                .service
                .list_deliveries(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    &query(None, Some(&cursor), 1),
                    NOW,
                )
                .await
                .expect("rest");
            assert_eq!(rest.items.len(), 1);
            assert_eq!(rest.items[0].id(), original.id());
            assert!(rest.next_cursor.is_none());
            let pending = harness
                .service
                .list_deliveries(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    &query(Some(WebhookDeliveryState::Pending), None, 10),
                    NOW,
                )
                .await
                .expect("filtered");
            assert!(pending.items.is_empty());
            assert!(matches!(
                harness
                    .service
                    .list_deliveries(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        endpoint.id(),
                        &query(None, Some("not a cursor"), 10),
                        NOW,
                    )
                    .await,
                Err(WebhookError::InvalidRequest(_))
            ));
            let missing = WebhookDeliveryId::parse("whd_doesnotexist0000").expect("id");
            assert!(matches!(
                harness
                    .service
                    .redeliver(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        endpoint.id(),
                        &missing,
                        NOW
                    )
                    .await,
                Err(WebhookError::DeliveryNotFound)
            ));
        });
    }

    #[test]
    fn the_retry_window_fails_a_delivery_and_retention_prunes_the_log() {
        let harness = harness();
        block_on(async {
            let endpoint = register(&harness, "https://hooks.example.test/mako")
                .await
                .endpoint;
            // One document the endpoint always rejects, among others it
            // accepts: the successes keep the endpoint from pausing, so the
            // rejected delivery runs out its whole retry window.
            harness
                .transport
                .fail_document("slow", WebhookTransportError::Timeout);
            harness.source.change("todos", "slow", None, false);
            let mut now = NOW + 1;
            harness.worker.run_once(now).await.expect("first attempt");
            let first_attempt = now;
            let mut failed = false;
            let mut passes = 0;
            for index in 0..400 {
                now += 900;
                passes += 1;
                harness
                    .source
                    .change("todos", &format!("ok-{index}"), None, false);
                let report = harness.worker.run_once(now).await.expect("pass");
                assert_eq!(report.delivered, 1, "the healthy document still delivers");
                assert_eq!(report.paused, 0);
                if report.failed > 0 {
                    failed = true;
                    break;
                }
            }
            assert!(failed, "the delivery fails once the window closes");
            let log = deliveries(&harness, endpoint.id()).await;
            let slow = log
                .iter()
                .find(|r| r.document_id() == "slow")
                .expect("slow");
            assert_eq!(slow.state(), WebhookDeliveryState::Failed);
            assert_eq!(slow.last_error(), Some("timeout"));
            assert!(slow.next_attempt_at_unix_seconds().is_none());
            assert!(now - first_attempt >= RETRY_WINDOW_SECONDS);
            assert!(now - first_attempt < RETRY_WINDOW_SECONDS + 2 * 900);
            // Backoff capped at fifteen minutes: roughly one attempt per pass.
            assert!(
                slow.attempts() >= 90 && slow.attempts() <= 110,
                "{}",
                slow.attempts()
            );
            let endpoint_record = harness
                .service
                .get(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    NOW,
                )
                .await
                .expect("endpoint");
            assert_eq!(endpoint_record.state(), WebhookEndpointState::Active);

            // Everything ages out after the retention window.
            let later = now + DELIVERY_RETENTION_SECONDS;
            let report = harness.worker.run_once(later).await.expect("retention");
            assert_eq!(report.cleaned, passes + 1);
            assert!(deliveries(&harness, endpoint.id()).await.is_empty());

            // The ceiling: more terminal deliveries than the maximum drop the oldest.
            let worker = WebhookWorker::new(
                harness.worker.store().clone(),
                DeveloperMailCipher::new(DeveloperMailEncryptionKey::derive(b"webhook key")),
                harness.transport.clone(),
                harness.source.clone(),
                WebhookWorkerConfig {
                    jitter: false,
                    maximum_deliveries_per_endpoint: 3,
                    ..WebhookWorkerConfig::default()
                },
            )
            .expect("worker");
            for index in 0..5 {
                harness
                    .source
                    .change("todos", &format!("d{index}"), None, false);
            }
            let report = worker.run_once(later + 1).await.expect("deliver five");
            assert_eq!(report.delivered, 5);
            assert_eq!(report.cleaned, 2, "two beyond the ceiling of three");
            let log = deliveries(&harness, endpoint.id()).await;
            assert_eq!(log.len(), 3);
            assert!(
                log.iter()
                    .all(|r| r.document_id() != "d0" && r.document_id() != "d1")
            );
        });
    }

    #[test]
    fn disabling_stops_intake_and_re_enabling_abandons_pending_deliveries() {
        let harness = harness();
        block_on(async {
            let endpoint = register(&harness, "https://hooks.example.test/mako")
                .await
                .endpoint;
            harness
                .transport
                .fail_next(usize::MAX, WebhookTransportError::Tls);
            harness.source.change("todos", "p", None, false);
            harness
                .worker
                .run_once(NOW + 1)
                .await
                .expect("queue and fail");
            let disabled = harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    WebhookEndpointUpdate {
                        enabled: Some(false),
                        ..WebhookEndpointUpdate::default()
                    },
                    NOW + 2,
                )
                .await
                .expect("disabled");
            assert_eq!(disabled.state(), WebhookEndpointState::Disabled);
            harness.source.change("todos", "q", None, false);
            let report = harness
                .worker
                .run_once(NOW + 100)
                .await
                .expect("disabled pass");
            assert_eq!(report.queued, 0);
            assert_eq!(report.attempted, 0);
            assert_eq!(deliveries(&harness, endpoint.id()).await.len(), 1);

            *harness.transport.fail_remaining.lock().expect("lock") = 0;
            let enabled = harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    WebhookEndpointUpdate {
                        enabled: Some(true),
                        ..WebhookEndpointUpdate::default()
                    },
                    NOW + 200,
                )
                .await
                .expect("enabled");
            assert_eq!(enabled.state(), WebhookEndpointState::Active);
            let log = deliveries(&harness, endpoint.id()).await;
            assert_eq!(log[0].state(), WebhookDeliveryState::Failed);
            assert_eq!(log[0].last_error(), Some("disabled"));
            // The cursor moved to the current high water: q is not delivered.
            harness.source.change("todos", "r", None, false);
            let report = harness
                .worker
                .run_once(NOW + 300)
                .await
                .expect("after enable");
            assert_eq!(report.queued, 1);
            assert_eq!(report.delivered, 1);
            let posted = harness.transport.posts();
            let body: Value = serde_json::from_slice(&posted[0].body).expect("json");
            assert_eq!(body["documentId"], "r");
        });
    }

    #[test]
    fn subscriptions_change_cursors_and_rotation_changes_the_signing_key() {
        let harness = harness();
        block_on(async {
            let created = register(&harness, "https://hooks.example.test/mako").await;
            let endpoint = created.endpoint;
            harness.source.change("notes", "n0", None, false);
            let updated = harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    WebhookEndpointUpdate {
                        subscriptions: Some(vec![subscription("notes", &[WebhookEvent::Insert])]),
                        description: Some("notes only".to_owned()),
                        ..WebhookEndpointUpdate::default()
                    },
                    NOW + 1,
                )
                .await
                .expect("updated");
            assert_eq!(updated.subscriptions().len(), 1);
            assert_eq!(updated.description(), "notes only");
            let store = harness.worker.store();
            assert!(
                store
                    .cursor(
                        &harness.fixture.tenant,
                        endpoint.id(),
                        &CollectionId::parse("todos").expect("id")
                    )
                    .await
                    .expect("cursor")
                    .is_none(),
                "the removed subscription drops its cursor"
            );
            assert_eq!(
                store
                    .cursor(
                        &harness.fixture.tenant,
                        endpoint.id(),
                        &CollectionId::parse("notes").expect("id")
                    )
                    .await
                    .expect("cursor"),
                Some(1),
                "the new subscription starts at the current high water"
            );
            harness.source.change("notes", "n1", None, false);
            harness.source.change("notes", "n1", Some("2-rev"), false);
            harness.source.change("todos", "t", None, false);
            let report = harness.worker.run_once(NOW + 2).await.expect("pass");
            assert_eq!(report.queued, 1, "only inserts on notes");

            let rotated = harness
                .service
                .rotate_secret(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    endpoint.id(),
                    NOW + 3,
                )
                .await
                .expect("rotated");
            assert_eq!(rotated.endpoint.secret_version(), 2);
            assert_ne!(
                rotated.signing_secret.expose(),
                created.signing_secret.expose()
            );
            harness.source.change("notes", "n2", None, false);
            harness.worker.run_once(NOW + 4).await.expect("pass");
            let posted = harness.transport.posts();
            assert_eq!(posted.len(), 2);
            assert!(verify(created.signing_secret.expose(), &posted[0]));
            assert!(verify(rotated.signing_secret.expose(), &posted[1]));
            assert!(!verify(created.signing_secret.expose(), &posted[1]));
            assert!(
                posted[1]
                    .headers
                    .iter()
                    .any(|(n, v)| n == "x-mako-secret-version" && v == "2")
            );
        });
    }

    #[test]
    fn registration_validates_urls_subscriptions_and_collections() {
        let harness = harness();
        block_on(async {
            let create = |url: &str, subscriptions: Vec<WebhookSubscription>| {
                harness.service.create(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    NewWebhookEndpoint {
                        url: url.to_owned(),
                        description: None,
                        subscriptions,
                        enabled: true,
                    },
                    NOW,
                )
            };
            let todos = || vec![subscription("todos", &[WebhookEvent::Insert])];
            for url in [
                "http://hooks.example.test/mako",
                "ftp://hooks.example.test/mako",
                "https://user:pw@hooks.example.test/mako",
                "not a url",
                "",
                "https://",
            ] {
                assert!(
                    matches!(
                        create(url, todos()).await,
                        Err(WebhookError::InvalidRequest(_))
                    ),
                    "{url:?} must be refused"
                );
            }
            assert!(create("http://127.0.0.1:8099/hook", todos()).await.is_ok());
            assert!(create("http://localhost:8099/hook", todos()).await.is_ok());
            let long = format!("https://hooks.example.test/{}", "a".repeat(2_048));
            assert!(matches!(
                create(&long, todos()).await,
                Err(WebhookError::InvalidRequest(_))
            ));

            assert!(matches!(
                create("https://hooks.example.test/x", Vec::new()).await,
                Err(WebhookError::InvalidRequest(_))
            ));
            assert!(matches!(
                create(
                    "https://hooks.example.test/x",
                    vec![subscription("todos", &[])]
                )
                .await,
                Err(WebhookError::InvalidRequest(_))
            ));
            assert!(matches!(
                create(
                    "https://hooks.example.test/x",
                    vec![subscription(
                        "todos",
                        &[WebhookEvent::Insert, WebhookEvent::Insert]
                    )]
                )
                .await,
                Err(WebhookError::InvalidRequest(_))
            ));
            assert!(matches!(
                create(
                    "https://hooks.example.test/x",
                    vec![
                        subscription("todos", &[WebhookEvent::Insert]),
                        subscription("todos", &[WebhookEvent::Delete])
                    ]
                )
                .await,
                Err(WebhookError::InvalidRequest(_))
            ));
            let many: Vec<WebhookSubscription> = (0..33)
                .map(|i| subscription(&format!("c{i}"), &[WebhookEvent::Insert]))
                .collect();
            assert!(matches!(
                create("https://hooks.example.test/x", many).await,
                Err(WebhookError::InvalidRequest(_))
            ));
            assert!(matches!(
                create("https://hooks.example.test/x", vec![subscription("missing", &[WebhookEvent::Insert])]).await,
                Err(WebhookError::CollectionNotFound(name)) if name == "missing"
            ));
            *harness.source.unreachable.lock().expect("lock") = true;
            assert!(matches!(
                create("https://hooks.example.test/x", todos()).await,
                Err(WebhookError::Source(_))
            ));
            *harness.source.unreachable.lock().expect("lock") = false;

            // A production deployment admits no plain HTTP at all.
            let strict = WebhookService::new(
                harness.worker.store().clone(),
                ProjectStore::new(harness.fixture.adapter.clone(), Durability::Memory)
                    .expect("projects"),
                OrganizationStore::new(harness.fixture.adapter.clone(), Durability::Memory)
                    .expect("organizations"),
                harness.fixture.audit.clone(),
                DeveloperMailCipher::new(DeveloperMailEncryptionKey::derive(b"webhook key")),
                harness.source.clone(),
                false,
            );
            assert!(matches!(
                strict
                    .create(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        NewWebhookEndpoint {
                            url: "http://127.0.0.1:8099/hook".to_owned(),
                            description: None,
                            subscriptions: todos(),
                            enabled: true,
                        },
                        NOW,
                    )
                    .await,
                Err(WebhookError::InvalidRequest(_))
            ));
        });
    }

    #[test]
    fn membership_decides_who_reads_and_who_writes_and_mutations_are_audited() {
        let harness = harness();
        block_on(async {
            let created = register(&harness, "https://hooks.example.test/mako").await;
            let listed = harness
                .service
                .list(&harness.fixture.viewer, &harness.fixture.tenant, NOW)
                .await
                .expect("a viewer reads");
            assert_eq!(listed.len(), 1);
            assert!(matches!(
                harness
                    .service
                    .create(
                        &harness.fixture.viewer,
                        &harness.fixture.tenant,
                        NewWebhookEndpoint {
                            url: "https://hooks.example.test/other".to_owned(),
                            description: None,
                            subscriptions: vec![subscription("todos", &[WebhookEvent::Insert])],
                            enabled: true,
                        },
                        NOW,
                    )
                    .await,
                Err(WebhookError::Forbidden)
            ));
            assert!(matches!(
                harness
                    .service
                    .list(&harness.fixture.stranger, &harness.fixture.tenant, NOW)
                    .await,
                Err(WebhookError::Forbidden)
            ));
            let elsewhere = TenantScope::new(
                harness.fixture.tenant.project_id().clone(),
                mako_api::EnvironmentId::parse("env_doesnotexist1").expect("environment"),
            );
            assert!(matches!(
                harness
                    .service
                    .list(&harness.fixture.owner, &elsewhere, NOW)
                    .await,
                Err(WebhookError::EnvironmentNotFound)
            ));
            let missing = WebhookEndpointId::parse("whk_doesnotexist0000").expect("id");
            assert!(matches!(
                harness
                    .service
                    .get(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &missing,
                        NOW
                    )
                    .await,
                Err(WebhookError::EndpointNotFound)
            ));

            harness
                .service
                .delete(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    created.endpoint.id(),
                    NOW + 1,
                )
                .await
                .expect("deleted");
            assert!(
                harness
                    .service
                    .list(&harness.fixture.owner, &harness.fixture.tenant, NOW)
                    .await
                    .expect("list")
                    .is_empty()
            );
            assert!(
                harness
                    .worker
                    .store()
                    .registry()
                    .await
                    .expect("registry")
                    .is_empty()
            );
            let actions: Vec<&'static str> = harness
                .fixture
                .audit
                .0
                .lock()
                .expect("audit")
                .iter()
                .map(|event| event.action.as_str())
                .collect();
            assert!(actions.contains(&"webhook_endpoint_create"));
            assert!(actions.contains(&"webhook_endpoint_delete"));
            let denied = harness
                .fixture
                .audit
                .0
                .lock()
                .expect("audit")
                .iter()
                .filter(|event| event.outcome == ControlAuditOutcome::Denied)
                .count();
            assert_eq!(denied, 2);
        });
    }

    #[test]
    fn secrets_never_appear_in_debug_output_and_ids_follow_the_public_pattern() {
        let harness = harness();
        block_on(async {
            let created = register(&harness, "https://hooks.example.test/mako").await;
            let secret = created.signing_secret.expose().to_owned();
            let hex = &secret["whsec_".len()..];
            for rendered in [
                format!("{created:?}"),
                format!("{:?}", created.endpoint),
                format!("{:?}", created.signing_secret),
                format!("{:?}", harness.service),
                format!("{:?}", harness.worker),
                format!("{:?}", harness.worker.store()),
            ] {
                assert!(!rendered.contains(&secret), "{rendered}");
                assert!(!rendered.contains(hex), "{rendered}");
            }
            let stored = harness
                .fixture
                .adapter
                .get(
                    &ControlKeyspace::webhook_endpoint_key(
                        harness.fixture.tenant.project_id(),
                        harness.fixture.tenant.environment_id(),
                        created.endpoint.id().as_str(),
                    )
                    .expect("key"),
                )
                .await
                .expect("read")
                .expect("record");
            assert!(!String::from_utf8_lossy(&stored).contains(hex));
        });
        for id in [
            "whk_abcdefghijklmnop",
            "whk_abcdefghijkl",
            &format!("whk_{}", "a".repeat(32)),
        ] {
            assert!(WebhookEndpointId::parse(id).is_ok(), "{id}");
        }
        for id in [
            "whd_abcdefghijklmnop",
            "whk_ABCDEFGHIJKLMNOP",
            "whk_short",
            "",
            "whk_",
        ] {
            assert!(WebhookEndpointId::parse(id).is_err(), "{id}");
        }
        let generated = WebhookDeliveryId::generate();
        assert!(WebhookDeliveryId::parse(generated.as_str()).is_ok());
        assert_eq!(generated.as_str().len(), 4 + 16);
        assert_ne!(WebhookEndpointId::generate(), WebhookEndpointId::generate());
    }

    #[test]
    fn signatures_and_backoff_follow_the_documented_scheme() {
        let signature = webhook_signature(b"whsec_test", 1_700_000_000, br#"{"id":"whd_x"}"#);
        assert!(signature.starts_with("t=1700000000,v1="));
        assert_eq!(signature.len(), "t=1700000000,v1=".len() + 64);
        assert!(
            signature[signature.len() - 64..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_test").expect("mac");
        mac.update(br#"1700000000.{"id":"whd_x"}"#);
        let expected: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert!(signature.ends_with(&expected));

        let config = WebhookWorkerConfig {
            jitter: false,
            ..WebhookWorkerConfig::default()
        };
        assert_eq!(config.backoff(1), 10);
        assert_eq!(config.backoff(2), 20);
        assert_eq!(config.backoff(3), 40);
        assert_eq!(config.backoff(7), 640);
        assert_eq!(config.backoff(8), 900);
        assert_eq!(config.backoff(40), 900);
        let jittered = WebhookWorkerConfig::default();
        for _ in 0..50 {
            let backoff = jittered.backoff(1);
            assert!((10..=12).contains(&backoff), "{backoff}");
        }
        assert!(
            WebhookWorkerConfig {
                intake_page: 0,
                ..WebhookWorkerConfig::default()
            }
            .validate()
            .is_err()
        );
    }
}
