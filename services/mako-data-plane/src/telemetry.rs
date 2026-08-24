//! Ship observability records from the data plane to the telemetry service.
//!
//! The telemetry store has always had an ingest endpoint and nothing has ever
//! called it, so every signal the management API serves from it — usage,
//! health, replication errors, authentication events, function metrics, project
//! logs, index state — answered from an empty store. This is the missing half.
//!
//! Emitting must never be able to fail a request that a tenant paid for with a
//! quota charge. Records are therefore buffered without blocking, the buffer is
//! bounded, and a worker drains it. When the buffer is full or telemetry is
//! down, records are dropped and counted rather than queued without limit or
//! pushed back onto the request path.

use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use mako_api::{
    EnvironmentId, ObservabilityPayload, ObservabilityRecord, ProjectId, QuotaResource,
    TELEMETRY_AUTHORIZATION_HEADER, TELEMETRY_INGEST_PATH, TELEMETRY_PROTOCOL_VERSION,
    TELEMETRY_VERSION_HEADER, TelemetryIngestRequest, TenantScope,
};
use mako_storage::{KvAdapter, TenantKeyspace};

/// Matches the telemetry service's own per-request ceiling.
const MAX_BATCH_RECORDS: usize = 256;
/// A few batches of headroom. Beyond this the deployment is not keeping up and
/// dropping is the correct answer, because the alternative is unbounded memory
/// on the path that serves documents.
const MAX_BUFFERED_RECORDS: usize = 4_096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct TelemetryEmitter {
    endpoint: SocketAddr,
    credential: String,
    source: String,
    buffered: Mutex<VecDeque<ObservabilityRecord>>,
    offset: AtomicU64,
    dropped: AtomicU64,
    delivered: AtomicU64,
}

impl std::fmt::Debug for TelemetryEmitter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelemetryEmitter")
            .field("endpoint", &self.endpoint)
            .field("source", &self.source)
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

impl TelemetryEmitter {
    #[must_use]
    pub fn new(endpoint: SocketAddr, credential: impl Into<String>, source: &str) -> Self {
        Self {
            endpoint,
            credential: credential.into(),
            source: source.to_owned(),
            buffered: Mutex::new(VecDeque::new()),
            offset: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            delivered: AtomicU64::new(0),
        }
    }

    /// Buffer a record. Never blocks on the network and never fails a request.
    pub fn record(&self, record: ObservabilityRecord) {
        let Ok(mut buffered) = self.buffered.lock() else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if buffered.len() >= MAX_BUFFERED_RECORDS {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        buffered.push_back(record);
    }

    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// How many records are waiting to be shipped.
    #[cfg(test)]
    #[must_use]
    pub fn buffered_for_test(&self) -> usize {
        self.buffered
            .lock()
            .map(|buffered| buffered.len())
            .unwrap_or(0)
    }

    #[must_use]
    pub fn delivered(&self) -> u64 {
        self.delivered.load(Ordering::Relaxed)
    }

    fn take_batch(&self) -> Vec<ObservabilityRecord> {
        let Ok(mut buffered) = self.buffered.lock() else {
            return Vec::new();
        };
        let take = buffered.len().min(MAX_BATCH_RECORDS);
        buffered.drain(..take).collect()
    }

    fn requeue(&self, batch: Vec<ObservabilityRecord>) {
        let Ok(mut buffered) = self.buffered.lock() else {
            self.dropped
                .fetch_add(batch.len() as u64, Ordering::Relaxed);
            return;
        };
        // A failed batch goes back in front so ordering survives a retry, but
        // only as far as the bound allows; the rest is dropped and counted.
        for record in batch.into_iter().rev() {
            if buffered.len() >= MAX_BUFFERED_RECORDS {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            buffered.push_front(record);
        }
    }

    /// Deliver at most one batch. Returns how many records were accepted.
    pub fn flush_once(&self) -> usize {
        let batch = self.take_batch();
        if batch.is_empty() {
            return 0;
        }
        let count = batch.len();
        // The offset makes a redelivered batch identifiable to the store. It
        // advances only when a batch is claimed, so a retry reuses its own.
        let offset = self.offset.fetch_add(1, Ordering::Relaxed) + 1;
        let request_id = format!("req_dataplane{offset:016x}");
        let payload = TelemetryIngestRequest {
            protocol_version: TELEMETRY_PROTOCOL_VERSION,
            request_id: request_id.clone(),
            source: self.source.clone(),
            offset,
            records: batch.clone(),
        };
        match self.deliver(&payload, &request_id) {
            Ok(()) => {
                self.delivered.fetch_add(count as u64, Ordering::Relaxed);
                count
            }
            Err(()) => {
                self.requeue(batch);
                0
            }
        }
    }

    fn deliver(&self, payload: &TelemetryIngestRequest, request_id: &str) -> Result<(), ()> {
        let body = serde_json::to_vec(payload).map_err(|_| ())?;
        let mut stream =
            TcpStream::connect_timeout(&self.endpoint, CONNECT_TIMEOUT).map_err(|_| ())?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(|_| ())?;
        stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(|_| ())?;

        let mut head = String::new();
        head.push_str(&format!("POST {TELEMETRY_INGEST_PATH} HTTP/1.1\r\n"));
        head.push_str(&format!("host: {}\r\n", self.endpoint));
        head.push_str("connection: close\r\n");
        head.push_str("content-type: application/json\r\n");
        head.push_str(&format!("content-length: {}\r\n", body.len()));
        head.push_str(&format!(
            "{TELEMETRY_VERSION_HEADER}: {TELEMETRY_PROTOCOL_VERSION}\r\n"
        ));
        head.push_str(&format!(
            "{TELEMETRY_AUTHORIZATION_HEADER}: {}\r\n",
            self.credential
        ));
        head.push_str(&format!("x-mako-request-id: {request_id}\r\n"));
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).map_err(|_| ())?;
        stream.write_all(&body).map_err(|_| ())?;
        stream.flush().map_err(|_| ())?;

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).map_err(|_| ())?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or(())?;
        let mut rest = Vec::new();
        let _ = reader.read_to_end(&mut rest);
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(())
        }
    }
}

