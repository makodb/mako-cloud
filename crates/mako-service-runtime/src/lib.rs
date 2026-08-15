//! Bounded, fail-closed HTTP transport shared by deployable Mako services.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    error::Error,
    fmt,
    io::{self, Cursor, Read, Write},
    net::SocketAddr,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use mako_api::{ApiError, ApiErrorEnvelope, ErrorCode, RetryAdvice, SafeDetail};
use rand_core::{OsRng, RngCore};
use serde::Serialize;
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use tiny_http::{HTTPVersion, Header, Method, Request, Response, ResponseBox, Server, StatusCode};

const RECEIVE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const STREAM_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Header used to correlate public responses, audit records, and internal calls.
pub const REQUEST_ID_HEADER: &str = "x-mako-request-id";
/// Conservative default for JSON API requests. Individual domain routes can
/// enforce smaller limits after the transport has bounded allocation.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 1_048_576;
/// Prevents an attacker from creating an unbounded number of request or stream
/// worker threads.
pub const DEFAULT_MAX_IN_FLIGHT_REQUESTS: usize = 128;
/// Matches the system service stop budget used by the beta deployment.
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(20);

const MAX_REQUEST_TARGET_BYTES: usize = 8_192;
const MAX_HEADER_COUNT: usize = 128;
const MAX_HEADER_BYTES: usize = 32_768;

/// Runtime limits and private listener identity for one native service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpTransportConfig {
    pub bind_address: SocketAddr,
    pub component: String,
    pub readiness_detail: String,
    pub max_request_body_bytes: usize,
    pub max_in_flight_requests: usize,
    pub shutdown_grace: Duration,
}

/// Safe, dynamically evaluated dependency readiness for the private route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadinessSnapshot {
    pub ready: bool,
    pub detail: String,
}

