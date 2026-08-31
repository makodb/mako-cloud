use std::{collections::BTreeMap, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use mako_api::{ErrorCode, RetryAdvice};
use mako_control_plane::{
    FunctionAdminError, FunctionBundleUpload, FunctionConfiguration, FunctionLogQuery,
    FunctionName, FunctionSourceFile, FunctionTestRequest, NewFunction, NewFunctionVersion,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    ControlPlaneGraph,
    http_support::{public_json, public_value, query_value, reject_unknown_query, tenant},
    management_http::{
        conflict, forbidden, invalid, limit, no_payload, no_query, not_found, parse_json,
        require_idempotency, require_json, unavailable, with_developer,
    },
};

const MAX_BUNDLE_BYTES: usize = 10 * 1024 * 1024;

pub(crate) fn add_function_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/function-bundles",
            handle_upload_bundle as Handler,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/functions",
            handle_list_functions,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/functions",
            handle_create_function,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}",
            handle_get_function,
        ),
        (
            HttpMethod::Patch,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}",
            handle_configure_function,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}",
            handle_delete_function,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions",
            handle_list_versions,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions",
            handle_deploy_version,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}",
            handle_get_version,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}",
            handle_delete_version,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}/actions/promote",
            handle_promote,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}/actions/health-check",
            handle_health_check,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}/actions/rollback",
            handle_rollback,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/actions/test",
            handle_test,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/logs",
            handle_logs,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_upload_bundle(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: BundleUploadWire = parse_json(request)?;
    let upload = bundle_upload(request, body)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let outcome = graph
            .function_service()
            .upload_bundle(&actor, &tenant, upload, now)
            .await
            .map_err(|error| function_error(request, error))?;
        let status = if outcome.artifact.is_some() {
            "ready"
        } else {
            "rejected"
        };
        let mut response = json!({
            "status": status,
            "diagnostics": outcome.diagnostics,
        });
        if let Some(artifact) = outcome.artifact
            && let Some(object) = response.as_object_mut()
        {
            object.insert("artifact".to_owned(), json!(artifact));
        }
        public_value(request, 200, response)
    })
}

fn handle_list_functions(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let items = graph
            .function_service()
            .list_functions(&actor, &tenant, limit(), now)
            .await
            .map_err(|error| function_error(request, error))?;
        public_value(request, 200, json!({"items": items}))
    })
}

fn handle_create_function(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: CreateFunctionWire = parse_json(request)?;
    let tenant = tenant(request)?;
    let name =
        FunctionName::parse(body.name).map_err(|_| invalid(request, "function name is invalid"))?;
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .function_service()
            .create_function(
                &actor,
                NewFunction {
                    tenant,
                    name,
                    configuration: body.configuration,
                    now_unix_seconds: now,
                },
            )
            .await
            .map_err(|error| function_error(request, error))?;
        public_json(request, 201, &record)
    })
}

fn handle_get_function(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    function_read(
        graph,
        request,
        |graph, actor, tenant, name, now| async move {
            let record = graph
                .function_service()
                .get_function(&actor, &tenant, &name, now)
                .await
                .map_err(|error| function_error(request, error))?;
            public_json(request, 200, &record)
        },
    )
}

fn handle_configure_function(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let configuration: FunctionConfiguration = parse_json(request)?;
    function_read(
        graph,
        request,
        |graph, actor, tenant, name, now| async move {
            let record = graph
                .function_service()
                .configure(&actor, &tenant, &name, configuration, now)
                .await
                .map_err(|error| function_error(request, error))?;
            public_json(request, 200, &record)
        },
    )
}

fn handle_delete_function(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    function_read(
        graph,
        request,
        |graph, actor, tenant, name, now| async move {
            let record = graph
                .function_service()
                .delete_function(&actor, &tenant, &name, now)
                .await
                .map_err(|error| function_error(request, error))?;
            public_json(request, 200, &record)
        },
    )
}

fn handle_list_versions(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    function_read(
        graph,
        request,
        |graph, actor, tenant, name, now| async move {
            let items = graph
                .function_service()
                .list_versions(&actor, &tenant, &name, limit(), now)
                .await
                .map_err(|error| function_error(request, error))?;
            public_value(request, 200, json!({"items": items}))
        },
    )
}

fn handle_deploy_version(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: DeployVersionWire = parse_json(request)?;
    function_read(
        graph,
        request,
        |graph, actor, tenant, name, now| async move {
            let record = graph
                .function_service()
                .deploy_version(
                    &actor,
                    NewFunctionVersion {
                        tenant,
                        function_name: name,
                        version: body.version,
                        bundle_digest: body.bundle_digest,
                        entrypoint: body.entrypoint,
                        runtime_version: body.runtime_version,
                        now_unix_seconds: now,
                    },
                )
                .await
                .map_err(|error| function_error(request, error))?;
            public_json(request, 202, &record)
        },
    )
}

