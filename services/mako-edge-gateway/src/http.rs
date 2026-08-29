use std::sync::Arc;

use async_trait::async_trait;
use futures::{StreamExt, executor::block_on};
use mako_api::{ErrorCode, RetryAdvice, TenantScope};
use mako_edge_gateway::{
    FunctionGateway, FunctionGatewayRequest, FunctionHttpMethod, FunctionRouteError,
    FunctionRouteResolver, ResolvedFunctionRoute, RuntimeResponseStream,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
    spawn_streaming_body,
};

use crate::{EdgeGatewayGraph, graph::CustomDomainLookupError};

const INVOCATION_ROUTE: &str = "/{project_ref}/functions/v1/{function_name}";
/// The shape a function request has on a custom domain: the hostname names
/// the environment, so there is no project reference in the path.
const CUSTOM_DOMAIN_ROUTE: &str = "/functions/v1/{function_name}";
/// Set by the reverse proxy on the custom-domain listener only, and
/// stripped on the platform's own hostname.
pub(crate) const CUSTOM_DOMAIN_HEADER: &str = "x-mako-custom-domain";

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
        let public_graph = Arc::clone(&graph);
        router.add_route(method, INVOCATION_ROUTE, move |request| {
            handle_invocation(&public_graph, &request)
        })?;
        let graph = Arc::clone(&graph);
        router.add_route(method, CUSTOM_DOMAIN_ROUTE, move |request| {
            handle_custom_domain_invocation(&graph, &request)
        })?;
    }
    Ok(())
}

fn handle_invocation(
    graph: &Arc<EdgeGatewayGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    // The public shape is never served on a custom domain, and the platform
    // hostname's proxy strips the header; one that arrives anyway is not
    // ours to honor.
    if request.header(CUSTOM_DOMAIN_HEADER).is_some() {
        return Err(not_found(request, "function route was not found"));
    }
    let project_ref = request
        .path_parameter("project_ref")
        .ok_or_else(|| invalid(request, "function route is invalid"))?;
    let function_name = request
        .path_parameter("function_name")
        .ok_or_else(|| invalid(request, "function route is invalid"))?;
    invoke(graph, request, project_ref, function_name, &graph.routes)
}

/// `/functions/v1/{name}` on a verified custom domain: the hostname's
/// environment is the project reference, and the route the control plane
/// resolves for it must list the hostname.
fn handle_custom_domain_invocation(
    graph: &Arc<EdgeGatewayGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let hostname = request
        .header(CUSTOM_DOMAIN_HEADER)
        .and_then(normalize_custom_domain)
        .ok_or_else(|| not_found(request, "function route was not found"))?;
    let function_name = request
        .path_parameter("function_name")
        .ok_or_else(|| invalid(request, "function route is invalid"))?;
    let tenant = match graph.custom_domains.tenant_for(&hostname) {
        Ok(Some(tenant)) => tenant,
        Ok(None) => return Err(not_found(request, "function route was not found")),
        Err(CustomDomainLookupError::Unavailable) => {
            return Err(unavailable(request, "custom domain routing is unavailable"));
        }
    };
    let project_ref = format!("{}--{}", tenant.project_id(), tenant.environment_id());
    let routes = CustomDomainRoutes {
        inner: &graph.routes,
        hostname: &hostname,
        tenant: &tenant,
    };
    invoke(graph, request, &project_ref, function_name, &routes)
}

/// The `X-Mako-Custom-Domain` value as the control plane stores hostnames:
/// lowercase, without a port or trailing dot; `None` if it is not a name.
pub(crate) fn normalize_custom_domain(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let without_port = trimmed
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
        .map_or(trimmed, |(host, _)| host);
    let hostname = without_port
        .strip_suffix('.')
        .unwrap_or(without_port)
        .to_ascii_lowercase();
    let valid = !hostname.is_empty()
        && hostname.len() <= 253
        && hostname.contains('.')
        && hostname.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        });
    valid.then_some(hostname)
}

/// Serves a route only when the request's custom hostname is one the route
/// lists for the hostname's own environment.
struct CustomDomainRoutes<'a> {
    inner: &'a dyn FunctionRouteResolver,
    hostname: &'a str,
    tenant: &'a TenantScope,
}

