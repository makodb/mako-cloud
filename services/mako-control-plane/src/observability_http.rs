use std::sync::Arc;

use mako_api::ObservabilityPage;
use mako_control_plane::{ObservabilityError, ObservabilityQuery, ObservabilitySignal};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    ControlPlaneGraph,
    http_support::{public_value, query_value, reject_unknown_query, tenant},
    management_http::{forbidden, internal, invalid, not_found, unavailable, with_developer},
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
        let page =
            public_page(&page).map_err(|_| internal(request, "response serialization failed"))?;
        public_value(request, 200, page)
    })
}

/// A page with its payloads named as the API's `…Signal` schemas name them.
///
/// Payloads are stored with their fields in snake case (the payload enum
/// renames its variants, not their fields), and the page was answered as it
/// was stored: `actor_id`, `request_id`, `collection_id`, `correlation_id`,
/// where the schemas and every client read `actorId`, `requestId`, ... -- so
/// the console showed those as blank. They are renamed here, on the way out,
/// so stored records keep their shape; `retryAfterUnixMilliseconds` then
/// becomes the schema's `retryAfter` timestamp like any other.
fn public_page(page: &ObservabilityPage) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(page)?;
    if let Some(Value::Array(items)) = value.get_mut("items") {
        for item in items {
            if let Some(Value::Object(payload)) = item.get_mut("payload") {
                *payload = std::mem::take(payload)
                    .into_iter()
                    .map(|(name, field)| (camel_case(&name), field))
                    .collect();
            }
        }
    }
    Ok(value)
}

fn camel_case(name: &str) -> String {
    let mut parts = name.split('_');
    let mut renamed = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut characters = part.chars();
        if let Some(first) = characters.next() {
            renamed.extend(first.to_uppercase());
            renamed.push_str(characters.as_str());
        }
    }
    renamed
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

#[cfg(test)]
mod tests {
    use mako_api::{
        EnvironmentId, EventOutcome, ObservabilityPage, ObservabilityPayload, ObservabilityRecord,
        ProjectId, QuotaResource, RetentionWindow, TenantScope,
    };

    use super::public_page;

    #[test]
    fn payload_fields_are_answered_in_the_schema_case() {
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").unwrap(),
            EnvironmentId::parse("env_abcdefgh").unwrap(),
        );
        let record = |payload| ObservabilityRecord {
            tenant: tenant.clone(),
            timestamp_unix_milliseconds: 1_000,
            payload,
        };
        let page = ObservabilityPage {
            items: vec![
                record(ObservabilityPayload::Audit {
                    organization_id: "org_abcdefgh".to_owned(),
                    actor_id: "dev_abcdefgh".to_owned(),
                    action: "function_read".to_owned(),
                    target: "control_resource/res_abcdefgh".to_owned(),
                    outcome: EventOutcome::Allowed,
                    request_id: "req_abcdefgh".to_owned(),
                    details: None,
                }),
                record(ObservabilityPayload::Quota {
                    resource: QuotaResource::ReplicationRequestsPerMinute,
                    limit: 10,
                    consumed: 11,
                    retry_after_unix_milliseconds: Some(2_000),
                }),
            ],
            next_cursor: None,
            retention: RetentionWindow {
                observed_at_unix_milliseconds: 10_000,
                retained_from_unix_milliseconds: 0,
                retention_seconds: 60,
            },
        };
        let value = public_page(&page).expect("page serializes");
        let audit = &value["items"][0]["payload"];
        assert_eq!(audit["actorId"], "dev_abcdefgh");
        assert_eq!(audit["requestId"], "req_abcdefgh");
        assert_eq!(audit["teamId"], "org_abcdefgh");
        assert!(audit.get("actor_id").is_none() && audit.get("request_id").is_none());
        let quota = &value["items"][1]["payload"];
        assert_eq!(quota["retryAfterUnixMilliseconds"], 2_000);
        assert!(quota.get("retry_after_unix_milliseconds").is_none());
        assert_eq!(value["items"][0]["timestampUnixMilliseconds"], 1_000);
    }
}