impl ReadinessSnapshot {
    #[must_use]
    pub fn ready(detail: impl Into<String>) -> Self {
        Self {
            ready: true,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn not_ready(detail: impl Into<String>) -> Self {
        Self {
            ready: false,
            detail: detail.into(),
        }
    }

    fn sanitized(self) -> Self {
        if self.detail.is_empty()
            || self.detail.len() > 512
            || self.detail.chars().any(char::is_control)
        {
            return Self::not_ready("readiness_detail_invalid");
        }
        self
    }
}

/// Production graphs implement this probe so `/readyz` reflects current
/// storage and dependency state instead of a startup-only success.
pub trait ReadinessProbe: Send + Sync {
    fn snapshot(&self) -> ReadinessSnapshot;
}

struct StaticReadinessProbe(String);

impl ReadinessProbe for StaticReadinessProbe {
    fn snapshot(&self) -> ReadinessSnapshot {
        ReadinessSnapshot::ready(self.0.clone())
    }
}

impl HttpTransportConfig {
    #[must_use]
    pub fn new(
        bind_address: SocketAddr,
        component: impl Into<String>,
        readiness_detail: impl Into<String>,
    ) -> Self {
        Self {
            bind_address,
            component: component.into(),
            readiness_detail: readiness_detail.into(),
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            max_in_flight_requests: DEFAULT_MAX_IN_FLIGHT_REQUESTS,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }

    fn validate(&self) -> Result<(), ServiceRuntimeError> {
        if !self.bind_address.ip().is_loopback() {
            return Err(ServiceRuntimeError::NonLoopbackBind);
        }
        if self.component.is_empty()
            || self.readiness_detail.is_empty()
            || self.max_request_body_bytes == 0
            || self.max_in_flight_requests == 0
            || self.shutdown_grace.is_zero()
        {
            return Err(ServiceRuntimeError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// HTTP methods accepted by the explicit route allowlist.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HttpMethod {
    Delete,
    Get,
    Head,
    Options,
    Patch,
    Post,
    Put,
}

impl HttpMethod {
    fn from_tiny(method: &Method) -> Option<Self> {
        match method {
            Method::Delete => Some(Self::Delete),
            Method::Get => Some(Self::Get),
            Method::Head => Some(Self::Head),
            Method::Options => Some(Self::Options),
            Method::Patch => Some(Self::Patch),
            Method::Post => Some(Self::Post),
            Method::Put => Some(Self::Put),
            _ => None,
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Delete => "DELETE",
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Patch => "PATCH",
            Self::Post => "POST",
            Self::Put => "PUT",
        })
    }
}

/// A fully bounded request passed to a domain route handler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    method: HttpMethod,
    path: String,
    query: Vec<(String, String)>,
    path_parameters: BTreeMap<String, String>,
    headers: BTreeMap<String, Vec<String>>,
    body: Vec<u8>,
    request_id: String,
    remote_address: Option<SocketAddr>,
}

impl HttpRequest {
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub fn for_test(
        method: HttpMethod,
        path: impl Into<String>,
        headers: impl IntoIterator<Item = (String, String)>,
        body: impl Into<Vec<u8>>,
        remote_address: Option<SocketAddr>,
    ) -> Self {
        let mut normalized = BTreeMap::<String, Vec<String>>::new();
        for (name, value) in headers {
            normalized
                .entry(name.to_ascii_lowercase())
                .or_default()
                .push(value);
        }
        Self {
            method,
            path: path.into(),
            query: Vec::new(),
            path_parameters: BTreeMap::new(),
            headers: normalized,
            body: body.into(),
            request_id: "req_test_support".to_owned(),
            remote_address,
        }
    }
    #[must_use]
    pub const fn method(&self) -> HttpMethod {
        self.method
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn query(&self) -> &[(String, String)] {
        &self.query
    }

    #[must_use]
    pub fn path_parameter(&self, name: &str) -> Option<&str> {
        self.path_parameters.get(name).map(String::as_str)
    }

    #[must_use]
    pub fn header_values(&self, name: &str) -> &[String] {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// Returns a header only when it appeared exactly once.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        let values = self.header_values(name);
        (values.len() == 1).then(|| values[0].as_str())
    }

    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    #[must_use]
    pub const fn remote_address(&self) -> Option<SocketAddr> {
        self.remote_address
    }
}

type HandlerResult = Result<HttpResponse, HttpApiError>;
type RouteHandler = Arc<dyn Fn(HttpRequest) -> HandlerResult + Send + Sync>;

#[derive(Clone)]
struct Route {
    method: HttpMethod,
    pattern: RoutePattern,
    handler: RouteHandler,
}

/// Explicit method-and-path allowlist for production domain handlers.
#[derive(Clone, Default)]
pub struct HttpRouter {
    routes: Vec<Route>,
}

impl HttpRouter {
    #[must_use]
    pub const fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Read-only route-contract inspection used by production qualification.
    /// It never exposes handlers or changes routing behavior.
    #[must_use]
    pub fn permits(&self, method: HttpMethod, path: &str) -> bool {
        matches!(
            self.resolve(Some(method), path),
            RouteResolution::Matched { .. }
        )
    }

    #[must_use]
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn dispatch_for_test(&self, mut request: HttpRequest) -> Option<HandlerResult> {
        let RouteResolution::Matched {
            handler,
            parameters,
        } = self.resolve(Some(request.method), &request.path)
        else {
            return None;
        };
        request.path_parameters = parameters;
        Some(handler(request))
    }

    /// Registers an exact segmented path. A segment written as `{name}` binds a
    /// single non-empty path parameter; wildcards and optional segments are not
    /// supported.
    pub fn add_route<F>(
        &mut self,
        method: HttpMethod,
        pattern: &str,
        handler: F,
    ) -> Result<(), RouteRegistrationError>
    where
        F: Fn(HttpRequest) -> HandlerResult + Send + Sync + 'static,
    {
        let parsed = RoutePattern::parse(pattern)?;
        if parsed.captures("/healthz").is_some() || parsed.captures("/readyz").is_some() {
            return Err(RouteRegistrationError::ReservedReadinessRoute);
        }
        if self.routes.iter().any(|existing| {
            existing.method == method
                && existing.pattern.specificity == parsed.specificity
                && existing.pattern.overlaps(&parsed)
        }) {
            return Err(RouteRegistrationError::AmbiguousRoute);
        }
        self.routes.push(Route {
            method,
            pattern: parsed,
            handler: Arc::new(handler),
        });
        Ok(())
    }

    fn resolve(&self, method: Option<HttpMethod>, path: &str) -> RouteResolution {
        let mut allowed = BTreeSet::new();
        let mut selected: Option<(usize, RouteHandler, BTreeMap<String, String>)> = None;
        for route in &self.routes {
            let Some(parameters) = route.pattern.captures(path) else {
                continue;
            };
            allowed.insert(route.method);
            if method != Some(route.method) {
                continue;
            }
            let candidate = (
                route.pattern.specificity,
                Arc::clone(&route.handler),
                parameters,
            );
            if selected
                .as_ref()
                .is_none_or(|current| candidate.0 > current.0)
            {
                selected = Some(candidate);
            }
        }
        if let Some((_, handler, parameters)) = selected {
            RouteResolution::Matched {
                handler,
                parameters,
            }
        } else if allowed.is_empty() {
            RouteResolution::NotFound
        } else {
            RouteResolution::MethodNotAllowed(allowed)
        }
    }
}

enum RouteResolution {
    Matched {
        handler: RouteHandler,
        parameters: BTreeMap<String, String>,
    },
    MethodNotAllowed(BTreeSet<HttpMethod>),
    NotFound,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RoutePattern {
    segments: Vec<RouteSegment>,
    specificity: usize,
}

impl RoutePattern {
    fn parse(value: &str) -> Result<Self, RouteRegistrationError> {
        if !valid_request_path(value) || value.contains('?') || value.contains('#') {
            return Err(RouteRegistrationError::InvalidPattern);
        }
        let mut names = HashSet::new();
        let mut specificity = 0;
        let mut segments = Vec::new();
        for segment in split_path(value).ok_or(RouteRegistrationError::InvalidPattern)? {
            if let Some(name) = segment
                .strip_prefix('{')
                .and_then(|part| part.strip_suffix('}'))
            {
                if !valid_parameter_name(name) || !names.insert(name.to_owned()) {
                    return Err(RouteRegistrationError::InvalidPattern);
                }
                segments.push(RouteSegment::Parameter(name.to_owned()));
            } else {
                if segment.contains(['{', '}']) {
                    return Err(RouteRegistrationError::InvalidPattern);
                }
                specificity += 1;
                segments.push(RouteSegment::Static(segment.to_owned()));
            }
        }
        Ok(Self {
            segments,
            specificity,
        })
    }

    fn captures(&self, path: &str) -> Option<BTreeMap<String, String>> {
        let incoming = split_path(path)?;
        if incoming.len() != self.segments.len() {
            return None;
        }
        let mut parameters = BTreeMap::new();
        for (pattern, value) in self.segments.iter().zip(incoming) {
            match pattern {
                RouteSegment::Static(expected) if expected != value => return None,
                RouteSegment::Static(_) => {}
                RouteSegment::Parameter(name) => {
                    parameters.insert(name.clone(), value.to_owned());
                }
            }
        }
        Some(parameters)
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.segments.len() == other.segments.len()
            && self
                .segments
                .iter()
                .zip(&other.segments)
                .all(|(left, right)| match (left, right) {
                    (RouteSegment::Static(left), RouteSegment::Static(right)) => left == right,
                    _ => true,
                })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RouteSegment {
    Static(String),
    Parameter(String),
}

fn split_path(path: &str) -> Option<Vec<&str>> {
    if path == "/" {
        return Some(Vec::new());
    }
    let remainder = path.strip_prefix('/')?;
    let segments = remainder.split('/').collect::<Vec<_>>();
    (!segments.iter().any(|segment| segment.is_empty())).then_some(segments)
}

fn valid_request_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= MAX_REQUEST_TARGET_BYTES
        && path.is_ascii()
        && !path.contains(['%', '\\', '\0'])
        && split_path(path).is_some()
}

fn valid_parameter_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// A bounded, channel-backed response body for SSE and function streaming.
/// Dropping the response disconnects producers, and service shutdown closes the
/// reader without waiting indefinitely for a new chunk.
pub struct StreamingBody {
    receiver: Receiver<Vec<u8>>,
    pending: Cursor<Vec<u8>>,
    stopping: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
    producer: Option<JoinHandle<()>>,
}

impl StreamingBody {
    fn with_shutdown(mut self, stopping: Arc<AtomicBool>) -> Self {
        self.stopping = stopping;
        self
    }
}

impl Read for StreamingBody {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let read = self.pending.read(output)?;
            if read > 0 {
                return Ok(read);
            }
            if self.stopping.load(Ordering::Relaxed) {
                return Ok(0);
            }
            match self.receiver.recv_timeout(STREAM_POLL_INTERVAL) {
                Ok(chunk) if chunk.is_empty() => {}
                Ok(chunk) => self.pending = Cursor::new(chunk),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
    }
}

impl Drop for StreamingBody {
    fn drop(&mut self) {
        self.disconnected.store(true, Ordering::Relaxed);
        if let Some(producer) = self.producer.take() {
            let _ = producer.join();
        }
    }
}

/// Producer half of a bounded streaming response.
#[derive(Clone)]
pub struct StreamSender {
    sender: SyncSender<Vec<u8>>,
    disconnected: Arc<AtomicBool>,
}

impl StreamSender {
    pub fn send(&self, chunk: impl Into<Vec<u8>>) -> Result<(), StreamSendError> {
        self.sender
            .send(chunk.into())
            .map_err(|_| StreamSendError::Closed)
    }

    pub fn try_send(&self, chunk: impl Into<Vec<u8>>) -> Result<(), StreamSendError> {
        self.sender
            .try_send(chunk.into())
            .map_err(|error| match error {
                TrySendError::Full(_) => StreamSendError::Full,
                TrySendError::Disconnected(_) => StreamSendError::Closed,
            })
    }

    /// Reports that the response body was dropped by the transport or client.
    /// Stream producers should check this between bounded pieces of work.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Relaxed)
    }
}

/// Creates a bounded streaming body. A zero-capacity channel is intentionally
/// rejected because it can deadlock a handler before the response is returned.
pub fn streaming_body(capacity: usize) -> Result<(StreamSender, StreamingBody), StreamConfigError> {
    if capacity == 0 {
        return Err(StreamConfigError);
    }
    let (sender, receiver) = sync_channel(capacity);
    let disconnected = Arc::new(AtomicBool::new(false));
    Ok((
        StreamSender {
            sender,
            disconnected: Arc::clone(&disconnected),
        },
        StreamingBody {
            receiver,
            pending: Cursor::new(Vec::new()),
            stopping: Arc::new(AtomicBool::new(false)),
            disconnected,
            producer: None,
        },
    ))
}

/// Creates a bounded stream and owns its producer until the response body is
/// dropped. The producer must avoid unbounded blocking and observe
/// [`StreamSender::is_disconnected`] between bounded operations.
pub fn spawn_streaming_body<F>(
    capacity: usize,
    producer: F,
) -> Result<StreamingBody, StreamConfigError>
where
    F: FnOnce(StreamSender) + Send + 'static,
{
    let (sender, mut body) = streaming_body(capacity)?;
    body.producer = Some(thread::spawn(move || producer(sender)));
    Ok(body)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamSendError {
    Full,
    Closed,
}

impl fmt::Display for StreamSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Full => "stream buffer is full",
            Self::Closed => "stream receiver is closed",
        })
    }
}

impl Error for StreamSendError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamConfigError;

impl fmt::Display for StreamConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("stream capacity must be positive")
    }
}

impl Error for StreamConfigError {}

enum ResponseBody {
    Fixed(Cursor<Vec<u8>>),
    Streaming(StreamingBody),
}

impl Read for ResponseBody {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Fixed(body) => body.read(output),
            Self::Streaming(body) => body.read(output),
        }
    }
}

