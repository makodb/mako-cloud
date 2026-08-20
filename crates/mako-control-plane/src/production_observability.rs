use std::{
    collections::BTreeMap,
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::TenantScope;
use mako_audit::{
    ActorIdentity, AuditFilter, AuditOutcome, AuditRecord, AuditStore, AuditStoreError,
    SignalScope, TelemetryRedactor,
};
use mako_storage::{HealthStatus as StorageHealthStatus, KvAdapter};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    EventOutcome, ObservabilityBackend, ObservabilityBackendError, ObservabilityPage,
    ObservabilityPayload, ObservabilityQuery, ObservabilityRecord, ObservabilitySignal,
    RetentionWindow,
};

pub use mako_api::{
    TELEMETRY_AUTHORIZATION_HEADER, TELEMETRY_HEALTH_PATH, TELEMETRY_INGEST_PATH,
    TELEMETRY_PROTOCOL_VERSION, TELEMETRY_QUERY_PATH, TELEMETRY_REQUEST_ID_HEADER,
    TELEMETRY_VERSION_HEADER, TelemetryIngestRequest, TelemetryIngestResponse,
};
const REQUEST_ID_HEADER: &str = TELEMETRY_REQUEST_ID_HEADER;
const AUTHORIZATION_HEADER: &str = TELEMETRY_AUTHORIZATION_HEADER;
const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_HEADERS: usize = 128;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_AUDIT_PAGE: usize = 100;

#[derive(Clone, Eq, PartialEq)]
pub struct TelemetryQueryCredential(Box<str>);

impl TelemetryQueryCredential {
    pub fn new(value: impl Into<String>) -> Result<Self, ProductionObservabilityError> {
        let value = value.into();
        if !(32..=1024).contains(&value.len()) || value.chars().any(char::is_control) {
            return Err(ProductionObservabilityError::InvalidConfiguration);
        }
        Ok(Self(value.into_boxed_str()))
    }
}

