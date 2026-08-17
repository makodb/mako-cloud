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
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use serde_json::Value;

pub const READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// Terminates a spawned service even when an assertion unwinds, so a failing
/// run never leaves a process holding a database lock.
pub struct ServiceProcess {
    pub name: &'static str,
    pub child: Child,
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill() {
            eprintln!("{} could not be terminated: {error}", self.name);
        }
        let _ = self.child.wait();
    }
}

/// Where the service binaries live. Defaults to the workspace target directory
/// so `cargo test` works, and is overridable for a release-profile run.
pub fn binary_directory() -> PathBuf {
    if let Ok(configured) = std::env::var("MAKO_SMOKE_BINARY_DIR") {
        return PathBuf::from(configured);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();
    workspace.join("target").join("debug")
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
        (
            "MAKO_INTERNAL_AUTH_SECRET".to_owned(),
            "smoke-internal-auth-secret-0123456789abcdef".to_owned(),
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
    let output = std::fs::File::create(&log).expect("service log");
    let errors = output.try_clone().expect("service log");
    let child = Command::new(&executable)
        .envs(environment)
        .env("MAKO_BIND_ADDR", format!("127.0.0.1:{port}"))
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(errors))
        .spawn()
        .unwrap_or_else(|error| panic!("{name} could not be started: {error}"));
    ServiceProcess { name, child }
}

/// Each service refuses to serve until its readiness passes, so a ready service
/// is proof its storage and identity dependencies opened.
pub fn await_readiness(port: u16, label: &str) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    let mut last = String::from("no response");
    while Instant::now() < deadline {
        match try_request(port, "GET", "/readyz", &BTreeMap::new(), None) {
            Ok((200, _)) => return,
            Ok((status, body)) => last = format!("status {status}: {body}"),
            Err(error) => last = error,
        }
        sleep(Duration::from_millis(200));
    }
    panic!("{label} did not become ready within {READINESS_TIMEOUT:?} ({last})");
}

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral port")
        .local_addr()
        .expect("bound address")
        .port()
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
    Ok((status, response))
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
