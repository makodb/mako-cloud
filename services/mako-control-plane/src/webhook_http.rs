//! Management routes for database webhooks: endpoints, their signing
//! secrets, and their delivery logs.
//!
//! The records live in the control store and the worker that delivers them
//! runs in this process, so every route here is a thin rendering of the
//! webhook service: verify the developer, hand the call to the service with
//! the request's tenant, and put the answer on the management wire in the
//! shapes the OpenAPI document promises.

use std::{num::NonZeroUsize, sync::Arc};

use mako_api::{ErrorCode, RetryAdvice};
use mako_control_plane::{
    NewWebhookEndpoint, WebhookDeliveryId, WebhookDeliveryQuery, WebhookDeliveryRecord,
    WebhookDeliveryState, WebhookEndpointCreated, WebhookEndpointId, WebhookEndpointRecord,
    WebhookEndpointState, WebhookEndpointUpdate, WebhookError, WebhookEvent, WebhookSubscription,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlPlaneGraph,
    http_support::{query_value, reject_unknown_query, tenant},
    management_http::{
        conflict, forbidden, format_timestamp, invalid, json, no_payload, no_query, not_found,
        parse_json, require_idempotency, require_json, unavailable, with_developer,
    },
};

const DEFAULT_DELIVERY_LIMIT: usize = 50;
const MAXIMUM_DELIVERY_LIMIT: usize = 200;
const MAXIMUM_CURSOR_BYTES: usize = 256;

pub(crate) fn add_webhook_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks",
            handle_list as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks",
            handle_create,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks/{webhookId}",
            handle_get,
        ),
        (
            HttpMethod::Patch,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks/{webhookId}",
            handle_update,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks/{webhookId}",
            handle_delete,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks/{webhookId}/actions/rotate-secret",
            handle_rotate_secret,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks/{webhookId}/actions/resume",
            handle_resume,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks/{webhookId}/deliveries",
            handle_list_deliveries,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/webhooks/{webhookId}/deliveries/{deliveryId}/actions/redeliver",
            handle_redeliver,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_list(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let endpoints = graph
            .webhook_service()
            .list(&actor, &tenant, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        let mut items = Vec::with_capacity(endpoints.len());
        for endpoint in &endpoints {
            items.push(endpoint_wire(request, endpoint)?);
        }
        json(request, 200, &ItemsWire { items })
    })
}

fn handle_create(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let body: CreateWire = parse_json(request)?;
    let input = NewWebhookEndpoint {
        url: body.url,
        description: body.description,
        subscriptions: body.subscriptions,
        enabled: body.enabled.unwrap_or(true),
    };
    with_developer(graph, request, |actor, now| async move {
        let created = graph
            .webhook_service()
            .create(&actor, &tenant, input, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        json(request, 201, &created_wire(request, &created)?)
    })
}

fn handle_get(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let id = webhook_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let endpoint = graph
            .webhook_service()
            .get(&actor, &tenant, &id, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        json(request, 200, &endpoint_wire(request, &endpoint)?)
    })
}

