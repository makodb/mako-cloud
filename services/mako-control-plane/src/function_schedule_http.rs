//! Management routes for scheduled functions: a function's schedules and
//! their run histories.
//!
//! The records live in the control store and the worker that fires them
//! runs in this process, so every route here is a thin rendering of the
//! schedule service: verify the developer, hand the call to the service
//! with the request's tenant and function, and put the answer on the
//! management wire in the shapes the OpenAPI document promises.

use std::{num::NonZeroUsize, sync::Arc};

use mako_api::{ErrorCode, RetryAdvice};
use mako_control_plane::{
    FunctionName, FunctionScheduleError, FunctionScheduleId, FunctionScheduleRecord,
    FunctionScheduleRequest, FunctionScheduleRunOutcome, FunctionScheduleRunQuery,
    FunctionScheduleRunRecord, FunctionScheduleRunSummary, FunctionScheduleState,
    FunctionScheduleUpdate, NewFunctionSchedule,
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

const DEFAULT_RUN_LIMIT: usize = 50;
const MAXIMUM_RUN_LIMIT: usize = 200;
const MAXIMUM_CURSOR_BYTES: usize = 256;
const TIMEZONE: &str = "UTC";

pub(crate) fn add_function_schedule_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/schedules",
            handle_list as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/schedules",
            handle_create,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/schedules/{scheduleId}",
            handle_get,
        ),
        (
            HttpMethod::Patch,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/schedules/{scheduleId}",
            handle_update,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/schedules/{scheduleId}",
            handle_delete,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/schedules/{scheduleId}/actions/run-now",
            handle_run_now,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/schedules/{scheduleId}/runs",
            handle_list_runs,
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
    let function = function_name(request)?;
    with_developer(graph, request, |actor, now| async move {
        let schedules = graph
            .function_schedule_service()
            .list(&actor, &tenant, &function, now)
            .await
            .map_err(|error| schedule_error(request, error))?;
        let mut items = Vec::with_capacity(schedules.len());
        for schedule in &schedules {
            items.push(schedule_wire(request, schedule)?);
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
    let function = function_name(request)?;
    let body: CreateWire = parse_json(request)?;
    let input = NewFunctionSchedule {
        name: body.name,
        cron: body.cron,
        request: body.request,
        enabled: body.enabled.unwrap_or(true),
    };
    with_developer(graph, request, |actor, now| async move {
        let created = graph
            .function_schedule_service()
            .create(&actor, &tenant, &function, input, now)
            .await
            .map_err(|error| schedule_error(request, error))?;
        json(request, 201, &schedule_wire(request, &created)?)
    })
}

fn handle_get(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let function = function_name(request)?;
    let id = schedule_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let schedule = graph
            .function_schedule_service()
            .get(&actor, &tenant, &function, &id, now)
            .await
            .map_err(|error| schedule_error(request, error))?;
        json(request, 200, &schedule_wire(request, &schedule)?)
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
    let function = function_name(request)?;
    let id = schedule_id(request)?;
    let body: UpdateWire = parse_json(request)?;
    let update = FunctionScheduleUpdate {
        name: body.name,
        cron: body.cron,
        request: body.request,
        enabled: body.enabled,
    };
    if update.is_empty() {
        return Err(invalid(request, "function schedule update changes nothing"));
    }
    with_developer(graph, request, |actor, now| async move {
        let schedule = graph
            .function_schedule_service()
            .update(&actor, &tenant, &function, &id, update, now)
            .await
            .map_err(|error| schedule_error(request, error))?;
        json(request, 200, &schedule_wire(request, &schedule)?)
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
    let function = function_name(request)?;
    let id = schedule_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        graph
            .function_schedule_service()
            .delete(&actor, &tenant, &function, &id, now)
            .await
            .map_err(|error| schedule_error(request, error))?;
        Ok(HttpResponse::empty(204))
    })
}

fn handle_run_now(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    no_body(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let function = function_name(request)?;
    let id = schedule_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let run = graph
            .function_schedule_service()
            .run_now(&actor, &tenant, &function, &id, now)
            .await
            .map_err(|error| schedule_error(request, error))?;
        json(request, 202, &run_wire(request, &run)?)
    })
}

fn handle_list_runs(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_body(request)?;
    reject_unknown_query(request, &["outcome", "cursor", "limit"])?;
    let outcome = query_value(request, "outcome")?
        .map(|value| {
            FunctionScheduleRunOutcome::parse(value).ok_or_else(|| {
                invalid(
                    request,
                    "outcome must be succeeded, failed, error, or skipped_overlap",
                )
            })
        })
        .transpose()?;
    let limit = match query_value(request, "limit")? {
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAXIMUM_RUN_LIMIT).contains(limit))
            .ok_or_else(|| invalid(request, "limit must be between 1 and 200"))?,
        None => DEFAULT_RUN_LIMIT,
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
    let function = function_name(request)?;
    let id = schedule_id(request)?;
    let query = FunctionScheduleRunQuery {
        outcome,
        cursor,
        limit,
    };
    with_developer(graph, request, |actor, now| async move {
        let page = graph
            .function_schedule_service()
            .list_runs(&actor, &tenant, &function, &id, &query, now)
            .await
            .map_err(|error| schedule_error(request, error))?;
        let mut items = Vec::with_capacity(page.items.len());
        for run in &page.items {
            items.push(run_wire(request, run)?);
        }
        json(
            request,
            200,
            &RunPageWire {
                items,
                next_cursor: page.next_cursor,
            },
        )
    })
}

fn no_body(request: &HttpRequest) -> Result<(), HttpApiError> {
    if request.body().is_empty() {
        Ok(())
    } else {
        Err(invalid(request, "request body is not supported"))
    }
}

fn function_name(request: &HttpRequest) -> Result<FunctionName, HttpApiError> {
    FunctionName::parse(
        request
            .path_parameter("functionName")
            .unwrap_or_default()
            .to_owned(),
    )
    .map_err(|_| invalid(request, "function path is invalid"))
}

fn schedule_id(request: &HttpRequest) -> Result<FunctionScheduleId, HttpApiError> {
    FunctionScheduleId::parse(request.path_parameter("scheduleId").unwrap_or_default())
        .map_err(|_| invalid(request, "function schedule path is invalid"))
}

/// The service names every refusal in terms a developer can act on; the
/// messages cross as they are, including the field-addressed cron refusal.
/// Storage failures read as unavailability.
fn schedule_error(request: &HttpRequest, error: FunctionScheduleError) -> HttpApiError {
    match error {
        FunctionScheduleError::EnvironmentNotFound => {
            not_found(request, "environment was not found")
        }
        FunctionScheduleError::FunctionNotFound => not_found(request, "function was not found"),
        FunctionScheduleError::ScheduleNotFound => {
            not_found(request, "function schedule was not found")
        }
        FunctionScheduleError::Forbidden => {
            forbidden(request, "function schedule operation is forbidden")
        }
        FunctionScheduleError::Conflict(message) => conflict(request, message),
        FunctionScheduleError::InvalidRequest(message) => invalid(request, message),
        FunctionScheduleError::InvalidCron(cron) => HttpApiError::new(
            400,
            ErrorCode::InvalidRequest,
            cron.to_string(),
            request.request_id(),
            RetryAdvice::Never,
        ),
        _ => unavailable(request, "function schedule service is unavailable"),
    }
}

fn schedule_wire(
    request: &HttpRequest,
    schedule: &FunctionScheduleRecord,
) -> Result<ScheduleWire, HttpApiError> {
    Ok(ScheduleWire {
        id: schedule.id().as_str().to_owned(),
        function_name: schedule.function_name().as_str().to_owned(),
        name: schedule.name().to_owned(),
        cron: schedule.cron().to_owned(),
        timezone: TIMEZONE,
        request: schedule.request().clone(),
        enabled: schedule.enabled(),
        state: schedule.state(),
        next_run_at: schedule
            .next_run_at_unix_seconds()
            .map(|at| format_timestamp(request, at))
            .transpose()?,
        last_run: schedule
            .last_run()
            .map(|run| run_summary_wire(request, run))
            .transpose()?,
        created_at: format_timestamp(request, schedule.created_at_unix_seconds())?,
        updated_at: format_timestamp(request, schedule.updated_at_unix_seconds())?,
    })
}

fn run_summary_wire(
    request: &HttpRequest,
    run: &FunctionScheduleRunSummary,
) -> Result<RunSummaryWire, HttpApiError> {
    Ok(RunSummaryWire {
        id: run.id.as_str().to_owned(),
        due_at: format_timestamp(request, run.due_at_unix_seconds)?,
        outcome: run.outcome,
        duration_milliseconds: run.duration_milliseconds,
        response_status: run.response_status,
    })
}

fn run_wire(
    request: &HttpRequest,
    run: &FunctionScheduleRunRecord,
) -> Result<RunWire, HttpApiError> {
    Ok(RunWire {
        id: run.id().as_str().to_owned(),
        schedule_id: run.schedule_id().as_str().to_owned(),
        function_name: run.function_name().as_str().to_owned(),
        function_version: run.function_version(),
        due_at: format_timestamp(request, run.due_at_unix_seconds())?,
        started_at: run
            .started_at_unix_seconds()
            .map(|at| format_timestamp(request, at))
            .transpose()?,
        completed_at: run
            .completed_at_unix_seconds()
            .map(|at| format_timestamp(request, at))
            .transpose()?,
        duration_milliseconds: run.duration_milliseconds(),
        outcome: run.outcome(),
        response_status: run.response_status(),
        error: run.error().map(str::to_owned),
        manual: run.manual(),
        created_at: format_timestamp(request, run.created_at_unix_seconds())?,
    })
}

/// `FunctionScheduleCreate` on the wire.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateWire {
    #[serde(default)]
    name: Option<String>,
    cron: String,
    #[serde(default)]
    request: Option<FunctionScheduleRequest>,
    #[serde(default)]
    enabled: Option<bool>,
}

