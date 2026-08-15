//! Drive Mako Cloud's application happy path against the real service binaries.
//!
//! Every other end-to-end suite in this repository runs against a mock: the
//! reference app ships an in-browser fake backend, and the console specs
//! intercept every `/v1/` request. This test starts the actual data plane and
//! control plane, seeds a tenant with the local bootstrap, and performs
//! application-user sign-up, sign-in, a document push, and a document pull over
//! HTTP. Nothing here is stubbed; if it passes, the happy path works.

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
const PROJECT_ID: &str = "prj_localboot";
const ENVIRONMENT_ID: &str = "env_localboot";
const COLLECTION_ID: &str = "todos";
const APP_EMAIL: &str = "smoke-user@local.test";
const APP_PASSWORD: &str = "SmokeUserPass1!";

/// Terminates a spawned service even when an assertion unwinds, so a failing
/// run never leaves a process holding a database lock.
struct ServiceProcess {
    name: &'static str,
    child: Child,
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill() {
            eprintln!("{} could not be terminated: {error}", self.name);
        }
        let _ = self.child.wait();
    }
}

#[test]
fn application_happy_path_succeeds_against_the_real_services() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-smoke-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let environment = service_environment(root);

    // Seed the tenant while nothing holds the database locks. The bootstrap
    // reports the public project key exactly once, at creation.
    let bootstrap = run_bootstrap(&binaries, &environment);
    let public_key = bootstrap["publicProjectKey"]
        .as_str()
        .expect("bootstrap reports a public project key")
        .to_owned();
    assert_eq!(bootstrap["projectId"], PROJECT_ID);
    assert_eq!(bootstrap["environmentId"], ENVIRONMENT_ID);
    assert_eq!(bootstrap["collectionId"], COLLECTION_ID);

    let data_port = free_port();
    let control_port = free_port();
    let _data = start_service(
        "mako-data-plane",
        &binaries,
        &environment,
        data_port,
        root.join("data-plane.log"),
    );
    let _control = start_service(
        "mako-control-plane",
        &binaries,
        &environment,
        control_port,
        root.join("control-plane.log"),
    );
    await_readiness(data_port, "data plane");
    await_readiness(control_port, "control plane");

    let base = format!("/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}");
    let keyed = BTreeMap::from([("x-mako-key".to_owned(), public_key.clone())]);

    // Sign up.
    let (status, _) = request(
        data_port,
        "POST",
        &format!("{base}/auth/signup"),
        &keyed,
        Some(&json!({ "email": APP_EMAIL, "password": APP_PASSWORD })),
    );
    assert!(
        (200..300).contains(&status),
        "sign-up failed with status {status}"
    );

    // Sign in and take the issued session.
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{base}/auth/signin"),
        &keyed,
        Some(&json!({ "email": APP_EMAIL, "password": APP_PASSWORD })),
    );
    assert_eq!(status, 200, "sign-in failed: {body}");
    let session: Value = serde_json::from_str(&body).expect("sign-in returns json");
    let access_token = session["accessToken"]
        .as_str()
        .expect("sign-in issues an access token")
        .to_owned();
    assert_eq!(session["user"]["email"], APP_EMAIL);

    let mut authenticated = keyed.clone();
    authenticated.insert("authorization".to_owned(), format!("Bearer {access_token}"));

    // Push a document.
    let mut pushing = authenticated.clone();
    pushing.insert(
        "idempotency-key".to_owned(),
        "smoke-happy-path-push-000001".to_owned(),
    );
    let document = json!({
        "id": "todo-smoke-1",
        "ownerId": "smoke-user",
        "title": "prove the happy path works",
        "updatedAt": 1_786_752_000_000_i64
    });
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{base}/collections/{COLLECTION_ID}/replication/push"),
        &pushing,
        Some(&json!({
            "schemaVersion": 1,
            "rows": [{
                "mutationId": "smoke-happy-path-mutation-1",
                "newDocumentState": document
            }]
        })),
    );
    assert_eq!(status, 200, "push failed: {body}");
    let push: Value = serde_json::from_str(&body).expect("push returns json");
    let outcome = &push["outcomes"][0];
    assert_eq!(
        outcome["status"], "accepted",
        "push was not accepted: {body}"
    );

    // Pull it back and prove it round-trips with its content intact.
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{base}/collections/{COLLECTION_ID}/replication/pull"),
        &authenticated,
        Some(&json!({ "schemaVersion": 1, "batchSize": 10 })),
    );
    assert_eq!(status, 200, "pull failed: {body}");
    let pull: Value = serde_json::from_str(&body).expect("pull returns json");
    let documents = pull["documents"]
        .as_array()
        .expect("pull returns a document array");
    let pulled = documents
        .iter()
        .find(|candidate| candidate["id"] == "todo-smoke-1")
        .unwrap_or_else(|| panic!("pushed document was not returned by pull: {body}"));
    assert_eq!(pulled["title"], "prove the happy path works");
    assert_eq!(pulled["ownerId"], "smoke-user");
    assert_eq!(pulled["updatedAt"], 1_786_752_000_000_i64);
    assert_eq!(pulled["_deleted"], false);

    // Negative control: without the issued session the same operations are
    // refused, which is what proves the run above depended on real
    // authentication rather than an open door.
    let (status, _) = request(
        data_port,
        "POST",
        &format!("{base}/collections/{COLLECTION_ID}/replication/pull"),
        &keyed,
        Some(&json!({ "schemaVersion": 1, "batchSize": 10 })),
    );
    assert!(
        !(200..300).contains(&status),
        "pull without a session must be refused, got {status}"
    );
}

