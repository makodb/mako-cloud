//! Rational, over HTTP, against a real stack.
//!
//! The sample application has three suites of its own — pure functions, every
//! screen against an in-browser fake, and the screens again against a local
//! stack — and none of them can run where the hosted qualification runs: they
//! need a browser. This one needs nothing but the two services, so the beta
//! can be asked the same question the developer's laptop is asked, which is
//! whether the application's own model works on the platform it is deployed
//! to.
//!
//! It builds the project from `examples/rational/mako/` — the very files the
//! bootstrap publishes and the app derives its RxDB schemas from — so a model
//! that stops working is a failure here rather than a surprise in a browser.
//! Then it walks the household's life: three ways in, sharing by claim,
//! an import, a rule, a receipt, an alert that leaves by webhook, and a push
//! that arrives after the device was away.
//!
//! What it deliberately does not cover is the edge functions, which need the
//! pinned runtime; `edge_function.rs` covers that path, and Rational's own
//! live suite covers its functions against it.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use hmac::{Hmac, Mac};
use mako_smoke::{
    CapturedDelivery, ObjectStoreStub, OidcProviderStub, SmtpCaptureStub, WebhookSinkStub,
    await_readiness, binary_directory, free_ports, mint_developer_session, raw_request, request,
    run_bootstrap, scratch_root, service_environment, start_service, try_request_full,
};
use serde_json::{Value, json};
use sha2::Sha256;

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const APP_ORIGIN: &str = "https://rational.example";
const APP_REDIRECT: &str = "https://rational.example/callback";
const OWNER_EMAIL: &str = "owner@rational.test";
const OWNER_PASSWORD: &str = "RationalSmoke1!";
const EDITOR_EMAIL: &str = "editor@rational.test";
const EDITOR_PASSWORD: &str = "RationalSmoke2!";
const OUTSIDER_EMAIL: &str = "outsider@rational.test";
const OUTSIDER_PASSWORD: &str = "RationalSmoke3!";
const HOUSEHOLD: &str = "hh_smoke";

