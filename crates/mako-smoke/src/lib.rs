//! Harness for driving the real Mako Cloud service binaries from a test.
//!
//! Every other end-to-end suite in this repository runs against a mock. The
//! tests in this crate start the actual processes and speak HTTP to them, so
//! what they prove is that the server implements its side of the protocol.
//! The pieces they share — process supervision, a blocking HTTP client, tenant
//! configuration confined to a temporary directory — live here.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use serde_json::Value;

mod dns_stub;
mod sign_in;
mod webhook_sink;
pub use dns_stub::DnsStub;
pub use sign_in::{CapturedMail, OidcProviderStub, SmtpCaptureStub};
pub use webhook_sink::{CapturedDelivery, WebhookSinkStub};

pub const READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// The shared secret every smoke service is configured with.
pub const INTERNAL_AUTH_SECRET: &str =
    "5f4e3d2c1b0a998877665544332211000112233445566778899aabbccddeeff0";

/// Terminates a spawned service even when an assertion unwinds, so a failing
/// run never leaves a process holding a database lock.
pub struct ServiceProcess {
    pub name: &'static str,
    pub port: u16,
    pub child: Child,
    stopped: bool,
}

impl ServiceProcess {
    /// Stop the service and wait until its port stops answering.
    ///
    /// Injecting a dependency outage means the dependency has to be genuinely
    /// gone before the next request, not merely signalled.
    pub fn stop(mut self) {
        self.terminate();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", self.port)).is_err() {
                return;
            }
            sleep(Duration::from_millis(100));
        }
        panic!(
            "{} kept answering on {} after being killed",
            self.name, self.port
        );
    }

    fn terminate(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Err(error) = self.child.kill() {
            eprintln!("{} could not be terminated: {error}", self.name);
        }
        let _ = self.child.wait();
    }
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Where the service binaries live: `MAKO_SMOKE_BINARY_DIR`, else the target
/// directory cargo is actually using, else the workspace's own.
///
/// Honouring `CARGO_TARGET_DIR` matters more than it looks. A run with a
/// custom target directory builds its test binaries there and leaves
/// `<workspace>/target/debug` untouched, so the suite used to pick up
/// whatever service binaries happened to be lying there -- yesterday's, or
/// none at all. A smoke test that exercises a service it did not build proves
/// nothing, and says so in the most confusing way available: a readiness
/// timeout in a service whose source is fine.
pub fn binary_directory() -> PathBuf {
    if let Ok(configured) = std::env::var("MAKO_SMOKE_BINARY_DIR") {
        return PathBuf::from(configured);
    }
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(target).join("debug");
    }
    workspace_root().join("target").join("debug")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Refuse to run against binaries older than the sources they are meant to
/// exercise. Silently testing a stale build is worse than not testing: it
/// reports on code nobody wrote and hides the code somebody did.
fn assert_binaries_are_current(binaries: &Path, name: &str) {
    // The newest binary in the directory, not this one: cargo relinks only
    // what a change actually reached, so a data-plane binary is legitimately
    // older than a control-plane edit. What is never legitimate is every
    // binary predating the newest source -- that is a tree nobody rebuilt.
    let Ok(entries) = std::fs::read_dir(binaries) else {
        return;
    };
    let built = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.metadata().and_then(|meta| meta.modified()).ok())
        .max();
    let Some(built) = built else {
        return;
    };
    let root = workspace_root();
    let mut newest_source: Option<(std::time::SystemTime, PathBuf)> = None;
    for area in ["crates", "services"] {
        let mut pending = vec![root.join(area)];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Only what compiles into a service counts. A test, a
                    // benchmark, or this harness itself can change without
                    // making a built binary stale, and a guard that fires on
                    // those is one people learn to ignore.
                    let name = path.file_name().unwrap_or_default();
                    let skip = matches!(
                        name.to_str(),
                        Some("target" | "tests" | "benches" | "examples" | "mako-smoke")
                    );
                    if !skip {
                        pending.push(path);
                    }
                } else if path.extension().is_some_and(|extension| extension == "rs")
                    && let Ok(modified) = entry.metadata().and_then(|meta| meta.modified())
                    && newest_source
                        .as_ref()
                        .is_none_or(|(seen, _)| modified > *seen)
                {
                    newest_source = Some((modified, path));
                }
            }
        }
    }
    if let Some((modified, path)) = newest_source
        && modified > built
    {
        let relative = path.strip_prefix(&root).unwrap_or(&path).display();
        panic!(
            "every binary in {1} is older than {relative} (checked for {0}); rebuild before \
             running the smoke suite \
             (`cargo build --workspace --bins`, and set MAKO_SMOKE_BINARY_DIR when \
             CARGO_TARGET_DIR points elsewhere)",
            name,
            binaries.display()
        );
    }
}

