//! Management routes for application file storage buckets.
//!
//! Buckets and their objects live in the data plane that serves the
//! environment; the control plane keeps no bucket state of its own. Every
//! route here verifies the developer, maps their team role onto identity-admin
//! permissions, forwards exactly one identity-admin operation (two for a
//! reconfiguration, which reads before it writes), and renders the data
//! plane's answer on the management wire. A refusal the data plane phrases for
//! a developer crosses verbatim, so an invalid rule or path reads the same
//! here as it does where it was rejected.

use std::sync::Arc;

use mako_api::{ErrorCode, TenantScope};
use mako_control_plane::DeveloperPrincipal;
use mako_internal_rpc::{IdentityAdminCommand, IdentityAdminOperation, InternalClientError};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    ControlPlaneGraph,
    http_support::{query_value, reject_unknown_query, tenant},
    identity_admin_http::identity_permissions,
    management_http::{
        conflict, format_timestamp, invalid, json, no_payload, no_query, parse_json,
        require_confirmation, require_idempotency, require_json, unavailable, with_developer,
    },
};

/// What a bucket allows per object unless it says otherwise: 1 MiB.
const DEFAULT_MAX_OBJECT_BYTES: u64 = 1_048_576;
/// The platform ceiling on one object: 16 MiB.
const MAX_OBJECT_BYTES_CEILING: u64 = 16_777_216;
const MAX_OBJECT_PATH_BYTES: usize = 1024;
const MAX_CURSOR_BYTES: usize = 4096;
const DEFAULT_OBJECT_LIST_LIMIT: u64 = 100;
const MAX_OBJECT_LIST_LIMIT: u64 = 1000;

pub(crate) fn add_storage_bucket_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/storage-buckets",
            handle_list_buckets as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/storage-buckets",
            handle_create_bucket,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/storage-buckets/{bucketId}",
            handle_get_bucket,
        ),
        (
            HttpMethod::Patch,
            "/v1/projects/{projectId}/environments/{environmentId}/storage-buckets/{bucketId}",
            handle_update_bucket,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/storage-buckets/{bucketId}",
            handle_delete_bucket,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/storage-buckets/{bucketId}/objects",
            handle_list_objects,
        ),
        (
            HttpMethod::Delete,
            // An object path keeps its slashes; the trailing rest segment binds
            // everything after `/objects/`.
            "/v1/projects/{projectId}/environments/{environmentId}/storage-buckets/{bucketId}/objects/{objectPath...}",
            handle_delete_object,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_list_buckets(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, _| async move {
        let listing: BucketListing = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::ListBuckets,
            json!({}),
            false,
        )
        .await?;
        let mut items = Vec::with_capacity(listing.items.len());
        for entry in &listing.items {
            items.push(bucket_wire(request, &entry.bucket, &entry.totals)?);
        }
        json(request, 200, &ItemsWire { items })
    })
}

fn handle_create_bucket(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let body: CreateBucketWire = parse_json(request)?;
    let config = body.into_config(request)?;
    with_developer(graph, request, |actor, _| async move {
        // Installing an id the data plane already holds would reconfigure it,
        // and a second POST is not how a bucket gets reconfigured. The one
        // exception is a replay that carries the configuration already in
        // place, which converges on the bucket it created.
        if let Some(existing) = inspect_bucket(graph, request, &actor, &tenant, &config.id).await? {
            if existing.bucket.config == config {
                let wire = bucket_wire(request, &existing.bucket, &existing.totals)?;
                return json(request, 201, &wire);
            }
            return Err(conflict(request, "storage bucket already exists"));
        }
        let record: BucketRecord = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::InstallBucket,
            json!({ "bucket": config }),
            true,
        )
        .await?;
        json(
            request,
            201,
            &bucket_wire(request, &record, &BucketTotals::default())?,
        )
    })
}

fn handle_get_bucket(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let bucket_id = bucket_id(request)?;
    with_developer(graph, request, |actor, _| async move {
        let inspected: BucketWithTotals = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::InspectBucket,
            json!({ "bucketId": bucket_id }),
            false,
        )
        .await?;
        json(
            request,
            200,
            &bucket_wire(request, &inspected.bucket, &inspected.totals)?,
        )
    })
}

