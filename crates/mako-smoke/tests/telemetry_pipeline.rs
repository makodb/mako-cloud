//! Prove observed work reaches the management API.
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
const COLLECTION_ID: &str = "todos";
const APP_EMAIL: &str = "telemetry-user@local.test";
const APP_PASSWORD: &str = "TelemetryUserPass1!";
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(60);

#[test]
fn observed_events_and_usage_reach_the_management_api() {
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

    // Replication is metered, so serving it must also report usage.
    let session_response: Value = serde_json::from_str(&body).expect("sign-in reports json");
    let access = session_response["accessToken"]
        .as_str()
        .expect("sign-in issues an access token")
        .to_owned();
    let mut replicating = keyed.clone();
    replicating.insert("authorization".to_owned(), format!("Bearer {access}"));
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/collections/{COLLECTION_ID}/replication/pull"),
        &replicating,
        Some(&json!({ "schemaVersion": 1, "batchSize": 10 })),
    );
    assert!(
        (200..300).contains(&status),
        "replication pull failed with {status}: {body}"
    );

    // A push changes stored size, which is what makes the tenant due for a
    // measurement. Nothing else in this test would produce one.
    let mut pushing = replicating.clone();
    pushing.insert(
        "idempotency-key".to_owned(),
        "telemetry-push-000001".to_owned(),
    );
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/collections/{COLLECTION_ID}/replication/push"),
        &pushing,
        Some(&json!({
            "schemaVersion": 1,
            "rows": [{
                "mutationId": "telemetry-mutation-000001",
                "newDocumentState": {
                    "id": "telemetry-1",
                    "ownerId": "telemetry-user",
                    "title": "stored so the tenant has a size",
                    "updatedAt": 1,
                },
            }],
        })),
    );
    assert!(
        (200..300).contains(&status),
        "replication push failed with {status}: {body}"
    );

    // --- The developer reads it back through the management API. -----------

    let session =
        mint_developer_session(&binaries, root, control_port, DEVELOPER_ID, DEVELOPER_EMAIL);
    let reading = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    // The bill is derived from the same records, so once usage has arrived it
    // must show up as billed quantities -- and must say it is not payable.
    let (status, body) = request(
        control_port,
        "GET",
        "/v1/organizations/org_localboot/bill",
        &reading,
        None,
    );
    assert_eq!(status, 200, "the bill was not served: {body}");
    let bill: Value = serde_json::from_str(&body).expect("bill json");
    assert_eq!(
        bill["collectable"], false,
        "the beta bill claims to be collectable"
    );
    assert_eq!(
        bill["totalMicroDollars"], 0,
        "a free organization was billed money: {body}"
    );
    assert_eq!(bill["creditsMicroDollars"], 0);
    assert_eq!(bill["balanceMicroDollars"], 0);
    assert!(
        bill["notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("no charge")),
        "the bill does not say nothing will be charged: {body}"
    );
    assert_eq!(
        bill["finalized"], false,
        "the live current month claimed to be a closed invoice: {body}"
    );

    // A month before the organization existed has no invoice to show, and a
    // malformed period is refused rather than guessed at.
    let (status, body) = request(
        control_port,
        "GET",
        "/v1/organizations/org_localboot/bill?period=2020-01",
        &reading,
        None,
    );
    assert_eq!(
        status, 404,
        "a period before the organization answered something: {body}"
    );
    let (status, body) = request(
        control_port,
        "GET",
        "/v1/organizations/org_localboot/bill?period=2020-13",
        &reading,
        None,
    );
    assert_eq!(status, 400, "a nonsense period was accepted: {body}");

    // A refused replication request must surface as a replication error:
    // this is what a developer debugging a client that cannot sync reads.
    let mut bad = keyed.clone();
    bad.insert(
        "authorization".to_owned(),
        format!("Bearer {}", "x".repeat(64)),
    );
    let (status, _) = request(
        data_port,
        "POST",
        &format!("{scope}/collections/{COLLECTION_ID}/replication/pull"),
        &bad,
        Some(&json!({ "schemaVersion": 1, "batchSize": 10 })),
    );
    assert!(
        !(200..300).contains(&status),
        "a garbage token was accepted for replication"
    );

    // An index built through the management API must surface as an
    // index-state record: the state deciding whether queries are answerable is
    // the thing this signal exists to show.
    let mut indexing = reading.clone();
    indexing.insert(
        "idempotency-key".to_owned(),
        "telemetry-index-000001".to_owned(),
    );
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/collections/{COLLECTION_ID}/indexes"),
        &indexing,
        Some(&json!({
            "name": "owner_index",
            "version": 1,
            "kind": "non_unique",
            "fields": [{ "path": "ownerId", "direction": "ascending" }],
        })),
    );
    assert!(
        (200..300).contains(&status),
        "index creation failed with {status}: {body}"
    );

    // Each signal names something only this test could have produced, so a
    // page that merely arrives is not mistaken for the record being reported.
    for (signal, described, marker) in [
        ("auth-events", "authentication event", "application_signin"),
        (
            "usage",
            "replication usage record",
            "replication_requests_per_minute",
        ),
        ("usage", "stored size sample", "storage_bytes"),
        ("usage", "application user count", "application_users"),
        ("health", "tenant health record", "mako-data-plane"),
        ("index-states", "index state record", "owner_index"),
        ("replication-errors", "replication error record", "todos"),
    ] {
        await_signal(
            control_port,
            &reading,
            &format!("{scope}/observability/{signal}?limit=50"),
            described,
            marker,
        );
    }

    // Once usage has arrived, the bill's quantities must derive from it: the
    // storage sample this test caused has to show up as a rated quantity, not
    // just as a telemetry record.
    let (status, body) = request(
        control_port,
        "GET",
        "/v1/organizations/org_localboot/bill",
        &reading,
        None,
    );
    assert_eq!(status, 200, "the bill was not served after usage: {body}");
    let bill: Value = serde_json::from_str(&body).expect("bill json");
    let stored = bill["lineItems"]
        .as_array()
        .expect("line items")
        .iter()
        .find(|item| item["resource"] == "storage_bytes")
        .expect("a storage line item")
        .clone();
    assert!(
        stored["quantity"].as_u64().is_some_and(|value| value > 0),
        "the bill shows no stored bytes although a sample was reported: {body}"
    );
    assert_eq!(bill["totalMicroDollars"], 0, "free stayed free: {body}");
}

/// Poll one observability signal until a record arrives.
fn await_signal(
    control_port: u16,
    reading: &BTreeMap<String, String>,
    path: &str,
    described: &str,
    marker: &str,
) {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    // Assigned on every path that reaches the deadline check.
    let mut last;
    loop {
        let (status, body) = request(control_port, "GET", path, reading, None);
        if status == 200 {
            // The response has to be a well-formed page before its contents
            // mean anything.
            serde_json::from_str::<Value>(&body).expect("observability page");
            // Delivery is batched, so a page can arrive carrying earlier
            // records before the one this test is waiting for. Keep polling
            // until the marker shows up rather than judging the first page.
            if body.contains(marker) {
                return;
            }
            last = format!("page has not carried {marker} yet: {body}");
        } else {
            last = format!("status {status}: {body}");
        }
        assert!(
            Instant::now() < deadline,
            "no {described} reached the management API within {DELIVERY_TIMEOUT:?} ({last})"
        );
        sleep(Duration::from_millis(500));
    }
}
