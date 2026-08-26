#![forbid(unsafe_code)]

use std::{
    env,
    error::Error,
    fmt, fs,
    net::SocketAddr,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::executor::block_on;
use mako_api::TenantScope;
use mako_audit::TelemetryRedactor;
use mako_control_plane::{
    HealthState, ObservabilityPage, ObservabilityPayload, ObservabilityRecord, ObservabilitySignal,
    RetentionWindow, TELEMETRY_AUTHORIZATION_HEADER, TELEMETRY_HEALTH_PATH, TELEMETRY_INGEST_PATH,
    TELEMETRY_PROTOCOL_VERSION, TELEMETRY_QUERY_PATH, TELEMETRY_VERSION_HEADER, TelemetryHealth,
    TelemetryIngestRequest, TelemetryIngestResponse, TelemetryQueryRequest, TelemetryQueryResponse,
};
use mako_service_runtime::{
    HttpMethod, HttpRequest, HttpResponse, HttpRouter, HttpTransportConfig, ReadinessProbe,
    ReadinessSnapshot, serve_http_transport_with_readiness,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, HealthStatus, KeyCondition, KeyRange,
    KvAdapter, ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity, ScanDirection,
    ScanRequest, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const COMPONENT: &str = "mako-telemetry-query";
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const MAX_INGEST_RECORDS: usize = 256;
const MAX_QUERY_ITEMS: usize = 1_000;
const MAX_CLOCK_SKEW_MILLISECONDS: u64 = 30_000;
/// More usage records in one checkpointed minute than this and the
/// cross-check abstains rather than compare a truncated sum.
const MAX_CROSS_CHECK_RECORDS: usize = 20_000;
const KEYSPACE: &[u8] = b"\0mako/telemetry/v1/";
const CHECKPOINT_KEYSPACE: &[u8] = b"\0mako/telemetry-checkpoint/v1/";

fn main() -> Result<(), Box<dyn Error>> {
    let config = ServiceConfig::from_environment()?;
    let credential = TelemetryCredential::read(&config.credential_path)?;
    let identity = ProductionVolumeIdentity::new(COMPONENT, &config.database_id)?;
    let storage = block_on(ProductionRocksDb::open(ProductionRocksDbConfig {
        database_path: config.database_path.clone(),
        identity,
        maximum_batch_operations: NonZeroUsize::new(512).expect("non-zero constant"),
        maximum_scan_items: NonZeroUsize::new(2_048).expect("non-zero constant"),
        transaction_lock_timeout: Duration::from_secs(5),
        transaction_expiration: Duration::from_secs(30),
        disk_warning_free_bytes: config.disk_warning_free_bytes,
        disk_critical_free_bytes: config.disk_critical_free_bytes,
    }))?;
    let adapter: Arc<dyn KvAdapter> = Arc::new(storage.adapter().clone());
    let redactor = Arc::new(TelemetryRedactor::new(std::iter::empty::<&str>())?);
    let store = Arc::new(TelemetryStore::new(
        Arc::clone(&adapter),
        config.region.clone(),
        config.retention,
        redactor,
    )?);
    let readiness: Arc<dyn ReadinessProbe> = Arc::new(TelemetryReadiness { storage: adapter });
    let router = router(store, credential)?;
    let mut transport = HttpTransportConfig::new(config.bind, COMPONENT, "telemetry_store_ready");
    transport.max_request_body_bytes = MAX_REQUEST_BODY_BYTES;
    transport.max_in_flight_requests = 64;
    serve_http_transport_with_readiness(transport, router, readiness)?;
    drop(storage);
    Ok(())
}

#[derive(Clone, Debug)]
struct ServiceConfig {
    bind: SocketAddr,
    region: String,
    retention: Duration,
    database_path: PathBuf,
    database_id: String,
    credential_path: PathBuf,
    disk_warning_free_bytes: u64,
    disk_critical_free_bytes: u64,
}

impl ServiceConfig {
    fn from_environment() -> Result<Self, ServiceError> {
        let bind = required_environment("MAKO_TELEMETRY_QUERY_BIND")?
            .parse::<SocketAddr>()
            .map_err(|_| ServiceError::InvalidConfiguration)?;
        let region = required_environment("MAKO_TELEMETRY_REGION")?;
        let retention_seconds = parse_u64("MAKO_TELEMETRY_RETENTION_SECONDS")?;
        let database_path = PathBuf::from(required_environment("MAKO_TELEMETRY_DATABASE_PATH")?);
        let database_id = required_environment("MAKO_TELEMETRY_DATABASE_ID")?;
        let credential_path = env::var_os("MAKO_TELEMETRY_AUTHORIZATION_FILE")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("CREDENTIALS_DIRECTORY")
                    .map(PathBuf::from)
                    .map(|directory| directory.join("telemetry-authorization"))
            })
            .ok_or(ServiceError::InvalidConfiguration)?;
        let disk_warning_free_bytes = parse_u64("MAKO_DISK_WARNING_FREE_BYTES")?;
        let disk_critical_free_bytes = parse_u64("MAKO_DISK_CRITICAL_FREE_BYTES")?;
        if !bind.ip().is_loopback()
            || !valid_region(&region)
            || retention_seconds == 0
            || !database_path.is_absolute()
            || !valid_database_id(&database_id)
            || !credential_path.is_absolute()
            || disk_critical_free_bytes == 0
            || disk_warning_free_bytes <= disk_critical_free_bytes
        {
            return Err(ServiceError::InvalidConfiguration);
        }
        Ok(Self {
            bind,
            region,
            retention: Duration::from_secs(retention_seconds),
            database_path,
            database_id,
            credential_path,
            disk_warning_free_bytes,
            disk_critical_free_bytes,
        })
    }
}