/// Where the service binaries live. Defaults to the workspace target directory
/// so `cargo test` works, and is overridable for a release-profile run.
fn binary_directory() -> PathBuf {
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

fn scratch_root() -> PathBuf {
    std::env::var("MAKO_STORAGE_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

/// Configuration for a throwaway tenant confined to one temporary directory.
fn service_environment(root: &Path) -> BTreeMap<String, String> {
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

fn run_bootstrap(binaries: &Path, environment: &BTreeMap<String, String>) -> Value {
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

fn start_service(
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
fn await_readiness(port: u16, label: &str) {
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral port")
        .local_addr()
        .expect("bound address")
        .port()
}

fn request(
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
fn try_request(
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
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse::<usize>().ok();
        }
    }

    let mut response = String::new();
    match content_length {
        // `connection: close` means end-of-stream terminates the body, which
        // also covers a chunked or unannounced length.
        Some(length) => {
            let mut buffer = vec![0_u8; length];
            reader
                .read_exact(&mut buffer)
                .map_err(|error| error.to_string())?;
            response = String::from_utf8_lossy(&buffer).into_owned();
        }
        None => {
            reader
                .read_to_string(&mut response)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok((status, response))
}

/// The bootstrap is the only way to reach a working local tenant, so its
/// convergence on re-run and its refusal outside a local environment are load
/// bearing rather than incidental.
#[test]
fn bootstrap_converges_on_rerun_and_refuses_outside_a_local_environment() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-smoke-bootstrap-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let environment = service_environment(workspace.path());

    let first = run_bootstrap(&binaries, &environment);
    let second = run_bootstrap(&binaries, &environment);
    for field in [
        "developerId",
        "organizationId",
        "projectId",
        "environmentId",
        "collectionId",
    ] {
        assert_eq!(
            first[field], second[field],
            "{field} must be stable across bootstrap runs"
        );
    }
    // The key's secret is unrecoverable after creation, so a re-run must still
    // hand back a usable credential rather than nothing.
    let replayed = second["publicProjectKey"]
        .as_str()
        .expect("re-run still reports a public project key");
    assert!(replayed.starts_with("mako_pk."), "unexpected key form");

    let mut hosted = environment.clone();
    hosted.insert("MAKO_ENVIRONMENT".to_owned(), "development".to_owned());
    let refused = Command::new(binaries.join("mako-local-bootstrap"))
        .envs(&hosted)
        .output()
        .expect("bootstrap runs");
    assert!(
        !refused.status.success(),
        "bootstrap must refuse a non-local environment"
    );
    let reason = String::from_utf8_lossy(&refused.stderr);
    assert!(
        reason.contains("refusing to run"),
        "refusal must say why: {reason}"
    );
}
