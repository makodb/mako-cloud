use std::{
    collections::VecDeque,
    num::{NonZeroU64, NonZeroUsize},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime},
};

use futures::executor::block_on;
use mako_api::{
    ApiError, ApiErrorEnvelope, CollectionId, CollectionScope, ErrorCode, RetryAdvice, SafeDetail,
    TenantScope,
};
use mako_audit::{ActorIdentity, AuditCategory, AuditOutcome};
use mako_documents::{SchemaVersion, ScopedCollectionEngine};
use mako_gateway::{
    AuthorizedReplicationRequest, ReplicationGatewayRequest, ReplicationOperation, ReplicationUsage,
};
use mako_identity::{AccessAuthorizationEpochs, AppUserId, IdentityStore};
use mako_policy::{CompiledPolicySet, SafeRequestMetadata, SubjectId};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
    StreamSendError, StreamSender, spawn_streaming_body,
};
use mako_storage::Durability;
use mako_sync::{
    LiveStreamError, LiveStreamEvent, LiveStreamLimits, LiveStreamRequest, LiveStreamSession,
    OpaqueCheckpoint, OpaqueStreamCursor, PullError, PullRequest, PullService, PushError,
    PushRequest, PushService, ReplicationTokenCodec, ReplicationTokenKey, ResyncReason, SseFrame,
};

use crate::{
    DataPlaneGraph,
    auth_http::{
        append_audit, internal_from_id, invalid, json, now_unix_seconds, parse_json, tenant,
        unavailable,
    },
    document_http::{active_collection_metadata, active_policy},
    graph::ReplicationAuthorizationError,
};

const PUBLIC_KEY_HEADER: &str = "x-mako-key";
const AUTHORIZATION_HEADER: &str = "authorization";
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const LIVE_EVENT_BATCH_SIZE: usize = 100;
const LIVE_SESSION_BUFFER_EVENTS: usize = 32;
const LIVE_TRANSPORT_BUFFER_CHUNKS: usize = 32;
const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const LIVE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

pub fn add_replication_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/replication/pull",
        Arc::clone(&graph),
        handle_pull,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/replication/push",
        Arc::clone(&graph),
        handle_push,
    )?;
    add_route(
        router,
        HttpMethod::Get,
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/replication/stream",
        graph,
        handle_stream,
    )?;
    Ok(())
}

fn add_route(
    router: &mut HttpRouter,
    method: HttpMethod,
    path: &str,
    graph: Arc<DataPlaneGraph>,
    handler: fn(&Arc<DataPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>,
) -> Result<(), RouteRegistrationError> {
    router.add_route(method, path, move |request| handler(&graph, &request))
}

fn handle_pull(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_json(request)?;
    let tenant = tenant(request)?;
    let scope = collection_scope(request, &tenant)?;
    let body: PullRequest = parse_json(request)?;
    body.validate()
        .map_err(|_| invalid(request, "replication pull request is invalid"))?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let authorized = authorize(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            ReplicationOperation::Pull,
            request.body().len(),
            now,
        )
        .await?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "collection scope is invalid"))?;
        let metadata = active_collection_metadata(request, &scoped).await?;
        let policy = active_policy(graph, request, &tenant, &scope, &scoped).await?;
        let authorizer = authorized.policy_read_authorizer(policy.as_ref());
        let service = PullService::new(
            &scoped,
            ReplicationTokenCodec::new(graph.replication_token_key()),
            metadata.schema_version(),
        );
        let response = service
            .pull_authorized(authorized.sync_context(), &body, &authorizer)
            .await
            .map_err(|error| map_pull_error(request, error))?;
        append_replication_audit(
            graph,
            &tenant,
            &authorized,
            scope.collection_id(),
            "replication_pull",
            "policy_filtered_batch",
            request,
            now,
        )
        .await?;
        json(request, 200, &response)
    })
}