fn handle_update_bucket(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let bucket_id = bucket_id(request)?;
    let body: UpdateBucketWire = parse_json(request)?;
    if body.is_empty() {
        return Err(invalid(request, "storage bucket update changes nothing"));
    }
    if let Some(max_object_bytes) = body.max_object_bytes {
        validate_max_object_bytes(request, max_object_bytes)?;
    }
    with_developer(graph, request, |actor, _| async move {
        // The data plane installs a whole configuration, so the current one
        // is read and the requested fields laid over it. Object totals do not
        // move with a reconfiguration, so the inspection's totals still hold.
        let current: BucketWithTotals = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::InspectBucket,
            json!({ "bucketId": bucket_id }),
            false,
        )
        .await?;
        let config = body.apply_to(current.bucket.config);
        let record: BucketRecord = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::InstallBucket,
            json!({ "bucket": config }),
            true,
        )
        .await?;
        json(
            request,
            200,
            &bucket_wire(request, &record, &current.totals)?,
        )
    })
}

fn handle_delete_bucket(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_body(request)?;
    require_confirmation(request)?;
    reject_unknown_query(request, &["deleteObjects"])?;
    let delete_objects = match query_value(request, "deleteObjects")? {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(invalid(request, "deleteObjects query is invalid")),
    };
    let tenant = tenant(request)?;
    let bucket_id = bucket_id(request)?;
    with_developer(graph, request, |actor, _| async move {
        let removal: StorageBucketRemovalWire = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::RemoveBucket,
            json!({ "bucketId": bucket_id, "deleteObjects": delete_objects }),
            false,
        )
        .await?;
        json(request, 200, &removal)
    })
}

fn handle_list_objects(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_body(request)?;
    reject_unknown_query(request, &["prefix", "limit", "cursor"])?;
    let prefix = query_value(request, "prefix")?.map(str::to_owned);
    if prefix.as_deref().is_some_and(|value| {
        value.is_empty()
            || value.len() > MAX_OBJECT_PATH_BYTES
            || value.chars().any(char::is_control)
    }) {
        return Err(invalid(request, "storage object prefix is invalid"));
    }
    let limit = match query_value(request, "limit")? {
        Some(value) => value
            .parse::<u64>()
            .ok()
            .filter(|limit| (1..=MAX_OBJECT_LIST_LIMIT).contains(limit))
            .ok_or_else(|| invalid(request, "storage object limit is invalid"))?,
        None => DEFAULT_OBJECT_LIST_LIMIT,
    };
    let cursor = query_value(request, "cursor")?.map(str::to_owned);
    if cursor.as_deref().is_some_and(|value| {
        value.is_empty() || value.len() > MAX_CURSOR_BYTES || value.chars().any(char::is_control)
    }) {
        return Err(invalid(request, "storage object cursor is invalid"));
    }
    let tenant = tenant(request)?;
    let bucket_id = bucket_id(request)?;
    with_developer(graph, request, |actor, _| async move {
        let listing: ObjectListing = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::ListBucketObjects,
            json!({
                "bucketId": bucket_id,
                "prefix": prefix,
                "limit": limit,
                "cursor": cursor,
            }),
            false,
        )
        .await?;
        let mut items = Vec::with_capacity(listing.items.len());
        for record in &listing.items {
            items.push(object_wire(request, record)?);
        }
        json(
            request,
            200,
            &StorageObjectPageWire {
                items,
                next_cursor: listing.next_cursor,
            },
        )
    })
}

fn handle_delete_object(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let bucket_id = bucket_id(request)?;
    let path = object_path(request)?;
    with_developer(graph, request, |actor, _| async move {
        let record: ObjectRecord = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::DeleteBucketObject,
            json!({ "bucketId": bucket_id, "path": path }),
            false,
        )
        .await?;
        json(request, 200, &object_wire(request, &record)?)
    })
}

