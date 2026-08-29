//! Management routes for a project's custom domains.
//!
//! Domains are project-level: the routes hang off the project, the body
//! names the environment the hostname serves, and authorization is the
//! project-settings rule (any member reads, a member who may mutate
//! projects writes). Every route is a thin rendering of the domain service
//! in the shapes the OpenAPI document promises.

use std::sync::Arc;

use mako_api::EnvironmentId;
use mako_control_plane::{
    CUSTOM_DOMAIN_VERIFICATION_RECORD_TYPE, CustomDomainError, CustomDomainId, CustomDomainRecord,
    CustomDomainState, NewCustomDomain,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlPlaneGraph,
    http_support::project_id,
    management_http::{
        conflict, forbidden, format_timestamp, invalid, json, no_payload, no_query, not_found,
        parse_json, require_idempotency, require_json, unavailable, with_developer,
    },
};

pub(crate) fn add_custom_domain_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/domains",
            handle_list as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/domains",
            handle_create,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/domains/{domainId}",
            handle_get,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/domains/{domainId}",
            handle_delete,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/domains/{domainId}/actions/verify",
            handle_verify,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_list(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let project = project_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let domains = graph
            .custom_domain_service()
            .list(&actor, &project, now)
            .await
            .map_err(|error| domain_error(request, error))?;
        let mut items = Vec::with_capacity(domains.len());
        for domain in &domains {
            items.push(domain_wire(request, domain)?);
        }
        json(request, 200, &ItemsWire { items })
    })
}

fn handle_create(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let project = project_id(request)?;
    let body: CreateWire = parse_json(request)?;
    let environment_id = EnvironmentId::parse(body.environment_id)
        .map_err(|_| invalid(request, "environmentId is invalid"))?;
    let input = NewCustomDomain {
        hostname: body.hostname,
        environment_id,
    };
    with_developer(graph, request, |actor, now| async move {
        let created = graph
            .custom_domain_service()
            .create(&actor, &project, input, now)
            .await
            .map_err(|error| domain_error(request, error))?;
        json(request, 201, &domain_wire(request, &created)?)
    })
}

fn handle_get(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let project = project_id(request)?;
    let id = domain_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let domain = graph
            .custom_domain_service()
            .get(&actor, &project, &id, now)
            .await
            .map_err(|error| domain_error(request, error))?;
        json(request, 200, &domain_wire(request, &domain)?)
    })
}

fn handle_delete(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_idempotency(request)?;
    let project = project_id(request)?;
    let id = domain_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        graph
            .custom_domain_service()
            .delete(&actor, &project, &id, now)
            .await
            .map_err(|error| domain_error(request, error))?;
        Ok(HttpResponse::empty(204))
    })
}

fn handle_verify(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_idempotency(request)?;
    let project = project_id(request)?;
    let id = domain_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let domain = graph
            .custom_domain_service()
            .verify(&actor, &project, &id, now)
            .await
            .map_err(|error| domain_error(request, error))?;
        json(request, 200, &domain_wire(request, &domain)?)
    })
}

fn domain_id(request: &HttpRequest) -> Result<CustomDomainId, HttpApiError> {
    CustomDomainId::parse(request.path_parameter("domainId").unwrap_or_default())
        .map_err(|_| invalid(request, "custom domain path is invalid"))
}

/// The service names every refusal in terms a developer can act on; the
/// messages cross as they are. Storage and publication failures read as
/// unavailability.
fn domain_error(request: &HttpRequest, error: CustomDomainError) -> HttpApiError {
    match error {
        CustomDomainError::ProjectNotFound => not_found(request, "project was not found"),
        CustomDomainError::EnvironmentNotFound => not_found(request, "environment was not found"),
        CustomDomainError::DomainNotFound => not_found(request, "custom domain was not found"),
        CustomDomainError::Forbidden => forbidden(request, "custom domain operation is forbidden"),
        CustomDomainError::Conflict(message) => conflict(request, message),
        CustomDomainError::InvalidRequest(message)
        | CustomDomainError::InvalidHostname(message) => invalid(request, message),
        _ => unavailable(request, "custom domain service is unavailable"),
    }
}

fn domain_wire(
    request: &HttpRequest,
    domain: &CustomDomainRecord,
) -> Result<DomainWire, HttpApiError> {
    Ok(DomainWire {
        id: domain.id().as_str().to_owned(),
        project_id: domain.project_id().as_str().to_owned(),
        environment_id: domain.environment_id().as_str().to_owned(),
        hostname: domain.hostname().to_owned(),
        state: domain.state(),
        verification: VerificationWire {
            record_name: domain.record_name(),
            record_type: CUSTOM_DOMAIN_VERIFICATION_RECORD_TYPE,
            record_value: domain.record_value().to_owned(),
        },
        verified_at: domain
            .verified_at_unix_seconds()
            .map(|at| format_timestamp(request, at))
            .transpose()?,
        last_checked_at: domain
            .last_checked_at_unix_seconds()
            .map(|at| format_timestamp(request, at))
            .transpose()?,
        last_error: domain.last_error().map(str::to_owned),
        created_at: format_timestamp(request, domain.created_at_unix_seconds())?,
        updated_at: format_timestamp(request, domain.updated_at_unix_seconds())?,
    })
}

/// `CustomDomainCreate` on the wire.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateWire {
    hostname: String,
    environment_id: String,
}

#[derive(Serialize)]
struct ItemsWire<T> {
    items: Vec<T>,
}

/// `CustomDomainVerification` on the wire.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerificationWire {
    record_name: String,
    record_type: &'static str,
    record_value: String,
}

