//! Scheduled functions: cron-driven invocations of a function's active
//! deployment, with a run history.
//!
//! A developer attaches a *schedule* -- a five-field cron expression in UTC
//! and the request to send -- to a deployed function. A control-plane worker
//! walks a due index every few seconds and, for each schedule whose next run
//! has fallen due, records a *run* and invokes the function through the edge
//! gateway, so admission, metering, metrics, logs, and audit apply exactly as
//! they do to any other invocation.
//!
//! Two rules keep a slow function from piling up:
//!
//! - **Overlap is skipped, never run concurrently.** A running invocation
//!   holds a per-schedule *lease* until it completes (or the lease expires,
//!   which bounds a crash mid-run). A due time that arrives while the lease
//!   is held is recorded as `skipped_overlap` and the schedule moves on.
//! - **The next due time advances before the invocation starts.** A run that
//!   takes longer than its interval therefore never re-fires its own due
//!   time; the following due time is what gets skipped.
//!
//! Due times missed while the worker was down are collapsed: the schedule
//! runs **once**, for the latest missed due time, and the earlier slots are
//! not recorded at all. A backlog of `* * * * *` over a weekend is one run,
//! not three thousand.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use mako_api::TenantScope;
use mako_internal_rpc::{
    ControlToEdgeClient, FunctionScheduleInvokeRequest, FunctionScheduleInvokeResponse,
    InternalClientError,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KeyValue, KvAdapter,
    ScanDirection, ScanRequest, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, CronError, CronSchedule, DeveloperPrincipal, FunctionName,
    FunctionRecord, FunctionState, OrganizationId, OrganizationStore, OrganizationStoreError,
    ProjectStore, ProjectStoreError,
};

pub const MAXIMUM_NAME_BYTES: usize = 128;
pub const MAXIMUM_PATH_BYTES: usize = 1_024;
pub const MAXIMUM_HEADERS: usize = 16;
pub const MAXIMUM_HEADER_NAME_BYTES: usize = 128;
pub const MAXIMUM_HEADER_VALUE_BYTES: usize = 1_024;
pub const MAXIMUM_CONTENT_TYPE_BYTES: usize = 128;
pub const MAXIMUM_BODY_BYTES: usize = 64 * 1024;
/// A function may carry at most this many schedules.
pub const MAXIMUM_SCHEDULES_PER_FUNCTION: usize = 100;
/// The gateway has this long to answer a scheduled invocation.
pub const INVOCATION_TIMEOUT_SECONDS: u64 = 60;
/// A lease outlives the timeout by this much before an abandoned run (a
/// worker that died mid-invocation) stops blocking the schedule.
pub const LEASE_GRACE_SECONDS: u64 = 30;
/// Runs are kept this long...
pub const RUN_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;
/// ...and never more than this many per schedule, oldest first.
pub const MAXIMUM_RUNS_PER_SCHEDULE: usize = 1_000;
/// Due schedules started in one pass; the rest wait for the next.
pub const RUNS_PER_PASS: usize = 20;
pub const USER_AGENT: &str = "mako-cloud-scheduler/1";
/// Headers a scheduled invocation carries so the function can tell.
pub const SCHEDULE_ID_HEADER: &str = "x-mako-schedule-id";
pub const SCHEDULE_RUN_ID_HEADER: &str = "x-mako-schedule-run-id";
pub const SCHEDULE_DUE_AT_HEADER: &str = "x-mako-schedule-due-at";
/// Stable `error` reasons on a run that could not complete.
pub const ERROR_TIMEOUT: &str = "timeout";
pub const ERROR_NO_ACTIVE_DEPLOYMENT: &str = "no_active_deployment";
pub const ERROR_GATEWAY_UNAVAILABLE: &str = "gateway_unavailable";
pub const ERROR_INVALID_REQUEST: &str = "invalid_request";
pub const ERROR_THROTTLED: &str = "throttled";

const ID_SUFFIX_LENGTH: usize = 16;
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const DUE_KIND_CRON: &str = "cron";
/// Every scan is bounded; a saturated scan fails closed.
const MAXIMUM_REGISTRY_SCAN: usize = 10_000;
const MAXIMUM_DUE_SCAN: usize = 1_000;
const MAXIMUM_SCHEDULE_SCAN: usize = MAXIMUM_SCHEDULES_PER_FUNCTION + 100;
/// The history scan behind a listing: the retention ceiling plus what may
/// accumulate before retention runs again.
const MAXIMUM_HISTORY_SCAN: usize = MAXIMUM_RUNS_PER_SCHEDULE + 500;
const RETENTION_BATCH: usize = 200;
const REMOVAL_BATCH: usize = 500;
const MAXIMUM_REMOVAL_ROUNDS: usize = 40;
/// Catching up a schedule that was down walks at most this many due times
/// forward before taking the last one it reached as the missed due time.
const MAXIMUM_CATCH_UP_STEPS: usize = 100_000;
const MAXIMUM_CONCURRENT_RETRIES: usize = 4;
const REFUSED_HEADERS: [&str; 5] = [
    "authorization",
    "host",
    "content-length",
    "content-type",
    "transfer-encoding",
];