/// A response with security-sensitive headers owned by the transport.
pub struct HttpResponse {
    status: u16,
    content_type: &'static str,
    headers: Vec<Header>,
    body: ResponseBody,
    body_length: Option<usize>,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("content_type", &self.content_type)
            .field("header_count", &self.headers.len())
            .field("body_length", &self.body_length)
            .finish_non_exhaustive()
    }
}

impl HttpResponse {
    #[must_use]
    pub fn empty(status: u16) -> Self {
        Self::bytes(status, "application/octet-stream", Vec::new())
    }

    #[must_use]
    pub fn bytes(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        let body_length = body.len();
        Self {
            status,
            content_type,
            headers: Vec::new(),
            body: ResponseBody::Fixed(Cursor::new(body)),
            body_length: Some(body_length),
        }
    }

    pub fn json<T: Serialize>(status: u16, value: &T) -> Result<Self, JsonResponseError> {
        serde_json::to_vec(value)
            .map(|body| Self::bytes(status, "application/json; charset=utf-8", body))
            .map_err(|_| JsonResponseError)
    }

    #[must_use]
    pub fn stream(status: u16, content_type: &'static str, body: StreamingBody) -> Self {
        Self {
            status,
            content_type,
            headers: Vec::new(),
            body: ResponseBody::Streaming(body),
            body_length: None,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Result<Self, InvalidResponseHeader> {
        if reserved_response_header(name) {
            return Err(InvalidResponseHeader);
        }
        let header = Header::from_bytes(name.as_bytes(), value.as_bytes())
            .map_err(|_| InvalidResponseHeader)?;
        self.headers.push(header);
        Ok(self)
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub const fn status_for_test(&self) -> u16 {
        self.status
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub fn header_for_test(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsonResponseError;

impl fmt::Display for JsonResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("response could not be serialized")
    }
}

impl Error for JsonResponseError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidResponseHeader;

impl fmt::Display for InvalidResponseHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("response header is invalid or transport-owned")
    }
}

impl Error for InvalidResponseHeader {}

fn reserved_response_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "cache-control"
            | "connection"
            | "content-length"
            | "content-security-policy"
            | "content-type"
            | "referrer-policy"
            | "server"
            | "transfer-encoding"
            | "x-content-type-options"
            | "x-frame-options"
            | REQUEST_ID_HEADER
    )
}

