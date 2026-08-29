//! Invoke a deployed edge function through the gateway, against the real stack.
//!
//! This is the only automated coverage of hosted function invocation. It starts
//! the pinned Supabase Edge Runtime, deploys a function through the genuine
//! administrative path, brings up the data plane, control plane, and edge
//! gateway, and asserts the function's own response comes back through the
//! gateway. Nothing is stubbed.
//!
//! It needs a container engine and is opt-in through `MAKO_RUN_EDGE_RUNTIME_TESTS=1`,
//! matching the other edge suites in this repository.

use std::{
    collections::BTreeMap,
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use mako_smoke::{
    await_readiness, binary_directory, mint_developer_session_with_secret, request, run_bootstrap,
    scratch_root, service_environment, start_service, try_request,
};

const PROJECT_ID: &str = "prj_localboot";
const ENVIRONMENT_ID: &str = "env_localboot";
const FUNCTION_NAME: &str = "hello";
const CONTAINER_NAME: &str = "mako-smoke-edge-runtime";

/// The edge gateway resolves its dependencies from compiled-in constants rather
/// than configuration, so this suite cannot choose its own ports and cannot run
/// beside a development stack.
const DATA_PLANE_PORT: u16 = 8080;
const CONTROL_PLANE_PORT: u16 = 8081;
const GATEWAY_PORT: u16 = 8082;
const SUPERVISOR_PORT: u16 = 9000;

// 64 hex characters: the session-minting tool and the internal-auth
// verifier both require exactly this shape.
const INTERNAL_AUTH_SECRET: &str =
    "656467652d736d6f6b652d696e7465726e616c2d617574682d30313233343536";
const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
/// The retained-log assertion spans the collector's fifteen-second cadence
/// plus batch delivery, so it waits far longer than it usually needs.
const LOG_DELIVERY_TIMEOUT: Duration = Duration::from_secs(75);
/// The supervisor requires exactly 32 bytes of hex.
const RUNTIME_STATE_KEY: &str = "6d616b6f2d656467652d736d6f6b652d73757065727669736f722d6b65793031";

/// Removes the runtime container even when an assertion unwinds.
struct RuntimeContainer {
    engine: Vec<String>,
}

impl RuntimeContainer {
    /// What the runtime printed. A container that exits during startup is the
    /// most likely failure here, and its output is the only explanation.
    fn logs(&self) -> String {
        engine_command(&self.engine)
            .args(["logs", CONTAINER_NAME])
            .output()
            .map_or_else(
                |error| format!("container logs unavailable: {error}"),
                |output| {
                    format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    )
                },
            )
    }
}

impl Drop for RuntimeContainer {
    fn drop(&mut self) {
        remove_container(&self.engine);
    }
}

