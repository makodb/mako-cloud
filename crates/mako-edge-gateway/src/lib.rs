//! Public edge-function routing and authentication boundary.

#![forbid(unsafe_code)]

use std::{error::Error, fmt, num::NonZeroU64, pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures::Stream;
use mako_api::{ApiError, ApiErrorEnvelope, ErrorCode, RetryAdvice, SafeDetail, TenantScope};
use mako_edge_runtime_protocol::RuntimeErrorCode;
use mako_gateway::{
    AccessTokenVerificationError, GatewayAccessTokenVerifier, GatewayQuotaCharge,
    GatewayQuotaDecision, GatewayQuotaEngine, GatewayQuotaPolicySource, GatewayQuotaResource,
    VerifiedAccessIdentity,
};
use rand_core::{OsRng, RngCore};

const MAX_PROJECT_REF_BYTES: usize = 128;
const MAX_PATH_AND_QUERY_BYTES: usize = 8 * 1024;
const MAX_HEADER_COUNT: usize = 128;
const MAX_HEADER_NAME_BYTES: usize = 128;
const MAX_HEADER_VALUE_BYTES: usize = 16 * 1024;
const MAX_ACCESS_TOKEN_BYTES: usize = 16 * 1024;

/// Identifies this workspace component in startup diagnostics.
pub const COMPONENT: &str = "edge-gateway";

/// HTTP methods supported by the public function contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionHttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

impl FunctionHttpMethod {
    pub fn parse(value: &str) -> Result<Self, FunctionGatewayInputError> {
        match value {
            "GET" => Ok(Self::Get),
            "HEAD" => Ok(Self::Head),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "PATCH" => Ok(Self::Patch),
            "DELETE" => Ok(Self::Delete),
            "OPTIONS" => Ok(Self::Options),
            _ => Err(FunctionGatewayInputError::UnsupportedMethod),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
        }
    }
}

/// Parsed public route. Project references remain untrusted until resolved by
/// `FunctionRouteResolver`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StableFunctionPath {
    project_ref: String,
    function_name: String,
    function_path: String,
}

impl StableFunctionPath {
    #[must_use]
    pub fn project_ref(&self) -> &str {
        &self.project_ref
    }

    #[must_use]
    pub fn function_name(&self) -> &str {
        &self.function_name
    }

    #[must_use]
    pub fn function_path(&self) -> &str {
        &self.function_path
    }
}

/// Parses `/{projectRef}/functions/v1/{functionName}` and any function-owned
/// suffix without accepting traversal or ambiguous separators.
pub fn parse_stable_function_path(
    path: &str,
) -> Result<StableFunctionPath, FunctionGatewayInputError> {
    if path.len() > MAX_PATH_AND_QUERY_BYTES || path.contains(['?', '#', '\\']) {
        return Err(FunctionGatewayInputError::InvalidPath);
    }
    let segments = path
        .strip_prefix('/')
        .ok_or(FunctionGatewayInputError::InvalidPath)?
        .split('/')
        .collect::<Vec<_>>();
    if segments.len() < 4
        || segments[1] != "functions"
        || segments[2] != "v1"
        || !valid_project_ref(segments[0])
        || !valid_function_name(segments[3])
        || segments[4..]
            .iter()
            .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."))
    {
        return Err(FunctionGatewayInputError::InvalidPath);
    }
    let function_path = if segments.len() == 4 {
        "/".to_owned()
    } else {
        format!("/{}", segments[4..].join("/"))
    };
    Ok(StableFunctionPath {
        project_ref: segments[0].to_owned(),
        function_name: segments[3].to_owned(),
        function_path,
    })
}

/// HTTP request accepted from the public listener after TLS termination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionGatewayRequest {
    /// Validated ingress request identifier preserved through audit, private
    /// RPC, runtime invocation, and the public response.
    pub request_id: String,
    pub method: FunctionHttpMethod,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Trusted ingress ordering from nearest to farthest platform region.
    pub region_priority: Vec<String>,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionalDeploymentHealth {
    pub region: String,
    pub healthy: bool,
    pub valid_until_unix_seconds: u64,
}

/// Trusted route metadata loaded from the active control-plane snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedFunctionRoute {
    pub tenant: TenantScope,
    pub organization_id: String,
    pub function_name: String,
    pub active_version: u64,
    pub selected_regions: Vec<String>,
    pub regional_deployments: Vec<RegionalDeploymentHealth>,
    pub verify_jwt: bool,
    pub request_limit_bytes: u64,
    pub response_limit_bytes: u64,
    /// The verified custom domains the function's environment is served on;
    /// a request arriving on a custom domain is served only when its host
    /// is one of them. Empty for an environment with none.
    pub custom_domains: Vec<String>,
    /// The browser origins the function's environment answers cross-origin,
    /// exactly as the data plane holds them: the gateway answers a preflight
    /// and labels a response only for an origin on this list, wherever the
    /// request arrived. Empty for an environment that allows none.
    pub allowed_origins: Vec<String>,
}

impl ResolvedFunctionRoute {
    /// Whether `origin` may be answered cross-origin for this route: an
    /// exact match against the environment's installed list.
    #[must_use]
    pub fn allows_origin(&self, origin: &str) -> bool {
        self.allowed_origins.iter().any(|listed| listed == origin)
    }