fn handle_push(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_json(request)?;
    validate_idempotency_key(request)?;
    let tenant = tenant(request)?;
    let scope = collection_scope(request, &tenant)?;
    let body: PushRequest = parse_json(request)?;
    body.validate()
        .map_err(|_| invalid(request, "replication push request is invalid"))?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let authorized = authorize(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            ReplicationOperation::Push,
            request.body().len(),
            now,
        )
        .await?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "collection scope is invalid"))?;
        let metadata = active_collection_metadata(request, &scoped).await?;
        let policy = active_policy(graph, request, &tenant, &scope, &scoped).await?;
        let validator = mako_documents::DocumentValidator::compile(&metadata)
            .map_err(|_| unavailable(request, "collection schema is unavailable"))?;
        let sequencer = graph
            .document_engine()
            .scope_sequencer(&tenant, &tenant, Durability::Sync)
            .map_err(|_| unavailable(request, "replication sequencer is unavailable"))?;
        let read_authorizer = authorized.policy_read_authorizer(policy.as_ref());
        let mutation_authorizer = authorized.policy_mutation_authorizer(policy.as_ref());
        let response = PushService::new(&scoped, &sequencer, &validator, Durability::Sync)
            .push(
                authorized.sync_context(),
                &body,
                &mutation_authorizer,
                &read_authorizer,
            )
            .await
            .map_err(|error| map_push_error(request, error))?;
        append_replication_audit(
            graph,
            &tenant,
            &authorized,
            scope.collection_id(),
            "replication_push",
            "row_outcomes_returned",
            request,
            now,
        )
        .await?;
        json(request, 200, &response)
    })
}

fn handle_stream(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    if !request.body().is_empty() {
        return Err(invalid(request, "replication stream body must be empty"));
    }
    let tenant = tenant(request)?;
    let scope = collection_scope(request, &tenant)?;
    let live_request = live_request(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let authorized = authorize(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            ReplicationOperation::Live,
            stream_usage_bytes(request),
            now,
        )
        .await?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "collection scope is invalid"))?;
        let metadata = active_collection_metadata(request, &scoped).await?;
        let policy = active_policy(graph, request, &tenant, &scope, &scoped).await?;
        let limits = live_limits();
        {
            let authorizer = authorized.policy_read_authorizer(policy.as_ref());
            LiveStreamSession::open(
                &scoped,
                ReplicationTokenCodec::new(graph.replication_token_key()),
                authorized.sync_context().clone(),
                &authorizer,
                metadata.schema_version(),
                &live_request,
                limits,
            )
            .await
            .map_err(|error| map_live_error(request, error))?;
        }
        append_replication_audit(
            graph,
            &tenant,
            &authorized,
            scope.collection_id(),
            "replication_stream_open",
            "policy_filtered_stream",
            request,
            now,
        )
        .await?;

        let identity = graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?;
        let epochs = graph
            .authorization_epoch_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "authorization authority is unavailable"))?;
        let subject = SubjectId::parse(authorized.sync_context().user_id().as_str())
            .map_err(|_| unavailable(request, "authorization context is unavailable"))?;
        let token_key = graph.replication_token_key().clone();
        let required_schema_version = metadata.schema_version();
        let body = spawn_streaming_body(LIVE_TRANSPORT_BUFFER_CHUNKS, move |sender| {
            run_live_stream(
                sender,
                scoped,
                token_key,
                authorized,
                policy,
                required_schema_version,
                live_request,
                identity,
                epochs,
                subject,
                limits,
            );
        })
        .map_err(|_| internal_from_id(request.request_id(), "replication stream is unavailable"))?;
        HttpResponse::stream(200, "text/event-stream; charset=utf-8", body)
            .with_header("X-Accel-Buffering", "no")
            .map_err(|_| {
                internal_from_id(request.request_id(), "replication stream is unavailable")
            })
    })
}

