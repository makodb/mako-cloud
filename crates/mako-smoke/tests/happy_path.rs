//! Drive Mako Cloud's application happy path against the real service binaries.
//!
//! Every other end-to-end suite in this repository runs against a mock: the
//! reference app ships an in-browser fake backend, and the console specs
//! intercept every `/v1/` request. This test starts the actual data plane and
//! control plane, seeds a tenant with the local bootstrap, and performs
//! application-user sign-up, sign-in, a document push, and a document pull over
//! HTTP. Nothing here is stubbed; if it passes, the happy path works.

use std::{collections::BTreeMap, process::Command};

use mako_smoke::{
    await_readiness, binary_directory, free_port, request, run_bootstrap, scratch_root,
    service_environment, start_service,
};
use serde_json::{Value, json};

const PROJECT_ID: &str = "prj_localboot";
const ENVIRONMENT_ID: &str = "env_localboot";
const COLLECTION_ID: &str = "todos";
const APP_EMAIL: &str = "smoke-user@local.test";
const APP_PASSWORD: &str = "SmokeUserPass1!";

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
