//! Prove a project moves between owners without anything else moving.
//!
//! A developer creates a project in their personal space, gives it an
//! environment and a collection, creates a team they own, and transfers the
//! project there. Afterwards the project is listed under the team only, keeps
//! its identifier and its collection, and comes back to the personal space
//! the same way. A rename changes the name and nothing else.
use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::{Duration, Instant};

use mako_smoke::{
    await_readiness, binary_directory, free_ports, mint_developer_session, request, run_bootstrap,
    scratch_root, service_environment, start_service,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";

fn await_active(control_port: u16, headers: &BTreeMap<String, String>, path: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let (status, body) = request(control_port, "GET", path, headers, None);
        assert_eq!(status, 200, "reading {path} failed: {body}");
        let record: Value = serde_json::from_str(&body).expect("lifecycle json");
        match record["state"].as_str() {
            Some("active") => return record,
            Some("provisioning") => {}
            other => panic!("{path} reached {other:?}: {body}"),
        }
        assert!(Instant::now() < deadline, "{path} did not become active");
        sleep(Duration::from_millis(250));
    }
}

fn project_ids(
    control_port: u16,
    headers: &BTreeMap<String, String>,
    team_id: &str,
) -> Vec<String> {
    let (status, body) = request(
        control_port,
        "GET",
        &format!("/v1/projects?teamId={team_id}"),
        headers,
        None,
    );
    assert_eq!(status, 200, "listing projects of {team_id} failed: {body}");
    let page: Value = serde_json::from_str(&body).expect("projects json");
    page["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["id"].as_str().expect("id").to_owned())
        .collect()
}

#[test]
fn a_project_is_transferred_between_a_personal_space_and_a_team() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-transfer-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let environment = service_environment(root);
    run_bootstrap(&binaries, &environment);
    let [data_port, control_port] = free_ports::<2>();
    let mut data_environment = environment.clone();
    data_environment.insert(
        "MAKO_BIND_ADDR".to_owned(),
        format!("127.0.0.1:{data_port}"),
    );
    let _data = start_service(
        "mako-data-plane",
        &binaries,
        &data_environment,
        data_port,
        root.join("data-plane.log"),
    );
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
    let bearer = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let manage = |key: &str| {
        let mut headers = bearer.clone();
        headers.insert("idempotency-key".to_owned(), format!("transfer-{key}"));
        headers
    };

    // --- An individual project with an environment and a collection. --------
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/projects",
        &manage("project"),
        Some(&json!({ "name": "Notes", "region": "local" })),
    );
    assert!(
        (200..300).contains(&status),
        "project creation failed: {body}"
    );
    let project: Value = serde_json::from_str(&body).expect("project json");
    let project_id = project["id"].as_str().expect("project id").to_owned();
    let personal_id = project["teamId"].as_str().expect("owner").to_owned();
    await_active(control_port, &bearer, &format!("/v1/projects/{project_id}"));
    let (status, body) = request(
        control_port,
        "POST",
        &format!("/v1/projects/{project_id}/environments"),
        &manage("environment"),
        Some(&json!({ "name": "production" })),
    );
    assert!(
        (200..300).contains(&status),
        "environment creation failed: {body}"
    );
    let environment_record: Value = serde_json::from_str(&body).expect("environment json");
    let environment_id = environment_record["id"]
        .as_str()
        .expect("environment id")
        .to_owned();
    await_active(
        control_port,
        &bearer,
        &format!("/v1/projects/{project_id}/environments/{environment_id}"),
    );
    let scope = format!("/v1/projects/{project_id}/environments/{environment_id}");
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/collections"),
        &manage("collection"),
        Some(&json!({
            "id": "notes",
            "schemaVersion": 1,
            "jsonSchema": {
                "type": "object",
                "required": ["id", "title"],
                "properties": { "id": { "type": "string" }, "title": { "type": "string" } },
                "additionalProperties": true,
            },
            "primaryKey": { "kind": "field", "field": "id" },
        })),
    );
    assert!(
        (200..300).contains(&status),
        "collection creation failed: {body}"
    );

    // --- A team the developer owns. ----------------------------------------
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/teams",
        &manage("team"),
        Some(&json!({ "name": "Acme" })),
    );
    assert!((200..300).contains(&status), "team creation failed: {body}");
    let team: Value = serde_json::from_str(&body).expect("team json");
    let team_id = team["id"].as_str().expect("team id").to_owned();
    assert_ne!(team_id, personal_id);

    // --- Transfer needs a confirmation; to the same owner it is refused. -----
    let mut confirmed = bearer.clone();
    confirmed.insert("confirmation".to_owned(), format!("transfer:{project_id}"));
    let (status, body) = request(
        control_port,
        "POST",
        &format!("/v1/projects/{project_id}/actions/transfer"),
        &bearer,
        Some(&json!({ "teamId": team_id })),
    );
    assert!(
        (400..500).contains(&status),
        "unconfirmed transfer must be refused: {body}"
    );
    let (status, body) = request(
        control_port,
        "POST",
        &format!("/v1/projects/{project_id}/actions/transfer"),
        &confirmed,
        Some(&json!({ "teamId": personal_id })),
    );
    assert_eq!(
        status, 409,
        "transfer to the current owner is refused: {body}"
    );

    // --- Personal space -> team: listed there only, resources intact. -------
    let (status, body) = request(
        control_port,
        "POST",
        &format!("/v1/projects/{project_id}/actions/transfer"),
        &confirmed,
        Some(&json!({ "teamId": team_id })),
    );
    assert_eq!(status, 200, "transfer to the team failed: {body}");
    let moved: Value = serde_json::from_str(&body).expect("moved project");
    assert_eq!(moved["id"], project_id);
    assert_eq!(moved["teamId"], team_id);
    assert_eq!(moved["state"], "active");
    assert_eq!(
        project_ids(control_port, &bearer, &team_id),
        vec![project_id.clone()]
    );
    assert!(project_ids(control_port, &bearer, &personal_id).is_empty());
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/collections/notes"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "the collection moved with the project: {body}");
    let (status, body) = request(
        control_port,
        "GET",
        &format!("/v1/projects/{project_id}/environments"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "{body}");
    let environments: Value = serde_json::from_str(&body).expect("environments json");
    assert_eq!(environments["items"][0]["id"], environment_id);

    // --- Team -> personal space, named by omission. --------------------------
    let (status, body) = request(
        control_port,
        "POST",
        &format!("/v1/projects/{project_id}/actions/transfer"),
        &confirmed,
        Some(&json!({})),
    );
    assert_eq!(
        status, 200,
        "transfer back to the personal space failed: {body}"
    );
    let returned: Value = serde_json::from_str(&body).expect("returned project");
    assert_eq!(returned["teamId"], personal_id);
    assert_eq!(
        project_ids(control_port, &bearer, &personal_id),
        vec![project_id.clone()]
    );
    assert!(project_ids(control_port, &bearer, &team_id).is_empty());

    // --- A rename changes the name and nothing else. -------------------------
    let (status, body) = request(
        control_port,
        "PATCH",
        &format!("/v1/projects/{project_id}"),
        &bearer,
        Some(&json!({ "name": "Field Notes" })),
    );
    assert_eq!(status, 200, "rename failed: {body}");
    let renamed: Value = serde_json::from_str(&body).expect("renamed project");
    assert_eq!(renamed["name"], "Field Notes");
    assert_eq!(renamed["teamId"], personal_id);
    assert_eq!(renamed["region"], "local");
    let (status, body) = request(
        control_port,
        "GET",
        &format!("/v1/projects?teamId={personal_id}"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "{body}");
    let listed: Value = serde_json::from_str(&body).expect("listing");
    assert_eq!(listed["items"][0]["name"], "Field Notes");
}
