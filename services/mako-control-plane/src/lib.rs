//! Production composition for the Mako Cloud control plane.

#![forbid(unsafe_code)]

mod allowed_origins_http;
mod audit;
mod auth_settings_http;
mod collection_http;
mod credential_http;
mod custom_domain_http;
mod data_job_http;
mod developer_auth_http;
mod developer_metrics;
mod developer_metrics_http;
mod email_template_http;
mod explorer_http;
mod explorer_invalidation;
mod function_http;
mod function_logs;
mod function_resolution;
mod function_schedule_http;
mod graph;
mod http_support;
mod identity;
mod identity_admin_http;
mod internal_http;
mod management_http;
mod observability_http;
mod operator_auth_http;
mod operator_http;
mod operator_provider;
mod policy_http;
mod smtp;
mod storage_bucket_http;
mod webhook_http;
mod workspace_http;

use std::sync::{Arc, OnceLock};
use std::thread::Thread;

use mako_service_runtime::{HttpRouter, RouteRegistrationError};

/// Handle to the provisioning worker thread, registered by the binary after it
/// spawns the worker. Creating a project or environment unparks it so the new
/// resource provisions within milliseconds instead of waiting for the next idle
/// poll -- without shortening the idle interval, which would contend for the
/// single-writer control database.
pub static PROVISIONING_WAKE: OnceLock<Thread> = OnceLock::new();

/// Wake the provisioning worker immediately, if the binary has registered it.
pub(crate) fn wake_provisioning_worker() {
    if let Some(thread) = PROVISIONING_WAKE.get() {
        thread.unpark();
    }
}

pub use function_resolution::{
    FunctionResolutionError, FunctionResolutionService, ResolvedFunctionConfiguration,
    ResolvedFunctionSecretValue,
};
pub use graph::{
    ControlPlaneGraph, ControlPlaneGraphError, ControlPlaneReadiness, ControlPlaneStorageMode,
};

pub fn control_plane_router(
    graph: Arc<ControlPlaneGraph>,
) -> Result<HttpRouter, RouteRegistrationError> {
    let mut router = HttpRouter::new();
    internal_http::add_internal_routes(&mut router, Arc::clone(&graph))?;
    developer_auth_http::add_developer_auth_routes(&mut router, Arc::clone(&graph))?;
    operator_auth_http::add_operator_auth_routes(&mut router, Arc::clone(&graph))?;
    developer_metrics_http::add_developer_metrics_route(&mut router, Arc::clone(&graph))?;
    management_http::add_management_routes(&mut router, Arc::clone(&graph))?;
    explorer_http::add_explorer_routes(&mut router, Arc::clone(&graph))?;
    data_job_http::add_data_job_routes(&mut router, Arc::clone(&graph))?;
    workspace_http::add_workspace_routes(&mut router, Arc::clone(&graph))?;
    identity_admin_http::add_identity_admin_routes(&mut router, Arc::clone(&graph))?;
    credential_http::add_credential_routes(&mut router, Arc::clone(&graph))?;
    email_template_http::add_email_template_routes(&mut router, Arc::clone(&graph))?;
    allowed_origins_http::add_allowed_origins_routes(&mut router, Arc::clone(&graph))?;
    function_http::add_function_routes(&mut router, Arc::clone(&graph))?;
    function_schedule_http::add_function_schedule_routes(&mut router, Arc::clone(&graph))?;
    custom_domain_http::add_custom_domain_routes(&mut router, Arc::clone(&graph))?;
    observability_http::add_observability_routes(&mut router, Arc::clone(&graph))?;
    operator_http::add_operator_routes(&mut router, Arc::clone(&graph))?;
    collection_http::add_collection_routes(&mut router, Arc::clone(&graph))?;
    storage_bucket_http::add_storage_bucket_routes(&mut router, Arc::clone(&graph))?;
    auth_settings_http::add_auth_settings_routes(&mut router, Arc::clone(&graph))?;
    webhook_http::add_webhook_routes(&mut router, Arc::clone(&graph))?;
    policy_http::add_policy_routes(&mut router, graph)?;
    Ok(router)
}
