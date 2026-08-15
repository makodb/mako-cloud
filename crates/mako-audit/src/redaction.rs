use std::{error::Error, fmt};

use crate::{AttributeValue, ObservabilityEvent, SafeAttributes};

const REDACTED: &str = "[REDACTED]";
const REDACTED_PAYLOAD: &str = "[REDACTED STRUCTURED PAYLOAD]";
const MAX_SENSITIVE_VALUES: usize = 256;
const MAX_SENSITIVE_VALUE_BYTES: usize = 64 * 1024;

/// Central fail-closed redactor for every telemetry producer.
pub struct TelemetryRedactor {
    sensitive_values: Vec<String>,
}

impl TelemetryRedactor {
    pub fn new<I, S>(sensitive_values: I) -> Result<Self, RedactionError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut values = Vec::new();
        for value in sensitive_values {
            let value = value.as_ref();
            if value.is_empty()
                || value.len() > MAX_SENSITIVE_VALUE_BYTES
                || value.contains('\0')
                || values.len() >= MAX_SENSITIVE_VALUES
            {
                return Err(RedactionError::InvalidSensitiveValue);
            }
            if !values.iter().any(|existing| existing == value) {
                values.push(value.to_owned());
            }
        }
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        Ok(Self {
            sensitive_values: values,
        })
    }

    /// Sanitizes a complete shared event before it crosses a process, storage,
    /// or export boundary. The result remains valid under the shared schema.
    pub fn sanitize_event(
        &self,
        event: &mut ObservabilityEvent,
    ) -> Result<RedactionReport, RedactionError> {
        let mut report = RedactionReport::default();
        match event {
            ObservabilityEvent::Log(event) => {
                let redacted = self.redact_text(&event.message);
                event.message = redacted.text;
                report.redacted_values += redacted.replacements;
                report.merge(self.sanitize_attributes(&mut event.attributes));
            }
            ObservabilityEvent::Metric(event) => {
                report.merge(self.sanitize_attributes(&mut event.attributes));
            }
            ObservabilityEvent::Trace(event) => {
                report.merge(self.sanitize_attributes(&mut event.attributes));
            }
            ObservabilityEvent::Usage(event) => {
                report.merge(self.sanitize_attributes(&mut event.attributes));
            }
            ObservabilityEvent::Audit(event) => {
                report.merge(self.sanitize_attributes(&mut event.details));
            }
        }
        event
            .validate()
            .map_err(|_| RedactionError::InvalidSanitizedEvent)?;
        Ok(report)
    }

    /// Sanitizes unstructured text for legacy producers while they migrate to
    /// the shared schema.
    #[must_use]
    pub fn redact_text(&self, source: &str) -> RedactedText {
        if looks_like_structured_payload(source) {
            return RedactedText {
                text: REDACTED_PAYLOAD.to_owned(),
                replacements: 1,
            };
        }
        let mut text = source.to_owned();
        let mut replacements = 0;
        for secret in &self.sensitive_values {
            let count = text.matches(secret).count();
            if count > 0 {
                text = text.replace(secret, REDACTED);
                replacements += count;
            }
        }
        replacements += redact_bearer_values(&mut text);
        for marker in [
            "password=",
            "password:",
            "passwd=",
            "passwd:",
            "authorization=",
            "authorization:",
            "cookie=",
            "cookie:",
        ] {
            replacements += redact_assignment_values(&mut text, marker);
        }
        replacements +=
            redact_prefixed_values(&mut text, &["mako_sk.", "mako_pk.", "mako_at.", "mako_rt."]);
        replacements += redact_jwt_values(&mut text);
        let controls = text
            .chars()
            .filter(|character| character.is_control())
            .count();
        if controls > 0 {
            text = text
                .chars()
                .map(|character| {
                    if character.is_control() {
                        ' '
                    } else {
                        character
                    }
                })
                .collect();
            replacements += controls;
        }
        RedactedText { text, replacements }
    }

    fn sanitize_attributes(&self, attributes: &mut SafeAttributes) -> RedactionReport {
        let before = attributes.0.len();
        attributes
            .0
            .retain(|name, _| !sensitive_attribute_name(name));
        let mut report = RedactionReport {
            dropped_attributes: before - attributes.0.len(),
            redacted_values: 0,
        };
        for value in attributes.0.values_mut() {
            if let AttributeValue::Text(text) = value {
                let redacted = self.redact_text(text);
                *text = redacted.text;
                report.redacted_values += redacted.replacements;
            }
        }
        report
    }
}

