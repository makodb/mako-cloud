//! Prove an observed event reaches the management API.
//!
//! The telemetry store has always had an ingest endpoint and nothing has ever
//! called it, so `queryAuthenticationEvents`, `queryProjectLogs`,
//! `queryProjectHealth`, `queryProjectUsage` and the rest answered from an
//! empty store on every deployment. This drives the whole loop: an application
//! user signs in against the data plane, the data plane emits the event, the
//! telemetry service retains it, and the developer reads it back through the
//! control plane.

use std::{
    collections::BTreeMap,
    fs,
    process::Command,
    thread::sleep,
    time::{Duration, Instant},
};

use mako_smoke::{
    INTERNAL_AUTH_SECRET, await_readiness, binary_directory, free_ports, mint_developer_session,
    request, run_bootstrap, scratch_root, service_environment, start_service,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const PROJECT_ID: &str = "prj_localboot";
const ENVIRONMENT_ID: &str = "env_localboot";
const APP_EMAIL: &str = "telemetry-user@local.test";
const APP_PASSWORD: &str = "TelemetryUserPass1!";
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(60);

#[test]
fn an_observed_authentication_event_reaches_the_management_api() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-telemetry-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let environment = service_environment(root);
    let bootstrap = run_bootstrap(&binaries, &environment);
    let public_key = bootstrap["publicProjectKey"]
        .as_str()
        .expect("bootstrap reports a public project key")
        .to_owned();

    let [data_port, control_port, telemetry_port] = free_ports::<3>();

    // The telemetry service authenticates ingest and query with the shared
    // internal secret, read from a file.
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
    // A RocksDB-owned service refuses to open an unprovisioned volume, which
    // is the same fail-closed rule production relies on.
    let telemetry_db = root.join("telemetry");
    let provisioned = Command::new(binaries.join("mako-storage-ops"))
        .arg("provision")
        .arg(format!(
            "--database-path={}",
            telemetry_db.to_string_lossy()
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

    // Both planes are pointed at that telemetry service: the data plane emits
    // to it, the control plane queries it back.
    let mut plane_environment = environment.clone();
    plane_environment.insert(
        "MAKO_TELEMETRY_QUERY_ENDPOINT".to_owned(),
        format!("127.0.0.1:{telemetry_port}"),
    );
    let _data = start_service(
        "mako-data-plane",
        &binaries,
        &plane_environment,
        data_port,
        root.join("data-plane.log"),
    );
    let mut control_environment = plane_environment.clone();
    control_environment.insert(
        "MAKO_DATA_PLANE_ENDPOINT".to_owned(),
        format!("127.0.0.1:{data_port}"),
    );
    let _control = start_service(
        "mako-control-plane",
        &binaries,
        &control_environment,
        control_port,
        root.join("control-plane.log"),
    );
    await_readiness(data_port, "mako-data-plane");
    await_readiness(control_port, "mako-control-plane");

    // --- Something observable happens. -------------------------------------

    let scope = format!("/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}");
    let keyed = BTreeMap::from([("x-mako-key".to_owned(), public_key)]);
    let credentials = json!({ "email": APP_EMAIL, "password": APP_PASSWORD });
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signup"),
        &keyed,
        Some(&credentials),
    );
    assert!(
        (200..300).contains(&status),
        "sign-up failed with {status}: {body}"
    );
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signin"),
        &keyed,
        Some(&credentials),
    );
    assert!(
        (200..300).contains(&status),
        "sign-in failed with {status}: {body}"
    );

    // --- The developer reads it back through the management API. -----------

    let session =
        mint_developer_session(&binaries, root, control_port, DEVELOPER_ID, DEVELOPER_EMAIL);
    let reading = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let path = format!("{scope}/observability/auth-events?limit=50");

    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    // Assigned on every path that reaches the deadline check.
    let mut last;
    loop {
        let (status, body) = request(control_port, "GET", &path, &reading, None);
        if status == 200 {
            let page: Value = serde_json::from_str(&body).expect("observability page");
            let items = page["items"].as_array().map_or(0, Vec::len);
            if items > 0 {
                // The record has to be the one this test caused, not any record.
                let carries_auth = page["items"].as_array().is_some_and(|items| {
                    items.iter().any(|item| {
                        item["payload"]["signal"] == "authentication_event"
                            || item["payload"].get("category").is_some()
                    })
                });
                assert!(
                    carries_auth,
                    "the page carries no authentication event: {body}"
                );
                return;
            }
            last = format!("page was empty: {body}");
        } else {
            last = format!("status {status}: {body}");
        }
        assert!(
            Instant::now() < deadline,
            "no authentication event reached the management API within {DELIVERY_TIMEOUT:?} ({})",
            last
        );
        sleep(Duration::from_millis(500));
    }
}