fn remove_container(engine: &[String]) {
    let _ = engine_command(engine)
        .args(["rm", "-f", CONTAINER_NAME])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[test]
fn deployed_function_is_served_through_the_edge_gateway() {
    let Some(engine) = container_engine() else {
        eprintln!(
            "skipping hosted edge invocation: set MAKO_RUN_EDGE_RUNTIME_TESTS=1 and configure \
             Docker or Podman. Run it with: npm run test:edge-e2e"
        );
        return;
    };
    // An interrupted run can leave a container holding the supervisor port.
    // Clearing it first lets this suite recover by itself rather than reporting
    // a port conflict that the caller has to resolve by hand.
    remove_container(&engine);
    for (port, what) in [
        (DATA_PLANE_PORT, "data plane"),
        (CONTROL_PLANE_PORT, "control plane"),
        (GATEWAY_PORT, "edge gateway"),
        (SUPERVISOR_PORT, "runtime supervisor"),
    ] {
        assert!(
            port_is_free(port),
            "port {port} is already in use, so the {what} cannot start. The edge gateway resolves \
             its dependencies from compiled-in constants, so this suite requires these exact \
             ports; stop any local stack first."
        );
    }

    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-edge-smoke-")
        .tempdir_in(scratch_root())
        .expect("edge workspace");
    let root = workspace.path();

    // A supervisor the control plane can authenticate to, on the contract a
    // deployment uses. `mako functions serve` cannot stand in for this: it
    // generates a random supervisor credential.
    let container = start_runtime(&engine, root);
    await_supervisor(&container);

    let mut environment = service_environment(root);
    environment.insert(
        "MAKO_INTERNAL_AUTH_SECRET".to_owned(),
        INTERNAL_AUTH_SECRET.to_owned(),
    );
    // The retained log store: the control plane collects each function's
    // supervisor buffer into it, and the developer reads it back through the
    // management API -- which is the loop this test closes.
    let telemetry_port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("free telemetry port");
        let port = listener.local_addr().expect("telemetry address").port();
        drop(listener);
        port
    };
    let credential = root.join("telemetry-authorization");
    fs::write(&credential, INTERNAL_AUTH_SECRET).expect("telemetry credential");
    fs::create_dir_all(root.join("telemetry")).expect("telemetry directory");
    let mut telemetry_environment = environment.clone();
    for (name, value) in [
        (
            "MAKO_TELEMETRY_QUERY_BIND".to_owned(),
            format!("127.0.0.1:{telemetry_port}"),
        ),
        ("MAKO_TELEMETRY_REGION".to_owned(), "local".to_owned()),
        (
            "MAKO_TELEMETRY_RETENTION_SECONDS".to_owned(),
            (7 * 24 * 60 * 60).to_string(),
        ),
        (
            "MAKO_TELEMETRY_DATABASE_PATH".to_owned(),
            root.join("telemetry").to_string_lossy().into_owned(),
        ),
        (
            "MAKO_TELEMETRY_DATABASE_ID".to_owned(),
            "mako-telemetry-local".to_owned(),
        ),
        (
            "MAKO_TELEMETRY_AUTHORIZATION_FILE".to_owned(),
            credential.to_string_lossy().into_owned(),
        ),
        (
            "MAKO_DISK_WARNING_FREE_BYTES".to_owned(),
            "134217728".to_owned(),
        ),
        (
            "MAKO_DISK_CRITICAL_FREE_BYTES".to_owned(),
            "67108864".to_owned(),
        ),
    ] {
        telemetry_environment.insert(name, value);
    }
    let provisioned = Command::new(binaries.join("mako-storage-ops"))
        .arg("provision")
        .arg(format!(
            "--database-path={}",
            root.join("telemetry").to_string_lossy()
        ))
        .arg("--service=mako-telemetry-query")
        .arg("--database-id=mako-telemetry-local")
        .arg("--confirm=PROVISION")
        .output()
        .expect("storage ops runs");
    assert!(
        provisioned.status.success(),
        "telemetry volume was not provisioned: {}",
        String::from_utf8_lossy(&provisioned.stderr)
    );
    let _telemetry = start_service(
        "mako-telemetry-query",
        &binaries,
        &telemetry_environment,
        telemetry_port,
        root.join("telemetry.log"),
    );
    environment.insert(
        "MAKO_TELEMETRY_QUERY_ENDPOINT".to_owned(),
        format!("127.0.0.1:{telemetry_port}"),
    );
    // The control plane defaults to 9001 while the gateway compiles in 9000, so
    // the supervisor address has to be pinned to the port the gateway will use.
    environment.insert(
        "MAKO_RUNTIME_SUPERVISOR_ENDPOINT".to_owned(),
        format!("127.0.0.1:{SUPERVISOR_PORT}"),
    );
    environment.insert("MAKO_REGION".to_owned(), "local".to_owned());

    // Deploying registers the deployment with the supervisor, which is what
    // makes the function invocable at all.
    let bootstrap = run_bootstrap(&binaries, &environment);
    assert_eq!(
        bootstrap["functionName"], FUNCTION_NAME,
        "bootstrap did not report a deployed function: {bootstrap}"
    );

    let mut data_environment = environment.clone();
    data_environment.insert(
        "MAKO_BIND_ADDR".to_owned(),
        format!("127.0.0.1:{DATA_PLANE_PORT}"),
    );
    let _data = start_service(
        "mako-data-plane",
        &binaries,
        &data_environment,
        DATA_PLANE_PORT,
        root.join("data-plane.log"),
    );
    let _control = start_service(
        "mako-control-plane",
        &binaries,
        &environment,
        CONTROL_PLANE_PORT,
        root.join("control-plane.log"),
    );
    // The gateway keeps its own storage; it cannot share the data plane's.
    let mut gateway_environment = environment.clone();
    gateway_environment.insert(
        "MAKO_ROCKSDB_PATH".to_owned(),
        root.join("gateway-rocksdb").to_string_lossy().into_owned(),
    );
    gateway_environment.insert(
        "MAKO_ROCKSDB_BACKUP_DESTINATION".to_owned(),
        root.join("gateway-backups").to_string_lossy().into_owned(),
    );
    fs::create_dir_all(root.join("gateway-rocksdb")).expect("gateway storage");
    fs::create_dir_all(root.join("gateway-backups")).expect("gateway backups");
    let _gateway = start_service(
        "mako-edge-gateway",
        &binaries,
        &gateway_environment,
        GATEWAY_PORT,
        root.join("edge-gateway.log"),
    );

    await_readiness(DATA_PLANE_PORT, "mako-data-plane");
    await_readiness(CONTROL_PLANE_PORT, "mako-control-plane");
    await_readiness(GATEWAY_PORT, "mako-edge-gateway");

    // The project reference encodes the environment; a bare project id never
    // resolves to a tenant.
    let route = format!("/{PROJECT_ID}--{ENVIRONMENT_ID}/functions/v1/{FUNCTION_NAME}");
    let (status, body) = request(GATEWAY_PORT, "GET", &route, &BTreeMap::new(), None);
    assert_eq!(
        status, 200,
        "invoking the deployed function through the gateway failed: {body}"
    );
    let response: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("function returned non-json: {body}"));
    assert_eq!(response["ok"], true, "unexpected function response: {body}");
    assert_eq!(
        response["function"], FUNCTION_NAME,
        "another function answered: {body}"
    );

    // A function that was never deployed must not resolve.
    let missing = format!("/{PROJECT_ID}--{ENVIRONMENT_ID}/functions/v1/absent");
    let (status, _) = request(GATEWAY_PORT, "GET", &missing, &BTreeMap::new(), None);
    assert_eq!(status, 404, "an undeployed function must not be served");

    // A bare project reference carries no environment and must be refused.
    let bare = format!("/{PROJECT_ID}/functions/v1/{FUNCTION_NAME}");
    let (status, _) = request(GATEWAY_PORT, "GET", &bare, &BTreeMap::new(), None);
    assert_ne!(
        status, 200,
        "a project reference without an environment must not resolve"
    );

    // The line the function printed must reach the retained log store and be
    // served through the management API -- with the address and password it
    // deliberately carries masked, because the store scrubs what customer
    // code writes before keeping it.
    let session = mint_developer_session_with_secret(
        &binaries,
        root,
        CONTROL_PLANE_PORT,
        DEVELOPER_ID,
        DEVELOPER_EMAIL,
        INTERNAL_AUTH_SECRET,
    );
    let reading = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let logs_path = format!(
        "/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}/observability/logs?limit=50"
    );
    let deadline = Instant::now() + LOG_DELIVERY_TIMEOUT;
    let stored = loop {
        let (status, body) = request(CONTROL_PLANE_PORT, "GET", &logs_path, &reading, None);
        // The masked form of the line the function printed: lifecycle
        // entries (deployment_loaded, invocation_completed) arrive first and
        // do not count as the printed output this test is about.
        if status == 200 && body.contains("c***@example.com") {
            break body;
        }
        assert!(
            Instant::now() < deadline,
            "the function's printed line never reached the retained store (last: {status} {body})"
        );
        sleep(Duration::from_secs(2));
    };
    assert!(
        stored.contains("password=[REDACTED]"),
        "the stored log line was not scrubbed: {stored}"
    );
    assert!(
        !stored.contains("caller@example.com") && !stored.contains("hunter2"),
        "the raw address or password reached the retained store: {stored}"
    );

    // --- A schedule invokes the deployed function through the gateway. -----
    // The scheduler is a control-plane worker calling the gateway's internal
    // route, so the run is admitted, metered, and recorded like any other
    // invocation. Run-now proves the path without waiting for a cron minute.
    let schedules_path = format!(
        "/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}/functions/{FUNCTION_NAME}/schedules"
    );
    let manage = |key: &str| {
        let mut headers = reading.clone();
        headers.insert(
            "idempotency-key".to_owned(),
            format!("edge-schedule-smoke-{key}"),
        );
        headers
    };
    let (status, body) = request(
        CONTROL_PLANE_PORT,
        "POST",
        &schedules_path,
        &manage("invalid"),
        Some(&serde_json::json!({ "cron": "every minute" })),
    );
    assert_eq!(
        status, 400,
        "an invalid expression is refused at save: {body}"
    );
    let (status, body) = request(
        CONTROL_PLANE_PORT,
        "POST",
        &schedules_path,
        &manage("create"),
        Some(&serde_json::json!({
            "name": "heartbeat",
            "cron": "*/5 * * * *",
            "request": { "method": "GET", "path": "/?source=schedule" },
        })),
    );
    assert_eq!(status, 201, "creating the schedule failed: {body}");
    let schedule: serde_json::Value = serde_json::from_str(&body).expect("schedule json");
    let schedule_id = schedule["id"].as_str().expect("schedule id").to_owned();
    assert_eq!(schedule["state"], "active");
    assert_eq!(schedule["timezone"], "UTC");
    assert!(
        schedule["nextRunAt"].as_str().is_some(),
        "an active schedule shows its next run: {body}"
    );
    let (status, body) = request(
        CONTROL_PLANE_PORT,
        "POST",
        &format!("{schedules_path}/{schedule_id}/actions/run-now"),
        &manage("run-now"),
        None,
    );
    assert_eq!(status, 202, "run-now failed: {body}");
    let queued: serde_json::Value = serde_json::from_str(&body).expect("run json");
    assert_eq!(queued["manual"], true);
    let run_id = queued["id"].as_str().expect("run id").to_owned();
    let runs_path = format!("{schedules_path}/{schedule_id}/runs?limit=20");
    let deadline = Instant::now() + Duration::from_secs(90);
    let finished = loop {
        let (status, body) = request(CONTROL_PLANE_PORT, "GET", &runs_path, &reading, None);
        assert_eq!(status, 200, "reading the run history failed: {body}");
        let page: serde_json::Value = serde_json::from_str(&body).expect("runs json");
        let run = page["items"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == run_id))
            .cloned();
        if let Some(run) = run
            && run["outcome"].as_str().is_some()
        {
            break run;
        }
        assert!(
            Instant::now() < deadline,
            "the manual run never completed (last: {body})"
        );
        sleep(Duration::from_secs(2));
    };
    assert_eq!(
        finished["outcome"], "succeeded",
        "the scheduled invocation reached the function: {finished}"
    );
    assert_eq!(finished["responseStatus"], 200);
    assert!(finished["durationMilliseconds"].as_u64().is_some());
    assert!(finished["functionVersion"].as_u64().is_some());
    let (status, body) = request(
        CONTROL_PLANE_PORT,
        "GET",
        &format!("{schedules_path}/{schedule_id}"),
        &reading,
        None,
    );
    assert_eq!(status, 200, "reading the schedule failed: {body}");
    let read: serde_json::Value = serde_json::from_str(&body).expect("schedule json");
    assert_eq!(
        read["lastRun"]["id"], run_id,
        "the schedule shows its last run: {body}"
    );
    // Pausing keeps the schedule but drops its next run.
    let (status, body) = request(
        CONTROL_PLANE_PORT,
        "PATCH",
        &format!("{schedules_path}/{schedule_id}"),
        &manage("pause"),
        Some(&serde_json::json!({ "enabled": false })),
    );
    assert_eq!(status, 200, "pausing failed: {body}");
    let paused: serde_json::Value = serde_json::from_str(&body).expect("schedule json");
    assert_eq!(paused["state"], "paused");
    assert!(
        paused["nextRunAt"].is_null(),
        "a paused schedule has no next run: {body}"
    );
    let (status, body) = request(
        CONTROL_PLANE_PORT,
        "DELETE",
        &format!("{schedules_path}/{schedule_id}"),
        &manage("delete"),
        None,
    );
    assert_eq!(status, 204, "deleting the schedule failed: {body}");
    let (status, body) = request(CONTROL_PLANE_PORT, "GET", &schedules_path, &reading, None);
    assert_eq!(status, 200);
    let listed: serde_json::Value = serde_json::from_str(&body).expect("list json");
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(0));
}

