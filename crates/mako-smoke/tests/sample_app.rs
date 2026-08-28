//! Build a sample application the way a developer would, then use it.
//!
//! `happy_path` proves an already-provisioned tenant serves documents. This
//! test proves the step before it: a seeded developer, holding nothing but a
//! management session, creates a project, an environment, a collection, a
//! signing key, a public project key, and a document policy through the public
//! management API — and only then does an application user sign up against
//! that freshly built backend and replicate a document through it.
//!
//! It exists because those two halves live in different databases. The control
//! plane owns projects in SQLite; the data plane serves documents from RocksDB.
//! Everything the control plane creates has to cross that boundary over
//! internal RPC before an application can see it, and nothing else in the test
//! suite exercises that crossing end to end.
//!
//! The second test takes the data plane away mid-flight and proves the control
//! plane refuses to report a policy version active that the data plane never
//! received. That is the failure direction that matters: a developer reading
//! their new rules as live while document traffic is still evaluated against
//! the old ones.

use std::{
    collections::BTreeMap,
    thread::sleep,
    time::{Duration, Instant},
};

use mako_smoke::{
    ServiceProcess, await_readiness, binary_directory, free_ports, mint_developer_session, request,
    run_bootstrap, scratch_root, service_environment, start_service,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const ORGANIZATION_ID: &str = "org_localboot";
const COLLECTION_ID: &str = "notes";
const APP_EMAIL: &str = "sample-app-user@local.test";
const APP_PASSWORD: &str = "SampleAppPass1!";
const PROVISIONING_TIMEOUT: Duration = Duration::from_secs(90);

/// A control plane and data plane running against one throwaway tenant, with
/// a developer management session already minted.
struct Services {
    data: Option<ServiceProcess>,
    _control: ServiceProcess,
    data_port: u16,
    control_port: u16,
    session: String,
    // Declared last so it is dropped last: fields drop in declaration order,
    // and removing the directory before the processes release their database
    // locks would race them.
    _workspace: tempfile::TempDir,
}

impl Services {
    /// Take the data plane away, and do not return until it is really gone.
    fn stop_data_plane(&mut self) {
        self.data.take().expect("data plane is running").stop();
    }
}

fn start(prefix: &str) -> Services {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let environment = service_environment(root);

    // Seeds the developer and their organization only. Everything a test uses
    // beyond that is created through the API.
    run_bootstrap(&binaries, &environment);

    let [data_port, control_port] = free_ports::<2>();
    let data = start_service(
        "mako-data-plane",
        &binaries,
        &environment,
        data_port,
        root.join("data-plane.log"),
    );
    // The control plane reaches the data plane over loopback internal RPC, and
    // this run put the data plane on an ephemeral port rather than the default.
    let mut control_environment = environment.clone();
    control_environment.insert(
        "MAKO_DATA_PLANE_ENDPOINT".to_owned(),
        format!("127.0.0.1:{data_port}"),
    );
    let control = start_service(
        "mako-control-plane",
        &binaries,
        &control_environment,
        control_port,
        root.join("control-plane.log"),
    );
    await_readiness(data_port, "mako-data-plane");
    await_readiness(control_port, "mako-control-plane");

    let session =
        mint_developer_session(&binaries, root, control_port, DEVELOPER_ID, DEVELOPER_EMAIL);
    Services {
        data: Some(data),
        _control: control,
        data_port,
        control_port,
        session,
        _workspace: workspace,
    }
}

/// Everything a developer does through the management API to turn an empty
/// organization into a backend an application can sign in to. Returns the
/// environment's URL scope and the public project key issued along the way.
fn build_app(services: &Services) -> (String, String) {
    let control_port = services.control_port;
    let session = &services.session;
    let manage = |suffix: &str| -> BTreeMap<String, String> {
        BTreeMap::from([
            ("authorization".to_owned(), format!("Bearer {session}")),
            ("idempotency-key".to_owned(), format!("sample-app-{suffix}")),
        ])
    };

    // --- The developer creates the project and its environment. ------------

    let project = created(
        control_port,
        "/v1/projects",
        &manage("project"),
        Some(&json!({
            "teamId": ORGANIZATION_ID,
            "name": "Sample App",
            "region": "local",
        })),
    );
    let project_id = identifier(&project);
    let environment_record = created(
        control_port,
        &format!("/v1/projects/{project_id}/environments"),
        &manage("environment"),
        Some(&json!({ "name": "production" })),
    );
    let environment_id = identifier(&environment_record);

    // Both are created in `provisioning` and advanced by a background worker,
    // so a test that went straight to the next call would be racing it.
    await_active(
        control_port,
        session,
        &format!("/v1/projects/{project_id}"),
        "project",
    );
    await_active(
        control_port,
        session,
        &format!("/v1/projects/{project_id}/environments/{environment_id}"),
        "environment",
    );

    let scope = format!("/v1/projects/{project_id}/environments/{environment_id}");

    // --- The developer defines the collection the app will store into. -----

    created(
        control_port,
        &format!("{scope}/collections"),
        &manage("collection"),
        Some(&json!({
            "id": COLLECTION_ID,
            "schemaVersion": 1,
            "jsonSchema": {
                "type": "object",
                "required": ["id", "ownerId", "title", "updatedAt"],
                "properties": {
                    "id": { "type": "string" },
                    "ownerId": { "type": "string" },
                    "title": { "type": "string" },
                    "updatedAt": { "type": "integer" },
                },
                "additionalProperties": true,
            },
            "primaryKey": { "kind": "field", "field": "id" },
        })),
    );

    // --- Credentials: a signing key for sessions, a public key for the app. -

    created(
        control_port,
        &format!("{scope}/signing-keys/actions/initialize"),
        &manage("signing-key"),
        // Initialization takes no body; the endpoint rejects one.
        None,
    );
    let issued = created(
        control_port,
        &format!("{scope}/credentials/public"),
        &manage("public-key"),
        Some(&json!({ "id": "key_sampleapp01" })),
    );
    // The raw key is returned once, at creation, and never again.
    let public_key = issued["value"]
        .as_str()
        .expect("the public project key is returned at creation")
        .to_owned();

    // --- A policy, without which every document request is denied. ---------

    created(
        control_port,
        &format!("{scope}/collections/{COLLECTION_ID}/policies"),
        &manage("policy-draft"),
        Some(&json!({
            "version": 1,
            "rules": [{
                "id": "owner-full-access",
                "effect": "allow",
                "operations": ["create", "read", "update", "delete"],
                "expression": "true",
            }],
        })),
    );
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/collections/{COLLECTION_ID}/policies/1/actions/activate"),
        &manage("policy-activate"),
        None,
    );
    assert_eq!(status, 200, "activating the policy failed: {body}");
    let activated: Value = serde_json::from_str(&body).expect("activation reports the policy");
    assert_eq!(activated["policy"]["state"], "active");

    (scope, public_key)
}