macro_rules! schedule_id {
    ($name:ident, $prefix:literal, $label:literal) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Accepts the public pattern `^` $prefix `[a-z0-9]{12,32}$`.
            pub fn parse(value: impl Into<String>) -> Result<Self, FunctionScheduleError> {
                let value = value.into();
                let Some(suffix) = value.strip_prefix($prefix) else {
                    return Err(FunctionScheduleError::InvalidRequest(concat!(
                        $label,
                        " id is invalid"
                    )));
                };
                if !(12..=32).contains(&suffix.len())
                    || !suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                {
                    return Err(FunctionScheduleError::InvalidRequest(concat!(
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

schedule_id!(FunctionScheduleId, "sch_", "function schedule");
schedule_id!(FunctionScheduleRunId, "run_", "function schedule run");

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
#[serde(rename_all = "UPPERCASE")]
pub enum FunctionScheduleMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl FunctionScheduleMethod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// The request each due time sends. Mirrors the public
/// `FunctionScheduleRequest`; the same shape is stored.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionScheduleRequest {
    #[serde(default = "default_method")]
    pub method: FunctionScheduleMethod,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "default_content_type")]
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

const fn default_method() -> FunctionScheduleMethod {
    FunctionScheduleMethod::Post
}

fn default_path() -> String {
    "/".to_owned()
}

fn default_content_type() -> String {
    "application/json".to_owned()
}

impl Default for FunctionScheduleRequest {
    fn default() -> Self {
        Self {
            method: default_method(),
            path: default_path(),
            headers: BTreeMap::new(),
            content_type: default_content_type(),
            body: None,
        }
    }
}

impl FunctionScheduleRequest {
    /// Checks every field against the public schema and the gateway's own
    /// rules, lowercasing header names so refusals cannot be spelled around.
    fn validate(&mut self) -> Result<(), FunctionScheduleError> {
        let invalid = FunctionScheduleError::InvalidRequest;
        if !self.path.starts_with('/')
            || self.path.len() > MAXIMUM_PATH_BYTES
            || self.path.chars().any(char::is_control)
            || self.path.contains(['#', '\\'])
        {
            return Err(invalid(
                "request path must begin with `/`, be at most 1024 bytes, and carry no control characters, `#`, or `\\`",
            ));
        }
        let (path, _query) = split_path_query(&self.path);
        if path != "/"
            && path[1..]
                .split('/')
                .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        {
            return Err(invalid(
                "request path segments must be non-empty and neither `.` nor `..`",
            ));
        }
        if self.headers.len() > MAXIMUM_HEADERS {
            return Err(invalid("request headers must number at most 16"));
        }
        let mut headers = BTreeMap::new();
        for (name, value) in std::mem::take(&mut self.headers) {
            let name = name.to_ascii_lowercase();
            if name.is_empty()
                || name.len() > MAXIMUM_HEADER_NAME_BYTES
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(invalid(
                    "request header names must be ASCII letters, digits, `-`, or `_`",
                ));
            }
            if REFUSED_HEADERS.contains(&name.as_str()) || name.starts_with("x-mako-") {
                return Err(invalid(
                    "request headers may not set authorization, host, content-length, content-type, transfer-encoding, or x-mako-* headers",
                ));
            }
            if value.len() > MAXIMUM_HEADER_VALUE_BYTES || value.chars().any(char::is_control) {
                return Err(invalid(
                    "request header values must be at most 1024 bytes without control characters",
                ));
            }
            if headers.insert(name, value).is_some() {
                return Err(invalid("request headers must not repeat a name"));
            }
        }
        self.headers = headers;
        if self.content_type.is_empty()
            || self.content_type.len() > MAXIMUM_CONTENT_TYPE_BYTES
            || self.content_type.chars().any(char::is_control)
            || !self.content_type.contains('/')
        {
            return Err(invalid(
                "request contentType must be a media type of at most 128 bytes",
            ));
        }
        if let Some(body) = &self.body {
            if body.len() > MAXIMUM_BODY_BYTES {
                return Err(invalid("request body must be at most 65536 bytes"));
            }
            if self.method == FunctionScheduleMethod::Get {
                return Err(invalid("a GET request carries no body"));
            }
        }
        Ok(())
    }
}

/// Splits a configured path at its first `?`.
fn split_path_query(path: &str) -> (&str, Option<&str>) {
    match path.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path, None),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FunctionScheduleState {
    Active,
    Paused,
}

impl FunctionScheduleState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionScheduleRunOutcome {
    Succeeded,
    Failed,
    Error,
    SkippedOverlap,
}

impl FunctionScheduleRunOutcome {
    pub const ALL: [Self; 4] = [
        Self::Succeeded,
        Self::Failed,
        Self::Error,
        Self::SkippedOverlap,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::SkippedOverlap => "skipped_overlap",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == value)
    }
}

/// The last run a schedule shows next to its next one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionScheduleRunSummary {
    pub id: FunctionScheduleRunId,
    pub due_at_unix_seconds: u64,
    pub outcome: FunctionScheduleRunOutcome,
    pub duration_milliseconds: Option<u64>,
    pub response_status: Option<u16>,
}

/// A schedule attached to a function. Mirrors the public `FunctionSchedule`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionScheduleRecord {
    id: FunctionScheduleId,
    tenant: TenantScope,
    function_name: FunctionName,
    name: String,
    cron: String,
    request: FunctionScheduleRequest,
    enabled: bool,
    next_run_at_unix_seconds: Option<u64>,
    last_run: Option<FunctionScheduleRunSummary>,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

impl FunctionScheduleRecord {
    #[must_use]
    pub fn id(&self) -> &FunctionScheduleId {
        &self.id
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn function_name(&self) -> &FunctionName {
        &self.function_name
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn cron(&self) -> &str {
        &self.cron
    }

    #[must_use]
    pub fn request(&self) -> &FunctionScheduleRequest {
        &self.request
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn state(&self) -> FunctionScheduleState {
        if self.enabled {
            FunctionScheduleState::Active
        } else {
            FunctionScheduleState::Paused
        }
    }

    /// Null while paused.
    #[must_use]
    pub const fn next_run_at_unix_seconds(&self) -> Option<u64> {
        self.next_run_at_unix_seconds
    }

    #[must_use]
    pub const fn last_run(&self) -> Option<&FunctionScheduleRunSummary> {
        self.last_run.as_ref()
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    /// The due-index entry name for the cron due time, while enabled.
    fn due_item(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        self.next_run_at_unix_seconds
            .map(|at| due_item(at, &self.id, DUE_KIND_CRON))
    }
}

fn due_item(at_unix_seconds: u64, id: &FunctionScheduleId, kind: &str) -> String {
    format!("{at_unix_seconds:020}/{}/{kind}", id.as_str())
}

/// What a registration supplies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewFunctionSchedule {
    pub name: Option<String>,
    pub cron: String,
    pub request: Option<FunctionScheduleRequest>,
    pub enabled: bool,
}

/// Fields omitted keep their values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionScheduleUpdate {
    pub name: Option<String>,
    pub cron: Option<String>,
    pub request: Option<FunctionScheduleRequest>,
    pub enabled: Option<bool>,
}

impl FunctionScheduleUpdate {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.cron.is_none()
            && self.request.is_none()
            && self.enabled.is_none()
    }
}

/// One run in a schedule's history. Mirrors the public `FunctionScheduleRun`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionScheduleRunRecord {
    id: FunctionScheduleRunId,
    schedule_id: FunctionScheduleId,
    tenant: TenantScope,
    function_name: FunctionName,
    function_version: Option<u64>,
    due_at_unix_seconds: u64,
    started_at_unix_seconds: Option<u64>,
    completed_at_unix_seconds: Option<u64>,
    duration_milliseconds: Option<u64>,
    outcome: Option<FunctionScheduleRunOutcome>,
    response_status: Option<u16>,
    error: Option<String>,
    manual: bool,
    created_at_unix_seconds: u64,
}

impl FunctionScheduleRunRecord {
    #[must_use]
    pub fn id(&self) -> &FunctionScheduleRunId {
        &self.id
    }

    #[must_use]
    pub fn schedule_id(&self) -> &FunctionScheduleId {
        &self.schedule_id
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn function_name(&self) -> &FunctionName {
        &self.function_name
    }

    #[must_use]
    pub const fn function_version(&self) -> Option<u64> {
        self.function_version
    }

    #[must_use]
    pub const fn due_at_unix_seconds(&self) -> u64 {
        self.due_at_unix_seconds
    }

    #[must_use]
    pub const fn started_at_unix_seconds(&self) -> Option<u64> {
        self.started_at_unix_seconds
    }

    #[must_use]
    pub const fn completed_at_unix_seconds(&self) -> Option<u64> {
        self.completed_at_unix_seconds
    }

    #[must_use]
    pub const fn duration_milliseconds(&self) -> Option<u64> {
        self.duration_milliseconds
    }

    /// Null while the run is queued or executing.
    #[must_use]
    pub const fn outcome(&self) -> Option<FunctionScheduleRunOutcome> {
        self.outcome
    }

    #[must_use]
    pub const fn response_status(&self) -> Option<u16> {
        self.response_status
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// True for a run started with run-now rather than by the cron.
    #[must_use]
    pub const fn manual(&self) -> bool {
        self.manual
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    fn new(schedule: &FunctionScheduleRecord, due_at: u64, manual: bool, now: u64) -> Self {
        Self {
            id: FunctionScheduleRunId::generate(),
            schedule_id: schedule.id.clone(),
            tenant: schedule.tenant.clone(),
            function_name: schedule.function_name.clone(),
            function_version: None,
            due_at_unix_seconds: due_at,
            started_at_unix_seconds: None,
            completed_at_unix_seconds: None,
            duration_milliseconds: None,
            outcome: None,
            response_status: None,
            error: None,
            manual,
            created_at_unix_seconds: now,
        }
    }

    fn skipped(schedule: &FunctionScheduleRecord, due_at: u64, now: u64) -> Self {
        let mut run = Self::new(schedule, due_at, false, now);
        run.outcome = Some(FunctionScheduleRunOutcome::SkippedOverlap);
        run
    }

    fn start(&mut self, now: u64) {
        self.started_at_unix_seconds = Some(now);
    }

    fn skip(&mut self) {
        self.outcome = Some(FunctionScheduleRunOutcome::SkippedOverlap);
    }

    fn complete(
        &mut self,
        result: &Result<FunctionScheduleInvokeResponse, FunctionScheduleInvokeError>,
        elapsed_milliseconds: u64,
        now: u64,
    ) {
        self.completed_at_unix_seconds = Some(now);
        self.duration_milliseconds = Some(elapsed_milliseconds);
        match result {
            Ok(response) => {
                self.function_version = Some(response.version);
                self.response_status = Some(response.status);
                self.outcome = Some(if (200..300).contains(&response.status) {
                    FunctionScheduleRunOutcome::Succeeded
                } else {
                    FunctionScheduleRunOutcome::Failed
                });
            }
            Err(error) => {
                self.outcome = Some(FunctionScheduleRunOutcome::Error);
                self.error = Some(error.stable_code().to_owned());
            }
        }
    }

    fn summary(&self) -> Option<FunctionScheduleRunSummary> {
        Some(FunctionScheduleRunSummary {
            id: self.id.clone(),
            due_at_unix_seconds: self.due_at_unix_seconds,
            outcome: self.outcome?,
            duration_milliseconds: self.duration_milliseconds,
            response_status: self.response_status,
        })
    }

    /// The history entry name: reverse-timestamped so a forward scan is
    /// newest first, reverse due time within a second, then the id so
    /// entries never collide.
    fn item(&self) -> String {
        run_item(
            self.created_at_unix_seconds,
            self.due_at_unix_seconds,
            &self.id,
        )
    }
}

fn run_item(
    created_at_unix_seconds: u64,
    due_at_unix_seconds: u64,
    id: &FunctionScheduleRunId,
) -> String {
    format!(
        "{:020}/{:020}/{}",
        u64::MAX - created_at_unix_seconds,
        u64::MAX - due_at_unix_seconds,
        id.as_str()
    )
}

/// How a run history is read: optionally one outcome, from a cursor, at
/// most `limit` entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionScheduleRunQuery {
    pub outcome: Option<FunctionScheduleRunOutcome>,
    pub cursor: Option<String>,
    pub limit: NonZeroUsize,
}

/// A page of a schedule's history, newest first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionScheduleRunPage {
    pub items: Vec<FunctionScheduleRunRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScheduleIndexEntry {
    tenant: TenantScope,
    function_name: FunctionName,
}

/// A due-index value: which schedule, and for a queued manual run, which
/// run (by its history entry name, so no second index is needed).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DueEntry {
    tenant: TenantScope,
    function_name: FunctionName,
    schedule_id: FunctionScheduleId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_item: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LeaseRecord {
    run_id: FunctionScheduleRunId,
    expires_at_unix_seconds: u64,
}

/// A due entry with the key it was read under and the time it fell due.
struct Due {
    key: Vec<u8>,
    at_unix_seconds: u64,
    entry: DueEntry,
}

fn encode_cursor(item: &str) -> String {
    URL_SAFE_NO_PAD.encode(item.as_bytes())
}

fn decode_cursor(cursor: &str) -> Result<String, FunctionScheduleError> {
    let invalid = || FunctionScheduleError::InvalidRequest("cursor is invalid");
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
    FunctionScheduleRunId::parse(id).map_err(|_| invalid())?;
    Ok(item)
}

/// The item segment of a system key: everything after the last domain
/// separator. Index keys carry ids and timestamps only, so the segment is
/// ASCII and the escape byte never occurs inside it.
fn decode_index_item(key: &[u8]) -> Result<String, FunctionScheduleError> {
    // Layout: [version, namespace] domain \0\0 item \0\0
    let body = key
        .strip_suffix(&[0, 0])
        .ok_or(FunctionScheduleError::CorruptRecord)?;
    let separator = body
        .windows(2)
        .rposition(|window| window == [0, 0])
        .ok_or(FunctionScheduleError::CorruptRecord)?;
    std::str::from_utf8(&body[separator + 2..])
        .map(str::to_owned)
        .map_err(|_| FunctionScheduleError::CorruptRecord)
}

fn format_timestamp(unix_seconds: u64) -> Option<String> {
    i64::try_from(unix_seconds)
        .ok()
        .and_then(|timestamp| OffsetDateTime::from_unix_timestamp(timestamp).ok())
        .and_then(|time| time.format(&Rfc3339).ok())
}

/// How a scheduled invocation reaches the function: the edge gateway over
/// the internal hop in production, a stub in tests.
pub trait FunctionScheduleInvoker: Send + Sync {
    fn invoke(
        &self,
        tenant: &TenantScope,
        request: &FunctionScheduleInvokeRequest,
    ) -> Result<FunctionScheduleInvokeResponse, FunctionScheduleInvokeError>;
}

/// Why an invocation could not complete. The function's own status, when
/// it answered, is not an error but an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionScheduleInvokeError {
    /// The gateway did not answer within the invocation timeout.
    Timeout,
    /// The function has no active deployment (or no longer exists).
    NoActiveDeployment,
    /// The gateway was unreachable, or reported its own dependencies down.
    GatewayUnavailable,
    /// The gateway refused the request's shape.
    InvalidRequest,
    /// The tenant's invocation quota or rate limit refused the run.
    Throttled,
}

impl FunctionScheduleInvokeError {
    /// The stable, non-sensitive reason a run shows.
    #[must_use]
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::Timeout => ERROR_TIMEOUT,
            Self::NoActiveDeployment => ERROR_NO_ACTIVE_DEPLOYMENT,
            Self::GatewayUnavailable => ERROR_GATEWAY_UNAVAILABLE,
            Self::InvalidRequest => ERROR_INVALID_REQUEST,
            Self::Throttled => ERROR_THROTTLED,
        }
    }
}

impl fmt::Display for FunctionScheduleInvokeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.stable_code())
    }
}

impl Error for FunctionScheduleInvokeError {}

impl FunctionScheduleInvoker for ControlToEdgeClient {
    fn invoke(
        &self,
        tenant: &TenantScope,
        request: &FunctionScheduleInvokeRequest,
    ) -> Result<FunctionScheduleInvokeResponse, FunctionScheduleInvokeError> {
        let request_id = random_token("req_");
        let idempotency_key = random_token("idem_");
        let started = Instant::now();
        self.invoke_function_schedule(tenant, &request_id, &idempotency_key, request)
            .map_err(|error| match error {
                InternalClientError::Remote { status: 404, .. } => {
                    FunctionScheduleInvokeError::NoActiveDeployment
                }
                InternalClientError::Remote { status: 429, .. } => {
                    FunctionScheduleInvokeError::Throttled
                }
                InternalClientError::Remote {
                    status: 400 | 401 | 403 | 413,
                    ..
                } => FunctionScheduleInvokeError::InvalidRequest,
                InternalClientError::InvalidPayload => FunctionScheduleInvokeError::InvalidRequest,
                InternalClientError::Unavailable | InternalClientError::TimedOut
                    if started.elapsed() >= Duration::from_secs(INVOCATION_TIMEOUT_SECONDS) =>
                {
                    FunctionScheduleInvokeError::Timeout
                }
                _ => FunctionScheduleInvokeError::GatewayUnavailable,
            })
    }
}