#[allow(clippy::too_many_arguments)]
fn run_live_stream(
    sender: StreamSender,
    scoped: ScopedCollectionEngine,
    token_key: ReplicationTokenKey,
    authorized: AuthorizedReplicationRequest,
    policy: Option<CompiledPolicySet>,
    required_schema_version: SchemaVersion,
    request: LiveStreamRequest,
    identity: IdentityStore,
    epochs: mako_policy::AuthorizationEpochStore,
    subject: SubjectId,
    limits: LiveStreamLimits,
) {
    let authorizer = authorized.policy_read_authorizer(policy.as_ref());
    let Ok(mut session) = block_on(LiveStreamSession::open(
        &scoped,
        ReplicationTokenCodec::new(&token_key),
        authorized.sync_context().clone(),
        &authorizer,
        required_schema_version,
        &request,
        limits,
    )) else {
        return;
    };
    let user_id = session_user_id(authorized.sync_context().user_id());
    let session_id = authorized.sync_context().session_id().clone();
    if block_on(session.heartbeat()).is_err() {
        return;
    }
    let mut pending = frames(session.drain_events());
    let mut last_heartbeat = Instant::now();

    loop {
        if sender.is_disconnected() {
            return;
        }
        if let Some(frame) = pending.front().cloned() {
            match sender.try_send(frame.bytes) {
                Ok(()) => {
                    pending.pop_front();
                    if frame.terminal {
                        return;
                    }
                }
                Err(StreamSendError::Closed) => return,
                Err(StreamSendError::Full) => {
                    if !pending.iter().any(|queued| queued.terminal) {
                        pending.clear();
                        pending.push_back(resync_frame(ResyncReason::StreamGap));
                    }
                    thread::sleep(LIVE_POLL_INTERVAL);
                }
            }
            continue;
        }

        let Ok(now) = system_now() else {
            return;
        };
        if !matches!(
            block_on(identity.session_is_active(&user_id, &session_id, now)),
            Ok(true)
        ) {
            return;
        }
        let Ok(current) = block_on(epochs.epochs_for(&subject)) else {
            return;
        };
        session.observe_authorization_epochs(AccessAuthorizationEpochs {
            environment: current.environment().get(),
            user: current.user().get(),
        });
        if block_on(session.poll()).is_err() {
            return;
        }
        if last_heartbeat.elapsed() >= LIVE_HEARTBEAT_INTERVAL {
            if block_on(session.heartbeat()).is_err() {
                return;
            }
            last_heartbeat = Instant::now();
        }
        pending = frames(session.drain_events());
        if pending.is_empty() {
            thread::sleep(LIVE_POLL_INTERVAL);
        }
    }
}

#[derive(Clone)]
struct PendingFrame {
    bytes: Vec<u8>,
    terminal: bool,
}

fn frames(events: impl IntoIterator<Item = LiveStreamEvent>) -> VecDeque<PendingFrame> {
    events
        .into_iter()
        .filter_map(|event| {
            let terminal = matches!(event, LiveStreamEvent::Resync { .. });
            SseFrame::from_event(&event).ok().map(|frame| PendingFrame {
                bytes: frame.as_str().as_bytes().to_vec(),
                terminal,
            })
        })
        .collect()
}

fn resync_frame(reason: ResyncReason) -> PendingFrame {
    frames([LiveStreamEvent::Resync { reason }])
        .pop_front()
        .expect("resync events always serialize")
}

async fn authorize(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    operation: ReplicationOperation,
    request_bytes: usize,
    now: u64,
) -> Result<AuthorizedReplicationRequest, HttpApiError> {
    let public_key = request
        .header(PUBLIC_KEY_HEADER)
        .ok_or_else(|| unauthenticated(request, "public project key is required"))?;
    let access_token = bearer_token(request)?;
    let request_bytes = u64::try_from(request_bytes)
        .ok()
        .and_then(NonZeroU64::new)
        .unwrap_or(NonZeroU64::MIN);
    let metadata = SafeRequestMetadata::new([
        ("method".to_owned(), request.method().to_string()),
        ("request_id".to_owned(), request.request_id().to_owned()),
    ])
    .map_err(|_| invalid(request, "replication request metadata is invalid"))?;
    let gateway_request = ReplicationGatewayRequest::new(
        tenant.clone(),
        collection_id.clone(),
        public_key,
        access_token,
        now,
        ReplicationUsage::new(operation, request_bytes),
        metadata,
    )
    .with_request_id(request.request_id());
    let authorized = graph
        .authorize_replication(tenant, gateway_request)
        .await
        .map_err(|error| match error {
            ReplicationAuthorizationError::AuthorityUnavailable => {
                unavailable(request, "replication authorization is unavailable")
            }
            ReplicationAuthorizationError::Gateway(error) => {
                let envelope = error.api_error().clone();
                HttpApiError::from_envelope(status_for(envelope.error.code), envelope)
            }
        })?;

    // Authorization is where replication is metered, so it is also where the
    // work becomes reportable. Only what the gateway actually charged for is
    // emitted, and only resources the product vocabulary already names.
    let observed_at = now.saturating_mul(1_000);
    for (resource, quantity, unit) in [
        (
            mako_api::QuotaResource::ReplicationRequestsPerMinute,
            1,
            "requests",
        ),
        (
            mako_api::QuotaResource::ReplicationBytesPerMonth,
            request_bytes.get(),
            "bytes",
        ),
    ] {
        graph.telemetry().record(mako_api::ObservabilityRecord {
            tenant: tenant.clone(),
            timestamp_unix_milliseconds: observed_at,
            payload: mako_api::ObservabilityPayload::Usage {
                resource,
                quantity,
                unit: unit.to_owned(),
            },
        });
    }
    Ok(authorized)
}