fn handle_get_version(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    version_operation(graph, request, VersionOperation::Get)
}

fn handle_delete_version(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    version_operation(graph, request, VersionOperation::Delete)
}

fn handle_promote(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_idempotency(request)?;
    version_operation(graph, request, VersionOperation::Promote)
}

fn handle_health_check(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    version_operation(graph, request, VersionOperation::Health)
}

fn handle_rollback(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_idempotency(request)?;
    version_operation(graph, request, VersionOperation::Rollback)
}

fn version_operation(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: VersionOperation,
) -> Result<HttpResponse, HttpApiError> {
    let version = function_version(request)?;
    function_read(
        graph,
        request,
        |graph, actor, tenant, name, now| async move {
            match operation {
                VersionOperation::Get => {
                    let value = graph
                        .function_service()
                        .get_version(&actor, &tenant, &name, version, now)
                        .await;
                    public_json(
                        request,
                        200,
                        &value.map_err(|error| function_error(request, error))?,
                    )
                }
                VersionOperation::Delete => {
                    graph
                        .function_service()
                        .delete_version(&actor, &tenant, &name, version, now)
                        .await
                        .map_err(|error| function_error(request, error))?;
                    Ok(HttpResponse::empty(204))
                }
                VersionOperation::Promote | VersionOperation::Rollback => {
                    let value = if operation == VersionOperation::Promote {
                        graph
                            .function_service()
                            .promote(&actor, &tenant, &name, version, now)
                            .await
                    } else {
                        graph
                            .function_service()
                            .rollback(&actor, &tenant, &name, version, now)
                            .await
                    }
                    .map_err(|error| function_error(request, error))?;
                    public_json(request, 200, &value)
                }
                VersionOperation::Health => {
                    let value = graph
                        .function_service()
                        .check_version_health(&actor, &tenant, &name, version, now)
                        .await
                        .map_err(|error| function_error(request, error))?;
                    public_json(request, 200, &value)
                }
            }
        },
    )
}

fn handle_test(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let body: FunctionTestWire = parse_json(request)?;
    let bytes = decode_bounded(
        request,
        &body.body,
        MAX_BUNDLE_BYTES,
        "function test body is invalid",
    )?;
    let test = FunctionTestRequest {
        method: body.method,
        path: body.path,
        headers: body.headers.into_iter().collect(),
        body: bytes,
    };
    function_read(
        graph,
        request,
        |graph, actor, tenant, name, now| async move {
            let response = graph
                .function_service()
                .test_invoke(&actor, &tenant, &name, body.version, &test, now)
                .await
                .map_err(|error| function_error(request, error))?;
            let headers = response.headers.into_iter().collect::<BTreeMap<_, _>>();
            public_value(
                request,
                200,
                json!({
                    "status": response.status,
                    "headers": headers,
                    "body": STANDARD.encode(response.body),
                    "correlationId": response.correlation_id,
                }),
            )
        },
    )
}

fn handle_logs(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    if !request.body().is_empty() {
        return Err(invalid(request, "request body is not supported"));
    }
    reject_unknown_query(request, &["cursor", "limit"])?;
    let cursor = query_value(request, "cursor")?.map(str::to_owned);
    let limit = match query_value(request, "limit")? {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| invalid(request, "function log limit is invalid"))?,
        None => 100,
    };
    if !(1..=1000).contains(&limit) || cursor.as_ref().is_some_and(|value| value.len() > 4096) {
        return Err(invalid(request, "function log query is invalid"));
    }
    let query = FunctionLogQuery { cursor, limit };
    function_read(
        graph,
        request,
        |graph, actor, tenant, name, now| async move {
            let page = graph
                .function_service()
                .logs(&actor, &tenant, &name, &query, now)
                .await
                .map_err(|error| function_error(request, error))?;
            public_value(request, 200, function_log_value(page))
        },
    )
}

fn function_read<F, Fut>(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    operation: F,
) -> Result<HttpResponse, HttpApiError>
where
    F: FnOnce(
        Arc<ControlPlaneGraph>,
        mako_control_plane::DeveloperPrincipal,
        mako_api::TenantScope,
        FunctionName,
        u64,
    ) -> Fut,
    Fut: std::future::Future<Output = Result<HttpResponse, HttpApiError>>,
{
    let tenant = tenant(request)?;
    let name = function_name(request)?;
    let graph_owned = Arc::clone(graph);
    with_developer(graph, request, |actor, now| {
        operation(graph_owned, actor, tenant, name, now)
    })
}