/// The schedule records in the control store: schedules, their registry
/// and due indexes, leases, and run histories.
#[derive(Clone)]
pub struct FunctionScheduleStore {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for FunctionScheduleStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionScheduleStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl FunctionScheduleStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
    ) -> Result<Self, FunctionScheduleError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(FunctionScheduleError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
        })
    }

    async fn apply(&self, write: AtomicWrite) -> Result<bool, FunctionScheduleError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(true),
            CompareAndWriteResult::Conflict { .. } => Ok(false),
        }
    }

    async fn write_unconditionally(&self, batch: WriteBatch) -> Result<(), FunctionScheduleError> {
        if batch.is_empty() {
            return Ok(());
        }
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
    ) -> Result<Vec<KeyValue>, FunctionScheduleError> {
        Ok(self
            .adapter
            .scan(ScanRequest::new(
                range,
                direction,
                NonZeroUsize::new(limit).ok_or(FunctionScheduleError::LimitExceeded)?,
            ))
            .await?)
    }

    /// The function record a schedule targets, if the function exists and
    /// has not been deleted.
    async fn function(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
    ) -> Result<Option<FunctionRecord>, FunctionScheduleError> {
        let key =
            ControlKeyspace::function_key(tenant.project_id(), tenant.environment_id(), name)?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: FunctionRecord = serde_json::from_slice(&value)?;
        if record.tenant() != tenant || record.name() != name {
            return Err(FunctionScheduleError::CorruptRecord);
        }
        Ok((record.state() != FunctionState::Deleted).then_some(record))
    }

    /// Every registered schedule, with where its record lives.
    async fn registry(
        &self,
    ) -> Result<Vec<(FunctionScheduleId, TenantScope, FunctionName)>, FunctionScheduleError> {
        let entries = self
            .scan(
                ControlKeyspace::function_schedule_index_range()?,
                ScanDirection::Forward,
                MAXIMUM_REGISTRY_SCAN,
            )
            .await?;
        if entries.len() >= MAXIMUM_REGISTRY_SCAN {
            return Err(FunctionScheduleError::LimitExceeded);
        }
        let mut registry = Vec::with_capacity(entries.len());
        for entry in entries {
            let index: ScheduleIndexEntry = serde_json::from_slice(&entry.value)?;
            let id = decode_index_item(&entry.key)?;
            registry.push((
                FunctionScheduleId::parse(id)?,
                index.tenant,
                index.function_name,
            ));
        }
        Ok(registry)
    }

    pub async fn get_schedule(
        &self,
        tenant: &TenantScope,
        function_name: &FunctionName,
        id: &FunctionScheduleId,
    ) -> Result<Option<FunctionScheduleRecord>, FunctionScheduleError> {
        let key = ControlKeyspace::function_schedule_key(
            tenant.project_id(),
            tenant.environment_id(),
            function_name.as_str(),
            id.as_str(),
        )?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: FunctionScheduleRecord = serde_json::from_slice(&value)?;
        if record.tenant != *tenant || record.function_name != *function_name || record.id != *id {
            return Err(FunctionScheduleError::CorruptRecord);
        }
        Ok(Some(record))
    }

    pub async fn list_schedules(
        &self,
        tenant: &TenantScope,
        function_name: &FunctionName,
    ) -> Result<Vec<FunctionScheduleRecord>, FunctionScheduleError> {
        let entries = self
            .scan(
                ControlKeyspace::function_schedules_range(
                    tenant.project_id(),
                    tenant.environment_id(),
                    function_name.as_str(),
                )?,
                ScanDirection::Forward,
                MAXIMUM_SCHEDULE_SCAN,
            )
            .await?;
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            let record: FunctionScheduleRecord = serde_json::from_slice(&entry.value)?;
            if record.tenant != *tenant || record.function_name != *function_name {
                return Err(FunctionScheduleError::CorruptRecord);
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

    fn schedule_key(record: &FunctionScheduleRecord) -> Result<Vec<u8>, FunctionScheduleError> {
        Ok(ControlKeyspace::function_schedule_key(
            record.tenant.project_id(),
            record.tenant.environment_id(),
            record.function_name.as_str(),
            record.id.as_str(),
        )?)
    }

    fn lease_key(
        tenant: &TenantScope,
        id: &FunctionScheduleId,
    ) -> Result<Vec<u8>, FunctionScheduleError> {
        Ok(ControlKeyspace::function_schedule_lease_key(
            tenant.project_id(),
            tenant.environment_id(),
            id.as_str(),
        )?)
    }

    /// Writes the record and moves its cron due entry with it.
    fn put_schedule(
        &self,
        batch: &mut WriteBatch,
        previous: Option<&FunctionScheduleRecord>,
        record: &FunctionScheduleRecord,
    ) -> Result<(), FunctionScheduleError> {
        batch.put(Self::schedule_key(record)?, serde_json::to_vec(record)?);
        let previous_due = previous.and_then(FunctionScheduleRecord::due_item);
        let next_due = record.due_item();
        if previous_due != next_due {
            if let Some(due) = &previous_due {
                batch.delete(ControlKeyspace::function_schedule_due_key(due)?);
            }
            if let Some(due) = &next_due {
                batch.put(
                    ControlKeyspace::function_schedule_due_key(due)?,
                    serde_json::to_vec(&DueEntry {
                        tenant: record.tenant.clone(),
                        function_name: record.function_name.clone(),
                        schedule_id: record.id.clone(),
                        run_item: None,
                    })?,
                );
            }
        }
        Ok(())
    }

    async fn insert_schedule(
        &self,
        record: &FunctionScheduleRecord,
    ) -> Result<(), FunctionScheduleError> {
        let key = Self::schedule_key(record)?;
        let mut batch = WriteBatch::with_capacity(3);
        self.put_schedule(&mut batch, None, record)?;
        batch.put(
            ControlKeyspace::function_schedule_index_key(record.id.as_str())?,
            serde_json::to_vec(&ScheduleIndexEntry {
                tenant: record.tenant.clone(),
                function_name: record.function_name.clone(),
            })?,
        );
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
            Err(FunctionScheduleError::Conflict(
                "function schedule already exists",
            ))
        }
    }

    /// Replaces a schedule if it is still `previous`; `extra` rides in the
    /// same atomic write.
    async fn replace_schedule(
        &self,
        previous: &FunctionScheduleRecord,
        next: &FunctionScheduleRecord,
        mut extra: WriteBatch,
    ) -> Result<bool, FunctionScheduleError> {
        if previous.id != next.id
            || previous.tenant != next.tenant
            || previous.function_name != next.function_name
        {
            return Err(FunctionScheduleError::Conflict(
                "function schedule identity changed",
            ));
        }
        self.put_schedule(&mut extra, Some(previous), next)?;
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key: Self::schedule_key(previous)?,
                value: serde_json::to_vec(previous)?,
            }],
            batch: extra,
            durability: self.durability,
        })
        .await
    }

    /// Removes the schedule, its indexes, its lease, and its history.
    async fn remove_schedule(
        &self,
        record: &FunctionScheduleRecord,
    ) -> Result<(), FunctionScheduleError> {
        let (project_id, environment_id) =
            (record.tenant.project_id(), record.tenant.environment_id());
        self.delete_range(ControlKeyspace::function_schedule_runs_range(
            project_id,
            environment_id,
            record.id.as_str(),
        )?)
        .await?;
        let mut batch = WriteBatch::with_capacity(4);
        batch.delete(Self::schedule_key(record)?);
        batch.delete(ControlKeyspace::function_schedule_index_key(
            record.id.as_str(),
        )?);
        batch.delete(Self::lease_key(&record.tenant, &record.id)?);
        if let Some(due) = record.due_item() {
            batch.delete(ControlKeyspace::function_schedule_due_key(&due)?);
        }
        // Queued manual runs keep their due entries; the worker drops them
        // when it finds the schedule gone.
        self.write_unconditionally(batch).await
    }

    async fn delete_range(&self, range: KeyRange) -> Result<(), FunctionScheduleError> {
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
        Err(FunctionScheduleError::LimitExceeded)
    }

    async fn delete_keys(
        &self,
        keys: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<(), FunctionScheduleError> {
        let mut batch = WriteBatch::new();
        for key in keys {
            batch.delete(key);
        }
        self.write_unconditionally(batch).await
    }

    /// The lease a running invocation holds, if it has not expired.
    async fn lease(
        &self,
        tenant: &TenantScope,
        id: &FunctionScheduleId,
        now_unix_seconds: u64,
    ) -> Result<Option<LeaseRecord>, FunctionScheduleError> {
        let Some(value) = self.adapter.get(&Self::lease_key(tenant, id)?).await? else {
            return Ok(None);
        };
        let lease: LeaseRecord = serde_json::from_slice(&value)?;
        Ok((lease.expires_at_unix_seconds > now_unix_seconds).then_some(lease))
    }

    fn put_lease(
        batch: &mut WriteBatch,
        tenant: &TenantScope,
        id: &FunctionScheduleId,
        lease: &LeaseRecord,
    ) -> Result<(), FunctionScheduleError> {
        batch.put(Self::lease_key(tenant, id)?, serde_json::to_vec(lease)?);
        Ok(())
    }

    fn run_key(record: &FunctionScheduleRunRecord) -> Result<Vec<u8>, FunctionScheduleError> {
        Ok(ControlKeyspace::function_schedule_run_key(
            record.tenant.project_id(),
            record.tenant.environment_id(),
            record.schedule_id.as_str(),
            &record.item(),
        )?)
    }

    fn put_run(
        batch: &mut WriteBatch,
        record: &FunctionScheduleRunRecord,
    ) -> Result<(), FunctionScheduleError> {
        batch.put(Self::run_key(record)?, serde_json::to_vec(record)?);
        Ok(())
    }

    /// Queues a manual run: the record and a due entry naming it.
    async fn insert_queued_run(
        &self,
        record: &FunctionScheduleRunRecord,
    ) -> Result<(), FunctionScheduleError> {
        let key = Self::run_key(record)?;
        let mut batch = WriteBatch::with_capacity(2);
        Self::put_run(&mut batch, record)?;
        batch.put(
            ControlKeyspace::function_schedule_due_key(&due_item(
                record.due_at_unix_seconds,
                &record.schedule_id,
                record.id.as_str(),
            ))?,
            serde_json::to_vec(&DueEntry {
                tenant: record.tenant.clone(),
                function_name: record.function_name.clone(),
                schedule_id: record.schedule_id.clone(),
                run_item: Some(record.item()),
            })?,
        );
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
            Err(FunctionScheduleError::Conflict(
                "function schedule run already exists",
            ))
        }
    }

    async fn replace_run(
        &self,
        previous: &FunctionScheduleRunRecord,
        next: &FunctionScheduleRunRecord,
        mut extra: WriteBatch,
    ) -> Result<bool, FunctionScheduleError> {
        if previous.id != next.id
            || previous.schedule_id != next.schedule_id
            || previous.tenant != next.tenant
            || previous.created_at_unix_seconds != next.created_at_unix_seconds
            || previous.due_at_unix_seconds != next.due_at_unix_seconds
        {
            return Err(FunctionScheduleError::Conflict(
                "function schedule run identity changed",
            ));
        }
        Self::put_run(&mut extra, next)?;
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key: Self::run_key(previous)?,
                value: serde_json::to_vec(previous)?,
            }],
            batch: extra,
            durability: self.durability,
        })
        .await
    }

    async fn get_run_by_item(
        &self,
        tenant: &TenantScope,
        schedule_id: &FunctionScheduleId,
        item: &str,
    ) -> Result<Option<FunctionScheduleRunRecord>, FunctionScheduleError> {
        let key = ControlKeyspace::function_schedule_run_key(
            tenant.project_id(),
            tenant.environment_id(),
            schedule_id.as_str(),
            item,
        )?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: FunctionScheduleRunRecord = serde_json::from_slice(&value)?;
        if record.tenant != *tenant || record.schedule_id != *schedule_id {
            return Err(FunctionScheduleError::CorruptRecord);
        }
        Ok(Some(record))
    }

    /// Everything due at or before `now`, earliest first, bounded.
    async fn due(&self, now_unix_seconds: u64) -> Result<Vec<Due>, FunctionScheduleError> {
        let entries = self
            .scan(
                ControlKeyspace::function_schedule_due_range()?,
                ScanDirection::Forward,
                MAXIMUM_DUE_SCAN,
            )
            .await?;
        let mut due = Vec::new();
        for entry in entries {
            let item = decode_index_item(&entry.key)?;
            let at: u64 = item
                .split('/')
                .next()
                .and_then(|prefix| prefix.parse().ok())
                .ok_or(FunctionScheduleError::CorruptRecord)?;
            if at > now_unix_seconds {
                break;
            }
            let parsed: DueEntry = serde_json::from_slice(&entry.value)?;
            if !item.contains(parsed.schedule_id.as_str()) {
                return Err(FunctionScheduleError::CorruptRecord);
            }
            due.push(Due {
                key: entry.key,
                at_unix_seconds: at,
                entry: parsed,
            });
        }
        Ok(due)
    }

    /// The history newest first, optionally narrowed to one outcome, from a
    /// cursor.
    async fn list_runs(
        &self,
        tenant: &TenantScope,
        schedule_id: &FunctionScheduleId,
        outcome: Option<FunctionScheduleRunOutcome>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FunctionScheduleRunPage, FunctionScheduleError> {
        let (project_id, environment_id) = (tenant.project_id(), tenant.environment_id());
        let mut range = ControlKeyspace::function_schedule_runs_range(
            project_id,
            environment_id,
            schedule_id.as_str(),
        )?;
        let cursor_key = cursor
            .map(|cursor| {
                let item = decode_cursor(cursor)?;
                ControlKeyspace::function_schedule_run_key(
                    project_id,
                    environment_id,
                    schedule_id.as_str(),
                    &item,
                )
                .map_err(FunctionScheduleError::from)
            })
            .transpose()?;
        if let Some(cursor_key) = &cursor_key {
            if !range.contains(cursor_key) {
                return Err(FunctionScheduleError::InvalidRequest("cursor is invalid"));
            }
            range.start_inclusive.clone_from(cursor_key);
        }
        let entries = self
            .scan(range, ScanDirection::Forward, MAXIMUM_HISTORY_SCAN)
            .await?;
        let mut items = Vec::with_capacity(limit.min(entries.len()));
        let mut remaining = false;
        for entry in entries {
            if cursor_key.as_ref() == Some(&entry.key) {
                continue;
            }
            let record: FunctionScheduleRunRecord = serde_json::from_slice(&entry.value)?;
            if record.tenant != *tenant || record.schedule_id != *schedule_id {
                return Err(FunctionScheduleError::CorruptRecord);
            }
            if outcome.is_some_and(|outcome| record.outcome != Some(outcome)) {
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
        Ok(FunctionScheduleRunPage { items, next_cursor })
    }

    /// Removes completed runs older than the retention window and, when the
    /// history holds more than the ceiling, the oldest completed ones beyond
    /// it. Bounded per call; the next pass continues.
    async fn cleanup_schedule(
        &self,
        tenant: &TenantScope,
        schedule_id: &FunctionScheduleId,
        now_unix_seconds: u64,
        config: &FunctionScheduleWorkerConfig,
    ) -> Result<usize, FunctionScheduleError> {
        let range = ControlKeyspace::function_schedule_runs_range(
            tenant.project_id(),
            tenant.environment_id(),
            schedule_id.as_str(),
        )?;
        let count =
            usize::try_from(self.adapter.count_keys(range.clone()).await?).unwrap_or(usize::MAX);
        let mut excess = count.saturating_sub(config.maximum_runs_per_schedule);
        // Oldest first: the history is newest first, so read it backwards.
        let entries = self
            .scan(range, ScanDirection::Reverse, RETENTION_BATCH)
            .await?;
        let mut batch = WriteBatch::new();
        let mut removed = 0;
        for entry in entries {
            let record: FunctionScheduleRunRecord = serde_json::from_slice(&entry.value)?;
            if record.outcome.is_none() {
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
            excess = excess.saturating_sub(1);
            removed += 1;
        }
        if removed > 0 {
            self.write_unconditionally(batch).await?;
        }
        Ok(removed)
    }
}

/// Attaches, reads, changes, and removes schedules with the same membership
/// rules as the environment's other resources: any member may read; a role
/// that can change projects may write. Every mutation is audited.
#[derive(Clone)]
pub struct FunctionScheduleService {
    store: FunctionScheduleStore,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
}

impl fmt::Debug for FunctionScheduleService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionScheduleService")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl FunctionScheduleService {
    #[must_use]
    pub fn new(
        store: FunctionScheduleStore,
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
    ) -> Self {
        Self {
            store,
            projects,
            organizations,
            audit,
        }
    }

    #[must_use]
    pub fn store(&self) -> &FunctionScheduleStore {
        &self.store
    }

    pub async fn list(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        function_name: &FunctionName,
        now_unix_seconds: u64,
    ) -> Result<Vec<FunctionScheduleRecord>, FunctionScheduleError> {
        self.authorize(
            actor,
            tenant,
            ControlAuditAction::FunctionScheduleRead,
            false,
            now_unix_seconds,
        )
        .await?;
        self.function(tenant, function_name).await?;
        self.store.list_schedules(tenant, function_name).await
    }

    pub async fn get(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        function_name: &FunctionName,
        id: &FunctionScheduleId,
        now_unix_seconds: u64,
    ) -> Result<FunctionScheduleRecord, FunctionScheduleError> {
        self.authorize(
            actor,
            tenant,
            ControlAuditAction::FunctionScheduleRead,
            false,
            now_unix_seconds,
        )
        .await?;
        self.function(tenant, function_name).await?;
        self.store
            .get_schedule(tenant, function_name, id)
            .await?
            .ok_or(FunctionScheduleError::ScheduleNotFound)
    }

    /// Attaches a schedule. The function must have an active deployment; the
    /// expression must be valid and have a due time within the search
    /// horizon; the request must satisfy the gateway's rules.
    pub async fn create(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        function_name: &FunctionName,
        input: NewFunctionSchedule,
        now_unix_seconds: u64,
    ) -> Result<FunctionScheduleRecord, FunctionScheduleError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::FunctionScheduleCreate,
                true,
                now_unix_seconds,
            )
            .await?;
        let function = self.function(tenant, function_name).await?;
        require_active_deployment(&function)?;
        let name = input.name.unwrap_or_default();
        validate_name(&name)?;
        let cron = CronSchedule::parse(&input.cron)?;
        let next_run_at = next_due_time(&cron, now_unix_seconds)?;
        let mut request = input.request.unwrap_or_default();
        request.validate()?;
        let existing = self.store.list_schedules(tenant, function_name).await?;
        if existing.len() >= MAXIMUM_SCHEDULES_PER_FUNCTION {
            return Err(FunctionScheduleError::Conflict(
                "function already has the maximum number of schedules",
            ));
        }
        let record = FunctionScheduleRecord {
            id: FunctionScheduleId::generate(),
            tenant: tenant.clone(),
            function_name: function_name.clone(),
            name,
            cron: input.cron,
            request,
            enabled: input.enabled,
            next_run_at_unix_seconds: input.enabled.then_some(next_run_at),
            last_run: None,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
        };
        self.store.insert_schedule(&record).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionScheduleCreate,
            &schedule_target(function_name, &record.id),
            now_unix_seconds,
        );
        Ok(record)
    }

    /// Changes name, expression, request, or the enabled flag. A changed
    /// expression or a change of the flag recomputes the next run from
    /// now; `enabled: false` pauses the schedule with a null next run.
    /// Re-enabling requires an active deployment, like creation.
    pub async fn update(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        function_name: &FunctionName,
        id: &FunctionScheduleId,
        update: FunctionScheduleUpdate,
        now_unix_seconds: u64,
    ) -> Result<FunctionScheduleRecord, FunctionScheduleError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::FunctionScheduleUpdate,
                true,
                now_unix_seconds,
            )
            .await?;
        if update.is_empty() {
            return Err(FunctionScheduleError::InvalidRequest(
                "update changes nothing",
            ));
        }
        let function = self.function(tenant, function_name).await?;
        let previous = self
            .store
            .get_schedule(tenant, function_name, id)
            .await?
            .ok_or(FunctionScheduleError::ScheduleNotFound)?;
        let mut next = previous.clone();
        if let Some(name) = update.name {
            validate_name(&name)?;
            next.name = name;
        }
        if let Some(cron) = update.cron {
            CronSchedule::parse(&cron)?;
            next.cron = cron;
        }
        if let Some(mut request) = update.request {
            request.validate()?;
            next.request = request;
        }
        if let Some(enabled) = update.enabled {
            if enabled && !previous.enabled {
                require_active_deployment(&function)?;
            }
            next.enabled = enabled;
        }
        if next.cron != previous.cron || next.enabled != previous.enabled {
            let cron = CronSchedule::parse(&next.cron)?;
            next.next_run_at_unix_seconds = if next.enabled {
                Some(next_due_time(&cron, now_unix_seconds)?)
            } else {
                None
            };
        }
        next.updated_at_unix_seconds = now_unix_seconds;
        if next != previous
            && !self
                .store
                .replace_schedule(&previous, &next, WriteBatch::new())
                .await?
        {
            return Err(FunctionScheduleError::Conflict(
                "function schedule changed concurrently; read it again",
            ));
        }
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionScheduleUpdate,
            &schedule_target(function_name, id),
            now_unix_seconds,
        );
        Ok(next)
    }

    /// Removes the schedule and its run history.
    pub async fn delete(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        function_name: &FunctionName,
        id: &FunctionScheduleId,
        now_unix_seconds: u64,
    ) -> Result<(), FunctionScheduleError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::FunctionScheduleDelete,
                true,
                now_unix_seconds,
            )
            .await?;
        self.function(tenant, function_name).await?;
        let record = self
            .store
            .get_schedule(tenant, function_name, id)
            .await?
            .ok_or(FunctionScheduleError::ScheduleNotFound)?;
        self.store.remove_schedule(&record).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionScheduleDelete,
            &schedule_target(function_name, id),
            now_unix_seconds,
        );
        Ok(())
    }

    /// Queues one invocation with the schedule's request for now, recorded
    /// as a manual run. Refused while a run of the schedule is executing.
    pub async fn run_now(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        function_name: &FunctionName,
        id: &FunctionScheduleId,
        now_unix_seconds: u64,
    ) -> Result<FunctionScheduleRunRecord, FunctionScheduleError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::FunctionScheduleRunNow,
                true,
                now_unix_seconds,
            )
            .await?;
        let function = self.function(tenant, function_name).await?;
        require_active_deployment(&function)?;
        let schedule = self
            .store
            .get_schedule(tenant, function_name, id)
            .await?
            .ok_or(FunctionScheduleError::ScheduleNotFound)?;
        if self
            .store
            .lease(tenant, id, now_unix_seconds)
            .await?
            .is_some()
        {
            return Err(FunctionScheduleError::Conflict(
                "a run of this schedule is still executing",
            ));
        }
        let run =
            FunctionScheduleRunRecord::new(&schedule, now_unix_seconds, true, now_unix_seconds);
        self.store.insert_queued_run(&run).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionScheduleRunNow,
            &format!("{}/{}", schedule_target(function_name, id), run.id.as_str()),
            now_unix_seconds,
        );
        Ok(run)
    }

    pub async fn list_runs(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        function_name: &FunctionName,
        id: &FunctionScheduleId,
        query: &FunctionScheduleRunQuery,
        now_unix_seconds: u64,
    ) -> Result<FunctionScheduleRunPage, FunctionScheduleError> {
        self.authorize(
            actor,
            tenant,
            ControlAuditAction::FunctionScheduleRead,
            false,
            now_unix_seconds,
        )
        .await?;
        self.function(tenant, function_name).await?;
        self.store
            .get_schedule(tenant, function_name, id)
            .await?
            .ok_or(FunctionScheduleError::ScheduleNotFound)?;
        self.store
            .list_runs(
                tenant,
                id,
                query.outcome,
                query.cursor.as_deref(),
                query.limit.get(),
            )
            .await
    }

    async fn function(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
    ) -> Result<FunctionRecord, FunctionScheduleError> {
        self.store
            .function(tenant, name)
            .await?
            .ok_or(FunctionScheduleError::FunctionNotFound)
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        action: ControlAuditAction,
        write: bool,
        now_unix_seconds: u64,
    ) -> Result<OrganizationId, FunctionScheduleError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(FunctionScheduleError::EnvironmentNotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(FunctionScheduleError::EnvironmentNotFound)?;
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
                target: audit_target(tenant, "authorization"),
                outcome: ControlAuditOutcome::Denied,
                at_unix_seconds: now_unix_seconds,
            });
            return Err(FunctionScheduleError::Forbidden);
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
            target: audit_target(tenant, target),
            outcome: ControlAuditOutcome::Allowed,
            at_unix_seconds,
        });
    }
}

