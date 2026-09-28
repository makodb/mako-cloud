use std::{num::NonZeroUsize, sync::Arc};

use mako_api::{
    DataJobConfirmationRequest, DataJobCreateRequest, DataJobDryRunRequest,
    EXPLORER_MAX_ACTIVE_JOBS_PER_TENANT, EXPLORER_MAX_UPLOAD_BYTES, ErrorCode, RetryAdvice,
};
use mako_control_plane::{ArtifactMethod, DataJobError};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::{
    ControlPlaneGraph,
    http_support::tenant,
    management_http::{
        conflict, forbidden, invalid, json, no_payload, no_query, not_found, parse_json,
        require_idempotency, require_json, unavailable, with_developer,
    },
};

pub(crate) fn add_data_job_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs",
            list_jobs as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs",
            create_job,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}",
            get_job,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/actions/cancel",
            cancel_job,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/actions/dry-run",
            dry_run,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/actions/confirm",
            confirm_job,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/artifact-grants/upload",
            upload_grant,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/artifact-grants/download",
            download_grant,
        ),
        (
            HttpMethod::Put,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/artifact",
            upload_artifact,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/artifact",
            download_artifact,
        ),
    ] {
        let graph = Arc::clone(&graph);
        // The artifact upload's body is the import file itself, which may be
        // as large as the documented upload bound; every other route keeps the
        // listener's bound.
        let body_limit = (method == HttpMethod::Put).then_some(EXPLORER_MAX_UPLOAD_BYTES as usize);
        router.add_route_with_body_limit(method, path, body_limit, move |request| {
            handler(&graph, &request)
        })?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn list_jobs(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, _| async move {
        let items = graph
            .data_job_service()
            .list(&actor, &tenant, NonZeroUsize::new(100).expect("job limit"))
            .await
            .map_err(|error| data_job_error(request, error))?;
        json(request, 200, &Items { items })
    })
}

fn create_job(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let tenant = tenant(request)?;
    let idempotency = require_idempotency(request)?.to_owned();
    let body: DataJobCreateRequest = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let job = graph
            .data_job_service()
            .create(&actor, &tenant, body, &idempotency, now)
            .await
            .map_err(|error| data_job_error(request, error))?;
        json(request, 202, &job)
    })
}

fn get_job(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let job_id = job_id(request)?.to_owned();
    with_developer(graph, request, |actor, _| async move {
        let job = graph
            .data_job_service()
            .get(&actor, &tenant, &job_id)
            .await
            .map_err(|error| data_job_error(request, error))?;
        json(request, 200, &job)
    })
}

fn cancel_job(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let job_id = job_id(request)?.to_owned();
    with_developer(graph, request, |actor, now| async move {
        let job = graph
            .data_job_service()
            .cancel(&actor, &tenant, &job_id, now)
            .await
            .map_err(|error| data_job_error(request, error))?;
        json(request, 202, &job)
    })
}

fn dry_run(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let tenant = tenant(request)?;
    let job_id = job_id(request)?.to_owned();
    let body: DataJobDryRunRequest = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let current = graph
            .data_job_service()
            .get(&actor, &tenant, &job_id)
            .await
            .map_err(|error| data_job_error(request, error))?;
        if current.manifest.as_ref().map(|manifest| &manifest.digest) == Some(&body.upload_digest) {
            return json(request, 200, &current);
        }
        let job = graph
            .data_job_service()
            .dry_run_import(
                &actor,
                &tenant,
                &job_id,
                &body.upload_digest,
                body.schema_version,
                now,
            )
            .await
            .map_err(|error| data_job_error(request, error))?;
        json(request, 200, &job)
    })
}

fn confirm_job(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let tenant = tenant(request)?;
    let job_id = job_id(request)?.to_owned();
    let body: DataJobConfirmationRequest = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let job = graph
            .data_job_service()
            .confirm(
                &actor,
                &tenant,
                &job_id,
                &body.expected_manifest_digest,
                body.acknowledge_partial_import_cancellation,
                now,
            )
            .await
            .map_err(|error| data_job_error(request, error))?;
        json(request, 202, &job)
    })
}

