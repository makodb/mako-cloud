use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
};

use futures::executor::block_on;
use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KvAdapter, RocksDbAdapter, RocksDbConfig,
    TenantKeyspace, WriteBatch,
};
use serde_json::json;

use crate::{
    CONTROL_DATA_REQUEST_BYTES, CONTROL_DATA_RESPONSE_BYTES, DeploymentKey,
    EncryptedResponseJournal, GuardDecision, InternalAuthError, InternalCaller,
    InternalClientError, InternalHttpClient, InternalHttpClientConfig, InternalReplayGuard,
    InternalRequestAuthenticator, InternalRoute, MAX_INTERNAL_BODY_BYTES, PreparedResponseJournal,
    ResponseJournalError, ResponseJournalLookup, ResponseJournalStoreOutcome,
    RocksInternalReplayGuard,
};

const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn slow_bulk_responses_outlive_interactive_timeout_without_changing_other_calls() {
    use crate::{
        ControlToDataClient, IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission,
        InternalClientError,
    };
    use std::{collections::BTreeSet, time::Duration};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
                assert!(headers.len() < 32 * 1024);
            }
            let headers = String::from_utf8(headers).unwrap();
            let header = |name: &str| {
                headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case(name).then(|| value.trim())
                    })
                    .unwrap()
            };
            let mut body = vec![0; header("content-length").parse::<usize>().unwrap()];
            stream.read_exact(&mut body).unwrap();
            let command: IdentityAdminCommand = serde_json::from_slice(&body).unwrap();
            assert_eq!(command.actor_id, "system/data-job-worker");
            thread::sleep(Duration::from_millis(250));
            // The first caller has timed out, so a broken pipe is expected.
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nx-mako-request-id: {}\r\nConnection: close\r\n\r\n{{\"ok\":true}}",
                header("x-mako-request-id")
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    let mut config = InternalHttpClientConfig::loopback(endpoint);
    config.io_timeout = Duration::from_millis(50);
    let client = ControlToDataClient::new(
        InternalHttpClient::new(
            config,
            DeploymentKey::derive(SECRET).unwrap(),
            InternalCaller::ControlPlane,
        )
        .unwrap(),
    )
    .unwrap();
    let tenant = tenant("prj_abcdefgh", "env_abcdefgh");
    for (index, operation) in [
        IdentityAdminOperation::ListProjectCredentials,
        IdentityAdminOperation::ImportDataJobBatch,
        IdentityAdminOperation::ExportDataJobPage,
    ]
    .into_iter()
    .enumerate()
    {
        let command = IdentityAdminCommand {
            operation,
            actor_id: "system/data-job-worker".into(),
            permissions: BTreeSet::from([IdentityAdminPermission::ExecuteDataJobs]),
            input: json!({}),
        };
        let result = client.administer::<serde_json::Value>(
            &tenant,
            &format!("req_budget_{index}"),
            &format!("idem_budget_{index}"),
            &command,
        );
        if index == 0 {
            assert!(matches!(result, Err(InternalClientError::TimedOut)));
            assert!(InternalClientError::TimedOut.is_dependency_unavailable());
        } else {
            assert_eq!(result.unwrap(), json!({"ok":true}));
        }
    }
    server.join().unwrap();
}

#[test]
fn canonical_signature_binds_route_caller_tenant_body_and_time() {
    let key = DeploymentKey::derive(SECRET).expect("deployment key");
    let signer = InternalRequestAuthenticator::new(key.clone(), InternalCaller::ControlPlane);
    let verifier = InternalRequestAuthenticator::new(key, InternalCaller::ControlPlane);
    let tenant = tenant("prj_abcdefgh", "env_abcdefgh");
    let signed = signer
        .sign(
            InternalRoute::IdentityAdmin,
            &tenant,
            "req_abcdefgh",
            "idem_abcdefgh",
            100,
            br#"{"operation":"search_users","actorId":"operator","input":{}}"#.to_vec(),
        )
        .expect("signed request");
    let verified = verifier
        .verify_signed(InternalRoute::IdentityAdmin, &signed, 100)
        .expect("verified request");
    assert_eq!(verified.tenant, tenant);
    assert_eq!(verified.caller, InternalCaller::ControlPlane);

    let mut tampered = signed.clone();
    tampered.body.push(b' ');
    assert_eq!(
        verifier.verify_signed(InternalRoute::IdentityAdmin, &tampered, 100),
        Err(InternalAuthError::Unauthenticated)
    );
    assert_eq!(
        verifier.verify_signed(InternalRoute::IdentityAdmin, &signed, 131),
        Err(InternalAuthError::Expired)
    );
    assert!(
        signer
            .sign(
                InternalRoute::IdentityVerify,
                &tenant,
                "req_abcdefgh",
                "idem_abcdefgh",
                100,
                b"{}".to_vec(),
            )
            .is_err()
    );
}

#[test]
fn response_journal_accepts_large_export_pages_but_keeps_a_finite_bound() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mut config = RocksDbConfig::new(directory.path());
        config.minimum_durability = Durability::Sync;
        let adapter: Arc<dyn KvAdapter> = Arc::new(RocksDbAdapter::open(config).unwrap());
        let tenant = tenant("prj_abcdefgh", "env_abcdefgh");
        let deployment_key = DeploymentKey::derive(SECRET).unwrap();
        let journal =
            EncryptedResponseJournal::new(adapter, &tenant, &tenant, &deployment_key).unwrap();
        let auth = InternalRequestAuthenticator::new(deployment_key, InternalCaller::ControlPlane);
        let signed = auth
            .sign(
                InternalRoute::IdentityAdmin,
                &tenant,
                "req_large_journal",
                "idem_large_journal",
                100,
                b"{}".to_vec(),
            )
            .unwrap();
        let verified = auth
            .verify_signed(InternalRoute::IdentityAdmin, &signed, 100)
            .unwrap();
        let response = vec![b'x'; CONTROL_DATA_RESPONSE_BYTES];
        assert!(matches!(
            journal
                .prepare_atomic(&verified, &response, 100)
                .await
                .unwrap(),
            PreparedResponseJournal::Fresh(_)
        ));
        assert_eq!(
            journal.store(&verified, &response, 100).await.unwrap(),
            ResponseJournalStoreOutcome::Stored
        );
        assert_eq!(
            journal.lookup(&verified, 101).await.unwrap(),
            ResponseJournalLookup::Replay(response)
        );
        let oversized = vec![b'x'; CONTROL_DATA_RESPONSE_BYTES + 1];
        assert!(matches!(
            journal.store(&verified, &oversized, 101).await,
            Err(ResponseJournalError::InvalidResponse)
        ));
        assert!(matches!(
            journal.prepare_atomic(&verified, &oversized, 101).await,
            Err(ResponseJournalError::InvalidResponse)
        ));
    });
}