#[test]
fn a_developer_builds_a_sample_app_and_an_application_user_replicates_through_it() {
    let services = start("mako-sample-app-");
    let data_port = services.data_port;
    let (scope, public_key) = build_app(&services);

    // --- The app is now built. An application user takes it from here. -----

    let keyed = BTreeMap::from([("x-mako-key".to_owned(), public_key)]);
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signup"),
        &keyed,
        Some(&json!({ "email": APP_EMAIL, "password": APP_PASSWORD })),
    );
    assert!(
        (200..300).contains(&status),
        "sign-up against the new project failed with {status}: {body}"
    );

    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signin"),
        &keyed,
        Some(&json!({ "email": APP_EMAIL, "password": APP_PASSWORD })),
    );
    assert!(
        (200..300).contains(&status),
        "sign-in against the new project failed with {status}: {body}"
    );
    let session_response: Value = serde_json::from_str(&body).expect("sign-in reports json");
    let access = session_response["accessToken"]
        .as_str()
        .expect("sign-in issues an access token")
        .to_owned();

    let mut replicating = keyed.clone();
    replicating.insert("authorization".to_owned(), format!("Bearer {access}"));

    // --- Push and pull, the way the RxDB replication protocol does. ---------

    let mut pushing = replicating.clone();
    pushing.insert(
        "idempotency-key".to_owned(),
        "sample-app-push-000001".to_owned(),
    );
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/collections/{COLLECTION_ID}/replication/push"),
        &pushing,
        Some(&json!({
            "schemaVersion": 1,
            "rows": [{
                "mutationId": "sample-app-mutation-000001",
                "newDocumentState": {
                    "id": "note-1",
                    "ownerId": "sample-app-user",
                    "title": "written through the app the developer just built",
                    "updatedAt": 1,
                },
            }],
        })),
    );
    assert!(
        (200..300).contains(&status),
        "push failed with {status}: {body}"
    );
    let pushed: Value = serde_json::from_str(&body).expect("push reports json");
    assert_eq!(
        pushed["outcomes"][0]["status"], "accepted",
        "push was not accepted: {body}"
    );

    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/collections/{COLLECTION_ID}/replication/pull"),
        &replicating,
        Some(&json!({ "schemaVersion": 1, "batchSize": 10 })),
    );
    assert!(
        (200..300).contains(&status),
        "pull failed with {status}: {body}"
    );
    let pulled: Value = serde_json::from_str(&body).expect("pull reports json");
    let titles: Vec<&str> = pulled["documents"]
        .as_array()
        .expect("pull reports documents")
        .iter()
        .filter_map(|document| document["title"].as_str())
        .collect();
    assert_eq!(
        titles,
        vec!["written through the app the developer just built"],
        "the pushed document did not come back: {body}"
    );
}

