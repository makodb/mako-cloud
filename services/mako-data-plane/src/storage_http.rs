//! Application file storage: buckets and objects behind the tenant's policies.
//!
//! Objects are addressed by bucket and a `/`-separated path. Uploads and
//! deletes need an application session or a service credential; reads need
//! one too unless the bucket is public. Every request is charged like the
//! document API, downloads are metered as egress, and stored bytes are
//! sampled as a level from the totals the storage service keeps.
use std::{num::NonZeroU64, sync::Arc};

use futures::executor::block_on;
use mako_api::{
    CollectionId, CollectionScope, ErrorCode, ObservabilityPayload, ObservabilityRecord,
    QuotaResource, RetryAdvice, TenantScope,
};
use mako_audit::{ActorIdentity, AuditCategory, AuditOutcome};
use mako_file_storage::{
    BucketConfig, FileStorageError, FileStorageService, MAX_APPLICATION_OBJECT_BYTES,
    ObjectPrecondition, ObjectPrincipal, ObjectRequest,
};
use mako_gateway::{
    GatewayQuotaCharge, GatewayQuotaDecision, GatewayQuotaPolicySource, GatewayQuotaResource,
};
use mako_internal_rpc::{
    DeleteBucketObjectInput, IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission,
    InspectBucketInput, InstallBucketInput, ListBucketObjectsInput, RemoveBucketInput,
};
use mako_policy::DocumentOperation;
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde_json::{Value, json};

use crate::{
    DataPlaneGraph,
    auth_http::{self, invalid, now_unix_seconds, tenant_for, unauthenticated, unavailable},
    document_http, internal_http,
};

const BASE: &str = "/v1/projects/{projectId}/environments/{environmentId}";
/// Headers and framing on top of the largest object a bucket may accept.
const UPLOAD_BODY_LIMIT: usize = MAX_APPLICATION_OBJECT_BYTES as usize + 4096;
const DEFAULT_LIST_LIMIT: usize = 100;

pub fn add_storage_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (prefix, service) in [("/storage", false), ("/service/storage", true)] {
        let listing = format!("{BASE}{prefix}/{{bucketId}}/objects");
        let object = format!("{listing}/{{objectPath...}}");
        let handler_graph = Arc::clone(&graph);
        router.add_route(HttpMethod::Get, &listing, move |request| {
            handle_list(&handler_graph, &request, service)
        })?;
        let handler_graph = Arc::clone(&graph);
        router.add_route_with_body_limit(
            HttpMethod::Put,
            &object,
            Some(UPLOAD_BODY_LIMIT),
            move |request| handle_put(&handler_graph, &request, service),
        )?;
        let handler_graph = Arc::clone(&graph);
        router.add_route(HttpMethod::Get, &object, move |request| {
            handle_get(&handler_graph, &request, service)
        })?;
        let handler_graph = Arc::clone(&graph);
        router.add_route(HttpMethod::Delete, &object, move |request| {
            handle_delete(&handler_graph, &request, service)
        })?;
    }
    Ok(())
}

// ---- application routes ----------------------------------------------------

fn handle_put(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    service: bool,
) -> Result<HttpResponse, HttpApiError> {
    block_on(async {
        let tenant = tenant_for(graph, request)?;
        let bucket_id = bucket_id(request)?;
        let path = object_path(request)?;
        let now = now_unix_seconds(request.request_id())?;
        let content_type = request
            .header("content-type")
            .ok_or_else(|| invalid(request, "content-type is required"))?
            .to_owned();
        // Parsed before anything is charged or written: a conditional
        // header the platform does not implement is a refusal, never a
        // silent unconditional write.
        let preconditions = preconditions(request)?;
        let (principal, actor) = principal(
            graph,
            request,
            &tenant,
            &bucket_id,
            DocumentOperation::Create,
            service,
            now,
        )
        .await?;
        charge(graph, &tenant, request, now, Vec::new()).await?;
        let ceiling = storage_ceiling(graph, &tenant, request).await?;
        let storage = storage(graph, request, &tenant)?;
        let outcome = storage
            .put_object_if(
                &bucket_id,
                &path,
                &content_type,
                request.body(),
                &object_request(principal, request, now, ceiling),
                &preconditions,
            )
            .await;
        audit(
            graph,
            &tenant,
            actor,
            &bucket_id,
            &path,
            "object_upload",
            &outcome,
            request,
            now,
        )
        .await;
        let record = outcome.map_err(|error| storage_error(request, error))?;
        graph.storage_sampler().mark_objects(&tenant);
        auth_http::json(request, 200, &record)
    })
}