/// Stable public failure plus its HTTP status.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpApiError {
    status: u16,
    envelope: Box<ApiErrorEnvelope>,
    headers: Vec<(String, String)>,
}

impl HttpApiError {
    #[must_use]
    pub fn new(
        status: u16,
        code: ErrorCode,
        message: impl Into<String>,
        request_id: impl Into<String>,
        retry: RetryAdvice,
    ) -> Self {
        Self {
            status,
            envelope: Box::new(ApiErrorEnvelope::new(ApiError::new(
                code, message, request_id, retry,
            ))),
            headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn from_envelope(status: u16, envelope: ApiErrorEnvelope) -> Self {
        Self {
            status,
            envelope: Box::new(envelope),
            headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn envelope(&self) -> &ApiErrorEnvelope {
        &self.envelope
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    fn into_response(self) -> HttpResponse {
        let mut response = HttpResponse::bytes(
            self.status,
            "application/json; charset=utf-8",
            serde_json::to_vec(&self.envelope)
                .expect("the stable API error envelope always serializes"),
        );
        for (name, value) in self.headers {
            if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                response.headers.push(header);
            }
        }
        response
    }
}

/// Route registration failures are detected during service startup, before a
/// listener can advertise readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteRegistrationError {
    InvalidPattern,
    ReservedReadinessRoute,
    AmbiguousRoute,
}

impl fmt::Display for RouteRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPattern => "route pattern is invalid",
            Self::ReservedReadinessRoute => "private readiness routes are transport-owned",
            Self::AmbiguousRoute => "route overlaps another method-and-path pattern",
        })
    }
}

impl Error for RouteRegistrationError {}

/// Serves private liveness and readiness routes until systemd requests a stop.
///
/// The listener is intentionally restricted to loopback. Public routing remains
/// the responsibility of the separately gated reverse proxy.
pub fn serve_private_readiness(
    bind_address: SocketAddr,
    component: &'static str,
    readiness_detail: &'static str,
) -> Result<(), ServiceRuntimeError> {
    serve_http_transport(
        HttpTransportConfig::new(bind_address, component, readiness_detail),
        HttpRouter::new(),
    )
}

/// Serves private readiness and an explicit production route allowlist.
///
/// Every request is body-bounded before a handler is called. Request workers
/// and streaming buffers are independently bounded, and all listeners remain
/// loopback-only for reverse-proxy admission control.
pub fn serve_http_transport(
    config: HttpTransportConfig,
    router: HttpRouter,
) -> Result<(), ServiceRuntimeError> {
    let readiness: Arc<dyn ReadinessProbe> =
        Arc::new(StaticReadinessProbe(config.readiness_detail.clone()));
    serve_http_transport_with_readiness(config, router, readiness)
}

/// Serves a route allowlist with a dynamic fail-closed readiness probe.
pub fn serve_http_transport_with_readiness(
    config: HttpTransportConfig,
    router: HttpRouter,
    readiness: Arc<dyn ReadinessProbe>,
) -> Result<(), ServiceRuntimeError> {
    config.validate()?;
    let server = Server::http(config.bind_address).map_err(|_| ServiceRuntimeError::BindFailed)?;
    let stopping = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGTERM, Arc::clone(&stopping))
        .and_then(|_| signal_hook::flag::register(SIGINT, Arc::clone(&stopping)))
        .map_err(|_| ServiceRuntimeError::SignalRegistrationFailed)?;
    let router = Arc::new(router);
    let config = Arc::new(config);
    let mut workers = Vec::new();

    println!(
        "private HTTP listener active: component={} bind={} routes={} body_limit_bytes={} in_flight_limit={}",
        config.component,
        config.bind_address,
        router.routes.len(),
        config.max_request_body_bytes,
        config.max_in_flight_requests,
    );
    while !stopping.load(Ordering::Relaxed) {
        reap_workers(&mut workers, &config.component);
        if let Some(request) = server
            .recv_timeout(RECEIVE_POLL_INTERVAL)
            .map_err(|_| ServiceRuntimeError::ReceiveFailed)?
        {
            if workers.len() >= config.max_in_flight_requests {
                respond_overloaded(request, Arc::clone(&stopping))?;
                continue;
            }
            let request_router = Arc::clone(&router);
            let request_config = Arc::clone(&config);
            let request_stopping = Arc::clone(&stopping);
            let request_readiness = Arc::clone(&readiness);
            workers.push(thread::spawn(move || {
                if let Err(error) = process_request(
                    request,
                    &request_config,
                    &request_router,
                    &request_readiness,
                    request_stopping,
                ) {
                    eprintln!(
                        "HTTP request failed: component={} class={error}",
                        request_config.component
                    );
                }
            }));
        }
    }
    drain_workers(
        &mut workers,
        config.shutdown_grace,
        &config.component,
        &stopping,
    );
    println!("graceful shutdown complete: component={}", config.component);
    Ok(())
}

