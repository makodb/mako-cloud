//! Append-only audit event contracts and persistence.

#![forbid(unsafe_code)]

mod log_store;
mod redaction;
mod retention;
mod schema;
mod store;
mod usage;

pub use log_store::{LogAppendOutcome, LogStore, LogStoreConfig, LogStoreError};
pub use redaction::{RedactedText, RedactionError, RedactionReport, TelemetryRedactor};
pub use retention::{DetailCompactionReport, RetentionMode};
pub use schema::{
    ActorIdentity, AttributeValue, AuditEvent, AuditOutcome, CorrelationId, LogLevel, MetricEvent,
    MetricValue, ObservabilityEvent, RequestId, ResourceReference, SafeAttributes, SchemaError,
    SignalContext, SignalScope, StructuredLogEvent, TELEMETRY_SCHEMA_VERSION, TraceId,
    TraceSpanEvent, TraceStatus, UsageEvent, UsageResource,
};
pub use store::{
    AppendOutcome, AuditActorKind, AuditCategory, AuditCompactionReport, AuditExport,
    AuditExportRequest, AuditFilter, AuditPage, AuditRecord, AuditStore, AuditStoreConfig,
    AuditStoreError, CursorSigningKey,
};
pub use usage::{
    UsageAggregate, UsageAggregation, UsageAggregationQuery, UsageRecordOutcome, UsageStore,
    UsageStoreConfig, UsageStoreError,
};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "audit";
