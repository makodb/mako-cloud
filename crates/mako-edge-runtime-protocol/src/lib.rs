//! Versioned messages exchanged with the replaceable edge-runtime supervisor.
//!
//! This crate deliberately contains no Supabase-specific control APIs. The
//! supervisor translates these Mako-owned messages to the pinned runtime.

#![forbid(unsafe_code)]

use std::{error::Error, fmt, net::IpAddr};

use mako_api::TenantScope;
use serde::{Deserialize, Serialize};

/// Header required on every request between an edge gateway, control plane,
/// and runtime supervisor.
pub const PROTOCOL_VERSION_HEADER: &str = "x-mako-runtime-protocol";
/// Current major version of the internal runtime protocol.
pub const PROTOCOL_VERSION: u16 = 1;
/// Correlation header that is safe to include in diagnostics.
pub const REQUEST_ID_HEADER: &str = "x-mako-request-id";
/// Trace propagation header used alongside W3C `traceparent`.
pub const TRACE_ID_HEADER: &str = "x-mako-trace-id";
/// Sensitive hop-by-hop header carrying the verified caller credential.
pub const CALLER_AUTHORIZATION_HEADER: &str = "x-mako-caller-authorization";
/// Deployment-managed credential used only on the loopback supervisor hop.
pub const RUNTIME_AUTHORIZATION_HEADER: &str = "x-mako-runtime-authorization";

pub const HEALTH_PATH: &str = "/_mako/runtime/v1/health";
pub const LOAD_DEPLOYMENT_PATH: &str = "/_mako/runtime/v1/deployments/load";
pub const PROBE_DEPLOYMENT_PATH: &str = "/_mako/runtime/v1/deployments/probe";
pub const TEST_DEPLOYMENT_PATH: &str = "/_mako/runtime/v1/deployments/test";
pub const DEPLOYMENT_LOGS_PATH: &str = "/_mako/runtime/v1/deployments/logs";
pub const RETIRE_DEPLOYMENT_PATH: &str = "/_mako/runtime/v1/deployments/retire";
pub const RETIRE_FUNCTION_PATH: &str = "/_mako/runtime/v1/functions/retire";

const EMBEDDED_RUNTIME_PIN: &str = include_str!("../../../infra/edge-runtime/runtime-pin.json");

/// Immutable upstream artifact selected for hosted and local execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimePin {
    pub provider: String,
    pub release: String,
    pub source_repository: String,
    pub source_commit: String,
    pub image_repository: String,
    pub image_digest: String,
    pub protocol_version: u16,
}

impl RuntimePin {
    /// Loads the repository-owned runtime pin embedded at compile time.
    pub fn embedded() -> Result<Self, ProtocolValidationError> {
        let pin: Self = serde_json::from_str(EMBEDDED_RUNTIME_PIN)
            .map_err(|_| ProtocolValidationError::InvalidRuntimePin)?;
        pin.validate()?;
        Ok(pin)
    }

    /// Returns the immutable OCI image reference used for pulls and deploys.
    #[must_use]
    pub fn image_reference(&self) -> String {
        format!("{}@{}", self.image_repository, self.image_digest)
    }

    fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.provider != "supabase-edge-runtime"
            || !valid_release(&self.release)
            || self.source_repository != "https://github.com/supabase/edge-runtime"
            || !valid_hex(&self.source_commit, 40)
            || self.image_repository != "docker.io/supabase/edge-runtime"
            || !valid_digest(&self.image_digest)
            || self.protocol_version != PROTOCOL_VERSION
        {
            return Err(ProtocolValidationError::InvalidRuntimePin);
        }
        Ok(())
    }
}

/// Uniquely addresses one immutable function deployment.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeploymentAddress {
    pub tenant: TenantScope,
    pub function_name: String,
    pub version: u64,
}

impl DeploymentAddress {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if !valid_function_name(&self.function_name) || self.version == 0 {
            return Err(ProtocolValidationError::InvalidDeploymentAddress);
        }
        Ok(())
    }
}

/// Bundle encoding accepted by the runtime adapter.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleFormat {
    SourceArchiveV1,
    Prebuilt,
}

/// Resource boundaries enforced for a deployment and every invocation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeLimits {
    pub cpu_milliseconds: u64,
    pub wall_milliseconds: u64,
    pub memory_bytes: u64,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub concurrency: u32,
    pub outbound_network: OutboundNetworkPolicy,
}