fn require_active_deployment(function: &FunctionRecord) -> Result<(), FunctionScheduleError> {
    match (function.state(), function.active_version()) {
        (FunctionState::Active, Some(_)) => Ok(()),
        _ => Err(FunctionScheduleError::Conflict(
            "function has no active deployment",
        )),
    }
}

fn validate_name(name: &str) -> Result<(), FunctionScheduleError> {
    if name.len() > MAXIMUM_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(FunctionScheduleError::InvalidRequest(
            "name must be at most 128 bytes without control characters",
        ));
    }
    Ok(())
}

/// The first due time after `now`, or the addressed refusal for an
/// expression that never fires within the horizon.
fn next_due_time(cron: &CronSchedule, now_unix_seconds: u64) -> Result<u64, FunctionScheduleError> {
    cron.next_after(now_unix_seconds)
        .ok_or(FunctionScheduleError::InvalidRequest(
            "cron expression has no due time within the next five years",
        ))
}

/// The last due time at or before `now`, starting from a due time `from`
/// that is itself at or before `now`. This is what a schedule that was down
/// runs for: once, for the latest slot it missed.
fn latest_due_at_or_before(cron: &CronSchedule, from: u64, now_unix_seconds: u64) -> u64 {
    let mut due = from;
    for _ in 0..MAXIMUM_CATCH_UP_STEPS {
        match cron.next_after(due) {
            Some(next) if next <= now_unix_seconds => due = next,
            _ => break,
        }
    }
    due
}

fn schedule_target(function_name: &FunctionName, id: &FunctionScheduleId) -> String {
    format!("{}/{}", function_name.as_str(), id.as_str())
}

