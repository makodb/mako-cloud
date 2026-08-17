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
    await_readiness, binary_directory, request, run_bootstrap, scratch_root, service_environment,
    start_service, try_request,
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

const INTERNAL_AUTH_SECRET: &str = "edge-smoke-internal-auth-secret-0123456789";
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

    await_readiness(DATA_PLANE_PORT, "data plane");
    await_readiness(CONTROL_PLANE_PORT, "control plane");
    await_readiness(GATEWAY_PORT, "edge gateway");

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