impl RuntimeLimits {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.cpu_milliseconds == 0
            || self.wall_milliseconds == 0
            || self.memory_bytes == 0
            || self.request_bytes == 0
            || self.response_bytes == 0
            || self.concurrency == 0
            || !self.outbound_network.is_valid()
        {
            return Err(ProtocolValidationError::InvalidLimits);
        }
        Ok(())
    }
}

/// Network access granted to one worker. There is deliberately no unrestricted
/// mode in protocol v1.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OutboundNetworkPolicy {
    DenyAll,
    AllowList {
        hosts: Vec<String>,
        max_requests_per_invocation: u32,
    },
}

impl OutboundNetworkPolicy {
    fn is_valid(&self) -> bool {
        match self {
            Self::DenyAll => true,
            Self::AllowList {
                hosts,
                max_requests_per_invocation,
            } => {
                *max_requests_per_invocation > 0
                    && !hosts.is_empty()
                    && hosts.len() <= 64
                    && hosts.iter().all(|host| valid_outbound_host(host))
            }
        }
    }
}

/// A secret identifier only. Secret values are delivered over a separate,
/// sensitive channel and never appear in deployment metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecretVersionReference {
    pub name: String,
    pub version: u64,
}

/// A deployment secret carried only in the authenticated sensitive load
/// envelope. Formatting never reveals the value.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SensitiveDeploymentSecret {
    pub reference: SecretVersionReference,
    value: String,
}

impl SensitiveDeploymentSecret {
    pub fn new(
        reference: SecretVersionReference,
        value: impl Into<String>,
    ) -> Result<Self, ProtocolValidationError> {
        let value = value.into();
        if !valid_environment_name(&reference.name)
            || reference.version == 0
            || value.is_empty()
            || value.len() > 64 * 1024
            || value.contains('\0')
        {
            return Err(ProtocolValidationError::InvalidDeployment);
        }
        Ok(Self { reference, value })
    }