#[derive(Clone, Eq, PartialEq)]
struct TelemetryCredential(Box<str>);

impl TelemetryCredential {
    fn read(path: &Path) -> Result<Self, ServiceError> {
        let source = fs::read_to_string(path).map_err(|_| ServiceError::InvalidConfiguration)?;
        let value = source.trim_end_matches(['\r', '\n']);
        if !(32..=1_024).contains(&value.len()) || value.chars().any(char::is_control) {
            return Err(ServiceError::InvalidConfiguration);
        }
        Ok(Self(value.to_owned().into_boxed_str()))
    }

    fn matches(&self, candidate: Option<&str>) -> bool {
        let expected = Sha256::digest(self.0.as_bytes());
        let supplied = Sha256::digest(candidate.unwrap_or_default().as_bytes());
        let mut difference = usize::from(candidate.is_none());
        for (left, right) in expected.iter().zip(supplied.iter()) {
            difference |= usize::from(left ^ right);
        }
        difference == 0
    }
}

impl fmt::Debug for TelemetryCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TelemetryCredential([REDACTED])")
    }
}

#[derive(Clone)]
struct TelemetryStore {
    storage: Arc<dyn KvAdapter>,
    region: String,
    retention: Duration,
    redactor: Arc<TelemetryRedactor>,
    durability: Durability,
}

impl TelemetryStore {
    fn new(
        storage: Arc<dyn KvAdapter>,
        region: String,
        retention: Duration,
        redactor: Arc<TelemetryRedactor>,
    ) -> Result<Self, ServiceError> {
        Self::with_durability(storage, region, retention, redactor, Durability::Sync)
    }

    fn with_durability(
        storage: Arc<dyn KvAdapter>,
        region: String,
        retention: Duration,
        redactor: Arc<TelemetryRedactor>,
        durability: Durability,
    ) -> Result<Self, ServiceError> {
        if !valid_region(&region)
            || retention.is_zero()
            || retention.as_millis() > u128::from(u64::MAX)
        {
            return Err(ServiceError::InvalidConfiguration);
        }
        Ok(Self {
            storage,
            region,
            retention,
            redactor,
            durability,
        })
    }

    async fn ready(&self) -> bool {
        self.storage.health().await.is_ok_and(|health| {
            health.status != HealthStatus::Unavailable && health.durability_verified
        })
    }

    async fn ingest(
        &self,
        mut request: TelemetryIngestRequest,
        now: u64,
    ) -> Result<TelemetryIngestResponse, ServiceError> {
        validate_ingest_request(&request, now, self.retention)?;
        for record in &mut request.records {
            redact_record(record, &self.redactor);
            validate_record(record, None, now, self.retained_from(now))?;
        }
        let digest = ingest_digest(&request)?;
        let checkpoint_key = checkpoint_key(&request.source);
        let current = self
            .storage
            .get(&checkpoint_key)
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        let checkpoint = current
            .as_deref()
            .map(serde_json::from_slice::<SourceCheckpoint>)
            .transpose()
            .map_err(|_| ServiceError::Unavailable)?;
        if let Some(checkpoint) = &checkpoint {
            if request.offset == checkpoint.offset {
                if digest == checkpoint.digest {
                    return Ok(TelemetryIngestResponse {
                        protocol_version: TELEMETRY_PROTOCOL_VERSION,
                        request_id: request.request_id,
                        source: request.source,
                        offset: request.offset,
                        accepted_records: request.records.len(),
                        replayed: true,
                    });
                }
                return Err(ServiceError::Conflict);
            }
            if request.offset != checkpoint.offset.saturating_add(1) {
                return Err(ServiceError::InvalidRequest);
            }
        } else if request.offset != 1 {
            return Err(ServiceError::InvalidRequest);
        }

        let mut batch = WriteBatch::with_capacity(request.records.len().saturating_add(1));
        for record in &request.records {
            batch.put(
                record_key(record),
                serde_json::to_vec(record).map_err(|_| ServiceError::Unavailable)?,
            );
        }
        let next_checkpoint = serde_json::to_vec(&SourceCheckpoint {
            offset: request.offset,
            digest,
        })
        .map_err(|_| ServiceError::Unavailable)?;
        batch.put(&checkpoint_key, next_checkpoint);
        let condition = current.map_or(
            KeyCondition::Missing {
                key: checkpoint_key.clone(),
            },
            |value| KeyCondition::ValueEquals {
                key: checkpoint_key,
                value,
            },
        );
        let applied = self
            .storage
            .compare_and_write(AtomicWrite {
                conditions: vec![condition],
                batch,
                durability: self.durability,
            })
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        if applied != CompareAndWriteResult::Applied {
            return Err(ServiceError::Conflict);
        }
        // A quota checkpoint summarizes what enforcement counted for a closed
        // window. Its source delivers in emission order, so by the time the
        // checkpoint is durable, every usage record the plane emitted for
        // that window is too -- which makes this the moment the ledger can be
        // held against it. A replayed batch returned above and is never
        // compared twice.
        self.cross_check_quota_checkpoints(&request.records, now)
            .await;
        Ok(TelemetryIngestResponse {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: request.request_id,
            source: request.source,
            offset: request.offset,
            accepted_records: request.records.len(),
            replayed: false,
        })
    }