fn handle_update(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let id = webhook_id(request)?;
    let body: UpdateWire = parse_json(request)?;
    let update = WebhookEndpointUpdate {
        url: body.url,
        description: body.description,
        subscriptions: body.subscriptions,
        enabled: body.enabled,
    };
    if update.is_empty() {
        return Err(invalid(request, "webhook endpoint update changes nothing"));
    }
    with_developer(graph, request, |actor, now| async move {
        let endpoint = graph
            .webhook_service()
            .update(&actor, &tenant, &id, update, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        json(request, 200, &endpoint_wire(request, &endpoint)?)
    })
}

fn handle_delete(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    no_body(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let id = webhook_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        graph
            .webhook_service()
            .delete(&actor, &tenant, &id, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        Ok(HttpResponse::empty(204))
    })
}

fn handle_rotate_secret(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    no_body(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let id = webhook_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let rotated = graph
            .webhook_service()
            .rotate_secret(&actor, &tenant, &id, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        json(request, 200, &created_wire(request, &rotated)?)
    })
}

fn handle_resume(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    no_body(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let id = webhook_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let endpoint = graph
            .webhook_service()
            .resume(&actor, &tenant, &id, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        json(request, 200, &endpoint_wire(request, &endpoint)?)
    })
}

fn handle_list_deliveries(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_body(request)?;
    reject_unknown_query(request, &["state", "cursor", "limit"])?;
    let state = query_value(request, "state")?
        .map(|value| {
            WebhookDeliveryState::parse(value)
                .ok_or_else(|| invalid(request, "state must be pending, delivered, or failed"))
        })
        .transpose()?;
    let limit = match query_value(request, "limit")? {
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAXIMUM_DELIVERY_LIMIT).contains(limit))
            .ok_or_else(|| invalid(request, "limit must be between 1 and 200"))?,
        None => DEFAULT_DELIVERY_LIMIT,
    };
    let limit = NonZeroUsize::new(limit).ok_or_else(|| invalid(request, "limit is invalid"))?;
    let cursor = query_value(request, "cursor")?.map(str::to_owned);
    if cursor.as_deref().is_some_and(|value| {
        value.is_empty()
            || value.len() > MAXIMUM_CURSOR_BYTES
            || value.chars().any(char::is_control)
    }) {
        return Err(invalid(request, "cursor is invalid"));
    }
    let tenant = tenant(request)?;
    let id = webhook_id(request)?;
    let query = WebhookDeliveryQuery {
        state,
        cursor,
        limit,
    };
    with_developer(graph, request, |actor, now| async move {
        let page = graph
            .webhook_service()
            .list_deliveries(&actor, &tenant, &id, &query, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        let mut items = Vec::with_capacity(page.items.len());
        for delivery in &page.items {
            items.push(delivery_wire(request, delivery)?);
        }
        json(
            request,
            200,
            &DeliveryPageWire {
                items,
                next_cursor: page.next_cursor,
            },
        )
    })
}

fn handle_redeliver(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    no_body(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let id = webhook_id(request)?;
    let delivery_id = delivery_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let delivery = graph
            .webhook_service()
            .redeliver(&actor, &tenant, &id, &delivery_id, now)
            .await
            .map_err(|error| webhook_error(request, error))?;
        json(request, 202, &delivery_wire(request, &delivery)?)
    })
}

fn no_body(request: &HttpRequest) -> Result<(), HttpApiError> {
    if request.body().is_empty() {
        Ok(())
    } else {
        Err(invalid(request, "request body is not supported"))
    }
}

fn webhook_id(request: &HttpRequest) -> Result<WebhookEndpointId, HttpApiError> {
    WebhookEndpointId::parse(request.path_parameter("webhookId").unwrap_or_default())
        .map_err(|_| invalid(request, "webhook endpoint path is invalid"))
}

fn delivery_id(request: &HttpRequest) -> Result<WebhookDeliveryId, HttpApiError> {
    WebhookDeliveryId::parse(request.path_parameter("deliveryId").unwrap_or_default())
        .map_err(|_| invalid(request, "webhook delivery path is invalid"))
}

/// The service names every refusal in terms a developer can act on; the
/// messages cross as they are. Storage and feed failures read as
/// unavailability.
fn webhook_error(request: &HttpRequest, error: WebhookError) -> HttpApiError {
    match error {
        WebhookError::EnvironmentNotFound => not_found(request, "environment was not found"),
        WebhookError::EndpointNotFound => not_found(request, "webhook endpoint was not found"),
        WebhookError::DeliveryNotFound => not_found(request, "webhook delivery was not found"),
        WebhookError::CollectionNotFound(collection) => HttpApiError::new(
            404,
            ErrorCode::NotFound,
            format!("collection {collection} was not found"),
            request.request_id(),
            RetryAdvice::Never,
        ),
        WebhookError::Forbidden => forbidden(request, "webhook operation is forbidden"),
        WebhookError::Conflict(message) => conflict(request, message),
        WebhookError::InvalidRequest(message) => invalid(request, message),
        WebhookError::Source(_) => unavailable(request, "collection change feed is unavailable"),
        _ => unavailable(request, "webhook service is unavailable"),
    }
}

