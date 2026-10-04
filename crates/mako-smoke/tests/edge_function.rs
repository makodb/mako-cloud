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
//!
//! ## What the suite deploys, and what it needs
//!
//! `mako-local-bootstrap` deploys both sample functions **in process**: it
//! constructs `FunctionAdminService` itself with an in-memory object store, so
//! this suite needs no S3 service at all. That is not how a developer deploys.
//! `mako-cloud functions deploy` and the management API upload the bundle through
//! the running control plane, which stores artifacts in the S3 object store
//! `MAKO_OBJECT_STORE_*` names -- without a reachable one, bundle upload and
//! `functions deployments create` answer `503 function administration is
//! unavailable`. See `docs/user-book.md#deploying-a-function-locally-needs-an-object-store`.
//!
//! The second sample calls back into the data plane from inside the container,
//! so the runtime needs a route to the host's loopback: see
//! [`runtime_network`].

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
/// The bootstrap's second sample: it imports `@mako-cloud/edge-sdk`, runs with
/// a scoped service credential handed to it as a supplied function secret, and
/// reads and writes a document whose id contains `:`.
const SERVICE_FUNCTION_NAME: &str = "budget";
/// The bootstrap's third sample: it attempts every escape out of the worker
/// sandbox and reports the outcome of each. Deployed with no secret and no
/// credential, so it is always present.
const SANDBOX_FUNCTION_NAME: &str = "sandbox";
const SERVICE_FUNCTION_DOCUMENT_ID: &str = "hh_local:groceries:2026-08";
const COLLECTION_ID: &str = "todos";
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
    // deployment uses. `mako-cloud functions serve` cannot stand in for this: it
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
    assert_eq!(
        bootstrap["serviceFunctionName"], SERVICE_FUNCTION_NAME,
        "bootstrap did not report the service-credential function: {bootstrap}"
    );
    assert_eq!(
        bootstrap["sandboxFunctionName"], SANDBOX_FUNCTION_NAME,
        "bootstrap did not report the sandbox probe function: {bootstrap}"
    );
    // The credential the bootstrap issued and handed to the function as a
    // supplied secret value. This test presents the same one directly, which
    // is how it checks the document without trusting the function's answer.
    let service_key = bootstrap["serviceCredential"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("bootstrap did not report the issued service credential: {bootstrap}")
        })
        .to_owned();

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

    // A function owns the path under its name: the gateway forwards it and the
    // runtime hands it over, so a function with more than one route works.
    // Requiring the bare name made every REST-shaped function unreachable.
    let owned = format!("/{PROJECT_ID}--{ENVIRONMENT_ID}/functions/v1/{FUNCTION_NAME}/orders/42");
    let (status, body) = request(GATEWAY_PORT, "GET", &owned, &BTreeMap::new(), None);
    assert_eq!(status, 200, "a function's own path was not routed: {body}");
    let answered: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("function returned non-json: {body}"));
    assert_eq!(
        answered["path"], "/orders/42",
        "the function is handed the path under its name: {body}"
    );

    // A function that was never deployed must not resolve.
    let missing = format!("/{PROJECT_ID}--{ENVIRONMENT_ID}/functions/v1/absent");
    let (status, _) = request(GATEWAY_PORT, "GET", &missing, &BTreeMap::new(), None);
    assert_eq!(status, 404, "an undeployed function must not be served");

    // A bare project reference carries no environment and must be refused.
    let bare = format!("/{PROJECT_ID}/functions/v1/{FUNCTION_NAME}");
    let (status, _) = request(GATEWAY_PORT, "GET", &bare, &BTreeMap::new(), None);
    assert_eq!(
        status, 404,
        "a project reference without an environment must not resolve"
    );

    // --- A function that imports the SDK, holds a supplied service secret,
    // --- and writes a document whose id has to be escaped in the path. ------
    //
    // One invocation covers all three: the bundle validator accepted the bare
    // `@mako-cloud/edge-sdk` specifier and the runtime resolved it, the
    // function read a scoped service credential the platform never generated,
    // and it created `hh_local:groceries:2026-08` -- an id `encodeURIComponent`
    // escapes -- through the `/service/` document routes.
    let service_route =
        format!("/{PROJECT_ID}--{ENVIRONMENT_ID}/functions/v1/{SERVICE_FUNCTION_NAME}");
    let (status, body) = request(GATEWAY_PORT, "GET", &service_route, &BTreeMap::new(), None);
    assert_eq!(
        status,
        200,
        "the function importing @mako-cloud/edge-sdk did not answer: {body}\n--- container \
         output ---\n{}",
        container.logs()
    );
    let written: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("the service function returned non-json: {body}"));
    assert_eq!(written["ok"], true, "the service function failed: {body}");
    assert_eq!(written["documentId"], SERVICE_FUNCTION_DOCUMENT_ID);
    assert_eq!(
        written["created"], true,
        "the first invocation creates the document: {body}"
    );

    // A second invocation is a fresh worker with a fresh request id: reading
    // the document back proves the first write reached the data plane rather
    // than any in-process state.
    let (status, body) = request(GATEWAY_PORT, "GET", &service_route, &BTreeMap::new(), None);
    assert_eq!(status, 200, "the second invocation failed: {body}");
    let reread: serde_json::Value = serde_json::from_str(&body).expect("service function json");
    assert_eq!(
        reread["created"], false,
        "the second invocation must find the document the first wrote: {body}"
    );
    assert_eq!(
        reread["readTitle"], "groceries",
        "the function read back what it wrote: {body}"
    );

    // --- The caller a function is told about. ------------------------------
    //
    // `createFunctionClientFromRequest` is the documented -- and only -- way a
    // function learns who called it, so every application that shares data
    // between users rests on it. The gateway verifies the bearer token and the
    // runtime hands the worker the credential on the reserved header; nothing
    // downstream reads the request's own `authorization`, which for a function
    // that does not require a token would be whatever the client typed.
    //
    // The gateway carried the verified token and no adapter ever sent it, so
    // in the hosted shape every function saw an anonymous caller. Locally
    // `mako-cloud functions serve` sets the header itself, which is why only a real gateway
    // and a real runtime, as here, can tell the two apart.
    let public_key = bootstrap["publicProjectKey"]
        .as_str()
        .unwrap_or_else(|| panic!("bootstrap did not report the public project key: {bootstrap}"));
    let application_email = "caller@application.test";
    let application_password = "Correct-Horse-Battery-42!";
    let auth_headers = BTreeMap::from([("x-mako-key".to_owned(), public_key.to_owned())]);
    let credentials = serde_json::json!({
        "email": application_email,
        "password": application_password,
    });
    let signup_path =
        format!("/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}/auth/signup");
    let (status, body) = request(
        DATA_PLANE_PORT,
        "POST",
        &signup_path,
        &auth_headers,
        Some(&credentials),
    );
    assert!(
        (200..300).contains(&status),
        "signing up an application user failed: {status} {body}"
    );
    let signin_path =
        format!("/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}/auth/signin");
    let (status, body) = request(
        DATA_PLANE_PORT,
        "POST",
        &signin_path,
        &auth_headers,
        Some(&credentials),
    );
    assert_eq!(
        status, 200,
        "signing in the application user failed: {body}"
    );
    let session: serde_json::Value = serde_json::from_str(&body).expect("session json");
    let application_token = session["accessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("no access token in the session: {body}"))
        .to_owned();
    let application_user_id = session["user"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no user id in the session: {body}"))
        .to_owned();

    let caller_route = format!("{service_route}/caller");
    let (status, body) = request(
        GATEWAY_PORT,
        "GET",
        &caller_route,
        &BTreeMap::from([(
            "authorization".to_owned(),
            format!("Bearer {application_token}"),
        )]),
        None,
    );
    assert_eq!(
        status,
        200,
        "the function could not identify its caller: {body}\n--- container output ---\n{}",
        container.logs()
    );
    let identified: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("the caller route returned non-json: {body}"));
    assert_eq!(
        identified["userId"], application_user_id,
        "the function is told which application user called it: {body}"
    );
    assert_eq!(identified["email"], application_email);

    // No token, no identity: a function that admits anonymous callers is told
    // there is nobody rather than handed one it cannot vouch for.
    let (status, body) = request(GATEWAY_PORT, "GET", &caller_route, &BTreeMap::new(), None);
    assert_eq!(
        status, 401,
        "an anonymous call must reach the function with no caller: {body}"
    );

    // A bearer token nobody issued never reaches the function at all.
    let (status, _) = request(
        GATEWAY_PORT,
        "GET",
        &caller_route,
        &BTreeMap::from([(
            "authorization".to_owned(),
            "Bearer forged.not-a-token.at-all".to_owned(),
        )]),
        None,
    );
    assert_eq!(
        status, 401,
        "an unverifiable token is refused by the gateway"
    );

    // And the document is really there, read directly rather than through the
    // function: the id is percent-encoded in the path exactly as the SDK sends
    // it, which is what the data-plane route has to decode before comparing it
    // with the body's primary key.
    let encoded_id = SERVICE_FUNCTION_DOCUMENT_ID.replace(':', "%3A");
    let document_path = format!(
        "/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}/service/collections/\
         {COLLECTION_ID}/documents/{encoded_id}"
    );
    let service_headers = |request_id: &str| {
        BTreeMap::from([
            ("x-mako-service-key".to_owned(), service_key.clone()),
            (
                "x-mako-bypass-reason".to_owned(),
                "edge smoke verifies the function's write".to_owned(),
            ),
            ("x-mako-request-id".to_owned(), request_id.to_owned()),
        ])
    };
    let (status, body) = request(
        DATA_PLANE_PORT,
        "GET",
        &document_path,
        &service_headers("req_edgesmokedocument000000000001"),
        None,
    );
    assert_eq!(
        status, 200,
        "an escaped document id must resolve on the service route: {body}"
    );
    let document: serde_json::Value = serde_json::from_str(&body).expect("document json");
    assert_eq!(
        document["primaryKey"], SERVICE_FUNCTION_DOCUMENT_ID,
        "the stored id is the decoded one: {body}"
    );
    assert_eq!(document["body"]["title"], "groceries");

    // A function's calls all carry its invocation's request id, so reading a
    // document and then writing it under that one id are two different
    // requests, not a reused id. Each reserves its own quota and both succeed;
    // refusing the write is what kept a function from recovering a command's
    // result. A retry of the very same write is still the same reservation, so
    // it replays instead of writing a second revision. None of this may surface
    // as `unavailable`, which once sent clients into a retry loop that could
    // never succeed.
    let reused = "req_edgesmokereuse00000000000001";
    let (status, body) = request(
        DATA_PLANE_PORT,
        "GET",
        &document_path,
        &service_headers(reused),
        None,
    );
    assert_eq!(
        status, 200,
        "the first use of a request id succeeds: {body}"
    );
    // `try_request` sets `content-type: application/json` for a body itself;
    // sending it again would make the header ambiguous and be refused first.
    let mut writing = service_headers(reused);
    writing.insert(
        "idempotency-key".to_owned(),
        "edge-smoke-request-id-reuse-mutation".to_owned(),
    );
    let (status, body) = request(
        DATA_PLANE_PORT,
        "POST",
        &document_path,
        &writing,
        Some(&serde_json::json!({
            "mutationId": "edge-smoke-request-id-reuse-mutation",
            "schemaVersion": 1,
            "operation": "update",
            "expectedRevision": document["revision"],
            "body": {
                "id": SERVICE_FUNCTION_DOCUMENT_ID,
                "ownerId": "usr_budget_function",
                "title": "reused",
                "updatedAt": 1_786_752_000_000_i64,
            },
        })),
    );
    assert_eq!(
        status, 200,
        "a write after a read under one request id is a different request: {body}"
    );
    let written: serde_json::Value = serde_json::from_str(&body).expect("write json");
    assert_eq!(written["status"], "applied", "{body}");
    assert_eq!(written["document"]["body"]["title"], "reused", "{body}");
    let (status, body) = request(
        DATA_PLANE_PORT,
        "POST",
        &document_path,
        &writing,
        Some(&serde_json::json!({
            "mutationId": "edge-smoke-request-id-reuse-mutation",
            "schemaVersion": 1,
            "operation": "update",
            "expectedRevision": document["revision"],
            "body": {
                "id": SERVICE_FUNCTION_DOCUMENT_ID,
                "ownerId": "usr_budget_function",
                "title": "reused",
                "updatedAt": 1_786_752_000_000_i64,
            },
        })),
    );
    assert_eq!(
        status, 200,
        "a retry of the same write replays, not an outage: {body}"
    );
    let replayed: serde_json::Value = serde_json::from_str(&body).expect("replay json");
    assert_eq!(replayed["status"], "replayed", "{body}");
    assert_eq!(
        replayed["document"]["revision"], written["document"]["revision"],
        "the retry writes no second revision: {body}"
    );

    // --- The worker sandbox: what tenant code may and may not do. ---------
    //
    // The runtime reads an *empty* permission list as "granted without
    // restriction" and a missing one as no grant at all, so a worker started
    // with `allow_net: []` could reach every host, and one with
    // `allow_write: []` could write anywhere the container can. The supervisor
    // therefore spells every denied capability `null`.
    //
    // This deploys a function that attempts each escape in turn and reports
    // the outcome of every one rather than throwing on the first. The
    // "foreign" origin it tries is the platform API's own host on the
    // neighbouring port -- the control plane, genuinely listening on this
    // machine -- so a refusal is a denial and not an unreachable address, and
    // it shows the grant is bounded to one `host:port` rather than to a host.
    let sandbox_route =
        format!("/{PROJECT_ID}--{ENVIRONMENT_ID}/functions/v1/{SANDBOX_FUNCTION_NAME}");
    let (status, body) = request(GATEWAY_PORT, "GET", &sandbox_route, &BTreeMap::new(), None);
    assert_eq!(
        status,
        200,
        "the sandbox probe function did not answer: {body}\n--- container output ---\n{}",
        container.logs()
    );
    let sandbox: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("the sandbox function returned non-json: {body}"));

    // `deny_all` denies the function's own destinations, not the platform API
    // the runtime injects: a function that could not reach it could not use
    // the SDK, which is what the `budget` assertions above exercise.
    assert_eq!(
        sandbox["platformFetch"]["outcome"], "succeeded",
        "a function must still reach the platform API origin under deny_all: {body}"
    );
    assert_eq!(
        sandbox["platformFetch"]["status"], 200,
        "the platform API origin answered the function: {body}"
    );

    // `NotCapable` is Deno's refusal for a capability the worker was never
    // granted. Asserting the error class, not just the failure, is what keeps
    // this from passing on a connection error or a missing file.
    for attempt in [
        "fetchForeignHost",
        "openSocketToForeignHost",
        "writeOutsideWorkerDirectory",
        "readSupervisorState",
        "readUngrantedEnvironmentVariable",
    ] {
        let outcome = &sandbox["attempts"][attempt];
        assert_eq!(
            outcome["outcome"], "refused",
            "{attempt} was not refused: {body}"
        );
        assert_eq!(
            outcome["error"], "NotCapable",
            "{attempt} was refused by something other than the permission sandbox: {body}"
        );
    }
    // The sandbox deployment declares `egress-probe.invalid` as an allowed
    // egress host -- a name RFC 2606 guarantees will never resolve. The proof
    // that the grant exists is the *class* of the failure: an undeclared host
    // dies as `NotCapable` before any resolver runs (asserted above), while
    // the declared one passes the permission layer and fails on the network.
    // No external connectivity is needed to tell the two apart.
    let declared = &sandbox["attempts"]["fetchDeclaredHost"];
    assert_eq!(
        declared["outcome"], "refused",
        "a reserved .invalid name must not actually answer: {body}"
    );
    assert_ne!(
        declared["error"], "NotCapable",
        "the declared egress host was refused by the permission sandbox, \
         so the deployment's allowlist never became a grant: {body}"
    );

    // These three are refused by the runtime image before a permission is
    // consulted -- it blocks subprocesses outright, exposes no `Deno.dlopen`,
    // and never resolves a module specifier computed at runtime -- so only the
    // refusal is asserted. The grants are withheld as well, which is what
    // keeps these refused if the image's surface ever widens.
    for attempt in ["spawnProcess", "loadNativeLibrary", "importRemoteModule"] {
        assert_eq!(
            sandbox["attempts"][attempt]["outcome"], "refused",
            "{attempt} was not refused: {body}"
        );
    }

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

    // --- A function secret carrying a value the developer supplies. --------
    //
    // The bootstrap wrote the deployed function's credential through the
    // domain service; this is the same thing over the public route, which is
    // what a developer or the console uses. The response must carry metadata
    // only: a supplied value is never echoed back, and creating a name twice
    // is a conflict rather than a silent overwrite.
    let supplied_path = format!(
        "/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}/function-secrets/SUPPLIED_KEY"
    );
    let supplied_value = "mako_sk.key_supplied.edge_smoke_do_not_log_00000000";
    let mut writing_secret = reading.clone();
    writing_secret.insert(
        "idempotency-key".to_owned(),
        "edge-smoke-supplied-secret".to_owned(),
    );
    let (status, body) = request(
        CONTROL_PLANE_PORT,
        "PUT",
        &supplied_path,
        &writing_secret,
        Some(&serde_json::json!({ "value": supplied_value })),
    );
    assert_eq!(status, 201, "a supplied secret value is accepted: {body}");
    assert!(
        !body.contains(supplied_value),
        "a supplied secret value is never returned: {body}"
    );
    let supplied: serde_json::Value = serde_json::from_str(&body).expect("secret json");
    assert_eq!(supplied["name"], "SUPPLIED_KEY");
    assert_eq!(supplied["version"], 1);
    assert_eq!(supplied["state"], "active");
    let (status, body) = request(
        CONTROL_PLANE_PORT,
        "PUT",
        &supplied_path,
        &writing_secret,
        Some(&serde_json::json!({ "value": "mako_sk.key_supplied.a_different_value_0000000000" })),
    );
    assert_eq!(status, 409, "a secret value is written once: {body}");
    let (status, body) = request(CONTROL_PLANE_PORT, "GET", &supplied_path, &reading, None);
    assert_eq!(status, 200, "reading the secret's metadata failed: {body}");
    assert!(
        !body.contains("edge_smoke_do_not_log"),
        "reading a secret must never disclose its value: {body}"
    );

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