fn audit_target(tenant: &TenantScope, target: &str) -> String {
    format!(
        "{}/{}/function-schedules/{}",
        tenant.project_id().as_str(),
        tenant.environment_id().as_str(),
        target
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionScheduleWorkerConfig {
    pub invocation_timeout_seconds: u64,
    pub lease_grace_seconds: u64,
    pub retention_seconds: u64,
    pub maximum_runs_per_schedule: usize,
    pub runs_per_pass: usize,
}

impl Default for FunctionScheduleWorkerConfig {
    fn default() -> Self {
        Self {
            invocation_timeout_seconds: INVOCATION_TIMEOUT_SECONDS,
            lease_grace_seconds: LEASE_GRACE_SECONDS,
            retention_seconds: RUN_RETENTION_SECONDS,
            maximum_runs_per_schedule: MAXIMUM_RUNS_PER_SCHEDULE,
            runs_per_pass: RUNS_PER_PASS,
        }
    }
}

impl FunctionScheduleWorkerConfig {
    fn validate(&self) -> Result<(), FunctionScheduleError> {
        if self.invocation_timeout_seconds == 0
            || self.retention_seconds == 0
            || self.maximum_runs_per_schedule == 0
            || self.runs_per_pass == 0
        {
            return Err(FunctionScheduleError::InvalidConfiguration);
        }
        Ok(())
    }

    const fn lease_seconds(&self) -> u64 {
        self.invocation_timeout_seconds
            .saturating_add(self.lease_grace_seconds)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionScheduleWorkerReport {
    /// Due entries read this pass.
    pub due: usize,
    /// Runs started (invoked).
    pub started: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub errored: usize,
    pub skipped_overlap: usize,
    /// Due entries that no longer named a live schedule or queued run.
    pub stale: usize,
    pub cleaned: usize,
}

/// Evaluates due times, invokes through the gateway, records runs.
#[derive(Clone)]
pub struct FunctionScheduleWorker {
    store: FunctionScheduleStore,
    invoker: Arc<dyn FunctionScheduleInvoker>,
    config: FunctionScheduleWorkerConfig,
}

impl fmt::Debug for FunctionScheduleWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionScheduleWorker")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl FunctionScheduleWorker {
    pub fn new(
        store: FunctionScheduleStore,
        invoker: Arc<dyn FunctionScheduleInvoker>,
        config: FunctionScheduleWorkerConfig,
    ) -> Result<Self, FunctionScheduleError> {
        config.validate()?;
        Ok(Self {
            store,
            invoker,
            config,
        })
    }

    #[must_use]
    pub fn store(&self) -> &FunctionScheduleStore {
        &self.store
    }

    /// One pass: every due entry at or before `now`, oldest first, up to
    /// `runs_per_pass` invocations, then retention. Invocations run one
    /// after another on the calling thread; a gateway that cannot be
    /// reached is an `error` outcome on the run, not a worker failure.
    pub async fn run_once(
        &self,
        now_unix_seconds: u64,
    ) -> Result<FunctionScheduleWorkerReport, FunctionScheduleError> {
        let mut report = FunctionScheduleWorkerReport::default();
        let entries = self.store.due(now_unix_seconds).await?;
        report.due = entries.len();
        for due in entries {
            if report.started >= self.config.runs_per_pass {
                break;
            }
            let Due {
                key,
                at_unix_seconds,
                entry,
            } = due;
            let Some(schedule) = self
                .store
                .get_schedule(&entry.tenant, &entry.function_name, &entry.schedule_id)
                .await?
            else {
                self.store.delete_keys([key]).await?;
                report.stale += 1;
                continue;
            };
            match entry.run_item {
                None => {
                    self.run_cron(
                        schedule,
                        at_unix_seconds,
                        key,
                        now_unix_seconds,
                        &mut report,
                    )
                    .await?;
                }
                Some(item) => {
                    self.run_manual(schedule, &item, key, now_unix_seconds, &mut report)
                        .await?;
                }
            }
        }
        for (id, tenant, _) in self.store.registry().await? {
            report.cleaned += self
                .store
                .cleanup_schedule(&tenant, &id, now_unix_seconds, &self.config)
                .await?;
        }
        Ok(report)
    }

    /// A cron due time. The entry is authoritative only while it still
    /// matches the record; otherwise the record was changed under it and
    /// the record's own entry will be found instead.
    async fn run_cron(
        &self,
        schedule: FunctionScheduleRecord,
        at_unix_seconds: u64,
        key: Vec<u8>,
        now_unix_seconds: u64,
        report: &mut FunctionScheduleWorkerReport,
    ) -> Result<(), FunctionScheduleError> {
        if !schedule.enabled || schedule.next_run_at_unix_seconds != Some(at_unix_seconds) {
            self.store.delete_keys([key]).await?;
            report.stale += 1;
            return Ok(());
        }
        let Ok(cron) = CronSchedule::parse(&schedule.cron) else {
            // Validated at save time; a record this parser no longer reads
            // is left as it is, visibly overdue, rather than fired blindly.
            self.store.delete_keys([key]).await?;
            report.stale += 1;
            return Ok(());
        };
        // Missed slots collapse into the latest one; the next due time is
        // computed from now, never from the missed slot.
        let due_at = latest_due_at_or_before(&cron, at_unix_seconds, now_unix_seconds);
        let mut next = schedule.clone();
        next.next_run_at_unix_seconds = cron.next_after(now_unix_seconds);
        if self
            .store
            .lease(&schedule.tenant, &schedule.id, now_unix_seconds)
            .await?
            .is_some()
        {
            let run = FunctionScheduleRunRecord::skipped(&schedule, due_at, now_unix_seconds);
            next.last_run = run.summary();
            let mut batch = WriteBatch::with_capacity(1);
            FunctionScheduleStore::put_run(&mut batch, &run)?;
            if self.store.replace_schedule(&schedule, &next, batch).await? {
                report.skipped_overlap += 1;
            }
            return Ok(());
        }
        let mut run = FunctionScheduleRunRecord::new(&schedule, due_at, false, now_unix_seconds);
        run.start(now_unix_seconds);
        let lease = LeaseRecord {
            run_id: run.id.clone(),
            expires_at_unix_seconds: now_unix_seconds.saturating_add(self.config.lease_seconds()),
        };
        let mut batch = WriteBatch::with_capacity(2);
        FunctionScheduleStore::put_run(&mut batch, &run)?;
        FunctionScheduleStore::put_lease(&mut batch, &schedule.tenant, &schedule.id, &lease)?;
        // Advancing the due time is part of taking the lease: a run that
        // outlives its interval can never re-fire its own due time.
        if !self.store.replace_schedule(&schedule, &next, batch).await? {
            return Ok(());
        }
        report.started += 1;
        self.execute(&next, run, now_unix_seconds, report).await
    }

    /// A queued manual run: it does not touch the cron due time, but it
    /// takes the same lease and is skipped the same way.
    async fn run_manual(
        &self,
        schedule: FunctionScheduleRecord,
        item: &str,
        key: Vec<u8>,
        now_unix_seconds: u64,
        report: &mut FunctionScheduleWorkerReport,
    ) -> Result<(), FunctionScheduleError> {
        let queued = match self
            .store
            .get_run_by_item(&schedule.tenant, &schedule.id, item)
            .await?
        {
            Some(run) if run.outcome.is_none() && run.started_at_unix_seconds.is_none() => run,
            _ => {
                self.store.delete_keys([key]).await?;
                report.stale += 1;
                return Ok(());
            }
        };
        let mut batch = WriteBatch::with_capacity(2);
        batch.delete(key);
        if self
            .store
            .lease(&schedule.tenant, &schedule.id, now_unix_seconds)
            .await?
            .is_some()
        {
            let mut skipped = queued.clone();
            skipped.skip();
            if self.store.replace_run(&queued, &skipped, batch).await? {
                report.skipped_overlap += 1;
                self.note_last_run(&schedule, &skipped).await?;
            }
            return Ok(());
        }
        let mut run = queued.clone();
        run.start(now_unix_seconds);
        let lease = LeaseRecord {
            run_id: run.id.clone(),
            expires_at_unix_seconds: now_unix_seconds.saturating_add(self.config.lease_seconds()),
        };
        FunctionScheduleStore::put_lease(&mut batch, &schedule.tenant, &schedule.id, &lease)?;
        if !self.store.replace_run(&queued, &run, batch).await? {
            return Ok(());
        }
        report.started += 1;
        self.execute(&schedule, run, now_unix_seconds, report).await
    }

    /// Invokes, then records the outcome, releases the lease, and notes the
    /// run on the schedule in one write.
    async fn execute(
        &self,
        schedule: &FunctionScheduleRecord,
        run: FunctionScheduleRunRecord,
        now_unix_seconds: u64,
        report: &mut FunctionScheduleWorkerReport,
    ) -> Result<(), FunctionScheduleError> {
        let request = self.invocation_request(schedule, &run)?;
        let started = Instant::now();
        let result = self.invoker.invoke(&schedule.tenant, &request);
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let completed_at = now_unix_seconds.saturating_add(elapsed / 1_000);
        let mut completed = run.clone();
        completed.complete(&result, elapsed, completed_at);
        match completed.outcome {
            Some(FunctionScheduleRunOutcome::Succeeded) => report.succeeded += 1,
            Some(FunctionScheduleRunOutcome::Failed) => report.failed += 1,
            _ => report.errored += 1,
        }
        let mut batch = WriteBatch::with_capacity(2);
        FunctionScheduleStore::put_run(&mut batch, &completed)?;
        batch.delete(FunctionScheduleStore::lease_key(
            &schedule.tenant,
            &schedule.id,
        )?);
        self.note_last_run_with(schedule, &completed, batch).await
    }

    async fn note_last_run(
        &self,
        schedule: &FunctionScheduleRecord,
        run: &FunctionScheduleRunRecord,
    ) -> Result<(), FunctionScheduleError> {
        self.note_last_run_with(schedule, run, WriteBatch::new())
            .await
    }

    /// Writes `extra` together with the schedule's `lastRun`, reloading the
    /// schedule until the write lands so a change a developer made
    /// meanwhile is never clobbered. If the schedule is gone, `extra` is
    /// written alone minus the run (its history went with it).
    async fn note_last_run_with(
        &self,
        schedule: &FunctionScheduleRecord,
        run: &FunctionScheduleRunRecord,
        extra: WriteBatch,
    ) -> Result<(), FunctionScheduleError> {
        let mut current = Some(schedule.clone());
        for _ in 0..MAXIMUM_CONCURRENT_RETRIES {
            let Some(previous) = current else {
                break;
            };
            let mut next = previous.clone();
            next.last_run = run.summary();
            if self
                .store
                .replace_schedule(&previous, &next, extra.clone())
                .await?
            {
                return Ok(());
            }
            current = self
                .store
                .get_schedule(&schedule.tenant, &schedule.function_name, &schedule.id)
                .await?;
        }
        if current.is_none() {
            // Deleted meanwhile: only the lease needs releasing.
            return self
                .store
                .delete_keys([FunctionScheduleStore::lease_key(
                    &schedule.tenant,
                    &schedule.id,
                )?])
                .await;
        }
        // Contended beyond patience: keep the run and release the lease
        // without touching the schedule's summary.
        self.store.write_unconditionally(extra).await
    }

    fn invocation_request(
        &self,
        schedule: &FunctionScheduleRecord,
        run: &FunctionScheduleRunRecord,
    ) -> Result<FunctionScheduleInvokeRequest, FunctionScheduleError> {
        let request = &schedule.request;
        let (path, query) = split_path_query(&request.path);
        let mut headers = BTreeMap::new();
        headers.insert("user-agent".to_owned(), USER_AGENT.to_owned());
        headers.insert(
            SCHEDULE_ID_HEADER.to_owned(),
            schedule.id.as_str().to_owned(),
        );
        headers.insert(
            SCHEDULE_RUN_ID_HEADER.to_owned(),
            run.id.as_str().to_owned(),
        );
        headers.insert(
            SCHEDULE_DUE_AT_HEADER.to_owned(),
            format_timestamp(run.due_at_unix_seconds)
                .ok_or(FunctionScheduleError::CorruptRecord)?,
        );
        let body = request.body.as_deref().unwrap_or_default();
        if request.method != FunctionScheduleMethod::Get {
            headers.insert("content-type".to_owned(), request.content_type.clone());
        }
        for (name, value) in &request.headers {
            headers.insert(name.clone(), value.clone());
        }
        Ok(FunctionScheduleInvokeRequest {
            project_id: schedule.tenant.project_id().as_str().to_owned(),
            environment_id: schedule.tenant.environment_id().as_str().to_owned(),
            function_name: schedule.function_name.as_str().to_owned(),
            schedule_id: schedule.id.as_str().to_owned(),
            run_id: run.id.as_str().to_owned(),
            method: request.method.as_str().to_owned(),
            path: path.to_owned(),
            query: query.map(str::to_owned),
            headers,
            body_base64: STANDARD.encode(body.as_bytes()),
        })
    }
}

#[derive(Debug)]
pub enum FunctionScheduleError {
    InvalidConfiguration,
    UnsupportedDurability,
    /// The project or environment does not exist.
    EnvironmentNotFound,
    FunctionNotFound,
    ScheduleNotFound,
    Forbidden,
    Conflict(&'static str),
    InvalidRequest(&'static str),
    InvalidCron(CronError),
    CorruptRecord,
    LimitExceeded,
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for FunctionScheduleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("function schedule configuration is invalid")
            }
            Self::UnsupportedDurability => {
                formatter.write_str("function schedule durability is unsupported")
            }
            Self::EnvironmentNotFound => formatter.write_str("environment was not found"),
            Self::FunctionNotFound => formatter.write_str("function was not found"),
            Self::ScheduleNotFound => formatter.write_str("function schedule was not found"),
            Self::Forbidden => formatter.write_str("function schedule operation is forbidden"),
            Self::Conflict(message) | Self::InvalidRequest(message) => formatter.write_str(message),
            Self::InvalidCron(error) => error.fmt(formatter),
            Self::CorruptRecord => formatter.write_str("function schedule record is corrupt"),
            Self::LimitExceeded => formatter.write_str("function schedule scan exceeded its bound"),
            Self::Project(_) => formatter.write_str("function schedule project lookup failed"),
            Self::Organization(_) => {
                formatter.write_str("function schedule organization lookup failed")
            }
            Self::Keyspace(_) => formatter.write_str("function schedule key is invalid"),
            Self::Storage(_) => formatter.write_str("function schedule storage operation failed"),
            Self::Json(_) => formatter.write_str("function schedule record encoding failed"),
        }
    }
}

impl Error for FunctionScheduleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidCron(error) => Some(error),
            Self::Project(error) => Some(error),
            Self::Organization(error) => Some(error),
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CronError> for FunctionScheduleError {
    fn from(value: CronError) -> Self {
        Self::InvalidCron(value)
    }
}

impl From<ProjectStoreError> for FunctionScheduleError {
    fn from(value: ProjectStoreError) -> Self {
        Self::Project(value)
    }
}

impl From<OrganizationStoreError> for FunctionScheduleError {
    fn from(value: OrganizationStoreError) -> Self {
        Self::Organization(value)
    }
}

impl From<ControlKeyspaceError> for FunctionScheduleError {
    fn from(value: ControlKeyspaceError) -> Self {
        Self::Keyspace(value)
    }
}

impl From<StorageError> for FunctionScheduleError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<serde_json::Error> for FunctionScheduleError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::executor::block_on;
    use serde_json::{Value, json};

    use super::*;
    use crate::email_template::tests::{Fixture, NOW, fixture};

    /// 2027-01-01T00:00:00Z: a Friday, on a whole day, well after the
    /// fixture's `NOW`.
    const START: u64 = 1_798_761_600;
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    #[derive(Clone)]
    struct Invoked {
        tenant: TenantScope,
        request: FunctionScheduleInvokeRequest,
    }

    /// Answers every invocation with `status` (version 3) after taking
    /// `hold` off the worker's clock, or fails with `failure`.
    #[derive(Default)]
    struct StubInvoker {
        invoked: Mutex<Vec<Invoked>>,
        status: Mutex<Option<u16>>,
        failure: Mutex<Option<FunctionScheduleInvokeError>>,
    }

    impl StubInvoker {
        fn answer(&self, status: u16) {
            *self.status.lock().expect("lock") = Some(status);
            *self.failure.lock().expect("lock") = None;
        }

        fn fail(&self, failure: FunctionScheduleInvokeError) {
            *self.failure.lock().expect("lock") = Some(failure);
        }

        fn invocations(&self) -> Vec<Invoked> {
            self.invoked.lock().expect("lock").clone()
        }
    }

    impl FunctionScheduleInvoker for StubInvoker {
        fn invoke(
            &self,
            tenant: &TenantScope,
            request: &FunctionScheduleInvokeRequest,
        ) -> Result<FunctionScheduleInvokeResponse, FunctionScheduleInvokeError> {
            self.invoked.lock().expect("lock").push(Invoked {
                tenant: tenant.clone(),
                request: request.clone(),
            });
            if let Some(failure) = *self.failure.lock().expect("lock") {
                return Err(failure);
            }
            Ok(FunctionScheduleInvokeResponse {
                status: self.status.lock().expect("lock").unwrap_or(200),
                version: 3,
                headers: BTreeMap::from([(
                    "content-type".to_owned(),
                    "application/json".to_owned(),
                )]),
                body_base64: STANDARD.encode(b"{\"ok\":true}"),
                duration_milliseconds: 12,
            })
        }
    }

    struct Harness {
        fixture: Fixture,
        invoker: Arc<StubInvoker>,
        service: FunctionScheduleService,
        worker: FunctionScheduleWorker,
        function: FunctionName,
    }

    /// Built outside any executor: the fixture drives its own `block_on`,
    /// and a nested one panics.
    fn harness() -> Harness {
        let fixture = fixture();
        let invoker = Arc::new(StubInvoker::default());
        let kv: Arc<dyn KvAdapter> = fixture.adapter.clone();
        let store = FunctionScheduleStore::new(kv.clone(), Durability::Memory).expect("store");
        let service = FunctionScheduleService::new(
            store.clone(),
            ProjectStore::new(kv.clone(), Durability::Memory).expect("projects"),
            OrganizationStore::new(kv, Durability::Memory).expect("organizations"),
            fixture.audit.clone(),
        );
        let worker = FunctionScheduleWorker::new(
            store,
            invoker.clone(),
            FunctionScheduleWorkerConfig::default(),
        )
        .expect("worker");
        let function = FunctionName::parse("nightly-report").expect("function name");
        let harness = Harness {
            fixture,
            invoker,
            service,
            worker,
            function,
        };
        block_on(write_function(&harness, "nightly-report", Some(3)));
        harness
    }

    /// Writes a function record the way the function service would, without
    /// deploying a bundle: the schedule module reads only its public shape.
    async fn write_function(harness: &Harness, name: &str, active_version: Option<u64>) {
        let tenant = &harness.fixture.tenant;
        let record = json!({
            "tenant": {
                "projectId": tenant.project_id().as_str(),
                "environmentId": tenant.environment_id().as_str(),
            },
            "name": name,
            "state": "active",
            "activeVersion": active_version,
            "configuration": {
                "verifyJwt": true,
                "regions": ["local"],
                "secretNames": [],
                "limits": {
                    "cpuMilliseconds": 100,
                    "wallMilliseconds": 1000,
                    "memoryBytes": 1_000_000,
                    "requestBytes": 1000,
                    "responseBytes": 1000,
                    "concurrency": 2,
                },
            },
            "createdAtUnixSeconds": NOW,
            "updatedAtUnixSeconds": NOW,
        });
        let key = ControlKeyspace::function_key(
            tenant.project_id(),
            tenant.environment_id(),
            &FunctionName::parse(name).expect("name"),
        )
        .expect("key");
        let mut batch = WriteBatch::with_capacity(1);
        batch.put(key, serde_json::to_vec(&record).expect("json"));
        harness
            .fixture
            .adapter
            .compare_and_write(AtomicWrite::unconditional(batch, Durability::Memory))
            .await
            .expect("function record");
    }

    fn new_schedule(cron: &str) -> NewFunctionSchedule {
        NewFunctionSchedule {
            name: Some("Nightly".to_owned()),
            cron: cron.to_owned(),
            request: Some(FunctionScheduleRequest {
                method: FunctionScheduleMethod::Post,
                path: "/reports?kind=daily".to_owned(),
                headers: BTreeMap::from([("X-Report".to_owned(), "nightly".to_owned())]),
                content_type: "application/json".to_owned(),
                body: Some("{\"day\":\"today\"}".to_owned()),
            }),
            enabled: true,
        }
    }

    async fn create(harness: &Harness, cron: &str, now: u64) -> FunctionScheduleRecord {
        harness
            .service
            .create(
                &harness.fixture.owner,
                &harness.fixture.tenant,
                &harness.function,
                new_schedule(cron),
                now,
            )
            .await
            .expect("schedule created")
    }

    async fn reload(harness: &Harness, id: &FunctionScheduleId) -> FunctionScheduleRecord {
        harness
            .service
            .get(
                &harness.fixture.owner,
                &harness.fixture.tenant,
                &harness.function,
                id,
                NOW,
            )
            .await
            .expect("schedule")
    }

    fn query(
        outcome: Option<FunctionScheduleRunOutcome>,
        limit: usize,
    ) -> FunctionScheduleRunQuery {
        FunctionScheduleRunQuery {
            outcome,
            cursor: None,
            limit: NonZeroUsize::new(limit).expect("limit"),
        }
    }

    async fn runs(harness: &Harness, id: &FunctionScheduleId) -> Vec<FunctionScheduleRunRecord> {
        harness
            .service
            .list_runs(
                &harness.fixture.owner,
                &harness.fixture.tenant,
                &harness.function,
                id,
                &query(None, 200),
                NOW,
            )
            .await
            .expect("history")
            .items
    }

    async fn pass(harness: &Harness, now: u64) -> FunctionScheduleWorkerReport {
        harness.worker.run_once(now).await.expect("worker pass")
    }

    #[test]
    fn a_schedule_targets_an_active_deployment_and_shows_its_next_run() {
        let harness = harness();
        block_on(async {
            let created = create(&harness, "0 3 * * *", START + 90 * MINUTE).await;
            assert!(created.id().as_str().starts_with("sch_"));
            assert_eq!(created.id().as_str().len(), 20);
            assert_eq!(created.name(), "Nightly");
            assert_eq!(created.cron(), "0 3 * * *");
            assert_eq!(created.state(), FunctionScheduleState::Active);
            assert_eq!(
                created.next_run_at_unix_seconds(),
                Some(START + 3 * HOUR),
                "01:30 rolls to 03:00 the same day"
            );
            assert!(created.last_run().is_none());
            assert_eq!(
                created.request().headers,
                BTreeMap::from([("x-report".to_owned(), "nightly".to_owned())]),
                "header names are lowercased"
            );

            // Listing and reading see the same record.
            let listed = harness
                .service
                .list(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    NOW,
                )
                .await
                .expect("list");
            assert_eq!(listed, vec![created.clone()]);
            assert_eq!(reload(&harness, created.id()).await, created);

            // Without an active deployment, creation is a conflict; a missing
            // function is not found.
            write_function(&harness, "undeployed", None).await;
            let refused = harness
                .service
                .create(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &FunctionName::parse("undeployed").expect("name"),
                    new_schedule("0 3 * * *"),
                    NOW,
                )
                .await
                .expect_err("no active deployment");
            assert!(matches!(
                refused,
                FunctionScheduleError::Conflict("function has no active deployment")
            ));
            let missing = harness
                .service
                .create(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &FunctionName::parse("nowhere").expect("name"),
                    new_schedule("0 3 * * *"),
                    NOW,
                )
                .await
                .expect_err("no such function");
            assert!(matches!(missing, FunctionScheduleError::FunctionNotFound));
            assert!(matches!(
                harness
                    .service
                    .get(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &harness.function,
                        &FunctionScheduleId::generate(),
                        NOW,
                    )
                    .await
                    .expect_err("unknown schedule"),
                FunctionScheduleError::ScheduleNotFound
            ));
        });
    }

    #[test]
    fn expressions_and_requests_are_validated_at_save_time() {
        let harness = harness();
        block_on(async {
            let attempt = |input: NewFunctionSchedule| {
                let harness = &harness;
                async move {
                    harness
                        .service
                        .create(
                            &harness.fixture.owner,
                            &harness.fixture.tenant,
                            &harness.function,
                            input,
                            NOW,
                        )
                        .await
                        .expect_err("refused")
                }
            };
            let bad_cron = attempt(new_schedule("61 * * * *")).await;
            assert_eq!(
                bad_cron.to_string(),
                "cron expression is invalid at minute: value is out of range"
            );
            let never = attempt(new_schedule("0 0 31 2 *")).await;
            assert!(matches!(
                never,
                FunctionScheduleError::InvalidRequest(
                    "cron expression has no due time within the next five years"
                )
            ));
            let with_request = |request: FunctionScheduleRequest| NewFunctionSchedule {
                request: Some(request),
                ..new_schedule("0 3 * * *")
            };
            let refused = |request: FunctionScheduleRequest| {
                let input = with_request(request);
                let attempt = &attempt;
                async move {
                    match attempt(input).await {
                        FunctionScheduleError::InvalidRequest(message) => message,
                        other => panic!("expected an invalid request, got {other:?}"),
                    }
                }
            };
            let base = FunctionScheduleRequest::default();
            assert!(
                refused(FunctionScheduleRequest {
                    path: "reports".to_owned(),
                    ..base.clone()
                })
                .await
                .starts_with("request path must begin with `/`")
            );
            assert!(
                refused(FunctionScheduleRequest {
                    path: "/reports/../secret".to_owned(),
                    ..base.clone()
                })
                .await
                .starts_with("request path segments")
            );
            assert!(
                refused(FunctionScheduleRequest {
                    path: "/reports/".to_owned(),
                    ..base.clone()
                })
                .await
                .starts_with("request path segments")
            );
            assert!(
                refused(FunctionScheduleRequest {
                    path: format!("/{}", "a".repeat(MAXIMUM_PATH_BYTES)),
                    ..base.clone()
                })
                .await
                .starts_with("request path must begin")
            );
            for name in [
                "Authorization",
                "host",
                "Content-Length",
                "content-type",
                "x-mako-anything",
            ] {
                assert!(
                    refused(FunctionScheduleRequest {
                        headers: BTreeMap::from([(name.to_owned(), "x".to_owned())]),
                        ..base.clone()
                    })
                    .await
                    .starts_with("request headers may not set"),
                    "{name}"
                );
            }
            assert!(
                refused(FunctionScheduleRequest {
                    headers: (0..17)
                        .map(|index| (format!("x-h{index}"), "v".to_owned()))
                        .collect(),
                    ..base.clone()
                })
                .await
                .starts_with("request headers must number")
            );
            assert!(
                refused(FunctionScheduleRequest {
                    headers: BTreeMap::from([("x-a".to_owned(), "a\nb".to_owned())]),
                    ..base.clone()
                })
                .await
                .starts_with("request header values")
            );
            assert!(
                refused(FunctionScheduleRequest {
                    headers: BTreeMap::from([
                        ("X-Dup".to_owned(), "a".to_owned()),
                        ("x-dup".to_owned(), "b".to_owned()),
                    ]),
                    ..base.clone()
                })
                .await
                .starts_with("request headers must not repeat")
            );
            assert!(
                refused(FunctionScheduleRequest {
                    body: Some("x".repeat(MAXIMUM_BODY_BYTES + 1)),
                    ..base.clone()
                })
                .await
                .starts_with("request body must be at most")
            );
            assert_eq!(
                refused(FunctionScheduleRequest {
                    method: FunctionScheduleMethod::Get,
                    body: Some("{}".to_owned()),
                    ..base.clone()
                })
                .await,
                "a GET request carries no body"
            );
            assert!(
                refused(FunctionScheduleRequest {
                    content_type: "json".to_owned(),
                    ..base.clone()
                })
                .await
                .starts_with("request contentType")
            );
            let long_name = attempt(NewFunctionSchedule {
                name: Some("n".repeat(MAXIMUM_NAME_BYTES + 1)),
                ..new_schedule("0 3 * * *")
            })
            .await;
            assert!(matches!(
                long_name,
                FunctionScheduleError::InvalidRequest(_)
            ));

            // A body-less GET at the root with default everything is fine.
            harness
                .service
                .create(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    NewFunctionSchedule {
                        name: None,
                        cron: "*/5 * * * *".to_owned(),
                        request: Some(FunctionScheduleRequest {
                            method: FunctionScheduleMethod::Get,
                            ..FunctionScheduleRequest::default()
                        }),
                        enabled: true,
                    },
                    NOW,
                )
                .await
                .expect("valid minimal schedule");
        });
    }

    #[test]
    fn update_recomputes_the_next_run_and_pausing_nulls_it() {
        let harness = harness();
        block_on(async {
            let created = create(&harness, "0 3 * * *", START).await;
            assert_eq!(created.next_run_at_unix_seconds(), Some(START + 3 * HOUR));

            // Renaming alone keeps the next run.
            let renamed = harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    FunctionScheduleUpdate {
                        name: Some("Renamed".to_owned()),
                        ..FunctionScheduleUpdate::default()
                    },
                    START + HOUR,
                )
                .await
                .expect("renamed");
            assert_eq!(renamed.name(), "Renamed");
            assert_eq!(renamed.next_run_at_unix_seconds(), Some(START + 3 * HOUR));
            assert_eq!(renamed.updated_at_unix_seconds(), START + HOUR);

            // A new expression recomputes from now.
            let hourly = harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    FunctionScheduleUpdate {
                        cron: Some("30 * * * *".to_owned()),
                        ..FunctionScheduleUpdate::default()
                    },
                    START + HOUR,
                )
                .await
                .expect("hourly");
            assert_eq!(
                hourly.next_run_at_unix_seconds(),
                Some(START + HOUR + 30 * MINUTE)
            );

            // Pausing nulls the next run and empties the due index; the
            // worker then has nothing to do.
            let paused = harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    FunctionScheduleUpdate {
                        enabled: Some(false),
                        ..FunctionScheduleUpdate::default()
                    },
                    START + HOUR,
                )
                .await
                .expect("paused");
            assert_eq!(paused.state(), FunctionScheduleState::Paused);
            assert_eq!(paused.next_run_at_unix_seconds(), None);
            let report = pass(&harness, START + DAY).await;
            assert_eq!(report.due, 0);
            assert!(harness.invoker.invocations().is_empty());

            // Resuming recomputes from now, and requires a deployment.
            let resumed = harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    FunctionScheduleUpdate {
                        enabled: Some(true),
                        ..FunctionScheduleUpdate::default()
                    },
                    START + 2 * HOUR,
                )
                .await
                .expect("resumed");
            assert_eq!(
                resumed.next_run_at_unix_seconds(),
                Some(START + 2 * HOUR + 30 * MINUTE)
            );
            harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    FunctionScheduleUpdate {
                        enabled: Some(false),
                        ..FunctionScheduleUpdate::default()
                    },
                    START + 2 * HOUR,
                )
                .await
                .expect("paused again");
            write_function(&harness, "nightly-report", None).await;
            assert!(matches!(
                harness
                    .service
                    .update(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &harness.function,
                        created.id(),
                        FunctionScheduleUpdate {
                            enabled: Some(true),
                            ..FunctionScheduleUpdate::default()
                        },
                        START + 2 * HOUR,
                    )
                    .await
                    .expect_err("undeployed"),
                FunctionScheduleError::Conflict("function has no active deployment")
            ));
            write_function(&harness, "nightly-report", Some(3)).await;

            // Empty and invalid updates are refused without touching the record.
            assert!(matches!(
                harness
                    .service
                    .update(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &harness.function,
                        created.id(),
                        FunctionScheduleUpdate::default(),
                        NOW,
                    )
                    .await
                    .expect_err("empty"),
                FunctionScheduleError::InvalidRequest("update changes nothing")
            ));
            assert!(matches!(
                harness
                    .service
                    .update(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &harness.function,
                        created.id(),
                        FunctionScheduleUpdate {
                            cron: Some("bad".to_owned()),
                            ..FunctionScheduleUpdate::default()
                        },
                        NOW,
                    )
                    .await
                    .expect_err("bad cron"),
                FunctionScheduleError::InvalidCron(_)
            ));

            // Deleting removes the schedule and its history.
            harness
                .service
                .delete(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    NOW,
                )
                .await
                .expect("deleted");
            assert!(matches!(
                harness
                    .service
                    .get(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &harness.function,
                        created.id(),
                        NOW,
                    )
                    .await
                    .expect_err("gone"),
                FunctionScheduleError::ScheduleNotFound
            ));
            assert!(
                harness
                    .worker
                    .store()
                    .registry()
                    .await
                    .expect("registry")
                    .is_empty()
            );
        });
    }

    #[test]
    fn a_due_schedule_is_invoked_with_its_request_and_the_run_recorded() {
        let harness = harness();
        block_on(async {
            let created = create(&harness, "0 3 * * *", START).await;
            let due = START + 3 * HOUR;

            // Not yet due: nothing happens.
            let early = pass(&harness, due - 1).await;
            assert_eq!(early.due, 0);
            assert!(harness.invoker.invocations().is_empty());

            // Due: invoked once, with the configured request and the
            // scheduler's own headers.
            let report = pass(&harness, due + 4).await;
            assert_eq!((report.due, report.started, report.succeeded), (1, 1, 1));
            let invocations = harness.invoker.invocations();
            assert_eq!(invocations.len(), 1);
            let invoked = &invocations[0];
            assert_eq!(invoked.tenant, harness.fixture.tenant);
            let request = &invoked.request;
            assert_eq!(
                request.project_id,
                harness.fixture.tenant.project_id().as_str()
            );
            assert_eq!(request.function_name, "nightly-report");
            assert_eq!(request.schedule_id, created.id().as_str());
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/reports");
            assert_eq!(request.query.as_deref(), Some("kind=daily"));
            assert_eq!(
                STANDARD.decode(&request.body_base64).expect("base64"),
                b"{\"day\":\"today\"}"
            );
            assert_eq!(
                request.headers.get("content-type").map(String::as_str),
                Some("application/json")
            );
            assert_eq!(
                request.headers.get("x-report").map(String::as_str),
                Some("nightly")
            );
            assert_eq!(
                request.headers.get(SCHEDULE_ID_HEADER).map(String::as_str),
                Some(created.id().as_str())
            );
            assert_eq!(
                request
                    .headers
                    .get(SCHEDULE_RUN_ID_HEADER)
                    .map(String::as_str),
                Some(request.run_id.as_str())
            );
            assert_eq!(
                request
                    .headers
                    .get(SCHEDULE_DUE_AT_HEADER)
                    .map(String::as_str),
                Some("2027-01-01T03:00:00Z")
            );
            assert_eq!(
                request.headers.get("user-agent").map(String::as_str),
                Some(USER_AGENT)
            );

            // The run is recorded and the schedule moved on.
            let history = runs(&harness, created.id()).await;
            assert_eq!(history.len(), 1);
            let run = &history[0];
            assert!(run.id().as_str().starts_with("run_"));
            assert_eq!(run.id().as_str(), request.run_id);
            assert_eq!(run.due_at_unix_seconds(), due);
            assert_eq!(run.started_at_unix_seconds(), Some(due + 4));
            assert_eq!(run.completed_at_unix_seconds(), Some(due + 4));
            assert_eq!(run.outcome(), Some(FunctionScheduleRunOutcome::Succeeded));
            assert_eq!(run.response_status(), Some(200));
            assert_eq!(run.function_version(), Some(3));
            assert!(run.duration_milliseconds().is_some());
            assert!(run.error().is_none());
            assert!(!run.manual());
            let schedule = reload(&harness, created.id()).await;
            assert_eq!(schedule.next_run_at_unix_seconds(), Some(due + DAY));
            let last = schedule.last_run().expect("last run");
            assert_eq!(last.id, *run.id());
            assert_eq!(last.outcome, FunctionScheduleRunOutcome::Succeeded);
            assert_eq!(last.response_status, Some(200));
            assert!(
                harness
                    .worker
                    .store()
                    .lease(&harness.fixture.tenant, created.id(), due + 4)
                    .await
                    .expect("lease")
                    .is_none(),
                "the lease is released on completion"
            );

            // The same due time never fires twice.
            let again = pass(&harness, due + 10).await;
            assert_eq!(again.due, 0);
            assert_eq!(harness.invoker.invocations().len(), 1);
        });
    }

    #[test]
    fn outcomes_follow_the_status_or_the_failure_reason() {
        let harness = harness();
        block_on(async {
            let created = create(&harness, "*/10 * * * *", START).await;
            let mut now = START + 10 * MINUTE;
            harness.invoker.answer(500);
            pass(&harness, now).await;
            now += 10 * MINUTE;
            harness.invoker.answer(302);
            pass(&harness, now).await;
            for failure in [
                FunctionScheduleInvokeError::Timeout,
                FunctionScheduleInvokeError::NoActiveDeployment,
                FunctionScheduleInvokeError::GatewayUnavailable,
                FunctionScheduleInvokeError::InvalidRequest,
                FunctionScheduleInvokeError::Throttled,
            ] {
                now += 10 * MINUTE;
                harness.invoker.fail(failure);
                let report = pass(&harness, now).await;
                assert_eq!(report.errored, 1);
            }
            now += 10 * MINUTE;
            harness.invoker.answer(204);
            pass(&harness, now).await;

            let history = runs(&harness, created.id()).await;
            let outcomes: Vec<(
                Option<FunctionScheduleRunOutcome>,
                Option<u16>,
                Option<&str>,
            )> = history
                .iter()
                .map(|run| (run.outcome(), run.response_status(), run.error()))
                .collect();
            assert_eq!(
                outcomes,
                vec![
                    (Some(FunctionScheduleRunOutcome::Succeeded), Some(204), None),
                    (
                        Some(FunctionScheduleRunOutcome::Error),
                        None,
                        Some("throttled")
                    ),
                    (
                        Some(FunctionScheduleRunOutcome::Error),
                        None,
                        Some("invalid_request")
                    ),
                    (
                        Some(FunctionScheduleRunOutcome::Error),
                        None,
                        Some("gateway_unavailable")
                    ),
                    (
                        Some(FunctionScheduleRunOutcome::Error),
                        None,
                        Some("no_active_deployment")
                    ),
                    (
                        Some(FunctionScheduleRunOutcome::Error),
                        None,
                        Some("timeout")
                    ),
                    (Some(FunctionScheduleRunOutcome::Failed), Some(302), None),
                    (Some(FunctionScheduleRunOutcome::Failed), Some(500), None),
                ],
                "newest first"
            );
            assert!(
                history
                    .iter()
                    .filter(|run| run.outcome() == Some(FunctionScheduleRunOutcome::Error))
                    .all(|run| run.function_version().is_none())
            );

            // The outcome filter and paging.
            let errors = harness
                .service
                .list_runs(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    &query(Some(FunctionScheduleRunOutcome::Error), 3),
                    NOW,
                )
                .await
                .expect("errors");
            assert_eq!(errors.items.len(), 3);
            let cursor = errors.next_cursor.expect("more errors");
            let rest = harness
                .service
                .list_runs(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    &FunctionScheduleRunQuery {
                        outcome: Some(FunctionScheduleRunOutcome::Error),
                        cursor: Some(cursor),
                        limit: NonZeroUsize::new(3).expect("limit"),
                    },
                    NOW,
                )
                .await
                .expect("rest");
            assert_eq!(rest.items.len(), 2);
            assert!(rest.next_cursor.is_none());
            assert_eq!(
                rest.items[0].error(),
                Some("no_active_deployment"),
                "paging continues after the cursor"
            );
            assert!(matches!(
                harness
                    .service
                    .list_runs(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &harness.function,
                        created.id(),
                        &FunctionScheduleRunQuery {
                            outcome: None,
                            cursor: Some("not-a-cursor".to_owned()),
                            limit: NonZeroUsize::new(3).expect("limit"),
                        },
                        NOW,
                    )
                    .await
                    .expect_err("bad cursor"),
                FunctionScheduleError::InvalidRequest("cursor is invalid")
            ));
        });
    }

    #[test]
    fn a_held_lease_skips_the_next_due_time_and_the_schedule_continues() {
        let harness = harness();
        block_on(async {
            let created = create(&harness, "*/5 * * * *", START).await;
            let first = START + 5 * MINUTE;
            // A run that is still executing: simulate the lease a running
            // invocation holds without completing it.
            let store = harness.worker.store();
            let lease = LeaseRecord {
                run_id: FunctionScheduleRunId::generate(),
                expires_at_unix_seconds: first + 90,
            };
            let mut batch = WriteBatch::with_capacity(1);
            FunctionScheduleStore::put_lease(
                &mut batch,
                &harness.fixture.tenant,
                created.id(),
                &lease,
            )
            .expect("lease");
            store.write_unconditionally(batch).await.expect("write");

            let report = pass(&harness, first + 1).await;
            assert_eq!(
                (report.due, report.started, report.skipped_overlap),
                (1, 0, 1)
            );
            assert!(harness.invoker.invocations().is_empty());
            let history = runs(&harness, created.id()).await;
            assert_eq!(history.len(), 1);
            assert_eq!(
                history[0].outcome(),
                Some(FunctionScheduleRunOutcome::SkippedOverlap)
            );
            assert_eq!(history[0].due_at_unix_seconds(), first);
            assert!(history[0].started_at_unix_seconds().is_none());
            let schedule = reload(&harness, created.id()).await;
            assert_eq!(
                schedule.next_run_at_unix_seconds(),
                Some(first + 5 * MINUTE),
                "the schedule continues after the skip"
            );
            assert_eq!(
                schedule.last_run().map(|run| run.outcome),
                Some(FunctionScheduleRunOutcome::SkippedOverlap)
            );

            // Run-now is refused while the lease is held.
            assert!(matches!(
                harness
                    .service
                    .run_now(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &harness.function,
                        created.id(),
                        first + 2,
                    )
                    .await
                    .expect_err("executing"),
                FunctionScheduleError::Conflict("a run of this schedule is still executing")
            ));

            // Once the lease expires (a worker that died mid-run), the next
            // due time runs normally.
            let second = first + 5 * MINUTE;
            let report = pass(&harness, lease.expires_at_unix_seconds.max(second)).await;
            assert_eq!((report.started, report.succeeded), (1, 1));
            assert_eq!(harness.invoker.invocations().len(), 1);
        });
    }

    #[test]
    fn missed_due_times_run_once_for_the_latest_slot() {
        let harness = harness();
        block_on(async {
            let created = create(&harness, "*/15 * * * *", START).await;
            // The worker was down for two days: 192 missed slots.
            let resumed = START + 2 * DAY + 7 * MINUTE;
            let report = pass(&harness, resumed).await;
            assert_eq!((report.due, report.started), (1, 1));
            let history = runs(&harness, created.id()).await;
            assert_eq!(history.len(), 1, "one run, not one per missed slot");
            assert_eq!(
                history[0].due_at_unix_seconds(),
                START + 2 * DAY,
                "for the latest missed due time"
            );
            let schedule = reload(&harness, created.id()).await;
            assert_eq!(
                schedule.next_run_at_unix_seconds(),
                Some(START + 2 * DAY + 15 * MINUTE),
                "and the next is computed from now"
            );
        });
    }

    #[test]
    fn run_now_queues_a_manual_run_the_worker_executes() {
        let harness = harness();
        block_on(async {
            let created = create(&harness, "0 3 * * *", START).await;
            let queued = harness
                .service
                .run_now(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    START + MINUTE,
                )
                .await
                .expect("queued");
            assert!(queued.manual());
            assert_eq!(queued.outcome(), None);
            assert_eq!(queued.started_at_unix_seconds(), None);
            assert_eq!(queued.due_at_unix_seconds(), START + MINUTE);
            assert_eq!(runs(&harness, created.id()).await, vec![queued.clone()]);

            let report = pass(&harness, START + MINUTE + 3).await;
            assert_eq!((report.due, report.started, report.succeeded), (1, 1, 1));
            let history = runs(&harness, created.id()).await;
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].id(), queued.id());
            assert!(history[0].manual());
            assert_eq!(
                history[0].started_at_unix_seconds(),
                Some(START + MINUTE + 3)
            );
            assert_eq!(
                history[0].outcome(),
                Some(FunctionScheduleRunOutcome::Succeeded)
            );
            let schedule = reload(&harness, created.id()).await;
            assert_eq!(
                schedule.next_run_at_unix_seconds(),
                Some(START + 3 * HOUR),
                "a manual run does not move the cron"
            );
            assert_eq!(
                schedule.last_run().map(|run| run.id.clone()),
                Some(queued.id().clone())
            );
            assert_eq!(
                harness.invoker.invocations()[0].request.run_id,
                queued.id().as_str()
            );

            // A second pass finds nothing queued.
            assert_eq!(pass(&harness, START + MINUTE + 10).await.due, 0);

            // Run-now on a paused schedule still queues; on an undeployed
            // function it is refused.
            harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    FunctionScheduleUpdate {
                        enabled: Some(false),
                        ..FunctionScheduleUpdate::default()
                    },
                    START + 2 * MINUTE,
                )
                .await
                .expect("paused");
            harness
                .service
                .run_now(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    START + 2 * MINUTE,
                )
                .await
                .expect("queued while paused");
            write_function(&harness, "nightly-report", None).await;
            assert!(matches!(
                harness
                    .service
                    .run_now(
                        &harness.fixture.owner,
                        &harness.fixture.tenant,
                        &harness.function,
                        created.id(),
                        START + 2 * MINUTE,
                    )
                    .await
                    .expect_err("undeployed"),
                FunctionScheduleError::Conflict("function has no active deployment")
            ));
        });
    }

    #[test]
    fn retention_prunes_old_and_excess_runs_but_never_a_queued_one() {
        let harness = harness();
        block_on(async {
            let store = harness.worker.store().clone();
            let worker = FunctionScheduleWorker::new(
                store.clone(),
                harness.invoker.clone(),
                FunctionScheduleWorkerConfig {
                    maximum_runs_per_schedule: 5,
                    retention_seconds: 10 * DAY,
                    ..FunctionScheduleWorkerConfig::default()
                },
            )
            .expect("worker");
            let created = create(&harness, "0 * * * *", START).await;
            let mut now = START;
            for _ in 0..8 {
                now += HOUR;
                worker.run_once(now).await.expect("pass");
            }
            let history = runs(&harness, created.id()).await;
            assert_eq!(history.len(), 5, "the ceiling holds, oldest pruned");
            assert_eq!(history[0].due_at_unix_seconds(), START + 8 * HOUR);
            assert_eq!(history[4].due_at_unix_seconds(), START + 4 * HOUR);

            // Ten days later everything completed has expired; a queued
            // manual run is untouched.
            let queued = harness
                .service
                .run_now(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    now + 11 * DAY,
                )
                .await
                .expect("queued");
            harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    FunctionScheduleUpdate {
                        enabled: Some(false),
                        ..FunctionScheduleUpdate::default()
                    },
                    now + 11 * DAY,
                )
                .await
                .expect("paused so the cron does not fire");
            // The pass executes the manual run and then prunes: everything
            // older than the window goes, the run that just completed stays.
            let report = worker.run_once(now + 11 * DAY).await.expect("pass");
            assert!(report.cleaned >= 5);
            let history = runs(&harness, created.id()).await;
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].id(), queued.id());
            assert_eq!(
                history[0].outcome(),
                Some(FunctionScheduleRunOutcome::Succeeded)
            );
        });
    }

    #[test]
    fn membership_decides_who_reads_and_who_writes_and_mutations_are_audited() {
        let harness = harness();
        block_on(async {
            let created = create(&harness, "0 3 * * *", START).await;
            // A viewer may read...
            harness
                .service
                .list(
                    &harness.fixture.viewer,
                    &harness.fixture.tenant,
                    &harness.function,
                    NOW,
                )
                .await
                .expect("viewer reads");
            // ...but not write.
            assert!(matches!(
                harness
                    .service
                    .create(
                        &harness.fixture.viewer,
                        &harness.fixture.tenant,
                        &harness.function,
                        new_schedule("0 3 * * *"),
                        NOW,
                    )
                    .await
                    .expect_err("viewer"),
                FunctionScheduleError::Forbidden
            ));
            assert!(matches!(
                harness
                    .service
                    .run_now(
                        &harness.fixture.viewer,
                        &harness.fixture.tenant,
                        &harness.function,
                        created.id(),
                        NOW,
                    )
                    .await
                    .expect_err("viewer"),
                FunctionScheduleError::Forbidden
            ));
            // A stranger sees nothing.
            assert!(matches!(
                harness
                    .service
                    .list(
                        &harness.fixture.stranger,
                        &harness.fixture.tenant,
                        &harness.function,
                        NOW,
                    )
                    .await
                    .expect_err("stranger"),
                FunctionScheduleError::Forbidden
            ));
            harness
                .service
                .update(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    FunctionScheduleUpdate {
                        enabled: Some(false),
                        ..FunctionScheduleUpdate::default()
                    },
                    NOW,
                )
                .await
                .expect("owner writes");
            harness
                .service
                .delete(
                    &harness.fixture.owner,
                    &harness.fixture.tenant,
                    &harness.function,
                    created.id(),
                    NOW,
                )
                .await
                .expect("owner deletes");
            let events = harness.fixture.audit.0.lock().expect("audit");
            let actions: Vec<(&str, ControlAuditOutcome)> = events
                .iter()
                .map(|event| (event.action.as_str(), event.outcome))
                .collect();
            assert!(actions.contains(&("function_schedule_create", ControlAuditOutcome::Allowed)));
            assert!(actions.contains(&("function_schedule_create", ControlAuditOutcome::Denied)));
            assert!(actions.contains(&("function_schedule_run_now", ControlAuditOutcome::Denied)));
            assert!(actions.contains(&("function_schedule_read", ControlAuditOutcome::Denied)));
            assert!(actions.contains(&("function_schedule_update", ControlAuditOutcome::Allowed)));
            assert!(actions.contains(&("function_schedule_delete", ControlAuditOutcome::Allowed)));
            let create_event = events
                .iter()
                .find(|event| {
                    event.action == ControlAuditAction::FunctionScheduleCreate
                        && event.outcome == ControlAuditOutcome::Allowed
                })
                .expect("create event");
            assert!(create_event.target.ends_with(&format!(
                "/function-schedules/nightly-report/{}",
                created.id().as_str()
            )));
        });
    }

    #[test]
    fn records_and_ids_keep_their_public_shapes() {
        let id = FunctionScheduleId::generate();
        assert!(id.as_str().starts_with("sch_"));
        assert_eq!(id.as_str().len(), 20);
        assert!(FunctionScheduleId::parse("sch_abcdefghijkl").is_ok());
        assert!(FunctionScheduleId::parse("sch_ABCDEFGHIJKL").is_err());
        assert!(FunctionScheduleId::parse("run_abcdefghijkl").is_err());
        assert!(FunctionScheduleRunId::parse("run_abcdefghijklmnop").is_ok());
        assert!(FunctionScheduleRunId::parse("run_short").is_err());

        let request: FunctionScheduleRequest = serde_json::from_value(json!({})).expect("defaults");
        assert_eq!(request, FunctionScheduleRequest::default());
        assert_eq!(
            serde_json::to_value(&request).expect("json"),
            json!({
                "method": "POST",
                "path": "/",
                "headers": {},
                "contentType": "application/json",
            }),
            "an absent body is omitted, not null"
        );
        assert!(
            serde_json::from_value::<FunctionScheduleRequest>(json!({ "method": "TRACE" }))
                .is_err()
        );
        assert!(
            serde_json::from_value::<FunctionScheduleRequest>(json!({ "query": "a=b" })).is_err(),
            "unknown fields are refused"
        );
        assert_eq!(
            serde_json::to_value(FunctionScheduleRunOutcome::SkippedOverlap).expect("json"),
            Value::String("skipped_overlap".to_owned())
        );
        assert_eq!(
            FunctionScheduleRunOutcome::parse("skipped_overlap"),
            Some(FunctionScheduleRunOutcome::SkippedOverlap)
        );
        assert_eq!(FunctionScheduleRunOutcome::parse("pending"), None);
        assert_eq!(FunctionScheduleMethod::Delete.as_str(), "DELETE");
        assert_eq!(
            FunctionScheduleInvokeError::NoActiveDeployment.stable_code(),
            "no_active_deployment"
        );
    }
}