fn endpoint_wire(
    request: &HttpRequest,
    endpoint: &WebhookEndpointRecord,
) -> Result<EndpointWire, HttpApiError> {
    Ok(EndpointWire {
        id: endpoint.id().as_str().to_owned(),
        url: endpoint.url().to_owned(),
        description: endpoint.description().to_owned(),
        subscriptions: endpoint.subscriptions().to_vec(),
        state: endpoint.state(),
        enabled: endpoint.enabled(),
        paused_reason: endpoint.paused_reason().map(str::to_owned),
        paused_at: endpoint
            .paused_at_unix_seconds()
            .map(|at| format_timestamp(request, at))
            .transpose()?,
        consecutive_failures: endpoint.consecutive_failures(),
        secret_version: endpoint.secret_version(),
        created_at: format_timestamp(request, endpoint.created_at_unix_seconds())?,
        updated_at: format_timestamp(request, endpoint.updated_at_unix_seconds())?,
    })
}

fn created_wire(
    request: &HttpRequest,
    created: &WebhookEndpointCreated,
) -> Result<CreatedWire, HttpApiError> {
    Ok(CreatedWire {
        endpoint: endpoint_wire(request, &created.endpoint)?,
        signing_secret: created.signing_secret.expose().to_owned(),
    })
}

