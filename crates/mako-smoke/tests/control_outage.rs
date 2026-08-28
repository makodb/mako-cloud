//! Prove the control plane keeps working when the data plane is gone.
//!
//! The two planes own different databases: the control plane owns developers,
//! organizations, and projects in SQLite, and the data plane owns application
//! users, documents, and policies in RocksDB. The whole point of that split is
//! that a tenant database failure must not take the portal down with it — a
//! developer still has to be able to sign in and diagnose the outage.
//!
//! Nothing else tests that against running processes. This starts the control
//! plane alone, pointed at an address where no data plane is listening, and
//! drives it over HTTP.

use std::{collections::BTreeMap, process::Command, time::Duration};

use mako_smoke::{
    await_readiness, binary_directory, free_ports, request, run_bootstrap, scratch_root,
    service_environment, start_service,
};
use serde_json::{Value, json};

const DEVELOPER_EMAIL: &str = "developer@local.test";
const DEVELOPER_PASSWORD: &str = "LocalBootstrap1!";
const ORGANIZATION_ID: &str = "org_localboot";
const PROJECT_ID: &str = "prj_localboot";
const ENVIRONMENT_ID: &str = "env_localboot";

#[test]
fn control_operations_continue_while_the_data_plane_is_unavailable() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-control-outage-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let environment = service_environment(root);

    // Seeded while nothing holds the locks, exactly as a real deployment would
    // have been before the outage started.
    run_bootstrap(&binaries, &environment);

    // The third port is never bound. That is the outage: the control plane is
    // configured for a data plane that is not there.
    let [control_port, absent_data_port, _spare] = free_ports::<3>();
    let mut control_environment = environment.clone();
    control_environment.insert(
        "MAKO_DATA_PLANE_ENDPOINT".to_owned(),
        format!("127.0.0.1:{absent_data_port}"),
    );
    // Hosted developer sign-in is part of the registration surface, which is
    // deny-by-default and refuses to open without protected mail settings.
    // Configuring them is enough — signing in sends no mail, so the relay
    // named here is never contacted.
    for (name, value) in [
        ("MAKO_DEVELOPER_REGISTRATION_ENABLED", "true"),
        ("MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME", "smtp.invalid"),
        ("MAKO_DEVELOPER_SMTP_PORT", "587"),
        ("MAKO_DEVELOPER_SMTP_TLS_MODE", "starttls"),
        ("MAKO_DEVELOPER_SMTP_USERNAME", "smoke@smtp.invalid"),
        (
            "MAKO_DEVELOPER_SMTP_SENDER",
            "Mako Smoke <no-reply@smtp.invalid>",
        ),
        (
            "MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF",
            "env:MAKO_SMOKE_MAIL_KEY",
        ),
        (
            "MAKO_DEVELOPER_SMTP_PASSWORD_REF",
            "env:MAKO_SMOKE_SMTP_PASSWORD",
        ),
        (
            "MAKO_SMOKE_MAIL_KEY",
            "smoke-developer-mail-encryption-secret",
        ),
        ("MAKO_SMOKE_SMTP_PASSWORD", "smoke-developer-smtp-password"),
    ] {
        control_environment.insert(name.to_owned(), value.to_owned());
    }
    let _control = start_service(
        "mako-control-plane",
        &binaries,
        &control_environment,
        control_port,
        root.join("control-plane.log"),
    );

    // Readiness is the first claim: the control plane must serve at all.
    await_readiness(control_port, "mako-control-plane");

    // --- The developer authenticates against control-owned identity. -------

    // Hosted sign-in is same-origin only, and with no public URL configured
    // the control plane's own origin is its bind address.
    let same_origin = BTreeMap::from([(
        "origin".to_owned(),
        format!("http://127.0.0.1:{control_port}"),
    )]);
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/developer-auth/sessions",
        &same_origin,
        Some(&json!({ "email": DEVELOPER_EMAIL, "password": DEVELOPER_PASSWORD })),
    );
    assert!(
        (200..300).contains(&status),
        "developer sign-in failed while the data plane was down, {status}: {body}"
    );
    let session: Value = serde_json::from_str(&body).expect("sign-in reports json");
    let access = session["accessToken"]
        .as_str()
        .expect("sign-in issues an access token")
        .to_owned();
    let authorized = BTreeMap::from([("authorization".to_owned(), format!("Bearer {access}"))]);

    // Step-up re-verification reads the stored credential back, so the rest of
    // the control-owned identity lifecycle is answering from the control
    // authority and not from the failed database.
    let mut stepping_up = authorized.clone();
    stepping_up.extend(same_origin.clone());
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/developer-auth/sessions/current/actions/verify-password",
        &stepping_up,
        Some(&json!({ "password": DEVELOPER_PASSWORD })),
    );
    assert!(
        (200..300).contains(&status),
        "password step-up failed while the data plane was down, {status}: {body}"
    );

    // --- Control-owned reads answer from SQLite. ---------------------------

    let (status, body) = request(
        control_port,
        "GET",
        &format!("/v1/projects/{PROJECT_ID}"),
        &authorized,
        None,
    );
    assert_eq!(
        status, 200,
        "reading control-owned project metadata failed: {body}"
    );

    // --- A tenant-dependent operation is scoped, not fatal. ----------------

    let mut mutating = authorized.clone();
    mutating.insert(
        "idempotency-key".to_owned(),
        "control-outage-collection".to_owned(),
    );
    let (status, body) = request(
        control_port,
        "POST",
        &format!("/v1/projects/{PROJECT_ID}/environments/{ENVIRONMENT_ID}/collections"),
        &mutating,
        Some(&json!({
            "id": "outage",
            "schemaVersion": 1,
            "jsonSchema": { "type": "object", "properties": { "id": { "type": "string" } } },
            "primaryKey": { "kind": "field", "field": "id" },
        })),
    );
    assert!(
        !(200..300).contains(&status),
        "a collection was reported created without a data plane to record it: {body}"
    );
    let error: Value = serde_json::from_str(&body).expect("the failure reports an api error");
    assert_eq!(
        error["error"]["code"], "unavailable",
        "the tenant-dependent failure is not reported as an unavailable dependency: {body}"
    );

    // --- And the control API is still whole afterwards. --------------------

    let (status, body) = request(control_port, "GET", "/v1/teams", &authorized, None);
    assert_eq!(
        status, 200,
        "the control API stopped serving after one tenant-dependent failure: {body}"
    );
    let organizations: Value = serde_json::from_str(&body).expect("organizations report json");
    assert!(
        organizations["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"] == ORGANIZATION_ID)),
        "control-owned organization state was not readable: {body}"
    );
}