/// Forward one identity-admin operation as the developer, with the
/// permissions their team role grants, and read back the shape the data
/// plane promised for it.
/// The environment's stored objects in one line, for the workspace summary's
/// backup inventory: how many buckets, objects, and bytes a recovery would
/// have to bring back. `None` when the developer may not read buckets or the
/// data plane cannot answer; the summary then says nothing rather than zero.
pub(crate) async fn object_storage_totals(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
) -> Option<Value> {
    let listed: Value = administer(
        graph,
        request,
        actor,
        tenant,
        IdentityAdminOperation::ListBuckets,
        serde_json::json!({}),
        false,
    )
    .await
    .ok()?;
    let items = listed.get("items")?.as_array()?;
    let mut object_count = 0u64;
    let mut total_bytes = 0u64;
    for item in items {
        let totals = item.get("totals")?;
        object_count = object_count.saturating_add(totals.get("objectCount")?.as_u64()?);
        total_bytes = total_bytes.saturating_add(totals.get("totalBytes")?.as_u64()?);
    }
    Some(serde_json::json!({
        "bucketCount": items.len(),
        "objectCount": object_count,
        "totalBytes": total_bytes,
    }))
}

async fn administer<T: DeserializeOwned>(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    operation: IdentityAdminOperation,
    input: Value,
    idempotent: bool,
) -> Result<T, HttpApiError> {
    let permissions = identity_permissions(graph, request, actor, tenant).await?;
    let command = IdentityAdminCommand {
        operation,
        actor_id: actor.identity_id().as_str().to_owned(),
        permissions,
        input,
    };
    let idempotency = if idempotent {
        require_idempotency(request)?
    } else {
        request.request_id()
    };
    let value: Value = graph
        .data_plane_identity_admin()
        .administer(tenant, request.request_id(), idempotency, &command)
        .map_err(|error| bucket_error(request, error))?;
    serde_json::from_value(value).map_err(|_| {
        unavailable(
            request,
            "storage bucket authority returned an unexpected response",
        )
    })
}

