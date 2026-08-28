//! Prove a developer can own a project without a team.
//!
//! Creating a project without naming a team must land it in the caller's
//! personal space -- created on first use, reused on the next, listed among
//! their teams marked personal, and refusing the things that only make
//! sense with more than one person in them.

use std::collections::BTreeMap;

use mako_smoke::{
    await_readiness, binary_directory, free_ports, mint_developer_session, request, run_bootstrap,
    scratch_root, service_environment, start_service,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";

#[test]
fn a_developer_creates_individual_projects_in_a_personal_space() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-personal-")
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
    let mut headers = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);

    // No team named: the project lands in a personal space.
    headers.insert(
        "idempotency-key".to_owned(),
        "personal-project-01".to_owned(),
    );
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/projects",
        &headers,
        Some(&json!({ "name": "My Notes", "region": "local" })),
    );
    // Creation answers 202: the project provisions asynchronously.
    assert!(
        (200..300).contains(&status),
        "individual project creation failed with {status}: {body}"
    );
    let project: Value = serde_json::from_str(&body).expect("project json");
    let personal_id = project["teamId"]
        .as_str()
        .expect("an individual project still names the team that holds it")
        .to_owned();
    assert!(personal_id.starts_with("org_"), "{personal_id}");

    // A second individual project lands in the same space.
    headers.insert(
        "idempotency-key".to_owned(),
        "personal-project-02".to_owned(),
    );
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/projects",
        &headers,
        Some(&json!({ "name": "Side Project", "region": "local" })),
    );
    assert!(
        (200..300).contains(&status),
        "second individual project failed with {status}: {body}"
    );
    let second: Value = serde_json::from_str(&body).expect("project json");
    assert_eq!(
        second["teamId"], personal_id,
        "a second individual project did not reuse the personal space"
    );

    // The space is listed among the developer's teams, marked personal.
    headers.remove("idempotency-key");
    let (status, body) = request(control_port, "GET", "/v1/teams", &headers, None);
    assert_eq!(status, 200, "team listing failed: {body}");
    let teams: Value = serde_json::from_str(&body).expect("teams json");
    let personal = teams["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|team| team["id"] == personal_id)
        .unwrap_or_else(|| panic!("the personal space is missing from the team list: {body}"));
    assert_eq!(personal["kind"], "personal", "{body}");
    // The bootstrapped team is still an ordinary team.
    let ordinary = teams["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|team| team["id"] == "org_localboot")
        .expect("the bootstrapped team is listed");
    assert_eq!(ordinary["kind"], "team");

    // One person's space refuses invitations and deletion.
    headers.insert(
        "idempotency-key".to_owned(),
        "personal-invite-01".to_owned(),
    );
    let (status, body) = request(
        control_port,
        "POST",
        &format!("/v1/teams/{personal_id}/invitations"),
        &headers,
        Some(&json!({
            "email": "friend@local.test",
            "role": "developer",
            "expiresAt": "2030-01-01T00:00:00Z"
        })),
    );
    assert_eq!(
        status, 409,
        "a personal space accepted an invitation: {body}"
    );
    assert!(body.contains("personal space"), "{body}");
    headers.remove("idempotency-key");
    headers.insert(
        "confirmation".to_owned(),
        "DELETE_PERSONAL_SPACE".to_owned(),
    );
    let (status, body) = request(
        control_port,
        "DELETE",
        &format!("/v1/teams/{personal_id}"),
        &headers,
        None,
    );
    assert_eq!(status, 409, "a personal space accepted deletion: {body}");
}
