use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use mako_api::TenantScope;
use mako_edge_runtime_protocol::{
    BundleFormat, DEPLOYMENT_LOGS_PATH, DeploymentAddress, DeploymentLogPage,
    DeploymentLogQueryRequest, DeploymentOperationRequest, DeploymentState, DeploymentStatus,
    FunctionAddress, FunctionOperationRequest, HEALTH_PATH, LOAD_DEPLOYMENT_PATH,
    LoadDeploymentRequest, OutboundNetworkPolicy, PROBE_DEPLOYMENT_PATH, PROTOCOL_VERSION,
    PROTOCOL_VERSION_HEADER, REQUEST_ID_HEADER, RETIRE_DEPLOYMENT_PATH, RETIRE_FUNCTION_PATH,
    RUNTIME_AUTHORIZATION_HEADER, RetireFunctionResult, RuntimeErrorEnvelope, RuntimeHealth,
    RuntimeLimits, RuntimePin, SecretVersionReference, SensitiveDeploymentSecret,
    SensitiveLoadDeploymentRequest, TEST_DEPLOYMENT_PATH, TestDeploymentRequest,
    TestDeploymentResponse, require_protocol,
};
use rand_core::{OsRng, RngCore};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{
    FunctionBackendError, FunctionBundleFormat, FunctionDeploymentBackend,
    FunctionDeploymentResult, FunctionDeploymentSpec, FunctionLogEntry, FunctionLogPage,
    FunctionLogQuery, FunctionName, FunctionTestRequest, FunctionTestResponse,
    function_bundle::MAX_FUNCTION_BUNDLE_BYTES,
};

const MAX_RESPONSE_HEADER_BYTES: usize = 32 * 1024;
const MAX_RESPONSE_HEADERS: usize = 128;
const MAX_RUNTIME_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RUNTIME_REQUEST_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_SECRET_BYTES: usize = 4 * 1024 * 1024;
const MAX_TEST_BODY_BYTES: usize = 10 * 1024 * 1024;
const MAX_LOG_MESSAGE_BYTES: usize = 16 * 1024;

#[derive(Clone, Eq, PartialEq)]
pub struct RuntimeSupervisorCredential(Box<str>);

impl RuntimeSupervisorCredential {
    pub fn new(value: impl Into<String>) -> Result<Self, RuntimeClientError> {
        let value = value.into();
        if !(32..=1024).contains(&value.len()) || value.chars().any(char::is_control) {
            return Err(RuntimeClientError::InvalidConfiguration);
        }
        Ok(Self(value.into_boxed_str()))
    }
}