pub fn scratch_root() -> PathBuf {
    std::env::var("MAKO_STORAGE_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

/// Configuration for a throwaway tenant confined to one temporary directory.
pub fn service_environment(root: &Path) -> BTreeMap<String, String> {
    let path = |segment: &str| root.join(segment).to_string_lossy().into_owned();
    for directory in [
        "rocksdb",
        "control",
        "control/migration",
        "control/staging",
        "control/published",
        "control/restore",
        "control/reserve",
        "backups",
    ] {
        std::fs::create_dir_all(root.join(directory)).expect("smoke directories");
    }
    BTreeMap::from([
        ("MAKO_ENVIRONMENT".to_owned(), "local".to_owned()),
        ("MAKO_REGION".to_owned(), "local".to_owned()),
        ("MAKO_ROCKSDB_PATH".to_owned(), path("rocksdb")),
        (
            "MAKO_ROCKSDB_BACKUP_DESTINATION".to_owned(),
            path("backups"),
        ),
        (
            "MAKO_CONTROL_SQLITE_PATH".to_owned(),
            path("control/control.sqlite3"),
        ),
        (
            "MAKO_CONTROL_SQLITE_LOCK_PATH".to_owned(),
            path("control/control.sqlite3.lock"),
        ),
        (
            "MAKO_CONTROL_SQLITE_IDENTITY".to_owned(),
            "mako-control-smoke".to_owned(),
        ),
        (
            "MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE".to_owned(),
            path("control/migration"),
        ),
        (
            "MAKO_CONTROL_SQLITE_BACKUP_STAGING".to_owned(),
            path("control/staging"),
        ),
        (
            "MAKO_CONTROL_SQLITE_BACKUP_PUBLISH".to_owned(),
            path("control/published"),
        ),
        (
            "MAKO_CONTROL_SQLITE_RESTORE_WORKSPACE".to_owned(),
            path("control/restore"),
        ),
        (
            "MAKO_CONTROL_SQLITE_RESERVE_PATH".to_owned(),
            path("control/reserve"),
        ),
        // Secrets are references, never inline values, so the run supplies both
        // the variable and the reference that points at it.
        // 64 hexadecimal characters, because the same secret derives the
        // developer session signing key and `mako-control-session` refuses any
        // other shape.
        (
            "MAKO_INTERNAL_AUTH_SECRET".to_owned(),
            INTERNAL_AUTH_SECRET.to_owned(),
        ),
        (
            "MAKO_INTERNAL_AUTH_SECRET_REF".to_owned(),
            "env:MAKO_INTERNAL_AUTH_SECRET".to_owned(),
        ),
        (
            "MAKO_OBJECT_STORE_ACCESS_KEY".to_owned(),
            "smoke-object-store-access-key".to_owned(),
        ),
        (
            "MAKO_OBJECT_STORE_ACCESS_KEY_REF".to_owned(),
            "env:MAKO_OBJECT_STORE_ACCESS_KEY".to_owned(),
        ),
        (
            "MAKO_OBJECT_STORE_SECRET_KEY".to_owned(),
            "smoke-object-store-secret-key-0123456789".to_owned(),
        ),
        (
            "MAKO_OBJECT_STORE_SECRET_KEY_REF".to_owned(),
            "env:MAKO_OBJECT_STORE_SECRET_KEY".to_owned(),
        ),
    ])
}

pub fn run_bootstrap(binaries: &Path, environment: &BTreeMap<String, String>) -> Value {
    let executable = binaries.join("mako-local-bootstrap");
    assert!(
        executable.exists(),
        "{} is missing; build the workspace binaries first (cargo build --workspace --bins) or \
         set MAKO_SMOKE_BINARY_DIR",
        executable.display()
    );
    let output = Command::new(&executable)
        .envs(environment)
        .output()
        .expect("bootstrap runs");
    assert!(
        output.status.success(),
        "bootstrap failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("bootstrap reports json")
}

/// Mint a developer management session for the bootstrapped developer.
///
/// The control plane never issues one over HTTP for a locally seeded developer
/// — hosted registration does — so deployment tooling ships `mako-control-session`
/// for exactly this. The issuer has to be the control plane's own, which is
/// derived from its bind address when no public URL is configured.
pub fn mint_developer_session(
    binaries: &Path,
    root: &Path,
    control_port: u16,
    identity_id: &str,
    email: &str,
) -> String {
    mint_developer_session_with_secret(
        binaries,
        root,
        control_port,
        identity_id,
        email,
        INTERNAL_AUTH_SECRET,
    )
}

/// Like [`mint_developer_session`], for a stack whose internal secret is not
/// the shared default: the session must be signed with whatever secret the
/// control plane actually verifies with, or it answers 401.
pub fn mint_developer_session_with_secret(
    binaries: &Path,
    root: &Path,
    control_port: u16,
    identity_id: &str,
    email: &str,
    internal_secret: &str,
) -> String {
    let secret = root.join("internal-auth");
    std::fs::write(&secret, internal_secret).expect("secret file");
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600))
        .expect("secret file is private");
    // The tool refuses to overwrite an existing session file.
    let output = root.join(format!("session-{control_port}.jwt"));
    let _ = std::fs::remove_file(&output);

    let executable = binaries.join("mako-control-session");
    let result = Command::new(&executable)
        .args(["--secret-file", &secret.to_string_lossy()])
        .args(["--output", &output.to_string_lossy()])
        .args([
            "--issuer",
            &format!("http://127.0.0.1:{control_port}/control-identity"),
        ])
        .args(["--identity-id", identity_id])
        .args(["--email", email])
        .args(["--display-name", "Smoke Developer"])
        .args(["--credential-epoch", "1"])
        .args(["--authorization-epoch", "1"])
        .args(["--ttl-seconds", "3600"])
        .output()
        .expect("session tool runs");
    assert!(
        result.status.success(),
        "developer session was not issued: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::read_to_string(&output)
        .expect("session token")
        .trim()
        .to_owned()
}

pub fn start_service(
    name: &'static str,
    binaries: &Path,
    environment: &BTreeMap<String, String>,
    port: u16,
    log: PathBuf,
) -> ServiceProcess {
    let executable = binaries.join(name);
    assert!(
        executable.exists(),
        "{} is missing; build the workspace binaries first (cargo build --workspace --bins) or \
         set MAKO_SMOKE_BINARY_DIR",
        executable.display()
    );
    assert_binaries_are_current(binaries, name);
    let output = std::fs::File::create(&log).expect("service log");
    let errors = output.try_clone().expect("service log");
    let child = Command::new(&executable)
        .envs(environment)
        .env("MAKO_BIND_ADDR", format!("127.0.0.1:{port}"))
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(errors))
        .spawn()
        .unwrap_or_else(|error| panic!("{name} could not be started: {error}"));
    ServiceProcess {
        name,
        port,
        child,
        stopped: false,
    }
}

/// Each service refuses to serve until its readiness passes, so a ready service
/// is proof its storage and identity dependencies opened.
///
/// The component is checked, not just the status. A port can be answered by
/// another process — a service that lost a bind race, or anything else on the
/// host — and a readiness gate that accepts any 200 would report that as
/// success while the service under test is dead.
pub fn await_readiness(port: u16, component: &str) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    let mut last = String::from("no response");
    while Instant::now() < deadline {
        match try_request(port, "GET", "/readyz", &BTreeMap::new(), None) {
            Ok((200, body)) => {
                let answered = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|value| value["component"].as_str().map(str::to_owned));
                match answered.as_deref() {
                    Some(name) if name == component => return,
                    Some(other) => panic!(
                        "port {port} is served by {other}, not {component}. The service under \
                         test is not the one answering."
                    ),
                    None => last = format!("readiness body did not name a component: {body}"),
                }
            }
            Ok((status, body)) => last = format!("status {status}: {body}"),
            Err(error) => last = error,
        }
        sleep(Duration::from_millis(200));
    }
    panic!("{component} did not become ready within {READINESS_TIMEOUT:?} ({last})");
}

