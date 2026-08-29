//! The observability wire model.
//!
//! These types are the contract between the services that observe work and the
//! telemetry service that retains it, so they live here rather than in either
//! side. The data plane emits records; the control plane queries them back.

use serde::{Deserialize, Serialize};

use crate::TenantScope;

/// Why a query is not a well-formed observability query.
///
/// Deliberately narrow: the service layer that owns authorization, retention,
/// and storage failures converts this into its own richer error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservabilityQueryError {
    InvalidQuery,
}

impl core::fmt::Display for ObservabilityQueryError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("observability query is invalid")
    }
}

impl core::error::Error for ObservabilityQueryError {}

fn safe_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

/// How long the enforcement window behind a `Quota` checkpoint record is.
///
/// A checkpoint's timestamp is the start of the window it summarizes, and the
/// window's length is this constant on both sides of the wire: the plane that
/// reads its quota counters and the store that compares them against the
/// usage ledger must agree on it, or the comparison sums the wrong minute.
pub const QUOTA_CHECKPOINT_WINDOW_MILLISECONDS: u64 = 60_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaResource {
    Environments,
    CollectionsPerEnvironment,
    StorageBytes,
    ReplicationRequestsPerMinute,
    ReplicationBytesPerMonth,
    ApplicationUsers,
    EdgeFunctions,
    EdgeInvocationsPerMonth,
    EdgeComputeMillisecondsPerMonth,
    LogBytesPerMonth,
    /// Bytes currently held in an environment's buckets: a level, sampled by
    /// the data plane the way `StorageBytes` is.
    ObjectStorageBytes,
    /// Bytes served by object downloads: a flow, recorded per request the way
    /// `ReplicationBytesPerMonth` is.
    ObjectEgressBytesPerMonth,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservabilitySignal {
    Usage,
    Quota,
    Health,
    ReplicationError,
    AuthenticationEvent,
    FunctionMetric,
    ProjectLog,
    IndexState,
    Audit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservabilityQuery {
    pub cursor: Option<String>,
    pub from_unix_milliseconds: Option<u64>,
    pub until_unix_milliseconds: Option<u64>,
    pub limit: usize,
}

impl ObservabilityQuery {
    pub fn validate(&self, now_unix_milliseconds: u64) -> Result<(), ObservabilityQueryError> {
        if self.limit == 0
            || self.limit > 1_000
            || self.cursor.as_ref().is_some_and(|cursor| {
                cursor.is_empty() || cursor.len() > 4_096 || cursor.chars().any(char::is_control)
            })
            || self
                .from_unix_milliseconds
                .is_some_and(|from| from > now_unix_milliseconds)
            || self
                .until_unix_milliseconds
                .is_some_and(|until| until > now_unix_milliseconds)
            || matches!(
                (self.from_unix_milliseconds, self.until_unix_milliseconds),
                (Some(from), Some(until)) if from > until
            )
        {
            return Err(ObservabilityQueryError::InvalidQuery);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RetentionWindow {
    pub retained_from_unix_milliseconds: u64,
    pub observed_at_unix_milliseconds: u64,
    pub retention_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservabilityPayload {
    Usage {
        resource: QuotaResource,
        quantity: u64,
        unit: String,
    },
    Quota {
        resource: QuotaResource,
        limit: u64,
        consumed: u64,
        retry_after_unix_milliseconds: Option<u64>,
    },
    Health {
        service: String,
        region: String,
        status: HealthState,
        diagnostic: Option<String>,
    },
    ReplicationError {
        collection_id: String,
        category: String,
        retryable: bool,
        message: String,
        correlation_id: String,
    },
    AuthenticationEvent {
        category: String,
        outcome: EventOutcome,
        application_user_id: Option<String>,
        message: String,
        correlation_id: String,
    },
    FunctionMetric {
        function_name: String,
        version: u64,
        region: String,
        invocation_count: u64,
        error_count: u64,
        latency_milliseconds: u64,
        compute_milliseconds: u64,
    },
    ProjectLog {
        source: String,
        level: String,
        message: String,
        correlation_id: String,
    },
    IndexState {
        collection_id: String,
        index_name: String,
        index_version: u64,
        state: String,
        progress_percent: u8,
        message: Option<String>,
    },
    Audit {
        /// Renamed on the wire when organizations became teams; records
        /// stored under the old name still read.
        #[serde(rename = "teamId", alias = "organizationId")]
        organization_id: String,
        actor_id: String,
        action: String,
        target: String,
        outcome: EventOutcome,
        request_id: String,
        details: Option<String>,
    },
}

impl ObservabilityPayload {
    #[must_use]
    pub const fn signal(&self) -> ObservabilitySignal {
        match self {
            Self::Usage { .. } => ObservabilitySignal::Usage,
            Self::Quota { .. } => ObservabilitySignal::Quota,
            Self::Health { .. } => ObservabilitySignal::Health,
            Self::ReplicationError { .. } => ObservabilitySignal::ReplicationError,
            Self::AuthenticationEvent { .. } => ObservabilitySignal::AuthenticationEvent,
            Self::FunctionMetric { .. } => ObservabilitySignal::FunctionMetric,
            Self::ProjectLog { .. } => ObservabilitySignal::ProjectLog,
            Self::IndexState { .. } => ObservabilitySignal::IndexState,
            Self::Audit { .. } => ObservabilitySignal::Audit,
        }
    }

    #[must_use]
    pub fn is_safe(&self) -> bool {
        match self {
            Self::Usage { unit, .. } => safe_text(unit, 64),
            Self::Quota { .. } => true,
            Self::Health {
                service,
                region,
                diagnostic,
                ..
            } => {
                safe_text(service, 128)
                    && safe_text(region, 64)
                    && diagnostic
                        .as_ref()
                        .is_none_or(|value| safe_text(value, 1_024))
            }
            Self::ReplicationError {
                collection_id,
                category,
                message,
                correlation_id,
                ..
            } => {
                safe_text(collection_id, 128)
                    && safe_text(category, 128)
                    && safe_text(message, 4_096)
                    && safe_text(correlation_id, 128)
            }
            Self::AuthenticationEvent {
                category,
                application_user_id,
                message,
                correlation_id,
                ..
            } => {
                safe_text(category, 128)
                    && application_user_id
                        .as_ref()
                        .is_none_or(|value| safe_text(value, 128))
                    && safe_text(message, 4_096)
                    && safe_text(correlation_id, 128)
            }
            Self::FunctionMetric {
                function_name,
                region,
                version,
                ..
            } => *version > 0 && safe_text(function_name, 63) && safe_text(region, 64),
            Self::ProjectLog {
                source,
                level,
                message,
                correlation_id,
            } => {
                safe_text(source, 128)
                    && safe_text(level, 32)
                    && safe_text(message, 4_096)
                    && safe_text(correlation_id, 128)
            }
            Self::IndexState {
                collection_id,
                index_name,
                index_version,
                state,
                progress_percent,
                message,
            } => {
                *index_version > 0
                    && *progress_percent <= 100
                    && safe_text(collection_id, 128)
                    && safe_text(index_name, 128)
                    && safe_text(state, 64)
                    && message.as_ref().is_none_or(|value| safe_text(value, 1_024))
            }
            Self::Audit {
                organization_id,
                actor_id,
                action,
                target,
                request_id,
                details,
                ..
            } => {
                safe_text(organization_id, 128)
                    && safe_text(actor_id, 128)
                    && safe_text(action, 128)
                    && safe_text(target, 1_024)
                    && safe_text(request_id, 128)
                    && details.as_ref().is_none_or(|value| safe_text(value, 2_048))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventOutcome {
    Allowed,
    Denied,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservabilityRecord {
    pub tenant: TenantScope,
    pub timestamp_unix_milliseconds: u64,
    pub payload: ObservabilityPayload,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservabilityPage {
    pub items: Vec<ObservabilityRecord>,
    pub next_cursor: Option<String>,
    pub retention: RetentionWindow,
}

/// The telemetry transport contract.
///
/// Services that observe work POST batches here; the telemetry service retains
/// them and answers queries. `source` and `offset` together identify a batch,
/// so a redelivery after an ambiguous failure is recognisable rather than
/// double-counted.
pub const TELEMETRY_PROTOCOL_VERSION: u16 = 1;
pub const TELEMETRY_VERSION_HEADER: &str = "x-mako-telemetry-version";
pub const TELEMETRY_REQUEST_ID_HEADER: &str = "x-mako-request-id";
pub const TELEMETRY_AUTHORIZATION_HEADER: &str = "x-mako-telemetry-authorization";
pub const TELEMETRY_HEALTH_PATH: &str = "/_mako/telemetry/v1/health";
pub const TELEMETRY_QUERY_PATH: &str = "/_mako/telemetry/v1/query";
pub const TELEMETRY_INGEST_PATH: &str = "/_mako/telemetry/v1/ingest";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryIngestRequest {
    pub protocol_version: u16,
    pub request_id: String,
    pub source: String,
    pub offset: u64,
    pub records: Vec<ObservabilityRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryIngestResponse {
    pub protocol_version: u16,
    pub request_id: String,
    pub source: String,
    pub offset: u64,
    pub accepted_records: usize,
    pub replayed: bool,
}
