//! Prove applications can store files next to their documents, under policy.
//!
//! A developer creates a bucket through the management API; an application
//! user uploads, downloads, lists, and deletes objects through the data plane
//! under the bucket's rules; another user is refused without a byte sent; a
//! path that would escape the bucket is refused; a public bucket serves reads
//! to anyone; and the developer sees counts, lists objects, and removes the
//! bucket only after confirming the loss of what it holds.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread::sleep;
use std::time::{Duration, Instant};

use mako_smoke::{
    ObjectStoreStub, await_readiness, binary_directory, free_ports, mint_developer_session,
    request, run_bootstrap, scratch_root, service_environment, start_service,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";

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
        sleep(Duration::from_millis(250));
    }
}

/// A raw HTTP request with an arbitrary body and content type, which the
/// harness's JSON helper cannot send.
fn raw_request(
    port: u16,
    method: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> (u16, Vec<u8>, BTreeMap<String, String>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("read timeout");
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(body).expect("write body");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response head");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .expect("status");
    let response_headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    (status, raw[split + 4..].to_vec(), response_headers)
}

#[test]
fn applications_store_files_under_policy_and_developers_govern_the_buckets() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-files-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let object_store = ObjectStoreStub::start();
    let mut environment = service_environment(root);
    environment.insert(
        "MAKO_OBJECT_STORE_ENDPOINT".to_owned(),
        object_store.endpoint.clone(),
    );
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
        headers.insert(
            "idempotency-key".to_owned(),
            format!("file-storage-smoke-{key}"),
        );
        headers
    };

    // --- A project, an environment, keys, and two application users. -------
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/projects",
        &manage("project"),
        Some(&json!({ "name": "Files", "region": "local" })),
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
        Some(&json!({ "id": "key_files0001" })),
    );
    assert!((200..300).contains(&status), "public key failed: {body}");
    let issued: Value = serde_json::from_str(&body).expect("key json");
    let public_key = issued["value"].as_str().expect("key value").to_owned();
    let keyed = BTreeMap::from([("x-mako-key".to_owned(), public_key)]);
    let sign_in = |email: &str| -> String {
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/auth/signup"),
            &keyed,
            Some(&json!({ "email": email, "password": "correct horse battery staple" })),
        );
        assert!(
            (200..300).contains(&status),
            "signup {email} failed: {body}"
        );
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/auth/signin"),
            &keyed,
            Some(&json!({ "email": email, "password": "correct horse battery staple" })),
        );
        assert!(
            (200..300).contains(&status),
            "signin {email} failed: {body}"
        );
        let session: Value = serde_json::from_str(&body).expect("session json");
        session["accessToken"]
            .as_str()
            .expect("access token")
            .to_owned()
    };
    let alice = sign_in("alice@app.test");
    let bob = sign_in("bob@app.test");
    let app = |token: &str, content_type: Option<&str>| -> BTreeMap<String, String> {
        let mut headers = BTreeMap::from([("authorization".to_owned(), format!("Bearer {token}"))]);
        if let Some(content_type) = content_type {
            headers.insert("content-type".to_owned(), content_type.to_owned());
        }
        headers
    };

    // --- The developer creates a bucket whose rules give owners their files. -
    let rules = json!([
        { "id": "owner-creates", "effect": "allow", "operations": ["create"], "expression": "new.owner_id == identity.user_id" },
        { "id": "owner-changes", "effect": "allow", "operations": ["update"], "expression": "old.owner_id == identity.user_id && new.owner_id == identity.user_id" },
        { "id": "owner-reads-deletes", "effect": "allow", "operations": ["read", "delete"], "expression": "old.owner_id == identity.user_id" }
    ]);
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/storage-buckets"),
        &manage("bucket"),
        Some(
            &json!({ "id": "attachments", "access": "policy", "maxObjectBytes": 4096, "allowedContentTypes": ["text/*", "image/png"], "rules": rules }),
        ),
    );
    assert_eq!(status, 201, "bucket creation failed: {body}");
    let bucket: Value = serde_json::from_str(&body).expect("bucket json");
    assert_eq!(bucket["id"], "attachments");
    assert_eq!(bucket["objectCount"], 0);

    // --- Alice uploads and reads her file; Bob and nobody cannot. -------------
    let object = format!("{scope}/storage/attachments/objects/notes/alice/todo.txt");
    let (status, body, _) = raw_request(
        data_port,
        "PUT",
        &object,
        &app(&alice, Some("text/plain")),
        b"buy milk",
    );
    assert_eq!(
        status,
        200,
        "upload failed: {}",
        String::from_utf8_lossy(&body)
    );
    let record: Value = serde_json::from_slice(&body).expect("object json");
    assert_eq!(record["path"], "notes/alice/todo.txt");
    assert_eq!(record["sizeBytes"], 8);
    assert_eq!(record["contentType"], "text/plain");
    let (status, body, headers) = raw_request(data_port, "GET", &object, &app(&alice, None), b"");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(body, b"buy milk");
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("text/plain")
    );
    let (status, body, _) = raw_request(data_port, "GET", &object, &app(&bob, None), b"");
    assert_eq!(
        status,
        403,
        "another user is refused: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        !body.windows(8).any(|window| window == b"buy milk"),
        "no bytes leave on a refusal"
    );
    let (status, _, _) = raw_request(data_port, "GET", &object, &BTreeMap::new(), b"");
    assert_eq!(status, 401, "a policy bucket needs a credential");
    let (status, _, _) = raw_request(data_port, "DELETE", &object, &app(&bob, None), b"");
    assert_eq!(status, 403);

    // --- Limits and paths fail closed. -----------------------------------------
    let (status, _, _) = raw_request(
        data_port,
        "PUT",
        &format!("{scope}/storage/attachments/objects/big.txt"),
        &app(&alice, Some("text/plain")),
        &[b'x'; 4097],
    );
    assert_eq!(status, 413, "over the bucket's size");
    let (status, _, _) = raw_request(
        data_port,
        "PUT",
        &format!("{scope}/storage/attachments/objects/app.bin"),
        &app(&alice, Some("application/octet-stream")),
        b"x",
    );
    assert_eq!(status, 400, "content type not allowed");
    let (status, _, _) = raw_request(
        data_port,
        "PUT",
        &format!("{scope}/storage/attachments/objects/..%2Fescape.txt"),
        &app(&alice, Some("text/plain")),
        b"x",
    );
    assert_eq!(status, 400, "a path escaping the bucket is refused");
    let (status, _, _) = raw_request(
        data_port,
        "PUT",
        &format!("{scope}/storage/attachments/objects/a/../b.txt"),
        &app(&alice, Some("text/plain")),
        b"x",
    );
    assert_eq!(status, 400);
    let (status, _, _) = raw_request(
        data_port,
        "PUT",
        &format!("{scope}/storage/missing-bucket/objects/x.txt"),
        &app(&alice, Some("text/plain")),
        b"x",
    );
    assert_eq!(status, 404);

    // --- Listings show only what the caller may read. ---------------------------
    let (status, body, _) = raw_request(
        data_port,
        "PUT",
        &format!("{scope}/storage/attachments/objects/notes/bob/todo.txt"),
        &app(&bob, Some("text/plain")),
        b"walk dog",
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let (status, body, _) = raw_request(
        data_port,
        "GET",
        &format!("{scope}/storage/attachments/objects?prefix=notes%2F"),
        &app(&alice, None),
        b"",
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let page: Value = serde_json::from_slice(&body).expect("page json");
    let paths: Vec<&str> = page["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["path"].as_str().expect("path"))
        .collect();
    assert_eq!(
        paths,
        ["notes/alice/todo.txt"],
        "bob's file stays out of alice's listing"
    );

    // --- The developer sees the bucket's totals and every object. ---------------
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/storage-buckets/attachments"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "{body}");
    let inspected: Value = serde_json::from_str(&body).expect("bucket json");
    assert_eq!(inspected["objectCount"], 2);
    assert_eq!(inspected["totalBytes"], 16);
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/storage-buckets/attachments/objects"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "{body}");
    let listed: Value = serde_json::from_str(&body).expect("objects json");
    assert_eq!(
        listed["items"].as_array().expect("items").len(),
        2,
        "the developer lists every object"
    );
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/storage-buckets"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "{body}");
    let buckets: Value = serde_json::from_str(&body).expect("buckets json");
    assert_eq!(buckets["items"][0]["id"], "attachments");
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/workspace/summary"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "{body}");
    let summary: Value = serde_json::from_str(&body).expect("summary json");
    let inventory = &summary["sections"]["backups"]["payload"]["objectStorage"];
    assert_eq!(
        inventory["bucketCount"], 1,
        "the backup inventory counts buckets: {summary}"
    );
    assert_eq!(inventory["objectCount"], 2);
    assert_eq!(inventory["totalBytes"], 16);

    // --- A public bucket serves reads to anyone, writes only under policy. -------
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/storage-buckets"),
        &manage("public-bucket"),
        Some(
            &json!({ "id": "public-assets", "access": "public", "maxObjectBytes": 4096, "rules": rules }),
        ),
    );
    assert_eq!(status, 201, "public bucket creation failed: {body}");
    let logo = format!("{scope}/storage/public-assets/objects/logo.png");
    let (status, _, _) = raw_request(
        data_port,
        "PUT",
        &logo,
        &app(&alice, Some("image/png")),
        b"PNG",
    );
    assert_eq!(status, 200);
    let (status, body, _) = raw_request(data_port, "GET", &logo, &BTreeMap::new(), b"");
    assert_eq!(status, 200, "anyone reads a public bucket");
    assert_eq!(body, b"PNG");
    let (status, _, _) = raw_request(
        data_port,
        "PUT",
        &logo,
        &BTreeMap::from([("content-type".to_owned(), "image/png".to_owned())]),
        b"X",
    );
    assert_eq!(status, 401, "nobody writes without a credential");

    // --- Alice deletes her file; the developer removes buckets with confirmation. -
    let (status, _, _) = raw_request(data_port, "DELETE", &object, &app(&alice, None), b"");
    assert_eq!(status, 200);
    let (status, _, _) = raw_request(data_port, "GET", &object, &app(&alice, None), b"");
    assert_eq!(status, 404);
    let mut confirmed = bearer.clone();
    confirmed.insert("confirmation".to_owned(), "delete:attachments".to_owned());
    let (status, body) = request(
        control_port,
        "DELETE",
        &format!("{scope}/storage-buckets/attachments"),
        &confirmed,
        None,
    );
    assert_eq!(
        status, 409,
        "a bucket with objects is not deleted without confirming their loss: {body}"
    );
    let (status, body) = request(
        control_port,
        "DELETE",
        &format!("{scope}/storage-buckets/attachments?deleteObjects=true"),
        &confirmed,
        None,
    );
    assert_eq!(status, 200, "{body}");
    let removed: Value = serde_json::from_str(&body).expect("removal json");
    assert_eq!(
        removed["objectCount"], 1,
        "bob's remaining file went with the bucket"
    );
    let (status, _) = request(
        control_port,
        "GET",
        &format!("{scope}/storage-buckets/attachments"),
        &bearer,
        None,
    );
    assert_eq!(status, 404);
}