#[test]
fn response_journal_atomically_replays_exact_encrypted_results_and_expires() {
    block_on(async {
        let directory = tempfile::tempdir().expect("temporary RocksDB");
        let mut rocks_config = RocksDbConfig::new(directory.path());
        rocks_config.minimum_durability = Durability::Sync;
        let adapter: Arc<dyn KvAdapter> =
            Arc::new(RocksDbAdapter::open(rocks_config).expect("RocksDB"));
        let tenant = tenant("prj_abcdefgh", "env_abcdefgh");
        let deployment_key = DeploymentKey::derive(SECRET).expect("deployment key");
        let journal =
            EncryptedResponseJournal::new(Arc::clone(&adapter), &tenant, &tenant, &deployment_key)
                .expect("journal");
        let auth = InternalRequestAuthenticator::new(deployment_key, InternalCaller::ControlPlane);
        let signed = auth
            .sign(
                InternalRoute::IdentityAdmin,
                &tenant,
                "req_journal",
                "idem_journal",
                100,
                br#"{"operation":"create_project_credential"}"#.to_vec(),
            )
            .expect("sign");
        let verified = auth
            .verify_signed(InternalRoute::IdentityAdmin, &signed, 100)
            .expect("verify");
        let response = br#"{"credential":"mako_sk.key.secret"}"#;
        let prepared = journal
            .prepare_atomic(&verified, response, 100)
            .await
            .expect("prepare");
        let PreparedResponseJournal::Fresh(conditional) = prepared else {
            panic!("expected fresh response journal");
        };
        let mut conditions = Vec::new();
        let mut batch = WriteBatch::new();
        batch.put(b"identity-mutation", b"committed");
        conditional.append_to(&mut conditions, &mut batch);
        assert_eq!(
            adapter
                .compare_and_write(AtomicWrite {
                    conditions,
                    batch,
                    durability: Durability::Sync,
                })
                .await
                .expect("commit"),
            CompareAndWriteResult::Applied
        );
        assert_eq!(
            journal.lookup(&verified, 101).await.expect("lookup"),
            ResponseJournalLookup::Replay(response.to_vec())
        );

        let keyspace = TenantKeyspace::new(
            tenant.project_id().as_str(),
            tenant.environment_id().as_str(),
        )
        .expect("keyspace");
        let stored = adapter
            .get(
                &keyspace
                    .internal_rpc_response_key("control-plane", "idem_journal")
                    .expect("journal key"),
            )
            .await
            .expect("storage")
            .expect("journal record");
        assert!(
            !stored
                .windows(response.len())
                .any(|window| window == response)
        );

        let changed = auth
            .sign(
                InternalRoute::IdentityAdmin,
                &tenant,
                "req_changed_journal",
                "idem_journal",
                101,
                br#"{"operation":"rotate_project_credential"}"#.to_vec(),
            )
            .expect("sign changed");
        let changed = auth
            .verify_signed(InternalRoute::IdentityAdmin, &changed, 101)
            .expect("verify changed");
        assert_eq!(
            journal.lookup(&changed, 101).await.expect("changed lookup"),
            ResponseJournalLookup::Conflict
        );
        assert_eq!(
            journal
                .lookup(&verified, 100 + 24 * 60 * 60)
                .await
                .expect("expired lookup"),
            ResponseJournalLookup::Expired
        );
        assert!(
            journal
                .cleanup_expired(&verified, 100 + 24 * 60 * 60)
                .await
                .expect("cleanup")
        );
        assert_eq!(
            journal
                .lookup(&verified, 100 + 24 * 60 * 60)
                .await
                .expect("cleaned lookup"),
            ResponseJournalLookup::Missing
        );
    });
}