impl fmt::Debug for RuntimeSupervisorCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RuntimeSupervisorCredential([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDeploymentClientConfig {
    pub endpoint: SocketAddr,
    pub region: String,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub maximum_response_bytes: usize,
}

impl RuntimeDeploymentClientConfig {
    #[must_use]
    pub fn loopback(endpoint: SocketAddr, region: impl Into<String>) -> Self {
        Self {
            endpoint,
            region: region.into(),
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(10),
            maximum_response_bytes: MAX_RUNTIME_RESPONSE_BYTES,
        }
    }

    fn validate(&self) -> Result<(), RuntimeClientError> {
        if !self.endpoint.ip().is_loopback()
            || !valid_region(&self.region)
            || self.connect_timeout.is_zero()
            || self.io_timeout.is_zero()
            || self.maximum_response_bytes == 0
            || self.maximum_response_bytes > MAX_RUNTIME_RESPONSE_BYTES
        {
            return Err(RuntimeClientError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct RuntimeDeploymentClient {
    config: RuntimeDeploymentClientConfig,
    credential: RuntimeSupervisorCredential,
    pin: RuntimePin,
}

impl RuntimeDeploymentClient {
    pub fn new(
        config: RuntimeDeploymentClientConfig,
        credential: RuntimeSupervisorCredential,
    ) -> Result<Self, RuntimeClientError> {
        config.validate()?;
        let pin = RuntimePin::embedded().map_err(|_| RuntimeClientError::InvalidConfiguration)?;
        Ok(Self {
            config,
            credential,
            pin,
        })
    }

    #[must_use]
    pub fn dependency_ready(&self) -> bool {
        self.health_request().is_ok_and(|health| {
            health.ready
                && health.protocol_version == PROTOCOL_VERSION
                && health.runtime_release == self.pin.release
                && health.source_commit == self.pin.source_commit
                && health.region == self.config.region
        })
    }

    fn health_request(&self) -> Result<RuntimeHealth, RuntimeClientError> {
        let request_id = request_id();
        self.send("GET", HEALTH_PATH, &request_id, &[])
    }

    fn deployment_status<T: Serialize>(
        &self,
        path: &str,
        request_id: &str,
        payload: &T,
        expected: &DeploymentAddress,
    ) -> Result<DeploymentStatus, RuntimeClientError> {
        let status: DeploymentStatus = self.send_json(path, request_id, payload)?;
        require_protocol(status.protocol_version)
            .map_err(|_| RuntimeClientError::InvalidResponse)?;
        if status.deployment != *expected
            || status.region != self.config.region
            || status
                .diagnostic_code
                .as_deref()
                .is_some_and(|value| !valid_diagnostic(value))
        {
            return Err(RuntimeClientError::InvalidResponse);
        }
        Ok(status)
    }

    fn send_json<T: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        request_id: &str,
        payload: &T,
    ) -> Result<R, RuntimeClientError> {
        let body = serde_json::to_vec(payload).map_err(|_| RuntimeClientError::InvalidRequest)?;
        if body.len() > MAX_RUNTIME_REQUEST_BYTES {
            return Err(RuntimeClientError::InvalidRequest);
        }
        self.send("POST", path, request_id, &body)
    }

    fn send<R: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        request_id: &str,
        body: &[u8],
    ) -> Result<R, RuntimeClientError> {
        if !valid_request_id(request_id) {
            return Err(RuntimeClientError::InvalidRequest);
        }
        let response = send_http(
            &self.config,
            &self.credential,
            method,
            path,
            request_id,
            body,
        )?;
        if response.request_id != request_id
            || response.protocol_version != PROTOCOL_VERSION.to_string()
        {
            return Err(RuntimeClientError::CorrelationFailed);
        }
        if !(200..300).contains(&response.status) {
            let envelope: RuntimeErrorEnvelope = serde_json::from_slice(&response.body)
                .map_err(|_| RuntimeClientError::InvalidResponse)?;
            if envelope.protocol_version != PROTOCOL_VERSION || envelope.request_id != request_id {
                return Err(RuntimeClientError::CorrelationFailed);
            }
            return Err(RuntimeClientError::RemoteRejected);
        }
        serde_json::from_slice(&response.body).map_err(|_| RuntimeClientError::InvalidResponse)
    }

    fn manifest(
        &self,
        spec: &FunctionDeploymentSpec,
    ) -> Result<LoadDeploymentRequest, RuntimeClientError> {
        if spec.bundle.is_empty()
            || spec.bundle.len() > MAX_FUNCTION_BUNDLE_BYTES
            || digest(&spec.bundle) != spec.bundle_digest
            || !spec
                .configuration
                .regions
                .iter()
                .any(|region| region == &self.config.region)
        {
            return Err(RuntimeClientError::InvalidRequest);
        }
        let secret_versions = spec
            .secrets
            .iter()
            .map(|secret| SecretVersionReference {
                name: secret.reference.name.as_str().to_owned(),
                version: secret.reference.version,
            })
            .collect::<Vec<_>>();
        let configured_names = spec
            .configuration
            .secret_names
            .iter()
            .map(|name| name.as_str())
            .collect::<BTreeSet<_>>();
        let supplied_names = secret_versions
            .iter()
            .map(|secret| secret.name.as_str())
            .collect::<BTreeSet<_>>();
        if configured_names != supplied_names || secret_versions.len() != supplied_names.len() {
            return Err(RuntimeClientError::InvalidRequest);
        }
        let manifest = LoadDeploymentRequest {
            protocol_version: PROTOCOL_VERSION,
            deployment: address(spec),
            bundle_digest: spec.bundle_digest.clone(),
            bundle_format: match spec.bundle_format {
                FunctionBundleFormat::SourceArchiveV1 => BundleFormat::SourceArchiveV1,
                FunctionBundleFormat::Prebuilt => BundleFormat::Prebuilt,
            },
            entrypoint: spec.entrypoint.clone(),
            runtime_release: spec.runtime_version.clone(),
            limits: RuntimeLimits {
                cpu_milliseconds: spec.configuration.limits.cpu_milliseconds,
                wall_milliseconds: spec.configuration.limits.wall_milliseconds,
                memory_bytes: spec.configuration.limits.memory_bytes,
                request_bytes: spec.configuration.limits.request_bytes,
                response_bytes: spec.configuration.limits.response_bytes,
                concurrency: spec.configuration.limits.concurrency,
                outbound_network: OutboundNetworkPolicy::DenyAll,
            },
            verify_jwt: spec.configuration.verify_jwt,
            secret_versions,
        };
        manifest
            .validate(&self.pin)
            .map_err(|_| RuntimeClientError::InvalidRequest)?;
        Ok(manifest)
    }

    fn sensitive_load(
        &self,
        spec: &FunctionDeploymentSpec,
        request_id: String,
    ) -> Result<SensitiveLoadDeploymentRequest, RuntimeClientError> {
        let manifest = self.manifest(spec)?;
        let mut total_secret_bytes = 0_usize;
        let secrets = spec
            .secrets
            .iter()
            .map(|secret| {
                let value = secret.expose_to_runtime_adapter();
                total_secret_bytes = total_secret_bytes.saturating_add(value.len());
                SensitiveDeploymentSecret::new(
                    SecretVersionReference {
                        name: secret.reference.name.as_str().to_owned(),
                        version: secret.reference.version,
                    },
                    value,
                )
                .map_err(|_| RuntimeClientError::InvalidRequest)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if total_secret_bytes > MAX_TOTAL_SECRET_BYTES {
            return Err(RuntimeClientError::InvalidRequest);
        }
        Ok(SensitiveLoadDeploymentRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            manifest,
            bundle_base64: STANDARD.encode(&spec.bundle),
            secrets,
        })
    }
}

impl fmt::Debug for RuntimeDeploymentClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeDeploymentClient")
            .field("endpoint", &self.config.endpoint)
            .field("region", &self.config.region)
            .field("runtime_release", &self.pin.release)
            .field("credential", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl FunctionDeploymentBackend for RuntimeDeploymentClient {
    async fn deploy(
        &self,
        spec: &FunctionDeploymentSpec,
    ) -> Result<FunctionDeploymentResult, FunctionBackendError> {
        let request_id = request_id();
        let payload = self
            .sensitive_load(spec, request_id.clone())
            .map_err(backend_error)?;
        let status = self
            .deployment_status(
                LOAD_DEPLOYMENT_PATH,
                &request_id,
                &payload,
                &payload.manifest.deployment,
            )
            .map_err(backend_error)?;
        deployment_result(status).map_err(backend_error)
    }

    async fn health(
        &self,
        spec: &FunctionDeploymentSpec,
    ) -> Result<FunctionDeploymentResult, FunctionBackendError> {
        let deployment = address(spec);
        let request_id = request_id();
        let payload = DeploymentOperationRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            deployment: deployment.clone(),
        };
        let status = self
            .deployment_status(PROBE_DEPLOYMENT_PATH, &request_id, &payload, &deployment)
            .map_err(backend_error)?;
        deployment_result(status).map_err(backend_error)
    }

    async fn test(
        &self,
        spec: &FunctionDeploymentSpec,
        request: &FunctionTestRequest,
    ) -> Result<FunctionTestResponse, FunctionBackendError> {
        validate_test_request(spec, request).map_err(backend_error)?;
        let request_id = request_id();
        let deployment = address(spec);
        let payload = TestDeploymentRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            deployment: deployment.clone(),
            method: request.method.clone(),
            path: request.path.clone(),
            headers: request.headers.clone(),
            body_base64: STANDARD.encode(&request.body),
        };
        let response: TestDeploymentResponse = self
            .send_json(TEST_DEPLOYMENT_PATH, &request_id, &payload)
            .map_err(backend_error)?;
        require_protocol(response.protocol_version)
            .map_err(|_| backend_error(RuntimeClientError::InvalidResponse))?;
        let body = STANDARD
            .decode(response.body_base64)
            .map_err(|_| backend_error(RuntimeClientError::InvalidResponse))?;
        if response.request_id != request_id
            || response.deployment != deployment
            || !(200..=599).contains(&response.status)
            || body.len() > MAX_TEST_BODY_BYTES
            || u64::try_from(body.len()).unwrap_or(u64::MAX)
                > spec.configuration.limits.response_bytes
            || !valid_headers(&response.headers, true)
        {
            return Err(backend_error(RuntimeClientError::InvalidResponse));
        }
        Ok(FunctionTestResponse {
            status: response.status,
            headers: response.headers,
            body,
            correlation_id: request_id,
        })
    }

    async fn logs(
        &self,
        tenant: &TenantScope,
        function_name: &FunctionName,
        query: &FunctionLogQuery,
    ) -> Result<FunctionLogPage, FunctionBackendError> {
        if query.limit == 0
            || query.limit > 1000
            || query
                .cursor
                .as_deref()
                .is_some_and(|cursor| !valid_cursor(cursor))
        {
            return Err(backend_error(RuntimeClientError::InvalidRequest));
        }
        let request_id = request_id();
        let function = FunctionAddress {
            tenant: tenant.clone(),
            function_name: function_name.as_str().to_owned(),
        };
        let payload = DeploymentLogQueryRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            function: function.clone(),
            cursor: query.cursor.clone(),
            limit: query.limit,
        };
        let page: DeploymentLogPage = self
            .send_json(DEPLOYMENT_LOGS_PATH, &request_id, &payload)
            .map_err(backend_error)?;
        if page.protocol_version != PROTOCOL_VERSION
            || page.request_id != request_id
            || page.function != function
            || page.items.len() > query.limit
            || page
                .next_cursor
                .as_deref()
                .is_some_and(|cursor| !valid_cursor(cursor))
        {
            return Err(backend_error(RuntimeClientError::InvalidResponse));
        }
        let items = page
            .items
            .into_iter()
            .map(|item| {
                if item.deployment.tenant != *tenant
                    || item.deployment.function_name != function_name.as_str()
                    || item.deployment.version == 0
                    || item.region != self.config.region
                    || !valid_log_level(&item.level)
                    || !valid_log_message(&item.message)
                    || !valid_correlation_id(&item.correlation_id)
                {
                    return Err(RuntimeClientError::InvalidResponse);
                }
                Ok(FunctionLogEntry {
                    timestamp_unix_milliseconds: item.timestamp_unix_milliseconds,
                    level: item.level,
                    message: item.message,
                    correlation_id: item.correlation_id,
                    version: item.deployment.version,
                    region: item.region,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend_error)?;
        Ok(FunctionLogPage {
            items,
            next_cursor: page.next_cursor,
        })
    }

    async fn delete_version(
        &self,
        tenant: &TenantScope,
        function_name: &FunctionName,
        version: u64,
    ) -> Result<(), FunctionBackendError> {
        let deployment = DeploymentAddress {
            tenant: tenant.clone(),
            function_name: function_name.as_str().to_owned(),
            version,
        };
        deployment
            .validate()
            .map_err(|_| backend_error(RuntimeClientError::InvalidRequest))?;
        let request_id = request_id();
        let payload = DeploymentOperationRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            deployment: deployment.clone(),
        };
        let status = self
            .deployment_status(RETIRE_DEPLOYMENT_PATH, &request_id, &payload, &deployment)
            .map_err(backend_error)?;
        if status.state != DeploymentState::Retired {
            return Err(backend_error(RuntimeClientError::InvalidResponse));
        }
        Ok(())
    }

    async fn delete_function(
        &self,
        tenant: &TenantScope,
        function_name: &FunctionName,
    ) -> Result<(), FunctionBackendError> {
        let function = FunctionAddress {
            tenant: tenant.clone(),
            function_name: function_name.as_str().to_owned(),
        };
        let request_id = request_id();
        let payload = FunctionOperationRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            function: function.clone(),
        };
        let result: RetireFunctionResult = self
            .send_json(RETIRE_FUNCTION_PATH, &request_id, &payload)
            .map_err(backend_error)?;
        if result.protocol_version != PROTOCOL_VERSION
            || result.request_id != request_id
            || result.function != function
            || result.retired_versions > 100_000
        {
            return Err(backend_error(RuntimeClientError::InvalidResponse));
        }
        Ok(())
    }
}

fn address(spec: &FunctionDeploymentSpec) -> DeploymentAddress {
    DeploymentAddress {
        tenant: spec.tenant.clone(),
        function_name: spec.function_name.as_str().to_owned(),
        version: spec.version,
    }
}

fn deployment_result(
    status: DeploymentStatus,
) -> Result<FunctionDeploymentResult, RuntimeClientError> {
    match status.state {
        DeploymentState::Healthy if status.diagnostic_code.is_none() => {
            Ok(FunctionDeploymentResult {
                healthy: true,
                diagnostic: None,
            })
        }
        DeploymentState::Loading | DeploymentState::Unhealthy => Ok(FunctionDeploymentResult {
            healthy: false,
            diagnostic: status
                .diagnostic_code
                .or_else(|| Some("runtime_not_healthy".to_owned())),
        }),
        DeploymentState::Healthy | DeploymentState::Retired => {
            Err(RuntimeClientError::InvalidResponse)
        }
    }
}

fn validate_test_request(
    spec: &FunctionDeploymentSpec,
    request: &FunctionTestRequest,
) -> Result<(), RuntimeClientError> {
    if request.body.len() > MAX_TEST_BODY_BYTES
        || u64::try_from(request.body.len()).unwrap_or(u64::MAX)
            > spec.configuration.limits.request_bytes
        || !valid_method(&request.method)
        || !valid_path(&request.path)
        || !valid_headers(&request.headers, false)
    {
        return Err(RuntimeClientError::InvalidRequest);
    }
    Ok(())
}

fn valid_method(value: &str) -> bool {
    matches!(
        value,
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
    )
}

fn valid_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 8192
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
}

fn valid_headers(headers: &[(String, String)], response: bool) -> bool {
    headers.len() <= 128
        && headers.iter().all(|(name, value)| {
            let lower = name.to_ascii_lowercase();
            valid_header_name(name)
                && value.len() <= 16 * 1024
                && !value.chars().any(char::is_control)
                && !matches!(
                    lower.as_str(),
                    "connection"
                        | "content-length"
                        | "host"
                        | "transfer-encoding"
                        | RUNTIME_AUTHORIZATION_HEADER
                        | PROTOCOL_VERSION_HEADER
                        | REQUEST_ID_HEADER
                )
                && (response || !lower.starts_with("x-mako-runtime-"))
        })
}

fn valid_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_region(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn valid_diagnostic(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
}

fn valid_log_level(value: &str) -> bool {
    matches!(value, "debug" | "info" | "warn" | "error")
}

fn valid_log_message(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LOG_MESSAGE_BYTES
        && !value.contains('\0')
        && !value.contains('\r')
}

fn valid_correlation_id(value: &str) -> bool {
    (8..=132).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_request_id(value: &str) -> bool {
    value.strip_prefix("req_").is_some_and(valid_correlation_id)
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn request_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut value = String::from("req_");
    for byte in bytes {
        use fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("formatting into a string cannot fail");
    }
    value
}

fn backend_error(_error: RuntimeClientError) -> FunctionBackendError {
    FunctionBackendError::new("runtime supervisor is unavailable")
        .expect("static safe backend error is valid")
}

#[derive(Debug, Eq, PartialEq)]
pub enum RuntimeClientError {
    InvalidConfiguration,
    InvalidRequest,
    Unavailable,
    ResponseTooLarge,
    InvalidResponse,
    CorrelationFailed,
    RemoteRejected,
}

impl fmt::Display for RuntimeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "runtime client configuration is invalid",
            Self::InvalidRequest => "runtime client request is invalid",
            Self::Unavailable => "runtime supervisor is unavailable",
            Self::ResponseTooLarge => "runtime supervisor response exceeded its bound",
            Self::InvalidResponse => "runtime supervisor response is invalid",
            Self::CorrelationFailed => "runtime supervisor response correlation failed",
            Self::RemoteRejected => "runtime supervisor rejected the request",
        })
    }
}

impl std::error::Error for RuntimeClientError {}

#[derive(Debug)]
struct RuntimeHttpResponse {
    status: u16,
    request_id: String,
    protocol_version: String,
    body: Vec<u8>,
}

fn send_http(
    config: &RuntimeDeploymentClientConfig,
    credential: &RuntimeSupervisorCredential,
    method: &str,
    path: &str,
    request_id: &str,
    body: &[u8],
) -> Result<RuntimeHttpResponse, RuntimeClientError> {
    let mut stream = TcpStream::connect_timeout(&config.endpoint, config.connect_timeout)
        .map_err(|_| RuntimeClientError::Unavailable)?;
    stream
        .set_read_timeout(Some(config.io_timeout))
        .and_then(|()| stream.set_write_timeout(Some(config.io_timeout)))
        .map_err(|_| RuntimeClientError::Unavailable)?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}: {}\r\n{}: {}\r\n{}: {}\r\n\r\n",
        config.endpoint,
        body.len(),
        PROTOCOL_VERSION_HEADER,
        PROTOCOL_VERSION,
        REQUEST_ID_HEADER,
        request_id,
        RUNTIME_AUTHORIZATION_HEADER,
        credential.0,
    )
    .and_then(|()| stream.write_all(body))
    .and_then(|()| stream.flush())
    .map_err(|_| RuntimeClientError::Unavailable)?;
    read_response(&mut stream, config.maximum_response_bytes)
}