/// Rational's own model directory, read rather than restated.
fn model_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/rational/mako")
        .canonicalize()
        .expect("examples/rational/mako is part of the workspace")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("reading {} failed: {error}", path.display());
    }))
    .unwrap_or_else(|error| panic!("{} is not json: {error}", path.display()))
}

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
fn rationals_own_model_serves_a_household_over_http() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-rational-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let store = ObjectStoreStub::start();
    let provider = OidcProviderStub::start("rational-client", "subject-rational", "mia@app.test");
    let relay = SmtpCaptureStub::start();
    let sink = WebhookSinkStub::start(0);

    let mut environment = service_environment(root);
    environment.insert(
        "MAKO_OBJECT_STORE_ENDPOINT".to_owned(),
        store.endpoint.clone(),
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
    // Application mail leaves through the same relay as developer mail, and
    // that relay is only configured when developer mail is.
    for (name, value) in [
        ("MAKO_DEVELOPER_REGISTRATION_ENABLED", "true"),
        ("MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME", "127.0.0.1"),
        ("MAKO_DEVELOPER_SMTP_PORT", &relay.port.to_string()),
        ("MAKO_DEVELOPER_SMTP_TLS_MODE", "plaintext"),
        (
            "MAKO_DEVELOPER_SMTP_SENDER",
            "Rational Smoke <no-reply@smoke.local>",
        ),
        (
            "MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF",
            "env:MAKO_SMOKE_MAIL_KEY",
        ),
        (
            "MAKO_SMOKE_MAIL_KEY",
            "rational-smoke-mail-encryption-secret",
        ),
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
    // A service that refuses to start says why in its own log and nowhere
    // else, and the workspace is a temporary directory that is about to be
    // gone. "Did not become ready" is not a diagnosis; the log is.
    let ready = |port: u16, component: &str, log: &str| {
        if std::panic::catch_unwind(|| await_readiness(port, component)).is_err() {
            panic!(
                "{component} never became ready; its log said:\n{}",
                fs::read_to_string(root.join(log)).unwrap_or_default()
            );
        }
    };
    ready(data_port, "mako-data-plane", "data-plane.log");
    ready(control_port, "mako-control-plane", "control-plane.log");
    let session =
        mint_developer_session(&binaries, root, control_port, DEVELOPER_ID, DEVELOPER_EMAIL);

    let bearer = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let manage = |suffix: &str| -> BTreeMap<String, String> {
        let mut headers = bearer.clone();
        headers.insert(
            "idempotency-key".to_owned(),
            format!("rational-smoke-{suffix}"),
        );
        headers
    };
    let created = |path: &str, suffix: &str, body: Option<&Value>| -> Value {
        let (status, response) = request(control_port, "POST", path, &manage(suffix), body);
        assert!(
            (200..300).contains(&status),
            "POST {path} failed with {status}: {response}"
        );
        serde_json::from_str(&response).unwrap_or_else(|_| json!({}))
    };

    // --- The developer publishes Rational's model. --------------------------

    let project = created(
        "/v1/projects",
        "project",
        Some(&json!({ "name": "Rational", "region": "local" })),
    );
    let project_id = project["id"].as_str().expect("project id").to_owned();
    await_active(control_port, &bearer, &format!("/v1/projects/{project_id}"));
    let environment_record = created(
        &format!("/v1/projects/{project_id}/environments"),
        "environment",
        Some(&json!({ "name": "development" })),
    );
    let environment_id = environment_record["id"]
        .as_str()
        .expect("environment id")
        .to_owned();
    let scope = format!("/v1/projects/{project_id}/environments/{environment_id}");
    await_active(control_port, &bearer, &scope);

    let model = read_json(&model_root().join("collections.json"));
    let schema_version = model["schemaVersion"].as_u64().expect("schema version");
    let collections = model["collections"].as_array().expect("collections");
    assert!(
        collections.len() >= 12,
        "the model publishes every collection the app opens"
    );
    for collection in collections {
        let id = collection["id"].as_str().expect("collection id");
        created(
            &format!("{scope}/collections"),
            &format!("collection-{id}"),
            Some(&json!({
                "id": id,
                "schemaVersion": schema_version,
                "jsonSchema": collection["jsonSchema"],
                "primaryKey": collection["primaryKey"],
            })),
        );
        for index in collection["indexes"].as_array().expect("indexes") {
            let name = index["name"].as_str().expect("index name");
            created(
                &format!("{scope}/collections/{id}/indexes"),
                &format!("index-{id}-{name}"),
                Some(&json!({
                    "name": name,
                    "version": 1,
                    "kind": "non_unique",
                    "fields": index["fields"]
                        .as_array()
                        .expect("index fields")
                        .iter()
                        .map(|field| json!({ "path": field, "direction": "ascending" }))
                        .collect::<Vec<_>>(),
                })),
            );
        }
        let policy = read_json(&model_root().join("policies").join(format!("{id}.json")));
        created(
            &format!("{scope}/collections/{id}/policies"),
            &format!("policy-{id}"),
            Some(&policy),
        );
        let version = policy["version"].as_u64().expect("policy version");
        let (status, body) = request(
            control_port,
            "POST",
            &format!("{scope}/collections/{id}/policies/{version}/actions/activate"),
            &manage(&format!("activate-{id}")),
            None,
        );
        assert_eq!(status, 200, "activating {id}'s policy failed: {body}");
    }

    let bucket = read_json(&model_root().join("buckets/receipts.json"));
    created(
        &format!("{scope}/storage-buckets"),
        "bucket",
        Some(&json!({
            "id": bucket["id"],
            "access": bucket["access"],
            "maxObjectBytes": bucket["maxObjectBytes"],
            "allowedContentTypes": bucket["allowedContentTypes"],
            "rules": bucket["rules"],
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
        Some(&json!({ "id": "key_rationalsmoke" })),
    );
    let public_key = issued["value"].as_str().expect("public key").to_owned();
    let (status, body) = request(
        control_port,
        "PUT",
        &format!("{scope}/allowed-origins"),
        &manage("origins"),
        Some(&json!({ "allowedOrigins": [APP_ORIGIN] })),
    );
    assert_eq!(status, 200, "the app's origin was not allowed: {body}");

    // --- Three ways in. -----------------------------------------------------

    let keyed = BTreeMap::from([("x-mako-key".to_owned(), public_key.clone())]);
    let sign_up = |email: &str, password: &str| {
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/auth/signup"),
            &keyed,
            Some(&json!({ "email": email, "password": password })),
        );
        assert!(
            (200..300).contains(&status),
            "sign-up for {email} failed: {body}"
        );
    };
    // Sign-up only reports that it was accepted; who the person is comes from
    // signing in, and so does the token that carries whatever claims they hold
    // at that moment.
    let sign_in = |email: &str, password: &str| -> (String, String) {
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/auth/signin"),
            &keyed,
            Some(&json!({ "email": email, "password": password })),
        );
        assert!(
            (200..300).contains(&status),
            "sign-in for {email} failed: {body}"
        );
        let session: Value = serde_json::from_str(&body).expect("session json");
        (
            session["accessToken"]
                .as_str()
                .expect("access token")
                .to_owned(),
            session["user"]["id"].as_str().expect("user id").to_owned(),
        )
    };

    sign_up(OWNER_EMAIL, OWNER_PASSWORD);
    sign_up(EDITOR_EMAIL, EDITOR_PASSWORD);
    sign_up(OUTSIDER_EMAIL, OUTSIDER_PASSWORD);
    let (_, owner_id) = sign_in(OWNER_EMAIL, OWNER_PASSWORD);
    let (_, editor_id) = sign_in(EDITOR_EMAIL, EDITOR_PASSWORD);

    let (status, body) = request(
        control_port,
        "PUT",
        &format!("{scope}/auth-settings"),
        &manage("auth-settings"),
        Some(&json!({
            "providers": [{
                "name": "stub",
                "kind": { "type": "oidc", "issuer": provider.endpoint },
                "clientId": provider.client_id,
                "clientSecret": "rational-client-secret",
                "scopes": ["openid", "email", "profile"],
                "enabled": true,
            }],
            "redirectUrls": [APP_REDIRECT],
            "magicLinks": { "enabled": true, "linkTtlSeconds": 900 },
        })),
    );
    assert_eq!(status, 200, "sign-in settings were not accepted: {body}");

    // A provider round trip, as the sign-in screen's provider button does it.
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/providers/stub/start"),
        &keyed,
        Some(&json!({ "redirectUrl": APP_REDIRECT })),
    );
    assert_eq!(status, 200, "provider start failed: {body}");
    let started: Value = serde_json::from_str(&body).expect("start json");
    let authorization_url = started["authorizationUrl"]
        .as_str()
        .expect("url")
        .to_owned();
    let state = parameter(&authorization_url, '?', "state").expect("state");
    let nonce = parameter(&authorization_url, '?', "nonce").expect("nonce");
    provider.expect_nonce(&nonce);
    let (status, headers, body) = try_request_full(
        data_port,
        "GET",
        &format!(
            "{scope}/auth/providers/stub/callback?code=rational-code&state={}",
            state.replace('+', "%2B")
        ),
        &BTreeMap::new(),
        None,
    )
    .expect("callback completes");
    assert_eq!(status, 302, "the callback did not redirect: {body}");
    let location = headers.get("location").expect("redirect location");
    let code = parameter(location, '#', "code").expect("one-time code");
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/providers/exchange"),
        &keyed,
        Some(&json!({ "code": code })),
    );
    assert_eq!(status, 200, "provider exchange failed: {body}");
    let provider_session: Value = serde_json::from_str(&body).expect("session json");
    assert_eq!(provider_session["user"]["email"], provider.email);

    // A magic link, as the sign-in screen's other button does it.
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/magic-link"),
        &keyed,
        Some(&json!({ "email": "lee@rational.test", "redirectUrl": APP_REDIRECT })),
    );
    assert_eq!(status, 202, "magic link request failed: {body}");
    let mail = relay
        .wait_for("lee@rational.test", Duration::from_secs(120))
        .expect("the magic link mail reaches the relay");
    let text = mail.text();
    let link = text
        .split_whitespace()
        .find(|word| word.contains("#magic_link_token="))
        .unwrap_or_else(|| panic!("the mail carries the link: {text}"));
    let token = parameter(link, '#', "magic_link_token").expect("magic link token");
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/magic-link/redeem"),
        &keyed,
        Some(&json!({ "token": token })),
    );
    assert_eq!(status, 200, "magic link redemption failed: {body}");

    // --- Sharing: the claim is what a policy reads, and only trusted code
    //     writes one. Rational's `households` function does this; here the
    //     service credential it holds does it directly, which is the same
    //     write through the same route.
    let credential = created(
        &format!("{scope}/credentials/service"),
        "service-key",
        Some(&json!({
            "id": "sk_rational_smoke",
            "scope": {
                "collections": ["users", "households", "memberships", "alerts"],
                "operations": ["create", "read", "update"],
            },
        })),
    );
    let service_key = credential["value"]
        .as_str()
        .expect("service key")
        .to_owned();
    let service_headers = |request_id: &str| -> BTreeMap<String, String> {
        BTreeMap::from([
            ("x-mako-service-key".to_owned(), service_key.clone()),
            (
                "x-mako-bypass-reason".to_owned(),
                "rational smoke acts as the households function".to_owned(),
            ),
            ("x-mako-request-id".to_owned(), request_id.to_owned()),
        ])
    };
    let set_role = |user_id: &str, role: &str, request_id: &str| {
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/service/users/{user_id}/app-metadata"),
            &service_headers(request_id),
            Some(&json!({
                "reason": "rational smoke sets a household role",
                "appMetadata": { "households": { HOUSEHOLD: role } },
            })),
        );
        assert!(
            (200..300).contains(&status),
            "setting {user_id}'s role failed: {body}"
        );
    };
    set_role(&owner_id, "owner", "req_rationalsmokeowner0000000001");
    set_role(&editor_id, "editor", "req_rationalsmokeeditor000000001");

    // The claim only reaches a token that was issued after it was written.
    let (owner, _) = sign_in(OWNER_EMAIL, OWNER_PASSWORD);
    let (editor, _) = sign_in(EDITOR_EMAIL, EDITOR_PASSWORD);
    let (outsider, _) = sign_in(OUTSIDER_EMAIL, OUTSIDER_PASSWORD);

    let app = |token: &str| -> BTreeMap<String, String> {
        let mut headers = keyed.clone();
        headers.insert("authorization".to_owned(), format!("Bearer {token}"));
        headers
    };
    let push = |token: &str, collection: &str, key: &str, rows: Value| -> (u16, String) {
        let mut headers = app(token);
        headers.insert(
            "idempotency-key".to_owned(),
            format!("rational-smoke-push-{key}"),
        );
        request(
            data_port,
            "POST",
            &format!("{scope}/collections/{collection}/replication/push"),
            &headers,
            Some(&json!({ "schemaVersion": schema_version, "rows": rows })),
        )
    };
    let pull = |token: &str, collection: &str| -> Value {
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/collections/{collection}/replication/pull"),
            &app(token),
            Some(&json!({ "schemaVersion": schema_version, "batchSize": 100 })),
        );
        assert!(
            (200..300).contains(&status),
            "pull of {collection} failed: {body}"
        );
        serde_json::from_str(&body).expect("pull json")
    };

    let stamp = 1_700_000_000_000_u64;
    let household = json!({
        "id": HOUSEHOLD,
        "household_id": HOUSEHOLD,
        "created_at": stamp,
        "updated_at": stamp,
        "name": "Smoke household",
        "currency": "USD",
        "owner_id": owner_id,
    });
    let (status, body) = push(
        &owner,
        "households",
        "household",
        json!([{ "mutationId": "rational-household-0001", "newDocumentState": household }]),
    );
    assert!(
        (200..300).contains(&status),
        "the owner could not create the household: {body}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("push json")["outcomes"][0]["status"],
        "accepted"
    );

    let account = json!({
        "id": "acct_smoke",
        "household_id": HOUSEHOLD,
        "created_at": stamp,
        "updated_at": stamp,
        "name": "Everyday",
        "type": "checking",
        "currency": "USD",
        "opening_balance": 500_000,
        "opening_date": "2026-01-01",
    });
    let (status, body) = push(
        &owner,
        "accounts",
        "account",
        json!([{ "mutationId": "rational-account-0001", "newDocumentState": account }]),
    );
    assert!((200..300).contains(&status), "account push failed: {body}");

    // An outsider holds no claim for this household, so the policy denies the
    // write and the read alike -- and says nothing about what is there.
    let (status, body) = push(
        &outsider,
        "accounts",
        "outsider-account",
        json!([{ "mutationId": "rational-outsider-0001", "newDocumentState": {
            "id": "acct_outsider", "household_id": HOUSEHOLD, "created_at": stamp,
            "updated_at": stamp, "name": "Not theirs", "type": "checking",
            "currency": "USD", "opening_balance": 0, "opening_date": "2026-01-01",
        }}]),
    );
    let denied: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
    assert!(
        !(200..300).contains(&status) || denied["outcomes"][0]["status"] != "accepted",
        "an outsider's write into a household must be denied: {status} {body}"
    );
    let outsider_view = pull(&outsider, "accounts");
    assert_eq!(
        outsider_view["documents"].as_array().map(Vec::len),
        Some(0),
        "an outsider reads none of the household's accounts: {outsider_view}"
    );

    // --- An import: many transactions in one push, as the CSV screen sends. --

    let batch: Vec<Value> = (1_i64..=25)
        .map(|index| {
            json!({
                "mutationId": format!("rational-import-{index:04}"),
                "newDocumentState": {
                    "id": format!("txn_import_{index:04}"),
                    "household_id": HOUSEHOLD,
                    "created_at": stamp,
                    "updated_at": stamp + u64::try_from(index).expect("positive"),
                    "account_id": "acct_smoke",
                    "date": "2026-08-01",
                    "amount": -(1_000 + index),
                    "currency": "USD",
                    "description": format!("BLUE BOTTLE #{index}"),
                    "tags": [],
                    "splits": [],
                    "import_batch_id": "imp_smoke",
                },
            })
        })
        .collect();
    let (status, body) = push(&owner, "transactions", "import", json!(batch));
    assert!((200..300).contains(&status), "the import failed: {body}");
    let pushed: Value = serde_json::from_str(&body).expect("push json");
    let accepted = pushed["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .filter(|outcome| outcome["status"] == "accepted")
        .count();
    assert_eq!(accepted, 25, "every imported row was accepted: {body}");

    // --- A rule, and the transaction it filed, which says which rule it was. -

    let (status, body) = push(
        &owner,
        "rules",
        "rule",
        json!([{ "mutationId": "rational-rule-0001", "newDocumentState": {
            "id": "rule_coffee", "household_id": HOUSEHOLD, "created_at": stamp,
            "updated_at": stamp, "name": "Coffee shops",
            "match": { "description_contains": "blue bottle" },
            "set_category_id": "cat_coffee", "add_tags": [], "priority": 10, "enabled": true,
        }}]),
    );
    assert!((200..300).contains(&status), "the rule push failed: {body}");
    // The device knows what it last saw, and says so -- the master state it
    // pulled, revision and all. A push without that is a create, and the row
    // it means to change is already there.
    let imported = latest(&pull(&owner, "transactions"), "txn_import_0001")
        .expect("the imported transaction")
        .clone();
    let mut filed = json!({
        "id": "txn_import_0001",
        "household_id": HOUSEHOLD,
        "created_at": stamp,
        "updated_at": stamp + 1_000,
        "account_id": "acct_smoke",
        "date": "2026-08-01",
        "amount": -1_001,
        "currency": "USD",
        "description": "BLUE BOTTLE #1",
        "tags": [],
        "splits": [],
        "import_batch_id": "imp_smoke",
        "category_id": "cat_coffee",
        "rule_id": "rule_coffee",
    });
    let (status, body) = push(
        &owner,
        "transactions",
        "filed",
        json!([{
            "mutationId": "rational-filed-0001",
            "assumedMasterState": imported,
            "newDocumentState": filed,
        }]),
    );
    assert!((200..300).contains(&status), "the filing failed: {body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("push json")["outcomes"][0]["status"],
        "accepted",
        "the filing was not accepted: {body}"
    );
    let filed_back = pull(&editor, "transactions");
    let carried = latest(&filed_back, "txn_import_0001")
        .expect("the editor sees the household's transactions");
    assert_eq!(
        carried["rule_id"], "rule_coffee",
        "a category chosen by a rule records the rule: {carried}"
    );
    // What the device holds now, revision and all, is what its next push will
    // claim to have seen.
    let before_offline = carried.clone();
    filed["updated_at"] = json!(stamp + 2_000);
    filed["notes"] = json!("edited while the device was away");

    // --- A receipt: the attribute names the household, the claim decides. ----

    let receipt = format!(
        "{scope}/storage/receipts/objects/households/{HOUSEHOLD}/transactions/txn_import_0001/receipt.png"
    );
    let mut uploading = app(&owner);
    uploading.insert("content-type".to_owned(), "image/png".to_owned());
    uploading.insert(
        "x-mako-object-attributes".to_owned(),
        format!("household_id={HOUSEHOLD}"),
    );
    let (status, body, _) = raw_request(data_port, "PUT", &receipt, &uploading, b"\x89PNG receipt");
    assert_eq!(
        status,
        200,
        "the owner could not attach a receipt: {}",
        String::from_utf8_lossy(&body)
    );
    let (status, body, _) = raw_request(data_port, "GET", &receipt, &app(&editor), b"");
    assert_eq!(status, 200, "another member reads the receipt");
    assert_eq!(body, b"\x89PNG receipt");
    let (status, body, _) = raw_request(data_port, "GET", &receipt, &app(&outsider), b"");
    assert_eq!(status, 403, "an outsider does not");
    assert!(
        !body.windows(7).any(|window| window == b"receipt"),
        "and no bytes leave on the refusal"
    );

    // --- An alert: written by trusted code, read by the household, and
    //     delivered once, signed, to the endpoint the developer registered.

    let registered = created(
        &format!("{scope}/webhooks"),
        "webhook",
        Some(&json!({
            "url": sink.url,
            "description": "Rational alerts",
            "subscriptions": [{ "collectionId": "alerts", "events": ["insert"] }],
        })),
    );
    let secret = registered["signingSecret"]
        .as_str()
        .expect("the signing secret is shown once")
        .to_owned();

    let alert_id = format!("alr_{HOUSEHOLD}.large.txn_import_0001");
    let (status, body) = request(
        data_port,
        "POST",
        &format!(
            "{scope}/service/collections/alerts/documents/{}",
            alert_id.replace('.', "%2E")
        ),
        &{
            let mut headers = service_headers("req_rationalsmokealert0000000001");
            headers.insert(
                "idempotency-key".to_owned(),
                "rational-alert-000000000001".to_owned(),
            );
            headers
        },
        Some(&json!({
            "mutationId": "rational-alert-000000000001",
            "operation": "create",
            "expectedRevision": null,
            "schemaVersion": schema_version,
            "body": {
                "id": alert_id,
                "household_id": HOUSEHOLD,
                "created_at": stamp,
                "updated_at": stamp,
                "kind": "alert",
                "alert_kind": "large_transaction",
                "fired_at": stamp,
                "message": "BLUE BOTTLE #1 on 2026-08-01",
                "amount": -1_001,
                "currency": "USD",
                "read": false,
                "transaction_id": "txn_import_0001",
                "account_id": "acct_smoke",
            },
        })),
    );
    assert!(
        (200..300).contains(&status),
        "the alert was not written: {status} {body}"
    );

    let alerts = pull(&owner, "alerts");
    assert!(
        alerts["documents"]
            .as_array()
            .expect("documents")
            .iter()
            .any(|document| document["id"] == alert_id.as_str()),
        "the household sees the alert nobody on a device decided: {alerts}"
    );

    let deliveries = sink.wait_for(1, Duration::from_secs(120));
    let delivered: Vec<&CapturedDelivery> = deliveries
        .iter()
        .filter(|delivery| {
            serde_json::from_str::<Value>(&delivery.body)
                .map(|body| body["documentId"] == alert_id.as_str())
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        delivered.len(),
        1,
        "one alert is one delivery: {:?}",
        deliveries.iter().map(|d| &d.body).collect::<Vec<_>>()
    );
    verify_signature(delivered[0], &secret);
    assert!(
        !delivered[0].body.contains("BLUE BOTTLE"),
        "a delivery names what changed, never what it holds: {}",
        delivered[0].body
    );

    // --- A device that was away pushes what it queued, and finds the rest. ---

    let (status, body) = push(
        &owner,
        "transactions",
        "offline",
        json!([
            { "mutationId": "rational-offline-0001", "newDocumentState": {
                "id": "txn_offline_1", "household_id": HOUSEHOLD, "created_at": stamp,
                "updated_at": stamp + 3_000, "account_id": "acct_smoke", "date": "2026-08-02",
                "amount": -4_200, "currency": "USD", "description": "PAID WHILE OFFLINE",
                "tags": [], "splits": [],
            }},
            {
                "mutationId": "rational-offline-0002",
                "assumedMasterState": before_offline,
                "newDocumentState": filed,
            },
        ]),
    );
    assert!(
        (200..300).contains(&status),
        "the queued writes were not pushed: {body}"
    );
    let queued: Value = serde_json::from_str(&body).expect("push json");
    for outcome in queued["outcomes"].as_array().expect("outcomes") {
        assert_eq!(
            outcome["status"], "accepted",
            "a queued write was refused on reconnect: {body}"
        );
    }
    let after = pull(&editor, "transactions");
    let queued_document =
        latest(&after, "txn_offline_1").expect("the other device sees what was written away");
    assert_eq!(queued_document["description"], "PAID WHILE OFFLINE");
    assert_eq!(
        latest(&after, "txn_import_0001").map(|document| document["notes"].clone()),
        Some(json!("edited while the device was away")),
        "and the edit the device queued against a row it already had"
    );
    let ids = distinct_ids(&after);
    assert_eq!(
        ids.len(),
        26,
        "twenty-five imported and one written offline, however many times each was written: {ids:?}"
    );
}

/// The newest row a pull carries for one document.
///
/// A pull walks the change log from the checkpoint it was given, so a document
/// written twice arrives twice, oldest first. The last one is the state.
fn latest<'a>(page: &'a Value, id: &str) -> Option<&'a Value> {
    page["documents"]
        .as_array()?
        .iter()
        .rfind(|document| document["id"] == id)
}

/// Every distinct document a pull carries, newest state per id.
fn distinct_ids(page: &Value) -> std::collections::BTreeSet<String> {
    page["documents"]
        .as_array()
        .map(|documents| {
            documents
                .iter()
                .filter_map(|document| document["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// A parameter from a URL's query (`?`) or fragment (`#`).
fn parameter(url: &str, separator: char, name: &str) -> Option<String> {
    let (_, rest) = url.split_once(separator)?;
    rest.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
