use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mako_api::{ApiErrorEnvelope, TenantScope};
use mako_service_runtime::REQUEST_ID_HEADER;
use rand_core::{OsRng, RngCore};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    ApplicationMailAcknowledgeRequest, ApplicationMailAcknowledgeResponse,
    ApplicationMailDrainRequest, ApplicationMailDrainResponse, DeploymentKey,
    FunctionSecretResolutionRequest, IdentityAdminCommand, IdentityVerificationRequest,
    InternalAuthError, InternalCaller, InternalRequestAuthenticator, InternalRoute,
    MAX_INTERNAL_BODY_BYTES, SignedInternalRequest,
};

const MAX_RESPONSE_HEADER_BYTES: usize = 32 * 1024;
const MAX_RESPONSE_HEADERS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InternalHttpClientConfig {
    pub endpoint: SocketAddr,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub maximum_response_bytes: usize,
}

impl InternalHttpClientConfig {
    #[must_use]
    pub fn loopback(endpoint: SocketAddr) -> Self {
        Self {
            endpoint,
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(5),
            maximum_response_bytes: 1024 * 1024,
        }
    }

    fn validate(&self) -> Result<(), InternalClientError> {
        if !self.endpoint.ip().is_loopback()
            || self.connect_timeout.is_zero()
            || self.io_timeout.is_zero()
            || self.maximum_response_bytes == 0
            || self.maximum_response_bytes > 16 * 1024 * 1024
        {
            return Err(InternalClientError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InternalResponse {
    pub status: u16,
    pub request_id: String,
    pub body: Vec<u8>,
}

#[derive(Clone)]
pub struct InternalHttpClient {
    config: InternalHttpClientConfig,
    authenticator: InternalRequestAuthenticator,
}

impl InternalHttpClient {
    pub fn new(
        config: InternalHttpClientConfig,
        deployment_key: DeploymentKey,
        caller: InternalCaller,
    ) -> Result<Self, InternalClientError> {
        config.validate()?;
        Ok(Self {
            config,
            authenticator: InternalRequestAuthenticator::new(deployment_key, caller),
        })
    }

    pub fn call<T: Serialize>(
        &self,
        route: InternalRoute,
        tenant: &TenantScope,
        request_id: &str,
        idempotency_key: &str,
        payload: &T,
    ) -> Result<InternalResponse, InternalClientError> {
        let body = serde_json::to_vec(payload).map_err(|_| InternalClientError::InvalidPayload)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| InternalClientError::ClockUnavailable)?
            .as_secs();
        let request =
            self.authenticator
                .sign(route, tenant, request_id, idempotency_key, timestamp, body)?;
        self.send(&request)
    }

    pub fn call_generated<T: Serialize>(
        &self,
        route: InternalRoute,
        tenant: &TenantScope,
        payload: &T,
    ) -> Result<InternalResponse, InternalClientError> {
        let request_id = format!("req_{}", random_hex_16());
        let idempotency_key = format!("idem_{}", random_hex_16());
        self.call(route, tenant, &request_id, &idempotency_key, payload)
    }

    /// A private operational probe. Business calls remain unavailable unless
    /// their target reports ready, so callers fail closed on dependency loss.
    pub fn probe_ready(&self) -> Result<bool, InternalClientError> {
        let response = send_http(&self.config, "GET", "/readyz", &BTreeMap::new(), &[])?;
        Ok(response.status == 200)
    }

    fn send(
        &self,
        request: &SignedInternalRequest,
    ) -> Result<InternalResponse, InternalClientError> {
        if request.body.len() > MAX_INTERNAL_BODY_BYTES {
            return Err(InternalClientError::InvalidPayload);
        }
        let response = send_http(
            &self.config,
            &request.route.method().to_string(),
            request.route.path(),
            &request.headers,
            &request.body,
        )?;
        let expected_request_id = request
            .headers
            .get(REQUEST_ID_HEADER)
            .ok_or(InternalClientError::InvalidPayload)?;
        if &response.request_id != expected_request_id {
            return Err(InternalClientError::CorrelationFailed);
        }
        if !(200..300).contains(&response.status) {
            let envelope: ApiErrorEnvelope = serde_json::from_slice(&response.body)
                .map_err(|_| InternalClientError::UnsafeRemoteError)?;
            if envelope.error.request_id != *expected_request_id {
                return Err(InternalClientError::CorrelationFailed);
            }
            return Err(InternalClientError::Remote {
                status: response.status,
                envelope: Box::new(envelope),
            });
        }
        Ok(response)
    }
}

#[derive(Clone)]
pub struct ControlToDataClient(InternalHttpClient);

impl ControlToDataClient {
    pub fn new(client: InternalHttpClient) -> Result<Self, InternalClientError> {
        if client.authenticator.caller() != InternalCaller::ControlPlane {
            return Err(InternalClientError::InvalidConfiguration);
        }
        Ok(Self(client))
    }

    pub fn administer<R: DeserializeOwned>(
        &self,
        tenant: &TenantScope,
        request_id: &str,
        idempotency_key: &str,
        command: &IdentityAdminCommand,
    ) -> Result<R, InternalClientError> {
        deserialize_response(self.0.call(
            InternalRoute::IdentityAdmin,
            tenant,
            request_id,
            idempotency_key,
            command,
        )?)
    }

    pub fn dependency_ready(&self) -> Result<bool, InternalClientError> {
        self.0.probe_ready()
    }

    /// Takes a lease on application mail the data plane wants sent. The
    /// request is not tenant-scoped -- one node holds every tenant's outbox --
    /// so it travels under the fixed application-mail scope.
    pub fn drain_application_mail(
        &self,
        request_id: &str,
        request: &ApplicationMailDrainRequest,
    ) -> Result<ApplicationMailDrainResponse, InternalClientError> {
        deserialize_response(self.0.call(
            InternalRoute::ApplicationMailDrain,
            &application_mail_scope(),
            request_id,
            request_id,
            request,
        )?)
    }

    /// Tells the data plane the control plane durably holds these intents.
    pub fn acknowledge_application_mail(
        &self,
        request_id: &str,
        request: &ApplicationMailAcknowledgeRequest,
    ) -> Result<ApplicationMailAcknowledgeResponse, InternalClientError> {
        deserialize_response(self.0.call(
            InternalRoute::ApplicationMailAcknowledge,
            &application_mail_scope(),
            request_id,
            request_id,
            request,
        )?)
    }
}

/// The scope application-mail routes travel under: not a tenant, a name for
/// the node-wide outbox, so the signed envelope still names a scope.
#[must_use]
pub fn application_mail_scope() -> TenantScope {
    TenantScope::new(
        mako_api::ProjectId::parse("prj_applicationmail0").expect("fixed project id"),
        mako_api::EnvironmentId::parse("env_applicationmail0").expect("fixed environment id"),
    )
}

#[derive(Clone)]
pub struct EdgeToDataClient(InternalHttpClient);

impl EdgeToDataClient {
    pub fn new(client: InternalHttpClient) -> Result<Self, InternalClientError> {
        if client.authenticator.caller() != InternalCaller::EdgeGateway {
            return Err(InternalClientError::InvalidConfiguration);
        }
        Ok(Self(client))
    }

    pub fn verify<R: DeserializeOwned>(
        &self,
        tenant: &TenantScope,
        request_id: &str,
        request: &IdentityVerificationRequest,
    ) -> Result<R, InternalClientError> {
        deserialize_response(self.0.call(
            InternalRoute::IdentityVerify,
            tenant,
            request_id,
            request_id,
            request,
        )?)
    }

    pub fn dependency_ready(&self) -> Result<bool, InternalClientError> {
        self.0.probe_ready()
    }
}

#[derive(Clone)]
pub struct EdgeToControlClient(InternalHttpClient);

impl EdgeToControlClient {
    pub fn new(client: InternalHttpClient) -> Result<Self, InternalClientError> {
        if client.authenticator.caller() != InternalCaller::EdgeGateway {
            return Err(InternalClientError::InvalidConfiguration);
        }
        Ok(Self(client))
    }

    pub fn resolve<R: DeserializeOwned>(
        &self,
        tenant: &TenantScope,
        request_id: &str,
        request: &FunctionSecretResolutionRequest,
    ) -> Result<R, InternalClientError> {
        deserialize_response(self.0.call(
            InternalRoute::FunctionSecretsResolve,
            tenant,
            request_id,
            request_id,
            request,
        )?)
    }

    pub fn dependency_ready(&self) -> Result<bool, InternalClientError> {
        self.0.probe_ready()
    }
}

fn deserialize_response<R: DeserializeOwned>(
    response: InternalResponse,
) -> Result<R, InternalClientError> {
    serde_json::from_slice(&response.body).map_err(|_| InternalClientError::InvalidResponse)
}

fn send_http(
    config: &InternalHttpClientConfig,
    method: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> Result<InternalResponse, InternalClientError> {
    let mut stream = TcpStream::connect_timeout(&config.endpoint, config.connect_timeout)
        .map_err(|_| InternalClientError::Unavailable)?;
    stream
        .set_read_timeout(Some(config.io_timeout))
        .map_err(|_| InternalClientError::Unavailable)?;
    stream
        .set_write_timeout(Some(config.io_timeout))
        .map_err(|_| InternalClientError::Unavailable)?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
        config.endpoint,
        body.len(),
    )
    .map_err(|_| InternalClientError::Unavailable)?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").map_err(|_| InternalClientError::Unavailable)?;
    }
    stream
        .write_all(b"\r\n")
        .and_then(|()| stream.write_all(body))
        .and_then(|()| stream.flush())
        .map_err(|_| InternalClientError::Unavailable)?;

    let maximum = config
        .maximum_response_bytes
        .saturating_add(MAX_RESPONSE_HEADER_BYTES)
        .saturating_add(1);
    let mut wire = Vec::new();
    stream
        .take(u64::try_from(maximum).unwrap_or(u64::MAX))
        .read_to_end(&mut wire)
        .map_err(|_| InternalClientError::Unavailable)?;
    if wire.len() >= maximum {
        return Err(InternalClientError::ResponseTooLarge);
    }
    parse_response(&wire, config.maximum_response_bytes)
}

fn parse_response(
    wire: &[u8],
    maximum_response_bytes: usize,
) -> Result<InternalResponse, InternalClientError> {
    let header_end = wire
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(InternalClientError::InvalidResponse)?;
    if header_end > MAX_RESPONSE_HEADER_BYTES {
        return Err(InternalClientError::InvalidResponse);
    }
    let head = std::str::from_utf8(&wire[..header_end])
        .map_err(|_| InternalClientError::InvalidResponse)?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(InternalClientError::InvalidResponse)?;
    let mut status_parts = status_line.split_ascii_whitespace();
    if status_parts.next() != Some("HTTP/1.1") {
        return Err(InternalClientError::InvalidResponse);
    }
    let status = status_parts
        .next()
        .ok_or(InternalClientError::InvalidResponse)?
        .parse::<u16>()
        .map_err(|_| InternalClientError::InvalidResponse)?;
    let header_lines = lines.collect::<Vec<_>>();
    if header_lines.len() > MAX_RESPONSE_HEADERS {
        return Err(InternalClientError::InvalidResponse);
    }
    let mut parsed = BTreeMap::new();
    for line in header_lines {
        let (name, value) = line
            .split_once(':')
            .ok_or(InternalClientError::InvalidResponse)?;
        let name = name.trim().to_ascii_lowercase();
        if parsed.insert(name, value.trim().to_owned()).is_some() {
            return Err(InternalClientError::InvalidResponse);
        }
    }
    if parsed.contains_key("transfer-encoding") {
        return Err(InternalClientError::InvalidResponse);
    }
    let content_length = parsed
        .get("content-length")
        .ok_or(InternalClientError::InvalidResponse)?
        .parse::<usize>()
        .map_err(|_| InternalClientError::InvalidResponse)?;
    let body = wire
        .get(header_end + 4..)
        .ok_or(InternalClientError::InvalidResponse)?;
    if body.len() != content_length || body.len() > maximum_response_bytes {
        return Err(InternalClientError::ResponseTooLarge);
    }
    let request_id = parsed
        .remove(REQUEST_ID_HEADER)
        .ok_or(InternalClientError::CorrelationFailed)?;
    Ok(InternalResponse {
        status,
        request_id,
        body: body.to_vec(),
    })
}

fn random_hex_16() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut output = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("formatting into a string cannot fail");
    }
    output
}