    fn validate_for(
        &self,
        requested_name: &str,
        request_id: &str,
    ) -> Result<(), FunctionGatewayError> {
        if self.function_name != requested_name
            || !valid_function_name(&self.function_name)
            || self.active_version == 0
            || self.request_limit_bytes == 0
            || self.response_limit_bytes == 0
            || !valid_platform_id(&self.organization_id, "org_")
            || self.selected_regions.is_empty()
            || self.selected_regions.len() > 16
            || has_invalid_or_duplicate_regions(&self.selected_regions)
            || self.custom_domains.len() > MAX_CUSTOM_DOMAINS
            || self
                .custom_domains
                .iter()
                .any(|hostname| !valid_custom_domain(hostname))
            || self.allowed_origins.len() > MAX_ALLOWED_ORIGINS
            || self
                .allowed_origins
                .iter()
                .enumerate()
                .any(|(index, origin)| {
                    !valid_origin(origin) || self.allowed_origins[..index].contains(origin)
                })
            || self.regional_deployments.len() > 64
            || self
                .regional_deployments
                .iter()
                .any(|health| !valid_region(&health.region))
            || self
                .regional_deployments
                .iter()
                .enumerate()
                .any(|(index, health)| {
                    self.regional_deployments[..index]
                        .iter()
                        .any(|candidate| candidate.region == health.region)
                })
        {
            return Err(FunctionGatewayError::internal_with_request_id(
                request_id,
                "function route metadata is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionRouteError {
    Unavailable,
}

impl fmt::Display for FunctionRouteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("function routing state is unavailable")
    }
}

impl Error for FunctionRouteError {}

#[async_trait]
pub trait FunctionRouteResolver: Send + Sync {
    async fn resolve(
        &self,
        project_ref: &str,
        function_name: &str,
    ) -> Result<Option<ResolvedFunctionRoute>, FunctionRouteError>;
}

pub trait FunctionAccessTokenVerifier: Send + Sync {
    fn verify(
        &self,
        token: &str,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<VerifiedAccessIdentity, AccessTokenVerificationError>;
}

impl FunctionAccessTokenVerifier for GatewayAccessTokenVerifier<'_> {
    fn verify(
        &self,
        token: &str,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<VerifiedAccessIdentity, AccessTokenVerificationError> {
        GatewayAccessTokenVerifier::verify(self, token, tenant, now_unix_seconds)
    }
}

/// A caller credential whose debug representation can never reveal the token.
#[derive(Clone, Eq, PartialEq)]
pub struct SensitiveCallerToken(String);

impl SensitiveCallerToken {
    #[must_use]
    pub fn expose_to_runtime_adapter(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SensitiveCallerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveCallerToken([REDACTED])")
    }
}

/// A request addressed to an explicit immutable deployment. Active-version
/// lookup never occurs inside the runtime adapter.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeFunctionInvocation {
    pub tenant: TenantScope,
    pub function_name: String,
    pub version: u64,
    pub region: String,
    pub method: FunctionHttpMethod,
    pub path_and_query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub response_limit_bytes: u64,
    pub request_id: String,
    pub trace_id: String,
    pub caller: Option<VerifiedAccessIdentity>,
    pub caller_token: Option<SensitiveCallerToken>,
    pub audit_context: FunctionInvocationAuditContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FunctionInvocationActor {
    ApplicationUser {
        user_id: String,
        session_id: String,
    },
    PublicWebhook,
    /// The control plane's scheduler, authenticated on the internal hop.
    /// The schedule and run are named so the audit trail and the function
    /// can tell a scheduled invocation from any other.
    Schedule {
        schedule_id: String,
        run_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionInvocationAuditContext {
    pub tenant: TenantScope,
    pub organization_id: String,
    pub function_name: String,
    pub version: u64,
    pub region: String,
    pub regional_failover: bool,
    pub request_id: String,
    pub trace_id: String,
    pub actor: FunctionInvocationActor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionInvocationUsage {
    pub request_bytes: u64,
    pub public: bool,
    pub reservation_id: String,
    pub now_unix_milliseconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FunctionAdmissionDecision {
    Allowed,
    Throttled {
        resource: String,
        retry_after_milliseconds: u64,
    },
    HardLimit {
        resource: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionAdmissionError {
    Unavailable,
}

impl fmt::Display for FunctionAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("function admission state is unavailable")
    }
}

impl Error for FunctionAdmissionError {}

#[async_trait]
pub trait FunctionInvocationAdmission: Send + Sync {
    async fn check(
        &self,
        tenant: &TenantScope,
        function_name: &str,
        usage: FunctionInvocationUsage,
    ) -> Result<FunctionAdmissionDecision, FunctionAdmissionError>;
}

#[derive(Clone)]
pub struct GatewayFunctionInvocationAdmission {
    engine: Arc<GatewayQuotaEngine>,
    policies: Arc<dyn GatewayQuotaPolicySource>,
}

impl GatewayFunctionInvocationAdmission {
    #[must_use]
    pub fn new(
        engine: Arc<GatewayQuotaEngine>,
        policies: Arc<dyn GatewayQuotaPolicySource>,
    ) -> Self {
        Self { engine, policies }
    }
}

#[async_trait]
impl FunctionInvocationAdmission for GatewayFunctionInvocationAdmission {
    async fn check(
        &self,
        tenant: &TenantScope,
        _function_name: &str,
        usage: FunctionInvocationUsage,
    ) -> Result<FunctionAdmissionDecision, FunctionAdmissionError> {
        let policy = self
            .policies
            .policy_for(tenant)
            .await
            .map_err(|_| FunctionAdmissionError::Unavailable)?;
        let mut charges = vec![GatewayQuotaCharge {
            resource: GatewayQuotaResource::FunctionInvocations,
            amount: NonZeroU64::MIN,
        }];
        if usage.public {
            charges.push(GatewayQuotaCharge {
                resource: GatewayQuotaResource::PublicFunctionInvocations,
                amount: NonZeroU64::MIN,
            });
        }
        if let Some(amount) = NonZeroU64::new(usage.request_bytes) {
            charges.push(GatewayQuotaCharge {
                resource: GatewayQuotaResource::FunctionRequestBytes,
                amount,
            });
        }
        match self
            .engine
            .check_and_reserve(
                tenant,
                &usage.reservation_id,
                &charges,
                &policy,
                usage.now_unix_milliseconds,
            )
            .await
            .map_err(|_| FunctionAdmissionError::Unavailable)?
        {
            GatewayQuotaDecision::Allowed => Ok(FunctionAdmissionDecision::Allowed),
            GatewayQuotaDecision::Throttled {
                resource,
                retry_after_milliseconds,
            } => Ok(FunctionAdmissionDecision::Throttled {
                resource: resource.as_str().to_owned(),
                retry_after_milliseconds,
            }),
            GatewayQuotaDecision::HardLimit { resource } => {
                Ok(FunctionAdmissionDecision::HardLimit {
                    resource: resource.as_str().to_owned(),
                })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionInvocationAuditOutcome {
    Admitted,
    Throttled,
    QuotaRejected,
    RuntimeRejected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionInvocationAuditEvent {
    pub context: FunctionInvocationAuditContext,
    pub outcome: FunctionInvocationAuditOutcome,
}

pub trait FunctionInvocationAuditSink: Send + Sync {
    fn record(&self, event: FunctionInvocationAuditEvent);
}

/// One completed invocation, measured where the response actually arrived.
///
/// Separate from the audit event because admission is audited before the
/// runtime runs, and latency does not exist yet at that point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionMetricEvent {
    pub tenant: TenantScope,
    pub function_name: String,
    pub version: u64,
    pub region: String,
    pub response_status: u16,
    pub elapsed_milliseconds: u64,
}

pub trait FunctionMetricsSink: Send + Sync {
    fn record(&self, event: FunctionMetricEvent);
}

pub type RuntimeResponseStream =
    Pin<Box<dyn Stream<Item = Result<Vec<u8>, RuntimeInvocationError>> + Send>>;

/// Runtime response whose body is relayed chunk-by-chunk by the HTTP adapter.
pub struct RuntimeFunctionResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: RuntimeResponseStream,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeInvocationError {
    pub code: RuntimeErrorCode,
    pub retryable: bool,
}

impl fmt::Display for RuntimeInvocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("function runtime invocation failed")
    }
}

impl Error for RuntimeInvocationError {}

#[async_trait]
pub trait FunctionRuntimeInvoker: Send + Sync {
    async fn invoke(
        &self,
        request: RuntimeFunctionInvocation,
    ) -> Result<RuntimeFunctionResponse, RuntimeInvocationError>;
}

/// Response returned to the public HTTP adapter without buffering the runtime
/// response body.
pub struct FunctionGatewayResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: RuntimeResponseStream,
    pub request_id: String,
    /// The deployment version that served.
    pub version: u64,
}

impl fmt::Debug for FunctionGatewayResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionGatewayResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body", &"<stream>")
            .field("request_id", &self.request_id)
            .field("version", &self.version)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FunctionGateway;

/// A request that has been validated and routed but not yet admitted.
struct PreparedInvocation {
    request_id: String,
    trace_id: String,
    stable_path: StableFunctionPath,
    route: ResolvedFunctionRoute,
    selected_region: SelectedFunctionRegion,
}

/// Who is invoking, as the transport established it.
struct InvocationCaller {
    identity: Option<VerifiedAccessIdentity>,
    token: Option<String>,
    actor: FunctionInvocationActor,
    /// Charged against the public-invocation quota as well.
    public: bool,
}

impl FunctionGateway {
    /// The public path: the caller is whoever the bearer token proves, or
    /// nobody on a route that admits anonymous callers.
    #[allow(clippy::too_many_arguments)]
    pub async fn invoke(
        &self,
        mut request: FunctionGatewayRequest,
        routes: &dyn FunctionRouteResolver,
        tokens: &dyn FunctionAccessTokenVerifier,
        admission: &dyn FunctionInvocationAdmission,
        audit: &dyn FunctionInvocationAuditSink,
        metrics: &dyn FunctionMetricsSink,
        runtime: &dyn FunctionRuntimeInvoker,
    ) -> Result<FunctionGatewayResponse, FunctionGatewayError> {
        // The scheduler's headers say an invocation is a schedule's, and
        // `docs/scheduled-functions.md` tells a function it may act on that.
        // A public caller may therefore not send them: otherwise anyone could
        // claim to be a schedule, and the documented check would be advice to
        // trust a stranger. They are dropped rather than refused -- a request
        // carrying one is not necessarily hostile, and a function that reads
        // them correctly sees the truth either way.
        request
            .headers
            .retain(|(name, _)| !is_schedule_header(name));
        let prepared = Self::prepare(&request, routes).await?;
        let request_id = &prepared.request_id;
        let supplied_token = optional_bearer_token(&request.headers)
            .map_err(|_| FunctionGatewayError::unauthenticated(request_id))?;
        if prepared.route.verify_jwt && supplied_token.is_none() {
            return Err(FunctionGatewayError::unauthenticated(request_id));
        }
        let token = supplied_token.map(str::to_owned);
        let identity = token
            .as_deref()
            .map(|token| {
                tokens
                    .verify(token, &prepared.route.tenant, request.now_unix_seconds)
                    .map_err(|_| FunctionGatewayError::unauthenticated(request_id))
            })
            .transpose()?;
        let actor = identity
            .as_ref()
            .map_or(FunctionInvocationActor::PublicWebhook, |identity| {
                FunctionInvocationActor::ApplicationUser {
                    user_id: identity.user_id().as_str().to_owned(),
                    session_id: identity.session_id().as_str().to_owned(),
                }
            });
        let caller = InvocationCaller {
            public: identity.is_none(),
            identity,
            token,
            actor,
        };
        Self::execute(
            request, prepared, caller, admission, audit, metrics, runtime,
        )
        .await
    }

    /// Invokes on behalf of a caller the transport has already
    /// authenticated -- the control plane's scheduler on the internal hop --
    /// so no bearer token is required, and none may be supplied. Admission,
    /// audit, and metrics apply exactly as on the public path; the
    /// invocation is charged as a function invocation but not as a public
    /// one, since it did not come from the public listener.
    #[allow(clippy::too_many_arguments)]
    pub async fn invoke_as(
        &self,
        request: FunctionGatewayRequest,
        actor: FunctionInvocationActor,
        routes: &dyn FunctionRouteResolver,
        admission: &dyn FunctionInvocationAdmission,
        audit: &dyn FunctionInvocationAuditSink,
        metrics: &dyn FunctionMetricsSink,
        runtime: &dyn FunctionRuntimeInvoker,
    ) -> Result<FunctionGatewayResponse, FunctionGatewayError> {
        let prepared = Self::prepare(&request, routes).await?;
        if request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        {
            return Err(FunctionGatewayError::invalid_request(&prepared.request_id));
        }
        let caller = InvocationCaller {
            identity: None,
            token: None,
            actor,
            public: false,
        };
        Self::execute(
            request, prepared, caller, admission, audit, metrics, runtime,
        )
        .await
    }

    /// Everything that happens before anyone is admitted: request shape,
    /// the stable path, the route, the region, the size limit.
    async fn prepare(
        request: &FunctionGatewayRequest,
        routes: &dyn FunctionRouteResolver,
    ) -> Result<PreparedInvocation, FunctionGatewayError> {
        let request_id = request.request_id.clone();
        if request_id.is_empty()
            || request_id.len() > 128
            || request_id.chars().any(char::is_control)
        {
            return Err(FunctionGatewayError::invalid_request(&generate_request_id()));
        }
        let trace_id = trace_id(&request.headers);
        let stable_path = parse_stable_function_path(&request.path)
            .map_err(|_| FunctionGatewayError::invalid_request(&request_id))?;
        validate_request_shape(request)
            .map_err(|_| FunctionGatewayError::invalid_request(&request_id))?;
        let route = routes
            .resolve(stable_path.project_ref(), stable_path.function_name())
            .await
            .map_err(|_| FunctionGatewayError::unavailable(&request_id))?
            .ok_or_else(|| FunctionGatewayError::not_found(&request_id))?;
        route.validate_for(stable_path.function_name(), &request_id)?;
        let selected_region = select_nearest_healthy_region(
            &request.region_priority,
            &route.selected_regions,
            &route.regional_deployments,
            request.now_unix_seconds,
        )
        .map_err(|_| FunctionGatewayError::deployment_unavailable(&request_id))?;
        if u64::try_from(request.body.len()).unwrap_or(u64::MAX) > route.request_limit_bytes {
            return Err(FunctionGatewayError::request_too_large(
                &request_id,
                route.request_limit_bytes,
            ));
        }
        Ok(PreparedInvocation {
            request_id,
            trace_id,
            stable_path,
            route,
            selected_region,
        })
    }

    /// Admission, audit, the runtime call, and the metric, for a caller the
    /// transport has established.
    async fn execute(
        request: FunctionGatewayRequest,
        prepared: PreparedInvocation,
        caller: InvocationCaller,
        admission: &dyn FunctionInvocationAdmission,
        audit: &dyn FunctionInvocationAuditSink,
        metrics: &dyn FunctionMetricsSink,
        runtime: &dyn FunctionRuntimeInvoker,
    ) -> Result<FunctionGatewayResponse, FunctionGatewayError> {
        let PreparedInvocation {
            request_id,
            trace_id,
            stable_path,
            route,
            selected_region,
        } = prepared;
        let audit_context = FunctionInvocationAuditContext {
            tenant: route.tenant.clone(),
            organization_id: route.organization_id.clone(),
            function_name: route.function_name.clone(),
            version: route.active_version,
            region: selected_region.region.clone(),
            regional_failover: selected_region.failed_over,
            request_id: request_id.clone(),
            trace_id: trace_id.clone(),
            actor: caller.actor,
        };
        let usage = FunctionInvocationUsage {
            request_bytes: u64::try_from(request.body.len()).unwrap_or(u64::MAX),
            public: caller.public,
            reservation_id: request_id.clone(),
            now_unix_milliseconds: request
                .now_unix_seconds
                .checked_mul(1_000)
                .ok_or_else(|| FunctionGatewayError::invalid_request(&request_id))?,
        };
        match admission
            .check(&route.tenant, &route.function_name, usage)
            .await
            .map_err(|_| FunctionGatewayError::admission_unavailable(&request_id))?
        {
            FunctionAdmissionDecision::Allowed => {
                audit.record(FunctionInvocationAuditEvent {
                    context: audit_context.clone(),
                    outcome: FunctionInvocationAuditOutcome::Admitted,
                });
            }
            FunctionAdmissionDecision::Throttled {
                resource,
                retry_after_milliseconds,
            } => {
                audit.record(FunctionInvocationAuditEvent {
                    context: audit_context,
                    outcome: FunctionInvocationAuditOutcome::Throttled,
                });
                return Err(FunctionGatewayError::throttled(
                    &request_id,
                    resource,
                    retry_after_milliseconds,
                ));
            }
            FunctionAdmissionDecision::HardLimit { resource } => {
                audit.record(FunctionInvocationAuditEvent {
                    context: audit_context,
                    outcome: FunctionInvocationAuditOutcome::QuotaRejected,
                });
                return Err(FunctionGatewayError::quota_exceeded(&request_id, resource));
            }
        }
        let path_and_query = match request.query {
            Some(query) if !query.is_empty() => {
                if query.len() > MAX_PATH_AND_QUERY_BYTES || query.chars().any(char::is_control) {
                    return Err(FunctionGatewayError::invalid_request(&request_id));
                }
                format!("{}?{query}", stable_path.function_path())
            }
            _ => stable_path.function_path().to_owned(),
        };
        let invocation_started = std::time::Instant::now();
        let metric_tenant = route.tenant.clone();
        let metric_name = route.function_name.clone();
        let metric_version = route.active_version;
        let metric_region = selected_region.region.clone();
        let runtime_response = runtime
            .invoke(RuntimeFunctionInvocation {
                tenant: route.tenant,
                function_name: route.function_name,
                version: route.active_version,
                region: selected_region.region,
                method: request.method,
                path_and_query,
                headers: forwarded_headers(request.headers),
                body: request.body,
                response_limit_bytes: route.response_limit_bytes,
                request_id: request_id.clone(),
                trace_id,
                caller: caller.identity,
                caller_token: caller.token.map(SensitiveCallerToken),
                audit_context: audit_context.clone(),
            })
            .await
            .map_err(|error| {
                audit.record(FunctionInvocationAuditEvent {
                    context: audit_context,
                    outcome: FunctionInvocationAuditOutcome::RuntimeRejected,
                });
                FunctionGatewayError::runtime(&request_id, error)
            })?;
        metrics.record(FunctionMetricEvent {
            tenant: metric_tenant,
            function_name: metric_name,
            version: metric_version,
            region: metric_region,
            response_status: runtime_response.status,
            elapsed_milliseconds: u64::try_from(invocation_started.elapsed().as_millis())
                .unwrap_or(u64::MAX),
        });
        if !(200..=599).contains(&runtime_response.status)
            || !headers_are_valid(&runtime_response.headers)
        {
            return Err(FunctionGatewayError::internal_with_request_id(
                &request_id,
                "function runtime response is invalid",
            ));
        }
        Ok(FunctionGatewayResponse {
            status: runtime_response.status,
            headers: forwarded_headers(runtime_response.headers),
            body: runtime_response.body,
            request_id,
            version: metric_version,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionGatewayInputError {
    UnsupportedMethod,
    InvalidPath,
    InvalidHeaders,
}

impl fmt::Display for FunctionGatewayInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("function request is invalid")
    }
}

impl Error for FunctionGatewayInputError {}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionGatewayError {
    status: u16,
    api_error: Box<ApiErrorEnvelope>,
}

impl FunctionGatewayError {
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    #[must_use]
    pub fn api_error(&self) -> &ApiErrorEnvelope {
        &self.api_error
    }

    fn invalid_request(request_id: &str) -> Self {
        error(
            400,
            ErrorCode::InvalidRequest,
            "function request is invalid",
            request_id,
            RetryAdvice::Never,
        )
    }

    fn unauthenticated(request_id: &str) -> Self {
        error(
            401,
            ErrorCode::Unauthenticated,
            "application-user session is invalid",
            request_id,
            RetryAdvice::Never,
        )
    }

    fn not_found(request_id: &str) -> Self {
        error(
            404,
            ErrorCode::NotFound,
            "function was not found",
            request_id,
            RetryAdvice::Never,
        )
    }

    fn unavailable(request_id: &str) -> Self {
        error(
            503,
            ErrorCode::Unavailable,
            "function routing is unavailable",
            request_id,
            RetryAdvice::AfterDelay { after_ms: 1_000 },
        )
    }

    fn deployment_unavailable(request_id: &str) -> Self {
        error(
            503,
            ErrorCode::Unavailable,
            "function deployment is unavailable",
            request_id,
            RetryAdvice::AfterDelay { after_ms: 1_000 },
        )
    }

    fn request_too_large(request_id: &str, limit: u64) -> Self {
        let mut failure = error(
            413,
            ErrorCode::InvalidRequest,
            "function request exceeds its configured limit",
            request_id,
            RetryAdvice::Never,
        );
        let safe_limit = i64::try_from(limit).unwrap_or(i64::MAX);
        failure
            .api_error
            .error
            .details
            .insert("limitBytes".to_owned(), SafeDetail::Integer(safe_limit));
        failure
    }

    fn admission_unavailable(request_id: &str) -> Self {
        error(
            503,
            ErrorCode::Unavailable,
            "function admission is unavailable",
            request_id,
            RetryAdvice::AfterDelay { after_ms: 1_000 },
        )
    }

    fn throttled(request_id: &str, resource: String, retry_after_milliseconds: u64) -> Self {
        let mut failure = error(
            429,
            ErrorCode::RateLimited,
            "function invocation is throttled",
            request_id,
            RetryAdvice::AfterDelay {
                after_ms: retry_after_milliseconds,
            },
        );
        failure
            .api_error
            .error
            .details
            .insert("resource".to_owned(), SafeDetail::String(resource));
        failure
    }

    fn quota_exceeded(request_id: &str, resource: String) -> Self {
        let mut failure = error(
            429,
            ErrorCode::QuotaExceeded,
            "function quota is exhausted",
            request_id,
            RetryAdvice::Never,
        );
        failure
            .api_error
            .error
            .details
            .insert("resource".to_owned(), SafeDetail::String(resource));
        failure
    }

    fn runtime(request_id: &str, runtime: RuntimeInvocationError) -> Self {
        let (status, code, message) = match runtime.code {
            RuntimeErrorCode::DeploymentNotFound => (
                503,
                ErrorCode::Unavailable,
                "function deployment is unavailable",
            ),
            RuntimeErrorCode::ProtocolMismatch | RuntimeErrorCode::InvalidDeployment => (
                500,
                ErrorCode::Internal,
                "function runtime rejected the request",
            ),
            _ => (
                503,
                ErrorCode::Unavailable,
                "function runtime is unavailable",
            ),
        };
        error(
            status,
            code,
            message,
            request_id,
            if runtime.retryable {
                RetryAdvice::Immediate
            } else {
                RetryAdvice::Never
            },
        )
    }

    fn internal_with_request_id(request_id: &str, message: &'static str) -> Self {
        error(
            500,
            ErrorCode::Internal,
            message,
            request_id,
            RetryAdvice::Never,
        )
    }
}

impl fmt::Display for FunctionGatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.api_error.error.message)
    }
}

impl Error for FunctionGatewayError {}

fn error(
    status: u16,
    code: ErrorCode,
    message: &'static str,
    request_id: &str,
    retry: RetryAdvice,
) -> FunctionGatewayError {
    FunctionGatewayError {
        status,
        api_error: Box::new(ApiErrorEnvelope::new(ApiError::new(
            code, message, request_id, retry,
        ))),
    }
}

fn validate_request_shape(
    request: &FunctionGatewayRequest,
) -> Result<(), FunctionGatewayInputError> {
    if !headers_are_valid(&request.headers) {
        return Err(FunctionGatewayInputError::InvalidHeaders);
    }
    Ok(())
}

fn headers_are_valid(headers: &[(String, String)]) -> bool {
    headers.len() <= MAX_HEADER_COUNT
        && headers.iter().all(|(name, value)| {
            !name.is_empty()
                && name.len() <= MAX_HEADER_NAME_BYTES
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                && value.len() <= MAX_HEADER_VALUE_BYTES
                && !value.contains(['\r', '\n', '\0'])
        })
}

/// What the runtime is given of the request's own headers.
///
/// `authorization` is not among them. The caller's credential reaches the
/// worker as `caller_token`, which the adapter presents as the sensitive
/// `x-mako-caller-authorization` header only after this gateway has verified
/// it -- so a function reads an identity the platform stands behind, and a
/// public function that was called with a bearer token nobody verified reads
/// no identity at all rather than an unchecked one.
fn forwarded_headers(headers: Vec<(String, String)>) -> Vec<(String, String)> {
    headers
        .into_iter()
        .filter(|(name, _)| {
            !is_hop_by_hop(name)
                && !is_internal_header(name)
                && !name.eq_ignore_ascii_case("authorization")
        })
        .collect()
}

/// What the scheduler alone may say. Set on the internal hop by
/// `invoke_as`, stripped from every public request by `invoke`.
fn is_schedule_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "x-mako-schedule-id" | "x-mako-schedule-run-id" | "x-mako-schedule-due-at"
    )
}

fn is_internal_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "x-mako-caller-authorization"
            | "x-mako-request-id"
            | "x-mako-runtime-protocol"
            | "x-mako-trace-id"
    )
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn optional_bearer_token(
    headers: &[(String, String)],
) -> Result<Option<&str>, FunctionGatewayInputError> {
    let mut values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_str());
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(FunctionGatewayInputError::InvalidHeaders);
    }
    let token = value
        .strip_prefix("Bearer ")
        .ok_or(FunctionGatewayInputError::InvalidHeaders)?;
    if token.is_empty()
        || token.len() > MAX_ACCESS_TOKEN_BYTES
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(FunctionGatewayInputError::InvalidHeaders);
    }
    Ok(Some(token))
}

/// A route may name at most this many custom domains.
const MAX_CUSTOM_DOMAINS: usize = 64;

/// A lowercase DNS name of at least two labels, as the control plane
/// stores verified custom domains.
fn valid_custom_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.contains('.')
        && value
            .split('.')
            .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
}