fn read_response(
    stream: &mut TcpStream,
    maximum_body_bytes: usize,
) -> Result<RuntimeHttpResponse, RuntimeClientError> {
    let maximum = maximum_body_bytes
        .saturating_add(MAX_RESPONSE_HEADER_BYTES)
        .saturating_add(1);
    let mut wire = Vec::new();
    stream
        .take(u64::try_from(maximum).unwrap_or(u64::MAX))
        .read_to_end(&mut wire)
        .map_err(|_| RuntimeClientError::Unavailable)?;
    if wire.len() >= maximum {
        return Err(RuntimeClientError::ResponseTooLarge);
    }
    parse_response(&wire, maximum_body_bytes)
}

fn parse_response(
    wire: &[u8],
    maximum_body_bytes: usize,
) -> Result<RuntimeHttpResponse, RuntimeClientError> {
    let header_end = wire
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(RuntimeClientError::InvalidResponse)?;
    if header_end > MAX_RESPONSE_HEADER_BYTES {
        return Err(RuntimeClientError::InvalidResponse);
    }
    let head = std::str::from_utf8(&wire[..header_end])
        .map_err(|_| RuntimeClientError::InvalidResponse)?;
    let mut lines = head.split("\r\n");
    let mut status_line = lines
        .next()
        .ok_or(RuntimeClientError::InvalidResponse)?
        .split_ascii_whitespace();
    if status_line.next() != Some("HTTP/1.1") {
        return Err(RuntimeClientError::InvalidResponse);
    }
    let status = status_line
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| (100..=599).contains(value))
        .ok_or(RuntimeClientError::InvalidResponse)?;
    let mut headers = BTreeMap::new();
    for (count, line) in lines.enumerate() {
        if count >= MAX_RESPONSE_HEADERS {
            return Err(RuntimeClientError::InvalidResponse);
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(RuntimeClientError::InvalidResponse)?;
        let name = name.trim().to_ascii_lowercase();
        if !valid_header_name(&name) || headers.insert(name, value.trim().to_owned()).is_some() {
            return Err(RuntimeClientError::InvalidResponse);
        }
    }
    if headers.contains_key("transfer-encoding")
        || headers.get("content-type").map(String::as_str) != Some("application/json")
    {
        return Err(RuntimeClientError::InvalidResponse);
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or(RuntimeClientError::InvalidResponse)?;
    let body = wire
        .get(header_end + 4..)
        .ok_or(RuntimeClientError::InvalidResponse)?;
    if body.len() != content_length || body.len() > maximum_body_bytes {
        return Err(RuntimeClientError::ResponseTooLarge);
    }
    Ok(RuntimeHttpResponse {
        status,
        request_id: headers
            .remove(REQUEST_ID_HEADER)
            .ok_or(RuntimeClientError::CorrelationFailed)?,
        protocol_version: headers
            .remove(PROTOCOL_VERSION_HEADER)
            .ok_or(RuntimeClientError::InvalidResponse)?,
        body: body.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        net::TcpListener,
        sync::{
            Arc,
            mpsc::{self, Receiver},
        },
        thread,
    };

    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_edge_runtime_protocol::{DeploymentLogEntry, SensitiveLoadDeploymentRequest};

    use super::*;
    use crate::{
        FunctionConfiguration, FunctionLimits, FunctionSecretName, FunctionSecretValue,
        FunctionSecretVersionReference,
    };

    const AUTHORIZATION: &str = "runtime-authorization-that-is-long-enough";
    const SECRET_VALUE: &str = "exact-function-secret";

    #[test]
    fn configuration_is_loopback_only_and_credentials_are_redacted() {
        let credential = RuntimeSupervisorCredential::new(AUTHORIZATION).expect("credential");
        assert_eq!(
            format!("{credential:?}"),
            "RuntimeSupervisorCredential([REDACTED])"
        );
        let mut config = RuntimeDeploymentClientConfig::loopback(
            "192.0.2.1:9001".parse().expect("endpoint"),
            "us-east-1-beta",
        );
        assert_eq!(
            RuntimeDeploymentClient::new(config.clone(), credential.clone()).unwrap_err(),
            RuntimeClientError::InvalidConfiguration,
        );
        config.endpoint = "127.0.0.1:9001".parse().expect("endpoint");
        config.region = "INVALID".to_owned();
        assert_eq!(
            RuntimeDeploymentClient::new(config, credential).unwrap_err(),
            RuntimeClientError::InvalidConfiguration,
        );
    }

    #[test]
    fn deploy_delivers_exact_secrets_on_the_authenticated_correlated_hop() {
        let expected = deployment_address();
        let response_address = expected.clone();
        let (endpoint, requests) = mock_runtime(vec![Box::new(move |request| {
            let request_id = request_header(request, REQUEST_ID_HEADER);
            response(
                200,
                request_id,
                &DeploymentStatus {
                    protocol_version: PROTOCOL_VERSION,
                    deployment: response_address.clone(),
                    state: DeploymentState::Healthy,
                    region: "us-east-1-beta".to_owned(),
                    diagnostic_code: None,
                },
            )
        })]);
        let client = client(endpoint);
        let spec = deployment_spec();
        assert_eq!(
            block_on(client.deploy(&spec)),
            Ok(FunctionDeploymentResult {
                healthy: true,
                diagnostic: None,
            })
        );

        let request = requests.recv().expect("request");
        assert!(request.starts_with(&format!("POST {LOAD_DEPLOYMENT_PATH} HTTP/1.1\r\n")));
        assert_eq!(
            request_header(&request, RUNTIME_AUTHORIZATION_HEADER),
            AUTHORIZATION
        );
        assert_eq!(request.matches(SECRET_VALUE).count(), 1);
        let payload: SensitiveLoadDeploymentRequest = request_json(&request);
        assert_eq!(payload.manifest.deployment, expected);
        assert_eq!(payload.secrets.len(), 1);
        assert_eq!(payload.secrets[0].expose_to_supervisor(), SECRET_VALUE);
        assert_eq!(
            STANDARD.decode(&payload.bundle_base64).expect("bundle"),
            b"bundle"
        );
        assert!(!format!("{payload:?}").contains(SECRET_VALUE));
        assert!(!format!("{client:?}").contains(AUTHORIZATION));
    }

    #[test]
    fn lifecycle_test_logs_and_retirement_are_strictly_tenant_bound() {
        let expected = deployment_address();
        let handlers: Vec<Handler> = vec![
            {
                let expected = expected.clone();
                Box::new(move |request| {
                    let request_id = request_header(request, REQUEST_ID_HEADER);
                    response(
                        200,
                        request_id,
                        &DeploymentStatus {
                            protocol_version: PROTOCOL_VERSION,
                            deployment: expected.clone(),
                            state: DeploymentState::Healthy,
                            region: "us-east-1-beta".to_owned(),
                            diagnostic_code: None,
                        },
                    )
                })
            },
            {
                let expected = expected.clone();
                Box::new(move |request| {
                    let request_id = request_header(request, REQUEST_ID_HEADER);
                    response(
                        200,
                        request_id,
                        &TestDeploymentResponse {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: request_id.to_owned(),
                            deployment: expected.clone(),
                            status: 202,
                            headers: vec![("content-type".to_owned(), "text/plain".to_owned())],
                            body_base64: STANDARD.encode(b"tested"),
                        },
                    )
                })
            },
            {
                let expected = expected.clone();
                Box::new(move |request| {
                    let request_id = request_header(request, REQUEST_ID_HEADER);
                    response(
                        200,
                        request_id,
                        &DeploymentLogPage {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: request_id.to_owned(),
                            function: FunctionAddress {
                                tenant: expected.tenant.clone(),
                                function_name: expected.function_name.clone(),
                            },
                            items: vec![DeploymentLogEntry {
                                deployment: expected.clone(),
                                timestamp_unix_milliseconds: 42,
                                level: "info".to_owned(),
                                message: "safe log".to_owned(),
                                correlation_id: "req_invocation0001".to_owned(),
                                region: "us-east-1-beta".to_owned(),
                            }],
                            next_cursor: Some("cursor_2".to_owned()),
                        },
                    )
                })
            },
            {
                let expected = expected.clone();
                Box::new(move |request| {
                    let request_id = request_header(request, REQUEST_ID_HEADER);
                    response(
                        200,
                        request_id,
                        &DeploymentStatus {
                            protocol_version: PROTOCOL_VERSION,
                            deployment: expected.clone(),
                            state: DeploymentState::Retired,
                            region: "us-east-1-beta".to_owned(),
                            diagnostic_code: None,
                        },
                    )
                })
            },
            {
                let expected = expected.clone();
                Box::new(move |request| {
                    let request_id = request_header(request, REQUEST_ID_HEADER);
                    response(
                        200,
                        request_id,
                        &RetireFunctionResult {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: request_id.to_owned(),
                            function: FunctionAddress {
                                tenant: expected.tenant.clone(),
                                function_name: expected.function_name.clone(),
                            },
                            retired_versions: 1,
                        },
                    )
                })
            },
        ];
        let (endpoint, requests) = mock_runtime(handlers);
        let client = client(endpoint);
        let spec = deployment_spec();
        block_on(async {
            assert!(client.health(&spec).await.expect("health").healthy);
            let tested = client
                .test(
                    &spec,
                    &FunctionTestRequest {
                        method: "POST".to_owned(),
                        path: "/test".to_owned(),
                        headers: vec![("accept".to_owned(), "text/plain".to_owned())],
                        body: b"input".to_vec(),
                    },
                )
                .await
                .expect("test");
            assert_eq!(tested.status, 202);
            assert_eq!(tested.body, b"tested");
            let page = client
                .logs(
                    &spec.tenant,
                    &spec.function_name,
                    &FunctionLogQuery {
                        cursor: None,
                        limit: 10,
                    },
                )
                .await
                .expect("logs");
            assert_eq!(page.items[0].version, 7);
            assert_eq!(page.next_cursor.as_deref(), Some("cursor_2"));
            client
                .delete_version(&spec.tenant, &spec.function_name, 7)
                .await
                .expect("retire deployment");
            client
                .delete_function(&spec.tenant, &spec.function_name)
                .await
                .expect("retire function");
        });
        let paths = (0..5)
            .map(|_| request_path(&requests.recv().expect("request")).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                PROBE_DEPLOYMENT_PATH,
                TEST_DEPLOYMENT_PATH,
                DEPLOYMENT_LOGS_PATH,
                RETIRE_DEPLOYMENT_PATH,
                RETIRE_FUNCTION_PATH,
            ]
        );
    }

    #[test]
    fn readiness_fails_closed_on_a_pin_or_correlation_mismatch() {
        let (endpoint, _requests) = mock_runtime(vec![Box::new(|request| {
            let request_id = request_header(request, REQUEST_ID_HEADER);
            response(
                200,
                request_id,
                &RuntimeHealth {
                    protocol_version: PROTOCOL_VERSION,
                    runtime_release: "v1.74.2".to_owned(),
                    source_commit: "47d04fdd22e33ea3fd904576cf3248d963d903a9".to_owned(),
                    region: "us-east-1-beta".to_owned(),
                    ready: true,
                },
            )
        })]);
        assert!(!client(endpoint).dependency_ready());

        let (endpoint, _requests) = mock_runtime(vec![Box::new(|_request| {
            response(
                200,
                "req_wrongcorrelation0000000000000000",
                &RuntimeHealth {
                    protocol_version: PROTOCOL_VERSION,
                    runtime_release: "v1.74.3".to_owned(),
                    source_commit: "47d04fdd22e33ea3fd904576cf3248d963d903a9".to_owned(),
                    region: "us-east-1-beta".to_owned(),
                    ready: true,
                },
            )
        })]);
        assert!(!client(endpoint).dependency_ready());
    }

    fn client(endpoint: SocketAddr) -> RuntimeDeploymentClient {
        RuntimeDeploymentClient::new(
            RuntimeDeploymentClientConfig::loopback(endpoint, "us-east-1-beta"),
            RuntimeSupervisorCredential::new(AUTHORIZATION).expect("credential"),
        )
        .expect("client")
    }

    fn deployment_spec() -> FunctionDeploymentSpec {
        let tenant = tenant();
        FunctionDeploymentSpec {
            tenant,
            function_name: FunctionName::parse("hello-world").expect("function"),
            version: 7,
            bundle_digest: digest(b"bundle"),
            entrypoint: "index.ts".to_owned(),
            runtime_version: "v1.74.3".to_owned(),
            configuration: FunctionConfiguration {
                verify_jwt: true,
                regions: vec!["us-east-1-beta".to_owned()],
                secret_names: vec![FunctionSecretName::parse("API_TOKEN").expect("name")],
                limits: FunctionLimits {
                    cpu_milliseconds: 100,
                    wall_milliseconds: 1000,
                    memory_bytes: 64 * 1024 * 1024,
                    request_bytes: 1024,
                    response_bytes: 1024,
                    concurrency: 2,
                },
            },
            bundle_format: FunctionBundleFormat::Prebuilt,
            bundle: Arc::from(b"bundle".as_slice()),
            secrets: vec![crate::FunctionDeploymentSecret::new(
                FunctionSecretVersionReference {
                    name: FunctionSecretName::parse("API_TOKEN").expect("name"),
                    version: 3,
                },
                FunctionSecretValue::parse(SECRET_VALUE).expect("value"),
            )],
        }
    }

    fn deployment_address() -> DeploymentAddress {
        DeploymentAddress {
            tenant: tenant(),
            function_name: "hello-world".to_owned(),
            version: 7,
        }
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }

    type Handler = Box<dyn Fn(&str) -> Vec<u8> + Send>;

    fn mock_runtime(handlers: Vec<Handler>) -> (SocketAddr, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let endpoint = listener.local_addr().expect("endpoint");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for handler in handlers {
                let (mut stream, _) = listener.accept().expect("connection");
                let request = read_request(&mut stream);
                let response = handler(&request);
                sender.send(request).expect("receiver");
                stream.write_all(&response).expect("response");
            }
        });
        (endpoint, receiver)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut request = Vec::new();
        let header_end = loop {
            if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
            let mut chunk = [0_u8; 4096];
            let size = stream.read(&mut chunk).expect("header");
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
        };
        let head = std::str::from_utf8(&request[..header_end]).expect("head");
        let content_length = head
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_, value)| value.trim().parse().expect("length"));
        while request.len() < header_end + content_length {
            let mut chunk = [0_u8; 4096];
            let size = stream.read(&mut chunk).expect("body");
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
        }
        String::from_utf8(request).expect("request")
    }

    fn response<T: Serialize>(status: u16, request_id: &str, body: &T) -> Vec<u8> {
        let body = serde_json::to_vec(body).expect("JSON");
        let reason = if status == 200 { "OK" } else { "Response" };
        let mut wire = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{REQUEST_ID_HEADER}: {request_id}\r\n{PROTOCOL_VERSION_HEADER}: {PROTOCOL_VERSION}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        wire.extend_from_slice(&body);
        wire
    }

    fn request_header<'a>(request: &'a str, expected: &str) -> &'a str {
        request
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case(expected))
            .map(|(_, value)| value.trim())
            .expect("request header")
    }

    fn request_path(request: &str) -> &str {
        request.split_whitespace().nth(1).expect("request path")
    }

    fn request_json<T: DeserializeOwned>(request: &str) -> T {
        let body = request.split_once("\r\n\r\n").expect("body").1;
        serde_json::from_str(body).expect("request JSON")
    }
}
