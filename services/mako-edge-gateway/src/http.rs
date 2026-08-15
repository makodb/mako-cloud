use std::sync::Arc;

use futures::{StreamExt, executor::block_on};
use mako_api::{ErrorCode, RetryAdvice};
use mako_edge_gateway::{
    FunctionGateway, FunctionGatewayRequest, FunctionHttpMethod, RuntimeResponseStream,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
    spawn_streaming_body,
};

use crate::EdgeGatewayGraph;

const INVOCATION_ROUTE: &str = "/{project_ref}/functions/v1/{function_name}";

pub(crate) fn add_routes(
    router: &mut HttpRouter,
    graph: Arc<EdgeGatewayGraph>,
) -> Result<(), RouteRegistrationError> {
    for method in [
        HttpMethod::Get,
        HttpMethod::Head,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
        HttpMethod::Options,
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, INVOCATION_ROUTE, move |request| {
            handle_invocation(&graph, &request)
        })?;
    }
    Ok(())
}

fn handle_invocation(
    graph: &Arc<EdgeGatewayGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let project_ref = request
        .path_parameter("project_ref")
        .ok_or_else(|| invalid(request, "function route is invalid"))?;
    let function_name = request
        .path_parameter("function_name")
        .ok_or_else(|| invalid(request, "function route is invalid"))?;
    let query = (!request.query().is_empty()).then(|| {
        request
            .query()
            .iter()
            .map(|(name, value)| format!("{}={}", percent_encode(name), percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    });
    let input = FunctionGatewayRequest {
        request_id: request.request_id().to_owned(),
        method: function_method(request.method()),
        path: format!("/{project_ref}/functions/v1/{function_name}"),
        query,
        headers: forwarded_request_headers(request),
        body: request.body().to_vec(),
        region_priority: vec![graph.region().to_owned()],
        now_unix_seconds: now_unix_seconds(request)?,
    };
    match block_on(FunctionGateway.invoke(
        input,
        &graph.routes,
        &graph.tokens,
        &graph.admission,
        graph.audit.as_ref(),
        &graph.runtime,
    )) {
        Ok(response) => stream_response(request, response.status, response.headers, response.body),
        Err(error) => HttpResponse::json(error.status(), error.api_error())
            .map_err(|_| internal(request, "function error response is unavailable")),
    }
}

fn stream_response(
    request: &HttpRequest,
    status: u16,
    mut headers: Vec<(String, String)>,
    mut source: RuntimeResponseStream,
) -> Result<HttpResponse, HttpApiError> {
    let content_type = headers
        .iter()
        .position(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|index| headers.remove(index).1)
        .map_or("application/octet-stream", |value| {
            known_content_type(&value)
        });
    let body = spawn_streaming_body(8, move |sender| {
        block_on(async move {
            while let Some(next) = source.next().await {
                match next {
                    Ok(chunk) if !sender.is_disconnected() => {
                        if sender.send(chunk).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        });
    })
    .map_err(|_| internal(request, "function response stream is unavailable"))?;
    let mut response = HttpResponse::stream(status, content_type, body);
    for (name, value) in headers {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "content-length" | "transfer-encoding" | "connection" | "x-request-id"
        ) {
            continue;
        }
        response = response
            .with_header(&name, &value)
            .map_err(|_| internal(request, "function response header is invalid"))?;
    }
    Ok(response)
}

fn forwarded_request_headers(request: &HttpRequest) -> Vec<(String, String)> {
    [
        "authorization",
        "accept",
        "content-type",
        "user-agent",
        "traceparent",
        "tracestate",
    ]
    .into_iter()
    .flat_map(|name| {
        request
            .header_values(name)
            .iter()
            .map(move |value| (name.to_owned(), value.clone()))
    })
    .collect()
}

const fn function_method(method: HttpMethod) -> FunctionHttpMethod {
    match method {
        HttpMethod::Get => FunctionHttpMethod::Get,
        HttpMethod::Head => FunctionHttpMethod::Head,
        HttpMethod::Post => FunctionHttpMethod::Post,
        HttpMethod::Put => FunctionHttpMethod::Put,
        HttpMethod::Patch => FunctionHttpMethod::Patch,
        HttpMethod::Delete => FunctionHttpMethod::Delete,
        HttpMethod::Options => FunctionHttpMethod::Options,
    }
}

fn known_content_type(value: &str) -> &'static str {
    match value.split(';').next().unwrap_or_default().trim() {
        "application/json" => "application/json; charset=utf-8",
        "text/plain" => "text/plain; charset=utf-8",
        "text/html" => "text/html; charset=utf-8",
        "text/event-stream" => "text/event-stream",
        "application/javascript" => "application/javascript; charset=utf-8",
        "application/octet-stream" => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
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

fn internal(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        500,
        ErrorCode::Internal,
        message,
        request.request_id(),
        RetryAdvice::AfterDelay { after_ms: 1_000 },
    )
}