fn process_request(
    mut request: Request,
    config: &HttpTransportConfig,
    router: &HttpRouter,
    readiness: &Arc<dyn ReadinessProbe>,
    stopping: Arc<AtomicBool>,
) -> Result<(), ServiceRuntimeError> {
    let request_id = match request_id(request.headers()) {
        Ok(request_id) => request_id,
        Err(()) => {
            let request_id = generate_request_id();
            return send_response(
                request,
                invalid_request_id(&request_id).into_response(),
                &request_id,
                stopping,
            );
        }
    };
    let (path, query) = match parse_request_target(request.url()) {
        Ok(target) => target,
        Err(()) => {
            return send_response(
                request,
                invalid_request(&request_id, "request target is invalid").into_response(),
                &request_id,
                stopping,
            );
        }
    };
    let method = HttpMethod::from_tiny(request.method());
    if let Some(readiness_route) = classify_readiness(method, &path) {
        let response = match readiness_route {
            ReadinessRoute::Health => readiness_response(
                200,
                &ReadinessPayload::Health {
                    status: "ok",
                    component: &config.component,
                },
            ),
            ReadinessRoute::Ready => {
                let snapshot = readiness.snapshot().sanitized();
                readiness_response(
                    if snapshot.ready { 200 } else { 503 },
                    &ReadinessPayload::Ready {
                        status: if snapshot.ready { "ready" } else { "not_ready" },
                        component: &config.component,
                        detail: &snapshot.detail,
                    },
                )
            }
            ReadinessRoute::MethodNotAllowed => method_not_allowed(
                &request_id,
                [HttpMethod::Get, HttpMethod::Head].into_iter().collect(),
            )
            .into_response(),
        };
        return send_response(request, response, &request_id, stopping);
    }

    let resolution = router.resolve(method, &path);
    let (handler, path_parameters, method) = match resolution {
        RouteResolution::Matched {
            handler,
            parameters,
        } => (
            handler,
            parameters,
            method.expect("a matched route always has a supported method"),
        ),
        RouteResolution::MethodNotAllowed(allowed) => {
            return send_response(
                request,
                method_not_allowed(&request_id, allowed).into_response(),
                &request_id,
                stopping,
            );
        }
        RouteResolution::NotFound => {
            return send_response(
                request,
                not_found(&request_id).into_response(),
                &request_id,
                stopping,
            );
        }
    };
    let headers = match collect_headers(request.headers()) {
        Ok(headers) => headers,
        Err(()) => {
            return send_response(
                request,
                request_headers_too_large(&request_id).into_response(),
                &request_id,
                stopping,
            );
        }
    };
    let body = match read_bounded_body(&mut request, config.max_request_body_bytes) {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => {
            return send_response(
                request,
                request_body_too_large(&request_id, config.max_request_body_bytes).into_response(),
                &request_id,
                stopping,
            );
        }
        Err(BodyReadError::ReadFailed) => {
            return send_response(
                request,
                invalid_request(&request_id, "request body could not be read").into_response(),
                &request_id,
                stopping,
            );
        }
    };
    let remote_address = request.remote_addr().copied();
    let domain_request = HttpRequest {
        method,
        path,
        query,
        path_parameters,
        headers,
        body,
        request_id: request_id.clone(),
        remote_address,
    };
    let response = match catch_unwind(AssertUnwindSafe(|| handler(domain_request))) {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => error.into_response(),
        Err(_) => {
            eprintln!(
                "HTTP handler panicked: component={} request_id={request_id}",
                config.component
            );
            internal_error(&request_id).into_response()
        }
    };
    send_response(request, response, &request_id, stopping)
}

fn respond_overloaded(
    request: Request,
    stopping: Arc<AtomicBool>,
) -> Result<(), ServiceRuntimeError> {
    let request_id = request_id(request.headers()).unwrap_or_else(|()| generate_request_id());
    let response = HttpApiError::new(
        503,
        ErrorCode::Unavailable,
        "service request capacity is exhausted",
        &request_id,
        RetryAdvice::AfterDelay { after_ms: 250 },
    )
    .with_header("Retry-After", "1")
    .into_response();
    send_response(request, response, &request_id, stopping)
}

fn send_response(
    request: Request,
    response: HttpResponse,
    request_id: &str,
    stopping: Arc<AtomicBool>,
) -> Result<(), ServiceRuntimeError> {
    if matches!(&response.body, ResponseBody::Streaming(_)) {
        return send_streaming_response(request, response, request_id, stopping);
    }
    let response = build_response(response, request_id, stopping)?;
    request
        .respond(response)
        .map_err(|_| ServiceRuntimeError::ResponseFailed)
}

fn send_streaming_response(
    request: Request,
    response: HttpResponse,
    request_id: &str,
    stopping: Arc<AtomicBool>,
) -> Result<(), ServiceRuntimeError> {
    let HttpResponse {
        status,
        content_type,
        headers,
        body,
        body_length: _,
    } = response;
    let ResponseBody::Streaming(body) = body else {
        return Err(ServiceRuntimeError::ResponseFailed);
    };
    let version = request.http_version().clone();
    let headers = response_headers(headers, content_type, request_id)?;
    let mut writer = request.into_writer();
    write_streaming_response(
        writer.as_mut(),
        version,
        status,
        &headers,
        body.with_shutdown(stopping),
    )
    .or_else(ignore_client_closing_error)
    .map_err(|_| ServiceRuntimeError::ResponseFailed)
}

fn write_streaming_response(
    writer: &mut dyn Write,
    version: HTTPVersion,
    status: u16,
    headers: &[Header],
    mut body: StreamingBody,
) -> io::Result<()> {
    let chunked = version >= HTTPVersion(1, 1);
    write!(
        writer,
        "HTTP/{}.{} {} {}\r\n",
        version.0,
        version.1,
        status,
        StatusCode(status).default_reason_phrase()
    )?;
    for header in headers {
        write!(
            writer,
            "{}: {}\r\n",
            header.field.as_str(),
            header.value.as_str()
        )?;
    }
    if chunked {
        writer.write_all(b"Transfer-Encoding: chunked\r\n")?;
    } else {
        writer.write_all(b"Connection: close\r\n")?;
    }
    writer.write_all(b"\r\n")?;
    writer.flush()?;

    let mut buffer = [0_u8; 16 * 1_024];
    loop {
        let read = body.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if chunked {
            write!(writer, "{read:x}\r\n")?;
        }
        writer.write_all(&buffer[..read])?;
        if chunked {
            writer.write_all(b"\r\n")?;
        }
        writer.flush()?;
    }
    if chunked {
        writer.write_all(b"0\r\n\r\n")?;
        writer.flush()?;
    }
    Ok(())
}

