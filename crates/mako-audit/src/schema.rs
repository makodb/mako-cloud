use std::{collections::BTreeMap, error::Error, fmt};

use mako_api::TenantScope;
use serde::{Deserialize, Serialize};

/// Version shared by all structured telemetry and audit records.
pub const TELEMETRY_SCHEMA_VERSION: u16 = 1;
const MAX_ATTRIBUTES: usize = 32;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SchemaError> {
        let value = value.into();
        if !value.starts_with("req_") || !safe_identifier(&value, 8, 128) {
            return Err(SchemaError::InvalidRequestId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(&self) -> Result<(), SchemaError> {
        if self.0.starts_with("req_") && safe_identifier(&self.0, 8, 128) {
            Ok(())
        } else {
            Err(SchemaError::InvalidRequestId)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CorrelationId(String);

impl CorrelationId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SchemaError> {
        let value = value.into();
        if !safe_identifier(&value, 8, 128) {
            return Err(SchemaError::InvalidCorrelationId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(&self) -> Result<(), SchemaError> {
        if safe_identifier(&self.0, 8, 128) {
            Ok(())
        } else {
            Err(SchemaError::InvalidCorrelationId)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TraceId(String);

impl TraceId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SchemaError> {
        let value = value.into();
        let w3c = value.len() == 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            && value.bytes().any(|byte| byte != b'0');
        let internal = value.starts_with("trc_") && safe_identifier(&value, 8, 128);
        if !w3c && !internal {
            return Err(SchemaError::InvalidTraceId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(&self) -> Result<(), SchemaError> {
        Self::parse(self.0.clone()).map(|_| ())
    }
}

/// Explicit scope label. Tenant variants always contain both project and
/// environment identifiers through `TenantScope`; unscoped empty strings are
/// impossible.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SignalScope {
    Tenant {
        tenant: TenantScope,
        organization_id: Option<String>,
    },
    Organization {
        organization_id: String,
    },
    Platform,
}

impl SignalScope {
    fn validate(&self) -> Result<(), SchemaError> {
        match self {
            Self::Tenant {
                organization_id, ..
            } => validate_optional_identifier(organization_id),
            Self::Organization { organization_id } => {
                validate_identifier(organization_id, 128, SchemaError::InvalidScope)
            }
            Self::Platform => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ActorIdentity {
    ApplicationUser {
        actor_id: String,
        session_id: String,
    },
    Developer {
        actor_id: String,
    },
    Service {
        actor_id: String,
    },
    Operator {
        actor_id: String,
        support_session_id: Option<String>,
    },
    System {
        component: String,
    },
    Anonymous,
}

impl ActorIdentity {
    fn validate(&self) -> Result<(), SchemaError> {
        match self {
            Self::ApplicationUser {
                actor_id,
                session_id,
            } => {
                validate_identifier(actor_id, 128, SchemaError::InvalidActor)?;
                validate_identifier(session_id, 128, SchemaError::InvalidActor)
            }
            Self::Developer { actor_id } | Self::Service { actor_id } => {
                validate_identifier(actor_id, 128, SchemaError::InvalidActor)
            }
            Self::Operator {
                actor_id,
                support_session_id,
            } => {
                validate_identifier(actor_id, 128, SchemaError::InvalidActor)?;
                validate_optional_identifier(support_session_id)
                    .map_err(|_| SchemaError::InvalidActor)
            }
            Self::System { component } => {
                validate_identifier(component, 128, SchemaError::InvalidActor)
            }
            Self::Anonymous => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceReference {
    kind: String,
    id: String,
}

impl ResourceReference {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Result<Self, SchemaError> {
        let resource = Self {
            kind: kind.into(),
            id: id.into(),
        };
        resource.validate()?;
        Ok(resource)
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    fn validate(&self) -> Result<(), SchemaError> {
        validate_name(&self.kind, 64, SchemaError::InvalidResource)?;
        validate_identifier(&self.id, 512, SchemaError::InvalidResource)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalContext {
    schema_version: u16,
    scope: SignalScope,
    actor: ActorIdentity,
    resource: ResourceReference,
    request_id: RequestId,
    correlation_id: CorrelationId,
    trace_id: Option<TraceId>,
}

impl SignalContext {
    pub fn new(
        scope: SignalScope,
        actor: ActorIdentity,
        resource: ResourceReference,
        request_id: RequestId,
        correlation_id: CorrelationId,
        trace_id: Option<TraceId>,
    ) -> Result<Self, SchemaError> {
        let context = Self {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            scope,
            actor,
            resource,
            request_id,
            correlation_id,
            trace_id,
        };
        context.validate()?;
        Ok(context)
    }

    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    #[must_use]
    pub const fn scope(&self) -> &SignalScope {
        &self.scope
    }

    #[must_use]
    pub const fn actor(&self) -> &ActorIdentity {
        &self.actor
    }

    #[must_use]
    pub const fn resource(&self) -> &ResourceReference {
        &self.resource
    }

    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    #[must_use]
    pub const fn correlation_id(&self) -> &CorrelationId {
        &self.correlation_id
    }

    #[must_use]
    pub const fn trace_id(&self) -> Option<&TraceId> {
        self.trace_id.as_ref()
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        if self.schema_version != TELEMETRY_SCHEMA_VERSION {
            return Err(SchemaError::UnsupportedVersion);
        }
        self.scope.validate()?;
        self.actor.validate()?;
        self.resource.validate()?;
        self.request_id.validate()?;
        self.correlation_id.validate()?;
        self.trace_id.as_ref().map_or(Ok(()), TraceId::validate)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AttributeValue {
    Text(String),
    Integer(i64),
    Unsigned(u64),
    Boolean(bool),
}

impl AttributeValue {
    fn validate(&self) -> Result<(), SchemaError> {
        if let Self::Text(value) = self {
            validate_text(value, 2_048, SchemaError::InvalidAttribute)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SafeAttributes(pub(crate) BTreeMap<String, AttributeValue>);

impl SafeAttributes {
    pub fn try_from_iter(
        values: impl IntoIterator<Item = (String, AttributeValue)>,
    ) -> Result<Self, SchemaError> {
        let attributes = Self(values.into_iter().collect());
        attributes.validate()?;
        Ok(attributes)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&AttributeValue> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &AttributeValue)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        if self.0.len() > MAX_ATTRIBUTES {
            return Err(SchemaError::TooManyAttributes);
        }
        for (name, value) in &self.0 {
            validate_name(name, 128, SchemaError::InvalidAttribute)?;
            value.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredLogEvent {
    pub context: SignalContext,
    pub occurred_at_unix_milliseconds: u64,
    pub service: String,
    pub region: String,
    pub level: LogLevel,
    pub event: String,
    pub message: String,
    pub attributes: SafeAttributes,
}

impl StructuredLogEvent {
    pub fn validate(&self) -> Result<(), SchemaError> {
        validate_common(
            &self.context,
            self.occurred_at_unix_milliseconds,
            &self.service,
            &self.region,
        )?;
        validate_name(&self.event, 128, SchemaError::InvalidEvent)?;
        validate_text(&self.message, 4_096, SchemaError::InvalidMessage)?;
        self.attributes.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum MetricValue {
    Counter {
        delta: u64,
    },
    Gauge {
        value: i64,
    },
    Distribution {
        count: u64,
        sum: u64,
        minimum: u64,
        maximum: u64,
    },
}

impl MetricValue {
    fn validate(&self) -> Result<(), SchemaError> {
        if let Self::Distribution {
            count,
            sum,
            minimum,
            maximum,
        } = self
            && (*count == 0
                || minimum > maximum
                || *sum < minimum.saturating_mul(*count)
                || *sum > maximum.saturating_mul(*count))
        {
            return Err(SchemaError::InvalidMetric);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricEvent {
    pub context: SignalContext,
    pub occurred_at_unix_milliseconds: u64,
    pub service: String,
    pub region: String,
    pub name: String,
    pub unit: String,
    pub value: MetricValue,
    pub attributes: SafeAttributes,
}

impl MetricEvent {
    pub fn validate(&self) -> Result<(), SchemaError> {
        validate_common(
            &self.context,
            self.occurred_at_unix_milliseconds,
            &self.service,
            &self.region,
        )?;
        validate_name(&self.name, 128, SchemaError::InvalidMetric)?;
        validate_name(&self.unit, 64, SchemaError::InvalidMetric)?;
        self.value.validate()?;
        self.attributes.validate()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    Ok,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSpanEvent {
    pub context: SignalContext,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub operation: String,
    pub started_at_unix_microseconds: u64,
    pub duration_microseconds: u64,
    pub status: TraceStatus,
    pub attributes: SafeAttributes,
}

impl TraceSpanEvent {
    pub fn validate(&self) -> Result<(), SchemaError> {
        self.context.validate()?;
        if self.context.trace_id().is_none()
            || !valid_span_id(&self.span_id)
            || self
                .parent_span_id
                .as_ref()
                .is_some_and(|span_id| !valid_span_id(span_id))
            || self.started_at_unix_microseconds == 0
            || self.duration_microseconds == 0
        {
            return Err(SchemaError::InvalidTrace);
        }
        validate_name(&self.operation, 128, SchemaError::InvalidTrace)?;
        self.attributes.validate()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageResource {
    StorageBytes,
    ReplicationRequests,
    ReplicationBytes,
    AuthenticationActivity,
    FunctionInvocations,
    FunctionCpuMilliseconds,
    FunctionWallMilliseconds,
    LogBytes,
    EgressRequests,
    EgressBytes,
}

impl UsageResource {
    #[must_use]
    pub const fn expected_unit(self) -> &'static str {
        match self {
            Self::StorageBytes | Self::ReplicationBytes | Self::LogBytes | Self::EgressBytes => {
                "bytes"
            }
            Self::ReplicationRequests | Self::EgressRequests => "requests",
            Self::AuthenticationActivity => "events",
            Self::FunctionInvocations => "invocations",
            Self::FunctionCpuMilliseconds | Self::FunctionWallMilliseconds => "milliseconds",
        }
    }

    #[must_use]
    pub const fn is_gauge(self) -> bool {
        matches!(self, Self::StorageBytes)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub context: SignalContext,
    pub usage_id: String,
    pub occurred_at_unix_milliseconds: u64,
    pub resource: UsageResource,
    pub quantity: u64,
    pub unit: String,
    pub attributes: SafeAttributes,
}

impl UsageEvent {
    pub fn validate(&self) -> Result<(), SchemaError> {
        self.context.validate()?;
        validate_identifier(&self.usage_id, 128, SchemaError::InvalidUsage)?;
        validate_timestamp(self.occurred_at_unix_milliseconds)?;
        validate_name(&self.unit, 64, SchemaError::InvalidUsage)?;
        if self.unit != self.resource.expected_unit() {
            return Err(SchemaError::InvalidUsage);
        }
        self.attributes.validate()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Allowed,
    Denied,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub context: SignalContext,
    pub event_id: String,
    pub occurred_at_unix_milliseconds: u64,
    pub action: String,
    pub outcome: AuditOutcome,
    pub reason_code: String,
    pub details: SafeAttributes,
}

impl AuditEvent {
    pub fn validate(&self) -> Result<(), SchemaError> {
        self.context.validate()?;
        validate_identifier(&self.event_id, 128, SchemaError::InvalidAudit)?;
        validate_timestamp(self.occurred_at_unix_milliseconds)?;
        validate_name(&self.action, 128, SchemaError::InvalidAudit)?;
        validate_name(&self.reason_code, 128, SchemaError::InvalidAudit)?;
        self.details.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "signal", content = "record", rename_all = "snake_case")]
pub enum ObservabilityEvent {
    Log(StructuredLogEvent),
    Metric(MetricEvent),
    Trace(TraceSpanEvent),
    Usage(UsageEvent),
    Audit(AuditEvent),
}

impl ObservabilityEvent {
    #[must_use]
    pub const fn context(&self) -> &SignalContext {
        match self {
            Self::Log(event) => &event.context,
            Self::Metric(event) => &event.context,
            Self::Trace(event) => &event.context,
            Self::Usage(event) => &event.context,
            Self::Audit(event) => &event.context,
        }
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        match self {
            Self::Log(event) => event.validate(),
            Self::Metric(event) => event.validate(),
            Self::Trace(event) => event.validate(),
            Self::Usage(event) => event.validate(),
            Self::Audit(event) => event.validate(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaError {
    UnsupportedVersion,
    InvalidScope,
    InvalidActor,
    InvalidResource,
    InvalidRequestId,
    InvalidCorrelationId,
    InvalidTraceId,
    InvalidTimestamp,
    InvalidService,
    InvalidRegion,
    InvalidEvent,
    InvalidMessage,
    InvalidAttribute,
    TooManyAttributes,
    InvalidMetric,
    InvalidTrace,
    InvalidUsage,
    InvalidAudit,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedVersion => "observability schema version is unsupported",
            Self::InvalidScope => "observability scope is invalid",
            Self::InvalidActor => "observability actor is invalid",
            Self::InvalidResource => "observability resource is invalid",
            Self::InvalidRequestId => "observability request identifier is invalid",
            Self::InvalidCorrelationId => "observability correlation identifier is invalid",
            Self::InvalidTraceId => "observability trace identifier is invalid",
            Self::InvalidTimestamp => "observability timestamp is invalid",
            Self::InvalidService => "observability service is invalid",
            Self::InvalidRegion => "observability region is invalid",
            Self::InvalidEvent => "structured log event is invalid",
            Self::InvalidMessage => "structured log message is invalid",
            Self::InvalidAttribute => "observability attribute is invalid",
            Self::TooManyAttributes => "observability record has too many attributes",
            Self::InvalidMetric => "metric record is invalid",
            Self::InvalidTrace => "trace record is invalid",
            Self::InvalidUsage => "usage record is invalid",
            Self::InvalidAudit => "audit record is invalid",
        })
    }
}

impl Error for SchemaError {}

fn validate_common(
    context: &SignalContext,
    timestamp: u64,
    service: &str,
    region: &str,
) -> Result<(), SchemaError> {
    context.validate()?;
    validate_timestamp(timestamp)?;
    validate_name(service, 128, SchemaError::InvalidService)?;
    validate_name(region, 64, SchemaError::InvalidRegion)
}

fn validate_timestamp(timestamp: u64) -> Result<(), SchemaError> {
    if timestamp == 0 {
        Err(SchemaError::InvalidTimestamp)
    } else {
        Ok(())
    }
}

fn validate_optional_identifier(value: &Option<String>) -> Result<(), SchemaError> {
    value.as_ref().map_or(Ok(()), |value| {
        validate_identifier(value, 128, SchemaError::InvalidScope)
    })
}

fn validate_identifier(value: &str, max: usize, error: SchemaError) -> Result<(), SchemaError> {
    if safe_identifier(value, 1, max) {
        Ok(())
    } else {
        Err(error)
    }
}

fn validate_name(value: &str, max: usize, error: SchemaError) -> Result<(), SchemaError> {
    if !value.is_empty()
        && value.len() <= max
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        Ok(())
    } else {
        Err(error)
    }
}

fn validate_text(value: &str, max: usize, error: SchemaError) -> Result<(), SchemaError> {
    if !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(error)
    }
}

fn safe_identifier(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/' | b'@')
        })
}

fn valid_span_id(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        && value.bytes().any(|byte| byte != b'0')
}

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId};

    use super::*;

    #[test]
    fn every_signal_serializes_with_shared_scope_and_correlation_fields() {
        let context = context(Some(
            TraceId::parse("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace"),
        ));
        let attributes = SafeAttributes::try_from_iter([
            ("http.status_code".to_owned(), AttributeValue::Unsigned(200)),
            ("cache.hit".to_owned(), AttributeValue::Boolean(true)),
        ])
        .expect("attributes");
        let events = [
            ObservabilityEvent::Log(StructuredLogEvent {
                context: context.clone(),
                occurred_at_unix_milliseconds: 1_000,
                service: "sync-gateway".to_owned(),
                region: "local".to_owned(),
                level: LogLevel::Info,
                event: "request.completed".to_owned(),
                message: "replication request completed".to_owned(),
                attributes: attributes.clone(),
            }),
            ObservabilityEvent::Metric(MetricEvent {
                context: context.clone(),
                occurred_at_unix_milliseconds: 1_000,
                service: "sync-gateway".to_owned(),
                region: "local".to_owned(),
                name: "requests.total".to_owned(),
                unit: "requests".to_owned(),
                value: MetricValue::Counter { delta: 1 },
                attributes: attributes.clone(),
            }),
            ObservabilityEvent::Trace(TraceSpanEvent {
                context: context.clone(),
                span_id: "00f067aa0ba902b7".to_owned(),
                parent_span_id: None,
                operation: "replication.pull".to_owned(),
                started_at_unix_microseconds: 1_000_000,
                duration_microseconds: 500,
                status: TraceStatus::Ok,
                attributes: attributes.clone(),
            }),
            ObservabilityEvent::Usage(UsageEvent {
                context: context.clone(),
                usage_id: "usg_example00".to_owned(),
                occurred_at_unix_milliseconds: 1_000,
                resource: UsageResource::ReplicationRequests,
                quantity: 1,
                unit: "requests".to_owned(),
                attributes: attributes.clone(),
            }),
            ObservabilityEvent::Audit(AuditEvent {
                context,
                event_id: "aud_example00".to_owned(),
                occurred_at_unix_milliseconds: 1_000,
                action: "replication.pull".to_owned(),
                outcome: AuditOutcome::Allowed,
                reason_code: "authorized".to_owned(),
                details: attributes,
            }),
        ];

        for event in events {
            event.validate().expect("valid event");
            let value = serde_json::to_value(event).expect("serialize event");
            let record = &value["record"];
            assert_eq!(record["context"]["schemaVersion"], 1);
            assert_eq!(
                record["context"]["scope"]["tenant"]["projectId"],
                "prj_example00"
            );
            assert_eq!(
                record["context"]["scope"]["tenant"]["environmentId"],
                "env_example00"
            );
            assert_eq!(record["context"]["actor"]["actorId"], "usr_example00");
            assert_eq!(record["context"]["resource"]["kind"], "collection");
            assert_eq!(record["context"]["requestId"], "req_example00");
            assert_eq!(record["context"]["correlationId"], "cor_example00");
        }
    }

    #[test]
    fn invalid_or_ambiguous_records_fail_validation() {
        assert_eq!(
            RequestId::parse("example"),
            Err(SchemaError::InvalidRequestId)
        );
        assert_eq!(
            TraceId::parse("00000000000000000000000000000000"),
            Err(SchemaError::InvalidTraceId)
        );
        assert_eq!(
            ResourceReference::new("Collection", "todos"),
            Err(SchemaError::InvalidResource)
        );
        let too_many = SafeAttributes::try_from_iter((0..=MAX_ATTRIBUTES).map(|index| {
            (
                format!("field.{index}"),
                AttributeValue::Unsigned(index as u64),
            )
        }));
        assert_eq!(too_many, Err(SchemaError::TooManyAttributes));

        let trace = TraceSpanEvent {
            context: context(None),
            span_id: "00f067aa0ba902b7".to_owned(),
            parent_span_id: None,
            operation: "replication.pull".to_owned(),
            started_at_unix_microseconds: 1,
            duration_microseconds: 1,
            status: TraceStatus::Error,
            attributes: SafeAttributes::default(),
        };
        assert_eq!(trace.validate(), Err(SchemaError::InvalidTrace));
    }

    fn context(trace_id: Option<TraceId>) -> SignalContext {
        SignalContext::new(
            SignalScope::Tenant {
                tenant: TenantScope::new(
                    ProjectId::parse("prj_example00").expect("project"),
                    EnvironmentId::parse("env_example00").expect("environment"),
                ),
                organization_id: Some("org_example00".to_owned()),
            },
            ActorIdentity::ApplicationUser {
                actor_id: "usr_example00".to_owned(),
                session_id: "ses_example00".to_owned(),
            },
            ResourceReference::new("collection", "todos").expect("resource"),
            RequestId::parse("req_example00").expect("request"),
            CorrelationId::parse("cor_example00").expect("correlation"),
            trace_id,
        )
        .expect("context")
    }
}
