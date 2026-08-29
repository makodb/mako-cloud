use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Wire-contract version for public API failures.
pub const API_ERROR_VERSION: &str = "v1";

/// Stable, machine-readable error categories exposed by Mako Cloud APIs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Unauthenticated,
    OperatorStepUpRequired,
    PermissionDenied,
    NotFound,
    Conflict,
    /// A request precondition (`If-Match`, `If-None-Match`) did not hold
    /// against the current state; answered `412`.
    PreconditionFailed,
    SchemaMismatch,
    CheckpointExpired,
    RateLimited,
    QuotaExceeded,
    Unavailable,
    Internal,
}

/// Tells a client whether and when repeating a failed request is safe.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RetryAdvice {
    Never,
    Immediate,
    AfterDelay { after_ms: u64 },
}

/// Restricted values allowed in public error details.
///
/// Document bodies and arbitrary nested JSON are deliberately excluded so a
/// service must make an explicit schema change before exposing richer data.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SafeDetail {
    String(String),
    Boolean(bool),
    Integer(i64),
    Decimal(f64),
    Strings(Vec<String>),
}

pub type SafeDetails = BTreeMap<String, SafeDetail>;

/// Error information shared across every public API.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub request_id: String,
    pub retry: RetryAdvice,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: SafeDetails,
}

impl ApiError {
    pub fn new(
        code: ErrorCode,
        message: impl Into<String>,
        request_id: impl Into<String>,
        retry: RetryAdvice,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            request_id: request_id.into(),
            retry,
            details: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_detail(mut self, key: impl Into<String>, value: SafeDetail) -> Self {
        self.details.insert(key.into(), value);
        self
    }
}

/// Versioned top-level failure response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorEnvelope {
    pub api_version: String,
    pub error: ApiError,
}

impl ApiErrorEnvelope {
    pub fn new(error: ApiError) -> Self {
        Self {
            api_version: API_ERROR_VERSION.to_owned(),
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn serializes_the_v1_wire_shape() {
        let envelope = ApiErrorEnvelope::new(
            ApiError::new(
                ErrorCode::RateLimited,
                "request rate exceeded",
                "req_123",
                RetryAdvice::AfterDelay { after_ms: 1_500 },
            )
            .with_detail("limit", SafeDetail::Integer(100)),
        );

        assert_eq!(
            serde_json::to_value(envelope).expect("error envelope must serialize"),
            json!({
                "apiVersion": "v1",
                "error": {
                    "code": "rate_limited",
                    "message": "request rate exceeded",
                    "requestId": "req_123",
                    "retry": {
                        "kind": "after_delay",
                        "afterMs": 1500
                    },
                    "details": {
                        "limit": 100
                    }
                }
            })
        );
    }

    #[test]
    fn omits_empty_details() {
        let envelope = ApiErrorEnvelope::new(ApiError::new(
            ErrorCode::NotFound,
            "resource not found",
            "req_456",
            RetryAdvice::Never,
        ));

        let value = serde_json::to_value(envelope).expect("error envelope must serialize");
        assert!(value["error"].get("details").is_none());
    }
}
