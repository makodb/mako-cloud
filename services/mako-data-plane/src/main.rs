//! Mako Cloud data-plane service entrypoint.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use std::sync::Arc;

use futures::executor::block_on;
use mako_config::{ServiceConfig, ServiceKind};
use mako_data_plane_service::{DataPlaneGraph, data_plane_router};
use mako_service_runtime::{
    HttpTransportConfig, ReadinessProbe, serve_http_transport_with_readiness,
};

fn main() -> ExitCode {
    start(ServiceKind::DataPlane)
}

fn start(service: ServiceKind) -> ExitCode {
    match ServiceConfig::load_from_process(service) {
        Ok(config) => {
            let graph = match DataPlaneGraph::open(&config) {
                Ok(graph) => Arc::new(graph),
                Err(error) => {
                    eprintln!("data-plane graph not ready: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let readiness = graph.readiness();
            if !readiness.is_ready() {
                eprintln!("{readiness}");
                return ExitCode::FAILURE;
            }
            println!(
                "startup configuration valid: {}",
                config.startup_diagnostic()
            );
            println!("{readiness}");
            let mut transport = HttpTransportConfig::new(
                config.bind_address,
                service.name(),
                "storage_identity_documents_policy_gateway_quota_audit_ready",
            );
            transport.max_request_body_bytes = usize::try_from(config.max_request_bytes)
                .expect("validated request size must fit this platform");
            transport.shutdown_grace = config.shutdown_grace;
            let readiness_probe: Arc<dyn ReadinessProbe> = graph.clone();
            let router = match data_plane_router(Arc::clone(&graph)) {
                Ok(router) => router,
                Err(error) => {
                    eprintln!("data-plane route composition failed: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let runtime = serve_http_transport_with_readiness(transport, router, readiness_probe);
            let shutdown = Arc::try_unwrap(graph)
                .map_err(|_| "data-plane graph still has active owners")
                .and_then(|graph| {
                    block_on(graph.shutdown())
                        .map_err(|_| "data-plane graph shutdown did not complete")
                });
            match (runtime, shutdown) {
                (Ok(()), Ok(())) => ExitCode::SUCCESS,
                (Err(error), _) => {
                    eprintln!("service runtime failed: {error}");
                    ExitCode::FAILURE
                }
                (_, Err(error)) => {
                    eprintln!("service shutdown failed: {error}");
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