impl fmt::Debug for TelemetryQueryCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TelemetryQueryCredential([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionObservabilityConfig {
    pub telemetry_endpoint: SocketAddr,
    pub region: String,
    pub retention: Duration,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub maximum_response_bytes: usize,
}

impl ProductionObservabilityConfig {
    #[must_use]
    pub fn loopback(telemetry_endpoint: SocketAddr, region: impl Into<String>) -> Self {
        Self {
            telemetry_endpoint,
            region: region.into(),
            retention: Duration::from_secs(90 * 24 * 60 * 60),
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(10),
            maximum_response_bytes: MAX_RESPONSE_BYTES,
        }
    }

    fn validate(&self) -> Result<(), ProductionObservabilityError> {
        if !self.telemetry_endpoint.ip().is_loopback()
            || !valid_region(&self.region)
            || self.retention.is_zero()
            || self.retention.as_millis() > u128::from(u64::MAX)
            || self.connect_timeout.is_zero()
            || self.io_timeout.is_zero()
            || self.maximum_response_bytes == 0
            || self.maximum_response_bytes > MAX_RESPONSE_BYTES
        {
            return Err(ProductionObservabilityError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ProductionObservabilityBackend {
    config: ProductionObservabilityConfig,
    audit: AuditStore,
    storage: Arc<dyn KvAdapter>,
    redactor: Arc<TelemetryRedactor>,
    credential: TelemetryQueryCredential,
}

impl ProductionObservabilityBackend {
    pub fn new(
        config: ProductionObservabilityConfig,
        audit: AuditStore,
        storage: Arc<dyn KvAdapter>,
        redactor: Arc<TelemetryRedactor>,
        credential: TelemetryQueryCredential,
    ) -> Result<Self, ProductionObservabilityError> {
        config.validate()?;
        Ok(Self {
            config,
            audit,
            storage,
            redactor,
            credential,
        })
    }

    pub async fn dependency_ready(&self) -> bool {
        let storage_ready = self.storage.health().await.is_ok_and(|health| {
            health.status != StorageHealthStatus::Unavailable && health.durability_verified
        });
        storage_ready && self.telemetry_ready()
    }

    fn telemetry_ready(&self) -> bool {
        let request_id = request_id();
        self.send::<TelemetryHealth>("GET", TELEMETRY_HEALTH_PATH, &request_id, &[])
            .is_ok_and(|health| {
                health.protocol_version == TELEMETRY_PROTOCOL_VERSION
                    && health.region == self.config.region
                    && health.ready
                    && health.retention_seconds == self.config.retention.as_secs()
            })
    }

    async fn audit_query(
        &self,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now: u64,
    ) -> Result<ObservabilityPage, ObservabilityBackendError> {
        let retained_from = now
            .saturating_sub(retention_milliseconds(&self.config))
            .max(1);
        let (from, cursor) = match query.cursor.as_deref() {
            None => (query.from_unix_milliseconds.unwrap_or(retained_from), None),
            Some(cursor) => {
                let decoded =
                    decode_audit_cursor(cursor).ok_or(ObservabilityBackendError::Unavailable)?;
                if query
                    .from_unix_milliseconds
                    .is_some_and(|from| from != decoded.from_unix_milliseconds)
                {
                    return Err(ObservabilityBackendError::Unavailable);
                }
                (decoded.from_unix_milliseconds, Some(decoded.store_cursor))
            }
        };
        if from < retained_from {
            return Err(ObservabilityBackendError::CursorExpired {
                retained_from_unix_milliseconds: retained_from,
            });
        }
        let limit = NonZeroUsize::new(query.limit.min(MAX_AUDIT_PAGE))
            .ok_or(ObservabilityBackendError::Unavailable)?;
        let page = self
            .audit
            .query(
                tenant,
                &AuditFilter::default(),
                from,
                query.until_unix_milliseconds,
                limit,
                cursor.as_deref(),
                now,
            )
            .await
            .map_err(|error| map_audit_error(error, retained_from))?;
        let mut result = ObservabilityPage {
            items: page
                .records
                .into_iter()
                .map(|record| audit_record(tenant, record))
                .collect::<Result<Vec<_>, _>>()?,
            next_cursor: page
                .next_cursor
                .map(|store_cursor| encode_audit_cursor(from, store_cursor))
                .transpose()
                .map_err(|_| ObservabilityBackendError::Unavailable)?,
            retention: RetentionWindow {
                retained_from_unix_milliseconds: page.retained_from_unix_milliseconds,
                observed_at_unix_milliseconds: page.observed_at_unix_milliseconds,
                retention_seconds: self.config.retention.as_secs(),
            },
        };
        redact_page(&mut result, &self.redactor);
        validate_page(&result, tenant, ObservabilitySignal::Audit, query, now)?;
        Ok(result)
    }

    fn telemetry_query(
        &self,
        tenant: &TenantScope,
        signal: ObservabilitySignal,
        query: &ObservabilityQuery,
        now: u64,
    ) -> Result<ObservabilityPage, ObservabilityBackendError> {
        let request_id = request_id();
        let payload = TelemetryQueryRequest {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: request_id.clone(),
            tenant: tenant.clone(),
            signal,
            query: query.clone(),
            observed_at_unix_milliseconds: now,
        };
        let body =
            serde_json::to_vec(&payload).map_err(|_| ObservabilityBackendError::Unavailable)?;
        if body.len() > 64 * 1024 {
            return Err(ObservabilityBackendError::Unavailable);
        }
        let response: TelemetryQueryResponse = self
            .send("POST", TELEMETRY_QUERY_PATH, &request_id, &body)
            .map_err(|_| ObservabilityBackendError::Unavailable)?;
        if response.protocol_version != TELEMETRY_PROTOCOL_VERSION
            || response.request_id != request_id
        {
            return Err(ObservabilityBackendError::Unavailable);
        }
        let mut page = response.page;
        redact_page(&mut page, &self.redactor);
        validate_page(&page, tenant, signal, query, now)?;
        Ok(page)
    }

    fn send<R: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        request_id: &str,
        body: &[u8],
    ) -> Result<R, ProductionObservabilityError> {
        let response = send_http(
            &self.config,
            &self.credential,
            method,
            path,
            request_id,
            body,
        )?;
        if response.status != 200
            || response.request_id != request_id
            || response.protocol_version != TELEMETRY_PROTOCOL_VERSION.to_string()
        {
            return Err(ProductionObservabilityError::InvalidResponse);
        }
        serde_json::from_slice(&response.body)
            .map_err(|_| ProductionObservabilityError::InvalidResponse)
    }
}

impl fmt::Debug for ProductionObservabilityBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductionObservabilityBackend")
            .field("telemetry_endpoint", &self.config.telemetry_endpoint)
            .field("region", &self.config.region)
            .field("retention", &self.config.retention)
            .field("redactor", &self.redactor)
            .field("credential", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ObservabilityBackend for ProductionObservabilityBackend {
    async fn query(
        &self,
        tenant: &TenantScope,
        signal: ObservabilitySignal,
        query: &ObservabilityQuery,
    ) -> Result<ObservabilityPage, ObservabilityBackendError> {
        if !valid_query(query) {
            return Err(ObservabilityBackendError::Unavailable);
        }
        let now = now_milliseconds().ok_or(ObservabilityBackendError::Unavailable)?;
        if signal == ObservabilitySignal::Audit {
            self.audit_query(tenant, query, now).await
        } else {
            self.telemetry_query(tenant, signal, query, now)
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryHealth {
    pub protocol_version: u16,
    pub region: String,
    pub ready: bool,
    pub retention_seconds: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct TelemetryQueryRequest {
    pub protocol_version: u16,
    pub request_id: String,
    pub tenant: TenantScope,
    pub signal: ObservabilitySignal,
    pub query: ObservabilityQuery,
    pub observed_at_unix_milliseconds: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryQueryResponse {
    pub protocol_version: u16,
    pub request_id: String,
    pub page: ObservabilityPage,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AuditCursor {
    version: u8,
    from_unix_milliseconds: u64,
    store_cursor: String,
}

fn encode_audit_cursor(
    from_unix_milliseconds: u64,
    store_cursor: String,
) -> Result<String, serde_json::Error> {
    serde_json::to_vec(&AuditCursor {
        version: 1,
        from_unix_milliseconds,
        store_cursor,
    })
    .map(|bytes| format!("audit1.{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn decode_audit_cursor(value: &str) -> Option<AuditCursor> {
    let encoded = value.strip_prefix("audit1.")?;
    let cursor: AuditCursor =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).ok()?).ok()?;
    (cursor.version == 1 && cursor.from_unix_milliseconds > 0).then_some(cursor)
}

fn audit_record(
    tenant: &TenantScope,
    record: AuditRecord,
) -> Result<ObservabilityRecord, ObservabilityBackendError> {
    let SignalScope::Tenant {
        tenant: record_tenant,
        organization_id: Some(organization_id),
    } = record.event.context.scope()
    else {
        return Err(ObservabilityBackendError::Unavailable);
    };
    if record_tenant != tenant {
        return Err(ObservabilityBackendError::Unavailable);
    }
    let actor_id = actor_id(record.event.context.actor());
    let details = (!record.event.details.is_empty())
        .then(|| serde_json::to_string(&record.event.details))
        .transpose()
        .map_err(|_| ObservabilityBackendError::Unavailable)?;
    Ok(ObservabilityRecord {
        tenant: tenant.clone(),
        timestamp_unix_milliseconds: record.event.occurred_at_unix_milliseconds,
        payload: ObservabilityPayload::Audit {
            organization_id: organization_id.clone(),
            actor_id,
            action: record.event.action,
            target: format!(
                "{}/{}",
                record.event.context.resource().kind(),
                record.event.context.resource().id()
            ),
            outcome: match record.event.outcome {
                AuditOutcome::Allowed => EventOutcome::Allowed,
                AuditOutcome::Denied => EventOutcome::Denied,
                AuditOutcome::Failed => EventOutcome::Failed,
            },
            request_id: record.event.context.request_id().as_str().to_owned(),
            details,
        },
    })
}

fn actor_id(actor: &ActorIdentity) -> String {
    match actor {
        ActorIdentity::ApplicationUser { actor_id, .. }
        | ActorIdentity::Developer { actor_id }
        | ActorIdentity::Service { actor_id }
        | ActorIdentity::Operator { actor_id, .. } => actor_id.clone(),
        ActorIdentity::System { component } => component.clone(),
        ActorIdentity::Anonymous => "anonymous".to_owned(),
    }
}

fn map_audit_error(error: AuditStoreError, retained_from: u64) -> ObservabilityBackendError {
    match error {
        AuditStoreError::CursorExpired | AuditStoreError::RetentionWindowExpired => {
            ObservabilityBackendError::CursorExpired {
                retained_from_unix_milliseconds: retained_from,
            }
        }
        _ => ObservabilityBackendError::Unavailable,
    }
}

fn redact_page(page: &mut ObservabilityPage, redactor: &TelemetryRedactor) {
    for item in &mut page.items {
        match &mut item.payload {
            ObservabilityPayload::Usage { unit, .. } => redact(unit, redactor),
            ObservabilityPayload::Quota { .. } => {}
            ObservabilityPayload::Health {
                service,
                region,
                diagnostic,
                ..
            } => {
                redact(service, redactor);
                redact(region, redactor);
                if let Some(value) = diagnostic {
                    redact(value, redactor);
                }
            }
            ObservabilityPayload::ReplicationError {
                collection_id,
                category,
                message,
                correlation_id,
                ..
            } => redact_many([collection_id, category, message, correlation_id], redactor),
            ObservabilityPayload::AuthenticationEvent {
                category,
                application_user_id,
                message,
                correlation_id,
                ..
            } => {
                redact_many([category, message, correlation_id], redactor);
                if let Some(value) = application_user_id {
                    redact(value, redactor);
                }
            }
            ObservabilityPayload::FunctionMetric {
                function_name,
                region,
                ..
            } => redact_many([function_name, region], redactor),
            ObservabilityPayload::ProjectLog {
                source,
                level,
                message,
                correlation_id,
            } => redact_many([source, level, message, correlation_id], redactor),
            ObservabilityPayload::IndexState {
                collection_id,
                index_name,
                state,
                message,
                ..
            } => {
                redact_many([collection_id, index_name, state], redactor);
                if let Some(value) = message {
                    redact(value, redactor);
                }
            }
            ObservabilityPayload::Audit {
                organization_id,
                actor_id,
                action,
                target,
                request_id,
                details,
                ..
            } => {
                redact_many(
                    [organization_id, actor_id, action, target, request_id],
                    redactor,
                );
                if let Some(value) = details {
                    redact(value, redactor);
                }
            }
        }
    }
}

fn redact_many<'a>(values: impl IntoIterator<Item = &'a mut String>, redactor: &TelemetryRedactor) {
    for value in values {
        redact(value, redactor);
    }
}

fn redact(value: &mut String, redactor: &TelemetryRedactor) {
    *value = redactor.redact_text(value).into_string();
}

fn validate_page(
    page: &ObservabilityPage,
    tenant: &TenantScope,
    signal: ObservabilitySignal,
    query: &ObservabilityQuery,
    now: u64,
) -> Result<(), ObservabilityBackendError> {
    if page.items.len() > query.limit
        || page.retention.retention_seconds == 0
        || page.retention.observed_at_unix_milliseconds > now
        || page.retention.retained_from_unix_milliseconds
            > page.retention.observed_at_unix_milliseconds
        || page.next_cursor.as_deref().is_some_and(|value| {
            value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
        })
        || page.items.iter().any(|item| {
            item.tenant != *tenant
                || payload_signal(&item.payload) != signal
                || item.timestamp_unix_milliseconds < page.retention.retained_from_unix_milliseconds
                || item.timestamp_unix_milliseconds > page.retention.observed_at_unix_milliseconds
                || query
                    .from_unix_milliseconds
                    .is_some_and(|from| item.timestamp_unix_milliseconds < from)
                || query
                    .until_unix_milliseconds
                    .is_some_and(|until| item.timestamp_unix_milliseconds > until)
        })
    {
        return Err(ObservabilityBackendError::Unavailable);
    }
    Ok(())
}

fn payload_signal(payload: &ObservabilityPayload) -> ObservabilitySignal {
    match payload {
        ObservabilityPayload::Usage { .. } => ObservabilitySignal::Usage,
        ObservabilityPayload::Quota { .. } => ObservabilitySignal::Quota,
        ObservabilityPayload::Health { .. } => ObservabilitySignal::Health,
        ObservabilityPayload::ReplicationError { .. } => ObservabilitySignal::ReplicationError,
        ObservabilityPayload::AuthenticationEvent { .. } => {
            ObservabilitySignal::AuthenticationEvent
        }
        ObservabilityPayload::FunctionMetric { .. } => ObservabilitySignal::FunctionMetric,
        ObservabilityPayload::ProjectLog { .. } => ObservabilitySignal::ProjectLog,
        ObservabilityPayload::IndexState { .. } => ObservabilitySignal::IndexState,
        ObservabilityPayload::Audit { .. } => ObservabilitySignal::Audit,
    }
}

fn valid_query(query: &ObservabilityQuery) -> bool {
    query.limit > 0
        && query.limit <= 1000
        && query.cursor.as_deref().is_none_or(|value| {
            !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
        })
        && !matches!(
            (query.from_unix_milliseconds, query.until_unix_milliseconds),
            (Some(from), Some(until)) if from > until
        )
}

fn valid_region(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn retention_milliseconds(config: &ProductionObservabilityConfig) -> u64 {
    u64::try_from(config.retention.as_millis()).expect("validated retention fits u64")
}

fn now_milliseconds() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

fn request_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut output = String::from("req_");
    for byte in bytes {
        use fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("formatting into a string cannot fail");
    }
    output
}

#[derive(Debug, Eq, PartialEq)]
pub enum ProductionObservabilityError {
    InvalidConfiguration,
    Unavailable,
    InvalidResponse,
    ResponseTooLarge,
}

impl fmt::Display for ProductionObservabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "production observability configuration is invalid",
            Self::Unavailable => "telemetry query source is unavailable",
            Self::InvalidResponse => "telemetry query source returned an invalid response",
            Self::ResponseTooLarge => "telemetry query response exceeded its bound",
        })
    }
}

impl std::error::Error for ProductionObservabilityError {}

struct HttpResponse {
    status: u16,
    request_id: String,
    protocol_version: String,
    body: Vec<u8>,
}

fn send_http(
    config: &ProductionObservabilityConfig,
    credential: &TelemetryQueryCredential,
    method: &str,
    path: &str,
    request_id: &str,
    body: &[u8],
) -> Result<HttpResponse, ProductionObservabilityError> {
    let mut stream = TcpStream::connect_timeout(&config.telemetry_endpoint, config.connect_timeout)
        .map_err(|_| ProductionObservabilityError::Unavailable)?;
    stream
        .set_read_timeout(Some(config.io_timeout))
        .and_then(|()| stream.set_write_timeout(Some(config.io_timeout)))
        .map_err(|_| ProductionObservabilityError::Unavailable)?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{TELEMETRY_VERSION_HEADER}: {TELEMETRY_PROTOCOL_VERSION}\r\n{REQUEST_ID_HEADER}: {request_id}\r\n{AUTHORIZATION_HEADER}: {}\r\n\r\n",
        config.telemetry_endpoint,
        body.len(),
        credential.0,
    )
    .and_then(|()| stream.write_all(body))
    .and_then(|()| stream.flush())
    .map_err(|_| ProductionObservabilityError::Unavailable)?;
    let maximum = config
        .maximum_response_bytes
        .saturating_add(MAX_HEADER_BYTES)
        .saturating_add(1);
    let mut wire = Vec::new();
    stream
        .take(u64::try_from(maximum).unwrap_or(u64::MAX))
        .read_to_end(&mut wire)
        .map_err(|_| ProductionObservabilityError::Unavailable)?;
    if wire.len() >= maximum {
        return Err(ProductionObservabilityError::ResponseTooLarge);
    }
    parse_response(&wire, config.maximum_response_bytes)
}

fn parse_response(
    wire: &[u8],
    maximum_body_bytes: usize,
) -> Result<HttpResponse, ProductionObservabilityError> {
    let header_end = wire
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .ok_or(ProductionObservabilityError::InvalidResponse)?;
    if header_end > MAX_HEADER_BYTES {
        return Err(ProductionObservabilityError::InvalidResponse);
    }
    let head = std::str::from_utf8(&wire[..header_end])
        .map_err(|_| ProductionObservabilityError::InvalidResponse)?;
    let mut lines = head.split("\r\n");
    let mut status = lines
        .next()
        .ok_or(ProductionObservabilityError::InvalidResponse)?
        .split_ascii_whitespace();
    if status.next() != Some("HTTP/1.1") {
        return Err(ProductionObservabilityError::InvalidResponse);
    }
    let status = status
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| (100..=599).contains(value))
        .ok_or(ProductionObservabilityError::InvalidResponse)?;
    let mut headers = BTreeMap::new();
    for (count, line) in lines.enumerate() {
        if count >= MAX_HEADERS {
            return Err(ProductionObservabilityError::InvalidResponse);
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(ProductionObservabilityError::InvalidResponse)?;
        if headers
            .insert(name.trim().to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err(ProductionObservabilityError::InvalidResponse);
        }
    }
    if headers.contains_key("transfer-encoding")
        || headers.get("content-type").map(String::as_str) != Some("application/json")
    {
        return Err(ProductionObservabilityError::InvalidResponse);
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or(ProductionObservabilityError::InvalidResponse)?;
    let body = wire
        .get(header_end + 4..)
        .ok_or(ProductionObservabilityError::InvalidResponse)?;
    if body.len() != content_length || body.len() > maximum_body_bytes {
        return Err(ProductionObservabilityError::ResponseTooLarge);
    }
    Ok(HttpResponse {
        status,
        request_id: headers
            .remove(REQUEST_ID_HEADER)
            .ok_or(ProductionObservabilityError::InvalidResponse)?,
        protocol_version: headers
            .remove(TELEMETRY_VERSION_HEADER)
            .ok_or(ProductionObservabilityError::InvalidResponse)?,
        body: body.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        net::TcpListener,
        sync::mpsc::{self, Receiver},
        thread,
    };

    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_audit::{
        ActorIdentity, AuditCategory, AuditEvent, AuditStoreConfig, CorrelationId,
        CursorSigningKey, RequestId, ResourceReference, SafeAttributes, SignalContext,
    };
    use mako_storage::{MemoryAdapter, RocksDbAdapter, RocksDbConfig};
    use tempfile::tempdir;

    use super::*;

    const AUTHORIZATION: &str = "telemetry-authorization-that-is-long-enough";
    const SENSITIVE_VALUE: &str = "never-return-this-secret";

    #[test]
    fn persistent_audit_queries_use_tenant_retention_and_signed_cursors() {
        block_on(async {
            let tenant = tenant();
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let audit = audit_store(adapter.clone(), 60 * 60 * 1000, 10);
            let now = now_milliseconds().expect("clock");
            let redactor = Arc::new(TelemetryRedactor::new([SENSITIVE_VALUE]).expect("redactor"));
            for (index, timestamp) in [now - 200, now - 100].into_iter().enumerate() {
                audit
                    .append(
                        &tenant,
                        AuditCategory::Control,
                        audit_event(&tenant, index, timestamp),
                        &redactor,
                    )
                    .await
                    .expect("append");
            }
            let listener = TcpListener::bind("127.0.0.1:0").expect("unused endpoint");
            let backend = backend(
                listener.local_addr().expect("address"),
                audit,
                adapter,
                redactor,
                Duration::from_secs(60 * 60),
            );
            let query = ObservabilityQuery {
                cursor: None,
                from_unix_milliseconds: Some(now - 1_000),
                until_unix_milliseconds: Some(now),
                limit: 1,
            };
            let first = backend
                .query(&tenant, ObservabilitySignal::Audit, &query)
                .await
                .expect("first page");
            assert_eq!(first.items.len(), 1);
            assert_eq!(
                payload_signal(&first.items[0].payload),
                ObservabilitySignal::Audit
            );
            let cursor = first.next_cursor.expect("cursor");
            assert!(cursor.starts_with("audit1."));
            let second = backend
                .query(
                    &tenant,
                    ObservabilitySignal::Audit,
                    &ObservabilityQuery {
                        cursor: Some(cursor),
                        ..query
                    },
                )
                .await
                .expect("second page");
            assert_eq!(second.items.len(), 1);

            let expired = backend
                .query(
                    &tenant,
                    ObservabilitySignal::Audit,
                    &ObservabilityQuery {
                        cursor: None,
                        from_unix_milliseconds: Some(now - 2 * 60 * 60 * 1000),
                        until_unix_milliseconds: Some(now),
                        limit: 1,
                    },
                )
                .await;
            assert!(matches!(
                expired,
                Err(ObservabilityBackendError::CursorExpired { .. })
            ));
        });
    }

    #[test]
    fn telemetry_queries_are_authenticated_bounded_tenant_bound_and_redacted() {
        let tenant = tenant();
        let response_tenant = tenant.clone();
        let (endpoint, requests) = mock_source(vec![Box::new(move |request| {
            let payload: TelemetryQueryRequest = request_json(request);
            let page = ObservabilityPage {
                items: vec![ObservabilityRecord {
                    tenant: response_tenant.clone(),
                    timestamp_unix_milliseconds: payload.observed_at_unix_milliseconds - 1,
                    payload: ObservabilityPayload::ProjectLog {
                        source: "edge-runtime".to_owned(),
                        level: "info".to_owned(),
                        message: format!("safe prefix {SENSITIVE_VALUE}"),
                        correlation_id: "req_invocation0001".to_owned(),
                    },
                }],
                next_cursor: None,
                retention: RetentionWindow {
                    retained_from_unix_milliseconds: payload.observed_at_unix_milliseconds - 1000,
                    observed_at_unix_milliseconds: payload.observed_at_unix_milliseconds,
                    retention_seconds: 60 * 60,
                },
            };
            response(
                &payload.request_id,
                &TelemetryQueryResponse {
                    protocol_version: TELEMETRY_PROTOCOL_VERSION,
                    request_id: payload.request_id.clone(),
                    page,
                },
            )
        })]);
        let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
        let redactor = Arc::new(TelemetryRedactor::new([SENSITIVE_VALUE]).expect("redactor"));
        let backend = backend(
            endpoint,
            audit_store(adapter.clone(), 60 * 60 * 1000, 10),
            adapter,
            redactor,
            Duration::from_secs(60 * 60),
        );
        let page = block_on(backend.query(
            &tenant,
            ObservabilitySignal::ProjectLog,
            &ObservabilityQuery {
                cursor: None,
                from_unix_milliseconds: None,
                until_unix_milliseconds: None,
                limit: 10,
            },
        ))
        .expect("page");
        let ObservabilityPayload::ProjectLog { message, .. } = &page.items[0].payload else {
            panic!("project log");
        };
        assert_eq!(message, "safe prefix [REDACTED]");
        let request = requests.recv().expect("request");
        assert_eq!(
            request_header(&request, AUTHORIZATION_HEADER),
            AUTHORIZATION
        );
        assert!(request.starts_with(&format!("POST {TELEMETRY_QUERY_PATH} HTTP/1.1\r\n")));
        assert!(!format!("{backend:?}").contains(AUTHORIZATION));
    }

    #[test]
    fn readiness_requires_compatible_source_and_durable_storage() {
        let (endpoint, _requests) = mock_source(vec![Box::new(|request| {
            response(
                request_header(request, REQUEST_ID_HEADER),
                &TelemetryHealth {
                    protocol_version: TELEMETRY_PROTOCOL_VERSION,
                    region: "us-east-1-beta".to_owned(),
                    ready: true,
                    retention_seconds: 60 * 60,
                },
            )
        })]);
        let directory = tempdir().expect("directory");
        let adapter: Arc<dyn KvAdapter> =
            Arc::new(RocksDbAdapter::open(RocksDbConfig::new(directory.path())).expect("RocksDB"));
        let backend = backend(
            endpoint,
            audit_store(adapter.clone(), 60 * 60 * 1000, 10),
            adapter,
            Arc::new(TelemetryRedactor::new([SENSITIVE_VALUE]).expect("redactor")),
            Duration::from_secs(60 * 60),
        );
        assert!(block_on(backend.dependency_ready()));

        let credential = TelemetryQueryCredential::new(AUTHORIZATION).expect("credential");
        assert!(!format!("{credential:?}").contains(AUTHORIZATION));
        let bad = ProductionObservabilityConfig::loopback(
            "192.0.2.1:9465".parse().expect("address"),
            "us-east-1-beta",
        );
        let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
        assert_eq!(
            ProductionObservabilityBackend::new(
                bad,
                audit_store(adapter.clone(), 1000, 10),
                adapter,
                Arc::new(TelemetryRedactor::new([SENSITIVE_VALUE]).expect("redactor")),
                credential,
            )
            .unwrap_err(),
            ProductionObservabilityError::InvalidConfiguration,
        );
    }

    fn backend(
        endpoint: SocketAddr,
        audit: AuditStore,
        adapter: Arc<dyn KvAdapter>,
        redactor: Arc<TelemetryRedactor>,
        retention: Duration,
    ) -> ProductionObservabilityBackend {
        let mut config = ProductionObservabilityConfig::loopback(endpoint, "us-east-1-beta");
        config.retention = retention;
        ProductionObservabilityBackend::new(
            config,
            audit,
            adapter,
            redactor,
            TelemetryQueryCredential::new(AUTHORIZATION).expect("credential"),
        )
        .expect("backend")
    }

    fn audit_store(
        adapter: Arc<dyn KvAdapter>,
        retention_milliseconds: u64,
        maximum_page: usize,
    ) -> AuditStore {
        let durability = adapter.capabilities().strongest_durability;
        AuditStore::new(
            adapter,
            AuditStoreConfig {
                durability,
                retention_milliseconds,
                maximum_page_records: NonZeroUsize::new(maximum_page).expect("page"),
                maximum_export_records: NonZeroUsize::new(100).expect("export"),
                maximum_examined_records: NonZeroUsize::new(1000).expect("examined"),
                cursor_signing_key: CursorSigningKey::new([7; 32]),
            },
        )
        .expect("audit")
    }

    fn audit_event(
        tenant: &TenantScope,
        index: usize,
        timestamp_unix_milliseconds: u64,
    ) -> AuditEvent {
        AuditEvent {
            context: SignalContext::new(
                SignalScope::Tenant {
                    tenant: tenant.clone(),
                    organization_id: Some("org_abcdefgh".to_owned()),
                },
                ActorIdentity::Developer {
                    actor_id: "dev_abcdefgh".to_owned(),
                },
                ResourceReference::new("function", format!("fn_{index:08}")).expect("resource"),
                RequestId::parse(format!("req_{index:016}")).expect("request"),
                CorrelationId::parse(format!("cor_{index:016}")).expect("correlation"),
                None,
            )
            .expect("context"),
            event_id: format!("evt_{index:016}"),
            occurred_at_unix_milliseconds: timestamp_unix_milliseconds,
            action: "function_read".to_owned(),
            outcome: AuditOutcome::Allowed,
            reason_code: "allowed".to_owned(),
            details: SafeAttributes::default(),
        }
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }

    type Handler = Box<dyn Fn(&str) -> Vec<u8> + Send>;

    fn mock_source(handlers: Vec<Handler>) -> (SocketAddr, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let endpoint = listener.local_addr().expect("endpoint");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for handler in handlers {
                let (mut stream, _) = listener.accept().expect("connection");
                let request = read_request(&mut stream);
                let response = handler(&request);
                sender.send(request).expect("receiver");
                stream.write_all(&response).expect("response");
            }
        });
        (endpoint, receiver)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut request = Vec::new();
        let header_end = loop {
            if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
            let mut chunk = [0_u8; 4096];
            let size = stream.read(&mut chunk).expect("header");
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
        };
        let head = std::str::from_utf8(&request[..header_end]).expect("head");
        let content_length = head
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_, value)| value.trim().parse().expect("length"));
        while request.len() < header_end + content_length {
            let mut chunk = [0_u8; 4096];
            let size = stream.read(&mut chunk).expect("body");
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
        }
        String::from_utf8(request).expect("request")
    }

    fn response<T: Serialize>(request_id: &str, body: &T) -> Vec<u8> {
        let body = serde_json::to_vec(body).expect("JSON");
        let mut wire = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{REQUEST_ID_HEADER}: {request_id}\r\n{TELEMETRY_VERSION_HEADER}: {TELEMETRY_PROTOCOL_VERSION}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        wire.extend_from_slice(&body);
        wire
    }

    fn request_header<'a>(request: &'a str, expected: &str) -> &'a str {
        request
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case(expected))
            .map(|(_, value)| value.trim())
            .expect("header")
    }

    fn request_json<T: DeserializeOwned>(request: &str) -> T {
        serde_json::from_str(request.split_once("\r\n\r\n").expect("body").1).expect("request JSON")
    }
}