fn handle_get(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    service: bool,
) -> Result<HttpResponse, HttpApiError> {
    block_on(async {
        let tenant = tenant_for(graph, request)?;
        let bucket_id = bucket_id(request)?;
        let path = object_path(request)?;
        let now = now_unix_seconds(request.request_id())?;
        let (principal, actor) = principal(
            graph,
            request,
            &tenant,
            &bucket_id,
            DocumentOperation::Read,
            service,
            now,
        )
        .await?;
        charge(graph, &tenant, request, now, Vec::new()).await?;
        let storage = storage(graph, request, &tenant)?;
        let outcome = storage
            .get_object(
                &bucket_id,
                &path,
                &object_request(principal, request, now, None),
            )
            .await;
        audit(
            graph,
            &tenant,
            actor,
            &bucket_id,
            &path,
            "object_read",
            &outcome,
            request,
            now,
        )
        .await;
        let stored = outcome.map_err(|error| storage_error(request, error))?;
        // Egress is charged and metered before a byte leaves: a refused download sends none.
        if let Some(bytes) = NonZeroU64::new(stored.record.size_bytes) {
            charge(
                graph,
                &tenant,
                request,
                now,
                vec![GatewayQuotaCharge {
                    resource: GatewayQuotaResource::EgressBytes,
                    amount: bytes,
                }],
            )
            .await?;
            let observed_at = now.saturating_mul(1_000);
            graph.quota_checkpoints().mark(&tenant, observed_at);
            graph.telemetry().record(ObservabilityRecord {
                tenant: tenant.clone(),
                timestamp_unix_milliseconds: observed_at,
                payload: ObservabilityPayload::Usage {
                    resource: QuotaResource::ObjectEgressBytesPerMonth,
                    quantity: bytes.get(),
                    unit: "bytes".to_owned(),
                },
            });
        }
        // The transport owns caching headers; the ETag is the plaintext digest.
        HttpResponse::bytes(200, stored.record.content_type.clone(), stored.bytes)
            .with_header("etag", &format!("\"{}\"", stored.record.digest))
            .map_err(|_| unavailable(request, "response could not be composed"))
    })
}

fn handle_delete(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    service: bool,
) -> Result<HttpResponse, HttpApiError> {
    block_on(async {
        let tenant = tenant_for(graph, request)?;
        let bucket_id = bucket_id(request)?;
        let path = object_path(request)?;
        let now = now_unix_seconds(request.request_id())?;
        let (principal, actor) = principal(
            graph,
            request,
            &tenant,
            &bucket_id,
            DocumentOperation::Delete,
            service,
            now,
        )
        .await?;
        charge(graph, &tenant, request, now, Vec::new()).await?;
        let storage = storage(graph, request, &tenant)?;
        let outcome = storage
            .delete_object(
                &bucket_id,
                &path,
                &object_request(principal, request, now, None),
            )
            .await;
        audit(
            graph,
            &tenant,
            actor,
            &bucket_id,
            &path,
            "object_delete",
            &outcome,
            request,
            now,
        )
        .await;
        let record = outcome.map_err(|error| storage_error(request, error))?;
        graph.storage_sampler().mark_objects(&tenant);
        auth_http::json(request, 200, &record)
    })
}

