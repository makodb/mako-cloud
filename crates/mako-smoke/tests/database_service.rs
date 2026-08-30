//! Use the database the way an application does.
//!
//! `sample_app` proves a developer can build a backend and that one document
//! survives a push and a pull. That is the replication path. It is not the
//! question "is this database usable" — an application also authenticates
//! users, reads and writes individual documents, queries them with predicates
//! and ordering, and depends on one user's data being unreachable to another.
//!
//! Everything below runs against the real service binaries as an ordinary
//! application user holding nothing but a public project key and a session.

use std::{
    collections::{BTreeMap, BTreeSet},
    thread::sleep,
    time::{Duration, Instant},
};

use mako_smoke::{
    ServiceProcess, await_readiness, binary_directory, free_ports, mint_developer_session,
    read_sse_frames, request, run_bootstrap, scratch_root, service_environment, start_service,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const ORGANIZATION_ID: &str = "org_localboot";
const COLLECTION_ID: &str = "notes";
const SECOND_COLLECTION_ID: &str = "reminders";
const ALICE: &str = "alice@local.test";
const BOB: &str = "bob@local.test";
const PASSWORD: &str = "ApplicationUserPass1!";
const PROVISIONING_TIMEOUT: Duration = Duration::from_secs(90);

struct Services {
    _data: ServiceProcess,
    _control: ServiceProcess,
    data_port: u16,
    control_port: u16,
    session: String,
    _workspace: tempfile::TempDir,
}

#[test]
fn an_application_user_authenticates_reads_writes_and_queries_their_own_data() {
    let services = start();
    let data = services.data_port;
    let (scope, key) = provision(&services);
    let keyed = BTreeMap::from([("x-mako-key".to_owned(), key)]);

    // --- Authentication -------------------------------------------------

    let alice = sign_up(data, &scope, &keyed, ALICE);
    let bob = sign_up(data, &scope, &keyed, BOB);

    // A session identifies who it belongs to.
    let (status, body) = request(
        data,
        "GET",
        &format!("{scope}/auth/user"),
        &bearer(&keyed, &alice.access),
        None,
    );
    assert_eq!(status, 200, "reading the signed-in user failed: {body}");
    let profile: Value = serde_json::from_str(&body).expect("user json");
    assert_eq!(profile["email"], ALICE, "the session named another user");
    let alice_id = profile["id"]
        .as_str()
        .expect("the session names a user id")
        .to_owned();

    // Sessions rotate rather than living forever.
    let (status, body) = request(
        data,
        "POST",
        &format!("{scope}/auth/token"),
        &keyed,
        Some(&json!({ "refreshToken": alice.refresh })),
    );
    assert!(
        (200..300).contains(&status),
        "refreshing the session failed with {status}: {body}"
    );
    let rotated: Value = serde_json::from_str(&body).expect("refresh json");
    let alice_access = rotated["accessToken"]
        .as_str()
        .expect("refresh issues an access token")
        .to_owned();

    // The signing keys an application verifies tokens against are published.
    let (status, body) = request(data, "GET", &format!("{scope}/auth/jwks"), &keyed, None);
    assert_eq!(status, 200, "the project JWKS is not served: {body}");
    let jwks: Value = serde_json::from_str(&body).expect("jwks json");
    assert!(
        jwks["keys"].as_array().is_some_and(|keys| !keys.is_empty()),
        "the project publishes no verification keys: {body}"
    );

    // --- Writing and reading a document ----------------------------------

    let alice_auth = bearer(&keyed, &alice_access);
    let note = format!("{scope}/collections/{COLLECTION_ID}/documents/note-alice-1");
    let (status, body) = request(
        data,
        "POST",
        &note,
        &writing(&keyed, &alice_access, "database-service-create-1"),
        Some(&json!({
            "mutationId": "database-service-create-1",
            "operation": "create",
            "expectedRevision": Value::Null,
            "schemaVersion": 1,
            "body": {
                "id": "note-alice-1",
                "owner_id": alice_id,
                "title": "first note",
                "updated_at": 1,
            },
        })),
    );
    assert!(
        (200..300).contains(&status),
        "creating a document failed with {status}: {body}"
    );
    let created: Value = serde_json::from_str(&body).expect("mutation json");
    // `currentRevision` reports the revision a conflicting write lost to; an
    // applied write carries its new revision on the document itself.
    assert_eq!(
        created["status"], "applied",
        "the write was not applied: {body}"
    );
    let revision = created["document"]["revision"]
        .as_str()
        .unwrap_or_else(|| panic!("a write reports a revision: {body}"))
        .to_owned();

    let (status, body) = request(data, "GET", &note, &alice_auth, None);
    assert_eq!(status, 200, "reading the document back failed: {body}");
    let fetched: Value = serde_json::from_str(&body).expect("document json");
    assert_eq!(
        fetched["body"]["title"], "first note",
        "reading the document back returned {body}"
    );

    // --- Querying ---------------------------------------------------------

    for (index, title) in [(2, "second note"), (3, "third note")] {
        let path = format!("{scope}/collections/{COLLECTION_ID}/documents/note-alice-{index}");
        let (status, body) = request(
            data,
            "POST",
            &path,
            &writing(
                &keyed,
                &alice_access,
                &format!("database-service-create-{index}"),
            ),
            Some(&json!({
                "mutationId": format!("database-service-create-{index}"),
                "operation": "create",
                "expectedRevision": Value::Null,
                "schemaVersion": 1,
                "body": {
                    "id": format!("note-alice-{index}"),
                    "owner_id": alice_id,
                    "title": title,
                    "updated_at": index,
                },
            })),
        );
        assert!(
            (200..300).contains(&status),
            "creating document {index} failed with {status}: {body}"
        );
    }

    let query = format!("{scope}/collections/{COLLECTION_ID}/documents/query");
    let (status, body) = request(
        data,
        "POST",
        &query,
        &alice_auth,
        Some(&json!({
            "predicates": [{ "field": "owner_id", "operator": "eq", "value": alice_id }],
            "sort": [{ "field": "updated_at", "direction": "desc" }],
            "cursor": Value::Null,
            "limit": 2,
        })),
    );
    assert_eq!(status, 200, "querying documents failed: {body}");
    let page: Value = serde_json::from_str(&body).expect("query json");
    let titles: Vec<&str> = page["documents"]
        .as_array()
        .expect("a query returns documents")
        .iter()
        .filter_map(|document| document["body"]["title"].as_str())
        .collect();
    assert_eq!(
        titles,
        vec!["third note", "second note"],
        "the query did not order and limit as asked: {body}"
    );
    assert!(
        page["nextCursor"].as_str().is_some(),
        "a truncated page reported no cursor to continue from: {body}"
    );

    // --- One user's data is not another's ---------------------------------

    let bob_auth = bearer(&keyed, &bob.access);
    let (status, body) = request(data, "GET", &note, &bob_auth, None);
    assert!(
        !(200..300).contains(&status),
        "another user read a document they do not own: {body}"
    );
    let (status, body) = request(
        data,
        "POST",
        &query,
        &bob_auth,
        Some(&json!({
            "predicates": [{ "field": "owner_id", "operator": "eq", "value": alice_id }],
            "sort": [],
            "cursor": Value::Null,
            "limit": 100,
        })),
    );
    let leaked = if (200..300).contains(&status) {
        let page: Value = serde_json::from_str(&body).expect("query json");
        page["documents"].as_array().map_or(0, Vec::len)
    } else {
        0
    };
    assert_eq!(
        leaked, 0,
        "another user's query returned documents they do not own: {body}"
    );

    // --- Updating and deleting --------------------------------------------

    let (status, body) = request(
        data,
        "POST",
        &note,
        &writing(&keyed, &alice_access, "database-service-update-1"),
        Some(&json!({
            "mutationId": "database-service-update-1",
            "operation": "update",
            "expectedRevision": revision,
            "schemaVersion": 1,
            "body": {
                "id": "note-alice-1",
                "owner_id": alice_id,
                "title": "first note, edited",
                "updated_at": 4,
            },
        })),
    );
    assert!(
        (200..300).contains(&status),
        "updating a document failed with {status}: {body}"
    );
    let updated: Value = serde_json::from_str(&body).expect("mutation json");
    let updated_revision = updated["document"]["revision"]
        .as_str()
        .unwrap_or_else(|| panic!("an update reports a revision: {body}"))
        .to_owned();
    assert_ne!(
        updated_revision, revision,
        "an update did not advance the revision"
    );

    // A stale revision must not silently overwrite.
    let (status, body) = request(
        data,
        "POST",
        &note,
        &writing(&keyed, &alice_access, "database-service-update-stale"),
        Some(&json!({
            "mutationId": "database-service-update-stale",
            "operation": "update",
            "expectedRevision": revision,
            "schemaVersion": 1,
            "body": {
                "id": "note-alice-1",
                "owner_id": alice_id,
                "title": "written from a stale read",
                "updated_at": 5,
            },
        })),
    );
    let conflicted = !(200..300).contains(&status)
        || serde_json::from_str::<Value>(&body)
            .ok()
            .is_some_and(|result| result["status"] != "applied");
    assert!(
        conflicted,
        "a write against a stale revision was accepted: {body}"
    );

    let (status, body) = request(
        data,
        "POST",
        &note,
        &writing(&keyed, &alice_access, "database-service-delete-1"),
        Some(&json!({
            "mutationId": "database-service-delete-1",
            "operation": "delete",
            "expectedRevision": updated_revision,
            "schemaVersion": 1,
            // A delete still carries a schema-valid body: the tombstone keeps
            // the last state so replication can hand it to a client.
            "body": {
                "id": "note-alice-1",
                "owner_id": alice_id,
                "title": "first note, edited",
                "updated_at": 4,
            },
        })),
    );
    assert!(
        (200..300).contains(&status),
        "deleting a document failed with {status}: {body}"
    );
    let (status, _) = request(data, "GET", &note, &alice_auth, None);
    assert!(
        !(200..300).contains(&status),
        "a deleted document is still readable"
    );

    // --- One live stream carries every collection --------------------------
    //
    // A browser opens six connections to one host, so an application with a
    // dozen collections cannot have a stream each: the later streams queue
    // behind the earlier ones and the pulls and pushes queue behind those,
    // which looks like a connected client that never syncs. One connection
    // carries them all, each event naming the collection it belongs to.
    let stream_path = format!("{scope}/replication/stream");
    let mut streaming = alice_auth.clone();
    streaming.insert("content-type".to_owned(), "application/json".to_owned());
    let (status, frames) = read_sse_frames(
        data,
        "POST",
        &stream_path,
        &streaming,
        Some(&json!({
            "collections": [
                { "collectionId": COLLECTION_ID, "schemaVersion": 1 },
                { "collectionId": SECOND_COLLECTION_ID, "schemaVersion": 1 },
            ]
        })),
        2,
        Duration::from_secs(20),
    )
    .expect("the multiplexed stream answers");
    assert_eq!(
        status, 200,
        "the multiplexed stream was refused: {frames:?}"
    );
    let named: BTreeSet<String> = frames
        .iter()
        .filter_map(|frame| {
            let data = frame
                .lines()
                .find_map(|line| line.strip_prefix("data:"))?
                .trim();
            let value: Value = serde_json::from_str(data).ok()?;
            value["collection"].as_str().map(str::to_owned)
        })
        .collect();
    assert!(
        named.contains(COLLECTION_ID) && named.contains(SECOND_COLLECTION_ID),
        "one connection carries both collections, each event naming its own: {frames:?}"
    );

    // The request contract is checked before anything streams: a collection
    // this caller cannot reach, or one named twice, is refused rather than
    // quietly dropped from a stream the client believes is complete.
    for body in [
        json!({ "collections": [] }),
        json!({ "collections": [
            { "collectionId": COLLECTION_ID, "schemaVersion": 1 },
            { "collectionId": COLLECTION_ID, "schemaVersion": 1 },
        ] }),
        json!({ "collections": [{ "collectionId": "absent", "schemaVersion": 1 }] }),
    ] {
        let (status, _) = read_sse_frames(
            data,
            "POST",
            &stream_path,
            &streaming,
            Some(&body),
            1,
            Duration::from_secs(5),
        )
        .expect("the stream answers");
        assert!(
            !(200..300).contains(&status),
            "an invalid stream request must be refused, got {status} for {body}"
        );
    }

    // --- Signing out ends the session -------------------------------------

    let (status, body) = request(
        data,
        "POST",
        &format!("{scope}/auth/signout"),
        &alice_auth,
        Some(&json!({ "refreshToken": rotated["refreshToken"] })),
    );
    assert!(
        (200..300).contains(&status),
        "signing out failed with {status}: {body}"
    );
    let (status, _) = request(
        data,
        "POST",
        &query,
        &alice_auth,
        Some(&json!({
            "predicates": [{ "field": "owner_id", "operator": "eq", "value": alice_id }],
            "sort": [],
            "cursor": Value::Null,
            "limit": 10,
        })),
    );
    assert!(
        !(200..300).contains(&status),
        "a signed-out session still reads documents"
    );
}

struct AppUser {
    access: String,
    refresh: String,
}

/// Document writes are idempotent operations and require a key.
/// Document writes are idempotent, and the service requires the key to equal
/// the mutation identifier so a replay cannot be reinterpreted as a different
/// write.
fn writing(
    keyed: &BTreeMap<String, String>,
    token: &str,
    mutation_id: &str,
) -> BTreeMap<String, String> {
    let mut headers = bearer(keyed, token);
    headers.insert("idempotency-key".to_owned(), mutation_id.to_owned());
    headers
}

fn bearer(keyed: &BTreeMap<String, String>, token: &str) -> BTreeMap<String, String> {
    let mut headers = keyed.clone();
    headers.insert("authorization".to_owned(), format!("Bearer {token}"));
    headers
}

fn sign_up(port: u16, scope: &str, keyed: &BTreeMap<String, String>, email: &str) -> AppUser {
    let credentials = json!({ "email": email, "password": PASSWORD });
    let (status, body) = request(
        port,
        "POST",
        &format!("{scope}/auth/signup"),
        keyed,
        Some(&credentials),
    );
    assert!(
        (200..300).contains(&status),
        "sign-up for {email} failed with {status}: {body}"
    );
    let (status, body) = request(
        port,
        "POST",
        &format!("{scope}/auth/signin"),
        keyed,
        Some(&credentials),
    );
    assert!(
        (200..300).contains(&status),
        "sign-in for {email} failed with {status}: {body}"
    );
    let session: Value = serde_json::from_str(&body).expect("session json");
    AppUser {
        access: session["accessToken"]
            .as_str()
            .expect("sign-in issues an access token")
            .to_owned(),
        refresh: session["refreshToken"]
            .as_str()
            .expect("sign-in issues a refresh token")
            .to_owned(),
    }
}

fn start() -> Services {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-database-service-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let environment = service_environment(root);
    run_bootstrap(&binaries, &environment);

    let [data_port, control_port] = free_ports::<2>();
    let data = start_service(
        "mako-data-plane",
        &binaries,
        &environment,
        data_port,
        root.join("data-plane.log"),
    );
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
        _data: data,
        _control: control,
        data_port,
        control_port,
        session,
        _workspace: workspace,
    }
}

