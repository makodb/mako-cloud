//! The internal route the control plane's scheduler invokes functions
//! through.
//!
//! The gateway is otherwise only a *caller* of internal routes. This one is
//! served here so a scheduled run takes the same path as a public
//! invocation -- route resolution, admission, audit, metrics, the runtime
//! -- rather than a side door into the runtime. The request is verified
//! like every internal request (signed by the deployment key, bound to a
//! tenant, replay-guarded), and the tenant in the body must be the tenant
//! the envelope was signed for.

use std::{collections::BTreeMap, sync::Arc, time::Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{StreamExt, executor::block_on};
use mako_api::{ErrorCode, RetryAdvice};
use mako_edge_gateway::{
    FunctionGateway, FunctionGatewayRequest, FunctionHttpMethod, FunctionInvocationActor,
};
use mako_internal_rpc::{
    FunctionScheduleInvokeRequest, FunctionScheduleInvokeResponse, GuardDecision,
    InternalReplayGuard, InternalRoute, RocksInternalReplayGuardError,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};

use crate::EdgeGatewayGraph;

/// What of the function's response body travels back to the scheduler.
const RESPONSE_PREVIEW_BYTES: usize = 4 * 1024;
const MAXIMUM_IDENTIFIER_BYTES: usize = 64;

pub(crate) fn add_internal_routes(
    router: &mut HttpRouter,
    graph: Arc<EdgeGatewayGraph>,
) -> Result<(), RouteRegistrationError> {
    router.add_route(
        HttpMethod::Post,
        InternalRoute::FunctionScheduleInvoke.path(),
        move |request| handle_schedule_invoke(&graph, &request),
    )
}

fn handle_schedule_invoke(
    graph: &Arc<EdgeGatewayGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let now = now_unix_seconds(request)?;
    block_on(async {
        let verified = graph
            .internal_authenticator()
            .verify(InternalRoute::FunctionScheduleInvoke, request, now)
            .map_err(|error| error.to_http_error(request.request_id()))?;
        let input: FunctionScheduleInvokeRequest = serde_json::from_slice(request.body())
            .map_err(|_| invalid(request, "scheduled invocation input is invalid"))?;
        if input.project_id != verified.tenant.project_id().as_str()
            || input.environment_id != verified.tenant.environment_id().as_str()
        {
            return Err(HttpApiError::new(
                403,
                ErrorCode::PermissionDenied,
                "scheduled invocation tenant does not match the signed tenant",
                request.request_id(),
                RetryAdvice::Never,
            ));
        }
        let guard = graph.internal_replay_guard(&verified.tenant).map_err(|_| {
            unavailable(
                request,
                "scheduled invocation replay protection is unavailable",
            )
        })?;
        if guard
            .claim(&verified, now)
            .await
            .map_err(|error| map_guard_error(request, error))?
            == GuardDecision::Duplicate
        {
            return Err(HttpApiError::new(
                409,
                ErrorCode::Conflict,
                "internal scheduled invocation request was already used",
                request.request_id(),
                RetryAdvice::Never,
            ));
        }
        if !valid_identifier(&input.schedule_id, "sch_")
            || !valid_identifier(&input.run_id, "run_")
            || !input.path.starts_with('/')
        {
            return Err(invalid(request, "scheduled invocation input is invalid"));
        }
        let method = FunctionHttpMethod::parse(&input.method)
            .map_err(|_| invalid(request, "scheduled invocation method is invalid"))?;
        let body = STANDARD
            .decode(&input.body_base64)
            .map_err(|_| invalid(request, "scheduled invocation body is invalid"))?;
        let suffix = if input.path == "/" { "" } else { &input.path };
        let gateway_request = FunctionGatewayRequest {
            request_id: request.request_id().to_owned(),
            method,
            path: format!(
                "/{}--{}/functions/v1/{}{suffix}",
                verified.tenant.project_id(),
                verified.tenant.environment_id(),
                input.function_name
            ),
            query: input.query,
            headers: input.headers.into_iter().collect(),
            body,
            region_priority: vec![graph.region().to_owned()],
            now_unix_seconds: now,
        };
        let actor = FunctionInvocationActor::Schedule {
            schedule_id: input.schedule_id,
            run_id: input.run_id,
        };
        let started = Instant::now();
        let response = match FunctionGateway
            .invoke_as(
                gateway_request,
                actor,
                &graph.routes,
                &graph.admission,
                graph.audit.as_ref(),
                &graph.metrics,
                &graph.runtime,
            )
            .await
        {
            Ok(response) => response,
            // The gateway's own refusal crosses as it is: the scheduler
            // records it as the run's error reason.
            Err(error) => {
                return HttpResponse::json(error.status(), error.api_error())
                    .map_err(|_| internal(request, "function error response is unavailable"));
            }
        };
        // The whole body is read so the duration covers the function's
        // complete answer; only a preview of it travels back.
        let mut preview = Vec::with_capacity(RESPONSE_PREVIEW_BYTES);
        let mut stream = response.body;
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| unavailable(request, "function response stream failed"))?;
            let room = RESPONSE_PREVIEW_BYTES.saturating_sub(preview.len());
            preview.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        let duration_milliseconds =
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let headers: BTreeMap<String, String> = response
            .headers
            .into_iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| ("content-type".to_owned(), value))
            .collect();
        HttpResponse::json(
            200,
            &FunctionScheduleInvokeResponse {
                status: response.status,
                version: response.version,
                headers,
                body_base64: STANDARD.encode(&preview),
                duration_milliseconds,
            },
        )
        .map_err(|_| internal(request, "scheduled invocation response is unavailable"))
    })
}