fn handle_list(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    service: bool,
) -> Result<HttpResponse, HttpApiError> {
    block_on(async {
        let tenant = tenant_for(graph, request)?;
        let bucket_id = bucket_id(request)?;
        let now = now_unix_seconds(request.request_id())?;
        let (principal, actor) = principal(
            graph,
            request,
            &tenant,
            &bucket_id,
            DocumentOperation::Read,
            service,
            now,
        )
        .await?;
        charge(graph, &tenant, request, now, Vec::new()).await?;
        let prefix = query(request, "prefix");
        let cursor = query(request, "cursor");
        let limit = match query(request, "limit") {
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|limit| (1..=1_000).contains(limit))
                .ok_or_else(|| invalid(request, "limit must be between 1 and 1000"))?,
            None => DEFAULT_LIST_LIMIT,
        };
        let storage = storage(graph, request, &tenant)?;
        let outcome = storage
            .list_objects(
                &bucket_id,
                prefix.as_deref(),
                limit,
                cursor.as_deref(),
                &object_request(principal, request, now, None),
            )
            .await;
        audit(
            graph,
            &tenant,
            actor,
            &bucket_id,
            "",
            "object_list",
            &outcome,
            request,
            now,
        )
        .await;
        let page = outcome.map_err(|error| storage_error(request, error))?;
        auth_http::json(
            request,
            200,
            &json!({ "items": page.items, "nextCursor": page.next_cursor }),
        )
    })
}

// ---- administration from the control plane ----------------------------------

pub(crate) async fn execute_bucket_operation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    let (permission, action) = match command.operation {
        IdentityAdminOperation::InstallBucket => {
            (IdentityAdminPermission::ManageBuckets, "bucket_install")
        }
        IdentityAdminOperation::RemoveBucket => {
            (IdentityAdminPermission::ManageBuckets, "bucket_remove")
        }
        IdentityAdminOperation::DeleteBucketObject => (
            IdentityAdminPermission::ManageBuckets,
            "bucket_object_delete",
        ),
        IdentityAdminOperation::ListBuckets => {
            (IdentityAdminPermission::ReadBuckets, "bucket_list")
        }
        IdentityAdminOperation::InspectBucket => {
            (IdentityAdminPermission::ReadBuckets, "bucket_inspect")
        }
        IdentityAdminOperation::ListBucketObjects => {
            (IdentityAdminPermission::ReadBuckets, "bucket_object_list")
        }
        _ => return Err(invalid(request, "identity operation dispatch is invalid")),
    };
    internal_http::require_permission(
        graph, request, tenant, command, permission, action, "buckets", now,
    )
    .await?;
    let storage = storage(graph, request, tenant)?;
    let developer = ActorIdentity::Developer {
        actor_id: command.actor_id.clone(),
    };
    let body = match command.operation {
        IdentityAdminOperation::InstallBucket => {
            let input: InstallBucketInput = internal_http::parse_input(request, &command.input)?;
            let config: BucketConfig = serde_json::from_value(input.bucket)
                .map_err(|_| invalid(request, "bucket configuration is invalid"))?;
            let bucket_id = config.id.clone();
            let outcome = storage.install_bucket(config, now).await;
            audit(
                graph,
                tenant,
                developer,
                &bucket_id,
                "",
                "bucket_install",
                &outcome,
                request,
                now,
            )
            .await;
            serde_json::to_value(outcome.map_err(|error| storage_error(request, error))?)
        }
        IdentityAdminOperation::RemoveBucket => {
            let input: RemoveBucketInput = internal_http::parse_input(request, &command.input)?;
            let outcome = storage
                .remove_bucket(&input.bucket_id, input.delete_objects)
                .await;
            audit(
                graph,
                tenant,
                developer,
                &input.bucket_id,
                "",
                "bucket_remove",
                &outcome,
                request,
                now,
            )
            .await;
            let removed = outcome.map_err(|error| storage_error(request, error))?;
            graph.storage_sampler().mark_objects(tenant);
            Ok(json!({ "objectCount": removed.object_count, "totalBytes": removed.total_bytes }))
        }
        IdentityAdminOperation::ListBuckets => {
            let buckets = storage
                .list_buckets()
                .await
                .map_err(|error| storage_error(request, error))?;
            let mut items = Vec::with_capacity(buckets.len());
            for bucket in buckets {
                let totals = storage
                    .store()
                    .totals(&bucket.config.id)
                    .await
                    .map_err(|error| storage_error(request, error))?;
                items.push(json!({ "bucket": bucket, "totals": totals }));
            }
            Ok(json!({ "items": items }))
        }
        IdentityAdminOperation::InspectBucket => {
            let input: InspectBucketInput = internal_http::parse_input(request, &command.input)?;
            let (bucket, totals) = storage
                .inspect_bucket(&input.bucket_id)
                .await
                .map_err(|error| storage_error(request, error))?;
            Ok(json!({ "bucket": bucket, "totals": totals }))
        }
        IdentityAdminOperation::ListBucketObjects => {
            let input: ListBucketObjectsInput =
                internal_http::parse_input(request, &command.input)?;
            let limit = usize::try_from(input.limit).unwrap_or(DEFAULT_LIST_LIMIT);
            let page = storage
                .list_objects(
                    &input.bucket_id,
                    input.prefix.as_deref(),
                    limit,
                    input.cursor.as_deref(),
                    &ObjectRequest {
                        principal: ObjectPrincipal::Service {
                            actor_id: command.actor_id.clone(),
                        },
                        request_metadata: Vec::new(),
                        now_unix_seconds: now,
                        storage_ceiling_bytes: None,
                    },
                )
                .await
                .map_err(|error| storage_error(request, error))?;
            Ok(json!({ "items": page.items, "nextCursor": page.next_cursor }))
        }
        IdentityAdminOperation::DeleteBucketObject => {
            let input: DeleteBucketObjectInput =
                internal_http::parse_input(request, &command.input)?;
            let outcome = storage
                .delete_object(
                    &input.bucket_id,
                    &input.path,
                    &ObjectRequest {
                        principal: ObjectPrincipal::Service {
                            actor_id: command.actor_id.clone(),
                        },
                        request_metadata: Vec::new(),
                        now_unix_seconds: now,
                        storage_ceiling_bytes: None,
                    },
                )
                .await;
            audit(
                graph,
                tenant,
                developer,
                &input.bucket_id,
                &input.path,
                "object_delete",
                &outcome,
                request,
                now,
            )
            .await;
            let record = outcome.map_err(|error| storage_error(request, error))?;
            graph.storage_sampler().mark_objects(tenant);
            serde_json::to_value(record)
        }
        _ => return Err(invalid(request, "identity operation dispatch is invalid")),
    }
    .map_err(|_| unavailable(request, "bucket response could not be encoded"))?;
    serde_json::to_vec(&body)
        .map_err(|_| unavailable(request, "bucket response could not be encoded"))
}

