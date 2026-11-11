//! Production composition for the Mako Cloud data plane.

#![forbid(unsafe_code)]

mod application_mail;
mod auth_http;
mod auth_provider_http;
mod document_http;
mod explorer_http;
mod explorer_metrics;
mod graph;
mod internal_http;
mod replication_http;
mod service_user_http;
mod storage_http;
pub mod telemetry;

use std::sync::Arc;

use mako_service_runtime::{CrossOriginMiddleware, HttpRouter, RouteRegistrationError};

pub use explorer_metrics::ExplorerMetricsSnapshot;
pub use graph::{
    AllowedOriginRegistry, CustomDomainRegistry, DataPlaneGraph, DataPlaneGraphError,
    DataPlaneIdentityError, DataPlaneReadiness, DataPlaneRefreshOutcome, DataPlaneSessionGrant,
    EnvironmentAllowedOrigins, StorageMode,
};

pub fn data_plane_router(graph: Arc<DataPlaneGraph>) -> Result<HttpRouter, RouteRegistrationError> {
    let mut router = HttpRouter::new();
    auth_http::add_auth_routes(&mut router, Arc::clone(&graph))?;
    auth_provider_http::add_auth_provider_routes(&mut router, Arc::clone(&graph))?;
    document_http::add_document_routes(&mut router, Arc::clone(&graph))?;
    explorer_http::add_explorer_routes(&mut router, Arc::clone(&graph))?;
    replication_http::add_replication_routes(&mut router, Arc::clone(&graph))?;
    storage_http::add_storage_routes(&mut router, Arc::clone(&graph))?;
    service_user_http::add_service_user_routes(&mut router, Arc::clone(&graph))?;
    application_mail::add_application_mail_routes(&mut router, Arc::clone(&graph))?;
    internal_http::add_internal_routes(&mut router, Arc::clone(&graph))?;
    // Cross-origin access is decided for every request, matched or not,
    // from the allowlist the path's environment installed -- the same
    // answer on the platform's hostname and on a custom domain, and no
    // answer at all for a route a browser application does not call.
    router.set_middleware(CrossOriginMiddleware::shared(Arc::new(
        EnvironmentAllowedOrigins::new(graph),
    )));
    Ok(router)
}
