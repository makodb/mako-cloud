//! Prove that document changes reach a registered endpoint, signed, durably.
//!
//! A developer registers an endpoint subscribed to a collection; the
//! endpoint is down for the first requests and every change from that
//! window arrives once it recovers, in order per document, each signed
//! with the secret shown once at registration and carrying no document
//! fields. The delivery log shows the retries; a redelivery is a new signed
//! delivery logged as such; a rotated secret signs what follows; removing
//! the endpoint ends it.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use mako_smoke::{
    CapturedDelivery, WebhookSinkStub, await_readiness, binary_directory, free_ports,
    mint_developer_session, request, run_bootstrap, scratch_root, service_environment,
    start_service,
};
use serde_json::{Value, json};
use sha2::Sha256;

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const COLLECTION_ID: &str = "orders";

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

fn verify_signature(delivery: &CapturedDelivery, secret: &str) {
    let signature = delivery
        .headers
        .get("x-mako-signature")
        .expect("every delivery is signed");
    let mut timestamp = None;
    let mut digest = None;
    for part in signature.split(',') {
        if let Some(value) = part.trim().strip_prefix("t=") {
            timestamp = Some(value.to_owned());
        } else if let Some(value) = part.trim().strip_prefix("v1=") {
            digest = Some(value.to_owned());
        }
    }
    let (timestamp, digest) = (timestamp.expect("t="), digest.expect("v1="));
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(format!("{timestamp}.{}", delivery.body).as_bytes());
    let expected: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(digest, expected, "signature must verify under the secret");
}

fn delivery_json(delivery: &CapturedDelivery) -> Value {
    serde_json::from_str(&delivery.body).expect("delivery body is json")
}