/// The other direction: when the control plane's own authority is the thing
/// that is unavailable, it must refuse to serve rather than come up on an
/// empty or unusable database.
#[test]
fn control_plane_refuses_to_serve_when_its_control_authority_is_unavailable() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-control-unavailable-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let mut environment = service_environment(root);

    // The configured database path is a directory, so the control authority
    // cannot be opened as a database file. Any other unopenable path would do;
    // what matters is that the service is asked to serve without one.
    let occupied = root.join("control/occupied");
    std::fs::create_dir_all(&occupied).expect("occupied path");
    environment.insert(
        "MAKO_CONTROL_SQLITE_PATH".to_owned(),
        occupied.to_string_lossy().into_owned(),
    );

    let [control_port] = free_ports::<1>();
    let output = Command::new(binaries.join("mako-control-plane"))
        .envs(&environment)
        .env("MAKO_BIND_ADDR", format!("127.0.0.1:{control_port}"))
        .output()
        .expect("control plane runs");

    assert!(
        !output.status.success(),
        "the control plane started without a usable control authority"
    );
    // Named, so this cannot pass on an unrelated startup failure.
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("control-plane storage could not be opened"),
        "refused for some other reason than its control authority: {diagnostic}"
    );
    // Nothing may be left serving on the port it was asked to bind.
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", control_port)).is_err(),
        "the control plane is answering on {control_port} after refusing to start"
    );
}
