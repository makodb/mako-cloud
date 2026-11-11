//! The private route the backup worker uses to have the data plane
//! checkpoint its own database while it keeps serving.
//!
//! Only the owning process can take a consistent checkpoint of a running
//! RocksDB database, so the worker asks for one over the signed loopback
//! protocol and finishes the backup from the linked copy itself.

use std::sync::Arc;

use futures::executor::block_on;
use mako_api::{ErrorCode, RetryAdvice};
use mako_internal_rpc::{
    BACKUP_WORKER_ENVIRONMENT_ID, BACKUP_WORKER_PROJECT_ID, GuardDecision, InternalCaller,
    InternalReplayGuard as _, InternalRoute, RocksInternalReplayGuardError,
    StorageCheckpointRequest, StorageCheckpointResponse,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use mako_storage::{BackupId, StorageError, StorageErrorKind};

use crate::{DataPlaneGraph, auth_http};

pub fn add_storage_checkpoint_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    router.add_route(
        HttpMethod::Post,
        InternalRoute::StorageCheckpoint.path(),
        move |request| handle_storage_checkpoint(&graph, &request),
    )
}

fn handle_storage_checkpoint(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let now = auth_http::now_unix_seconds(request.request_id())?;
    block_on(async {
        let verified = graph
            .internal_authenticator(InternalCaller::BackupWorker)
            .verify(InternalRoute::StorageCheckpoint, request, now)
            .map_err(|error| error.to_http_error(request.request_id()))?;
        if verified.tenant.project_id().as_str() != BACKUP_WORKER_PROJECT_ID
            || verified.tenant.environment_id().as_str() != BACKUP_WORKER_ENVIRONMENT_ID
        {
            return Err(HttpApiError::new(
                403,
                ErrorCode::PermissionDenied,
                "storage checkpoint scope is invalid",
                request.request_id(),
                RetryAdvice::Never,
            ));
        }
        let input: StorageCheckpointRequest = serde_json::from_slice(request.body())
            .map_err(|_| auth_http::invalid(request, "storage checkpoint input is invalid"))?;
        let backup_id = BackupId::new(input.backup_id.clone())
            .map_err(|_| auth_http::invalid(request, "backup identifier is invalid"))?;
        let guard = graph
            .internal_replay_guard(&verified.tenant, &verified.tenant)
            .map_err(|_| {
                auth_http::unavailable(request, "checkpoint replay protection is unavailable")
            })?;
        let decision = guard
            .claim(&verified, now)
            .await
            .map_err(|error| match error {
                RocksInternalReplayGuardError::Replay
                | RocksInternalReplayGuardError::IdempotencyMismatch => {
                    conflict(request, "storage checkpoint request was already used")
                }
                _ => auth_http::unavailable(request, "checkpoint replay protection failed"),
            })?;
        if decision == GuardDecision::Duplicate {
            return Err(conflict(
                request,
                "storage checkpoint request was already used",
            ));
        }
        let checkpoint = graph
            .create_online_checkpoint(backup_id)
            .map_err(|error| checkpoint_error(request, &error))?;
        let response = StorageCheckpointResponse {
            backup_id: input.backup_id,
            checkpoint_path: checkpoint.path.to_string_lossy().into_owned(),
            barrier_wait_milliseconds: millis(checkpoint.timing.barrier_wait),
            creation_milliseconds: millis(checkpoint.timing.creation),
        };
        HttpResponse::json(200, &response).map_err(|_| {
            auth_http::internal_from_id(
                request.request_id(),
                "storage checkpoint response serialization failed",
            )
        })
    })
}

/// A busy barrier or another checkpoint is retryable on the next schedule;
/// a misconfigured root is not, and neither leaks a storage detail.
fn checkpoint_error(request: &HttpRequest, error: &StorageError) -> HttpApiError {
    match error.kind {
        StorageErrorKind::Conflict => {
            conflict(request, "another storage checkpoint is in progress")
        }
        StorageErrorKind::Unavailable if error.retryable => auth_http::unavailable(
            request,
            "open commit groups did not drain within the checkpoint bound",
        ),
        StorageErrorKind::Unsupported => {
            auth_http::unavailable(request, "online checkpoints need production storage")
        }
        _ => auth_http::unavailable(request, "online checkpoint could not be created"),
    }
}