fn ignore_client_closing_error(error: io::Error) -> io::Result<()> {
    match error.kind() {
        io::ErrorKind::BrokenPipe
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset => Ok(()),
        _ => Err(error),
    }
}

fn build_response(
    response: HttpResponse,
    request_id: &str,
    stopping: Arc<AtomicBool>,
) -> Result<ResponseBox, ServiceRuntimeError> {
    let headers = response_headers(response.headers, response.content_type, request_id)?;
    let body = match response.body {
        ResponseBody::Fixed(body) => ResponseBody::Fixed(body),
        ResponseBody::Streaming(body) => {
            ResponseBody::Streaming(body.with_shutdown(Arc::clone(&stopping)))
        }
    };
    Ok(Response::new(
        StatusCode(response.status),
        headers,
        body,
        response.body_length,
        None,
    )
    .boxed())
}

fn response_headers(
    mut headers: Vec<Header>,
    content_type: &'static str,
    request_id: &str,
) -> Result<Vec<Header>, ServiceRuntimeError> {
    for (name, value) in [
        ("Content-Type", content_type),
        ("Cache-Control", "no-store"),
        (
            "Content-Security-Policy",
            "default-src 'none'; frame-ancestors 'none'; base-uri 'none'",
        ),
        ("Referrer-Policy", "no-referrer"),
        ("Server", "mako"),
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        (REQUEST_ID_HEADER, request_id),
    ] {
        headers.push(
            Header::from_bytes(name.as_bytes(), value.as_bytes())
                .map_err(|_| ServiceRuntimeError::HeaderConstructionFailed)?,
        );
    }
    Ok(headers)
}

fn collect_headers(headers: &[Header]) -> Result<BTreeMap<String, Vec<String>>, ()> {
    if headers.len() > MAX_HEADER_COUNT {
        return Err(());
    }
    let total = headers.iter().try_fold(0_usize, |total, header| {
        total
            .checked_add(header.field.as_str().len())?
            .checked_add(header.value.len())
    });
    if total.is_none_or(|total| total > MAX_HEADER_BYTES) {
        return Err(());
    }
    let mut result = BTreeMap::<String, Vec<String>>::new();
    for header in headers {
        result
            .entry(header.field.as_str().as_str().to_ascii_lowercase())
            .or_default()
            .push(header.value.as_str().to_owned());
    }
    Ok(result)
}

fn read_bounded_body(
    request: &mut Request,
    maximum_bytes: usize,
) -> Result<Vec<u8>, BodyReadError> {
    if request
        .body_length()
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(BodyReadError::TooLarge);
    }
    let read_limit = u64::try_from(maximum_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut body = Vec::with_capacity(request.body_length().unwrap_or(0).min(maximum_bytes));
    request
        .as_reader()
        .take(read_limit)
        .read_to_end(&mut body)
        .map_err(|_| BodyReadError::ReadFailed)?;
    if body.len() > maximum_bytes {
        Err(BodyReadError::TooLarge)
    } else {
        Ok(body)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BodyReadError {
    TooLarge,
    ReadFailed,
}

fn parse_request_target(value: &str) -> Result<(String, Vec<(String, String)>), ()> {
    if value.len() > MAX_REQUEST_TARGET_BYTES || !value.is_ascii() || value.contains('#') {
        return Err(());
    }
    let (path, query) = value.split_once('?').unwrap_or((value, ""));
    if !valid_request_path(path) {
        return Err(());
    }
    let query = url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    Ok((path.to_owned(), query))
}

fn request_id(headers: &[Header]) -> Result<String, ()> {
    let mut values = headers
        .iter()
        .filter(|header| header.field.equiv(REQUEST_ID_HEADER));
    let Some(value) = values.next() else {
        return Ok(generate_request_id());
    };
    if values.next().is_some() || !valid_request_id(value.value.as_str()) {
        return Err(());
    }
    Ok(value.value.as_str().to_owned())
}

fn valid_request_id(value: &str) -> bool {
    (8..=128).contains(&value.len())
        && value.starts_with("req_")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn generate_request_id() -> String {
    let mut random = [0_u8; 12];
    OsRng.fill_bytes(&mut random);
    let mut request_id = String::with_capacity(28);
    request_id.push_str("req_");
    for byte in random {
        use fmt::Write;
        write!(&mut request_id, "{byte:02x}").expect("writing to a string cannot fail");
    }
    request_id
}

fn not_found(request_id: &str) -> HttpApiError {
    HttpApiError::new(
        404,
        ErrorCode::NotFound,
        "route not found",
        request_id,
        RetryAdvice::Never,
    )
}

fn method_not_allowed(request_id: &str, allowed: BTreeSet<HttpMethod>) -> HttpApiError {
    let allow = allowed
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    HttpApiError::new(
        405,
        ErrorCode::InvalidRequest,
        "method not allowed",
        request_id,
        RetryAdvice::Never,
    )
    .with_header("Allow", allow)
}

fn invalid_request(request_id: &str, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        400,
        ErrorCode::InvalidRequest,
        message,
        request_id,
        RetryAdvice::Never,
    )
}

fn invalid_request_id(request_id: &str) -> HttpApiError {
    invalid_request(request_id, "request identifier is invalid")
}

fn request_headers_too_large(request_id: &str) -> HttpApiError {
    HttpApiError::new(
        431,
        ErrorCode::InvalidRequest,
        "request headers exceed transport limits",
        request_id,
        RetryAdvice::Never,
    )
}

fn request_body_too_large(request_id: &str, limit: usize) -> HttpApiError {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut error = HttpApiError::new(
        413,
        ErrorCode::InvalidRequest,
        "request body exceeds transport limit",
        request_id,
        RetryAdvice::Never,
    );
    error
        .envelope
        .error
        .details
        .insert("limitBytes".to_owned(), SafeDetail::Integer(limit));
    error
}

fn internal_error(request_id: &str) -> HttpApiError {
    HttpApiError::new(
        500,
        ErrorCode::Internal,
        "internal service error",
        request_id,
        RetryAdvice::Never,
    )
}

#[derive(Serialize)]
#[serde(untagged)]
enum ReadinessPayload<'a> {
    Health {
        status: &'a str,
        component: &'a str,
    },
    Ready {
        status: &'a str,
        component: &'a str,
        detail: &'a str,
    },
}

fn readiness_response(status: u16, payload: &ReadinessPayload<'_>) -> HttpResponse {
    HttpResponse::json(status, payload).expect("readiness payload always serializes")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadinessRoute {
    Health,
    Ready,
    MethodNotAllowed,
}

fn classify_readiness(method: Option<HttpMethod>, path: &str) -> Option<ReadinessRoute> {
    match (method, path) {
        (Some(HttpMethod::Get | HttpMethod::Head), "/healthz") => Some(ReadinessRoute::Health),
        (Some(HttpMethod::Get | HttpMethod::Head), "/readyz") => Some(ReadinessRoute::Ready),
        (_, "/healthz" | "/readyz") => Some(ReadinessRoute::MethodNotAllowed),
        _ => None,
    }
}

fn reap_workers(workers: &mut Vec<JoinHandle<()>>, component: &str) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            if worker.join().is_err() {
                eprintln!("HTTP worker panicked: component={component}");
            }
        } else {
            index += 1;
        }
    }
}