    async fn query(
        &self,
        request: TelemetryQueryRequest,
    ) -> Result<TelemetryQueryResponse, ServiceError> {
        validate_query_request(&request)?;
        let retained_from = self.retained_from(request.observed_at_unix_milliseconds);
        let from = request
            .query
            .from_unix_milliseconds
            .unwrap_or(retained_from);
        let until = request
            .query
            .until_unix_milliseconds
            .unwrap_or(request.observed_at_unix_milliseconds);
        if from < retained_from || from > until || until > request.observed_at_unix_milliseconds {
            return Err(ServiceError::CursorExpired);
        }
        let prefix = record_prefix(&request.tenant, request.signal)?;
        let mut start = timestamp_key(&prefix, from);
        if let Some(cursor) = &request.query.cursor {
            let decoded = decode_cursor(cursor)?;
            if !decoded.starts_with(&prefix)
                || decoded < start
                || decoded >= timestamp_key(&prefix, until.saturating_add(1))
            {
                return Err(ServiceError::InvalidRequest);
            }
            start = decoded;
            start.push(0);
        }
        let range = KeyRange::new(start, timestamp_key(&prefix, until.saturating_add(1)))
            .map_err(|_| ServiceError::InvalidRequest)?;
        let scan_limit = request
            .query
            .limit
            .saturating_add(1)
            .min(MAX_QUERY_ITEMS + 1);
        let rows = self
            .storage
            .scan(ScanRequest::new(
                range,
                ScanDirection::Forward,
                NonZeroUsize::new(scan_limit).ok_or(ServiceError::InvalidRequest)?,
            ))
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        let has_more = rows.len() > request.query.limit;
        let selected = if has_more {
            &rows[..request.query.limit]
        } else {
            rows.as_slice()
        };
        let mut items = selected
            .iter()
            .map(|row| serde_json::from_slice::<ObservabilityRecord>(&row.value))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ServiceError::Unavailable)?;
        for record in &mut items {
            validate_record(
                record,
                Some((&request.tenant, request.signal)),
                request.observed_at_unix_milliseconds,
                retained_from,
            )?;
            redact_record(record, &self.redactor);
        }
        let next_cursor = has_more
            .then(|| selected.last().map(|row| encode_cursor(&row.key)))
            .flatten();
        Ok(TelemetryQueryResponse {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: request.request_id,
            page: ObservabilityPage {
                items,
                next_cursor,
                retention: RetentionWindow {
                    retained_from_unix_milliseconds: retained_from,
                    observed_at_unix_milliseconds: request.observed_at_unix_milliseconds,
                    retention_seconds: self.retention.as_secs(),
                },
            },
        })
    }

    fn retained_from(&self, observed_at: u64) -> u64 {
        observed_at
            .saturating_sub(retention_milliseconds(self.retention))
            .max(1)
    }

    /// Hold the usage ledger against what enforcement counted.
    ///
    /// Each `Quota` record in an accepted batch summarizes one closed minute
    /// of one tenant's counters. The ledger's answer for the same minute is
    /// the sum of the usage records this store kept. A material difference
    /// means one of billing or enforcement is wrong, and is reported as a
    /// degraded health record for the tenant plus an operator-visible log
    /// line. Nothing here may fail the ingest that triggered it: the batch
    /// is already durable, and a cross-check that cannot run is a missed
    /// sample, not an error.
    async fn cross_check_quota_checkpoints(&self, records: &[ObservabilityRecord], now: u64) {
        for record in records {
            let ObservabilityPayload::Quota {
                resource,
                consumed,
                retry_after_unix_milliseconds: None,
                ..
            } = &record.payload
            else {
                continue;
            };
            let window_start = record.timestamp_unix_milliseconds;
            let Ok(Some(ledger)) = self
                .ledger_window_total(&record.tenant, *resource, window_start)
                .await
            else {
                continue;
            };
            if !material_divergence(*resource, ledger, *consumed) {
                continue;
            }
            let resource_name = quota_resource_name(*resource);
            eprintln!(
                "telemetry meter cross-check divergence project={} environment={} resource={resource_name} ledger={ledger} enforcement_counter={consumed} window_start_unix_milliseconds={window_start}",
                record.tenant.project_id(),
                record.tenant.environment_id(),
            );
            let alert = ObservabilityRecord {
                tenant: record.tenant.clone(),
                timestamp_unix_milliseconds: now,
                payload: ObservabilityPayload::Health {
                    service: "mako-telemetry-query".to_owned(),
                    region: self.region.clone(),
                    status: HealthState::Degraded,
                    diagnostic: Some(format!(
                        "meter cross-check diverged for {resource_name}: the usage ledger says {ledger}, the enforcement counter says {consumed}, for the minute starting {window_start}"
                    )),
                },
            };
            let Ok(value) = serde_json::to_vec(&alert) else {
                continue;
            };
            let mut batch = WriteBatch::with_capacity(1);
            batch.put(record_key(&alert), value);
            let _ = self.storage.write(batch, self.durability).await;
        }
    }

    /// What the ledger holds for one tenant, resource, and checkpoint window.
    /// Answers `None` when the window holds more records than a bounded scan
    /// may visit, because a truncated sum would cry divergence falsely.
    async fn ledger_window_total(
        &self,
        tenant: &TenantScope,
        resource: mako_api::QuotaResource,
        window_start: u64,
    ) -> Result<Option<u64>, ServiceError> {
        let prefix = record_prefix(tenant, ObservabilitySignal::Usage)?;
        let mut start = timestamp_key(&prefix, window_start);
        let end = timestamp_key(
            &prefix,
            window_start.saturating_add(mako_api::QUOTA_CHECKPOINT_WINDOW_MILLISECONDS),
        );
        // Every storage engine bounds a single scan, so the window is walked
        // in pages the engines all accept.
        let page_limit = NonZeroUsize::new(MAX_QUERY_ITEMS).ok_or(ServiceError::InvalidRequest)?;
        let mut visited = 0_usize;
        let mut total: u64 = 0;
        loop {
            let range = KeyRange::new(start.clone(), end.clone())
                .map_err(|_| ServiceError::InvalidRequest)?;
            let rows = self
                .storage
                .scan(ScanRequest::new(range, ScanDirection::Forward, page_limit))
                .await
                .map_err(|_| ServiceError::Unavailable)?;
            let page_full = rows.len() >= page_limit.get();
            visited = visited.saturating_add(rows.len());
            if visited > MAX_CROSS_CHECK_RECORDS {
                return Ok(None);
            }
            let next_start = rows.last().map(|row| {
                let mut key = row.key.clone();
                key.push(0);
                key
            });
            for row in rows {
                let Ok(record) = serde_json::from_slice::<ObservabilityRecord>(&row.value) else {
                    continue;
                };
                if let ObservabilityPayload::Usage {
                    resource: recorded,
                    quantity,
                    ..
                } = record.payload
                    && recorded == resource
                {
                    total = total.saturating_add(quantity);
                }
            }
            match next_start {
                Some(key) if page_full => start = key,
                _ => return Ok(Some(total)),
            }
        }
    }
}