fn live_request(request: &HttpRequest) -> Result<LiveStreamRequest, HttpApiError> {
    let mut schema_version = None;
    let mut checkpoint = None;
    let mut cursor = None;
    for (name, value) in request.query() {
        match name.as_str() {
            "schemaVersion" if schema_version.is_none() => {
                schema_version = value.parse::<u64>().ok().filter(|version| *version > 0);
                if schema_version.is_none() {
                    return Err(invalid(request, "schemaVersion is invalid"));
                }
            }
            "checkpoint" if checkpoint.is_none() => {
                checkpoint = Some(parse_opaque::<OpaqueCheckpoint>(request, value)?);
            }
            "cursor" if cursor.is_none() => {
                cursor = Some(parse_opaque::<OpaqueStreamCursor>(request, value)?);
            }
            _ => return Err(invalid(request, "replication stream query is invalid")),
        }
    }
    let live = LiveStreamRequest {
        schema_version: schema_version
            .ok_or_else(|| invalid(request, "schemaVersion is required"))?,
        checkpoint,
        cursor,
    };
    live.validate()
        .map_err(|_| invalid(request, "replication stream query is invalid"))?;
    Ok(live)
}

fn parse_opaque<T: for<'de> serde::Deserialize<'de>>(
    request: &HttpRequest,
    value: &str,
) -> Result<T, HttpApiError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| invalid(request, "replication stream token is invalid"))
}

fn bearer_token(request: &HttpRequest) -> Result<&str, HttpApiError> {
    let header = request
        .header(AUTHORIZATION_HEADER)
        .ok_or_else(|| unauthenticated(request, "application access token is required"))?;
    let mut parts = header.split_ascii_whitespace();
    let scheme = parts
        .next()
        .ok_or_else(|| unauthenticated(request, "application access token is required"))?;
    let token = parts
        .next()
        .ok_or_else(|| unauthenticated(request, "application access token is required"))?;
    if !scheme.eq_ignore_ascii_case("bearer") || parts.next().is_some() {
        return Err(unauthenticated(
            request,
            "application access token is invalid",
        ));
    }
    Ok(token)
}

fn collection_scope(
    request: &HttpRequest,
    tenant: &TenantScope,
) -> Result<CollectionScope, HttpApiError> {
    let collection = request
        .path_parameter("collectionId")
        .ok_or_else(|| invalid(request, "collection path is invalid"))?;
    Ok(CollectionScope::new(
        tenant.clone(),
        CollectionId::parse(collection)
            .map_err(|_| invalid(request, "collection path is invalid"))?,
    ))
}

fn validate_idempotency_key(request: &HttpRequest) -> Result<(), HttpApiError> {
    let key = request
        .header(IDEMPOTENCY_HEADER)
        .ok_or_else(|| invalid(request, "idempotency key is required"))?;
    if !(16..=200).contains(&key.len()) || key.chars().any(char::is_control) {
        return Err(invalid(request, "idempotency key is invalid"));
    }
    Ok(())
}

fn require_json(request: &HttpRequest) -> Result<(), HttpApiError> {
    let content_type = request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type != Some("application/json") {
        return Err(invalid(request, "content type must be application/json"));
    }
    Ok(())
}

fn stream_usage_bytes(request: &HttpRequest) -> usize {
    request.path().len().saturating_add(
        request
            .query()
            .iter()
            .map(|(key, value)| key.len().saturating_add(value.len()).saturating_add(2))
            .sum(),
    )
}

fn live_limits() -> LiveStreamLimits {
    LiveStreamLimits::new(
        NonZeroUsize::new(LIVE_EVENT_BATCH_SIZE).expect("live batch size is positive"),
        NonZeroUsize::new(LIVE_SESSION_BUFFER_EVENTS).expect("live buffer is positive"),
    )
}

