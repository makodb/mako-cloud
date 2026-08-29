//! A loopback HTTP endpoint that keeps every webhook delivery it is handed,
//! and can play an endpoint that is down for a while first.
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// One request the sink accepted, headers lower-cased.
#[derive(Clone, Debug)]
pub struct CapturedDelivery {
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

pub struct WebhookSinkStub {
    pub url: String,
    deliveries: Arc<Mutex<Vec<CapturedDelivery>>>,
    refusals: Arc<AtomicUsize>,
    refused: Arc<AtomicUsize>,
}

impl WebhookSinkStub {
    /// Answers the first `refuse_first` requests with `503` and keeps none of
    /// them; every request after that is stored and answered `200`.
    pub fn start(refuse_first: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("sink listener");
        let port = listener.local_addr().expect("sink address").port();
        let deliveries = Arc::new(Mutex::new(Vec::new()));
        let refusals = Arc::new(AtomicUsize::new(refuse_first));
        let refused = Arc::new(AtomicUsize::new(0));
        let (captured, remaining, count) = (
            Arc::clone(&deliveries),
            Arc::clone(&refusals),
            Arc::clone(&refused),
        );
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Some((headers, body)) = read_request(&mut stream) else {
                    continue;
                };
                let refuse = remaining
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                        (left > 0).then(|| left - 1)
                    })
                    .is_ok();
                let (status, reason) = if refuse {
                    count.fetch_add(1, Ordering::SeqCst);
                    (503, "Service Unavailable")
                } else {
                    captured
                        .lock()
                        .expect("deliveries")
                        .push(CapturedDelivery { headers, body });
                    (200, "OK")
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.flush();
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}/hooks/mako"),
            deliveries,
            refusals,
            refused,
        }
    }

    /// Make the next `count` requests fail again.
    pub fn refuse_next(&self, count: usize) {
        self.refusals.store(count, Ordering::SeqCst);
    }

    /// How many requests were refused so far.
    pub fn refused(&self) -> usize {
        self.refused.load(Ordering::SeqCst)
    }

    pub fn deliveries(&self) -> Vec<CapturedDelivery> {
        self.deliveries.lock().expect("deliveries").clone()
    }

    /// Waits until at least `count` deliveries were accepted.
    pub fn wait_for(&self, count: usize, timeout: Duration) -> Vec<CapturedDelivery> {
        let deadline = Instant::now() + timeout;
        loop {
            let seen = self.deliveries();
            if seen.len() >= count || Instant::now() >= deadline {
                return seen;
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<(BTreeMap<String, String>, String)> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 8192];
    let (head_end, length) = loop {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..end]).to_string();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse().ok())
                })
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while raw.len() < head_end + length {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
    }
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let body = String::from_utf8_lossy(&raw[head_end..head_end + length.min(raw.len() - head_end)])
        .into_owned();
    Some((headers, body))
}