/// Whether a ledger/counter difference is worth an alert. Small differences
/// are expected -- an admitted request whose usage record was retried, or one
/// straddling a settle edge -- so an alert needs both an absolute floor and a
/// share of the larger side.
fn material_divergence(resource: mako_api::QuotaResource, ledger: u64, counter: u64) -> bool {
    let floor = match resource {
        mako_api::QuotaResource::ReplicationBytesPerMonth => 1024 * 1024,
        _ => 2,
    };
    ledger.abs_diff(counter) > floor.max(ledger.max(counter) / 20)
}

fn quota_resource_name(resource: mako_api::QuotaResource) -> String {
    serde_json::to_string(&resource)
        .map(|name| name.trim_matches('"').to_owned())
        .unwrap_or_else(|_| format!("{resource:?}"))
}

struct TelemetryReadiness {
    storage: Arc<dyn KvAdapter>,
}

impl ReadinessProbe for TelemetryReadiness {
    fn snapshot(&self) -> ReadinessSnapshot {
        if block_on(self.storage.health()).is_ok_and(|health| {
            health.status != HealthStatus::Unavailable && health.durability_verified
        }) {
            ReadinessSnapshot::ready("telemetry_store_ready")
        } else {
            ReadinessSnapshot::not_ready("telemetry_store_unavailable")
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SourceCheckpoint {
    offset: u64,
    digest: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryErrorEnvelope<'a> {
    protocol_version: u16,
    request_id: &'a str,
    code: &'static str,
    retryable: bool,
}

fn router(
    store: Arc<TelemetryStore>,
    credential: TelemetryCredential,
) -> Result<HttpRouter, ServiceError> {
    let mut router = HttpRouter::new();
    let health_store = Arc::clone(&store);
    let health_credential = credential.clone();
    router
        .add_route(HttpMethod::Get, TELEMETRY_HEALTH_PATH, move |request| {
            if let Some(response) = authorize(&request, &health_credential) {
                return Ok(response);
            }
            let ready = block_on(health_store.ready());
            telemetry_response(
                200,
                &TelemetryHealth {
                    protocol_version: TELEMETRY_PROTOCOL_VERSION,
                    region: health_store.region.clone(),
                    ready,
                    retention_seconds: health_store.retention.as_secs(),
                },
            )
        })
        .map_err(|_| ServiceError::InvalidConfiguration)?;

    let query_store = Arc::clone(&store);
    let query_credential = credential.clone();
    router
        .add_route(HttpMethod::Post, TELEMETRY_QUERY_PATH, move |request| {
            if let Some(response) = authorize(&request, &query_credential) {
                return Ok(response);
            }
            let payload = serde_json::from_slice::<TelemetryQueryRequest>(request.body())
                .map_err(|_| http_error(&request, ServiceError::InvalidRequest))?;
            if payload.request_id != request.request_id() {
                return Ok(error_response(&request, ServiceError::InvalidRequest));
            }
            match block_on(query_store.query(payload)) {
                Ok(response) => telemetry_response(200, &response),
                Err(error) => Ok(error_response(&request, error)),
            }
        })
        .map_err(|_| ServiceError::InvalidConfiguration)?;

    router
        .add_route(HttpMethod::Post, TELEMETRY_INGEST_PATH, move |request| {
            if let Some(response) = authorize(&request, &credential) {
                return Ok(response);
            }
            let payload = serde_json::from_slice::<TelemetryIngestRequest>(request.body())
                .map_err(|_| http_error(&request, ServiceError::InvalidRequest))?;
            if payload.request_id != request.request_id() {
                return Ok(error_response(&request, ServiceError::InvalidRequest));
            }
            let now = now_milliseconds()
                .ok_or_else(|| http_error(&request, ServiceError::Unavailable))?;
            match block_on(store.ingest(payload, now)) {
                Ok(response) => telemetry_response(200, &response),
                Err(error) => Ok(error_response(&request, error)),
            }
        })
        .map_err(|_| ServiceError::InvalidConfiguration)?;
    Ok(router)
}

fn authorize(request: &HttpRequest, credential: &TelemetryCredential) -> Option<HttpResponse> {
    if request.header(TELEMETRY_VERSION_HEADER) != Some(&TELEMETRY_PROTOCOL_VERSION.to_string()) {
        return Some(error_response(request, ServiceError::ProtocolMismatch));
    }
    if !credential.matches(request.header(TELEMETRY_AUTHORIZATION_HEADER)) {
        return Some(error_response(request, ServiceError::Unauthorized));
    }
    None
}

fn telemetry_response<T: Serialize>(
    status: u16,
    value: &T,
) -> Result<HttpResponse, mako_service_runtime::HttpApiError> {
    serde_json::to_vec(value)
        .map(|body| {
            HttpResponse::bytes(status, "application/json", body)
                .with_header(
                    TELEMETRY_VERSION_HEADER,
                    &TELEMETRY_PROTOCOL_VERSION.to_string(),
                )
                .expect("static telemetry response header is valid")
        })
        .map_err(|_| impossible_http_error())
}

fn error_response(request: &HttpRequest, error: ServiceError) -> HttpResponse {
    let (status, code, retryable) = match error {
        ServiceError::ProtocolMismatch => (426, "protocol_mismatch", false),
        ServiceError::Unauthorized => (401, "unauthorized", false),
        ServiceError::InvalidRequest => (400, "invalid_request", false),
        ServiceError::CursorExpired => (410, "cursor_expired", false),
        ServiceError::Conflict => (409, "source_conflict", false),
        ServiceError::Unavailable | ServiceError::InvalidConfiguration => {
            (503, "telemetry_unavailable", true)
        }
    };
    let body = serde_json::to_vec(&TelemetryErrorEnvelope {
        protocol_version: TELEMETRY_PROTOCOL_VERSION,
        request_id: request.request_id(),
        code,
        retryable,
    })
    .unwrap_or_else(|_| b"{}".to_vec());
    HttpResponse::bytes(status, "application/json", body)
        .with_header(
            TELEMETRY_VERSION_HEADER,
            &TELEMETRY_PROTOCOL_VERSION.to_string(),
        )
        .expect("static telemetry response header is valid")
}

fn http_error(request: &HttpRequest, error: ServiceError) -> mako_service_runtime::HttpApiError {
    use mako_api::{ErrorCode, RetryAdvice};
    let (status, code, message) = match error {
        ServiceError::InvalidRequest => (
            400,
            ErrorCode::InvalidRequest,
            "telemetry request is invalid",
        ),
        _ => (
            503,
            ErrorCode::Unavailable,
            "telemetry service is unavailable",
        ),
    };
    mako_service_runtime::HttpApiError::new(
        status,
        code,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn impossible_http_error() -> mako_service_runtime::HttpApiError {
    use mako_api::{ErrorCode, RetryAdvice};
    mako_service_runtime::HttpApiError::new(
        503,
        ErrorCode::Unavailable,
        "telemetry service is unavailable",
        "req_telemetry_internal",
        RetryAdvice::Never,
    )
}

fn validate_ingest_request(
    request: &TelemetryIngestRequest,
    now: u64,
    retention: Duration,
) -> Result<(), ServiceError> {
    if request.protocol_version != TELEMETRY_PROTOCOL_VERSION
        || !valid_request_id(&request.request_id)
        || !valid_source(&request.source)
        || request.offset == 0
        || request.records.is_empty()
        || request.records.len() > MAX_INGEST_RECORDS
    {
        return Err(ServiceError::InvalidRequest);
    }
    let retained_from = now.saturating_sub(retention_milliseconds(retention)).max(1);
    for record in &request.records {
        validate_record(record, None, now, retained_from)?;
        if record.payload.signal() == ObservabilitySignal::Audit {
            return Err(ServiceError::InvalidRequest);
        }
    }
    Ok(())
}

fn validate_query_request(request: &TelemetryQueryRequest) -> Result<(), ServiceError> {
    let actual_now = now_milliseconds().ok_or(ServiceError::Unavailable)?;
    if request.protocol_version != TELEMETRY_PROTOCOL_VERSION
        || !valid_request_id(&request.request_id)
        || request.signal == ObservabilitySignal::Audit
        || request.query.limit == 0
        || request.query.limit > MAX_QUERY_ITEMS
        || request.query.cursor.as_deref().is_some_and(|cursor| {
            cursor.is_empty() || cursor.len() > 4_096 || cursor.chars().any(char::is_control)
        })
        || actual_now.abs_diff(request.observed_at_unix_milliseconds) > MAX_CLOCK_SKEW_MILLISECONDS
    {
        return Err(ServiceError::InvalidRequest);
    }
    Ok(())
}

fn validate_record(
    record: &ObservabilityRecord,
    expected: Option<(&TenantScope, ObservabilitySignal)>,
    observed_at: u64,
    retained_from: u64,
) -> Result<(), ServiceError> {
    if record.timestamp_unix_milliseconds < retained_from
        || record.timestamp_unix_milliseconds > observed_at
        || !record.payload.is_safe()
        || expected.is_some_and(|(tenant, signal)| {
            record.tenant != *tenant || record.payload.signal() != signal
        })
    {
        return Err(ServiceError::InvalidRequest);
    }
    Ok(())
}

fn redact_record(record: &mut ObservabilityRecord, redactor: &TelemetryRedactor) {
    let redact = |value: &mut String| *value = redactor.redact_text(value).into_string();
    match &mut record.payload {
        ObservabilityPayload::Usage { unit, .. } => redact(unit),
        ObservabilityPayload::Quota { .. } => {}
        ObservabilityPayload::Health {
            service,
            region,
            diagnostic,
            ..
        } => {
            redact(service);
            redact(region);
            if let Some(value) = diagnostic {
                redact(value);
            }
        }
        ObservabilityPayload::ReplicationError {
            collection_id,
            category,
            message,
            correlation_id,
            ..
        } => {
            redact(collection_id);
            redact(category);
            redact(message);
            redact(correlation_id);
        }
        ObservabilityPayload::AuthenticationEvent {
            category,
            application_user_id,
            message,
            correlation_id,
            ..
        } => {
            redact(category);
            if let Some(value) = application_user_id {
                redact(value);
            }
            redact(message);
            redact(correlation_id);
        }
        ObservabilityPayload::FunctionMetric {
            function_name,
            region,
            ..
        } => {
            redact(function_name);
            redact(region);
        }
        ObservabilityPayload::ProjectLog {
            source,
            level,
            message,
            correlation_id,
        } => {
            redact(source);
            redact(level);
            // Log text is written by customer code, so it gets the log scrub
            // -- credentials plus email masking -- applied here at the store,
            // the one choke point no producer can bypass.
            *message = redactor.scrub_log_text(message).into_string();
            redact(correlation_id);
        }
        ObservabilityPayload::IndexState {
            collection_id,
            index_name,
            state,
            message,
            ..
        } => {
            redact(collection_id);
            redact(index_name);
            redact(state);
            if let Some(value) = message {
                redact(value);
            }
        }
        ObservabilityPayload::Audit { .. } => {}
    }
}

fn record_prefix(
    tenant: &TenantScope,
    signal: ObservabilitySignal,
) -> Result<Vec<u8>, ServiceError> {
    let mut key = KEYSPACE.to_vec();
    key.extend_from_slice(tenant.project_id().as_str().as_bytes());
    key.push(0);
    key.extend_from_slice(tenant.environment_id().as_str().as_bytes());
    key.push(0);
    key.push(signal_code(signal)?);
    key.push(0);
    Ok(key)
}

fn timestamp_key(prefix: &[u8], timestamp: u64) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(&timestamp.to_be_bytes());
    key
}

fn record_key(record: &ObservabilityRecord) -> Vec<u8> {
    let mut key = timestamp_key(
        &record_prefix(&record.tenant, record.payload.signal())
            .expect("validated non-audit signal"),
        record.timestamp_unix_milliseconds,
    );
    let mut suffix = [0_u8; 16];
    OsRng.fill_bytes(&mut suffix);
    key.extend_from_slice(&suffix);
    key
}

fn checkpoint_key(source: &str) -> Vec<u8> {
    let mut key = CHECKPOINT_KEYSPACE.to_vec();
    key.extend_from_slice(source.as_bytes());
    key
}

fn signal_code(signal: ObservabilitySignal) -> Result<u8, ServiceError> {
    Ok(match signal {
        ObservabilitySignal::Usage => 1,
        ObservabilitySignal::Quota => 2,
        ObservabilitySignal::Health => 3,
        ObservabilitySignal::ReplicationError => 4,
        ObservabilitySignal::AuthenticationEvent => 5,
        ObservabilitySignal::FunctionMetric => 6,
        ObservabilitySignal::ProjectLog => 7,
        ObservabilitySignal::IndexState => 8,
        ObservabilitySignal::Audit => return Err(ServiceError::InvalidRequest),
    })
}

fn encode_cursor(key: &[u8]) -> String {
    format!("telemetry1.{}", URL_SAFE_NO_PAD.encode(key))
}

fn decode_cursor(value: &str) -> Result<Vec<u8>, ServiceError> {
    value
        .strip_prefix("telemetry1.")
        .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
        .filter(|key| key.len() <= 512)
        .ok_or(ServiceError::InvalidRequest)
}

fn ingest_digest(request: &TelemetryIngestRequest) -> Result<String, ServiceError> {
    serde_json::to_vec(&(&request.source, request.offset, &request.records))
        .map(|bytes| format!("sha256:{:x}", Sha256::digest(bytes)))
        .map_err(|_| ServiceError::InvalidRequest)
}

fn retention_milliseconds(retention: Duration) -> u64 {
    u64::try_from(retention.as_millis()).expect("validated retention fits u64")
}

fn now_milliseconds() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

fn valid_request_id(value: &str) -> bool {
    value.strip_prefix("req_").is_some_and(|suffix| {
        (8..=128).contains(&suffix.len())
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn valid_source(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn valid_region(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_database_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn required_environment(name: &str) -> Result<String, ServiceError> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .ok_or(ServiceError::InvalidConfiguration)
}

fn parse_u64(name: &str) -> Result<u64, ServiceError> {
    required_environment(name)?
        .parse()
        .map_err(|_| ServiceError::InvalidConfiguration)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceError {
    InvalidConfiguration,
    ProtocolMismatch,
    Unauthorized,
    InvalidRequest,
    CursorExpired,
    Conflict,
    Unavailable,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "telemetry service configuration is invalid",
            Self::ProtocolMismatch => "telemetry protocol is unsupported",
            Self::Unauthorized => "telemetry authorization failed",
            Self::InvalidRequest => "telemetry request is invalid",
            Self::CursorExpired => "telemetry cursor is outside retention",
            Self::Conflict => "telemetry source offset conflicts",
            Self::Unavailable => "telemetry storage is unavailable",
        })
    }
}

impl Error for ServiceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_control_plane::ObservabilityQuery;
    use mako_storage::MemoryAdapter;

    fn tenant(suffix: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(format!("prj_{suffix}00000000")).expect("project"),
            EnvironmentId::parse(format!("env_{suffix}00000000")).expect("environment"),
        )
    }

    fn store(adapter: MemoryAdapter) -> TelemetryStore {
        TelemetryStore::with_durability(
            Arc::new(adapter),
            "us-east-1-beta".to_owned(),
            Duration::from_secs(90 * 24 * 60 * 60),
            Arc::new(TelemetryRedactor::new(std::iter::empty::<&str>()).expect("redactor")),
            Durability::Memory,
        )
        .expect("store")
    }

    fn record(tenant: TenantScope, timestamp: u64, message: &str) -> ObservabilityRecord {
        ObservabilityRecord {
            tenant,
            timestamp_unix_milliseconds: timestamp,
            payload: ObservabilityPayload::ProjectLog {
                source: "edge-runtime".to_owned(),
                level: "info".to_owned(),
                message: message.to_owned(),
                correlation_id: "req_telemetry01".to_owned(),
            },
        }
    }

    fn ingest(
        tenant: TenantScope,
        timestamp: u64,
        offset: u64,
        message: &str,
    ) -> TelemetryIngestRequest {
        TelemetryIngestRequest {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: format!("req_ingest{offset:08}"),
            source: "edge-runtime".to_owned(),
            offset,
            records: vec![record(tenant, timestamp, message)],
        }
    }

    #[test]
    fn durable_source_offsets_are_idempotent_and_conflict_on_changed_replay() {
        let adapter = MemoryAdapter::new();
        let first_store = store(adapter.clone());
        let now = 1_800_000_000_000;
        let request = ingest(tenant("alpha"), now - 1_000, 1, "safe");
        let first = block_on(first_store.ingest(request.clone(), now)).expect("first ingest");
        assert!(!first.replayed);
        let restarted = store(adapter);
        let replay = block_on(restarted.ingest(request, now)).expect("replay");
        assert!(replay.replayed);
        let changed = ingest(tenant("alpha"), now - 1_000, 1, "changed");
        assert_eq!(
            block_on(restarted.ingest(changed, now)),
            Err(ServiceError::Conflict)
        );
    }

    #[test]
    fn queries_are_tenant_scoped_paginated_and_redacted_before_storage() {
        let adapter = MemoryAdapter::new();
        let store = store(adapter);
        let now = now_milliseconds().expect("clock");
        let alpha = tenant("alpha");
        let beta = tenant("beta");
        block_on(store.ingest(
            TelemetryIngestRequest {
                records: vec![
                    record(
                        alpha.clone(),
                        now - 2_000,
                        "Authorization: Bearer secret-token",
                    ),
                    record(alpha.clone(), now - 1_000, "safe second"),
                ],
                ..ingest(alpha.clone(), now - 2_000, 1, "unused")
            },
            now,
        ))
        .expect("alpha ingest");
        block_on(store.ingest(ingest(beta, now - 500, 2, "beta only"), now)).expect("beta ingest");
        let first = block_on(store.query(TelemetryQueryRequest {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: "req_query00000001".to_owned(),
            tenant: alpha.clone(),
            signal: ObservabilitySignal::ProjectLog,
            query: ObservabilityQuery {
                cursor: None,
                from_unix_milliseconds: Some(now - 10_000),
                until_unix_milliseconds: Some(now),
                limit: 1,
            },
            observed_at_unix_milliseconds: now,
        }))
        .expect("first page");
        assert_eq!(first.page.items.len(), 1);
        let ObservabilityPayload::ProjectLog { message, .. } = &first.page.items[0].payload else {
            panic!("project log");
        };
        assert!(!message.contains("secret-token"));
        let second = block_on(store.query(TelemetryQueryRequest {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: "req_query00000002".to_owned(),
            tenant: alpha,
            signal: ObservabilitySignal::ProjectLog,
            query: ObservabilityQuery {
                cursor: first.page.next_cursor,
                from_unix_milliseconds: Some(now - 10_000),
                until_unix_milliseconds: Some(now),
                limit: 10,
            },
            observed_at_unix_milliseconds: now,
        }))
        .expect("second page");
        assert_eq!(second.page.items.len(), 1);
    }

    fn usage(tenant: TenantScope, timestamp: u64, quantity: u64) -> ObservabilityRecord {
        ObservabilityRecord {
            tenant,
            timestamp_unix_milliseconds: timestamp,
            payload: ObservabilityPayload::Usage {
                resource: mako_api::QuotaResource::ReplicationRequestsPerMinute,
                quantity,
                unit: "requests".to_owned(),
            },
        }
    }

    fn checkpoint(tenant: TenantScope, window_start: u64, consumed: u64) -> ObservabilityRecord {
        ObservabilityRecord {
            tenant,
            timestamp_unix_milliseconds: window_start,
            payload: ObservabilityPayload::Quota {
                resource: mako_api::QuotaResource::ReplicationRequestsPerMinute,
                limit: 120,
                consumed,
                retry_after_unix_milliseconds: None,
            },
        }
    }

    fn health_alerts(store: &TelemetryStore, tenant: TenantScope, now: u64) -> Vec<String> {
        block_on(store.query(TelemetryQueryRequest {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: "req_query_alerts1".to_owned(),
            tenant,
            signal: ObservabilitySignal::Health,
            query: ObservabilityQuery {
                cursor: None,
                from_unix_milliseconds: None,
                until_unix_milliseconds: Some(now),
                limit: 10,
            },
            observed_at_unix_milliseconds: now,
        }))
        .expect("health query")
        .page
        .items
        .into_iter()
        .map(|record| match record.payload {
            ObservabilityPayload::Health {
                status, diagnostic, ..
            } => {
                assert_eq!(status, HealthState::Degraded);
                diagnostic.unwrap_or_default()
            }
            other => panic!("expected a health record, got {other:?}"),
        })
        .collect()
    }

    /// The ledger and the enforcement counters watch the same admitted
    /// requests; when a checkpoint arrives, the store must compare and only
    /// call out a difference that matters. An agreeing checkpoint must stay
    /// silent, or the alert would train everyone to ignore it.
    #[test]
    fn a_checkpoint_is_compared_against_the_ledger_and_material_divergence_alerts() {
        let adapter = MemoryAdapter::new();
        let store = store(adapter);
        let now = now_milliseconds().expect("clock");
        let window = (now - 180_000) / 60_000 * 60_000;
        let alpha = tenant("alpha");

        // Three admitted requests in the window, as the plane reported them.
        block_on(store.ingest(
            TelemetryIngestRequest {
                records: vec![
                    usage(alpha.clone(), window, 1),
                    usage(alpha.clone(), window + 1_000, 1),
                    usage(alpha.clone(), window + 59_999, 1),
                    // The neighboring minute must not leak into the sum.
                    usage(alpha.clone(), window + 60_000, 1),
                ],
                ..ingest(alpha.clone(), window, 1, "unused")
            },
            now,
        ))
        .expect("usage ingest");

        // Enforcement counted the same three: no alert.
        block_on(store.ingest(
            TelemetryIngestRequest {
                records: vec![checkpoint(alpha.clone(), window, 3)],
                ..ingest(alpha.clone(), window, 2, "unused")
            },
            now,
        ))
        .expect("agreeing checkpoint");
        assert_eq!(
            health_alerts(&store, alpha.clone(), now),
            Vec::<String>::new(),
            "an agreeing checkpoint raised an alert"
        );

        // Enforcement counted far more than the ledger kept: the ledger is
        // losing records, and someone must hear about it.
        block_on(store.ingest(
            TelemetryIngestRequest {
                records: vec![checkpoint(alpha.clone(), window, 50)],
                ..ingest(alpha.clone(), window, 3, "unused")
            },
            now,
        ))
        .expect("divergent checkpoint");
        let alerts = health_alerts(&store, alpha, now);
        assert_eq!(alerts.len(), 1, "material divergence did not alert");
        assert!(
            alerts[0].contains("cross-check")
                && alerts[0].contains("ledger says 3")
                && alerts[0].contains("counter says 50"),
            "the alert does not say what diverged: {}",
            alerts[0]
        );
    }

    /// A log line is customer-written text; whatever producer sends it, the
    /// stored copy must have credentials and addresses masked, because the
    /// store is the one choke point every producer passes through.
    #[test]
    fn a_project_log_is_scrubbed_before_it_is_stored() {
        let adapter = MemoryAdapter::new();
        let store = store(adapter);
        let now = now_milliseconds().expect("clock");
        let alpha = tenant("alpha");
        block_on(store.ingest(
            TelemetryIngestRequest {
                records: vec![ObservabilityRecord {
                    tenant: alpha.clone(),
                    timestamp_unix_milliseconds: now - 1_000,
                    payload: ObservabilityPayload::ProjectLog {
                        source: "function:checkout".to_owned(),
                        level: "error".to_owned(),
                        message: "card declined for bob@example.com password=hunter2".to_owned(),
                        correlation_id: "req_scrub000001".to_owned(),
                    },
                }],
                ..ingest(alpha.clone(), now - 1_000, 1, "unused")
            },
            now,
        ))
        .expect("log ingest");
        let page = block_on(store.query(TelemetryQueryRequest {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: "req_query_scrub01".to_owned(),
            tenant: alpha,
            signal: ObservabilitySignal::ProjectLog,
            query: ObservabilityQuery {
                cursor: None,
                from_unix_milliseconds: None,
                until_unix_milliseconds: Some(now),
                limit: 10,
            },
            observed_at_unix_milliseconds: now,
        }))
        .expect("log query");
        assert_eq!(page.page.items.len(), 1);
        let ObservabilityPayload::ProjectLog { message, .. } = &page.page.items[0].payload else {
            panic!("expected a project log");
        };
        assert_eq!(
            message, "card declined for b***@example.com password=[REDACTED]",
            "the stored line still carries the address or the password"
        );
    }

    /// Replaying a checkpoint batch must not repeat its alert: redelivery is
    /// the transfer protocol working as designed, not a second divergence.
    #[test]
    fn a_replayed_checkpoint_batch_does_not_alert_twice() {
        let adapter = MemoryAdapter::new();
        let store = store(adapter);
        let now = now_milliseconds().expect("clock");
        let window = (now - 180_000) / 60_000 * 60_000;
        let alpha = tenant("alpha");
        let divergent = TelemetryIngestRequest {
            records: vec![checkpoint(alpha.clone(), window, 50)],
            ..ingest(alpha.clone(), window, 1, "unused")
        };
        block_on(store.ingest(divergent.clone(), now)).expect("first delivery");
        let replay = block_on(store.ingest(divergent, now)).expect("redelivery");
        assert!(replay.replayed);
        assert_eq!(
            health_alerts(&store, alpha, now).len(),
            1,
            "a replayed batch was compared and alerted again"
        );
    }
}