#[test]
fn durable_guard_rejects_replay_and_preserves_idempotency() {
    block_on(async {
        let directory = tempfile::tempdir().expect("temporary RocksDB");
        let mut rocks_config = RocksDbConfig::new(directory.path());
        rocks_config.minimum_durability = Durability::Sync;
        let adapter: Arc<dyn KvAdapter> =
            Arc::new(RocksDbAdapter::open(rocks_config).expect("RocksDB"));
        let tenant = tenant("prj_abcdefgh", "env_abcdefgh");
        let guard = RocksInternalReplayGuard::new(Arc::clone(&adapter), &tenant, &tenant)
            .expect("replay guard");
        let auth = InternalRequestAuthenticator::new(
            DeploymentKey::derive(SECRET).expect("deployment key"),
            InternalCaller::ControlPlane,
        );
        let first_signed = auth
            .sign(
                InternalRoute::IdentityAdmin,
                &tenant,
                "req_first",
                "idem_shared",
                100,
                br#"{"operation":"delete_user","actorId":"operator","input":{"id":"usr_a"}}"#
                    .to_vec(),
            )
            .expect("first request");
        let first = auth
            .verify_signed(InternalRoute::IdentityAdmin, &first_signed, 100)
            .expect("verify first");
        assert_eq!(
            guard.claim(&first, 100).await.expect("fresh claim"),
            GuardDecision::Fresh
        );
        assert!(guard.claim(&first, 100).await.is_err());

        let retry_signed = auth
            .sign(
                InternalRoute::IdentityAdmin,
                &tenant,
                "req_retry",
                "idem_shared",
                101,
                br#"{"operation":"delete_user","actorId":"operator","input":{"id":"usr_a"}}"#
                    .to_vec(),
            )
            .expect("retry request");
        let retry = auth
            .verify_signed(InternalRoute::IdentityAdmin, &retry_signed, 101)
            .expect("verify retry");
        assert_eq!(
            guard.claim(&retry, 101).await.expect("duplicate claim"),
            GuardDecision::Duplicate
        );

        let changed_signed = auth
            .sign(
                InternalRoute::IdentityAdmin,
                &tenant,
                "req_changed",
                "idem_shared",
                102,
                br#"{"operation":"restore_user","actorId":"operator","input":{"id":"usr_a"}}"#
                    .to_vec(),
            )
            .expect("changed request");
        let changed = auth
            .verify_signed(InternalRoute::IdentityAdmin, &changed_signed, 102)
            .expect("verify changed");
        assert!(guard.claim(&changed, 102).await.is_err());
    });
}

#[test]
fn identity_administration_admits_a_document_sized_batch_and_other_routes_do_not() {
    let key = DeploymentKey::derive(SECRET).expect("deployment key");
    let signer = InternalRequestAuthenticator::new(key.clone(), InternalCaller::ControlPlane);
    let verifier = InternalRequestAuthenticator::new(key, InternalCaller::ControlPlane);
    let tenant = tenant("prj_abcdefgh", "env_abcdefgh");
    let body = |bytes: usize| {
        serde_json::to_vec(
            &json!({"operation": "import_data_job_batch", "input": "x".repeat(bytes)}),
        )
        .expect("body")
    };
    let batch = signer
        .sign(
            InternalRoute::IdentityAdmin,
            &tenant,
            "req_abcdefgh",
            "idem_abcdefgh",
            100,
            body(3 * 512 * 1024),
        )
        .expect("a batch carrying a 1 MiB document signs");
    verifier
        .verify_signed(InternalRoute::IdentityAdmin, &batch, 100)
        .expect("and verifies");
    assert_eq!(
        signer
            .sign(
                InternalRoute::IdentityAdmin,
                &tenant,
                "req_abcdefgh",
                "idem_abcdefgh",
                100,
                body(CONTROL_DATA_REQUEST_BYTES),
            )
            .err(),
        Some(InternalAuthError::InvalidBody)
    );
    assert_eq!(
        signer
            .sign(
                InternalRoute::ApplicationMailDrain,
                &tenant,
                "req_abcdefgh",
                "idem_abcdefgh",
                100,
                body(MAX_INTERNAL_BODY_BYTES),
            )
            .err(),
        Some(InternalAuthError::InvalidBody)
    );
}

