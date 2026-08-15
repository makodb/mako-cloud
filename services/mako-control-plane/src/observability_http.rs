use std::sync::Arc;

use mako_control_plane::{ObservabilityError, ObservabilityQuery, ObservabilitySignal};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    ControlPlaneGraph,
    http_support::{public_json, query_value, reject_unknown_query, tenant},
    management_http::{forbidden, invalid, not_found, unavailable, with_developer},
};

pub(crate) fn add_observability_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (path, signal) in [
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/usage",
            ObservabilitySignal::Usage,
        ),
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/quotas",
            ObservabilitySignal::Quota,
        ),
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/health",
            ObservabilitySignal::Health,
        ),
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/replication-errors",
            ObservabilitySignal::ReplicationError,
        ),
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/auth-events",
            ObservabilitySignal::AuthenticationEvent,
        ),
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/function-metrics",
            ObservabilitySignal::FunctionMetric,
        ),
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/logs",
            ObservabilitySignal::ProjectLog,
        ),
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/index-states",
            ObservabilitySignal::IndexState,
        ),
        (
            "/v1/projects/{projectId}/environments/{environmentId}/observability/audit-events",
            ObservabilitySignal::Audit,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(HttpMethod::Get, path, move |request| {
            handle_observability(&graph, &request, signal)
        })?;
    }
    Ok(())
}

fn handle_observability(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    signal: ObservabilitySignal,
) -> Result<HttpResponse, HttpApiError> {
    if !request.body().is_empty() {
        return Err(invalid(request, "request body is not supported"));
    }
    reject_unknown_query(request, &["cursor", "from", "until", "limit"])?;
    let query = ObservabilityQuery {
        cursor: query_value(request, "cursor")?.map(str::to_owned),
        from_unix_milliseconds: query_timestamp(request, "from")?,
        until_unix_milliseconds: query_timestamp(request, "until")?,
        limit: match query_value(request, "limit")? {
            Some(value) => value
                .parse::<usize>()
                .map_err(|_| invalid(request, "observability limit is invalid"))?,
            None => 100,
        },
    };
    if !(1..=1_000).contains(&query.limit)
        || query.cursor.as_ref().is_some_and(|cursor| {
            cursor.is_empty() || cursor.len() > 4_096 || cursor.chars().any(char::is_control)
        })
    {
        return Err(invalid(request, "observability query is invalid"));
    }
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let now_milliseconds = now.saturating_mul(1_000);
        let service = graph.observability_service();
        let page = match signal {
            ObservabilitySignal::Usage => {
                service
                    .query_usage(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
            ObservabilitySignal::Quota => {
                service
                    .query_quotas(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
            ObservabilitySignal::Health => {
                service
                    .query_health(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
            ObservabilitySignal::ReplicationError => {
                service
                    .query_replication_errors(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
            ObservabilitySignal::AuthenticationEvent => {
                service
                    .query_authentication_events(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
            ObservabilitySignal::FunctionMetric => {
                service
                    .query_function_metrics(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
            ObservabilitySignal::ProjectLog => {
                service
                    .query_logs(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
            ObservabilitySignal::IndexState => {
                service
                    .query_index_states(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
            ObservabilitySignal::Audit => {
                service
                    .query_audit_events(&actor, &tenant, &query, now_milliseconds)
                    .await
            }
        }
        .map_err(|error| observability_error(request, error))?;
        public_json(request, 200, &page)
    })
}

fn query_timestamp(request: &HttpRequest, name: &str) -> Result<Option<u64>, HttpApiError> {
    query_value(request, name)?
        .map(|value| {
            OffsetDateTime::parse(value, &Rfc3339)
                .ok()
                .and_then(|time| u64::try_from(time.unix_timestamp_nanos() / 1_000_000).ok())
                .ok_or_else(|| invalid(request, "observability timestamp is invalid"))
        })
        .transpose()
}

fn observability_error(request: &HttpRequest, error: ObservabilityError) -> HttpApiError {
    match error {
        ObservabilityError::NotFound => not_found(request, "observability scope was not found"),
        ObservabilityError::Forbidden => forbidden(request, "observability query is forbidden"),
        ObservabilityError::InvalidQuery | ObservabilityError::RetentionExpired { .. } => invalid(
            request,
            "observability query is outside its valid retention window",
        ),
        _ => unavailable(request, "observability dependencies are unavailable"),
    }
}
