use std::{collections::BTreeMap, error::Error, fmt, str::FromStr};

use mako_api::{ErrorCode, RetryAdvice, TenantScope};
use mako_service_runtime::{HttpApiError, HttpRequest, REQUEST_ID_HEADER};
use rand_core::{OsRng, RngCore};

use crate::contract::{
    CALLER_HEADER, DIGEST_HEADER, ENVIRONMENT_HEADER, IDEMPOTENCY_HEADER,
    INTERNAL_PROTOCOL_VERSION, InternalCaller, InternalRoute, MAX_INTERNAL_BODY_BYTES,
    NONCE_HEADER, PROJECT_HEADER, SIGNATURE_HEADER, TIMESTAMP_HEADER, VERSION_HEADER,
};

const MAX_CLOCK_SKEW_SECONDS: u64 = 30;
const MAX_IDENTIFIER_BYTES: usize = 128;

/// A domain-separated deployment key. Debug output never exposes key bytes.
#[derive(Clone)]
pub struct DeploymentKey([u8; 32]);

impl DeploymentKey {
    pub fn derive(secret: &str) -> Result<Self, InternalAuthError> {
        if secret.len() < 32 || secret.len() > 64 * 1024 || secret.chars().any(char::is_control) {
            return Err(InternalAuthError::InvalidKeyMaterial);
        }
        Ok(Self(blake3::derive_key(
            "mako/internal-rpc/deployment-request-signing/v1",
            secret.as_bytes(),
        )))
    }

    pub(crate) fn response_encryption_key(&self) -> [u8; 32] {
        blake3::derive_key("mako/internal-rpc/success-response-encryption/v1", &self.0)
    }
}

impl fmt::Debug for DeploymentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeploymentKey([REDACTED])")
    }
}

/// Complete request emitted by an internal client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedInternalRequest {
    pub route: InternalRoute,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

/// Authenticated context bound to the request body, tenant, caller and route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedInternalRequest {
    pub route: InternalRoute,
    pub caller: InternalCaller,
    pub tenant: TenantScope,
    pub request_id: String,
    pub idempotency_key: String,
    pub nonce: String,
    pub timestamp_unix_seconds: u64,
    pub operation_digest: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct InternalRequestAuthenticator {
    key: DeploymentKey,
    caller: InternalCaller,
}

impl InternalRequestAuthenticator {
    #[must_use]
    pub const fn new(key: DeploymentKey, caller: InternalCaller) -> Self {
        Self { key, caller }
    }

    pub(crate) const fn caller(&self) -> InternalCaller {
        self.caller
    }

    pub fn sign(
        &self,
        route: InternalRoute,
        tenant: &TenantScope,
        request_id: &str,
        idempotency_key: &str,
        timestamp_unix_seconds: u64,
        body: Vec<u8>,
    ) -> Result<SignedInternalRequest, InternalAuthError> {
        if route.caller() != self.caller {
            return Err(InternalAuthError::CallerNotAllowed);
        }
        validate_request_parts(request_id, idempotency_key, &body)?;
        let nonce = random_nonce();
        let body_digest = digest_hex(&body);
        let canonical = canonical_request(&CanonicalRequest {
            route,
            caller: self.caller,
            tenant,
            request_id,
            idempotency_key,
            timestamp: timestamp_unix_seconds,
            nonce: &nonce,
            body_digest: &body_digest,
        });
        let signature = keyed_digest_hex(&self.key, canonical.as_bytes());
        let headers = BTreeMap::from([
            (
                VERSION_HEADER.to_owned(),
                INTERNAL_PROTOCOL_VERSION.to_owned(),
            ),
            (CALLER_HEADER.to_owned(), self.caller.as_str().to_owned()),
            (
                PROJECT_HEADER.to_owned(),
                tenant.project_id().as_str().to_owned(),
            ),
            (
                ENVIRONMENT_HEADER.to_owned(),
                tenant.environment_id().as_str().to_owned(),
            ),
            (REQUEST_ID_HEADER.to_owned(), request_id.to_owned()),
            (IDEMPOTENCY_HEADER.to_owned(), idempotency_key.to_owned()),
            (
                TIMESTAMP_HEADER.to_owned(),
                timestamp_unix_seconds.to_string(),
            ),
            (NONCE_HEADER.to_owned(), nonce),
            (DIGEST_HEADER.to_owned(), body_digest),
            (SIGNATURE_HEADER.to_owned(), signature),
            ("content-type".to_owned(), "application/json".to_owned()),
        ]);
        Ok(SignedInternalRequest {
            route,
            headers,
            body,
        })
    }