fn drain_workers(
    workers: &mut Vec<JoinHandle<()>>,
    grace: Duration,
    component: &str,
    stopping: &AtomicBool,
) {
    stopping.store(true, Ordering::Relaxed);
    let deadline = Instant::now() + grace;
    while !workers.is_empty() && Instant::now() < deadline {
        reap_workers(workers, component);
        if !workers.is_empty() {
            thread::sleep(Duration::from_millis(10));
        }
    }
    reap_workers(workers, component);
    if !workers.is_empty() {
        eprintln!(
            "graceful shutdown budget exhausted: component={component} remaining_workers={}",
            workers.len()
        );
    }
}

/// Stable, non-sensitive failure classes for supervised service startup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceRuntimeError {
    NonLoopbackBind,
    InvalidConfiguration,
    BindFailed,
    SignalRegistrationFailed,
    ReceiveFailed,
    HeaderConstructionFailed,
    ResponseFailed,
}

impl fmt::Display for ServiceRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NonLoopbackBind => "private service listener must bind to loopback",
            Self::InvalidConfiguration => "HTTP transport configuration is invalid",
            Self::BindFailed => "private service listener could not bind",
            Self::SignalRegistrationFailed => "service shutdown handlers could not be installed",
            Self::ReceiveFailed => "private service listener failed",
            Self::HeaderConstructionFailed => "private response headers are invalid",
            Self::ResponseFailed => "private readiness response failed",
        })
    }
}