/// The bucket as the data plane holds it, or `None` when it holds no such
/// bucket.
async fn inspect_bucket(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    bucket_id: &str,
) -> Result<Option<BucketWithTotals>, HttpApiError> {
    match administer::<BucketWithTotals>(
        graph,
        request,
        actor,
        tenant,
        IdentityAdminOperation::InspectBucket,
        json!({ "bucketId": bucket_id }),
        false,
    )
    .await
    {
        Ok(inspected) => Ok(Some(inspected)),
        Err(error) if error.envelope().error.code == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The data plane names what was wrong with a bucket, a rule, or a path in
/// terms a developer can act on, and its envelope has already been correlated
/// to this request, so a client-side refusal crosses verbatim. An
/// authentication failure between the two planes is a deployment fault, not
/// the developer's, and reads as unavailability like every other one.
fn bucket_error(request: &HttpRequest, error: InternalClientError) -> HttpApiError {
    match error {
        InternalClientError::Remote { status, envelope }
            if (400..500).contains(&status) && status != 401 =>
        {
            HttpApiError::from_envelope(status, *envelope)
        }
        _ => unavailable(request, "storage bucket administration is unavailable"),
    }
}

fn no_body(request: &HttpRequest) -> Result<(), HttpApiError> {
    if request.body().is_empty() {
        Ok(())
    } else {
        Err(invalid(request, "request body is not supported"))
    }
}

fn bucket_id(request: &HttpRequest) -> Result<String, HttpApiError> {
    let value = request.path_parameter("bucketId").unwrap_or_default();
    if valid_bucket_id(value) {
        Ok(value.to_owned())
    } else {
        Err(invalid(request, "storage bucket path is invalid"))
    }
}

/// `^[a-z][a-z0-9-]{1,62}$`, as the public contract states it.
fn valid_bucket_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (2..=63).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn validate_max_object_bytes(request: &HttpRequest, value: u64) -> Result<(), HttpApiError> {
    if (1..=MAX_OBJECT_BYTES_CEILING).contains(&value) {
        Ok(())
    } else {
        Err(invalid(
            request,
            "maxObjectBytes must be between 1 and 16777216 bytes",
        ))
    }
}

fn object_path(request: &HttpRequest) -> Result<String, HttpApiError> {
    decode_object_path(request.path_parameter("objectPath").unwrap_or_default())
        .ok_or_else(|| invalid(request, "storage object path is invalid"))
}

/// Percent-decode the captured rest of the path and refuse anything that
/// could name a location the bucket does not contain: `.` and `..` segments,
/// empty segments, control characters, and malformed escapes.
fn decode_object_path(raw: &str) -> Option<String> {
    let decoded = percent_decode(raw)?;
    if decoded.is_empty()
        || decoded.len() > MAX_OBJECT_PATH_BYTES
        || decoded.chars().any(char::is_control)
        || decoded
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return None;
    }
    Some(decoded)
}

fn percent_decode(raw: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(raw.len());
    let mut input = raw.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = hex_value(input.next()?)?;
            let low = hex_value(input.next()?)?;
            bytes.push((high << 4) | low);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn bucket_wire(
    request: &HttpRequest,
    record: &BucketRecord,
    totals: &BucketTotals,
) -> Result<StorageBucketWire, HttpApiError> {
    Ok(StorageBucketWire {
        id: record.config.id.clone(),
        access: record.config.access,
        max_object_bytes: record.config.max_object_bytes,
        allowed_content_types: record.config.allowed_content_types.clone(),
        rules: record.config.rules.clone(),
        version: record.version,
        object_count: totals.object_count,
        total_bytes: totals.total_bytes,
        created_at: format_timestamp(request, record.created_at_unix_seconds)?,
        updated_at: format_timestamp(request, record.updated_at_unix_seconds)?,
    })
}

fn object_wire(
    request: &HttpRequest,
    record: &ObjectRecord,
) -> Result<StorageObjectWire, HttpApiError> {
    Ok(StorageObjectWire {
        path: record.path.clone(),
        content_type: record.content_type.clone(),
        size_bytes: record.size_bytes,
        owner_id: record.owner_id.clone(),
        digest: record.digest.clone(),
        created_at: format_timestamp(request, record.created_at_unix_seconds)?,
        updated_at: format_timestamp(request, record.updated_at_unix_seconds)?,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum BucketAccess {
    Policy,
    Public,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum RuleEffect {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ObjectOperation {
    Create,
    Read,
    Update,
    Delete,
}

/// One access rule, exactly as the developer wrote it and the data plane
/// installed it; the data plane validates the expression.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BucketRule {
    id: String,
    effect: RuleEffect,
    operations: Vec<ObjectOperation>,
    expression: String,
}

/// The configuration a bucket is installed with. This is the `bucket` input
/// of `InstallBucket` and the `config` of every bucket record the data plane
/// reports.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct BucketConfig {
    id: String,
    access: BucketAccess,
    max_object_bytes: u64,
    allowed_content_types: Vec<String>,
    rules: Vec<BucketRule>,
}

/// A bucket as the data plane reports it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BucketRecord {
    config: BucketConfig,
    version: u64,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BucketTotals {
    object_count: u64,
    total_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct BucketWithTotals {
    bucket: BucketRecord,
    totals: BucketTotals,
}

#[derive(Debug, Deserialize)]
struct BucketListing {
    items: Vec<BucketWithTotals>,
}

/// An object as the data plane reports it. The stored digest and bucket id
/// are the data plane's own bookkeeping and stay off the management wire.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectRecord {
    path: String,
    content_type: String,
    size_bytes: u64,
    #[serde(default)]
    owner_id: Option<String>,
    digest: String,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectListing {
    items: Vec<ObjectRecord>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateBucketWire {
    id: String,
    #[serde(default)]
    access: Option<BucketAccess>,
    #[serde(default)]
    max_object_bytes: Option<u64>,
    #[serde(default)]
    allowed_content_types: Option<Vec<String>>,
    #[serde(default)]
    rules: Option<Vec<BucketRule>>,
}

impl CreateBucketWire {
    fn into_config(self, request: &HttpRequest) -> Result<BucketConfig, HttpApiError> {
        if !valid_bucket_id(&self.id) {
            return Err(invalid(request, "storage bucket id is invalid"));
        }
        let max_object_bytes = self.max_object_bytes.unwrap_or(DEFAULT_MAX_OBJECT_BYTES);
        validate_max_object_bytes(request, max_object_bytes)?;
        Ok(BucketConfig {
            id: self.id,
            access: self.access.unwrap_or(BucketAccess::Policy),
            max_object_bytes,
            allowed_content_types: self.allowed_content_types.unwrap_or_default(),
            rules: self.rules.unwrap_or_default(),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateBucketWire {
    #[serde(default)]
    access: Option<BucketAccess>,
    #[serde(default)]
    max_object_bytes: Option<u64>,
    #[serde(default)]
    allowed_content_types: Option<Vec<String>>,
    #[serde(default)]
    rules: Option<Vec<BucketRule>>,
}

impl UpdateBucketWire {
    fn is_empty(&self) -> bool {
        self.access.is_none()
            && self.max_object_bytes.is_none()
            && self.allowed_content_types.is_none()
            && self.rules.is_none()
    }

    fn apply_to(self, current: BucketConfig) -> BucketConfig {
        BucketConfig {
            id: current.id,
            access: self.access.unwrap_or(current.access),
            max_object_bytes: self.max_object_bytes.unwrap_or(current.max_object_bytes),
            allowed_content_types: self
                .allowed_content_types
                .unwrap_or(current.allowed_content_types),
            rules: self.rules.unwrap_or(current.rules),
        }
    }
}

#[derive(Serialize)]
struct ItemsWire<T> {
    items: Vec<T>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageBucketWire {
    id: String,
    access: BucketAccess,
    max_object_bytes: u64,
    allowed_content_types: Vec<String>,
    rules: Vec<BucketRule>,
    version: u64,
    object_count: u64,
    total_bytes: u64,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageBucketRemovalWire {
    object_count: u64,
    total_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageObjectWire {
    path: String,
    content_type: String,
    size_bytes: u64,
    owner_id: Option<String>,
    digest: String,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageObjectPageWire {
    items: Vec<StorageObjectWire>,
    next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use mako_api::{ApiError, ApiErrorEnvelope, RetryAdvice};
    use serde_json::json;

    use super::*;

    fn request() -> HttpRequest {
        HttpRequest::for_test(HttpMethod::Get, "/", Vec::new(), Vec::new(), None)
    }

    #[test]
    fn object_paths_are_decoded_and_kept_inside_the_bucket() {
        assert_eq!(
            decode_object_path("users/42/me.png").as_deref(),
            Some("users/42/me.png")
        );
        assert_eq!(
            decode_object_path("users%2F42%2Fme%20avatar.png").as_deref(),
            Some("users/42/me avatar.png")
        );
        assert_eq!(decode_object_path("me.png").as_deref(), Some("me.png"));
        for escaping in [
            "..",
            "users/../secrets.txt",
            "users/%2E%2E/secrets.txt",
            "%2e%2e/secrets.txt",
            "users/./me.png",
            "users//me.png",
            "/users/me.png",
            "users/me.png/",
            "users/me%00.png",
            "users/me%0a.png",
            "users/me%zz.png",
            "users/me%2",
            "users/%ff.png",
            "",
        ] {
            assert!(
                decode_object_path(escaping).is_none(),
                "{escaping:?} must be refused"
            );
        }
        let long = "a".repeat(MAX_OBJECT_PATH_BYTES);
        assert!(decode_object_path(&long).is_some());
        assert!(decode_object_path(&format!("{long}b")).is_none());
    }

    #[test]
    fn bucket_ids_follow_the_public_pattern() {
        for valid in ["ab", "avatars", "user-uploads-2", "a1"] {
            assert!(valid_bucket_id(valid), "{valid:?} is a bucket id");
        }
        for invalid in [
            "a",
            "",
            "Avatars",
            "1avatars",
            "-avatars",
            "avatars_v2",
            "avatars/x",
        ] {
            assert!(!valid_bucket_id(invalid), "{invalid:?} is not a bucket id");
        }
        assert!(valid_bucket_id(&format!("a{}", "b".repeat(62))));
        assert!(!valid_bucket_id(&format!("a{}", "b".repeat(63))));
    }

    #[test]
    fn a_create_request_fills_the_documented_defaults_and_installs_the_config_verbatim() {
        let body: CreateBucketWire =
            serde_json::from_value(json!({ "id": "avatars" })).expect("minimal body");
        let config = body.into_config(&request()).expect("defaults");
        assert_eq!(
            serde_json::to_value(&config).expect("config"),
            json!({
                "id": "avatars",
                "access": "policy",
                "maxObjectBytes": 1_048_576,
                "allowedContentTypes": [],
                "rules": [],
            })
        );

        let body: CreateBucketWire = serde_json::from_value(json!({
            "id": "avatars",
            "access": "public",
            "maxObjectBytes": 16_777_216,
            "allowedContentTypes": ["image/*"],
            "rules": [{
                "id": "owner-writes",
                "effect": "allow",
                "operations": ["create", "update", "delete"],
                "expression": "old.owner_id == identity.user_id",
            }],
        }))
        .expect("full body");
        let config = body.into_config(&request()).expect("explicit settings");
        assert_eq!(config.access, BucketAccess::Public);
        assert_eq!(config.rules[0].operations.len(), 3);

        let over: CreateBucketWire =
            serde_json::from_value(json!({ "id": "avatars", "maxObjectBytes": 16_777_217 }))
                .expect("body");
        assert_eq!(
            over.into_config(&request())
                .expect_err("above the ceiling")
                .envelope()
                .error
                .message,
            "maxObjectBytes must be between 1 and 16777216 bytes"
        );
        let bad_id: CreateBucketWire =
            serde_json::from_value(json!({ "id": "Avatars" })).expect("body");
        assert!(bad_id.into_config(&request()).is_err());
        assert!(
            serde_json::from_value::<CreateBucketWire>(json!({ "id": "avatars", "extra": 1 }))
                .is_err(),
            "unknown fields are refused"
        );
    }

    #[test]
    fn an_update_lays_its_fields_over_the_installed_configuration() {
        let current = BucketConfig {
            id: "avatars".to_owned(),
            access: BucketAccess::Policy,
            max_object_bytes: 1_048_576,
            allowed_content_types: vec!["image/*".to_owned()],
            rules: vec![BucketRule {
                id: "owner-reads".to_owned(),
                effect: RuleEffect::Allow,
                operations: vec![ObjectOperation::Read],
                expression: "old.owner_id == identity.user_id".to_owned(),
            }],
        };
        let empty: UpdateBucketWire = serde_json::from_value(json!({})).expect("empty");
        assert!(empty.is_empty());
        let update: UpdateBucketWire =
            serde_json::from_value(json!({ "access": "public", "maxObjectBytes": 2048 }))
                .expect("update");
        assert!(!update.is_empty());
        let merged = update.apply_to(current.clone());
        assert_eq!(merged.id, "avatars");
        assert_eq!(merged.access, BucketAccess::Public);
        assert_eq!(merged.max_object_bytes, 2048);
        assert_eq!(merged.allowed_content_types, current.allowed_content_types);
        assert_eq!(merged.rules, current.rules);
        let cleared: UpdateBucketWire =
            serde_json::from_value(json!({ "rules": [], "allowedContentTypes": [] }))
                .expect("cleared");
        let merged = cleared.apply_to(current);
        assert!(merged.rules.is_empty());
        assert!(merged.allowed_content_types.is_empty());
    }

    #[test]
    fn data_plane_records_render_on_the_management_wire() {
        let inspected: BucketWithTotals = serde_json::from_value(json!({
            "bucket": {
                "config": {
                    "id": "avatars",
                    "access": "policy",
                    "maxObjectBytes": 1_048_576,
                    "allowedContentTypes": ["image/*"],
                    "rules": [{
                        "id": "owner-reads",
                        "effect": "allow",
                        "operations": ["read"],
                        "expression": "old.owner_id == identity.user_id",
                    }],
                },
                "version": 3,
                "createdAtUnixSeconds": 1_786_579_200,
                "updatedAtUnixSeconds": 1_786_582_800,
                "somethingNewer": true,
            },
            "totals": { "objectCount": 3, "totalBytes": 12_345 },
        }))
        .expect("inspect shape");
        let wire = bucket_wire(&request(), &inspected.bucket, &inspected.totals).expect("wire");
        assert_eq!(
            serde_json::to_value(&wire).expect("bucket wire"),
            json!({
                "id": "avatars",
                "access": "policy",
                "maxObjectBytes": 1_048_576,
                "allowedContentTypes": ["image/*"],
                "rules": [{
                    "id": "owner-reads",
                    "effect": "allow",
                    "operations": ["read"],
                    "expression": "old.owner_id == identity.user_id",
                }],
                "version": 3,
                "objectCount": 3,
                "totalBytes": 12_345,
                "createdAt": "2026-08-13T00:00:00Z",
                "updatedAt": "2026-08-13T01:00:00Z",
            })
        );

        let listing: ObjectListing = serde_json::from_value(json!({
            "items": [{
                "bucketId": "avatars",
                "path": "users/42/me.png",
                "contentType": "image/png",
                "sizeBytes": 512,
                "ownerId": null,
                "digest": "sha256:abc",
                "storedDigest": "sha256:def",
                "createdAtUnixSeconds": 1_786_579_200,
                "updatedAtUnixSeconds": 1_786_579_200,
            }],
            "nextCursor": null,
        }))
        .expect("listing shape");
        let object = object_wire(&request(), &listing.items[0]).expect("object wire");
        assert_eq!(
            serde_json::to_value(&object).expect("object wire"),
            json!({
                "path": "users/42/me.png",
                "contentType": "image/png",
                "sizeBytes": 512,
                "ownerId": null,
                "digest": "sha256:abc",
                "createdAt": "2026-08-13T00:00:00Z",
                "updatedAt": "2026-08-13T00:00:00Z",
            })
        );
        let page = StorageObjectPageWire {
            items: vec![object],
            next_cursor: listing.next_cursor,
        };
        assert_eq!(
            serde_json::to_value(&page).expect("page wire")["nextCursor"],
            Value::Null
        );
    }

    #[test]
    fn a_data_plane_refusal_crosses_verbatim_but_a_failure_reads_as_unavailable() {
        let refused = bucket_error(
            &request(),
            InternalClientError::Remote {
                status: 400,
                envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                    ErrorCode::InvalidRequest,
                    "rule owner-reads: expression references unknown field",
                    "req_test_support",
                    RetryAdvice::Never,
                ))),
            },
        );
        assert_eq!(refused.envelope().error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            refused.envelope().error.message,
            "rule owner-reads: expression references unknown field"
        );

        let conflicted = bucket_error(
            &request(),
            InternalClientError::Remote {
                status: 409,
                envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                    ErrorCode::Conflict,
                    "bucket holds 3 objects; pass deleteObjects to remove them",
                    "req_test_support",
                    RetryAdvice::Never,
                ))),
            },
        );
        assert_eq!(conflicted.envelope().error.code, ErrorCode::Conflict);

        for failure in [
            InternalClientError::Unavailable,
            InternalClientError::InvalidResponse,
            InternalClientError::Remote {
                status: 401,
                envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                    ErrorCode::Unauthenticated,
                    "internal caller is not authenticated",
                    "req_test_support",
                    RetryAdvice::Never,
                ))),
            },
            InternalClientError::Remote {
                status: 500,
                envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                    ErrorCode::Internal,
                    "storage failed",
                    "req_test_support",
                    RetryAdvice::Never,
                ))),
            },
        ] {
            let mapped = bucket_error(&request(), failure);
            assert_eq!(mapped.envelope().error.code, ErrorCode::Unavailable);
            assert_eq!(
                mapped.envelope().error.message,
                "storage bucket administration is unavailable"
            );
        }
    }
}