    pub fn verify(
        &self,
        expected_route: InternalRoute,
        request: &HttpRequest,
        now_unix_seconds: u64,
    ) -> Result<VerifiedInternalRequest, InternalAuthError> {
        self.verify_parts(
            expected_route,
            IncomingRequestParts {
                method: request.method(),
                path: request.path(),
                body: request.body(),
                transport_request_id: request.request_id(),
                header: |name| request.header(name),
            },
            now_unix_seconds,
        )
    }

    /// Verifies a signed request before transport. This is also useful for
    /// deterministic protocol tests and non-HTTP loopback transports.
    pub fn verify_signed(
        &self,
        expected_route: InternalRoute,
        request: &SignedInternalRequest,
        now_unix_seconds: u64,
    ) -> Result<VerifiedInternalRequest, InternalAuthError> {
        let request_id = request
            .headers
            .get(REQUEST_ID_HEADER)
            .ok_or(InternalAuthError::Unauthenticated)?;
        self.verify_parts(
            expected_route,
            IncomingRequestParts {
                method: request.route.method(),
                path: request.route.path(),
                body: &request.body,
                transport_request_id: request_id,
                header: |name| request.headers.get(name).map(String::as_str),
            },
            now_unix_seconds,
        )
    }

    fn verify_parts<'a, F>(
        &self,
        expected_route: InternalRoute,
        request: IncomingRequestParts<'a, F>,
        now_unix_seconds: u64,
    ) -> Result<VerifiedInternalRequest, InternalAuthError>
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        if request.method != expected_route.method() || request.path != expected_route.path() {
            return Err(InternalAuthError::RouteNotAllowed);
        }
        if request.body.is_empty() || request.body.len() > MAX_INTERNAL_BODY_BYTES {
            return Err(InternalAuthError::InvalidBody);
        }
        if (request.header)("content-type") != Some("application/json") {
            return Err(InternalAuthError::InvalidBody);
        }
        if (request.header)(VERSION_HEADER) != Some(INTERNAL_PROTOCOL_VERSION) {
            return Err(InternalAuthError::UnsupportedVersion);
        }
        let caller = (request.header)(CALLER_HEADER)
            .ok_or(())
            .and_then(|value| InternalCaller::from_str(value).map_err(|_| ()))
            .map_err(|()| InternalAuthError::Unauthenticated)?;
        if caller != self.caller || caller != expected_route.caller() {
            return Err(InternalAuthError::CallerNotAllowed);
        }
        let tenant = TenantScope::require(
            (request.header)(PROJECT_HEADER),
            (request.header)(ENVIRONMENT_HEADER),
        )
        .map_err(|_| InternalAuthError::InvalidTenant)?;
        let request_id = required_identifier((request.header)(REQUEST_ID_HEADER))?;
        if request_id != request.transport_request_id {
            return Err(InternalAuthError::Unauthenticated);
        }
        let idempotency_key = required_identifier((request.header)(IDEMPOTENCY_HEADER))?;
        let nonce = required_identifier((request.header)(NONCE_HEADER))?;
        if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(InternalAuthError::Unauthenticated);
        }
        let timestamp = (request.header)(TIMESTAMP_HEADER)
            .ok_or(())
            .and_then(|value| value.parse::<u64>().map_err(|_| ()))
            .map_err(|()| InternalAuthError::Unauthenticated)?;
        if now_unix_seconds.abs_diff(timestamp) > MAX_CLOCK_SKEW_SECONDS {
            return Err(InternalAuthError::Expired);
        }
        let supplied_digest =
            (request.header)(DIGEST_HEADER).ok_or(InternalAuthError::Unauthenticated)?;
        let expected_digest = digest_hex(request.body);
        if !valid_digest(supplied_digest)
            || !constant_time_equal(supplied_digest.as_bytes(), expected_digest.as_bytes())
        {
            return Err(InternalAuthError::Unauthenticated);
        }
        let supplied_signature =
            (request.header)(SIGNATURE_HEADER).ok_or(InternalAuthError::Unauthenticated)?;
        let canonical = canonical_request(&CanonicalRequest {
            route: expected_route,
            caller,
            tenant: &tenant,
            request_id: &request_id,
            idempotency_key: &idempotency_key,
            timestamp,
            nonce: &nonce,
            body_digest: supplied_digest,
        });
        let expected_signature = keyed_digest_hex(&self.key, canonical.as_bytes());
        if !valid_digest(supplied_signature)
            || !constant_time_equal(supplied_signature.as_bytes(), expected_signature.as_bytes())
        {
            return Err(InternalAuthError::Unauthenticated);
        }
        let operation_digest = blake3::hash(
            format!(
                "{}\n{}\n{}\n{}\n{}\n{}\n{}",
                caller,
                expected_route.method(),
                expected_route.path(),
                tenant.project_id().as_str(),
                tenant.environment_id().as_str(),
                idempotency_key,
                supplied_digest,
            )
            .as_bytes(),
        );
        Ok(VerifiedInternalRequest {
            route: expected_route,
            caller,
            tenant,
            request_id,
            idempotency_key,
            nonce,
            timestamp_unix_seconds: timestamp,
            operation_digest: *operation_digest.as_bytes(),
        })
    }
}

