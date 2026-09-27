use std::sync::Arc;

use mako_control_plane::{
    CredentialAdminError, FunctionSecretIssue, FunctionSecretName, FunctionSecretValue,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    ControlPlaneGraph,
    http_support::{public_json, public_value, tenant},
    management_http::{
        conflict, forbidden, invalid, no_payload, no_query, not_found, parse_json,
        require_idempotency, require_json, unavailable, with_developer,
    },
};

pub(crate) fn add_credential_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/function-secrets",
            handle_create_secret as Handler,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/function-secrets/{secretName}",
            handle_get_secret,
        ),
        (
            HttpMethod::Put,
            "/v1/projects/{projectId}/environments/{environmentId}/function-secrets/{secretName}",
            handle_create_secret_value,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/function-secrets/{secretName}",
            handle_retire_secret,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/function-secrets/{secretName}/actions/rotate",
            handle_rotate_secret,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_create_secret(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: SecretNameWire = parse_json(request)?;
    let name = FunctionSecretName::parse(body.name)
        .map_err(|_| invalid(request, "function secret name is invalid"))?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let issue = graph
            .credential_service()
            .create_function_secret(&actor, &tenant, name, now)
            .await
            .map_err(|error| create_secret_error(request, error))?;
        secret_issue(request, 201, issue)
    })
}

/// Creates a secret from a value the caller already holds -- a scoped service
/// credential a function needs, most often -- rather than one the platform
/// generates. It is stored, attached by version, injected, and redacted
/// exactly like a generated value; the only difference is where the bytes came
/// from, and that they are never echoed back, so the response is metadata
/// only.
fn handle_create_secret_value(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: SecretValueWire = parse_json(request)?;
    let value = FunctionSecretValue::parse(body.value)
        .map_err(|_| invalid(request, "function secret value is invalid"))?;
    let tenant = tenant(request)?;
    let name = secret_name(request)?;
    with_developer(graph, request, |actor, now| async move {
        let metadata = graph
            .credential_service()
            .create_function_secret_with_value(&actor, &tenant, name, value, now)
            .await
            .map_err(|error| create_secret_error(request, error))?;
        public_json(request, 201, &metadata)
    })
}

fn handle_get_secret(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let name = secret_name(request)?;
    with_developer(graph, request, |actor, now| async move {
        let metadata = graph
            .credential_service()
            .function_secret_metadata(&actor, &tenant, &name, now)
            .await
            .map_err(|error| credential_error(request, error))?;
        public_json(request, 200, &metadata)
    })
}

fn handle_retire_secret(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let name = secret_name(request)?;
    with_developer(graph, request, |actor, now| async move {
        let metadata = graph
            .credential_service()
            .retire_function_secret(&actor, &tenant, &name, now)
            .await
            .map_err(|error| credential_error(request, error))?;
        public_json(request, 200, &metadata)
    })
}

fn handle_rotate_secret(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_idempotency(request)?;
    // A rotation may carry the replacement, because a name is written once and
    // a function reads its secret by that name: without this, a secret that
    // holds the wrong value holds it for the life of the environment, and a
    // function whose credential must change has nowhere to put the new one.
    let supplied = if request.body().is_empty() {
        None
    } else {
        require_json(request)?;
        let body: SecretValueWire = parse_json(request)?;
        Some(
            FunctionSecretValue::parse(body.value)
                .map_err(|_| invalid(request, "function secret value is invalid"))?,
        )
    };
    let tenant = tenant(request)?;
    let name = secret_name(request)?;
    with_developer(graph, request, |actor, now| async move {
        let service = graph.credential_service();
        // A supplied value is never echoed, so that rotation answers with the
        // same disclosure as supplied creation: metadata only.
        match supplied {
            Some(value) => {
                let metadata = service
                    .rotate_function_secret_with_value(&actor, &tenant, &name, value, now)
                    .await
                    .map_err(|error| credential_error(request, error))?;
                public_json(request, 201, &metadata)
            }
            None => {
                let issue = service
                    .rotate_function_secret(&actor, &tenant, &name, now)
                    .await
                    .map_err(|error| credential_error(request, error))?;
                secret_issue(request, 201, issue)
            }
        }
    })
}

fn secret_issue(
    request: &HttpRequest,
    status: u16,
    issue: FunctionSecretIssue,
) -> Result<HttpResponse, HttpApiError> {
    public_value(
        request,
        status,
        json!({
            "secret": issue.metadata,
            "value": issue.value.expose_once(),
        }),
    )
}

fn secret_name(request: &HttpRequest) -> Result<FunctionSecretName, HttpApiError> {
    FunctionSecretName::parse(
        request
            .path_parameter("secretName")
            .unwrap_or_default()
            .to_owned(),
    )
    .map_err(|_| invalid(request, "function secret path is invalid"))
}

/// A secret's name is written once, so a create that conflicts is one for a
/// name already in use; saying only "conflicts with current state" left the
/// developer to guess that rotation is how a value is replaced.
fn create_secret_error(request: &HttpRequest, error: CredentialAdminError) -> HttpApiError {
    match error {
        CredentialAdminError::Conflict => conflict(
            request,
            "a function secret with this name already exists; rotate it to replace its value",
        ),
        other => credential_error(request, other),
    }
}

fn credential_error(request: &HttpRequest, error: CredentialAdminError) -> HttpApiError {
    match error {
        CredentialAdminError::NotFound => not_found(request, "function secret was not found"),
        CredentialAdminError::Forbidden => forbidden(
            request,
            "your team role does not allow managing function secrets",
        ),
        CredentialAdminError::Conflict
        | CredentialAdminError::InvalidSecretState
        | CredentialAdminError::VersionExhausted => {
            conflict(request, "function secret conflicts with current state")
        }
        CredentialAdminError::InvalidSecretName | CredentialAdminError::InvalidSecretValue => {
            invalid(request, "function secret input is invalid")
        }
        _ => unavailable(request, "function secret service is unavailable"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretNameWire {
    name: String,
}

/// The supplied value never reaches a log or an error: it is parsed into
/// `FunctionSecretValue`, whose `Debug` is redacted, and dropped from there.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretValueWire {
    value: String,
}