#[derive(Debug)]
pub enum InternalClientError {
    InvalidConfiguration,
    InvalidPayload,
    ClockUnavailable,
    Unavailable,
    ResponseTooLarge,
    InvalidResponse,
    CorrelationFailed,
    UnsafeRemoteError,
    Remote {
        status: u16,
        envelope: Box<ApiErrorEnvelope>,
    },
    Authentication(InternalAuthError),
}

impl InternalClientError {
    #[must_use]
    pub const fn is_dependency_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

impl fmt::Display for InternalClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "internal client configuration is invalid",
            Self::InvalidPayload => "internal client payload is invalid",
            Self::ClockUnavailable => "system clock is unavailable",
            Self::Unavailable => "internal dependency is unavailable",
            Self::ResponseTooLarge => "internal response exceeded its configured bound",
            Self::InvalidResponse => "internal dependency returned an invalid response",
            Self::CorrelationFailed => "internal response correlation failed",
            Self::UnsafeRemoteError => "internal dependency returned an unsafe error",
            Self::Remote { .. } => "internal dependency rejected the request",
            Self::Authentication(_) => "internal request could not be authenticated",
        })
    }
}

impl Error for InternalClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Authentication(error) => Some(error),
            Self::InvalidConfiguration
            | Self::InvalidPayload
            | Self::ClockUnavailable
            | Self::Unavailable
            | Self::ResponseTooLarge
            | Self::InvalidResponse
            | Self::CorrelationFailed
            | Self::UnsafeRemoteError
            | Self::Remote { .. } => None,
        }
    }
}

impl From<InternalAuthError> for InternalClientError {
    fn from(error: InternalAuthError) -> Self {
        Self::Authentication(error)
    }
}