fn delivery_wire(
    request: &HttpRequest,
    delivery: &WebhookDeliveryRecord,
) -> Result<DeliveryWire, HttpApiError> {
    Ok(DeliveryWire {
        id: delivery.id().as_str().to_owned(),
        endpoint_id: delivery.endpoint_id().as_str().to_owned(),
        event: delivery.event(),
        collection_id: delivery.collection_id().as_str().to_owned(),
        document_id: delivery.document_id().to_owned(),
        revision: delivery.revision().to_owned(),
        commit_position: delivery.commit_position(),
        state: delivery.state(),
        attempts: delivery.attempts(),
        next_attempt_at: delivery
            .next_attempt_at_unix_seconds()
            .map(|at| format_timestamp(request, at))
            .transpose()?,
        last_response_status: delivery.last_response_status(),
        last_error: delivery.last_error().map(str::to_owned),
        redelivery_of: delivery.redelivery_of().map(|id| id.as_str().to_owned()),
        created_at: format_timestamp(request, delivery.created_at_unix_seconds())?,
        delivered_at: delivery
            .delivered_at_unix_seconds()
            .map(|at| format_timestamp(request, at))
            .transpose()?,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateWire {
    url: String,
    #[serde(default)]
    description: Option<String>,
    subscriptions: Vec<WebhookSubscription>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateWire {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    subscriptions: Option<Vec<WebhookSubscription>>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Serialize)]
struct ItemsWire<T> {
    items: Vec<T>,
}

/// `WebhookEndpoint` on the wire.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EndpointWire {
    id: String,
    url: String,
    description: String,
    subscriptions: Vec<WebhookSubscription>,
    state: WebhookEndpointState,
    enabled: bool,
    paused_reason: Option<String>,
    paused_at: Option<String>,
    consecutive_failures: u64,
    secret_version: u64,
    created_at: String,
    updated_at: String,
}

/// `WebhookEndpointCreated`: the secret is shown here and nowhere else.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedWire {
    endpoint: EndpointWire,
    signing_secret: String,
}

/// `WebhookDelivery` on the wire.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryWire {
    id: String,
    endpoint_id: String,
    event: WebhookEvent,
    collection_id: String,
    document_id: String,
    revision: String,
    commit_position: u64,
    state: WebhookDeliveryState,
    attempts: u64,
    next_attempt_at: Option<String>,
    last_response_status: Option<u16>,
    last_error: Option<String>,
    redelivery_of: Option<String>,
    created_at: String,
    delivered_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryPageWire {
    items: Vec<DeliveryWire>,
    next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn create_and_update_bodies_follow_the_contract() {
        let create: CreateWire = serde_json::from_value(json!({
            "url": "https://hooks.example.test/mako",
            "subscriptions": [{ "collectionId": "todos", "events": ["insert", "delete"] }],
        }))
        .expect("minimal create");
        assert!(create.description.is_none());
        assert!(
            create.enabled.is_none(),
            "defaults to enabled at the service"
        );
        assert_eq!(
            create.subscriptions[0].events,
            [WebhookEvent::Insert, WebhookEvent::Delete]
        );
        assert!(
            serde_json::from_value::<CreateWire>(json!({
                "url": "https://hooks.example.test/mako",
                "subscriptions": [],
                "secret": "mine",
            }))
            .is_err(),
            "unknown fields are refused"
        );
        assert!(
            serde_json::from_value::<CreateWire>(json!({
                "url": "https://hooks.example.test/mako",
                "subscriptions": [{ "collectionId": "todos", "events": ["upsert"] }],
            }))
            .is_err(),
            "unknown events are refused"
        );
        let update: UpdateWire =
            serde_json::from_value(json!({ "enabled": false })).expect("update");
        assert_eq!(update.enabled, Some(false));
        assert!(update.url.is_none());
        let empty: UpdateWire = serde_json::from_value(json!({})).expect("empty");
        assert!(
            WebhookEndpointUpdate {
                url: empty.url,
                description: empty.description,
                subscriptions: empty.subscriptions,
                enabled: empty.enabled,
            }
            .is_empty()
        );
    }

    #[test]
    fn path_ids_are_validated_and_errors_map_to_the_public_codes() {
        let request = HttpRequest::for_test(
            HttpMethod::Get,
            "/v1/projects/prj_example00/environments/env_example00/webhooks/",
            Vec::new(),
            Vec::new(),
            None,
        );
        assert_eq!(
            webhook_id(&request)
                .expect_err("missing id")
                .envelope()
                .error
                .code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            delivery_id(&request)
                .expect_err("missing id")
                .envelope()
                .error
                .code,
            ErrorCode::InvalidRequest
        );
        for (error, code) in [
            (WebhookError::EnvironmentNotFound, ErrorCode::NotFound),
            (WebhookError::EndpointNotFound, ErrorCode::NotFound),
            (WebhookError::DeliveryNotFound, ErrorCode::NotFound),
            (
                WebhookError::CollectionNotFound("todos".to_owned()),
                ErrorCode::NotFound,
            ),
            (WebhookError::Forbidden, ErrorCode::PermissionDenied),
            (
                WebhookError::Conflict("webhook endpoint is paused; resume it first"),
                ErrorCode::Conflict,
            ),
            (
                WebhookError::InvalidRequest("url must use https"),
                ErrorCode::InvalidRequest,
            ),
            (WebhookError::CorruptRecord, ErrorCode::Unavailable),
        ] {
            assert_eq!(webhook_error(&request, error).envelope().error.code, code);
        }
        let paused = webhook_error(
            &request,
            WebhookError::Conflict("webhook endpoint is paused; resume it first"),
        );
        assert_eq!(
            paused.envelope().error.message,
            "webhook endpoint is paused; resume it first"
        );
        let missing = webhook_error(
            &request,
            WebhookError::CollectionNotFound("todos".to_owned()),
        );
        assert_eq!(
            missing.envelope().error.message,
            "collection todos was not found"
        );
    }
}