/// The most origins one environment allows; the public contract's bound.
const MAX_ALLOWED_ORIGINS: usize = 16;
const MAX_ORIGIN_BYTES: usize = 262;

/// An origin in the exact form a browser sends: `http://` or `https://`, a
/// lowercase host of letters, digits, dots, and hyphens, an optional port,
/// and nothing else. Trusted metadata is still checked: an origin the
/// gateway could not match byte for byte must never become an echoed header.
fn valid_origin(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_ORIGIN_BYTES {
        return false;
    }
    let Some(authority) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    port.is_none_or(|port| {
        !port.is_empty()
            && port.len() <= 5
            && port.bytes().all(|byte| byte.is_ascii_digit())
            && port
                .parse::<u32>()
                .is_ok_and(|port| (1..=65_535).contains(&port))
    }) && !host.is_empty()
        && host.len() <= 253
        && host
            .split('.')
            .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'))
        && host.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
}

fn valid_project_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROJECT_REF_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_platform_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && (8..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedFunctionRegion {
    pub region: String,
    pub failed_over: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionalSelectionError {
    InvalidPriority,
    NoHealthySelectedRegion,
}

impl fmt::Display for RegionalSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPriority => "regional priority is invalid",
            Self::NoHealthySelectedRegion => "no selected region is healthy",
        })
    }
}