fn bundle_upload(
    request: &HttpRequest,
    body: BundleUploadWire,
) -> Result<FunctionBundleUpload, HttpApiError> {
    match body {
        BundleUploadWire::Source {
            entrypoint,
            files,
            dependencies,
        } => {
            let files = files
                .into_iter()
                .map(|file| {
                    Ok(FunctionSourceFile {
                        path: file.path,
                        contents: decode_bounded(
                            request,
                            &file.content_base64,
                            MAX_BUNDLE_BYTES,
                            "function source content is invalid",
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, HttpApiError>>()?;
            Ok(FunctionBundleUpload::Source {
                entrypoint,
                files,
                dependencies,
            })
        }
        BundleUploadWire::Prebuilt {
            entrypoint,
            bundle_base64,
        } => Ok(FunctionBundleUpload::Prebuilt {
            entrypoint,
            bundle: decode_bounded(
                request,
                &bundle_base64,
                MAX_BUNDLE_BYTES,
                "prebuilt function bundle is invalid",
            )?,
        }),
    }
}

fn decode_bounded(
    request: &HttpRequest,
    value: &str,
    maximum: usize,
    message: &'static str,
) -> Result<Vec<u8>, HttpApiError> {
    if value.len()
        > maximum
            .saturating_mul(4)
            .saturating_div(3)
            .saturating_add(4)
    {
        return Err(invalid(request, message));
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| invalid(request, message))?;
    if bytes.is_empty() || bytes.len() > maximum {
        Err(invalid(request, message))
    } else {
        Ok(bytes)
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

fn function_version(request: &HttpRequest) -> Result<u64, HttpApiError> {
    request
        .path_parameter("functionVersion")
        .unwrap_or_default()
        .parse::<u64>()
        .ok()
        .filter(|version| *version > 0)
        .ok_or_else(|| invalid(request, "function version path is invalid"))
}

fn function_log_value(page: mako_control_plane::FunctionLogPage) -> Value {
    let items = page
        .items
        .into_iter()
        .map(|entry| {
            json!({
                "timestampUnixMilliseconds": entry.timestamp_unix_milliseconds,
                "level": entry.level,
                "message": entry.message,
                "correlationId": entry.correlation_id,
                "version": entry.version,
                "region": entry.region,
            })
        })
        .collect::<Vec<_>>();
    json!({"items": items, "nextCursor": page.next_cursor})
}

fn function_error(request: &HttpRequest, error: FunctionAdminError) -> HttpApiError {
    match error {
        FunctionAdminError::NotFound | FunctionAdminError::BundleNotFound => {
            not_found(request, "function resource was not found")
        }
        FunctionAdminError::Forbidden => forbidden(request, "function operation is forbidden"),
        FunctionAdminError::Conflict
        | FunctionAdminError::InvalidState
        | FunctionAdminError::DeploymentUnhealthy
        | FunctionAdminError::NoActiveVersion
        | FunctionAdminError::ActiveVersionCannotBeDeleted => {
            conflict(request, "function operation conflicts with current state")
        }
        FunctionAdminError::InvalidFunction
        | FunctionAdminError::InvalidConfiguration
        | FunctionAdminError::InvalidDeployment
        | FunctionAdminError::InvalidTestRequest
        | FunctionAdminError::InvalidLogQuery => invalid(request, "function input is invalid"),
        // The one validation refusal that names its input: the offending host
        // is the developer's own declaration, bounded and control-free, and a
        // refusal that does not say which entry failed cannot be acted on.
        ref denied @ FunctionAdminError::InvalidAllowedHost(_) => HttpApiError::new(
            400,
            ErrorCode::InvalidRequest,
            denied.to_string(),
            request.request_id(),
            RetryAdvice::Never,
        ),
        // The backend builds a bounded, control-character-free diagnostic for
        // exactly this: saying which dependency refused and why. Discarding it
        // left an operator with "unavailable" and nothing to act on.
        FunctionAdminError::Backend(ref backend) => {
            let detail = backend.to_string();
            eprintln!(
                "function administration failed: request={} cause=backend detail={detail}",
                request.request_id()
            );
            HttpApiError::new(
                503,
                ErrorCode::Unavailable,
                format!("the function runtime backend refused this operation: {detail}"),
                request.request_id(),
                RetryAdvice::AfterDelay { after_ms: 1_000 },
            )
        }
        // The rest are the platform's own faults, so they stay generic to the
        // caller -- but an operator reading the log learns which one.
        ref other => {
            eprintln!(
                "function administration failed: request={} cause={other:?}",
                request.request_id()
            );
            unavailable(request, "function administration is unavailable")
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum VersionOperation {
    Get,
    Delete,
    Promote,
    Health,
    Rollback,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateFunctionWire {
    name: String,
    configuration: FunctionConfiguration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeployVersionWire {
    version: u64,
    bundle_digest: String,
    entrypoint: String,
    runtime_version: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BundleUploadWire {
    Source {
        entrypoint: String,
        files: Vec<SourceFileWire>,
        dependencies: BTreeMap<String, String>,
    },
    Prebuilt {
        entrypoint: String,
        bundle_base64: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SourceFileWire {
    path: String,
    content_base64: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FunctionTestWire {
    version: Option<u64>,
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}