/// The developer side: a project with a collection whose policy scopes every
/// document to the user that owns it.
fn provision(services: &Services) -> (String, String) {
    let control = services.control_port;
    let session = &services.session;
    let manage = |suffix: &str| -> BTreeMap<String, String> {
        BTreeMap::from([
            ("authorization".to_owned(), format!("Bearer {session}")),
            ("idempotency-key".to_owned(), format!("db-service-{suffix}")),
        ])
    };

    let project = created(
        control,
        "/v1/projects",
        &manage("project"),
        Some(&json!({
            "teamId": ORGANIZATION_ID,
            "name": "Database Service",
            "region": "local",
        })),
    );
    let project_id = project["id"].as_str().expect("project id").to_owned();
    let environment = created(
        control,
        &format!("/v1/projects/{project_id}/environments"),
        &manage("environment"),
        Some(&json!({ "name": "production" })),
    );
    let environment_id = environment["id"]
        .as_str()
        .expect("environment id")
        .to_owned();
    await_active(control, session, &format!("/v1/projects/{project_id}"));
    await_active(
        control,
        session,
        &format!("/v1/projects/{project_id}/environments/{environment_id}"),
    );
    let scope = format!("/v1/projects/{project_id}/environments/{environment_id}");

    created(
        control,
        &format!("{scope}/collections"),
        &manage("collection"),
        Some(&json!({
            "id": COLLECTION_ID,
            "schemaVersion": 1,
            "jsonSchema": {
                "type": "object",
                "required": ["id", "owner_id", "title", "updated_at"],
                "properties": {
                    "id": { "type": "string" },
                    "owner_id": { "type": "string" },
                    "title": { "type": "string" },
                    "updated_at": { "type": "integer" },
                },
                "additionalProperties": true,
            },
            "primaryKey": { "kind": "field", "field": "id" },
        })),
    );
    // Queries are refused unless an index covers them, so the developer
    // declares one rather than relying on a scan.
    created(
        control,
        &format!("{scope}/collections/{COLLECTION_ID}/indexes"),
        &manage("index"),
        Some(&json!({
            "name": "owner_updated",
            "version": 1,
            "kind": "non_unique",
            "fields": [
                { "path": "owner_id", "direction": "ascending" },
                { "path": "updated_at", "direction": "descending" },
            ],
        })),
    );
    await_index(
        control,
        session,
        &format!("{scope}/collections/{COLLECTION_ID}/indexes/owner_updated/1"),
    );
    // A second index, because one HTTP listing inspects every index it lists
    // and each inspection is its own internal request. Sending them all under
    // the caller's request id made the second look like a replay of the first,
    // so listing a collection with more than one index answered `conflict` --
    // in the console, the CLI, and every setup script. One index could never
    // have shown it.
    created(
        control,
        &format!("{scope}/collections/{COLLECTION_ID}/indexes"),
        &manage("index-second"),
        Some(&json!({
            "name": "title_updated",
            "version": 1,
            "kind": "non_unique",
            "fields": [
                { "path": "title", "direction": "ascending" },
                { "path": "updated_at", "direction": "descending" },
            ],
        })),
    );
    await_index(
        control,
        session,
        &format!("{scope}/collections/{COLLECTION_ID}/indexes/title_updated/1"),
    );
    let (status, body) = request(
        control,
        "GET",
        &format!("{scope}/collections/{COLLECTION_ID}/indexes"),
        &BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]),
        None,
    );
    assert_eq!(status, 200, "listing two indexes failed: {body}");
    let listed: Value = serde_json::from_str(&body).expect("index listing json");
    let names: Vec<&str> = listed["items"]
        .as_array()
        .expect("index items")
        .iter()
        .filter_map(|item| item["name"].as_str())
        .collect();
    assert!(
        names.contains(&"owner_updated") && names.contains(&"title_updated"),
        "both indexes are listed with their state: {body}"
    );

    created(
        control,
        &format!("{scope}/signing-keys/actions/initialize"),
        &manage("signing-key"),
        None,
    );
    let issued = created(
        control,
        &format!("{scope}/credentials/public"),
        &manage("public-key"),
        Some(&json!({ "id": "key_dbservice01" })),
    );
    let key = issued["value"].as_str().expect("public key").to_owned();

    // Ownership, not a blanket allow: this is what makes one user's documents
    // unreachable to another.
    created(
        control,
        &format!("{scope}/collections/{COLLECTION_ID}/policies"),
        &manage("policy"),
        Some(&json!({
            "version": 1,
            "rules": [
                {
                    "id": "owner-creates",
                    "effect": "allow",
                    "operations": ["create"],
                    "expression": "new.owner_id == identity.user_id",
                },
                {
                    "id": "owner-uses",
                    "effect": "allow",
                    "operations": ["read", "update", "delete"],
                    "expression": "old.owner_id == identity.user_id",
                },
            ],
        })),
    );
    let (status, body) = request(
        control,
        "POST",
        &format!("{scope}/collections/{COLLECTION_ID}/policies/1/actions/activate"),
        &manage("activate"),
        None,
    );
    assert_eq!(status, 200, "activating the policy failed: {body}");

    // A second collection, because one stream per collection is what a
    // browser cannot afford: six connections to a host, and an application
    // has a dozen collections. Two is enough to prove one connection carries
    // them and every event says which it belongs to.
    created(
        control,
        &format!("{scope}/collections"),
        &manage("second-collection"),
        Some(&json!({
            "id": SECOND_COLLECTION_ID,
            "schemaVersion": 1,
            "jsonSchema": {
                "type": "object",
                "required": ["id", "owner_id", "title", "updated_at"],
                "properties": {
                    "id": { "type": "string" },
                    "owner_id": { "type": "string" },
                    "title": { "type": "string" },
                    "updated_at": { "type": "integer" },
                },
                "additionalProperties": true,
            },
            "primaryKey": { "kind": "field", "field": "id" },
        })),
    );
    created(
        control,
        &format!("{scope}/collections/{SECOND_COLLECTION_ID}/policies"),
        &manage("second-policy"),
        Some(&json!({
            "version": 1,
            "rules": [
                {
                    "id": "owner-creates",
                    "effect": "allow",
                    "operations": ["create"],
                    "expression": "new.owner_id == identity.user_id",
                },
                {
                    "id": "owner-uses",
                    "effect": "allow",
                    "operations": ["read", "update", "delete"],
                    "expression": "old.owner_id == identity.user_id",
                },
            ],
        })),
    );
    let (status, body) = request(
        control,
        "POST",
        &format!("{scope}/collections/{SECOND_COLLECTION_ID}/policies/1/actions/activate"),
        &manage("second-activate"),
        None,
    );
    assert_eq!(
        status, 200,
        "activating the second collection's policy failed: {body}"
    );

    (scope, key)
}

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