/// The engine to drive, or `None` when this suite is not enabled. Mirrors the
/// other edge suites: `MAKO_EDGE_TEST_ENGINE` selects podman or docker, and
/// `MAKO_EDGE_TEST_ENGINE_PREFIX_JSON` supplies arguments that must precede the
/// subcommand, such as an isolated graph root.
fn container_engine() -> Option<Vec<String>> {
    if std::env::var("MAKO_RUN_EDGE_RUNTIME_TESTS").ok()? != "1" {
        return None;
    }
    let binary = match std::env::var("MAKO_EDGE_TEST_ENGINE").as_deref() {
        Ok("podman") => "podman",
        _ => "docker",
    };
    let prefix: Vec<String> = std::env::var("MAKO_EDGE_TEST_ENGINE_PREFIX_JSON")
        .ok()
        .map_or_else(Vec::new, |source| {
            serde_json::from_str(&source)
                .expect("MAKO_EDGE_TEST_ENGINE_PREFIX_JSON must be a JSON string array")
        });
    let mut engine = vec![binary.to_owned()];
    engine.extend(prefix);
    Some(engine)
}

fn engine_command(engine: &[String]) -> Command {
    let mut command = Command::new(&engine[0]);
    command.args(&engine[1..]);
    command
}

/// Start the pinned runtime on the same contract a deployment uses. The parts
/// that matter are the authorization, which must equal the services' internal
/// auth secret, and the region, which must equal theirs.
fn start_runtime(engine: &[String], root: &Path) -> RuntimeContainer {
    let pin = runtime_pin();
    let canary = root.join("canary");
    let state = root.join("supervisor-state");
    fs::create_dir_all(&canary).expect("canary directory");
    fs::create_dir_all(&state).expect("supervisor state directory");
    fs::write(
        canary.join("index.ts"),
        "export default { fetch: () => new Response(\"ok\", { status: 200 }) };\n",
    )
    .expect("canary function");

    let main_worker = workspace_root().join("packages/cli/runtime/main");
    assert!(
        main_worker.join("index.ts").exists(),
        "{} is missing; the runtime main worker is required",
        main_worker.display()
    );

    let status = engine_command(engine)
        .args([
            "run",
            "--detach",
            "--name",
            CONTAINER_NAME,
            "--init",
            "--read-only",
            "--publish",
            &format!("127.0.0.1:{SUPERVISOR_PORT}:9000"),
            "--mount",
            &format!(
                "type=bind,src={},dst=/home/deno/functions/main,readonly",
                main_worker.display()
            ),
            "--mount",
            &format!(
                "type=bind,src={},dst=/home/deno/functions/canary,readonly",
                canary.display()
            ),
            "--mount",
            &format!(
                "type=bind,src={},dst=/var/lib/mako-runtime-supervisor",
                state.display()
            ),
            "--tmpfs",
            "/tmp:rw,noexec,nosuid,size=128m",
            "--tmpfs",
            "/var/lib/mako-runtime-workers:rw,noexec,nosuid,size=128m",
        ])
        .args(runtime_environment())
        .arg(&pin)
        .args([
            "start",
            "--policy",
            "per_request",
            "--user-worker-request-idle-timeout",
            "5000",
            "--main-service",
            "/home/deno/functions/main",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .expect("container engine runs");
    assert!(
        status.success(),
        "the pinned runtime container could not be started. Pull it first: {pin}"
    );
    RuntimeContainer {
        engine: engine.to_vec(),
    }
}

fn runtime_environment() -> Vec<String> {
    [
        ("DENO_DIR", "/tmp/deno-cache".to_owned()),
        ("EDGE_RUNTIME_PORT", "9000".to_owned()),
        (
            "MAKO_API_URL",
            format!("http://host.containers.internal:{DATA_PLANE_PORT}"),
        ),
        ("MAKO_ENTRYPOINT", "index.ts".to_owned()),
        ("MAKO_ENVIRONMENT_ID", ENVIRONMENT_ID.to_owned()),
        ("MAKO_FUNCTION_NAME", "health".to_owned()),
        (
            "MAKO_FUNCTION_PATH",
            "/home/deno/functions/canary".to_owned(),
        ),
        ("MAKO_JWKS", String::new()),
        ("MAKO_JWT_AUDIENCE", String::new()),
        ("MAKO_JWT_ISSUER", String::new()),
        ("MAKO_PROJECT_ID", PROJECT_ID.to_owned()),
        (
            "MAKO_RUNTIME_AUTHORIZATION",
            INTERNAL_AUTH_SECRET.to_owned(),
        ),
        ("MAKO_RUNTIME_REGION", "local".to_owned()),
        (
            "MAKO_RUNTIME_STATE_PATH",
            "/var/lib/mako-runtime-supervisor".to_owned(),
        ),
        ("MAKO_RUNTIME_STATE_KEY", RUNTIME_STATE_KEY.to_owned()),
        (
            "MAKO_RUNTIME_WORKER_PATH",
            "/var/lib/mako-runtime-workers".to_owned(),
        ),
        ("MAKO_USER_ENV_NAMES", "[]".to_owned()),
        ("MAKO_VERIFY_JWT", "false".to_owned()),
        ("MAKO_WALL_TIME_MS", "5000".to_owned()),
    ]
    .into_iter()
    .flat_map(|(name, value)| ["--env".to_owned(), format!("{name}={value}")])
    .collect()
}

/// The supervisor reports ready only once it has loaded its state and agrees on
/// the protocol, release, and region, so this is proof it is usable rather than
/// merely listening.
fn await_supervisor(container: &RuntimeContainer) {
    let headers = BTreeMap::from([
        (
            "x-mako-runtime-authorization".to_owned(),
            INTERNAL_AUTH_SECRET.to_owned(),
        ),
        ("x-mako-runtime-protocol".to_owned(), "1".to_owned()),
        (
            "x-mako-request-id".to_owned(),
            "req_edgesmoke00000000000000000000".to_owned(),
        ),
    ]);
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last = String::from("no response");
    while Instant::now() < deadline {
        match try_request(
            SUPERVISOR_PORT,
            "GET",
            "/_mako/runtime/v1/health",
            &headers,
            None,
        ) {
            Ok((200, body)) if body.contains("\"ready\":true") => return,
            // A rejected credential or protocol will not start working by
            // waiting, and both mean this suite is misconfigured rather than
            // slow.
            Ok((status @ (401 | 403 | 426), body)) => panic!(
                "the runtime supervisor rejected the management credential (status {status}: \
                 {body}). MAKO_RUNTIME_AUTHORIZATION in the container must equal the internal \
                 auth secret the services use."
            ),
            Ok((status, body)) => last = format!("status {status}: {body}"),
            Err(error) => last = error,
        }
        sleep(Duration::from_millis(500));
    }
    panic!(
        "the runtime supervisor did not become ready ({last})\n--- container output ---\n{}",
        container.logs()
    );
}

fn runtime_pin() -> String {
    let path = workspace_root().join("infra/edge-runtime/runtime-pin.json");
    let pin: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("runtime pin"))
            .expect("runtime pin");
    format!(
        "{}@{}",
        pin["imageRepository"].as_str().expect("image repository"),
        pin["imageDigest"].as_str().expect("image digest")
    )
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn port_is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}