impl fmt::Debug for TelemetryRedactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelemetryRedactor")
            .field("sensitive_value_count", &self.sensitive_values.len())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedText {
    text: String,
    replacements: usize,
}

impl RedactedText {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.text
    }

    #[must_use]
    pub const fn replacements(&self) -> usize {
        self.replacements
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RedactionReport {
    pub dropped_attributes: usize,
    pub redacted_values: usize,
}

impl RedactionReport {
    fn merge(&mut self, other: Self) {
        self.dropped_attributes += other.dropped_attributes;
        self.redacted_values += other.redacted_values;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedactionError {
    InvalidSensitiveValue,
    InvalidSanitizedEvent,
}

impl fmt::Display for RedactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSensitiveValue => "telemetry sensitive value is invalid",
            Self::InvalidSanitizedEvent => "sanitized telemetry event is invalid",
        })
    }
}

impl Error for RedactionError {}

fn sensitive_attribute_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase().replace('-', "_");
    if normalized.contains("private_key") || normalized.contains("api_key") {
        return true;
    }
    let parts = normalized.split(['.', '_']).collect::<Vec<_>>();
    parts.iter().any(|part| {
        matches!(
            *part,
            "password"
                | "passwd"
                | "passphrase"
                | "authorization"
                | "cookie"
                | "token"
                | "credential"
                | "secret"
                | "privatekey"
                | "apikey"
        )
    }) || parts
        .iter()
        .any(|part| matches!(*part, "body" | "payload" | "document"))
}

fn looks_like_structured_payload(source: &str) -> bool {
    let trimmed = source.trim();
    (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
}

fn redact_assignment_values(text: &mut String, marker: &str) -> usize {
    let mut replacements = 0;
    let mut search_from = 0;
    loop {
        let lowercase = text[search_from..].to_ascii_lowercase();
        let Some(relative) = lowercase.find(marker) else {
            break;
        };
        let value_start = search_from + relative + marker.len();
        let value_start = text[value_start..]
            .char_indices()
            .find(|(_, character)| !character.is_whitespace() && *character != '"')
            .map_or(value_start, |(offset, _)| value_start + offset);
        let value_end = token_end(text, value_start);
        if value_end <= value_start {
            search_from = value_start;
            continue;
        }
        text.replace_range(value_start..value_end, REDACTED);
        replacements += 1;
        search_from = value_start + REDACTED.len();
    }
    replacements
}

fn redact_prefixed_values(text: &mut String, prefixes: &[&str]) -> usize {
    let mut replacements = 0;
    for prefix in prefixes {
        let mut search_from = 0;
        while let Some(relative) = text[search_from..].find(prefix) {
            let start = search_from + relative;
            let end = token_end(text, start);
            text.replace_range(start..end, REDACTED);
            replacements += 1;
            search_from = start + REDACTED.len();
        }
    }
    replacements
}

fn redact_bearer_values(text: &mut String) -> usize {
    let mut replacements = 0;
    let mut search_from = 0;
    loop {
        let lowercase = text[search_from..].to_ascii_lowercase();
        let Some(relative) = lowercase.find("bearer ") else {
            break;
        };
        let start = search_from + relative + "bearer ".len();
        let end = token_end(text, start);
        if end <= start {
            break;
        }
        text.replace_range(start..end, REDACTED);
        replacements += 1;
        search_from = start + REDACTED.len();
    }
    replacements
}

fn redact_jwt_values(text: &mut String) -> usize {
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            start.get_or_insert(index);
        } else if let Some(token_start) = start.take() {
            let candidate = &text[token_start..index];
            if candidate.len() >= 16
                && candidate.split('.').count() == 3
                && candidate.split('.').all(|part| {
                    part.len() >= 2
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                })
            {
                ranges.push((token_start, index));
            }
        }
    }
    for (start, end) in ranges.iter().rev() {
        text.replace_range(*start..*end, REDACTED);
    }
    ranges.len()
}

