//! Prove a project's own domain is served only while its DNS proof stands.
//!
//! A developer adds a domain and gets a TXT record to publish; until it is
//! published nothing is served on the name and no certificate may be issued
//! for it (the `ask` gate says no). Publishing the record verifies the
//! domain: the gate says yes and the application API answers requests that
//! arrive on the name. The environment's own cross-origin allowlist decides
//! which browser origins may call that API -- the same answer on the
//! platform's hostname and on the domain, and never on the management API.
//! Removing the record fails re-verification, serving stops, and the
//! developer is told why. Removing the domain ends it.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mako_smoke::{
    DnsStub, await_readiness, binary_directory, free_ports, mint_developer_session, request,
    run_bootstrap, scratch_root, service_environment, start_service, try_request_full,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const HOSTNAME: &str = "api.example.test";

fn await_active(control_port: u16, headers: &BTreeMap<String, String>, path: &str) {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let (status, body) = request(control_port, "GET", path, headers, None);
        assert_eq!(status, 200, "reading {path} failed: {body}");
        let record: Value = serde_json::from_str(&body).expect("lifecycle json");
        match record["state"].as_str() {
            Some("active") => return,
            Some("provisioning") => {}
            other => panic!("{path} reached {other:?}: {body}"),
        }
        assert!(Instant::now() < deadline, "{path} did not become active");
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn a_domain_is_served_only_while_its_dns_proof_stands() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-domains-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let dns = DnsStub::start();
    let mut environment = service_environment(root);
    environment.insert("MAKO_DNS_RESOLVER".to_owned(), dns.address.to_string());
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
        headers.insert("idempotency-key".to_owned(), format!("domains-smoke-{key}"));
        headers
    };

    // --- A project, an environment, and a public key. -----------------------
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/projects",
        &manage("project"),
        Some(&json!({ "name": "Domains", "region": "local" })),
    );
    assert!(
        (200..300).contains(&status),
        "project creation failed: {body}"
    );
    let project: Value = serde_json::from_str(&body).expect("project json");
    let project_id = project["id"].as_str().expect("project id").to_owned();
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
    let scope = format!("/v1/projects/{project_id}/environments/{environment_id}");
    await_active(control_port, &bearer, &scope);
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/signing-keys/actions/initialize"),
        &manage("signing"),
        None,
    );
    assert!(
        (200..300).contains(&status),
        "signing key init failed: {body}"
    );
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/credentials/public"),
        &manage("public-key"),
        Some(&json!({ "id": "key_domains0001" })),
    );
    assert!((200..300).contains(&status), "public key failed: {body}");
    let issued: Value = serde_json::from_str(&body).expect("key json");
    let public_key = issued["value"].as_str().expect("key value").to_owned();
    let keyed = BTreeMap::from([("x-mako-key".to_owned(), public_key)]);
    let mut on_domain = keyed.clone();
    on_domain.insert("x-mako-custom-domain".to_owned(), HOSTNAME.to_owned());
    let credentials =
        json!({ "email": "dom@app.test", "password": "correct horse battery staple" });
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signup"),
        &keyed,
        Some(&credentials),
    );
    assert!((200..300).contains(&status), "signup failed: {body}");
    let domains_path = format!("/v1/projects/{project_id}/domains");
    let ask = |host: &str| -> u16 {
        let (status, _) = request(
            control_port,
            "GET",
            &format!("/_internal/v1/custom-domains/ask?domain={host}"),
            &BTreeMap::new(),
            None,
        );
        status
    };
    let sign_in_on_domain = || -> u16 {
        let (status, _) = request(
            data_port,
            "POST",
            &format!("{scope}/auth/signin"),
            &on_domain,
            Some(&credentials),
        );
        status
    };

    // --- Names the platform will not take. ----------------------------------
    for hostname in ["localhost", "127.0.0.1", "not a host", "single"] {
        let (status, body) = request(
            control_port,
            "POST",
            &domains_path,
            &manage(&format!("bad-{}", hostname.len())),
            Some(&json!({ "hostname": hostname, "environmentId": environment_id })),
        );
        assert_eq!(status, 400, "{hostname} must be refused: {body}");
    }

    // --- The developer adds the domain and gets the record to publish. ------
    let (status, body) = request(
        control_port,
        "POST",
        &domains_path,
        &manage("add"),
        Some(&json!({ "hostname": HOSTNAME, "environmentId": environment_id })),
    );
    assert_eq!(status, 201, "adding the domain failed: {body}");
    let domain: Value = serde_json::from_str(&body).expect("domain json");
    let domain_id = domain["id"].as_str().expect("domain id").to_owned();
    assert_eq!(domain["state"], "pending");
    assert_eq!(domain["hostname"], HOSTNAME);
    assert_eq!(domain["verification"]["recordType"], "TXT");
    assert_eq!(
        domain["verification"]["recordName"],
        format!("_mako-verify.{HOSTNAME}")
    );
    let record_value = domain["verification"]["recordValue"]
        .as_str()
        .expect("record value")
        .to_owned();
    assert!(record_value.starts_with("mako-domain-verify="));
    // The same name cannot be claimed twice.
    let (status, body) = request(
        control_port,
        "POST",
        &domains_path,
        &manage("add-again"),
        Some(&json!({ "hostname": HOSTNAME, "environmentId": environment_id })),
    );
    assert_eq!(status, 409, "a claimed hostname is refused: {body}");

    // --- Unproven, the domain is neither certified nor served. --------------
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{domains_path}/{domain_id}/actions/verify"),
        &manage("verify-1"),
        None,
    );
    assert_eq!(status, 200, "verify failed: {body}");
    let checked: Value = serde_json::from_str(&body).expect("domain json");
    assert_eq!(checked["state"], "pending");
    assert_eq!(checked["lastError"], "record_missing");
    assert!(checked["lastCheckedAt"].as_str().is_some());
    assert_eq!(ask(HOSTNAME), 404, "no certificate before verification");
    assert_eq!(
        sign_in_on_domain(),
        404,
        "the application API is not served on an unverified name"
    );

    // --- The record is published; the domain verifies and is served. --------
    dns.set_txt(
        &format!("_mako-verify.{HOSTNAME}"),
        &[record_value.as_str()],
    );
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{domains_path}/{domain_id}/actions/verify"),
        &manage("verify-2"),
        None,
    );
    assert_eq!(status, 200, "verify failed: {body}");
    let verified: Value = serde_json::from_str(&body).expect("domain json");
    assert_eq!(verified["state"], "verified", "{body}");
    assert!(verified["verifiedAt"].as_str().is_some());
    assert!(verified["lastError"].is_null());
    assert_eq!(ask(HOSTNAME), 200, "a verified name may be certified");
    assert_eq!(ask("other.example.test"), 404);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = sign_in_on_domain();
        if status == 200 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the application API never answered on the verified name (last {status})"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    // Another project's name is still refused on this tenant.
    let mut foreign = keyed.clone();
    foreign.insert(
        "x-mako-custom-domain".to_owned(),
        "other.example.test".to_owned(),
    );
    let (status, _) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signin"),
        &foreign,
        Some(&credentials),
    );
    assert_eq!(
        status, 404,
        "a name not installed for the tenant is refused"
    );

    // --- A browser on an allowlisted origin may call the API. --------------
    // The allowlist belongs to the environment, so it answers on the
    // platform's own hostname and on the verified domain alike.
    const APP_ORIGIN: &str = "http://127.0.0.1:5173";
    const OTHER_ORIGIN: &str = "http://127.0.0.1:5174";
    let origins_path = format!("{scope}/allowed-origins");
    let put_origins = |key: &str, origins: Value| -> (u16, String) {
        request(
            control_port,
            "PUT",
            &origins_path,
            &manage(key),
            Some(&json!({ "allowedOrigins": origins })),
        )
    };
    // The header set is the platform's, not a header a caller may choose.
    let cross_origin = |method: &str, path: &str, origin: Option<&str>, on_domain: bool| {
        let mut headers = keyed.clone();
        if on_domain {
            headers.insert("x-mako-custom-domain".to_owned(), HOSTNAME.to_owned());
        }
        if let Some(origin) = origin {
            headers.insert("origin".to_owned(), origin.to_owned());
            if method == "OPTIONS" {
                headers.insert(
                    "access-control-request-method".to_owned(),
                    "POST".to_owned(),
                );
            }
        }
        let body = (method == "POST").then_some(&credentials);
        try_request_full(data_port, method, path, &headers, body).expect("request completes")
    };
    let signin = format!("{scope}/auth/signin");
    let labelled = |headers: &BTreeMap<String, String>| {
        headers
            .keys()
            .any(|name| name.starts_with("access-control-"))
    };

    let (status, body) = request(control_port, "GET", &origins_path, &bearer, None);
    assert_eq!(status, 200, "reading the allowlist failed: {body}");
    let listed: Value = serde_json::from_str(&body).expect("origins json");
    assert_eq!(
        listed["allowedOrigins"],
        json!([]),
        "an environment allows no origin until one is set"
    );
    // Before an origin is listed nothing is labelled and no preflight is
    // answered: the sign-in route has no `OPTIONS`.
    let (status, headers, _) = cross_origin("OPTIONS", &signin, Some(APP_ORIGIN), false);
    assert_eq!(
        status, 405,
        "an unanswered preflight routes as it always did"
    );
    assert!(!labelled(&headers), "{headers:?}");

    for malformed in [
        json!(["http://app.example.test"]),
        json!(["https://app.example.test/"]),
        json!(["app.example.test"]),
        json!(["https://App.Example.test/path"]),
    ] {
        let (status, body) = put_origins(
            &format!("origins-bad-{}", malformed.to_string().len()),
            malformed.clone(),
        );
        assert_eq!(status, 400, "{malformed} must be refused: {body}");
    }

    let (status, body) = put_origins("origins-add", json!([APP_ORIGIN]));
    assert_eq!(status, 200, "setting the allowlist failed: {body}");
    let updated: Value = serde_json::from_str(&body).expect("origins json");
    assert_eq!(updated["allowedOrigins"], json!([APP_ORIGIN]));

    // On the platform hostname: the preflight is answered and the request
    // that follows it is labelled.
    let (status, headers, body) = cross_origin("OPTIONS", &signin, Some(APP_ORIGIN), false);
    assert_eq!(
        status, 204,
        "the preflight is answered without routing: {body}"
    );
    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .map(String::as_str),
        Some(APP_ORIGIN)
    );
    assert_eq!(
        headers
            .get("access-control-allow-methods")
            .map(String::as_str),
        Some("GET, POST, PUT, PATCH, DELETE, OPTIONS")
    );
    assert_eq!(
        headers
            .get("access-control-allow-headers")
            .map(String::as_str),
        Some("authorization, content-type, x-mako-key, idempotency-key, if-none-match, if-match")
    );
    assert_eq!(
        headers.get("access-control-max-age").map(String::as_str),
        Some("600")
    );
    assert_eq!(headers.get("vary").map(String::as_str), Some("Origin"));

    let (status, headers, body) = cross_origin("POST", &signin, Some(APP_ORIGIN), false);
    assert_eq!(status, 200, "sign-in failed: {body}");
    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .map(String::as_str),
        Some(APP_ORIGIN)
    );
    assert_eq!(
        headers
            .get("access-control-expose-headers")
            .map(String::as_str),
        Some("etag, x-mako-request-id, content-type")
    );
    assert_eq!(headers.get("vary").map(String::as_str), Some("Origin"));

    // The same answers on the environment's verified domain.
    let (status, headers, _) = cross_origin("OPTIONS", &signin, Some(APP_ORIGIN), true);
    assert_eq!(status, 204);
    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .map(String::as_str),
        Some(APP_ORIGIN)
    );
    let (status, headers, body) = cross_origin("POST", &signin, Some(APP_ORIGIN), true);
    assert_eq!(status, 200, "sign-in on the domain failed: {body}");
    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .map(String::as_str),
        Some(APP_ORIGIN)
    );

    // An origin the environment does not list learns nothing.
    for on_domain in [false, true] {
        let (status, headers, _) = cross_origin("OPTIONS", &signin, Some(OTHER_ORIGIN), on_domain);
        assert_eq!(
            status, 405,
            "an unlisted origin's preflight is not answered"
        );
        assert!(!labelled(&headers), "{headers:?}");
        let (status, headers, _) = cross_origin("POST", &signin, Some(OTHER_ORIGIN), on_domain);
        assert_eq!(
            status, 200,
            "the request itself is not refused by the platform"
        );
        assert!(
            !labelled(&headers),
            "an unlisted origin receives no cross-origin header: {headers:?}"
        );
    }

    // The management API is never answered cross-origin, whatever the
    // environment allows for its application API.
    let mut managed = manage("origins-cors");
    managed.insert("origin".to_owned(), APP_ORIGIN.to_owned());
    let (status, headers, _) =
        try_request_full(control_port, "GET", &origins_path, &managed, None).expect("read");
    assert_eq!(status, 200);
    assert!(
        !labelled(&headers),
        "the management API never emits cross-origin headers: {headers:?}"
    );

    // Clearing the list withdraws cross-origin access at once.
    let (status, body) = put_origins("origins-clear", json!([]));
    assert_eq!(status, 200, "clearing the allowlist failed: {body}");
    let cleared: Value = serde_json::from_str(&body).expect("origins json");
    assert_eq!(cleared["allowedOrigins"], json!([]));
    for on_domain in [false, true] {
        let (_, headers, _) = cross_origin("POST", &signin, Some(APP_ORIGIN), on_domain);
        assert!(
            !labelled(&headers),
            "an emptied allowlist ends cross-origin access: {headers:?}"
        );
    }

    // --- The record disappears; re-verification fails and serving stops. ----
    dns.clear(&format!("_mako-verify.{HOSTNAME}"));
    let mut last: Value = Value::Null;
    for attempt in 0..2 {
        let (status, body) = request(
            control_port,
            "POST",
            &format!("{domains_path}/{domain_id}/actions/verify"),
            &manage(&format!("verify-fail-{attempt}")),
            None,
        );
        assert_eq!(status, 200, "verify failed: {body}");
        last = serde_json::from_str(&body).expect("domain json");
    }
    assert_eq!(
        last["state"], "failed",
        "two misses fail the domain: {last}"
    );
    assert_eq!(last["lastError"], "record_missing");
    assert!(
        last["verifiedAt"].as_str().is_some(),
        "the record of when it was verified stays"
    );
    assert_eq!(ask(HOSTNAME), 404, "a failed name is no longer certified");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = sign_in_on_domain();
        if status == 404 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "serving did not stop on the failed name (last {status})"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    let (status, body) = request(control_port, "GET", &domains_path, &bearer, None);
    assert_eq!(status, 200);
    let listed: Value = serde_json::from_str(&body).expect("list json");
    assert_eq!(listed["items"][0]["state"], "failed");

    // --- Removing the domain ends it. ----------------------------------------
    let (status, body) = request(
        control_port,
        "DELETE",
        &format!("{domains_path}/{domain_id}"),
        &manage("delete"),
        None,
    );
    assert_eq!(status, 204, "removal failed: {body}");
    let (status, body) = request(control_port, "GET", &domains_path, &bearer, None);
    assert_eq!(status, 200);
    let listed: Value = serde_json::from_str(&body).expect("list json");
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(0));
    assert_eq!(ask(HOSTNAME), 404);
}