fn await_active(port: u16, session: &str, path: &str) {
    let headers = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let deadline = Instant::now() + PROVISIONING_TIMEOUT;
    let mut last = String::from("no response");
    while Instant::now() < deadline {
        let (status, body) = request(port, "GET", path, &headers, None);
        if status == 200 {
            let record: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            if record["state"] == "active" {
                return;
            }
            last = format!("state {}", record["state"]);
        } else {
            last = format!("status {status}: {body}");
        }
        sleep(Duration::from_millis(500));
    }
    panic!("{path} did not become active within {PROVISIONING_TIMEOUT:?} ({last})");
}

/// An index is built online, so it is not usable the moment it is accepted.
fn await_index(port: u16, session: &str, path: &str) {
    let headers = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let deadline = Instant::now() + PROVISIONING_TIMEOUT;
    let mut last = String::from("no response");
    while Instant::now() < deadline {
        let (status, body) = request(port, "GET", path, &headers, None);
        if status == 200 {
            let index: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            match index["state"].as_str() {
                Some("active") => return,
                Some("failed") => panic!("the index build failed: {body}"),
                other => last = format!("state {other:?}"),
            }
        } else {
            last = format!("status {status}: {body}");
        }
        sleep(Duration::from_millis(300));
    }
    panic!("{path} did not become active within {PROVISIONING_TIMEOUT:?} ({last})");
}