#[test]
fn document_changes_reach_a_registered_endpoint_signed_and_durably() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-webhooks-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    // Down for the first two requests, up after.
    let sink = WebhookSinkStub::start(2);
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
        headers.insert(
            "idempotency-key".to_owned(),
            format!("webhooks-smoke-{key}"),
        );
        headers
    };
    let created = |path: &str, key: &str, body: Option<&Value>| -> Value {
        let (status, text) = request(control_port, "POST", path, &manage(key), body);
        assert!(
            (200..300).contains(&status),
            "POST {path} failed: {status} {text}"
        );
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };

    // --- A project, an environment, a collection, a key, a user. -----------
    let project = created(
        "/v1/projects",
        "project",
        Some(&json!({ "name": "Hooks", "region": "local" })),
    );
    let project_id = project["id"].as_str().expect("project id").to_owned();
    await_active(control_port, &bearer, &format!("/v1/projects/{project_id}"));
    let environment_record = created(
        &format!("/v1/projects/{project_id}/environments"),
        "environment",
        Some(&json!({ "name": "production" })),
    );
    let environment_id = environment_record["id"]
        .as_str()
        .expect("environment id")
        .to_owned();
    let scope = format!("/v1/projects/{project_id}/environments/{environment_id}");
    await_active(control_port, &bearer, &scope);
    created(
        &format!("{scope}/collections"),
        "collection",
        Some(&json!({
            "id": COLLECTION_ID,
            "schemaVersion": 1,
            "jsonSchema": {
                "type": "object",
                "required": ["id", "total", "updatedAt"],
                "properties": {
                    "id": { "type": "string" },
                    "total": { "type": "integer" },
                    "updatedAt": { "type": "integer" },
                },
                "additionalProperties": true,
            },
            "primaryKey": { "kind": "field", "field": "id" },
        })),
    );
    created(
        &format!("{scope}/signing-keys/actions/initialize"),
        "signing",
        None,
    );
    let issued = created(
        &format!("{scope}/credentials/public"),
        "public-key",
        Some(&json!({ "id": "key_webhooks001" })),
    );
    let public_key = issued["value"].as_str().expect("key value").to_owned();
    created(
        &format!("{scope}/collections/{COLLECTION_ID}/policies"),
        "policy",
        Some(&json!({
            "version": 1,
            "rules": [{ "id": "all", "effect": "allow", "operations": ["create", "read", "update", "delete"], "expression": "true" }],
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
    let keyed = BTreeMap::from([("x-mako-key".to_owned(), public_key)]);
    let credentials =
        json!({ "email": "clerk@app.test", "password": "correct horse battery staple" });
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signup"),
        &keyed,
        Some(&credentials),
    );
    assert!((200..300).contains(&status), "signup failed: {body}");
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signin"),
        &keyed,
        Some(&credentials),
    );
    assert!((200..300).contains(&status), "signin failed: {body}");
    let session: Value = serde_json::from_str(&body).expect("session json");
    let access = session["accessToken"]
        .as_str()
        .expect("access token")
        .to_owned();
    let mut replicating = keyed.clone();
    replicating.insert("authorization".to_owned(), format!("Bearer {access}"));
    let push = |key: &str, rows: Value| {
        let mut headers = replicating.clone();
        headers.insert("idempotency-key".to_owned(), format!("webhooks-push-{key}"));
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/collections/{COLLECTION_ID}/replication/push"),
            &headers,
            Some(&json!({ "schemaVersion": 1, "rows": rows })),
        );
        assert!((200..300).contains(&status), "push {key} failed: {body}");
        let pushed: Value = serde_json::from_str(&body).expect("push json");
        for outcome in pushed["outcomes"].as_array().expect("outcomes") {
            assert_eq!(
                outcome["status"], "accepted",
                "push {key} not accepted: {body}"
            );
        }
    };
    let pull = || -> Vec<Value> {
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/collections/{COLLECTION_ID}/replication/pull"),
            &replicating,
            Some(&json!({ "schemaVersion": 1, "batchSize": 50 })),
        );
        assert!((200..300).contains(&status), "pull failed: {body}");
        let pulled: Value = serde_json::from_str(&body).expect("pull json");
        pulled["documents"].as_array().cloned().unwrap_or_default()
    };

    // A write before registration is never delivered.
    push(
        "before",
        json!([{ "mutationId": "hooks-mutation-000000", "newDocumentState": { "id": "order-0", "total": 1, "updatedAt": 1 } }]),
    );

    // --- The developer registers the endpoint; the secret is shown once. ----
    let registered = created(
        &format!("{scope}/webhooks"),
        "endpoint",
        Some(&json!({
            "url": sink.url,
            "description": "order pipeline",
            "subscriptions": [{ "collectionId": COLLECTION_ID, "events": ["insert", "update", "delete"] }],
        })),
    );
    let endpoint_id = registered["endpoint"]["id"]
        .as_str()
        .expect("endpoint id")
        .to_owned();
    let secret = registered["signingSecret"]
        .as_str()
        .expect("secret shown once")
        .to_owned();
    assert_eq!(registered["endpoint"]["state"], "active");
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/webhooks"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "listing endpoints failed: {body}");
    assert!(
        !body.contains(&secret),
        "the signing secret must never be listed: {body}"
    );

    // --- Changes while the endpoint is down, then after it recovers. -------
    push(
        "insert-a",
        json!([{ "mutationId": "hooks-mutation-000001", "newDocumentState": { "id": "order-a", "total": 10, "updatedAt": 2 } }]),
    );
    let order_a = pull()
        .into_iter()
        .find(|doc| doc["id"] == "order-a")
        .expect("order-a is pulled back");
    push(
        "update-a",
        json!([{ "mutationId": "hooks-mutation-000002", "assumedMasterState": order_a, "newDocumentState": { "id": "order-a", "total": 12, "updatedAt": 3 } }]),
    );
    push(
        "insert-b",
        json!([{ "mutationId": "hooks-mutation-000003", "newDocumentState": { "id": "order-b", "total": 5, "updatedAt": 4 } }]),
    );

    let deliveries = sink.wait_for(3, Duration::from_secs(120));
    assert_eq!(
        deliveries.len(),
        3,
        "every change from the outage window is delivered after recovery: {deliveries:?}"
    );
    assert!(sink.refused() >= 2, "the endpoint was down first");
    let events: Vec<(String, String)> = deliveries
        .iter()
        .map(|delivery| {
            let body = delivery_json(delivery);
            (
                body["documentId"].as_str().unwrap_or("").to_owned(),
                body["event"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    let position = |document: &str, event: &str| {
        events
            .iter()
            .position(|(d, e)| d == document && e == event)
            .unwrap_or_else(|| panic!("{document} {event} delivered: {events:?}"))
    };
    assert!(
        position("order-a", "insert") < position("order-a", "update"),
        "in order per document: {events:?}"
    );
    position("order-b", "insert");
    assert!(
        !events.iter().any(|(document, _)| document == "order-0"),
        "a change before registration is not delivered: {events:?}"
    );
    for delivery in &deliveries {
        verify_signature(delivery, &secret);
        assert_eq!(
            delivery
                .headers
                .get("x-mako-webhook-id")
                .map(String::as_str),
            Some(endpoint_id.as_str())
        );
        assert!(delivery.headers.contains_key("x-mako-delivery-id"));
        let body = delivery_json(delivery);
        assert_eq!(body["collection"], COLLECTION_ID);
        assert_eq!(body["projectId"], project_id);
        assert_eq!(body["environmentId"], environment_id);
        assert!(body["revision"].as_str().is_some_and(|r| !r.is_empty()));
        assert!(body["commitPosition"].as_u64().is_some());
        assert!(
            body.get("total").is_none() && body.get("document").is_none(),
            "a delivery carries no document fields: {body}"
        );
    }

    // --- The log shows the retries. ------------------------------------------
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/webhooks/{endpoint_id}/deliveries?limit=50"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "delivery log failed: {body}");
    let log: Value = serde_json::from_str(&body).expect("log json");
    let items = log["items"].as_array().expect("items");
    assert_eq!(items.len(), 3, "three deliveries logged: {body}");
    assert!(
        items.iter().all(|item| item["state"] == "delivered"),
        "{body}"
    );
    assert!(
        items
            .iter()
            .any(|item| item["attempts"].as_u64().unwrap_or(0) >= 2),
        "the retries are on the log: {body}"
    );
    assert!(items.iter().all(|item| item["lastResponseStatus"] == 200));

    // --- A redelivery is a new signed delivery, logged as such. -------------
    let original = items
        .iter()
        .find(|item| item["documentId"] == "order-b")
        .expect("order-b delivery")["id"]
        .as_str()
        .expect("delivery id")
        .to_owned();
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/webhooks/{endpoint_id}/deliveries/{original}/actions/redeliver"),
        &manage("redeliver"),
        None,
    );
    assert_eq!(status, 202, "redelivery failed: {body}");
    let redelivery: Value = serde_json::from_str(&body).expect("redelivery json");
    assert_eq!(redelivery["redeliveryOf"], original);
    let deliveries = sink.wait_for(4, Duration::from_secs(60));
    assert_eq!(
        deliveries.len(),
        4,
        "the redelivery arrives: {deliveries:?}"
    );
    let redelivered = delivery_json(&deliveries[3]);
    assert_eq!(redelivered["documentId"], "order-b");
    assert_eq!(redelivered["event"], "insert");
    assert_eq!(redelivered["redeliveryOf"], original);
    verify_signature(&deliveries[3], &secret);

    // --- A rotated secret signs what follows. --------------------------------
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/webhooks/{endpoint_id}/actions/rotate-secret"),
        &manage("rotate"),
        None,
    );
    assert_eq!(status, 200, "rotation failed: {body}");
    let rotated: Value = serde_json::from_str(&body).expect("rotation json");
    let new_secret = rotated["signingSecret"]
        .as_str()
        .expect("new secret")
        .to_owned();
    assert_ne!(new_secret, secret);
    assert_eq!(rotated["endpoint"]["secretVersion"], 2);
    push(
        "insert-c",
        json!([{ "mutationId": "hooks-mutation-000004", "newDocumentState": { "id": "order-c", "total": 7, "updatedAt": 5 } }]),
    );
    let deliveries = sink.wait_for(5, Duration::from_secs(60));
    assert_eq!(deliveries.len(), 5, "the change after rotation arrives");
    assert_eq!(delivery_json(&deliveries[4])["documentId"], "order-c");
    assert_eq!(
        deliveries[4]
            .headers
            .get("x-mako-secret-version")
            .map(String::as_str),
        Some("2")
    );
    verify_signature(&deliveries[4], &new_secret);

    // --- Removing the endpoint ends it. --------------------------------------
    let (status, body) = request(
        control_port,
        "DELETE",
        &format!("{scope}/webhooks/{endpoint_id}"),
        &manage("delete"),
        None,
    );
    assert_eq!(status, 204, "removal failed: {body}");
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/webhooks"),
        &bearer,
        None,
    );
    assert_eq!(status, 200);
    let listed: Value = serde_json::from_str(&body).expect("list json");
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(0));
    push(
        "insert-d",
        json!([{ "mutationId": "hooks-mutation-000005", "newDocumentState": { "id": "order-d", "total": 1, "updatedAt": 6 } }]),
    );
    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(
        sink.deliveries().len(),
        5,
        "nothing is delivered after removal"
    );
}