struct IncomingRequestParts<'a, F>
where
    F: Fn(&str) -> Option<&'a str>,
{
    method: mako_service_runtime::HttpMethod,
    path: &'a str,
    body: &'a [u8],
    transport_request_id: &'a str,
    header: F,
}

fn validate_request_parts(
    request_id: &str,
    idempotency_key: &str,
    body: &[u8],
) -> Result<(), InternalAuthError> {
    if !valid_identifier(request_id) || !valid_identifier(idempotency_key) {
        return Err(InternalAuthError::InvalidIdentifier);
    }
    if body.is_empty() || body.len() > MAX_INTERNAL_BODY_BYTES {
        return Err(InternalAuthError::InvalidBody);
    }
    serde_json::from_slice::<serde_json::Value>(body)
        .map_err(|_| InternalAuthError::InvalidBody)?;
    Ok(())
}

fn required_identifier(value: Option<&str>) -> Result<String, InternalAuthError> {
    let value = value.ok_or(InternalAuthError::Unauthenticated)?;
    if !valid_identifier(value) {
        return Err(InternalAuthError::Unauthenticated);
    }
    Ok(value.to_owned())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

struct CanonicalRequest<'a> {
    route: InternalRoute,
    caller: InternalCaller,
    tenant: &'a TenantScope,
    request_id: &'a str,
    idempotency_key: &'a str,
    timestamp: u64,
    nonce: &'a str,
    body_digest: &'a str,
}

fn canonical_request(request: &CanonicalRequest<'_>) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        INTERNAL_PROTOCOL_VERSION,
        request.caller,
        request.route.method(),
        request.route.path(),
        request.tenant.project_id().as_str(),
        request.tenant.environment_id().as_str(),
        request.request_id,
        request.idempotency_key,
        request.timestamp,
        request.nonce,
        request.body_digest,
    )
}

fn digest_hex(value: &[u8]) -> String {
    encode_hex(blake3::hash(value).as_bytes())
}

fn keyed_digest_hex(key: &DeploymentKey, value: &[u8]) -> String {
    encode_hex(blake3::keyed_hash(&key.0, value).as_bytes())
}

fn random_nonce() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    encode_hex(&bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let difference = left
        .iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        });
    difference == 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InternalAuthError {
    InvalidKeyMaterial,
    InvalidIdentifier,
    InvalidBody,
    UnsupportedVersion,
    Unauthenticated,
    CallerNotAllowed,
    RouteNotAllowed,
    InvalidTenant,
    Expired,
}

impl InternalAuthError {
    #[must_use]
    pub fn to_http_error(self, request_id: &str) -> HttpApiError {
        let (status, code, message) = match self {
            Self::InvalidBody | Self::InvalidIdentifier | Self::UnsupportedVersion => (
                400,
                ErrorCode::InvalidRequest,
                "internal request is invalid",
            ),
            Self::CallerNotAllowed | Self::RouteNotAllowed | Self::InvalidTenant => (
                403,
                ErrorCode::PermissionDenied,
                "internal request is not authorized",
            ),
            Self::InvalidKeyMaterial | Self::Unauthenticated | Self::Expired => (
                401,
                ErrorCode::Unauthenticated,
                "internal request authentication failed",
            ),
        };
        HttpApiError::new(status, code, message, request_id, RetryAdvice::Never)
    }
}

impl fmt::Display for InternalAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidKeyMaterial => "internal deployment key material is invalid",
            Self::InvalidIdentifier => "internal request identifier is invalid",
            Self::InvalidBody => "internal request body is invalid",
            Self::UnsupportedVersion => "internal protocol version is unsupported",
            Self::Unauthenticated => "internal request authentication failed",
            Self::CallerNotAllowed => "internal caller is not allowed for this route",
            Self::RouteNotAllowed => "internal route or method is not allowed",
            Self::InvalidTenant => "internal tenant binding is invalid",
            Self::Expired => "internal request timestamp is outside the accepted window",
        })
    }
}

impl Error for InternalAuthError {}