fn upload_grant(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    artifact_grant(graph, request, ArtifactMethod::Upload)
}

fn download_grant(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    artifact_grant(graph, request, ArtifactMethod::Download)
}

fn artifact_grant(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    method: ArtifactMethod,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let job_id = job_id(request)?.to_owned();
    with_developer(graph, request, |actor, now| async move {
        let grant = graph
            .data_job_service()
            .artifact_grant(&actor, &tenant, &job_id, method, now)
            .await
            .map_err(|error| data_job_error(request, error))?;
        json(request, 201, &grant)
    })
}

fn upload_artifact(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let grant = only_grant_query(request)?.to_owned();
    let digest = request
        .header("digest")
        .and_then(|value| value.strip_prefix("sha-256="))
        .ok_or_else(|| invalid(request, "artifact digest header is required"))?
        .to_owned();
    let digest = format!("sha256:{digest}");
    let tenant = tenant(request)?;
    let job_id = job_id(request)?.to_owned();
    let bytes: Arc<[u8]> = Arc::from(request.body());
    with_developer(graph, request, |actor, now| async move {
        let job = graph
            .data_job_service()
            .upload_import(&actor, &tenant, &job_id, &grant, &digest, bytes, now)
            .await
            .map_err(|error| data_job_error(request, error))?;
        json(request, 202, &job)
    })
}

fn download_artifact(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let grant = only_grant_query(request)?.to_owned();
    let tenant = tenant(request)?;
    let job_id = job_id(request)?.to_owned();
    with_developer(graph, request, |actor, now| async move {
        let bytes = graph
            .data_job_service()
            .download_export(&actor, &tenant, &job_id, &grant, now)
            .await
            .map_err(|error| data_job_error(request, error))?;
        let digest = format!("sha-256={:x}", Sha256::digest(&bytes));
        HttpResponse::bytes(200, "application/x-ndjson", bytes.to_vec())
            .with_header("Digest", &digest)
            .map_err(|_| unavailable(request, "artifact response is unavailable"))
    })
}

fn only_grant_query(request: &HttpRequest) -> Result<&str, HttpApiError> {
    if request.query().len() != 1 || request.query()[0].0 != "grant" {
        return Err(invalid(request, "artifact grant query is required"));
    }
    let value = request.query()[0].1.as_str();
    if value.len() > 8_192 || value.chars().any(char::is_whitespace) {
        return Err(invalid(request, "artifact grant is invalid"));
    }
    Ok(value)
}

fn job_id(request: &HttpRequest) -> Result<&str, HttpApiError> {
    request
        .path_parameter("jobId")
        .filter(|value| value.starts_with("djob_") && value.len() <= 80)
        .ok_or_else(|| not_found(request, "data job was not found"))
}

fn data_job_error(request: &HttpRequest, error: DataJobError) -> HttpApiError {
    match error {
        DataJobError::InvalidRequest
        | DataJobError::InvalidArtifact
        | DataJobError::InvalidArtifactGrant => invalid(request, "data-job input is invalid"),
        DataJobError::InvalidState | DataJobError::Conflict => conflict(
            request,
            "data-job state changed or does not permit this action",
        ),
        DataJobError::NotFound => not_found(request, "data job was not found"),
        DataJobError::Forbidden => forbidden(request, "data-job action is forbidden"),
        // Jobs waiting for an upload or a confirmation count too, so say
        // what to clear rather than only that a limit was reached.
        DataJobError::QuotaExceeded => HttpApiError::new(
            409,
            ErrorCode::Conflict,
            format!(
                "this environment already has {EXPLORER_MAX_ACTIVE_JOBS_PER_TENANT} unfinished data jobs; finish or cancel one first (mako-cloud data jobs list)"
            ),
            request.request_id(),
            RetryAdvice::Never,
        ),
        ref mismatch @ DataJobError::SchemaVersionMismatch { .. } => HttpApiError::new(
            409,
            ErrorCode::Conflict,
            mismatch.to_string(),
            request.request_id(),
            RetryAdvice::Never,
        ),
        _ => unavailable(request, "data-job service is unavailable"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Items<T> {
    items: Vec<T>,
}
