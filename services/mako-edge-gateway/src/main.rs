//! Mako Cloud edge-function gateway service entrypoint.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use mako_config::{ServiceConfig, ServiceKind};
use mako_service_runtime::{HttpTransportConfig, serve_http_transport_with_readiness};

use mako_edge_gateway_service::{EdgeGatewayGraph, edge_gateway_router};

fn main() -> ExitCode {
    start(ServiceKind::EdgeGateway)
}

fn start(service: ServiceKind) -> ExitCode {
    if let Err(error) = mako_edge_runtime_protocol::RuntimePin::embedded() {
        eprintln!("edge runtime configuration invalid: {error}");
        return ExitCode::FAILURE;
    }
    match ServiceConfig::load_from_process(service) {
        Ok(config) => {
            println!(
                "startup configuration valid: {}; component={}",
                config.startup_diagnostic(),
                mako_edge_gateway::COMPONENT,
            );
            let graph = match EdgeGatewayGraph::open(&config) {
                Ok(graph) => std::sync::Arc::new(graph),
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::FAILURE;
                }
            };
            let router = match edge_gateway_router(std::sync::Arc::clone(&graph)) {
                Ok(router) => router,
                Err(error) => {
                    eprintln!("edge route registration failed: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut transport = HttpTransportConfig::new(
                config.bind_address,
                service.name(),
                "edge_gateway_dependencies_ready",
            );
            transport.max_request_body_bytes = usize::try_from(config.max_request_bytes)
                .unwrap_or(16 * 1024 * 1024)
                .min(16 * 1024 * 1024);
            transport.shutdown_grace = config.shutdown_grace;
            match serve_http_transport_with_readiness(transport, router, graph) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("service runtime failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
