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
mod storage_http;
pub mod telemetry;

use std::sync::Arc;

use mako_service_runtime::{HttpRouter, RouteRegistrationError};

pub use explorer_metrics::ExplorerMetricsSnapshot;
pub use graph::{
    DataPlaneGraph, DataPlaneGraphError, DataPlaneIdentityError, DataPlaneReadiness,
    DataPlaneRefreshOutcome, DataPlaneSessionGrant, StorageMode,
};

pub fn data_plane_router(graph: Arc<DataPlaneGraph>) -> Result<HttpRouter, RouteRegistrationError> {
    let mut router = HttpRouter::new();
    auth_http::add_auth_routes(&mut router, Arc::clone(&graph))?;
    auth_provider_http::add_auth_provider_routes(&mut router, Arc::clone(&graph))?;
    document_http::add_document_routes(&mut router, Arc::clone(&graph))?;
    explorer_http::add_explorer_routes(&mut router, Arc::clone(&graph))?;
    replication_http::add_replication_routes(&mut router, Arc::clone(&graph))?;
    storage_http::add_storage_routes(&mut router, Arc::clone(&graph))?;
    application_mail::add_application_mail_routes(&mut router, Arc::clone(&graph))?;
    internal_http::add_internal_routes(&mut router, graph)?;
    Ok(router)
}