/// `CustomDomain` on the wire.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DomainWire {
    id: String,
    project_id: String,
    environment_id: String,
    hostname: String,
    state: CustomDomainState,
    verification: VerificationWire,
    verified_at: Option<String>,
    last_checked_at: Option<String>,
    last_error: Option<String>,
    created_at: String,
    updated_at: String,
}

#[cfg(test)]
mod tests {
    use mako_api::ErrorCode;
    use serde_json::json;

    use super::*;

    #[test]
    fn the_create_body_follows_the_contract() {
        let create: CreateWire = serde_json::from_value(json!({
            "hostname": "api.example.com",
            "environmentId": "env_example00",
        }))
        .expect("create");
        assert_eq!(create.hostname, "api.example.com");
        assert_eq!(create.environment_id, "env_example00");
        assert!(
            serde_json::from_value::<CreateWire>(json!({ "hostname": "api.example.com" })).is_err(),
            "environmentId is required"
        );
        assert!(
            serde_json::from_value::<CreateWire>(json!({
                "hostname": "api.example.com",
                "environmentId": "env_example00",
                "state": "verified",
            }))
            .is_err(),
            "unknown fields are refused"
        );
    }

    #[test]
    fn path_ids_are_validated_and_errors_map_to_the_public_codes() {
        let request = HttpRequest::for_test(
            HttpMethod::Get,
            "/v1/projects/prj_example00/domains/",
            Vec::new(),
            Vec::new(),
            None,
        );
        assert_eq!(
            domain_id(&request)
                .expect_err("missing id")
                .envelope()
                .error
                .code,
            ErrorCode::InvalidRequest
        );
        for (error, code) in [
            (CustomDomainError::ProjectNotFound, ErrorCode::NotFound),
            (CustomDomainError::EnvironmentNotFound, ErrorCode::NotFound),
            (CustomDomainError::DomainNotFound, ErrorCode::NotFound),
            (CustomDomainError::Forbidden, ErrorCode::PermissionDenied),
            (
                CustomDomainError::Conflict("hostname is already claimed by a project"),
                ErrorCode::Conflict,
            ),
            (
                CustomDomainError::InvalidHostname("hostname must have at least two labels"),
                ErrorCode::InvalidRequest,
            ),
            (CustomDomainError::CorruptRecord, ErrorCode::Unavailable),
        ] {
            assert_eq!(domain_error(&request, error).envelope().error.code, code);
        }
        assert_eq!(
            domain_error(
                &request,
                CustomDomainError::InvalidHostname("hostname must not be localhost")
            )
            .envelope()
            .error
            .message,
            "hostname must not be localhost"
        );
    }

    #[test]
    fn wire_shapes_match_the_openapi_document() {
        let domain = DomainWire {
            id: "dom_abcdefghijklmnop".to_owned(),
            project_id: "prj_example00".to_owned(),
            environment_id: "env_example00".to_owned(),
            hostname: "api.example.com".to_owned(),
            state: CustomDomainState::Failed,
            verification: VerificationWire {
                record_name: "_mako-verify.api.example.com".to_owned(),
                record_type: CUSTOM_DOMAIN_VERIFICATION_RECORD_TYPE,
                record_value: "mako-domain-verify=abc".to_owned(),
            },
            verified_at: Some("2027-01-01T00:00:00Z".to_owned()),
            last_checked_at: Some("2027-01-02T00:00:00Z".to_owned()),
            last_error: Some("record_missing".to_owned()),
            created_at: "2027-01-01T00:00:00Z".to_owned(),
            updated_at: "2027-01-02T00:00:00Z".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(ItemsWire {
                items: vec![domain]
            })
            .expect("json"),
            json!({
                "items": [{
                    "id": "dom_abcdefghijklmnop",
                    "projectId": "prj_example00",
                    "environmentId": "env_example00",
                    "hostname": "api.example.com",
                    "state": "failed",
                    "verification": {
                        "recordName": "_mako-verify.api.example.com",
                        "recordType": "TXT",
                        "recordValue": "mako-domain-verify=abc",
                    },
                    "verifiedAt": "2027-01-01T00:00:00Z",
                    "lastCheckedAt": "2027-01-02T00:00:00Z",
                    "lastError": "record_missing",
                    "createdAt": "2027-01-01T00:00:00Z",
                    "updatedAt": "2027-01-02T00:00:00Z",
                }],
            })
        );
        let pending = DomainWire {
            id: "dom_abcdefghijklmnop".to_owned(),
            project_id: "prj_example00".to_owned(),
            environment_id: "env_example00".to_owned(),
            hostname: "api.example.com".to_owned(),
            state: CustomDomainState::Pending,
            verification: VerificationWire {
                record_name: "_mako-verify.api.example.com".to_owned(),
                record_type: CUSTOM_DOMAIN_VERIFICATION_RECORD_TYPE,
                record_value: "mako-domain-verify=abc".to_owned(),
            },
            verified_at: None,
            last_checked_at: None,
            last_error: None,
            created_at: "2027-01-01T00:00:00Z".to_owned(),
            updated_at: "2027-01-01T00:00:00Z".to_owned(),
        };
        let value = serde_json::to_value(pending).expect("json");
        assert_eq!(value["state"], json!("pending"));
        assert_eq!(
            value["verifiedAt"],
            json!(null),
            "nulls are present, never omitted"
        );
        assert_eq!(value["lastCheckedAt"], json!(null));
        assert_eq!(value["lastError"], json!(null));
    }
}