fn session_user_id(user_id: &AppUserId) -> AppUserId {
    user_id.clone()
}

fn system_now() -> Result<u64, std::time::SystemTimeError> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
}

#[allow(clippy::too_many_arguments)]
async fn append_replication_audit(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    authorized: &AuthorizedReplicationRequest,
    collection_id: &CollectionId,
    action: &str,
    reason: &str,
    request: &HttpRequest,
    now: u64,
) -> Result<(), HttpApiError> {
    append_audit(
        graph,
        tenant,
        AuditCategory::Policy,
        ActorIdentity::ApplicationUser {
            actor_id: authorized.sync_context().user_id().as_str().to_owned(),
            session_id: authorized.sync_context().session_id().as_str().to_owned(),
        },
        "collection",
        collection_id.as_str(),
        action,
        AuditOutcome::Allowed,
        reason,
        request.request_id(),
        now,
    )
    .await
}

fn map_pull_error(request: &HttpRequest, error: PullError) -> HttpApiError {
    match error {
        PullError::Contract(_)
        | PullError::Token(_)
        | PullError::ExpectedNullCheckpoint
        | PullError::ScopeMismatch
        | PullError::CheckpointBeyondHighWater => {
            invalid(request, "replication checkpoint or pull request is invalid")
        }
        PullError::SchemaMigrationRequired(_) => schema_mismatch(request),
        PullError::CheckpointExpired { minimum_position } => {
            checkpoint_expired(request, minimum_position)
        }
        PullError::ChangeLog(_) | PullError::Retention(_) | PullError::ChangeScanDidNotAdvance => {
            unavailable(request, "replication pull is unavailable")
        }
    }
}

fn map_push_error(request: &HttpRequest, error: PushError) -> HttpApiError {
    match error {
        PushError::Contract(_) | PushError::ScopeMismatch => {
            invalid(request, "replication push request is invalid")
        }
        PushError::SchemaMigrationRequired(_) => schema_mismatch(request),
        PushError::Document(_)
        | PushError::Mutation(_)
        | PushError::Read(_)
        | PushError::Sequencer(_) => unavailable(request, "replication push is unavailable"),
    }
}

fn map_live_error(request: &HttpRequest, error: LiveStreamError) -> HttpApiError {
    match error {
        LiveStreamError::Contract(_)
        | LiveStreamError::Token(_)
        | LiveStreamError::ScopeMismatch
        | LiveStreamError::PositionBeyondHighWater => {
            invalid(request, "replication stream position is invalid")
        }
        LiveStreamError::SchemaMigrationRequired(_) => schema_mismatch(request),
        LiveStreamError::ChangeLog(_)
        | LiveStreamError::Retention(_)
        | LiveStreamError::Json(_)
        | LiveStreamError::InvalidLimits
        | LiveStreamError::ChangeScanDidNotAdvance => {
            unavailable(request, "replication stream is unavailable")
        }
    }
}

fn schema_mismatch(request: &HttpRequest) -> HttpApiError {
    HttpApiError::new(
        409,
        ErrorCode::SchemaMismatch,
        "replication schema migration is required",
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn checkpoint_expired(request: &HttpRequest, minimum_position: u64) -> HttpApiError {
    let envelope = ApiErrorEnvelope::new(
        ApiError::new(
            ErrorCode::CheckpointExpired,
            "replication checkpoint has expired",
            request.request_id(),
            RetryAdvice::Never,
        )
        .with_detail(
            "minimumPosition",
            SafeDetail::String(minimum_position.to_string()),
        ),
    );
    HttpApiError::from_envelope(409, envelope)
}

fn unauthenticated(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        401,
        ErrorCode::Unauthenticated,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

const fn status_for(code: ErrorCode) -> u16 {
    match code {
        ErrorCode::InvalidRequest => 400,
        ErrorCode::Unauthenticated | ErrorCode::OperatorStepUpRequired => 401,
        ErrorCode::PermissionDenied => 403,
        ErrorCode::NotFound => 404,
        ErrorCode::Conflict | ErrorCode::SchemaMismatch | ErrorCode::CheckpointExpired => 409,
        ErrorCode::RateLimited | ErrorCode::QuotaExceeded => 429,
        ErrorCode::Unavailable => 503,
        ErrorCode::Internal => 500,
    }
}
