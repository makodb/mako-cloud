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

use std::{
    collections::BTreeMap,
    thread::sleep,
    time::{Duration, Instant},
};

use mako_smoke::{
    await_readiness, binary_directory, free_ports, mint_developer_session, request, run_bootstrap,
    scratch_root, service_environment, start_service,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const ORGANIZATION_ID: &str = "org_localboot";
const COLLECTION_ID: &str = "notes";
const APP_EMAIL: &str = "sample-app-user@local.test";
const APP_PASSWORD: &str = "SampleAppPass1!";
const PROVISIONING_TIMEOUT: Duration = Duration::from_secs(90);

#[test]
fn a_developer_builds_a_sample_app_and_an_application_user_replicates_through_it() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-sample-app-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let environment = service_environment(root);

    // Seeds the developer and their organization only. Everything this test
    // uses beyond that is created through the API.
    run_bootstrap(&binaries, &environment);

    let [data_port, control_port] = free_ports::<2>();
    let _data = start_service(
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
    let _control = start_service(
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
            "organizationId": ORGANIZATION_ID,
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
        &session,
        &format!("/v1/projects/{project_id}"),
        "project",
    );
    await_active(
        control_port,
        &session,
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
