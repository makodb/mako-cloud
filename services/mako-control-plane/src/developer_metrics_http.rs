use std::{sync::Arc, time::SystemTime};

use futures::executor::block_on;
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpResponse, HttpRouter, RouteRegistrationError,
};

use crate::{
    ControlPlaneGraph,
    developer_metrics::DeveloperMetricsRenderContext,
    management_http::{internal, no_payload, no_query, unavailable},
};

pub(crate) fn add_developer_metrics_route(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    router.add_route(HttpMethod::Get, "/metrics", move |request| {
        no_query(&request)?;
        no_payload(&request)?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|_| unavailable(&request, "developer metrics clock is unavailable"))?;
        let health = block_on(graph.developer_registration_store().health_snapshot(now))
            .map_err(|_| unavailable(&request, "developer metrics state is unavailable"))?;
        let operator_health = block_on(
            graph
                .operator_password_authentication()
                .store()
                .health_snapshot(now),
        )
        .map_err(|_| unavailable(&request, "operator metrics state is unavailable"))?;
        let config = graph.developer_registration_service().config();
        let operator_config = graph.operator_password_authentication().config();
        let control_storage = graph.control_storage_health_signals().ok();
        let body = graph
            .developer_metrics()
            .render(DeveloperMetricsRenderContext {
                health: &health,
                registration_enabled: config.enabled,
                mail_ready: config.mail_ready,
                operator_health: &operator_health,
                operator_password_enabled: operator_config.enabled,
                operator_break_glass_enabled: graph.operator_break_glass_bearer_enabled(),
                control_storage: control_storage.as_ref(),
            });
        if body.len() > 64 * 1024 {
            return Err(internal(
                &request,
                "developer metrics response is too large",
            ));
        }
        Ok::<_, HttpApiError>(HttpResponse::bytes(
            200,
            "text/plain; version=0.0.4; charset=utf-8",
            body,
        ))
    })
}
