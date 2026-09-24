use mako_api::{EnvironmentId, ProjectId, TenantScope};
use mako_service_runtime::{HttpApiError, HttpRequest, HttpResponse};
use serde::Serialize;
use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::management_http::{format_timestamp, internal, invalid, json};

pub(crate) fn tenant(request: &HttpRequest) -> Result<TenantScope, HttpApiError> {
    let project = ProjectId::parse(request.path_parameter("projectId").unwrap_or_default())
        .map_err(|_| invalid(request, "project path is invalid"))?;
    let environment =
        EnvironmentId::parse(request.path_parameter("environmentId").unwrap_or_default())
            .map_err(|_| invalid(request, "environment path is invalid"))?;
    Ok(TenantScope::new(project, environment))
}

pub(crate) fn project_id(request: &HttpRequest) -> Result<ProjectId, HttpApiError> {
    ProjectId::parse(request.path_parameter("projectId").unwrap_or_default())
        .map_err(|_| invalid(request, "project path is invalid"))
}

pub(crate) fn query_value<'a>(
    request: &'a HttpRequest,
    name: &str,
) -> Result<Option<&'a str>, HttpApiError> {
    let mut values = request
        .query()
        .iter()
        .filter_map(|(key, value)| (key == name).then_some(value.as_str()));
    let value = values.next();
    if values.next().is_some() {
        return Err(invalid(request, "query parameter is duplicated"));
    }
    Ok(value)
}

/// Refuses a request body and nothing else. A handler that reads query
/// parameters uses this with `reject_unknown_query`, not `no_payload`, which
/// also refuses every query parameter.
pub(crate) fn no_body(request: &HttpRequest) -> Result<(), HttpApiError> {
    if request.body().is_empty() {
        Ok(())
    } else {
        Err(invalid(request, "request body is not supported"))
    }
}

pub(crate) fn reject_unknown_query(
    request: &HttpRequest,
    allowed: &[&str],
) -> Result<(), HttpApiError> {
    if request
        .query()
        .iter()
        .any(|(name, _)| !allowed.contains(&name.as_str()))
    {
        Err(invalid(request, "query parameter is unsupported"))
    } else {
        Ok(())
    }
}

pub(crate) fn public_json<T: Serialize>(
    request: &HttpRequest,
    status: u16,
    value: &T,
) -> Result<HttpResponse, HttpApiError> {
    let value = serde_json::to_value(value)
        .map_err(|_| internal(request, "response serialization failed"))?;
    public_value(request, status, value)
}

pub(crate) fn public_value(
    request: &HttpRequest,
    status: u16,
    mut value: Value,
) -> Result<HttpResponse, HttpApiError> {
    normalize(request, &mut value)?;
    json(request, status, &value)
}

fn normalize(request: &HttpRequest, value: &mut Value) -> Result<(), HttpApiError> {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize(request, item)?;
            }
        }
        Value::Object(object) => {
            flatten_user_view(object);
            normalize_credential_issue(object);
            object.remove("tenant");
            object.remove("scope");
            let keys = object.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                let Some(mut field) = object.remove(&key) else {
                    continue;
                };
                let public_key =
                    timestamp_field(&key).map_or_else(|| key.clone(), |(name, _)| name);
                if let Some((_, milliseconds)) = timestamp_field(&key) {
                    field = timestamp_value(request, field, milliseconds)?;
                } else {
                    normalize(request, &mut field)?;
                }
                object.insert(public_key, field);
            }
        }
        _ => {}
    }
    Ok(())
}

fn flatten_user_view(object: &mut Map<String, Value>) {
    let Some(Value::Object(mut user)) = object.remove("user") else {
        return;
    };
    user.remove("scope");
    for (name, value) in user {
        object.insert(name, value);
    }
}

fn normalize_credential_issue(object: &mut Map<String, Value>) {
    let Some(metadata) = object.remove("metadata") else {
        return;
    };
    let Some(Value::String(value)) = object.remove("credential") else {
        object.insert("metadata".to_owned(), metadata);
        return;
    };
    object.insert("credential".to_owned(), metadata);
    object.insert("value".to_owned(), Value::String(value));
}

fn timestamp_field(name: &str) -> Option<(String, bool)> {
    if name == "atUnixSeconds" {
        return Some(("timestamp".to_owned(), false));
    }
    name.strip_suffix("UnixMilliseconds")
        .map(|public| (public.to_owned(), true))
        .or_else(|| {
            name.strip_suffix("UnixSeconds")
                .map(|public| (public.to_owned(), false))
        })
}

fn timestamp_value(
    request: &HttpRequest,
    value: Value,
    milliseconds: bool,
) -> Result<Value, HttpApiError> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Number(number) => {
            let raw = number
                .as_u64()
                .ok_or_else(|| internal(request, "stored timestamp is invalid"))?;
            let formatted = if milliseconds {
                i128::from(raw)
                    .checked_mul(1_000_000)
                    .and_then(|nanos| OffsetDateTime::from_unix_timestamp_nanos(nanos).ok())
                    .and_then(|time| time.format(&Rfc3339).ok())
                    .ok_or_else(|| internal(request, "stored timestamp is invalid"))?
            } else {
                format_timestamp(request, raw)?
            };
            Ok(Value::String(formatted))
        }
        _ => Err(internal(request, "stored timestamp is invalid")),
    }
}
