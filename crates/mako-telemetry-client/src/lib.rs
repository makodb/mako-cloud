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
    /// A batch that was claimed but not yet accepted by the store. It keeps
    /// its offset across retries: the store's checkpoint advances one offset
    /// at a time, so a batch that burns a fresh offset per attempt runs away
    /// from the checkpoint and the stream never recovers.
    in_flight: Mutex<Option<(u64, Vec<ObservabilityRecord>)>>,
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
        // The store's checkpoint for a source survives this process, but the
        // offset counter here does not, so a reused source name would resume
        // at one against a checkpoint far ahead and be refused forever. A
        // per-boot suffix starts a fresh stream instead; the abandoned
        // checkpoint row is a few bytes of history, not a leak.
        let boot = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let source = format!("{source}.{:x}-{:x}", std::process::id(), boot);
        Self {
            endpoint,
            credential: credential.into(),
            source: source.chars().take(128).collect(),
            buffered: Mutex::new(VecDeque::new()),
            in_flight: Mutex::new(None),
            offset: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            delivered: AtomicU64::new(0),
        }
    }

    /// Buffer a record. Never blocks on the network and never fails a request.
    pub fn record(&self, record: ObservabilityRecord) {
        let _ = self.try_record(record);
    }

    /// Buffer a record, saying whether it was actually kept. A producer that
    /// can wait -- a collector pass, not a request path -- should stop on
    /// `false` instead of letting its progress mark advance past records
    /// this buffer shed.
    pub fn try_record(&self, record: ObservabilityRecord) -> bool {
        let Ok(mut buffered) = self.buffered.lock() else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        if buffered.len() >= MAX_BUFFERED_RECORDS {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        buffered.push_back(record);
        true
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

    /// Deliver at most one batch. Returns how many records were accepted.
    pub fn flush_once(&self) -> usize {
        // A batch that failed to deliver is retried before anything new is
        // taken, under the offset it was claimed with: the store recognizes
        // the redelivery by digest, and the stream stays contiguous.
        let (offset, batch) = {
            let Ok(mut in_flight) = self.in_flight.lock() else {
                return 0;
            };
            match in_flight.take() {
                Some(claimed) => claimed,
                None => {
                    let batch = self.take_batch();
                    if batch.is_empty() {
                        return 0;
                    }
                    (self.offset.fetch_add(1, Ordering::Relaxed) + 1, batch)
                }
            }
        };
        let count = batch.len();
        let request_id = format!("req_telemetry{offset:016x}");
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
                if let Ok(mut in_flight) = self.in_flight.lock() {
                    *in_flight = Some((offset, batch));
                }
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

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    use mako_api::{
        EnvironmentId, ObservabilityPayload, ObservabilityRecord, ProjectId, QuotaResource,
        TenantScope,
    };

    use super::*;

    fn record() -> ObservabilityRecord {
        ObservabilityRecord {
            tenant: TenantScope::new(
                ProjectId::parse("prj_emitter00001").expect("project"),
                EnvironmentId::parse("env_emitter00001").expect("environment"),
            ),
            timestamp_unix_milliseconds: 1,
            payload: ObservabilityPayload::Usage {
                resource: QuotaResource::ReplicationRequestsPerMinute,
                quantity: 1,
                unit: "requests".to_owned(),
            },
        }
    }

    /// A failed batch must retry under the offset it was claimed with. If a
    /// retry claimed a fresh offset, every failure would push the client one
    /// step further ahead of the store's checkpoint and the stream would
    /// never deliver again.
    #[test]
    fn a_failed_batch_retries_under_its_own_offset() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let endpoint = listener.local_addr().expect("address");
        let (sent, seen) = mpsc::channel::<u64>();
        let server = thread::spawn(move || {
            // First attempt is refused, second accepted; each reports the
            // offset it carried.
            for (index, stream) in listener.incoming().take(2).enumerate() {
                let mut stream = stream.expect("connection");
                // Read headers, then exactly the advertised body: the client
                // holds its socket open for the response, so reading to EOF
                // would deadlock against it.
                let mut raw = Vec::new();
                let mut byte = [0_u8; 1];
                while !raw.ends_with(b"\r\n\r\n") && matches!(stream.read(&mut byte), Ok(1)) {
                    raw.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&raw).to_string();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .expect("request advertises a body length");
                let mut body = vec![0_u8; length];
                stream.read_exact(&mut body).expect("body");
                raw.extend_from_slice(&body);
                let text = String::from_utf8_lossy(&raw);
                let offset = text
                    .rfind("\"offset\":")
                    .map(|at| {
                        text[at + 9..]
                            .chars()
                            .take_while(char::is_ascii_digit)
                            .collect::<String>()
                    })
                    .and_then(|digits| digits.parse::<u64>().ok())
                    .expect("payload carries an offset");
                sent.send(offset).expect("report offset");
                let status = if index == 0 {
                    "HTTP/1.1 503 Service Unavailable"
                } else {
                    "HTTP/1.1 200 OK"
                };
                let _ = stream.write_all(
                    format!("{status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                        .as_bytes(),
                );
            }
        });

        let emitter =
            TelemetryEmitter::new(endpoint, "0123456789abcdef0123456789abcdef", "mako.test");
        emitter.record(record());
        assert_eq!(
            emitter.flush_once(),
            0,
            "the refused batch counted as delivered"
        );
        assert_eq!(emitter.flush_once(), 1, "the retry was not delivered");
        server.join().expect("server");

        let first = seen.recv().expect("first offset");
        let second = seen.recv().expect("second offset");
        assert_eq!(
            (first, second),
            (1, 1),
            "the retry did not reuse the claimed offset"
        );
    }
}