#[test]
fn policy_activation_fails_when_the_data_plane_cannot_record_it() {
    let mut services = start("mako-policy-outage-");
    let control_port = services.control_port;
    let session = services.session.clone();
    let (scope, _) = build_app(&services);
    let policies = format!("{scope}/collections/{COLLECTION_ID}/policies");
    let reading = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let manage = |suffix: &str| -> BTreeMap<String, String> {
        BTreeMap::from([
            ("authorization".to_owned(), format!("Bearer {session}")),
            (
                "idempotency-key".to_owned(),
                format!("policy-outage-{suffix}"),
            ),
        ])
    };

    // A second version, authored while everything is still healthy. Its rules
    // differ from version 1 so that enforcing the wrong one would be visible.
    created(
        control_port,
        &policies,
        &manage("draft"),
        Some(&json!({
            "version": 2,
            "rules": [{
                "id": "read-only",
                "effect": "allow",
                "operations": ["read"],
                "expression": "true",
            }],
        })),
    );

    // The data plane is the store that actually enforces policies. Taking it
    // away is the outage this scenario is about.
    services.stop_data_plane();

    let (status, body) = request(
        control_port,
        "POST",
        &format!("{policies}/2/actions/activate"),
        &manage("activate"),
        None,
    );
    assert!(
        !(200..300).contains(&status),
        "activation reported success while the data plane was unreachable: {body}"
    );
    let error: Value = serde_json::from_str(&body).expect("activation reports an api error");
    assert!(
        error["error"]["retry"]["kind"]
            .as_str()
            .is_some_and(|kind| kind != "never"),
        "activation failure is not retryable, so a caller has no path back: {body}"
    );

    // The version the data plane never received must not be presented as the
    // one in force. Version 1 is what documents are still evaluated against.
    let (status, body) = request(control_port, "GET", &policies, &reading, None);
    assert_eq!(status, 200, "reading the active policy failed: {body}");
    let active: Value = serde_json::from_str(&body).expect("active policy reports json");
    assert_eq!(
        active["policy"]["version"], 1,
        "a version the data plane never recorded is reported as active: {body}"
    );
}

/// Perform a management call that is expected to create something.
fn created(
    port: u16,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: Option<&Value>,
) -> Value {
    let (status, response) = request(port, "POST", path, headers, body);
    assert!(
        (200..300).contains(&status),
        "POST {path} failed with {status}: {response}"
    );
    serde_json::from_str(&response).unwrap_or(Value::Null)
}

fn identifier(record: &Value) -> String {
    record["id"]
        .as_str()
        .unwrap_or_else(|| panic!("record has no identifier: {record}"))
        .to_owned()
}

/// Wait for an asynchronously provisioned resource to become usable.
fn await_active(port: u16, session: &str, path: &str, what: &str) {
    let headers = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let deadline = Instant::now() + PROVISIONING_TIMEOUT;
    let mut last = String::from("no response");
    while Instant::now() < deadline {
        let (status, body) = request(port, "GET", path, &headers, None);
        if status == 200 {
            let record: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            match record["state"].as_str() {
                Some("active") => return,
                Some(state) => last = format!("state {state}"),
                None => last = format!("no state in {body}"),
            }
        } else {
            last = format!("status {status}: {body}");
        }
        sleep(Duration::from_millis(500));
    }
    panic!("{what} did not become active within {PROVISIONING_TIMEOUT:?} ({last})");
}
