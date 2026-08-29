//! Production composition for the public edge-function gateway.

#![forbid(unsafe_code)]

mod graph;
mod http;
mod internal_http;
mod runtime;

pub use graph::{EdgeGatewayGraph, EdgeGatewayGraphError};

use std::sync::Arc;

use mako_service_runtime::{CrossOriginMiddleware, HttpRouter, RouteRegistrationError};

pub fn edge_gateway_router(
    graph: Arc<EdgeGatewayGraph>,
) -> Result<HttpRouter, RouteRegistrationError> {
    let mut router = HttpRouter::new();
    http::add_routes(&mut router, Arc::clone(&graph))?;
    internal_http::add_internal_routes(&mut router, Arc::clone(&graph))?;
    // Preflights for a function invocation are answered here, and only for
    // an origin the function's environment lists -- on the platform's
    // hostname and on a custom domain alike. An `OPTIONS` that is not such
    // a preflight still reaches the function, which may answer it itself.
    router.set_middleware(CrossOriginMiddleware::shared(Arc::new(
        http::FunctionRouteOrigins::new(graph),
    )));
    Ok(router)
}