/// The `--network` value the runtime container runs with, if any.
///
/// Every service in this suite binds loopback, and a function reaches the data
/// plane at `host.containers.internal`. Rootless Podman's default pasta
/// networking forwards that address to the host's *external* addresses only,
/// so a loopback-bound data plane answers `connection refused`;
/// `--map-host-loopback` is what makes the host's loopback reachable without
/// exposing any service beyond it. `MAKO_EDGE_TEST_NETWORK` replaces the value
/// for another engine or host layout (Docker typically wants `host`), and an
/// empty value leaves the engine's default in place.
fn runtime_network(engine: &[String]) -> Option<String> {
    if let Ok(value) = std::env::var("MAKO_EDGE_TEST_NETWORK") {
        return (!value.is_empty()).then_some(value);
    }
    // Asked, not assumed from the binary's name: where `podman-docker` is
    // installed, `docker` *is* podman, and on podman's default network a
    // worker's fetch to the data plane is refused -- the services listen on
    // the host's loopback, which only `--map-host-loopback` reaches. Guessing
    // by name left the suite red with a connection refused from inside the
    // function and nothing pointing at the network.
    engine_is_podman(engine).then(|| "pasta:--map-host-loopback,169.254.1.2".to_owned())
}

fn engine_is_podman(engine: &[String]) -> bool {
    if engine
        .first()
        .is_some_and(|binary| binary.ends_with("podman"))
    {
        return true;
    }
    engine_command(engine)
        .arg("--version")
        .output()
        .is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .to_ascii_lowercase()
                .contains("podman")
        })
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
    // The runtime supplies `@mako-cloud/edge-sdk` to every worker from this
    // file. Without it the supervisor refuses to start, and a function that
    // imports the SDK could not resolve it.
    assert!(
        main_worker.join("edge-sdk-source.ts").exists(),
        "{} is missing; run `npm run build:runtime-module -w @mako-cloud/edge-sdk`",
        main_worker.join("edge-sdk-source.ts").display()
    );

    let mut command = engine_command(engine);
    command
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
        .args(runtime_environment());
    if let Some(network) = runtime_network(engine) {
        command.args(["--network".to_owned(), network]);
    }
    let status = command
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
