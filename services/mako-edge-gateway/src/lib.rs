//! Production composition for the public edge-function gateway.

#![forbid(unsafe_code)]

mod graph;
mod http;
mod runtime;

pub use graph::{EdgeGatewayGraph, EdgeGatewayGraphError};

use std::sync::Arc;

use mako_service_runtime::{HttpRouter, RouteRegistrationError};

pub fn edge_gateway_router(
    graph: Arc<EdgeGatewayGraph>,
) -> Result<HttpRouter, RouteRegistrationError> {
    let mut router = HttpRouter::new();
    http::add_routes(&mut router, graph)?;
    Ok(router)
}
