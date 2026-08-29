//! Management routes for an environment's application-user email templates.

use std::sync::Arc;

use mako_api::{ErrorCode, RetryAdvice};
use mako_control_plane::{EmailTemplateError, EmailTemplateKind, EmailTemplateText};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlPlaneGraph,
    http_support::{public_json, tenant},
    management_http::{
        conflict, forbidden, invalid, no_payload, no_query, not_found, parse_json,
        require_idempotency, require_json, unavailable, with_developer,
    },
};

pub(crate) fn add_email_template_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/email-templates",
            handle_list as Handler,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/email-templates/{templateKind}",
            handle_get,
        ),
        (
            HttpMethod::Put,
            "/v1/projects/{projectId}/environments/{environmentId}/email-templates/{templateKind}",
            handle_update,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/email-templates/{templateKind}",
            handle_reset,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/email-templates/{templateKind}/actions/preview",
            handle_preview,
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
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let items = graph
            .email_template_service()
            .list(&actor, &tenant, now)
            .await
            .map_err(|error| template_error(request, error))?;
        public_json(request, 200, &ListWire { items })
    })
}

fn handle_get(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let kind = template_kind(request)?;
    with_developer(graph, request, |actor, now| async move {
        let view = graph
            .email_template_service()
            .get(&actor, &tenant, kind, now)
            .await
            .map_err(|error| template_error(request, error))?;
        public_json(request, 200, &view)
    })
}

fn handle_update(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let body: EmailTemplateText = parse_json(request)?;
    let tenant = tenant(request)?;
    let kind = template_kind(request)?;
    with_developer(graph, request, |actor, now| async move {
        let view = graph
            .email_template_service()
            .update(&actor, &tenant, kind, body, now)
            .await
            .map_err(|error| template_error(request, error))?;
        public_json(request, 200, &view)
    })
}

fn handle_reset(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let kind = template_kind(request)?;
    with_developer(graph, request, |actor, now| async move {
        let view = graph
            .email_template_service()
            .reset(&actor, &tenant, kind, now)
            .await
            .map_err(|error| template_error(request, error))?;
        public_json(request, 200, &view)
    })
}

fn handle_preview(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    // The body is optional: without one the stored template is previewed.
    let body: PreviewWire = if request.body().is_empty() {
        PreviewWire::default()
    } else {
        require_json(request)?;
        parse_json(request)?
    };
    let tenant = tenant(request)?;
    let kind = template_kind(request)?;
    with_developer(graph, request, |actor, now| async move {
        let render = graph
            .email_template_service()
            .preview(&actor, &tenant, kind, body.subject, body.text_body, now)
            .await
            .map_err(|error| template_error(request, error))?;
        public_json(request, 200, &render)
    })
}

fn template_kind(request: &HttpRequest) -> Result<EmailTemplateKind, HttpApiError> {
    EmailTemplateKind::parse(request.path_parameter("templateKind").unwrap_or_default()).ok_or_else(
        || {
            invalid(
                request,
                "template kind must be verification, recovery, invitation, or magic_link",
            )
        },
    )
}

fn template_error(request: &HttpRequest, error: EmailTemplateError) -> HttpApiError {
    match error {
        EmailTemplateError::NotFound => not_found(request, "environment was not found"),
        EmailTemplateError::Forbidden => {
            forbidden(request, "email template operation is forbidden")
        }
        EmailTemplateError::Conflict => conflict(
            request,
            "email template changed concurrently; read it again",
        ),
        // The validation message names the field and the rule it broke; it
        // is derived from the developer's own text, never from stored data.
        EmailTemplateError::InvalidTemplate(message) => HttpApiError::new(
            400,
            ErrorCode::InvalidRequest,
            message,
            request.request_id(),
            RetryAdvice::Never,
        ),
        _ => unavailable(request, "email template service is unavailable"),
    }
}

#[derive(Serialize)]
struct ListWire {
    items: Vec<mako_control_plane::EmailTemplateView>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PreviewWire {
    subject: Option<String>,
    text_body: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_template_kind_is_a_bad_request() {
        let request = HttpRequest::for_test(
            HttpMethod::Get,
            "/v1/projects/prj_example00/environments/env_example00/email-templates/",
            Vec::new(),
            Vec::new(),
            None,
        );
        let error = template_kind(&request).expect_err("no path parameter");
        assert_eq!(error.envelope().error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn preview_body_is_optional_and_partial() {
        let empty: PreviewWire = serde_json::from_str("{}").expect("empty object");
        assert!(empty.subject.is_none() && empty.text_body.is_none());
        let partial: PreviewWire =
            serde_json::from_str(r#"{"subject":"Hi {{project_name}}"}"#).expect("subject only");
        assert_eq!(partial.subject.as_deref(), Some("Hi {{project_name}}"));
        assert!(partial.text_body.is_none());
        assert!(serde_json::from_str::<PreviewWire>(r#"{"html":"<b>no</b>"}"#).is_err());
    }
}