fn token_end(text: &str, start: usize) -> usize {
    text[start..]
        .char_indices()
        .find(|(_, character)| {
            character.is_whitespace()
                || matches!(*character, '"' | '\'' | ',' | ';' | ')' | ']' | '}')
        })
        .map_or(text.len(), |(offset, _)| start + offset)
}

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId, TenantScope};

    use crate::{
        ActorIdentity, AttributeValue, CorrelationId, LogLevel, ObservabilityEvent, RequestId,
        ResourceReference, SafeAttributes, SignalContext, SignalScope, StructuredLogEvent, TraceId,
    };

    use super::*;

    #[test]
    fn central_redaction_drops_sensitive_fields_and_scrubs_all_text_forms() {
        let secret = "function-secret-canary";
        let redactor = TelemetryRedactor::new([secret]).expect("redactor");
        assert!(!format!("{redactor:?}").contains(secret));
        let mut event = ObservabilityEvent::Log(StructuredLogEvent {
            context: context(),
            occurred_at_unix_milliseconds: 1_000,
            service: "edge-runtime".to_owned(),
            region: "local".to_owned(),
            level: LogLevel::Error,
            event: "function.failed".to_owned(),
            message: format!(
                "password=hunter2 Authorization: Bearer abc.def.ghi credential=mako_sk.example.value {secret}"
            ),
            attributes: SafeAttributes::try_from_iter([
                (
                    "document.body".to_owned(),
                    AttributeValue::Text("private document".to_owned()),
                ),
                (
                    "refresh_token".to_owned(),
                    AttributeValue::Text("raw token".to_owned()),
                ),
                (
                    "error.message".to_owned(),
                    AttributeValue::Text(format!("failed with {secret}")),
                ),
            ])
            .expect("attributes"),
        });

        let report = redactor.sanitize_event(&mut event).expect("redaction");
        assert_eq!(report.dropped_attributes, 2);
        assert!(report.redacted_values >= 5);
        let encoded = serde_json::to_string(&event).expect("serialize");
        for forbidden in [
            secret,
            "hunter2",
            "abc.def.ghi",
            "mako_sk.example.value",
            "private document",
            "raw token",
        ] {
            assert!(!encoded.contains(forbidden), "leaked {forbidden}");
        }
        assert!(encoded.contains(REDACTED));
    }

    #[test]
    fn structured_payloads_and_controls_are_removed_from_legacy_text() {
        let redactor = TelemetryRedactor::new(Vec::<String>::new()).expect("redactor");
        assert_eq!(
            redactor
                .redact_text(r#"{"body":{"email":"private@example.test"}}"#)
                .as_str(),
            REDACTED_PAYLOAD
        );
        assert_eq!(
            redactor.redact_text("safe\nmessage").as_str(),
            "safe message"
        );
    }

    #[test]
    fn invalid_sensitive_inputs_fail_without_echoing_values() {
        let error = TelemetryRedactor::new([""]).expect_err("empty secret");
        assert_eq!(error, RedactionError::InvalidSensitiveValue);
        assert!(!error.to_string().contains("secret"));
    }

    fn context() -> SignalContext {
        SignalContext::new(
            SignalScope::Tenant {
                tenant: TenantScope::new(
                    ProjectId::parse("prj_example00").expect("project"),
                    EnvironmentId::parse("env_example00").expect("environment"),
                ),
                organization_id: Some("org_example00".to_owned()),
            },
            ActorIdentity::System {
                component: "edge-runtime".to_owned(),
            },
            ResourceReference::new("function", "hello-world").expect("resource"),
            RequestId::parse("req_example00").expect("request"),
            CorrelationId::parse("cor_example00").expect("correlation"),
            Some(TraceId::parse("trc_example00").expect("trace")),
        )
        .expect("context")
    }
}