/// Allocate `count` distinct ephemeral ports.
///
/// The kernel can hand back a port that a previous call already released, so
/// asking for them together and rejecting duplicates is what keeps two services
/// from being pointed at the same address. This narrows the window rather than
/// closing it — another process can still take a port between here and the
/// service's bind — which is why `await_readiness` verifies who answered.
pub fn free_ports<const N: usize>() -> [u16; N] {
    for _ in 0..64 {
        let mut listeners = Vec::with_capacity(N);
        for _ in 0..N {
            listeners.push(TcpListener::bind("127.0.0.1:0").expect("ephemeral port"));
        }
        let ports: Vec<u16> = listeners
            .iter()
            .map(|listener| listener.local_addr().expect("bound address").port())
            .collect();
        // Holding every listener until all are bound is what makes them distinct.
        drop(listeners);
        let mut unique = ports.clone();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() == N {
            return ports.try_into().expect("exactly N ports");
        }
    }
    panic!("could not allocate {N} distinct ephemeral ports");
}

pub fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: Option<&Value>,
) -> (u16, String) {
    try_request(port, method, path, headers, body).expect("request completes")
}

/// A minimal blocking HTTP/1.1 client. The workspace deliberately has no async
/// runtime or HTTP client dependency, and a loopback JSON request needs neither.
pub fn try_request(
    port: u16,
    method: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: Option<&Value>,
) -> Result<(u16, String), String> {
    try_request_full(port, method, path, headers, body).map(|(status, _, body)| (status, body))
}