#[test]
fn http_client_is_loopback_only_and_requires_correlated_bounded_responses() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let endpoint = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection");
        let mut request = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            let size = stream.read(&mut chunk).expect("request");
            assert!(size > 0, "client closed an incomplete request");
            request.extend_from_slice(&chunk[..size]);
            let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let head = std::str::from_utf8(&request[..header_end]).expect("UTF-8 headers");
            let content_length = head
                .split("\r\n")
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .expect("content length")
                .parse::<usize>()
                .expect("numeric content length");
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        let text = std::str::from_utf8(&request).expect("UTF-8 request");
        assert!(
            text.starts_with("POST /_internal/v1/data/identity/verify HTTP/1.1\r\n"),
            "unexpected request: {text:?}"
        );
        assert!(text.contains("x-mako-internal-signature:"));
        let request_id = text
            .split("\r\n")
            .find_map(|line| line.strip_prefix("x-mako-request-id: ").map(str::to_owned))
            .expect("request id");
        let body = br#"{"verified":true}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nx-mako-request-id: {}\r\nContent-Type: application/json\r\n\r\n",
            body.len(),
            request_id,
        )
        .expect("response headers");
        stream.write_all(body).expect("response body");
    });
    let client = InternalHttpClient::new(
        InternalHttpClientConfig::loopback(endpoint),
        DeploymentKey::derive(SECRET).expect("deployment key"),
        InternalCaller::EdgeGateway,
    )
    .expect("client");
    let response = client
        .call(
            InternalRoute::IdentityVerify,
            &tenant("prj_abcdefgh", "env_abcdefgh"),
            "req_network",
            "idem_network",
            &json!({"operation":"access_token","presentedCredential":"token"}),
        )
        .expect("response");
    assert_eq!(response.status, 200);
    assert_eq!(response.request_id, "req_network");
    server.join().expect("server");
    // An oversized request is refused before anything is signed or sent, and
    // is not mistaken for an authentication failure.
    assert!(matches!(
        client.call(
            InternalRoute::IdentityVerify,
            &tenant("prj_abcdefgh", "env_abcdefgh"),
            "req_oversize",
            "idem_oversize",
            &json!({"presentedCredential": "x".repeat(MAX_INTERNAL_BODY_BYTES)}),
        ),
        Err(InternalClientError::RequestTooLarge)
    ));

    let public = "192.0.2.1:8080".parse().expect("address");
    assert!(
        InternalHttpClient::new(
            InternalHttpClientConfig::loopback(public),
            DeploymentKey::derive(SECRET).expect("deployment key"),
            InternalCaller::EdgeGateway,
        )
        .is_err()
    );
}

#[test]
fn route_allowlist_is_exact() {
    assert_eq!(
        InternalRoute::from_method_path(
            mako_service_runtime::HttpMethod::Post,
            "/_internal/v1/control/operator-entitlements/plan"
        ),
        Some(InternalRoute::OperatorEntitlementPlan)
    );
    assert_eq!(
        InternalRoute::OperatorEntitlementPlan.caller(),
        InternalCaller::OperatorAdmin
    );
    assert_eq!(
        InternalRoute::OperatorEntitlementApply.caller(),
        InternalCaller::OperatorAdmin
    );
    assert_eq!(
        InternalRoute::from_method_path(
            mako_service_runtime::HttpMethod::Post,
            "/_internal/v1/data/identity/admin"
        ),
        Some(InternalRoute::IdentityAdmin)
    );
    assert_eq!(
        InternalRoute::from_method_path(
            mako_service_runtime::HttpMethod::Get,
            "/_internal/v1/data/identity/admin"
        ),
        None
    );
    assert_eq!(
        InternalRoute::from_method_path(
            mako_service_runtime::HttpMethod::Post,
            "/_internal/v2/data/identity/admin"
        ),
        None
    );
}

fn tenant(project: &str, environment: &str) -> TenantScope {
    TenantScope::require(Some(project), Some(environment)).expect("tenant")
}