impl Error for RegionalSelectionError {}

/// Selects the first fresh, healthy deployment in the trusted ingress
/// proximity ordering. Health entries outside `selected_regions` are ignored.
pub fn select_nearest_healthy_region(
    region_priority: &[String],
    selected_regions: &[String],
    deployments: &[RegionalDeploymentHealth],
    now_unix_seconds: u64,
) -> Result<SelectedFunctionRegion, RegionalSelectionError> {
    if region_priority.is_empty()
        || region_priority.len() > 64
        || has_invalid_or_duplicate_regions(region_priority)
    {
        return Err(RegionalSelectionError::InvalidPriority);
    }
    let mut skipped_selected_region = false;
    for region in region_priority {
        if !selected_regions.contains(region) {
            continue;
        }
        let healthy = deployments.iter().any(|deployment| {
            deployment.region == *region
                && deployment.healthy
                && now_unix_seconds <= deployment.valid_until_unix_seconds
        });
        if healthy {
            return Ok(SelectedFunctionRegion {
                region: region.clone(),
                failed_over: skipped_selected_region,
            });
        }
        skipped_selected_region = true;
    }
    Err(RegionalSelectionError::NoHealthySelectedRegion)
}

fn has_invalid_or_duplicate_regions(regions: &[String]) -> bool {
    regions.iter().enumerate().any(|(index, region)| {
        !valid_region(region) || regions[..index].iter().any(|candidate| candidate == region)
    })
}