/// `FunctionScheduleUpdate` on the wire.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateWire {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cron: Option<String>,
    #[serde(default)]
    request: Option<FunctionScheduleRequest>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Serialize)]
struct ItemsWire<T> {
    items: Vec<T>,
}

/// `FunctionSchedule` on the wire.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleWire {
    id: String,
    function_name: String,
    name: String,
    cron: String,
    timezone: &'static str,
    request: FunctionScheduleRequest,
    enabled: bool,
    state: FunctionScheduleState,
    next_run_at: Option<String>,
    last_run: Option<RunSummaryWire>,
    created_at: String,
    updated_at: String,
}

/// `FunctionScheduleRunSummary` on the wire.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSummaryWire {
    id: String,
    due_at: String,
    outcome: FunctionScheduleRunOutcome,
    duration_milliseconds: Option<u64>,
    response_status: Option<u16>,
}

/// `FunctionScheduleRun` on the wire.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunWire {
    id: String,
    schedule_id: String,
    function_name: String,
    function_version: Option<u64>,
    due_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    duration_milliseconds: Option<u64>,
    outcome: Option<FunctionScheduleRunOutcome>,
    response_status: Option<u16>,
    error: Option<String>,
    manual: bool,
    created_at: String,
}

/// `FunctionScheduleRunPage` on the wire.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunPageWire {
    items: Vec<RunWire>,
    next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use mako_control_plane::{CronSchedule, FunctionScheduleMethod};
    use serde_json::json;

    use super::*;

    #[test]
    fn create_and_update_bodies_follow_the_contract() {
        let create: CreateWire =
            serde_json::from_value(json!({ "cron": "0 3 * * *" })).expect("minimal create");
        assert!(create.name.is_none());
        assert!(create.request.is_none());
        assert!(create.enabled.is_none(), "defaults to enabled at the route");
        let full: CreateWire = serde_json::from_value(json!({
            "name": "Nightly",
            "cron": "0 3 * * *",
            "request": {
                "method": "PUT",
                "path": "/reports?kind=daily",
                "headers": { "x-report": "nightly" },
                "contentType": "text/plain",
                "body": "hello",
            },
            "enabled": false,
        }))
        .expect("full create");
        let request = full.request.expect("request");
        assert_eq!(request.method, FunctionScheduleMethod::Put);
        assert_eq!(request.path, "/reports?kind=daily");
        assert_eq!(request.body.as_deref(), Some("hello"));
        assert_eq!(full.enabled, Some(false));
        assert!(
            serde_json::from_value::<CreateWire>(json!({ "cron": "0 3 * * *", "timezone": "UTC" }))
                .is_err(),
            "unknown fields are refused"
        );
        assert!(
            serde_json::from_value::<CreateWire>(json!({ "name": "no cron" })).is_err(),
            "cron is required"
        );
        assert!(
            serde_json::from_value::<CreateWire>(json!({
                "cron": "0 3 * * *",
                "request": { "method": "HEAD" },
            }))
            .is_err(),
            "only the documented methods"
        );
        let update: UpdateWire =
            serde_json::from_value(json!({ "enabled": false })).expect("update");
        assert_eq!(update.enabled, Some(false));
        assert!(update.cron.is_none());
        let empty: UpdateWire = serde_json::from_value(json!({})).expect("empty");
        assert!(
            FunctionScheduleUpdate {
                name: empty.name,
                cron: empty.cron,
                request: empty.request,
                enabled: empty.enabled,
            }
            .is_empty()
        );
    }

    #[test]
    fn path_ids_are_validated_and_errors_map_to_the_public_codes() {
        let request = HttpRequest::for_test(
            HttpMethod::Get,
            "/v1/projects/prj_example00/environments/env_example00/functions/hello/schedules/",
            Vec::new(),
            Vec::new(),
            None,
        );
        assert_eq!(
            schedule_id(&request)
                .expect_err("missing id")
                .envelope()
                .error
                .code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            function_name(&request)
                .expect_err("missing function")
                .envelope()
                .error
                .code,
            ErrorCode::InvalidRequest
        );
        let cron_error = CronSchedule::parse("61 * * * *").expect_err("refused");
        for (error, code) in [
            (
                FunctionScheduleError::EnvironmentNotFound,
                ErrorCode::NotFound,
            ),
            (FunctionScheduleError::FunctionNotFound, ErrorCode::NotFound),
            (FunctionScheduleError::ScheduleNotFound, ErrorCode::NotFound),
            (
                FunctionScheduleError::Forbidden,
                ErrorCode::PermissionDenied,
            ),
            (
                FunctionScheduleError::Conflict("function has no active deployment"),
                ErrorCode::Conflict,
            ),
            (
                FunctionScheduleError::InvalidRequest("a GET request carries no body"),
                ErrorCode::InvalidRequest,
            ),
            (
                FunctionScheduleError::InvalidCron(cron_error.clone()),
                ErrorCode::InvalidRequest,
            ),
            (FunctionScheduleError::CorruptRecord, ErrorCode::Unavailable),
        ] {
            assert_eq!(schedule_error(&request, error).envelope().error.code, code);
        }
        assert_eq!(
            schedule_error(&request, FunctionScheduleError::InvalidCron(cron_error))
                .envelope()
                .error
                .message,
            "cron expression is invalid at minute: value is out of range"
        );
        assert_eq!(
            schedule_error(
                &request,
                FunctionScheduleError::Conflict("a run of this schedule is still executing"),
            )
            .envelope()
            .error
            .message,
            "a run of this schedule is still executing"
        );
    }

    #[test]
    fn wire_shapes_match_the_openapi_document() {
        let request = HttpRequest::for_test(HttpMethod::Get, "/", Vec::new(), Vec::new(), None);
        let summary = RunSummaryWire {
            id: "run_abcdefghijklmnop".to_owned(),
            due_at: format_timestamp(&request, 1_798_761_600).expect("timestamp"),
            outcome: FunctionScheduleRunOutcome::SkippedOverlap,
            duration_milliseconds: None,
            response_status: None,
        };
        let schedule = ScheduleWire {
            id: "sch_abcdefghijklmnop".to_owned(),
            function_name: "nightly-report".to_owned(),
            name: String::new(),
            cron: "0 3 * * *".to_owned(),
            timezone: TIMEZONE,
            request: FunctionScheduleRequest::default(),
            enabled: false,
            state: FunctionScheduleState::Paused,
            next_run_at: None,
            last_run: Some(summary),
            created_at: "2027-01-01T00:00:00Z".to_owned(),
            updated_at: "2027-01-01T00:00:00Z".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(&schedule).expect("json"),
            json!({
                "id": "sch_abcdefghijklmnop",
                "functionName": "nightly-report",
                "name": "",
                "cron": "0 3 * * *",
                "timezone": "UTC",
                "request": {
                    "method": "POST",
                    "path": "/",
                    "headers": {},
                    "contentType": "application/json",
                },
                "enabled": false,
                "state": "paused",
                "nextRunAt": null,
                "lastRun": {
                    "id": "run_abcdefghijklmnop",
                    "dueAt": "2027-01-01T00:00:00Z",
                    "outcome": "skipped_overlap",
                    "durationMilliseconds": null,
                    "responseStatus": null,
                },
                "createdAt": "2027-01-01T00:00:00Z",
                "updatedAt": "2027-01-01T00:00:00Z",
            })
        );
        let run = RunWire {
            id: "run_abcdefghijklmnop".to_owned(),
            schedule_id: "sch_abcdefghijklmnop".to_owned(),
            function_name: "nightly-report".to_owned(),
            function_version: None,
            due_at: "2027-01-01T00:00:00Z".to_owned(),
            started_at: None,
            completed_at: None,
            duration_milliseconds: None,
            outcome: None,
            response_status: None,
            error: None,
            manual: true,
            created_at: "2027-01-01T00:00:00Z".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(RunPageWire {
                items: vec![run],
                next_cursor: None,
            })
            .expect("json"),
            json!({
                "items": [{
                    "id": "run_abcdefghijklmnop",
                    "scheduleId": "sch_abcdefghijklmnop",
                    "functionName": "nightly-report",
                    "functionVersion": null,
                    "dueAt": "2027-01-01T00:00:00Z",
                    "startedAt": null,
                    "completedAt": null,
                    "durationMilliseconds": null,
                    "outcome": null,
                    "responseStatus": null,
                    "error": null,
                    "manual": true,
                    "createdAt": "2027-01-01T00:00:00Z",
                }],
                "nextCursor": null,
            }),
            "a queued run carries every field, null where nothing happened yet"
        );
    }
}