    #[must_use]
    pub fn expose_to_supervisor(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for SensitiveDeploymentSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SensitiveDeploymentSecret")
            .field("reference", &self.reference)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// Idempotent control request used to load an immutable deployment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LoadDeploymentRequest {
    pub protocol_version: u16,
    pub deployment: DeploymentAddress,
    pub bundle_digest: String,
    pub bundle_format: BundleFormat,
    pub entrypoint: String,
    pub runtime_release: String,
    pub limits: RuntimeLimits,
    pub verify_jwt: bool,
    pub secret_versions: Vec<SecretVersionReference>,
}

impl LoadDeploymentRequest {
    pub fn validate(&self, pin: &RuntimePin) -> Result<(), ProtocolValidationError> {
        require_protocol(self.protocol_version)?;
        self.deployment.validate()?;
        self.limits.validate()?;
        if !valid_digest(&self.bundle_digest)
            || !valid_relative_path(&self.entrypoint)
            || self.runtime_release != pin.release
            || self.secret_versions.len() > 64
            || self
                .secret_versions
                .iter()
                .any(|secret| !valid_environment_name(&secret.name) || secret.version == 0)
            || has_duplicate_secret_references(&self.secret_versions)
        {
            return Err(ProtocolValidationError::InvalidDeployment);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SensitiveLoadDeploymentRequest {
    pub protocol_version: u16,
    pub request_id: String,
    pub manifest: LoadDeploymentRequest,
    pub bundle_base64: String,
    pub secrets: Vec<SensitiveDeploymentSecret>,
}

impl fmt::Debug for SensitiveLoadDeploymentRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SensitiveLoadDeploymentRequest")
            .field("protocol_version", &self.protocol_version)
            .field("request_id", &self.request_id)
            .field("manifest", &self.manifest)
            .field("bundle_bytes_base64", &self.bundle_base64.len())
            .field("secrets", &self.secrets)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeploymentOperationRequest {
    pub protocol_version: u16,
    pub request_id: String,
    pub deployment: DeploymentAddress,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionAddress {
    pub tenant: TenantScope,
    pub function_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionOperationRequest {
    pub protocol_version: u16,
    pub request_id: String,
    pub function: FunctionAddress,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestDeploymentRequest {
    pub protocol_version: u16,
    pub request_id: String,
    pub deployment: DeploymentAddress,
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body_base64: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestDeploymentResponse {
    pub protocol_version: u16,
    pub request_id: String,
    pub deployment: DeploymentAddress,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body_base64: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeploymentLogQueryRequest {
    pub protocol_version: u16,
    pub request_id: String,
    pub function: FunctionAddress,
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeploymentLogEntry {
    pub deployment: DeploymentAddress,
    pub timestamp_unix_milliseconds: u64,
    pub level: String,
    pub message: String,
    pub correlation_id: String,
    pub region: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeploymentLogPage {
    pub protocol_version: u16,
    pub request_id: String,
    pub function: FunctionAddress,
    pub items: Vec<DeploymentLogEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RetireFunctionResult {
    pub protocol_version: u16,
    pub request_id: String,
    pub function: FunctionAddress,
    pub retired_versions: usize,
}

fn has_duplicate_secret_references(references: &[SecretVersionReference]) -> bool {
    references.iter().enumerate().any(|(index, reference)| {
        references[..index].iter().any(|candidate| {
            candidate.name == reference.name && candidate.version == reference.version
        })
    })
}

/// Lifecycle state returned by load, probe, and retire operations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentState {
    Loading,
    Healthy,
    Unhealthy,
    Retired,
}

/// Sanitized result for a deployment lifecycle operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeploymentStatus {
    pub protocol_version: u16,
    pub deployment: DeploymentAddress,
    pub state: DeploymentState,
    pub region: String,
    pub diagnostic_code: Option<String>,
}

/// Runtime process readiness reported to trusted callers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeHealth {
    pub protocol_version: u16,
    pub runtime_release: String,
    pub source_commit: String,
    pub region: String,
    pub ready: bool,
}

/// Non-sensitive failure classes understood across gateway/runtime versions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeErrorCode {
    ProtocolMismatch,
    InvalidDeployment,
    DeploymentNotFound,
    DeploymentUnhealthy,
    ConcurrencyLimited,
    RequestTooLarge,
    ResponseTooLarge,
    CpuLimitExceeded,
    WallTimeExceeded,
    MemoryLimitExceeded,
    EgressDenied,
    WorkerCrashed,
    RuntimeUnavailable,
}

/// Stable error envelope for the internal protocol. It intentionally carries
/// no user-code exception text, request body, token, or secret value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeErrorEnvelope {
    pub protocol_version: u16,
    pub code: RuntimeErrorCode,
    pub request_id: String,
    pub retryable: bool,
}

pub fn require_protocol(version: u16) -> Result<(), ProtocolValidationError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtocolValidationError::ProtocolMismatch)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolValidationError {
    ProtocolMismatch,
    InvalidRuntimePin,
    InvalidDeploymentAddress,
    InvalidLimits,
    InvalidDeployment,
}

impl fmt::Display for ProtocolValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ProtocolMismatch => "edge runtime protocol version is unsupported",
            Self::InvalidRuntimePin => "edge runtime pin is invalid",
            Self::InvalidDeploymentAddress => "edge runtime deployment address is invalid",
            Self::InvalidLimits => "edge runtime limits are invalid",
            Self::InvalidDeployment => "edge runtime deployment is invalid",
        })
    }
}

impl Error for ProtocolValidationError {}

fn valid_release(value: &str) -> bool {
    let Some(version) = value.strip_prefix('v') else {
        return false;
    };
    let mut parts = version.split('.');
    parts.clone().count() == 3
        && parts.all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|digest| valid_hex(digest, 64))
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_function_name(value: &str) -> bool {
    value.len() <= 63
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        && !value.chars().any(char::is_control)
}

fn valid_environment_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    value.len() <= 128
        && (first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_outbound_host(value: &str) -> bool {
    let labels_are_valid = value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    });
    !value.is_empty()
        && value.len() <= 253
        && labels_are_valid
        && value.parse::<IpAddr>().is_err()
        && !matches!(
            value,
            "localhost" | "metadata" | "metadata.google.internal" | "instance-data.ec2.internal"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_api::{EnvironmentId, ProjectId};

    fn address() -> DeploymentAddress {
        DeploymentAddress {
            tenant: TenantScope::new(
                ProjectId::parse("prj_example00").expect("project"),
                EnvironmentId::parse("env_example00").expect("environment"),
            ),
            function_name: "hello-world".to_owned(),
            version: 7,
        }
    }

    #[test]
    fn embedded_pin_is_immutable_and_valid() {
        let pin = RuntimePin::embedded().expect("valid pin");
        assert_eq!(pin.release, "v1.74.3");
        assert_eq!(
            pin.source_commit,
            "47d04fdd22e33ea3fd904576cf3248d963d903a9"
        );
        assert_eq!(
            pin.image_reference(),
            "docker.io/supabase/edge-runtime@sha256:c52405002a890ca9fcf77978671c57f3a988e03174afb277f84ac65bc917013c"
        );
    }

    #[test]
    fn deployment_wire_contract_is_strict_and_contains_references_only() {
        let pin = RuntimePin::embedded().expect("valid pin");
        let request = LoadDeploymentRequest {
            protocol_version: PROTOCOL_VERSION,
            deployment: address(),
            bundle_digest:
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            bundle_format: BundleFormat::SourceArchiveV1,
            entrypoint: "src/index.ts".to_owned(),
            runtime_release: pin.release.clone(),
            limits: RuntimeLimits {
                cpu_milliseconds: 100,
                wall_milliseconds: 1_000,
                memory_bytes: 64 * 1024 * 1024,
                request_bytes: 1024,
                response_bytes: 2048,
                concurrency: 2,
                outbound_network: OutboundNetworkPolicy::DenyAll,
            },
            verify_jwt: true,
            secret_versions: vec![SecretVersionReference {
                name: "API_TOKEN".to_owned(),
                version: 3,
            }],
        };
        request.validate(&pin).expect("valid deployment");
        let encoded = serde_json::to_string(&request).expect("serialize");
        assert!(!encoded.contains("secretValue"));
        assert!(serde_json::from_str::<LoadDeploymentRequest>(&encoded).is_ok());
        let with_unknown = encoded.replacen('{', "{\"unknown\":true,", 1);
        assert!(serde_json::from_str::<LoadDeploymentRequest>(&with_unknown).is_err());
    }

    #[test]
    fn sensitive_load_values_serialize_exactly_but_never_format() {
        let secret = SensitiveDeploymentSecret::new(
            SecretVersionReference {
                name: "API_TOKEN".to_owned(),
                version: 3,
            },
            "exact-sensitive-value",
        )
        .expect("secret");
        assert_eq!(secret.expose_to_supervisor(), "exact-sensitive-value");
        assert!(!format!("{secret:?}").contains("exact-sensitive-value"));
        let encoded = serde_json::to_string(&secret).expect("serialize");
        assert_eq!(encoded.matches("exact-sensitive-value").count(), 1);
        assert!(
            serde_json::from_str::<SensitiveDeploymentSecret>(
                r#"{"reference":{"name":"API_TOKEN","version":3},"value":"x","unknown":true}"#
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_protocol_runtime_and_tenant_ambiguity() {
        assert_eq!(
            require_protocol(PROTOCOL_VERSION + 1),
            Err(ProtocolValidationError::ProtocolMismatch)
        );
        assert!(serde_json::from_str::<DeploymentAddress>(
            r#"{"tenant":{"projectId":"prj_short","environmentId":"env_example00"},"functionName":"hello","version":1}"#
        )
        .is_err());

        let pin = RuntimePin::embedded().expect("valid pin");
        let mut request = LoadDeploymentRequest {
            protocol_version: PROTOCOL_VERSION,
            deployment: address(),
            bundle_digest:
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            bundle_format: BundleFormat::Prebuilt,
            entrypoint: "index.ts".to_owned(),
            runtime_release: "v1.74.2".to_owned(),
            limits: RuntimeLimits {
                cpu_milliseconds: 1,
                wall_milliseconds: 1,
                memory_bytes: 1,
                request_bytes: 1,
                response_bytes: 1,
                concurrency: 1,
                outbound_network: OutboundNetworkPolicy::DenyAll,
            },
            verify_jwt: true,
            secret_versions: vec![],
        };
        assert_eq!(
            request.validate(&pin),
            Err(ProtocolValidationError::InvalidDeployment)
        );
        request.runtime_release = pin.release.clone();
        request.deployment.version = 0;
        assert_eq!(
            request.validate(&pin),
            Err(ProtocolValidationError::InvalidDeploymentAddress)
        );
    }

    #[test]
    fn adversarial_egress_policy_has_no_unrestricted_or_metadata_destination() {
        assert!(serde_json::from_str::<OutboundNetworkPolicy>(r#"{"mode":"allow_all"}"#).is_err());
        for host in [
            "169.254.169.254",
            "127.0.0.1",
            "localhost",
            "metadata",
            "metadata.google.internal",
            "instance-data.ec2.internal",
            "-invalid.example",
            "invalid-.example",
        ] {
            assert!(
                !OutboundNetworkPolicy::AllowList {
                    hosts: vec![host.to_owned()],
                    max_requests_per_invocation: 1,
                }
                .is_valid(),
                "adversarial destination must be rejected: {host}"
            );
        }
        assert!(
            OutboundNetworkPolicy::AllowList {
                hosts: vec!["api.example.com".to_owned()],
                max_requests_per_invocation: 1,
            }
            .is_valid()
        );
    }
}
