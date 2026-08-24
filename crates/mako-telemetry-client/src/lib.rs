#![forbid(unsafe_code)]

//! Ship observability records to the telemetry service.
//!
//! The telemetry store has always had an ingest endpoint and nothing ever
//! called it, so every signal the management API serves from it answered from
//! an empty store. This is the emitting half, shared by every service that
//! observes work: the data plane sees documents and sessions, the edge gateway
//! sees invocations, and neither should carry its own copy of this.
//!
//! Emitting must never be able to fail a request that a tenant paid for with a
//! quota charge. Records are therefore buffered without blocking, the buffer is
//! bounded, and a worker drains it. When the buffer is full or telemetry is
//! down, records are dropped and counted rather than queued without limit or
//! pushed back onto the request path.

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use mako_api::{
    ObservabilityRecord, TELEMETRY_AUTHORIZATION_HEADER, TELEMETRY_INGEST_PATH,
    TELEMETRY_PROTOCOL_VERSION, TELEMETRY_VERSION_HEADER, TelemetryIngestRequest,
};

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
    ///
    /// A depth that keeps climbing means telemetry is not keeping up and
    /// records are about to be dropped, so this is worth reporting rather than
    /// keeping for tests.
    #[must_use]
    pub fn buffered(&self) -> usize {
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