/// [`try_request`] that also hands back the response headers, lower-cased —
/// a redirect's `location` is the whole point of a sign-in callback.
pub fn try_request_full(
    port: u16,
    method: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: Option<&Value>,
) -> Result<(u16, BTreeMap<String, String>, String), String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| error.to_string())?;

    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    request.push_str("connection: close\r\n");
    if body.is_some() {
        request.push_str("content-type: application/json\r\n");
    }
    request.push_str(&format!("content-length: {}\r\n", payload.len()));
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|error| error.to_string())?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("malformed status line: {status_line:?}"))?
        .parse()
        .map_err(|_| format!("malformed status code: {status_line:?}"))?;

    let mut content_length = None;
    let mut chunked = false;
    let mut response_headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            response_headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }

    let mut response = String::new();
    if chunked {
        // Function responses stream, so the gateway sends them chunked. Reading
        // the socket raw would hand the caller the chunk framing instead of the
        // body.
        response = read_chunked(&mut reader)?;
    } else {
        match content_length {
            Some(length) => {
                let mut buffer = vec![0_u8; length];
                reader
                    .read_exact(&mut buffer)
                    .map_err(|error| error.to_string())?;
                response = String::from_utf8_lossy(&buffer).into_owned();
            }
            // `connection: close` means end-of-stream terminates the body.
            None => {
                reader
                    .read_to_string(&mut response)
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok((status, response_headers, response))
}

/// Decode a chunked body into its payload.
fn read_chunked(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut body = Vec::new();
    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .map_err(|error| error.to_string())?;
        // A chunk size may carry extensions after a semicolon.
        let size_text = header.trim_end();
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        if size_text.is_empty() {
            return Err(format!("malformed chunk header: {header:?}"));
        }
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("malformed chunk size: {size_text:?}"))?;
        if size == 0 {
            break;
        }
        let mut chunk = vec![0_u8; size];
        reader
            .read_exact(&mut chunk)
            .map_err(|error| error.to_string())?;
        body.extend_from_slice(&chunk);
        let mut terminator = [0_u8; 2];
        reader
            .read_exact(&mut terminator)
            .map_err(|error| error.to_string())?;
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// A loopback stand-in for the S3-compatible object store: enough of the
/// protocol for the platform's client (bucket HEAD/PUT, object PUT with
/// `if-none-match`, GET, DELETE), in memory, unauthenticated. The smoke
/// stack has no seaweed to talk to; this lets the real client code run.
pub struct ObjectStoreStub {
    pub endpoint: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl ObjectStoreStub {
    /// Listens on a free loopback port until dropped.
    pub fn start() -> Self {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("stub listener");
        let port = listener.local_addr().expect("stub address").port();
        listener
            .set_nonblocking(true)
            .expect("stub listener non-blocking");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut objects: BTreeMap<String, Vec<u8>> = BTreeMap::new();
            let mut buckets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            while !stopping.load(std::sync::atomic::Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(accepted) => accepted,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut raw = Vec::new();
                let mut buffer = [0u8; 8192];
                let (head_end, mut content_length) = loop {
                    let read = match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break (None, 0),
                        Ok(read) => read,
                    };
                    raw.extend_from_slice(&buffer[..read]);
                    if let Some(position) = raw.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let head = String::from_utf8_lossy(&raw[..position]).to_string();
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.trim()
                                    .eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        break (Some(position + 4), length);
                    }
                };
                let Some(head_end) = head_end else { continue };
                while raw.len() < head_end + content_length {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => raw.extend_from_slice(&buffer[..read]),
                    }
                }
                if raw.len() < head_end + content_length {
                    content_length = raw.len().saturating_sub(head_end);
                }
                let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                let mut request_line = head.lines().next().unwrap_or("").split_whitespace();
                let method = request_line.next().unwrap_or("").to_owned();
                let target = request_line.next().unwrap_or("/").to_owned();
                let path = target.split('?').next().unwrap_or("/").to_owned();
                let body = raw[head_end..head_end + content_length].to_vec();
                let has_if_none_match = head
                    .lines()
                    .any(|line| line.to_ascii_lowercase().starts_with("if-none-match:"));
                let mut segments = path.trim_start_matches('/').splitn(2, '/');
                let bucket = segments.next().unwrap_or("").to_owned();
                let key = segments.next().map(str::to_owned);
                let (status, payload): (u16, Vec<u8>) = match (method.as_str(), key) {
                    ("PUT", None) => {
                        buckets.insert(bucket);
                        (200, Vec::new())
                    }
                    ("HEAD", None) => (
                        if buckets.contains(&bucket) { 200 } else { 404 },
                        Vec::new(),
                    ),
                    ("PUT", Some(key)) => {
                        let full = format!("{bucket}/{key}");
                        if has_if_none_match && objects.contains_key(&full) {
                            (412, Vec::new())
                        } else {
                            objects.insert(full, body);
                            (200, Vec::new())
                        }
                    }
                    ("GET", Some(key)) => match objects.get(&format!("{bucket}/{key}")) {
                        Some(bytes) => (200, bytes.clone()),
                        None => (404, Vec::new()),
                    },
                    ("HEAD", Some(key)) => (
                        if objects.contains_key(&format!("{bucket}/{key}")) {
                            200
                        } else {
                            404
                        },
                        Vec::new(),
                    ),
                    ("DELETE", Some(key)) => {
                        objects.remove(&format!("{bucket}/{key}"));
                        (204, Vec::new())
                    }
                    _ => (400, Vec::new()),
                };
                let reason = match status {
                    200 => "OK",
                    204 => "No Content",
                    404 => "Not Found",
                    412 => "Precondition Failed",
                    _ => "Bad Request",
                };
                let mut response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                )
                .into_bytes();
                if method != "HEAD" {
                    response.extend_from_slice(&payload);
                }
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        Self {
            endpoint: format!("http://127.0.0.1:{port}"),
            stop,
        }
    }
}