fn valid_region(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn generate_request_id() -> String {
    let mut random = [0_u8; 12];
    OsRng.fill_bytes(&mut random);
    let mut request_id = String::from("req_");
    for byte in random {
        use fmt::Write;
        write!(&mut request_id, "{byte:02x}").expect("writing to a string cannot fail");
    }
    request_id
}

fn trace_id(headers: &[(String, String)]) -> String {
    let propagated = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("traceparent"))
        .and_then(|(_, value)| parse_traceparent(value));
    propagated.map_or_else(generate_trace_id, |value| format!("trc_{value}"))
}

fn parse_traceparent(value: &str) -> Option<&str> {
    let mut parts = value.split('-');
    let version = parts.next()?;
    let trace = parts.next()?;
    let parent = parts.next()?;
    let flags = parts.next()?;
    if parts.next().is_none()
        && version.len() == 2
        && trace.len() == 32
        && trace != "00000000000000000000000000000000"
        && parent.len() == 16
        && parent != "0000000000000000"
        && flags.len() == 2
        && [version, trace, parent, flags].into_iter().all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    {
        Some(trace)
    } else {
        None
    }
}

fn generate_trace_id() -> String {
    let mut random = [0_u8; 16];
    OsRng.fill_bytes(&mut random);
    let mut trace_id = String::from("trc_");
    for byte in random {
        use fmt::Write;
        write!(&mut trace_id, "{byte:02x}").expect("writing to a string cannot fail");
    }
    trace_id
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::{StreamExt, stream};
    use mako_api::{EnvironmentId, ProjectId};
    use mako_gateway::{
        AccessTokenVerificationConfig, GatewayAuthorizationEpochResolver, GatewaySessionResolver,
    };
    use mako_identity::{
        AccessAuthorizationEpochs, AccessTokenConfig, AccessTokenInput, AccessTokenIssuer,
        AppUserId, KeyEncryptionKey, ProjectSigningKeyRing, SessionId,
    };
    use serde_json::Map;

    use super::*;

    struct Routes {
        route: ResolvedFunctionRoute,
    }

    #[async_trait]
    impl FunctionRouteResolver for Routes {
        async fn resolve(
            &self,
            project_ref: &str,
            function_name: &str,
        ) -> Result<Option<ResolvedFunctionRoute>, FunctionRouteError> {
            Ok(
                (project_ref == "demo" && function_name == self.route.function_name)
                    .then(|| self.route.clone()),
            )
        }
    }

    struct AccessState;

    impl GatewaySessionResolver for AccessState {
        fn is_active(&self, _: &TenantScope, _: &AppUserId, _: &SessionId, _: u64) -> bool {
            true
        }
    }

    impl GatewayAuthorizationEpochResolver for AccessState {
        fn current_epochs(
            &self,
            _: &TenantScope,
            _: &AppUserId,
        ) -> Option<AccessAuthorizationEpochs> {
            Some(AccessAuthorizationEpochs {
                environment: 2,
                user: 4,
            })
        }
    }

    struct Admission(FunctionAdmissionDecision);

    #[async_trait]
    impl FunctionInvocationAdmission for Admission {
        async fn check(
            &self,
            _: &TenantScope,
            _: &str,
            _: FunctionInvocationUsage,
        ) -> Result<FunctionAdmissionDecision, FunctionAdmissionError> {
            Ok(self.0.clone())
        }
    }

    #[derive(Default)]
    struct Metrics(std::sync::Mutex<Vec<FunctionMetricEvent>>);

    impl FunctionMetricsSink for Metrics {
        fn record(&self, event: FunctionMetricEvent) {
            self.0.lock().expect("metrics").push(event);
        }
    }

    #[derive(Default)]
    struct Audit {
        events: Mutex<Vec<FunctionInvocationAuditEvent>>,
    }

    impl FunctionInvocationAuditSink for Audit {
        fn record(&self, event: FunctionInvocationAuditEvent) {
            self.events.lock().expect("audit events").push(event);
        }
    }

    #[derive(Default)]
    struct Runtime {
        invocations: Mutex<Vec<RuntimeFunctionInvocation>>,
    }

    #[async_trait]
    impl FunctionRuntimeInvoker for Runtime {
        async fn invoke(
            &self,
            request: RuntimeFunctionInvocation,
        ) -> Result<RuntimeFunctionResponse, RuntimeInvocationError> {
            self.invocations.lock().expect("invocations").push(request);
            Ok(RuntimeFunctionResponse {
                status: 201,
                headers: vec![
                    ("content-type".to_owned(), "text/plain".to_owned()),
                    ("connection".to_owned(), "close".to_owned()),
                ],
                body: Box::pin(stream::iter([
                    Ok(b"stream-".to_vec()),
                    Ok(b"response".to_vec()),
                ])),
            })
        }
    }

    #[test]
    fn stable_paths_and_standard_methods_are_strict() {
        let parsed = parse_stable_function_path("/demo/functions/v1/hello-world/orders/42")
            .expect("stable path");
        assert_eq!(parsed.project_ref(), "demo");
        assert_eq!(parsed.function_name(), "hello-world");
        assert_eq!(parsed.function_path(), "/orders/42");
        for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
            assert_eq!(
                FunctionHttpMethod::parse(method).expect("method").as_str(),
                method
            );
        }
        assert!(FunctionHttpMethod::parse("TRACE").is_err());
        assert!(parse_stable_function_path("/demo/functions/v1/hello/../secret").is_err());
    }

    #[test]
    fn a_completed_invocation_is_measured_where_its_response_arrived() {
        futures::executor::block_on(async {
            let tenant_scope = tenant("prj_abcdefgh");
            let (token, jwks) = access_token(&tenant_scope);
            let state = AccessState;
            let verifier = GatewayAccessTokenVerifier::new(
                &jwks,
                &state,
                &state,
                AccessTokenVerificationConfig::new("https://issuer.test", "mako-functions", 30)
                    .expect("config"),
            );
            let metrics = Metrics::default();
            FunctionGateway
                .invoke(
                    request(&token, b"request".to_vec()),
                    &routes(tenant_scope.clone(), 1024),
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &Audit::default(),
                    &metrics,
                    &Runtime::default(),
                )
                .await
                .expect("invocation");
            let events = metrics.0.lock().expect("metrics");
            assert_eq!(events.len(), 1, "one invocation is one metric event");
            let event = &events[0];
            assert_eq!(event.tenant, tenant_scope);
            assert_eq!(event.response_status, 201);
            assert_eq!(event.version, 7, "the metric names the version that served");
        });
    }

    #[test]
    fn protected_request_routes_explicit_version_and_preserves_streaming() {
        futures::executor::block_on(async {
            let tenant_scope = tenant("prj_abcdefgh");
            let (token, jwks) = access_token(&tenant_scope);
            let state = AccessState;
            let verifier = GatewayAccessTokenVerifier::new(
                &jwks,
                &state,
                &state,
                AccessTokenVerificationConfig::new("https://issuer.test", "mako-functions", 30)
                    .expect("config"),
            );
            let runtime = Runtime::default();
            let audit = Audit::default();
            let response = FunctionGateway
                .invoke(
                    request(&token, b"request".to_vec()),
                    &routes(tenant_scope.clone(), 1024),
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &Metrics::default(),
                    &runtime,
                )
                .await
                .expect("invocation");
            assert_eq!(response.status, 201);
            assert_eq!(
                response.headers,
                vec![("content-type".to_owned(), "text/plain".to_owned())]
            );
            assert!(response.request_id.starts_with("req_"));
            let chunks = response
                .body
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("stream");
            assert_eq!(chunks, vec![b"stream-".to_vec(), b"response".to_vec()]);

            let invocations = runtime.invocations.lock().expect("invocations");
            let invocation = invocations.first().expect("invocation");
            assert_eq!(invocation.tenant, tenant_scope);
            assert_eq!(invocation.version, 7);
            assert_eq!(invocation.region, "iad");
            assert_eq!(invocation.trace_id, "trc_4bf92f3577b34da6a3ce929d0e0e4736");
            assert_eq!(invocation.path_and_query, "/orders/42?expand=true");
            assert_eq!(
                invocation
                    .caller
                    .as_ref()
                    .expect("authenticated caller")
                    .user_id()
                    .as_str(),
                "usr_abcdefgh"
            );
            assert_eq!(
                invocation
                    .caller_token
                    .as_ref()
                    .expect("caller token")
                    .expose_to_runtime_adapter(),
                token
            );
            assert!(!format!("{:?}", invocation.caller_token).contains(&token));
            // The credential travels as the caller token and only as the
            // caller token: a worker that read `authorization` would be
            // reading whatever the client sent, verified or not.
            assert!(
                !invocation
                    .headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("authorization")),
                "the raw authorization header must not reach the runtime"
            );
            assert!(
                invocation
                    .headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("traceparent")),
                "the request's own headers still reach the runtime"
            );
            assert!(matches!(
                invocation.audit_context.actor,
                FunctionInvocationActor::ApplicationUser { .. }
            ));
            assert_eq!(invocation.audit_context.region, "iad");
            assert_eq!(invocation.audit_context.trace_id, invocation.trace_id);
            assert!(!invocation.audit_context.regional_failover);
            assert_eq!(audit.events.lock().expect("events").len(), 1);
        });
    }

    #[test]
    fn jwt_and_request_limit_fail_before_user_code() {
        futures::executor::block_on(async {
            let tenant_scope = tenant("prj_abcdefgh");
            let (token, jwks) = access_token(&tenant_scope);
            let state = AccessState;
            let verifier = GatewayAccessTokenVerifier::new(
                &jwks,
                &state,
                &state,
                AccessTokenVerificationConfig::new("https://issuer.test", "mako-functions", 30)
                    .expect("config"),
            );
            let runtime = Runtime::default();
            let audit = Audit::default();
            let missing = FunctionGateway
                .invoke(
                    request("", vec![]),
                    &routes(tenant_scope.clone(), 4),
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &Metrics::default(),
                    &runtime,
                )
                .await
                .expect_err("JWT required");
            assert_eq!(missing.status(), 401);
            assert_eq!(missing.api_error().error.code, ErrorCode::Unauthenticated);

            let oversized = FunctionGateway
                .invoke(
                    request(&token, vec![0; 5]),
                    &routes(tenant_scope, 4),
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &Metrics::default(),
                    &runtime,
                )
                .await
                .expect_err("request limit");
            assert_eq!(oversized.status(), 413);

            let other_tenant = tenant("prj_ijklmnop");
            let cross_project = FunctionGateway
                .invoke(
                    request(&token, vec![]),
                    &routes(other_tenant, 4),
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &Metrics::default(),
                    &runtime,
                )
                .await
                .expect_err("cross-project token");
            assert_eq!(cross_project.status(), 401);
            assert!(runtime.invocations.lock().expect("invocations").is_empty());
        });
    }

    #[test]
    fn explicit_public_route_keeps_admission_and_audit_context() {
        futures::executor::block_on(async {
            let tenant_scope = tenant("prj_abcdefgh");
            let (_, jwks) = access_token(&tenant_scope);
            let state = AccessState;
            let verifier = GatewayAccessTokenVerifier::new(
                &jwks,
                &state,
                &state,
                AccessTokenVerificationConfig::new("https://issuer.test", "mako-functions", 30)
                    .expect("config"),
            );
            let mut public_route = routes(tenant_scope, 1024);
            public_route.route.verify_jwt = false;
            let audit = Audit::default();
            let runtime = Runtime::default();
            let response = FunctionGateway
                .invoke(
                    request("", b"webhook".to_vec()),
                    &public_route,
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &Metrics::default(),
                    &runtime,
                )
                .await
                .expect("public invocation");
            assert_eq!(response.status, 201);
            {
                let invocations = runtime.invocations.lock().expect("invocations");
                let invocation = invocations.first().expect("invocation");
                assert!(invocation.caller.is_none());
                assert!(invocation.caller_token.is_none());
                assert_eq!(
                    invocation.audit_context.actor,
                    FunctionInvocationActor::PublicWebhook
                );
            }
            assert_eq!(
                audit.events.lock().expect("events")[0].outcome,
                FunctionInvocationAuditOutcome::Admitted
            );

            let throttled_runtime = Runtime::default();
            let throttled = FunctionGateway
                .invoke(
                    request("", vec![]),
                    &public_route,
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Throttled {
                        resource: "public_function_requests".to_owned(),
                        retry_after_milliseconds: 2_000,
                    }),
                    &audit,
                    &Metrics::default(),
                    &throttled_runtime,
                )
                .await
                .expect_err("rate limit");
            assert_eq!(throttled.status(), 429);
            assert_eq!(throttled.api_error().error.code, ErrorCode::RateLimited);
            assert_eq!(
                throttled.api_error().error.retry,
                RetryAdvice::AfterDelay { after_ms: 2_000 }
            );
            assert!(
                throttled_runtime
                    .invocations
                    .lock()
                    .expect("invocations")
                    .is_empty()
            );

            let hard_limit_runtime = Runtime::default();
            let hard_limit = FunctionGateway
                .invoke(
                    request("", vec![]),
                    &public_route,
                    &verifier,
                    &Admission(FunctionAdmissionDecision::HardLimit {
                        resource: "function_invocations".to_owned(),
                    }),
                    &audit,
                    &Metrics::default(),
                    &hard_limit_runtime,
                )
                .await
                .expect_err("hard quota");
            assert_eq!(hard_limit.status(), 429);
            assert_eq!(hard_limit.api_error().error.code, ErrorCode::QuotaExceeded);
            assert_eq!(hard_limit.api_error().error.retry, RetryAdvice::Never);
            assert!(
                hard_limit_runtime
                    .invocations
                    .lock()
                    .expect("invocations")
                    .is_empty()
            );
        });
    }

    /// `docs/scheduled-functions.md` tells a function that must behave
    /// differently when scheduled to check `x-mako-schedule-id`. That advice
    /// is only sound if a public caller cannot send one, so the public path
    /// drops the scheduler's headers before the function ever sees them.
    #[test]
    fn a_public_caller_cannot_claim_to_be_a_schedule() {
        futures::executor::block_on(async {
            let tenant_scope = tenant("prj_abcdefgh");
            let (token, jwks) = access_token(&tenant_scope);
            let state = AccessState;
            let verifier = GatewayAccessTokenVerifier::new(
                &jwks,
                &state,
                &state,
                AccessTokenVerificationConfig::new("https://issuer.test", "mako-functions", 30)
                    .expect("config"),
            );
            let runtime = Runtime::default();
            let mut forged = request(&token, b"request".to_vec());
            for name in [
                "x-mako-schedule-id",
                "X-Mako-Schedule-Run-Id",
                "x-mako-schedule-due-at",
            ] {
                forged.headers.push((name.to_owned(), "forged".to_owned()));
            }
            FunctionGateway
                .invoke(
                    forged,
                    &routes(tenant_scope, 1024),
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &Audit::default(),
                    &Metrics::default(),
                    &runtime,
                )
                .await
                .expect("invocation");
            let invocations = runtime.invocations.lock().expect("invocations");
            let invocation = invocations.first().expect("invocation");
            assert!(
                !invocation
                    .headers
                    .iter()
                    .any(|(name, _)| name.to_ascii_lowercase().starts_with("x-mako-schedule")),
                "a public caller's schedule headers must not reach the function"
            );
            assert_eq!(
                invocation.audit_context.actor,
                FunctionInvocationActor::ApplicationUser {
                    user_id: invocation
                        .caller
                        .as_ref()
                        .expect("caller")
                        .user_id()
                        .as_str()
                        .to_owned(),
                    session_id: invocation
                        .caller
                        .as_ref()
                        .expect("caller")
                        .session_id()
                        .as_str()
                        .to_owned(),
                },
                "and the actor is still the application user it actually was"
            );
        });
    }

    /// The scheduler's hop is authenticated by the transport, not by a
    /// bearer token, so a scheduled invocation reaches a JWT-protected
    /// function without one -- but admission, audit, and metrics apply to it
    /// exactly as to any other invocation.
    #[test]
    fn a_scheduled_invocation_carries_its_actor_and_is_still_admitted() {
        futures::executor::block_on(async {
            let tenant_scope = tenant("prj_abcdefgh");
            let protected = routes(tenant_scope.clone(), 1024);
            assert!(protected.route.verify_jwt);
            let actor = FunctionInvocationActor::Schedule {
                schedule_id: "sch_abcdefghijklmnop".to_owned(),
                run_id: "run_abcdefghijklmnop".to_owned(),
            };
            let audit = Audit::default();
            let metrics = Metrics::default();
            let runtime = Runtime::default();
            let mut scheduled = request("", b"{\"day\":\"today\"}".to_vec());
            scheduled.headers.push((
                "x-mako-schedule-id".to_owned(),
                "sch_abcdefghijklmnop".to_owned(),
            ));
            let response = FunctionGateway
                .invoke_as(
                    scheduled.clone(),
                    actor.clone(),
                    &protected,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &metrics,
                    &runtime,
                )
                .await
                .expect("scheduled invocation");
            assert_eq!(response.status, 201);
            assert_eq!(
                response.version, 7,
                "the response names the version that served"
            );
            {
                let invocations = runtime.invocations.lock().expect("invocations");
                let invocation = invocations.first().expect("invocation");
                assert!(invocation.caller.is_none());
                assert!(invocation.caller_token.is_none());
                assert_eq!(invocation.audit_context.actor, actor);
                assert_eq!(invocation.path_and_query, "/orders/42?expand=true");
                assert!(
                    invocation
                        .headers
                        .iter()
                        .any(|(name, value)| name == "x-mako-schedule-id"
                            && value == "sch_abcdefghijklmnop"),
                    "the schedule headers reach the function"
                );
            }
            {
                let events = audit.events.lock().expect("events");
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].outcome, FunctionInvocationAuditOutcome::Admitted);
                assert_eq!(events[0].context.actor, actor);
            }
            assert_eq!(metrics.0.lock().expect("metrics").len(), 1);

            // A bearer token has no place on this hop.
            let (token, _) = access_token(&tenant_scope);
            let with_token = FunctionGateway
                .invoke_as(
                    request(&token, vec![]),
                    actor.clone(),
                    &protected,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &metrics,
                    &runtime,
                )
                .await
                .expect_err("no bearer token on the scheduler hop");
            assert_eq!(with_token.status(), 400);

            // Admission still refuses: a throttled tenant's schedule does not run.
            let throttled_runtime = Runtime::default();
            let throttled = FunctionGateway
                .invoke_as(
                    scheduled.clone(),
                    actor.clone(),
                    &protected,
                    &Admission(FunctionAdmissionDecision::Throttled {
                        resource: "function_invocations".to_owned(),
                        retry_after_milliseconds: 2_000,
                    }),
                    &audit,
                    &metrics,
                    &throttled_runtime,
                )
                .await
                .expect_err("throttled");
            assert_eq!(throttled.status(), 429);
            assert!(
                throttled_runtime
                    .invocations
                    .lock()
                    .expect("invocations")
                    .is_empty()
            );
            assert_eq!(
                audit
                    .events
                    .lock()
                    .expect("events")
                    .last()
                    .map(|event| event.outcome),
                Some(FunctionInvocationAuditOutcome::Throttled)
            );

            // The request limit is enforced before anything runs.
            let oversized = FunctionGateway
                .invoke_as(
                    scheduled,
                    actor,
                    &routes(tenant_scope, 4),
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &metrics,
                    &Runtime::default(),
                )
                .await
                .expect_err("request limit");
            assert_eq!(oversized.status(), 413);
        });
    }

    #[test]
    fn nearest_healthy_selected_region_fails_over_without_using_unauthorized_health() {
        futures::executor::block_on(async {
            let tenant_scope = tenant("prj_abcdefgh");
            let (token, jwks) = access_token(&tenant_scope);
            let state = AccessState;
            let verifier = GatewayAccessTokenVerifier::new(
                &jwks,
                &state,
                &state,
                AccessTokenVerificationConfig::new("https://issuer.test", "mako-functions", 30)
                    .expect("config"),
            );
            let mut failover_route = routes(tenant_scope, 1024);
            failover_route.route.regional_deployments = vec![
                RegionalDeploymentHealth {
                    region: "iad".to_owned(),
                    healthy: false,
                    valid_until_unix_seconds: 200,
                },
                RegionalDeploymentHealth {
                    region: "sfo".to_owned(),
                    healthy: true,
                    valid_until_unix_seconds: 200,
                },
                RegionalDeploymentHealth {
                    region: "fra".to_owned(),
                    healthy: true,
                    valid_until_unix_seconds: 200,
                },
            ];
            let audit = Audit::default();
            let runtime = Runtime::default();
            let mut gateway_request = request(&token, vec![]);
            gateway_request.region_priority =
                vec!["iad".to_owned(), "sfo".to_owned(), "fra".to_owned()];
            FunctionGateway
                .invoke(
                    gateway_request.clone(),
                    &failover_route,
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &Metrics::default(),
                    &runtime,
                )
                .await
                .expect("authorized failover");
            {
                let invocations = runtime.invocations.lock().expect("invocations");
                assert_eq!(invocations[0].region, "fra");
                assert!(invocations[0].audit_context.regional_failover);
            }

            failover_route.route.regional_deployments[2].healthy = false;
            let rejected_runtime = Runtime::default();
            let error = FunctionGateway
                .invoke(
                    gateway_request,
                    &failover_route,
                    &verifier,
                    &Admission(FunctionAdmissionDecision::Allowed),
                    &audit,
                    &Metrics::default(),
                    &rejected_runtime,
                )
                .await
                .expect_err("unauthorized healthy region cannot be used");
            assert_eq!(error.status(), 503);
            assert!(
                rejected_runtime
                    .invocations
                    .lock()
                    .expect("invocations")
                    .is_empty()
            );
        });
    }

    /// The verified custom domains a route carries are trusted metadata
    /// from the control plane, checked to the shape it stores them in.
    #[test]
    fn route_metadata_refuses_malformed_custom_domains() {
        let mut route = routes(tenant("prj_example00"), 1_024).route;
        route
            .validate_for("hello-world", "req_1")
            .expect("no custom domains");
        route.custom_domains = vec!["api.example.com".to_owned(), "a-1.b2.example".to_owned()];
        route
            .validate_for("hello-world", "req_1")
            .expect("lowercase DNS names");
        for invalid in [
            "",
            "API.example.com",
            "example",
            "-api.example.com",
            "api-.example.com",
            "api_1.example.com",
            "api..example.com",
            "api.example.com/",
            "api.example.com:443",
        ] {
            route.custom_domains = vec![invalid.to_owned()];
            assert!(
                route.validate_for("hello-world", "req_1").is_err(),
                "{invalid:?} must be refused"
            );
        }
        route.custom_domains = (0..=MAX_CUSTOM_DOMAINS)
            .map(|index| format!("h{index}.example.com"))
            .collect();
        assert!(
            route.validate_for("hello-world", "req_1").is_err(),
            "the list is bounded"
        );
    }

    /// The environment's allowlist is trusted metadata too: it carries only
    /// exact, unique, bounded origins, and matches byte for byte.
    #[test]
    fn route_metadata_refuses_malformed_allowed_origins() {
        let mut route = routes(tenant("prj_example00"), 1_024).route;
        route.allowed_origins = vec![
            "https://app.example.com".to_owned(),
            "http://127.0.0.1:5173".to_owned(),
        ];
        route
            .validate_for("hello-world", "req_1")
            .expect("exact origins");
        assert!(route.allows_origin("https://app.example.com"));
        assert!(route.allows_origin("http://127.0.0.1:5173"));
        assert!(!route.allows_origin("https://other.example.com"));
        assert!(!route.allows_origin("https://APP.example.com"));
        for malformed in [
            vec!["app.example.com".to_owned()],
            vec!["https://App.example.com".to_owned()],
            vec!["https://app.example.com/".to_owned()],
            vec!["https://app.example.com:0".to_owned()],
            vec!["https://*.example.com".to_owned()],
            vec![
                "https://app.example.com".to_owned(),
                "https://app.example.com".to_owned(),
            ],
            (0..=MAX_ALLOWED_ORIGINS)
                .map(|index| format!("https://app{index}.example.com"))
                .collect(),
        ] {
            route.allowed_origins = malformed.clone();
            assert!(
                route.validate_for("hello-world", "req_1").is_err(),
                "{malformed:?} must be refused"
            );
        }
        route.allowed_origins = Vec::new();
        route
            .validate_for("hello-world", "req_1")
            .expect("an environment may allow none");
        assert!(!route.allows_origin("https://app.example.com"));
    }

    fn request(token: &str, body: Vec<u8>) -> FunctionGatewayRequest {
        FunctionGatewayRequest {
            request_id: "req_testgateway0001".to_owned(),
            method: FunctionHttpMethod::Post,
            path: "/demo/functions/v1/hello-world/orders/42".to_owned(),
            query: Some("expand=true".to_owned()),
            headers: [
                (!token.is_empty())
                    .then(|| ("authorization".to_owned(), format!("Bearer {token}"))),
                Some((
                    "traceparent".to_owned(),
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned(),
                )),
            ]
            .into_iter()
            .flatten()
            .collect(),
            body,
            region_priority: vec!["iad".to_owned(), "fra".to_owned()],
            now_unix_seconds: 101,
        }
    }

    fn routes(tenant: TenantScope, request_limit_bytes: u64) -> Routes {
        Routes {
            route: ResolvedFunctionRoute {
                tenant,
                organization_id: "org_example00".to_owned(),
                function_name: "hello-world".to_owned(),
                active_version: 7,
                selected_regions: vec!["iad".to_owned(), "fra".to_owned()],
                regional_deployments: vec![
                    RegionalDeploymentHealth {
                        region: "iad".to_owned(),
                        healthy: true,
                        valid_until_unix_seconds: 200,
                    },
                    RegionalDeploymentHealth {
                        region: "fra".to_owned(),
                        healthy: true,
                        valid_until_unix_seconds: 200,
                    },
                ],
                verify_jwt: true,
                request_limit_bytes,
                response_limit_bytes: 4096,
                custom_domains: Vec::new(),
                allowed_origins: Vec::new(),
            },
        }
    }

    fn access_token(tenant: &TenantScope) -> (String, mako_identity::JsonWebKeySet) {
        let encryption_key = KeyEncryptionKey::generate();
        let mut ring = ProjectSigningKeyRing::new(tenant.clone());
        ring.create_initial(&encryption_key, 100).expect("key");
        let token = AccessTokenIssuer::new(
            &ring,
            &encryption_key,
            AccessTokenConfig::new("https://issuer.test", "mako-functions", 300).expect("config"),
        )
        .issue(
            AccessTokenInput {
                tenant: tenant.clone(),
                user_id: AppUserId::parse("usr_abcdefgh").expect("user"),
                role: "member".to_owned(),
                email: "policy@example.test".to_owned(),
                email_verified: true,
                session_id: SessionId::parse("ses_abcdefgh").expect("session"),
                authorization_epochs: AccessAuthorizationEpochs {
                    environment: 2,
                    user: 4,
                },
                trusted_claims: Map::new(),
            },
            100,
        )
        .expect("token")
        .expose_for_authorization_header()
        .to_owned();
        (token, ring.jwks())
    }

    fn tenant(project: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(project).expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