#[async_trait]
impl FunctionRouteResolver for CustomDomainRoutes<'_> {
    async fn resolve(
        &self,
        project_ref: &str,
        function_name: &str,
    ) -> Result<Option<ResolvedFunctionRoute>, FunctionRouteError> {
        Ok(self
            .inner
            .resolve(project_ref, function_name)
            .await?
            .filter(|route| {
                &route.tenant == self.tenant
                    && route
                        .custom_domains
                        .iter()
                        .any(|hostname| hostname == self.hostname)
            }))
    }
}

fn invoke(
    graph: &Arc<EdgeGatewayGraph>,
    request: &HttpRequest,
    project_ref: &str,
    function_name: &str,
    routes: &dyn FunctionRouteResolver,
) -> Result<HttpResponse, HttpApiError> {
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
        routes,
        &graph.tokens,
        &graph.admission,
        graph.audit.as_ref(),
        &graph.metrics,
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

fn not_found(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        404,
        ErrorCode::NotFound,
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
    use mako_api::{EnvironmentId, ProjectId};
    use mako_edge_gateway::RegionalDeploymentHealth;

    use super::*;

    fn tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(project).expect("project"),
            EnvironmentId::parse(environment).expect("environment"),
        )
    }

    struct FixedRoutes(ResolvedFunctionRoute);

    #[async_trait]
    impl FunctionRouteResolver for FixedRoutes {
        async fn resolve(
            &self,
            _project_ref: &str,
            function_name: &str,
        ) -> Result<Option<ResolvedFunctionRoute>, FunctionRouteError> {
            Ok((function_name == self.0.function_name).then(|| self.0.clone()))
        }
    }

    fn route(custom_domains: &[&str]) -> ResolvedFunctionRoute {
        ResolvedFunctionRoute {
            tenant: tenant("prj_example00", "env_example00"),
            organization_id: "org_example00".to_owned(),
            function_name: "hello".to_owned(),
            active_version: 1,
            selected_regions: vec!["local".to_owned()],
            regional_deployments: vec![RegionalDeploymentHealth {
                region: "local".to_owned(),
                healthy: true,
                valid_until_unix_seconds: u64::MAX,
            }],
            verify_jwt: false,
            request_limit_bytes: 1_024,
            response_limit_bytes: 1_024,
            custom_domains: custom_domains
                .iter()
                .map(|hostname| (*hostname).to_owned())
                .collect(),
        }
    }

    #[test]
    fn the_custom_domain_header_is_normalized_to_a_stored_hostname() {
        for (raw, expected) in [
            ("API.Example.COM", Some("api.example.com")),
            (" api.example.com. ", Some("api.example.com")),
            ("api.example.com:443", Some("api.example.com")),
            ("", None),
            ("localhost", None),
            ("api.example.com/path", None),
            ("api_1.example.com", None),
            ("api.example.com:abc", None),
        ] {
            assert_eq!(normalize_custom_domain(raw).as_deref(), expected, "{raw:?}");
        }
    }

    #[test]
    fn a_custom_domain_route_is_served_only_for_a_listed_hostname_of_its_own_tenant() {
        let resolved = route(&["api.example.com", "app.example.org"]);
        let own = tenant("prj_example00", "env_example00");
        let other = tenant("prj_example00", "env_example01");
        let inner = FixedRoutes(resolved);
        for (hostname, tenant, served) in [
            ("api.example.com", &own, true),
            ("app.example.org", &own, true),
            ("other.example.com", &own, false),
            ("api.example.com", &other, false),
        ] {
            let routes = CustomDomainRoutes {
                inner: &inner,
                hostname,
                tenant,
            };
            let outcome = block_on(routes.resolve("prj_example00--env_example00", "hello"))
                .expect("resolved");
            assert_eq!(outcome.is_some(), served, "{hostname} for {tenant:?}");
        }
        let routes = CustomDomainRoutes {
            inner: &FixedRoutes(route(&[])),
            hostname: "api.example.com",
            tenant: &own,
        };
        assert!(
            block_on(routes.resolve("prj_example00--env_example00", "hello"))
                .expect("resolved")
                .is_none(),
            "an environment without custom domains serves none"
        );
    }
}