/// How often a tenant's stored size is re-measured.
///
/// Stored size only moves when a tenant writes, so nothing is gained by
/// sampling one that is idle: its last sample is still true. Writes mark a
/// tenant due and the sample is taken off the request path, because measuring
/// walks the tenant's range and a document write must not pay for that.
const STORAGE_SAMPLE_INTERVAL: Duration = Duration::from_secs(300);
/// Bounded so a burst across many tenants cannot grow this without limit.
const MAX_PENDING_SAMPLES: usize = 4_096;

pub struct StorageSampler {
    due: Mutex<HashMap<(String, String), Instant>>,
    last_sampled: Mutex<HashMap<(String, String), Instant>>,
}

impl Default for StorageSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageSampler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            due: Mutex::new(HashMap::new()),
            last_sampled: Mutex::new(HashMap::new()),
        }
    }

    /// Note that a tenant's stored size may have changed.
    pub fn mark(&self, tenant: &TenantScope) {
        let key = tenant_key(tenant);
        if let Ok(last) = self.last_sampled.lock()
            && last
                .get(&key)
                .is_some_and(|at| at.elapsed() < STORAGE_SAMPLE_INTERVAL)
        {
            return;
        }
        let Ok(mut due) = self.due.lock() else {
            return;
        };
        if due.len() >= MAX_PENDING_SAMPLES {
            return;
        }
        due.entry(key).or_insert_with(Instant::now);
    }

    fn take_due(&self) -> Vec<(String, String)> {
        let Ok(mut due) = self.due.lock() else {
            return Vec::new();
        };
        due.drain().map(|(key, _)| key).collect()
    }

    fn record_sampled(&self, key: (String, String)) {
        if let Ok(mut last) = self.last_sampled.lock() {
            last.insert(key, Instant::now());
        }
    }

    /// Measure every tenant marked since the last pass and report each as
    /// usage. Returns how many were sampled.
    pub async fn sample_due(
        &self,
        adapter: &Arc<dyn KvAdapter>,
        emitter: &TelemetryEmitter,
        now_unix_seconds: u64,
    ) -> usize {
        let mut sampled = 0;
        for key in self.take_due() {
            let Ok(keyspace) = TenantKeyspace::new(key.0.as_bytes(), key.1.as_bytes()) else {
                continue;
            };
            let Ok(range) = keyspace.tenant_range() else {
                continue;
            };
            let Ok(bytes) = adapter.stored_bytes(range).await else {
                // Leave it unmarked so the next write marks it again rather
                // than recording a sample that never happened.
                continue;
            };
            let (Ok(project), Ok(environment)) = (
                ProjectId::parse(key.0.clone()),
                EnvironmentId::parse(key.1.clone()),
            ) else {
                continue;
            };
            emitter.record(ObservabilityRecord {
                tenant: TenantScope::new(project, environment),
                timestamp_unix_milliseconds: now_unix_seconds.saturating_mul(1_000),
                payload: ObservabilityPayload::Usage {
                    resource: QuotaResource::StorageBytes,
                    quantity: bytes,
                    unit: "bytes".to_owned(),
                },
            });
            self.record_sampled(key);
            sampled += 1;
        }
        sampled
    }
}

fn tenant_key(tenant: &TenantScope) -> (String, String) {
    (
        tenant.project_id().as_str().to_owned(),
        tenant.environment_id().as_str().to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mako_storage::{Durability, MemoryAdapter, WriteBatch};

    use super::*;

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_sampler0001").expect("project"),
            EnvironmentId::parse("env_sampler0001").expect("environment"),
        )
    }

    fn emitter() -> TelemetryEmitter {
        // Never flushed in these tests, so the endpoint is never contacted.
        TelemetryEmitter::new(
            "127.0.0.1:1".parse().expect("address"),
            "0123456789abcdef0123456789abcdef",
            "mako.test",
        )
    }

    #[test]
    fn a_marked_tenant_is_sampled_once_and_not_again_until_the_interval_passes() {
        block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let keyspace = TenantKeyspace::new(
                tenant().project_id().as_str().as_bytes(),
                tenant().environment_id().as_str().as_bytes(),
            )
            .expect("keyspace");
            let mut batch = WriteBatch::new();
            batch.put(
                keyspace.tenant_range().expect("range").start_inclusive,
                b"x",
            );
            adapter
                .write(batch, Durability::Memory)
                .await
                .expect("write");

            let sampler = StorageSampler::new();
            let emitter = emitter();

            // Nothing is measured until a write says the size may have moved.
            assert_eq!(sampler.sample_due(&adapter, &emitter, 1).await, 0);

            sampler.mark(&tenant());
            assert_eq!(sampler.sample_due(&adapter, &emitter, 1).await, 1);
            assert_eq!(emitter.buffered_for_test(), 1);

            // A tenant that keeps writing is not re-measured on every write:
            // measuring walks its range, and the answer barely moves.
            sampler.mark(&tenant());
            sampler.mark(&tenant());
            assert_eq!(
                sampler.sample_due(&adapter, &emitter, 2).await,
                0,
                "a tenant was re-measured inside the sampling interval"
            );
            assert_eq!(emitter.buffered_for_test(), 1);
        });
    }
}