impl Error for ServiceRuntimeError {}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn preserves_only_exact_private_readiness_routes() {
        assert_eq!(
            classify_readiness(Some(HttpMethod::Get), "/healthz"),
            Some(ReadinessRoute::Health)
        );
        assert_eq!(
            classify_readiness(Some(HttpMethod::Head), "/readyz"),
            Some(ReadinessRoute::Ready)
        );
        assert_eq!(
            classify_readiness(Some(HttpMethod::Post), "/readyz"),
            Some(ReadinessRoute::MethodNotAllowed)
        );
        for path in ["/", "/readyz/", "/v1/projects", "/metrics"] {
            assert_eq!(classify_readiness(Some(HttpMethod::Get), path), None);
        }
    }

    #[test]
    fn routes_by_exact_method_and_prefers_static_segments() {
        let mut router = HttpRouter::new();
        router
            .add_route(HttpMethod::Get, "/v1/projects/{project_id}", |request| {
                HttpResponse::json(
                    200,
                    &json!({"project": request.path_parameter("project_id")}),
                )
                .map_err(|_| internal_error(request.request_id()))
            })
            .expect("parameter route");
        router
            .add_route(HttpMethod::Get, "/v1/projects/current", |request| {
                HttpResponse::json(200, &json!({"project": "current"}))
                    .map_err(|_| internal_error(request.request_id()))
            })
            .expect("static route");

        let RouteResolution::Matched { parameters, .. } =
            router.resolve(Some(HttpMethod::Get), "/v1/projects/example")
        else {
            panic!("parameter route should match");
        };
        assert_eq!(parameters["project_id"], "example");
        let RouteResolution::Matched { parameters, .. } =
            router.resolve(Some(HttpMethod::Get), "/v1/projects/current")
        else {
            panic!("static route should match");
        };
        assert!(parameters.is_empty());
        assert!(matches!(
            router.resolve(Some(HttpMethod::Post), "/v1/projects/example"),
            RouteResolution::MethodNotAllowed(_)
        ));
        assert!(matches!(
            router.resolve(Some(HttpMethod::Get), "/v1/projects/example/"),
            RouteResolution::NotFound
        ));
    }

    #[test]
    fn rejects_ambiguous_invalid_and_readiness_patterns() {
        let mut router = HttpRouter::new();
        let handler = |request: HttpRequest| Err(internal_error(request.request_id()));
        router
            .add_route(HttpMethod::Get, "/v1/{left}/fixed", handler)
            .expect("first route");
        assert_eq!(
            router.add_route(HttpMethod::Get, "/v1/value/{right}", handler),
            Err(RouteRegistrationError::AmbiguousRoute)
        );
        assert_eq!(
            router.add_route(HttpMethod::Get, "/healthz", handler),
            Err(RouteRegistrationError::ReservedReadinessRoute)
        );
        assert_eq!(
            router.add_route(HttpMethod::Get, "/v1/{bad-name}", handler),
            Err(RouteRegistrationError::InvalidPattern)
        );
    }

    #[test]
    fn validates_and_generates_safe_request_identifiers() {
        assert!(valid_request_id("req_example00"));
        for invalid in ["example", "req_bad value", "req_", "req_bad/value"] {
            assert!(!valid_request_id(invalid));
        }
        let generated = generate_request_id();
        assert!(valid_request_id(&generated));
        assert_eq!(generated.len(), 28);
    }

    #[test]
    fn stable_errors_receive_transport_security_headers() {
        let request_id = "req_example00";
        let response = build_response(
            not_found(request_id).into_response(),
            request_id,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("response");
        assert_eq!(response.status_code(), StatusCode(404));
        for (name, expected) in [
            (REQUEST_ID_HEADER, request_id),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
            ("X-Frame-Options", "DENY"),
            ("Referrer-Policy", "no-referrer"),
        ] {
            assert!(
                response.headers().iter().any(|header| {
                    header.field.equiv(name) && header.value.as_str() == expected
                })
            );
        }
        let mut body = String::new();
        response
            .into_reader()
            .read_to_string(&mut body)
            .expect("body");
        let body: Value = serde_json::from_str(&body).expect("JSON error");
        assert_eq!(body["apiVersion"], "v1");
        assert_eq!(body["error"]["code"], "not_found");
        assert_eq!(body["error"]["requestId"], request_id);
    }

    #[test]
    fn body_limit_error_exposes_only_the_bound() {
        let error = request_body_too_large("req_example00", 4_096);
        assert_eq!(error.status, 413);
        assert_eq!(error.envelope.error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.envelope.error.details["limitBytes"],
            SafeDetail::Integer(4_096)
        );
    }

    #[test]
    fn streaming_body_is_bounded_ordered_and_shutdown_aware() {
        let (sender, mut body) = streaming_body(2).expect("stream");
        sender.try_send(b"first".to_vec()).expect("first chunk");
        sender.try_send(b"second".to_vec()).expect("second chunk");
        assert_eq!(
            sender.try_send(b"overflow".to_vec()),
            Err(StreamSendError::Full)
        );
        drop(sender);
        let mut output = Vec::new();
        body.read_to_end(&mut output).expect("stream read");
        assert_eq!(output, b"firstsecond");

        let (_sender, mut body) = streaming_body(1).expect("stream");
        let stopping = Arc::new(AtomicBool::new(true));
        body = body.with_shutdown(stopping);
        let mut byte = [0_u8; 1];
        assert_eq!(body.read(&mut byte).expect("shutdown read"), 0);
    }

    #[test]
    fn streaming_wire_response_flushes_each_bounded_chunk() {
        #[derive(Default)]
        struct RecordingWriter {
            bytes: Vec<u8>,
            flush_positions: Vec<usize>,
        }

        impl Write for RecordingWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                self.flush_positions.push(self.bytes.len());
                Ok(())
            }
        }

        let (sender, body) = streaming_body(2).expect("stream");
        sender.try_send(b"first".to_vec()).expect("first chunk");
        sender.try_send(b"second".to_vec()).expect("second chunk");
        drop(sender);
        let mut writer = RecordingWriter::default();
        write_streaming_response(
            &mut writer,
            HTTPVersion(1, 1),
            200,
            &response_headers(Vec::new(), "text/event-stream", "req_stream_test").expect("headers"),
            body,
        )
        .expect("wire response");

        assert_eq!(writer.flush_positions.len(), 4);
        assert!(
            writer
                .flush_positions
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        let wire = String::from_utf8(writer.bytes).expect("ASCII response");
        assert!(wire.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(wire.contains("Transfer-Encoding: chunked\r\n"));
        assert!(wire.contains("\r\n5\r\nfirst\r\n6\r\nsecond\r\n0\r\n\r\n"));
    }

    #[test]
    fn spawned_stream_producer_observes_disconnect_before_body_drop_returns() {
        let exited = Arc::new(AtomicBool::new(false));
        let producer_exited = Arc::clone(&exited);
        let body = spawn_streaming_body(1, move |sender| {
            while !sender.is_disconnected() {
                thread::sleep(Duration::from_millis(1));
            }
            producer_exited.store(true, Ordering::Relaxed);
        })
        .expect("spawned stream");
        drop(body);
        assert!(exited.load(Ordering::Relaxed));
    }

    #[test]
    fn prevents_handlers_from_overriding_security_headers() {
        assert!(
            HttpResponse::empty(204)
                .with_header("cache-control", "public")
                .is_err()
        );
        assert!(
            HttpResponse::empty(204)
                .with_header("Location", "/v1/resources/one")
                .is_ok()
        );
    }

    #[test]
    fn transport_config_rejects_public_or_unbounded_listeners() {
        let public = HttpTransportConfig::new(
            "0.0.0.0:8080".parse().expect("address"),
            "data-plane",
            "ready",
        );
        assert_eq!(public.validate(), Err(ServiceRuntimeError::NonLoopbackBind));

        let mut unbounded = HttpTransportConfig::new(
            "127.0.0.1:8080".parse().expect("address"),
            "data-plane",
            "ready",
        );
        unbounded.max_in_flight_requests = 0;
        assert_eq!(
            unbounded.validate(),
            Err(ServiceRuntimeError::InvalidConfiguration)
        );
    }
}