fn conflict(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        409,
        ErrorCode::Conflict,
        message,
        request.request_id(),
        RetryAdvice::AfterDelay { after_ms: 1_000 },
    )
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        net::{IpAddr, Ipv4Addr, SocketAddr},
        time::{SystemTime, UNIX_EPOCH},
    };

    use mako_api::{EnvironmentId, ProjectId, TenantScope};
    use mako_internal_rpc::{DeploymentKey, InternalRequestAuthenticator};
    use mako_storage::{ProductionVolumeIdentity, provision_production_volume};

    use super::*;
    use crate::graph::test_support::{DeploymentEnvironment, config_for, local_tempdir, tenant};

    const ROOT_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn backup_scope() -> TenantScope {
        TenantScope::new(
            ProjectId::parse(BACKUP_WORKER_PROJECT_ID).expect("project"),
            EnvironmentId::parse(BACKUP_WORKER_ENVIRONMENT_ID).expect("environment"),
        )
    }

    fn signed(scope: &TenantScope, idempotency_key: &str) -> HttpRequest {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        let body = serde_json::to_vec(&StorageCheckpointRequest {
            backup_id: format!("dp-{idempotency_key}"),
        })
        .expect("body");
        let request = InternalRequestAuthenticator::new(
            DeploymentKey::derive(ROOT_KEY).expect("key"),
            InternalCaller::BackupWorker,
        )
        .sign(
            InternalRoute::StorageCheckpoint,
            scope,
            // `HttpRequest::for_test` always carries this transport request id,
            // and verification binds the signature to it.
            "req_test_support",
            idempotency_key,
            now,
            body,
        )
        .expect("signed");
        HttpRequest::for_test(
            HttpMethod::Post,
            InternalRoute::StorageCheckpoint.path(),
            request.headers,
            request.body,
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40_000)),
        )
    }

    fn production_graph(root: &std::path::Path) -> (Arc<DataPlaneGraph>, std::path::PathBuf) {
        let config = config_for(root, DeploymentEnvironment::Production);
        provision_production_volume(
            &config.rocksdb.path,
            ProductionVolumeIdentity::new("mako-data-plane", "mako-data-plane-us-east-1-beta")
                .expect("identity"),
        )
        .expect("provision");
        let checkpoints = mako_storage::online_checkpoint_root(&config.rocksdb.path);
        (
            Arc::new(DataPlaneGraph::open(&config).expect("production graph")),
            checkpoints,
        )
    }

    #[test]
    fn the_backup_worker_gets_a_linked_checkpoint_once_per_signed_request() {
        let directory = local_tempdir("data-plane-online-checkpoint");
        let (graph, checkpoints) = production_graph(directory.path());
        let request = signed(&backup_scope(), "checkpoint-0001");

        let response = handle_storage_checkpoint(&graph, &request).expect("checkpoint");
        assert_eq!(response.status_for_test(), 200);
        let body: StorageCheckpointResponse =
            serde_json::from_slice(response.body_for_test().expect("body")).expect("response");
        assert_eq!(body.backup_id, "dp-checkpoint-0001");
        let path = std::path::PathBuf::from(&body.checkpoint_path);
        assert!(path.starts_with(&checkpoints));
        assert!(path.join("CURRENT").is_file(), "a RocksDB checkpoint");

        let replay = handle_storage_checkpoint(&graph, &request).expect_err("replay");
        assert_eq!(replay.envelope().error.code, ErrorCode::Conflict);
    }

    #[test]
    fn a_checkpoint_signed_for_a_tenant_scope_is_refused() {
        let directory = local_tempdir("data-plane-checkpoint-scope");
        let (graph, checkpoints) = production_graph(directory.path());
        let error = handle_storage_checkpoint(&graph, &signed(&tenant(), "checkpoint-scope"))
            .expect_err("tenant scope refused");
        assert_eq!(error.envelope().error.code, ErrorCode::PermissionDenied);
        assert_eq!(fs::read_dir(checkpoints).map_or(0, Iterator::count), 0);
    }

    #[test]
    fn local_storage_takes_no_online_checkpoints() {
        let directory = local_tempdir("data-plane-checkpoint-local");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let graph = Arc::new(DataPlaneGraph::open(&config).expect("graph"));
        let error = handle_storage_checkpoint(&graph, &signed(&backup_scope(), "checkpoint-local"))
            .expect_err("local storage refuses");
        assert_eq!(error.envelope().error.code, ErrorCode::Unavailable);
    }
}