// ---- helpers -----------------------------------------------------------------

fn storage(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
) -> Result<FileStorageService, HttpApiError> {
    graph
        .file_storage(tenant)
        .map_err(|_| unavailable(request, "file storage is unavailable"))
}

fn bucket_id(request: &HttpRequest) -> Result<String, HttpApiError> {
    let value = request
        .path_parameter("bucketId")
        .ok_or_else(|| invalid(request, "bucket id is required"))?;
    if !mako_object_store::valid_bucket_name(value) {
        return Err(invalid(request, "bucket id is invalid"));
    }
    Ok(value.to_owned())
}

/// The object path as the route captured it, percent-decoded. What comes back
/// is validated again by the storage service, which owns the path rules.
fn object_path(request: &HttpRequest) -> Result<String, HttpApiError> {
    let raw = request
        .path_parameter("objectPath")
        .ok_or_else(|| invalid(request, "object path is required"))?;
    percent_decode(raw).ok_or_else(|| invalid(request, "object path is not valid percent-encoding"))
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            decoded.push(u8::from_str_radix(text, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn query(request: &HttpRequest, name: &str) -> Option<String> {
    request
        .query()
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
}

/// The conditional headers an upload may carry, as the storage service's
/// preconditions.
///
/// `If-None-Match: *` makes the upload create-only; `If-Match: *` makes it
/// replace-only; `If-Match: "<etag>"` pins it to the exact version the
/// caller read, where the ETag is the one a download returns. Anything else
/// -- a list of tags, a weak tag, an `If-None-Match` other than `*` -- is
/// refused rather than ignored: a caller that asked for a condition the
/// platform does not implement must not be answered as if it had asked for
/// nothing.
fn preconditions(request: &HttpRequest) -> Result<Vec<ObjectPrecondition>, HttpApiError> {
    let mut preconditions = Vec::new();
    if !request.header_values("if-none-match").is_empty() {
        let value = request
            .header("if-none-match")
            .map(str::trim)
            .ok_or_else(|| invalid(request, "if-none-match is repeated"))?;
        if value != "*" {
            return Err(invalid(
                request,
                "if-none-match supports only * on an upload",
            ));
        }
        preconditions.push(ObjectPrecondition::NotStored);
    }
    if !request.header_values("if-match").is_empty() {
        let value = request
            .header("if-match")
            .map(str::trim)
            .ok_or_else(|| invalid(request, "if-match is repeated"))?;
        preconditions.push(if value == "*" {
            ObjectPrecondition::Stored
        } else {
            ObjectPrecondition::Digest(entity_tag(request, value)?)
        });
    }
    Ok(preconditions)
}

/// The digest inside a strong entity tag, exactly as an upload's response
/// and a download's `ETag` carry it.
fn entity_tag(request: &HttpRequest, value: &str) -> Result<String, HttpApiError> {
    let digest = value
        .strip_prefix('"')
        .and_then(|tag| tag.strip_suffix('"'))
        .filter(|digest| {
            !digest.is_empty()
                && digest.len() <= 128
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
        })
        .ok_or_else(|| invalid(request, "if-match must be one strong entity tag or *"))?;
    Ok(digest.to_owned())
}

fn object_request(
    principal: ObjectPrincipal,
    request: &HttpRequest,
    now: u64,
    storage_ceiling_bytes: Option<u64>,
) -> ObjectRequest {
    ObjectRequest {
        principal,
        request_metadata: vec![
            (
                "method".to_owned(),
                method_name(request.method()).to_owned(),
            ),
            ("request_id".to_owned(), request.request_id().to_owned()),
        ],
        now_unix_seconds: now,
        storage_ceiling_bytes,
    }
}

fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
        _ => "OTHER",
    }
}

/// Who is asking: a service credential (verified and audited as a bypass), an
/// application session, or nobody at all. A missing credential is not an
/// error here; the storage service decides whether the bucket admits it.
async fn principal(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &TenantScope,
    bucket_id: &str,
    operation: DocumentOperation,
    service: bool,
    now: u64,
) -> Result<(ObjectPrincipal, ActorIdentity), HttpApiError> {
    if service {
        let scope = CollectionScope::new(
            tenant.clone(),
            CollectionId::parse(bucket_id).map_err(|_| invalid(request, "bucket id is invalid"))?,
        );
        let authorizer =
            document_http::service_authorizer(graph, request, tenant, &scope, operation, now)
                .await?;
        let actor_id = authorizer.actor_id().to_owned();
        return Ok((
            ObjectPrincipal::Service {
                actor_id: actor_id.clone(),
            },
            ActorIdentity::Service { actor_id },
        ));
    }
    if request.header("authorization").is_none() {
        return Ok((ObjectPrincipal::Anonymous, ActorIdentity::Anonymous));
    }
    let access = auth_http::verify_bearer(graph, tenant, request, now).await?;
    Ok((
        ObjectPrincipal::User {
            user_id: access.user_id().as_str().to_owned(),
            role: access.role().to_owned(),
            email: access.policy_email(),
            trusted_claims: Value::Object(access.trusted_claims().clone()),
        },
        ActorIdentity::ApplicationUser {
            actor_id: access.user_id().as_str().to_owned(),
            session_id: access.session_id().as_str().to_owned(),
        },
    ))
}

/// Every storage request is one egress request; downloads add their bytes.
async fn charge(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    request: &HttpRequest,
    now: u64,
    mut charges: Vec<GatewayQuotaCharge>,
) -> Result<(), HttpApiError> {
    let with_bytes = !charges.is_empty();
    if !with_bytes {
        charges.push(GatewayQuotaCharge {
            resource: GatewayQuotaResource::EgressRequests,
            amount: NonZeroU64::new(1).expect("one is non-zero"),
        });
    }
    let route_digest = &blake3::hash(request.path().as_bytes()).to_hex()[..16];
    let reservation = format!(
        "{}.storage{}.{route_digest}",
        request.request_id(),
        if with_bytes { ".bytes" } else { "" }
    );
    let policy = graph
        .quota_policies()
        .policy_for(tenant)
        .await
        .map_err(|_| unavailable(request, "quota authority is unavailable"))?;
    match graph
        .quota_engine()
        .check_and_reserve(
            tenant,
            &reservation,
            &charges,
            &policy,
            now.saturating_mul(1_000),
        )
        .await
        .map_err(|_| unavailable(request, "quota authority is unavailable"))?
    {
        GatewayQuotaDecision::Allowed => Ok(()),
        GatewayQuotaDecision::Throttled {
            retry_after_milliseconds,
            ..
        } => Err(HttpApiError::new(
            429,
            ErrorCode::RateLimited,
            "storage request rate exceeded",
            request.request_id(),
            RetryAdvice::AfterDelay {
                after_ms: retry_after_milliseconds,
            },
        )),
        GatewayQuotaDecision::HardLimit { .. } => Err(HttpApiError::new(
            429,
            ErrorCode::QuotaExceeded,
            "storage quota exceeded",
            request.request_id(),
            RetryAdvice::Never,
        )),
    }
}

/// The plan's ceiling on stored object bytes, carried on the installed quota
/// policy as a nominal hard window whose limit is the byte total.
async fn storage_ceiling(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    request: &HttpRequest,
) -> Result<Option<u64>, HttpApiError> {
    let policy = graph
        .quota_policies()
        .policy_for(tenant)
        .await
        .map_err(|_| unavailable(request, "quota authority is unavailable"))?;
    Ok(policy
        .limit(GatewayQuotaResource::ObjectStorageBytes)
        .and_then(|limit| limit.hard)
        .map(|window| window.limit.get()))
}

// Every audited object action carries the same nine facts; a struct would name them once
// but read worse at seven call sites.
#[allow(clippy::too_many_arguments)]
async fn audit<T>(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    actor: ActorIdentity,
    bucket_id: &str,
    path: &str,
    action: &str,
    outcome: &Result<T, FileStorageError>,
    request: &HttpRequest,
    now: u64,
) {
    let (result, reason) = match outcome {
        Ok(_) => (AuditOutcome::Allowed, "storage_authorized"),
        Err(FileStorageError::Denied(_)) => (AuditOutcome::Denied, "storage_policy_denied"),
        Err(FileStorageError::StorageCapExceeded { .. }) => {
            (AuditOutcome::Denied, "storage_cap_exceeded")
        }
        Err(FileStorageError::ObjectNotFound | FileStorageError::BucketNotFound) => {
            (AuditOutcome::Denied, "storage_not_found")
        }
        Err(_) => (AuditOutcome::Failed, "storage_failed"),
    };
    let resource = if path.is_empty() {
        bucket_id.to_owned()
    } else {
        format!("{bucket_id}/{path}")
    };
    let _ = auth_http::append_audit(
        graph,
        tenant,
        AuditCategory::Policy,
        actor,
        "object",
        &resource,
        action,
        result,
        reason,
        request.request_id(),
        now,
    )
    .await;
}

fn storage_error(request: &HttpRequest, error: FileStorageError) -> HttpApiError {
    let message = error.to_string();
    let (status, code, retry) = match error {
        FileStorageError::InvalidBucket(_)
        | FileStorageError::InvalidPath(_)
        | FileStorageError::InvalidPolicy(_)
        | FileStorageError::ContentTypeNotAllowed => {
            (400, ErrorCode::InvalidRequest, RetryAdvice::Never)
        }
        FileStorageError::ObjectTooLarge { .. } => {
            (413, ErrorCode::InvalidRequest, RetryAdvice::Never)
        }
        FileStorageError::BucketNotFound | FileStorageError::ObjectNotFound => {
            (404, ErrorCode::NotFound, RetryAdvice::Never)
        }
        FileStorageError::BucketNotEmpty | FileStorageError::Conflict => {
            (409, ErrorCode::Conflict, RetryAdvice::Immediate)
        }
        FileStorageError::PreconditionFailed => {
            (412, ErrorCode::PreconditionFailed, RetryAdvice::Never)
        }
        FileStorageError::StorageCapExceeded { .. } => {
            (429, ErrorCode::QuotaExceeded, RetryAdvice::Never)
        }
        FileStorageError::Denied(ref code) if code == "credential_required" => {
            return unauthenticated(request, "a credential is required for this bucket");
        }
        FileStorageError::Denied(_) => (403, ErrorCode::PermissionDenied, RetryAdvice::Never),
        FileStorageError::Corrupt => (500, ErrorCode::Internal, RetryAdvice::Never),
        FileStorageError::Storage | FileStorageError::ObjectStore => {
            (503, ErrorCode::Unavailable, RetryAdvice::Immediate)
        }
    };
    HttpApiError::new(status, code, &message, request.request_id(), retry)
}

#[cfg(test)]
mod tests {
    use mako_api::ErrorCode;
    use mako_file_storage::ObjectPrecondition;
    use mako_service_runtime::{HttpMethod, HttpRequest};

    use super::{percent_decode, preconditions};

    /// An upload's conditional headers become the storage service's
    /// preconditions, and a condition the platform does not implement is
    /// refused rather than dropped -- a caller that asked for create-only
    /// must never be answered as if it had asked for an overwrite.
    #[test]
    fn conditional_upload_headers_become_preconditions_or_a_refusal() {
        let request = |headers: &[(&str, &str)]| {
            HttpRequest::for_test(
                HttpMethod::Put,
                "/v1/projects/prj_example00/environments/env_example00/storage/receipts/objects/a.txt",
                headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
                Vec::new(),
                None,
            )
        };
        assert!(preconditions(&request(&[])).expect("none").is_empty());
        assert_eq!(
            preconditions(&request(&[("If-None-Match", "*")])).expect("create only"),
            vec![ObjectPrecondition::NotStored]
        );
        assert_eq!(
            preconditions(&request(&[("If-Match", "*")])).expect("replace only"),
            vec![ObjectPrecondition::Stored]
        );
        assert_eq!(
            preconditions(&request(&[("If-Match", " \"blake3:abc123\" ")])).expect("pinned"),
            vec![ObjectPrecondition::Digest("blake3:abc123".to_owned())]
        );
        for headers in [
            vec![("If-None-Match", "\"blake3:abc123\"")],
            vec![("If-None-Match", "")],
            vec![("If-Match", "blake3:abc123")],
            vec![("If-Match", "W/\"blake3:abc123\"")],
            vec![("If-Match", "\"a\", \"b\"")],
            vec![("If-Match", "\"\"")],
            vec![("If-Match", "*"), ("If-Match", "*")],
        ] {
            let error = preconditions(&request(&headers)).expect_err("refused");
            assert_eq!(
                error.envelope().error.code,
                ErrorCode::InvalidRequest,
                "{headers:?}"
            );
        }
    }

    #[test]
    fn object_paths_decode_percent_escapes_and_refuse_malformed_ones() {
        assert_eq!(
            percent_decode("users/42/me.png").as_deref(),
            Some("users/42/me.png")
        );
        assert_eq!(
            percent_decode("a%2Fb%20c.txt").as_deref(),
            Some("a/b c.txt")
        );
        assert_eq!(
            percent_decode("caf%C3%A9.txt").as_deref(),
            Some("caf\u{e9}.txt")
        );
        assert!(percent_decode("bad%2").is_none());
        assert!(percent_decode("bad%zz").is_none());
        assert!(percent_decode("%FF%FE").is_none(), "not UTF-8");
    }
}