impl Drop for ObjectStoreStub {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write, net::TcpListener, thread};

    use super::*;

    /// Serve one readiness response naming `component`, then stop.
    fn readiness_server(component: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("probe listener");
        let port = listener.local_addr().expect("probe address").port();
        thread::spawn(move || {
            for stream in listener.incoming().take(4) {
                let Ok(mut stream) = stream else { continue };
                let body = format!(
                    "{{\"status\":\"ready\",\"component\":\"{component}\",\"detail\":\"probe\"}}"
                );
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        port
    }

    #[test]
    fn readiness_accepts_only_the_service_under_test() {
        let port = readiness_server("mako-data-plane");
        // The matching component is what readiness is waiting for.
        await_readiness(port, "mako-data-plane");
    }

    #[test]
    fn readiness_rejects_another_service_answering_the_port() {
        // A port can be answered by a service that won a bind race, or by
        // anything else on the host. Accepting any 200 would report that as
        // success while the service under test is dead.
        let port = readiness_server("mako-data-plane");
        let outcome = std::panic::catch_unwind(|| await_readiness(port, "mako-control-plane"));
        let panic = outcome.expect_err("readiness must reject another service");
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| String::from("<non-string panic>"));
        assert!(
            message.contains("is served by mako-data-plane, not mako-control-plane"),
            "unexpected rejection message: {message}"
        );
    }

    #[test]
    fn allocated_ports_are_distinct() {
        let [first, second, third] = free_ports::<3>();
        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_ne!(first, third);
    }
}