fn valid_identifier(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() <= MAXIMUM_IDENTIFIER_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn map_guard_error(request: &HttpRequest, error: RocksInternalReplayGuardError) -> HttpApiError {
    match error {
        RocksInternalReplayGuardError::IdempotencyMismatch => HttpApiError::new(
            409,
            ErrorCode::Conflict,
            "scheduled invocation idempotency key conflicts",
            request.request_id(),
            RetryAdvice::Never,
        ),
        RocksInternalReplayGuardError::Replay => HttpApiError::new(
            409,
            ErrorCode::Conflict,
            "internal scheduled invocation nonce was already used",
            request.request_id(),
            RetryAdvice::Never,
        ),
        _ => unavailable(
            request,
            "scheduled invocation replay protection is unavailable",
        ),
    }
}

fn now_unix_seconds(request: &HttpRequest) -> Result<u64, HttpApiError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().max(1))
        .map_err(|_| internal(request, "service clock is unavailable"))
}

fn invalid(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        400,
        ErrorCode::InvalidRequest,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn unavailable(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        503,
        ErrorCode::Unavailable,
        message,
        request.request_id(),
        RetryAdvice::AfterDelay { after_ms: 1_000 },
    )
}

fn internal(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        500,
        ErrorCode::Internal,
        message,
        request.request_id(),
        RetryAdvice::AfterDelay { after_ms: 1_000 },
    )
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use mako_api::{EnvironmentId, ProjectId, TenantScope};
    use mako_config::{ConfigLoader, ServiceConfig, ServiceKind};
    use mako_internal_rpc::{DeploymentKey, InternalCaller, InternalRequestAuthenticator};
    use tempfile::{Builder, TempDir};

    use super::*;

    const ROOT_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    /// The request id `HttpRequest::for_test` stamps; the signed envelope
    /// must name the same one or verification refuses it.
    const TEST_REQUEST_ID: &str = "req_test_support";

    fn input(tenant: &TenantScope) -> FunctionScheduleInvokeRequest {
        FunctionScheduleInvokeRequest {
            project_id: tenant.project_id().as_str().to_owned(),
            environment_id: tenant.environment_id().as_str().to_owned(),
            function_name: "nightly-report".to_owned(),
            schedule_id: "sch_abcdefghijklmnop".to_owned(),
            run_id: "run_abcdefghijklmnop".to_owned(),
            method: "POST".to_owned(),
            path: "/".to_owned(),
            query: None,
            headers: BTreeMap::new(),
            body_base64: STANDARD.encode(b"{}"),
        }
    }

    fn signed(
        caller: InternalCaller,
        tenant: &TenantScope,
        input: &FunctionScheduleInvokeRequest,
        idempotency_key: &str,
    ) -> HttpRequest {
        let authenticator = InternalRequestAuthenticator::new(
            DeploymentKey::derive(ROOT_KEY).expect("key"),
            caller,
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        let signed = authenticator
            .sign(
                InternalRoute::FunctionScheduleInvoke,
                tenant,
                TEST_REQUEST_ID,
                idempotency_key,
                now,
                serde_json::to_vec(input).expect("json"),
            )
            .expect("signed");
        HttpRequest::for_test(
            HttpMethod::Post,
            InternalRoute::FunctionScheduleInvoke.path(),
            signed.headers,
            signed.body,
            None,
        )
    }

    /// A refusal raised by the route is `Err(code)`; an answer the gateway
    /// produced (including its own error envelopes) is `Ok(status)`.
    fn outcome_of(result: Option<Result<HttpResponse, HttpApiError>>) -> Result<u16, ErrorCode> {
        match result.expect("the route is registered") {
            Ok(response) => Ok(response.status_for_test()),
            Err(error) => Err(error.envelope().error.code),
        }
    }

    #[test]
    fn the_scheduler_hop_is_verified_before_anything_is_invoked() {
        let directory = local_tempdir("edge-gateway-schedule-hop");
        let config = config_for(directory.path());
        let graph = Arc::new(EdgeGatewayGraph::open(&config).expect("edge graph"));
        let router = crate::edge_gateway_router(Arc::clone(&graph)).expect("routes");
        let tenant = TenantScope::new(
            ProjectId::parse("prj_d2b2dcc3f5acbcc2105200bafbde2134".to_owned()).expect("project"),
            EnvironmentId::parse("env_4cfece7ecdaeba1f83064208c1cd84d6".to_owned())
                .expect("environment"),
        );

        // Unsigned: not even an internal-protocol request, refused before
        // anything in the body is read.
        let unsigned = HttpRequest::for_test(
            HttpMethod::Post,
            InternalRoute::FunctionScheduleInvoke.path(),
            [("content-type".to_owned(), "application/json".to_owned())],
            serde_json::to_vec(&input(&tenant)).expect("json"),
            None,
        );
        assert_eq!(
            outcome_of(router.dispatch_for_test(unsigned)),
            Err(ErrorCode::InvalidRequest)
        );

        // Versioned but unsigned: refused by authentication.
        let mut unsigned_headers: Vec<(String, String)> = Vec::new();
        for name in [
            "x-mako-internal-version",
            "x-mako-internal-caller",
            "x-mako-project-id",
            "x-mako-environment-id",
            "x-mako-request-id",
            "x-mako-idempotency-key",
            "x-mako-internal-timestamp",
            "x-mako-internal-nonce",
            "x-mako-content-digest",
            "content-type",
        ] {
            if let Some(value) = signed(
                InternalCaller::ControlPlane,
                &tenant,
                &input(&tenant),
                "idem_unsigned",
            )
            .header(name)
            {
                unsigned_headers.push((name.to_owned(), value.to_owned()));
            }
        }
        let unsigned = HttpRequest::for_test(
            HttpMethod::Post,
            InternalRoute::FunctionScheduleInvoke.path(),
            unsigned_headers,
            serde_json::to_vec(&input(&tenant)).expect("json"),
            None,
        );
        assert_eq!(
            outcome_of(router.dispatch_for_test(unsigned)),
            Err(ErrorCode::Unauthenticated)
        );

        // Signed by the wrong caller: the route belongs to the control plane.
        // (Signing refuses a route the caller does not own, so the envelope
        // is forged from a caller-agnostic signature and fails as such.)
        let wrong_caller = signed(
            InternalCaller::ControlPlane,
            &tenant,
            &input(&tenant),
            "idem_forged",
        );
        let mut forged: Vec<(String, String)> = Vec::new();
        for name in [
            "x-mako-internal-version",
            "x-mako-project-id",
            "x-mako-environment-id",
            "x-mako-request-id",
            "x-mako-idempotency-key",
            "x-mako-internal-timestamp",
            "x-mako-internal-nonce",
            "x-mako-content-digest",
            "x-mako-internal-signature",
            "content-type",
        ] {
            if let Some(value) = wrong_caller.header(name) {
                forged.push((name.to_owned(), value.to_owned()));
            }
        }
        forged.push((
            "x-mako-internal-caller".to_owned(),
            InternalCaller::EdgeGateway.as_str().to_owned(),
        ));
        let forged = HttpRequest::for_test(
            HttpMethod::Post,
            InternalRoute::FunctionScheduleInvoke.path(),
            forged,
            wrong_caller.body().to_vec(),
            None,
        );
        assert_eq!(
            outcome_of(router.dispatch_for_test(forged)),
            Err(ErrorCode::PermissionDenied)
        );

        // A body naming another tenant than the envelope was signed for.
        let other = TenantScope::new(
            ProjectId::parse("prj_0000000000000000000000000000000f".to_owned()).expect("project"),
            tenant.environment_id().clone(),
        );
        let mismatched = signed(
            InternalCaller::ControlPlane,
            &tenant,
            &input(&other),
            "idem_mismatched",
        );
        assert_eq!(
            outcome_of(router.dispatch_for_test(mismatched)),
            Err(ErrorCode::PermissionDenied)
        );

        // Properly signed: the request is admitted to the gateway proper,
        // which cannot resolve the function without a control plane and
        // says so -- as a gateway error envelope with the request's id,
        // which is what the scheduler records as the run's reason.
        let proper = signed(
            InternalCaller::ControlPlane,
            &tenant,
            &input(&tenant),
            "idem_proper",
        );
        let response = router
            .dispatch_for_test(proper)
            .expect("registered")
            .expect("gateway errors are answered, not raised");
        assert_eq!(response.status_for_test(), 503);
        let body: serde_json::Value =
            serde_json::from_slice(response.body_for_test().expect("body")).expect("json");
        assert_eq!(body["error"]["code"], "unavailable");
        assert_eq!(body["error"]["requestId"], TEST_REQUEST_ID);

        // The same signed envelope again is a nonce replay, and a fresh
        // envelope under an idempotency key already used is a duplicate;
        // both are refused before the gateway is asked anything.
        let replayed = signed(
            InternalCaller::ControlPlane,
            &tenant,
            &input(&tenant),
            "idem_replay",
        );
        let first = router
            .dispatch_for_test(replayed.clone())
            .expect("registered")
            .expect("answered");
        assert_eq!(first.status_for_test(), 503);
        assert_eq!(
            outcome_of(router.dispatch_for_test(replayed)),
            Err(ErrorCode::Conflict)
        );
        let duplicate = signed(
            InternalCaller::ControlPlane,
            &tenant,
            &input(&tenant),
            "idem_replay",
        );
        assert_eq!(
            outcome_of(router.dispatch_for_test(duplicate)),
            Err(ErrorCode::Conflict)
        );
    }

    fn config_for(root: &Path) -> ServiceConfig {
        let rocksdb = root.join("rocksdb");
        let backup = root.join("backup");
        fs::create_dir_all(&rocksdb).expect("database directory");
        fs::create_dir_all(&backup).expect("backup directory");
        ConfigLoader::from_environment([
            ("MAKO_ENVIRONMENT", "local"),
            ("MAKO_REGION", "us-east-1-beta"),
            ("MAKO_PUBLIC_URL", "https://api.example.test"),
            ("MAKO_ROCKSDB_PATH", rocksdb.to_str().expect("UTF-8 path")),
            (
                "MAKO_ROCKSDB_BACKUP_DESTINATION",
                backup.to_str().expect("UTF-8 path"),
            ),
            ("MAKO_ROCKSDB_DISK_WARNING_FREE_BYTES", "134217728"),
            ("MAKO_ROCKSDB_DISK_CRITICAL_FREE_BYTES", "67108864"),
            (
                "MAKO_INTERNAL_AUTH_SECRET_REF",
                "env:TEST_EDGE_GATEWAY_SCHEDULE_KEY",
            ),
            ("TEST_EDGE_GATEWAY_SCHEDULE_KEY", ROOT_KEY),
        ])
        .load(ServiceKind::EdgeGateway)
        .expect("configuration")
    }

    fn local_tempdir(prefix: &str) -> TempDir {
        let root = std::env::current_dir()
            .expect("working directory")
            .join(".local");
        fs::create_dir_all(&root).expect("local test root");
        Builder::new()
            .prefix(prefix)
            .tempdir_in(root)
            .expect("temporary directory")
    }

    #[test]
    fn identifiers_are_prefixed_and_bounded() {
        assert!(valid_identifier("sch_abcdefghijklmnop", "sch_"));
        assert!(!valid_identifier("run_abcdefghijklmnop", "sch_"));
        assert!(!valid_identifier("sch_abc/def", "sch_"));
        assert!(!valid_identifier(
            &format!("sch_{}", "a".repeat(64)),
            "sch_"
        ));
    }
}
