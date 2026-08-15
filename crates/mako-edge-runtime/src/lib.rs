//! Regional supervision for isolated edge-function workers.

#![forbid(unsafe_code)]

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use mako_api::TenantScope;
use mako_audit::TelemetryRedactor;
use mako_edge_runtime_protocol::{
    DeploymentAddress, LoadDeploymentRequest, ProtocolValidationError, RuntimeErrorCode,
    RuntimePin, SecretVersionReference,
};
use sha2::{Digest, Sha256};

/// Identifies this workspace component in startup diagnostics.
pub const COMPONENT: &str = "edge-runtime";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorConfig {
    pub region: String,
    pub max_project_concurrency: u32,
    pub max_invocations_per_worker: u64,
    pub max_worker_age_milliseconds: u64,
}

impl SupervisorConfig {
    pub fn validate(&self) -> Result<(), SupervisorError> {
        if self.region.is_empty()
            || self.region.len() > 64
            || self.region.chars().any(char::is_control)
            || self.max_project_concurrency == 0
            || self.max_invocations_per_worker == 0
            || self.max_worker_age_milliseconds == 0
        {
            return Err(SupervisorError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerIdentity {
    pub deployment: DeploymentAddress,
    pub region: String,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerTerminationReason {
    DeploymentRetired,
    CleanRecycle,
    CrashRecovery,
    RejectedIdentity,
    SupersededStart,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerRequest {
    pub request_id: String,
    pub trace_id: String,
    pub method: String,
    pub path_and_query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body_chunks: Vec<Vec<u8>>,
    pub logs: Vec<WorkerLog>,
    pub usage: WorkerResourceUsage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerLog {
    pub timestamp_unix_milliseconds: u64,
    pub level: WorkerLogLevel,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkerResourceUsage {
    pub cpu_milliseconds: u64,
    pub wall_milliseconds: u64,
    pub peak_memory_bytes: u64,
    pub outbound_requests: u32,
    pub outbound_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerFailureKind {
    UserError,
    CpuLimitExceeded,
    WallTimeExceeded,
    MemoryLimitExceeded,
    EgressDenied,
    WorkerCrashed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerFailure {
    pub kind: WorkerFailureKind,
    pub logs: Vec<WorkerLog>,
    pub usage: WorkerResourceUsage,
}

impl fmt::Display for WorkerFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("isolated worker failed")
    }
}

impl Error for WorkerFailure {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionInvocationStatus {
    Succeeded,
    Failed(RuntimeErrorCode),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionInvocationTelemetry {
    pub deployment: DeploymentAddress,
    pub region: String,
    pub request_id: String,
    pub trace_id: String,
    pub status: FunctionInvocationStatus,
    pub http_status: Option<u16>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub usage: WorkerResourceUsage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionStructuredLog {
    pub deployment: DeploymentAddress,
    pub region: String,
    pub request_id: String,
    pub trace_id: String,
    pub timestamp_unix_milliseconds: u64,
    pub level: WorkerLogLevel,
    pub message: String,
}

pub trait FunctionTelemetrySink: Send + Sync {
    fn record_invocation(&self, telemetry: FunctionInvocationTelemetry);
    fn record_log(&self, log: FunctionStructuredLog);
}

#[async_trait]
pub trait IsolatedWorker: Send + Sync {
    fn identity(&self) -> &WorkerIdentity;

    async fn invoke(&self, request: WorkerRequest) -> Result<WorkerResponse, WorkerFailure>;

    async fn terminate(&self, reason: WorkerTerminationReason);
}

#[derive(Clone)]
pub struct WorkerStartRequest {
    pub manifest: LoadDeploymentRequest,
    pub bundle: Arc<[u8]>,
    pub secrets: Vec<InjectedSecret>,
    pub region: String,
    pub generation: u64,
}

impl fmt::Debug for WorkerStartRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerStartRequest")
            .field("manifest", &self.manifest)
            .field("bundle_bytes", &self.bundle.len())
            .field(
                "secret_versions",
                &self
                    .secrets
                    .iter()
                    .map(InjectedSecret::reference)
                    .collect::<Vec<_>>(),
            )
            .field("region", &self.region)
            .field("generation", &self.generation)
            .finish()
    }
}

/// A deployment-scoped secret delivered only to the trusted worker factory.
#[derive(Clone, Eq, PartialEq)]
pub struct InjectedSecret {
    reference: SecretVersionReference,
    value: String,
}

impl InjectedSecret {
    pub fn new(
        reference: SecretVersionReference,
        value: impl Into<String>,
    ) -> Result<Self, SupervisorError> {
        let value = value.into();
        if value.is_empty() || value.len() > 64 * 1024 || value.contains('\0') {
            return Err(SupervisorError::InvalidSecretInjection);
        }
        Ok(Self { reference, value })
    }

    #[must_use]
    pub const fn reference(&self) -> &SecretVersionReference {
        &self.reference
    }

    /// Exposes the value only at the trusted runtime-adapter boundary.
    #[must_use]
    pub fn expose_to_worker_factory(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for InjectedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InjectedSecret")
            .field("reference", &self.reference)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerStartError {
    Unavailable,
    InvalidBundle,
}

impl fmt::Display for WorkerStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("isolated worker could not start")
    }
}

impl Error for WorkerStartError {}

#[async_trait]
pub trait WorkerFactory: Send + Sync {
    /// Starts a worker that MUST enforce every CPU, wall, memory, and outbound
    /// network boundary in `request.manifest.limits` outside user code.
    async fn start(
        &self,
        request: WorkerStartRequest,
    ) -> Result<Arc<dyn IsolatedWorker>, WorkerStartError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorDeploymentStatus {
    pub deployment: DeploymentAddress,
    pub region: String,
    pub generation: Option<u64>,
    pub ready: bool,
}

struct WorkerSlot {
    worker: Arc<dyn IsolatedWorker>,
    generation: u64,
    created_at_milliseconds: u64,
    accepted_invocations: u64,
    active_invocations: u32,
}

struct DeploymentPool {
    manifest: LoadDeploymentRequest,
    bundle: Arc<[u8]>,
    secrets: Vec<InjectedSecret>,
    current: Option<WorkerSlot>,
    next_generation: u64,
    starting: bool,
}

#[derive(Default)]
struct SupervisorState {
    deployments: HashMap<DeploymentAddress, DeploymentPool>,
    project_active: HashMap<TenantScope, u32>,
}

#[derive(Clone)]
pub struct RegionalWorkerSupervisor {
    config: SupervisorConfig,
    pin: RuntimePin,
    factory: Arc<dyn WorkerFactory>,
    telemetry: Arc<dyn FunctionTelemetrySink>,
    state: Arc<Mutex<SupervisorState>>,
}

impl fmt::Debug for RegionalWorkerSupervisor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegionalWorkerSupervisor")
            .field("config", &self.config)
            .field("runtime_release", &self.pin.release)
            .finish_non_exhaustive()
    }
}

impl RegionalWorkerSupervisor {
    pub fn new(
        config: SupervisorConfig,
        factory: Arc<dyn WorkerFactory>,
        telemetry: Arc<dyn FunctionTelemetrySink>,
    ) -> Result<Self, SupervisorError> {
        config.validate()?;
        Ok(Self {
            config,
            pin: RuntimePin::embedded()?,
            factory,
            telemetry,
            state: Arc::new(Mutex::new(SupervisorState::default())),
        })
    }

    pub async fn load(
        &self,
        manifest: LoadDeploymentRequest,
        bundle: Arc<[u8]>,
        secrets: Vec<InjectedSecret>,
        now_milliseconds: u64,
    ) -> Result<SupervisorDeploymentStatus, SupervisorError> {
        manifest.validate(&self.pin)?;
        verify_bundle(&manifest.bundle_digest, &bundle)?;
        validate_secret_injection(&manifest.secret_versions, &secrets)?;
        let address = manifest.deployment.clone();
        {
            let mut state = self.lock_state()?;
            if let Some(existing) = state.deployments.get(&address) {
                if existing.manifest != manifest
                    || existing.bundle.as_ref() != bundle.as_ref()
                    || existing.secrets != secrets
                {
                    return Err(SupervisorError::DeploymentConflict);
                }
                return Ok(status(existing, &self.config.region));
            }
            state.deployments.insert(
                address.clone(),
                DeploymentPool {
                    manifest,
                    bundle,
                    secrets,
                    current: None,
                    next_generation: 2,
                    starting: true,
                },
            );
        }
        match self.start_reserved(&address, 1, now_milliseconds).await {
            Ok(status) => Ok(status),
            Err(error) => {
                self.lock_state()?.deployments.remove(&address);
                Err(error)
            }
        }
    }

    pub fn health(
        &self,
        address: &DeploymentAddress,
    ) -> Result<SupervisorDeploymentStatus, SupervisorError> {
        let state = self.lock_state()?;
        let pool = state
            .deployments
            .get(address)
            .ok_or(SupervisorError::DeploymentNotFound)?;
        Ok(status(pool, &self.config.region))
    }

    pub async fn invoke(
        &self,
        address: &DeploymentAddress,
        request: WorkerRequest,
        now_milliseconds: u64,
    ) -> Result<WorkerResponse, SupervisorInvocationError> {
        let request_id = request.request_id.clone();
        let trace_id = request.trace_id.clone();
        if !valid_request_id(&request_id) || !valid_trace_id(&trace_id) {
            return Err(SupervisorInvocationError::new(
                RuntimeErrorCode::InvalidDeployment,
                "req_invalid",
                false,
            ));
        }
        let limits = self
            .invocation_limits(address)
            .map_err(|error| SupervisorInvocationError::from_supervisor(&error, &request_id))?;
        let request_bytes = u64::try_from(request.body.len()).unwrap_or(u64::MAX);
        if request_bytes > limits.request_bytes {
            let error = SupervisorInvocationError::new(
                RuntimeErrorCode::RequestTooLarge,
                &request_id,
                false,
            );
            self.record_telemetry(
                address,
                &request_id,
                &trace_id,
                FunctionInvocationStatus::Failed(error.code()),
                None,
                request_bytes,
                0,
                WorkerResourceUsage::default(),
                &[],
            );
            return Err(error);
        }
        self.ensure_worker(address, now_milliseconds)
            .await
            .map_err(|error| SupervisorInvocationError::from_supervisor(&error, &request_id))?;
        let permit = loop {
            match self
                .acquire(address, now_milliseconds)
                .map_err(|error| SupervisorInvocationError::from_supervisor(&error, &request_id))?
            {
                AcquireResult::Permit(permit) => break permit,
                AcquireResult::Recycle(worker) => {
                    worker
                        .terminate(WorkerTerminationReason::CleanRecycle)
                        .await;
                    self.ensure_worker(address, now_milliseconds)
                        .await
                        .map_err(|error| {
                            SupervisorInvocationError::from_supervisor(&error, &request_id)
                        })?;
                }
            }
        };
        let result = permit.worker.invoke(request).await;
        let crashed = matches!(
            &result,
            Err(WorkerFailure {
                kind: WorkerFailureKind::CpuLimitExceeded
                    | WorkerFailureKind::WallTimeExceeded
                    | WorkerFailureKind::MemoryLimitExceeded
                    | WorkerFailureKind::WorkerCrashed,
                ..
            })
        );
        let recycle = self
            .release(&permit, crashed, now_milliseconds)
            .map_err(|error| SupervisorInvocationError::from_supervisor(&error, &request_id))?;
        if let Some((worker, reason)) = recycle {
            worker.terminate(reason).await;
            // Recovery is best effort for this response. A later invocation
            // retries startup if the replacement is temporarily unavailable.
            let _ = self.ensure_worker(address, now_milliseconds).await;
        }
        let (final_result, status, http_status, response_bytes, usage, logs) = match result {
            Ok(mut response) => {
                let response_bytes = response.body_chunks.iter().try_fold(0_u64, |total, chunk| {
                    total.checked_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX))
                });
                if response_bytes.is_none_or(|size| size > limits.response_bytes) {
                    (
                        Err(SupervisorInvocationError::new(
                            RuntimeErrorCode::ResponseTooLarge,
                            &request_id,
                            false,
                        )),
                        FunctionInvocationStatus::Failed(RuntimeErrorCode::ResponseTooLarge),
                        None,
                        response_bytes.unwrap_or(u64::MAX),
                        response.usage,
                        response.logs,
                    )
                } else {
                    let response_bytes = response_bytes.expect("bounded response size");
                    let status = response.status;
                    let usage = response.usage;
                    let logs = std::mem::take(&mut response.logs);
                    (
                        Ok(response),
                        FunctionInvocationStatus::Succeeded,
                        Some(status),
                        response_bytes,
                        usage,
                        logs,
                    )
                }
            }
            Err(failure) => {
                let code = worker_failure_code(failure.kind);
                (
                    Err(SupervisorInvocationError::new(code, &request_id, false)),
                    FunctionInvocationStatus::Failed(code),
                    None,
                    0,
                    failure.usage,
                    failure.logs,
                )
            }
        };
        self.record_telemetry(
            address,
            &request_id,
            &trace_id,
            status,
            http_status,
            request_bytes,
            response_bytes,
            usage,
            &logs,
        );
        final_result
    }

    fn invocation_limits(
        &self,
        address: &DeploymentAddress,
    ) -> Result<mako_edge_runtime_protocol::RuntimeLimits, SupervisorError> {
        self.lock_state()?
            .deployments
            .get(address)
            .map(|pool| pool.manifest.limits.clone())
            .ok_or(SupervisorError::DeploymentNotFound)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_telemetry(
        &self,
        address: &DeploymentAddress,
        request_id: &str,
        trace_id: &str,
        status: FunctionInvocationStatus,
        http_status: Option<u16>,
        request_bytes: u64,
        response_bytes: u64,
        usage: WorkerResourceUsage,
        logs: &[WorkerLog],
    ) {
        let secret_values = self
            .lock_state()
            .ok()
            .and_then(|state| {
                state.deployments.get(address).map(|pool| {
                    pool.secrets
                        .iter()
                        .map(|secret| secret.value.clone())
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        let redactor = TelemetryRedactor::new(secret_values);
        for log in logs.iter().take(1_000) {
            let message = redactor.as_ref().map_or_else(
                |_| "[REDACTED]".to_owned(),
                |redactor| redactor.redact_text(&log.message).into_string(),
            );
            let message = sanitize_log_message(&message);
            self.telemetry.record_log(FunctionStructuredLog {
                deployment: address.clone(),
                region: self.config.region.clone(),
                request_id: request_id.to_owned(),
                trace_id: trace_id.to_owned(),
                timestamp_unix_milliseconds: log.timestamp_unix_milliseconds,
                level: log.level,
                message,
            });
        }
        self.telemetry
            .record_invocation(FunctionInvocationTelemetry {
                deployment: address.clone(),
                region: self.config.region.clone(),
                request_id: request_id.to_owned(),
                trace_id: trace_id.to_owned(),
                status,
                http_status,
                request_bytes,
                response_bytes,
                usage,
            });
    }

    pub async fn retire(&self, address: &DeploymentAddress) -> Result<(), SupervisorError> {
        let worker = {
            let mut state = self.lock_state()?;
            let pool = state
                .deployments
                .get(address)
                .ok_or(SupervisorError::DeploymentNotFound)?;
            if pool
                .current
                .as_ref()
                .is_some_and(|slot| slot.active_invocations != 0)
            {
                return Err(SupervisorError::DeploymentBusy);
            }
            state
                .deployments
                .remove(address)
                .and_then(|pool| pool.current.map(|slot| slot.worker))
        };
        if let Some(worker) = worker {
            worker
                .terminate(WorkerTerminationReason::DeploymentRetired)
                .await;
        }
        Ok(())
    }

    async fn ensure_worker(
        &self,
        address: &DeploymentAddress,
        now_milliseconds: u64,
    ) -> Result<(), SupervisorError> {
        let generation = {
            let mut state = self.lock_state()?;
            let pool = state
                .deployments
                .get_mut(address)
                .ok_or(SupervisorError::DeploymentNotFound)?;
            if pool.current.is_some() {
                return Ok(());
            }
            if pool.starting {
                return Err(SupervisorError::WorkerStarting);
            }
            let generation = pool.next_generation;
            pool.next_generation = pool.next_generation.saturating_add(1);
            pool.starting = true;
            generation
        };
        self.start_reserved(address, generation, now_milliseconds)
            .await?;
        Ok(())
    }

    async fn start_reserved(
        &self,
        address: &DeploymentAddress,
        generation: u64,
        now_milliseconds: u64,
    ) -> Result<SupervisorDeploymentStatus, SupervisorError> {
        let request = {
            let state = self.lock_state()?;
            let pool = state
                .deployments
                .get(address)
                .ok_or(SupervisorError::DeploymentNotFound)?;
            WorkerStartRequest {
                manifest: pool.manifest.clone(),
                bundle: pool.bundle.clone(),
                secrets: pool.secrets.clone(),
                region: self.config.region.clone(),
                generation,
            }
        };
        let worker = match self.factory.start(request).await {
            Ok(worker) => worker,
            Err(error) => {
                if let Some(pool) = self.lock_state()?.deployments.get_mut(address) {
                    pool.starting = false;
                }
                return Err(error.into());
            }
        };
        let expected_identity = WorkerIdentity {
            deployment: address.clone(),
            region: self.config.region.clone(),
            generation,
        };
        if worker.identity() != &expected_identity {
            worker
                .terminate(WorkerTerminationReason::RejectedIdentity)
                .await;
            if let Some(pool) = self.lock_state()?.deployments.get_mut(address) {
                pool.starting = false;
            }
            return Err(SupervisorError::WorkerIdentityMismatch);
        }
        let superseded = {
            let mut state = self.lock_state()?;
            match state.deployments.get_mut(address) {
                None => true,
                Some(pool) if pool.current.is_some() || !pool.starting => true,
                Some(pool) => {
                    pool.current = Some(WorkerSlot {
                        worker: worker.clone(),
                        generation,
                        created_at_milliseconds: now_milliseconds,
                        accepted_invocations: 0,
                        active_invocations: 0,
                    });
                    pool.starting = false;
                    false
                }
            }
        };
        if superseded {
            worker
                .terminate(WorkerTerminationReason::SupersededStart)
                .await;
            return Err(SupervisorError::WorkerStartSuperseded);
        }
        self.health(address)
    }

    fn acquire(
        &self,
        address: &DeploymentAddress,
        now_milliseconds: u64,
    ) -> Result<AcquireResult, SupervisorError> {
        let mut state = self.lock_state()?;
        let project_active = state
            .project_active
            .get(&address.tenant)
            .copied()
            .unwrap_or_default();
        if project_active >= self.config.max_project_concurrency {
            return Err(SupervisorError::ProjectConcurrencyLimited);
        }
        let pool = state
            .deployments
            .get_mut(address)
            .ok_or(SupervisorError::DeploymentNotFound)?;
        let slot = pool
            .current
            .as_mut()
            .ok_or(SupervisorError::WorkerStarting)?;
        let expired = slot.accepted_invocations >= self.config.max_invocations_per_worker
            || now_milliseconds.saturating_sub(slot.created_at_milliseconds)
                >= self.config.max_worker_age_milliseconds;
        if expired {
            if slot.active_invocations != 0 {
                return Err(SupervisorError::WorkerRecycling);
            }
            let worker = pool.current.take().expect("worker slot was present").worker;
            return Ok(AcquireResult::Recycle(worker));
        }
        if slot.active_invocations >= pool.manifest.limits.concurrency {
            return Err(SupervisorError::DeploymentConcurrencyLimited);
        }
        slot.active_invocations += 1;
        slot.accepted_invocations += 1;
        let permit = InvocationPermit {
            deployment: address.clone(),
            generation: slot.generation,
            worker: slot.worker.clone(),
        };
        state
            .project_active
            .insert(address.tenant.clone(), project_active + 1);
        Ok(AcquireResult::Permit(permit))
    }

    fn release(
        &self,
        permit: &InvocationPermit,
        crashed: bool,
        now_milliseconds: u64,
    ) -> Result<Option<WorkerRecycle>, SupervisorError> {
        let mut state = self.lock_state()?;
        let project_active = state
            .project_active
            .get(&permit.deployment.tenant)
            .copied()
            .ok_or(SupervisorError::CorruptState)?;
        if project_active <= 1 {
            state.project_active.remove(&permit.deployment.tenant);
        } else {
            state
                .project_active
                .insert(permit.deployment.tenant.clone(), project_active - 1);
        }
        let pool = state
            .deployments
            .get_mut(&permit.deployment)
            .ok_or(SupervisorError::DeploymentNotFound)?;
        let Some(slot) = pool
            .current
            .as_mut()
            .filter(|slot| slot.generation == permit.generation)
        else {
            return Ok(None);
        };
        slot.active_invocations = slot
            .active_invocations
            .checked_sub(1)
            .ok_or(SupervisorError::CorruptState)?;
        let expired = slot.active_invocations == 0
            && (slot.accepted_invocations >= self.config.max_invocations_per_worker
                || now_milliseconds.saturating_sub(slot.created_at_milliseconds)
                    >= self.config.max_worker_age_milliseconds);
        if crashed || expired {
            let worker = pool.current.take().expect("worker slot was present").worker;
            pool.starting = false;
            return Ok(Some((
                worker,
                if crashed {
                    WorkerTerminationReason::CrashRecovery
                } else {
                    WorkerTerminationReason::CleanRecycle
                },
            )));
        }
        Ok(None)
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, SupervisorState>, SupervisorError> {
        self.state.lock().map_err(|_| SupervisorError::CorruptState)
    }
}

struct InvocationPermit {
    deployment: DeploymentAddress,
    generation: u64,
    worker: Arc<dyn IsolatedWorker>,
}

type WorkerRecycle = (Arc<dyn IsolatedWorker>, WorkerTerminationReason);

enum AcquireResult {
    Permit(InvocationPermit),
    Recycle(Arc<dyn IsolatedWorker>),
}

fn status(pool: &DeploymentPool, region: &str) -> SupervisorDeploymentStatus {
    SupervisorDeploymentStatus {
        deployment: pool.manifest.deployment.clone(),
        region: region.to_owned(),
        generation: pool.current.as_ref().map(|worker| worker.generation),
        ready: pool.current.is_some() && !pool.starting,
    }
}

fn verify_bundle(expected_digest: &str, bundle: &[u8]) -> Result<(), SupervisorError> {
    let digest = Sha256::digest(bundle);
    let mut actual = String::from("sha256:");
    for byte in digest {
        use fmt::Write;
        write!(&mut actual, "{byte:02x}").expect("writing to a string cannot fail");
    }
    if actual == expected_digest {
        Ok(())
    } else {
        Err(SupervisorError::BundleDigestMismatch)
    }
}

fn validate_secret_injection(
    references: &[SecretVersionReference],
    secrets: &[InjectedSecret],
) -> Result<(), SupervisorError> {
    if references.len() != secrets.len()
        || secrets.iter().enumerate().any(|(index, secret)| {
            !references
                .iter()
                .any(|reference| reference == secret.reference())
                || secrets[..index]
                    .iter()
                    .any(|candidate| candidate.reference() == secret.reference())
        })
    {
        return Err(SupervisorError::InvalidSecretInjection);
    }
    Ok(())
}

fn valid_request_id(value: &str) -> bool {
    value.len() >= 8
        && value.len() <= 128
        && value.starts_with("req_")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_trace_id(value: &str) -> bool {
    value.len() >= 8
        && value.len() <= 128
        && value.starts_with("trc_")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

const fn worker_failure_code(kind: WorkerFailureKind) -> RuntimeErrorCode {
    match kind {
        WorkerFailureKind::UserError => RuntimeErrorCode::DeploymentUnhealthy,
        WorkerFailureKind::CpuLimitExceeded => RuntimeErrorCode::CpuLimitExceeded,
        WorkerFailureKind::WallTimeExceeded => RuntimeErrorCode::WallTimeExceeded,
        WorkerFailureKind::MemoryLimitExceeded => RuntimeErrorCode::MemoryLimitExceeded,
        WorkerFailureKind::EgressDenied => RuntimeErrorCode::EgressDenied,
        WorkerFailureKind::WorkerCrashed => RuntimeErrorCode::WorkerCrashed,
    }
}

fn sanitize_log_message(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len().min(16 * 1024));
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if sanitized.len() + character.len_utf8() > 16 * 1024 {
            break;
        }
        sanitized.push(character);
    }
    sanitized
}

/// Sanitized invocation failure returned across the runtime protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorInvocationError {
    code: RuntimeErrorCode,
    request_id: String,
    retryable: bool,
}

impl SupervisorInvocationError {
    fn new(code: RuntimeErrorCode, request_id: &str, retryable: bool) -> Self {
        Self {
            code,
            request_id: request_id.to_owned(),
            retryable,
        }
    }

    fn from_supervisor(error: &SupervisorError, request_id: &str) -> Self {
        Self::new(
            error.runtime_code(),
            request_id,
            matches!(
                error,
                SupervisorError::WorkerStarting
                    | SupervisorError::WorkerStart(_)
                    | SupervisorError::WorkerStartSuperseded
                    | SupervisorError::ProjectConcurrencyLimited
                    | SupervisorError::DeploymentConcurrencyLimited
                    | SupervisorError::WorkerRecycling
            ),
        )
    }

    #[must_use]
    pub const fn code(&self) -> RuntimeErrorCode {
        self.code
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }
}

impl fmt::Display for SupervisorInvocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("function invocation failed")
    }
}

impl Error for SupervisorInvocationError {}

#[derive(Debug, Eq, PartialEq)]
pub enum SupervisorError {
    InvalidConfiguration,
    InvalidProtocol(ProtocolValidationError),
    InvalidSecretInjection,
    BundleDigestMismatch,
    DeploymentConflict,
    DeploymentNotFound,
    DeploymentBusy,
    WorkerStarting,
    WorkerStart(WorkerStartError),
    WorkerStartSuperseded,
    WorkerIdentityMismatch,
    ProjectConcurrencyLimited,
    DeploymentConcurrencyLimited,
    WorkerRecycling,
    UserCodeFailed,
    WorkerCrashed,
    CorruptState,
}

impl SupervisorError {
    #[must_use]
    pub const fn runtime_code(&self) -> RuntimeErrorCode {
        match self {
            Self::InvalidConfiguration
            | Self::InvalidProtocol(_)
            | Self::InvalidSecretInjection
            | Self::BundleDigestMismatch
            | Self::DeploymentConflict
            | Self::WorkerIdentityMismatch
            | Self::CorruptState => RuntimeErrorCode::InvalidDeployment,
            Self::DeploymentNotFound => RuntimeErrorCode::DeploymentNotFound,
            Self::DeploymentBusy
            | Self::WorkerStarting
            | Self::WorkerStart(_)
            | Self::WorkerStartSuperseded
            | Self::WorkerRecycling => RuntimeErrorCode::RuntimeUnavailable,
            Self::ProjectConcurrencyLimited | Self::DeploymentConcurrencyLimited => {
                RuntimeErrorCode::ConcurrencyLimited
            }
            Self::UserCodeFailed => RuntimeErrorCode::DeploymentUnhealthy,
            Self::WorkerCrashed => RuntimeErrorCode::WorkerCrashed,
        }
    }
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "worker supervisor configuration is invalid",
            Self::InvalidProtocol(_) => "runtime protocol message is invalid",
            Self::InvalidSecretInjection => "runtime secret injection is invalid",
            Self::BundleDigestMismatch => "function bundle digest does not match",
            Self::DeploymentConflict => "function deployment address already exists",
            Self::DeploymentNotFound => "function deployment was not found",
            Self::DeploymentBusy => "function deployment still has active invocations",
            Self::WorkerStarting => "function worker is starting",
            Self::WorkerStart(_) => "function worker failed to start",
            Self::WorkerStartSuperseded => "function worker start was superseded",
            Self::WorkerIdentityMismatch => {
                "function worker identity does not match its deployment"
            }
            Self::ProjectConcurrencyLimited => "project function concurrency is exhausted",
            Self::DeploymentConcurrencyLimited => "function deployment concurrency is exhausted",
            Self::WorkerRecycling => "function worker is recycling",
            Self::UserCodeFailed => "function execution failed",
            Self::WorkerCrashed => "function worker crashed",
            Self::CorruptState => "function supervisor state is unavailable",
        })
    }
}

impl Error for SupervisorError {}

impl From<ProtocolValidationError> for SupervisorError {
    fn from(error: ProtocolValidationError) -> Self {
        Self::InvalidProtocol(error)
    }
}

impl From<WorkerStartError> for SupervisorError {
    fn from(error: WorkerStartError) -> Self {
        Self::WorkerStart(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use mako_api::{EnvironmentId, ProjectId};
    use mako_edge_runtime_protocol::{
        BundleFormat, OutboundNetworkPolicy, PROTOCOL_VERSION, RuntimeLimits,
    };
    use proptest::prelude::*;

    use super::*;

    #[derive(Default)]
    struct Factory {
        starts: Mutex<Vec<WorkerIdentity>>,
        started_secrets: Mutex<Vec<Vec<InjectedSecret>>>,
        crash_generation: AtomicU64,
        forced_failure: Mutex<Option<WorkerFailureKind>>,
        response_bytes: AtomicU64,
        terminations: Arc<Mutex<Vec<(WorkerIdentity, WorkerTerminationReason)>>>,
    }

    #[derive(Default)]
    struct Telemetry {
        invocations: Mutex<Vec<FunctionInvocationTelemetry>>,
        logs: Mutex<Vec<FunctionStructuredLog>>,
    }

    impl FunctionTelemetrySink for Telemetry {
        fn record_invocation(&self, telemetry: FunctionInvocationTelemetry) {
            self.invocations
                .lock()
                .expect("telemetry invocations")
                .push(telemetry);
        }

        fn record_log(&self, log: FunctionStructuredLog) {
            self.logs.lock().expect("telemetry logs").push(log);
        }
    }

    #[async_trait]
    impl WorkerFactory for Factory {
        async fn start(
            &self,
            request: WorkerStartRequest,
        ) -> Result<Arc<dyn IsolatedWorker>, WorkerStartError> {
            self.started_secrets
                .lock()
                .expect("started secrets")
                .push(request.secrets.clone());
            let identity = WorkerIdentity {
                deployment: request.manifest.deployment,
                region: request.region,
                generation: request.generation,
            };
            self.starts.lock().expect("starts").push(identity.clone());
            Ok(Arc::new(Worker {
                identity,
                failure: if self.crash_generation.load(Ordering::SeqCst) == request.generation {
                    Some(WorkerFailureKind::WorkerCrashed)
                } else {
                    *self.forced_failure.lock().expect("forced failure")
                },
                response_bytes: self.response_bytes.load(Ordering::SeqCst),
                terminations: self.terminations.clone(),
            }))
        }
    }

    struct Worker {
        identity: WorkerIdentity,
        failure: Option<WorkerFailureKind>,
        response_bytes: u64,
        terminations: Arc<Mutex<Vec<(WorkerIdentity, WorkerTerminationReason)>>>,
    }

    #[async_trait]
    impl IsolatedWorker for Worker {
        fn identity(&self) -> &WorkerIdentity {
            &self.identity
        }

        async fn invoke(&self, _: WorkerRequest) -> Result<WorkerResponse, WorkerFailure> {
            if let Some(kind) = self.failure {
                Err(WorkerFailure {
                    kind,
                    logs: vec![worker_log()],
                    usage: worker_usage(),
                })
            } else {
                let body = if self.response_bytes == 0 {
                    self.identity.generation.to_string().into_bytes()
                } else {
                    vec![0; usize::try_from(self.response_bytes).expect("test response size")]
                };
                Ok(WorkerResponse {
                    status: 200,
                    headers: vec![],
                    body_chunks: vec![body],
                    logs: vec![worker_log()],
                    usage: worker_usage(),
                })
            }
        }

        async fn terminate(&self, reason: WorkerTerminationReason) {
            self.terminations
                .lock()
                .expect("terminations")
                .push((self.identity.clone(), reason));
        }
    }

    #[test]
    fn workers_are_bound_to_exact_tenants_and_concurrency_is_admitted() {
        futures::executor::block_on(async {
            let factory = Arc::new(Factory::default());
            let supervisor = supervisor(factory.clone(), 2);
            let first = deployment("prj_abcdefgh", 1);
            let second = deployment("prj_ijklmnop", 1);
            supervisor
                .load(first.clone(), bundle(), vec![], 1)
                .await
                .expect("first load");
            supervisor
                .load(second.clone(), bundle(), vec![], 1)
                .await
                .expect("second load");
            let starts = factory.starts.lock().expect("starts");
            assert_eq!(starts.len(), 2);
            assert_ne!(starts[0].deployment.tenant, starts[1].deployment.tenant);
            drop(starts);

            let first_permit = match supervisor
                .acquire(&first.deployment, 2)
                .expect("first permit")
            {
                AcquireResult::Permit(permit) => permit,
                AcquireResult::Recycle(_) => panic!("unexpected recycle"),
            };
            match supervisor.acquire(&first.deployment, 2) {
                Err(error) => {
                    assert_eq!(error, SupervisorError::DeploymentConcurrencyLimited);
                }
                Ok(_) => panic!("deployment concurrency should be limited"),
            }
            supervisor
                .release(&first_permit, false, 2)
                .expect("release");
        });
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn arbitrary_edge_deployments_cannot_alias_another_tenants_worker(
            project_a in "[a-z0-9_-]{8,24}",
            environment_a in "[a-z0-9_-]{8,24}",
            project_b in "[a-z0-9_-]{8,24}",
            environment_b in "[a-z0-9_-]{8,24}",
        ) {
            prop_assume!(project_a != project_b || environment_a != environment_b);
            futures::executor::block_on(async {
                let factory = Arc::new(Factory::default());
                let supervisor = supervisor(factory.clone(), 10);
                let first = deployment_for(&project_a, &environment_a, 1);
                let second = deployment_for(&project_b, &environment_b, 1);
                supervisor.load(first.clone(), bundle(), vec![], 1).await.expect("first load");
                supervisor.load(second.clone(), bundle(), vec![], 1).await.expect("second load");
                prop_assert_eq!(
                    supervisor.health(&first.deployment).expect("first health").deployment,
                    first.deployment.clone(),
                );
                prop_assert_eq!(
                    supervisor.health(&second.deployment).expect("second health").deployment,
                    second.deployment.clone(),
                );
                let starts = factory.starts.lock().expect("starts");
                prop_assert_eq!(starts.len(), 2);
                prop_assert_ne!(&starts[0].deployment.tenant, &starts[1].deployment.tenant);
                Ok(())
            })?;
        }
    }

    #[test]
    fn crash_recovery_and_clean_recycling_create_new_generations() {
        futures::executor::block_on(async {
            let factory = Arc::new(Factory::default());
            factory.crash_generation.store(1, Ordering::SeqCst);
            let supervisor = supervisor(factory.clone(), 1);
            let manifest = deployment("prj_abcdefgh", 1);
            supervisor
                .load(manifest.clone(), bundle(), vec![], 1)
                .await
                .expect("load");
            let crashed = supervisor
                .invoke(&manifest.deployment, worker_request(), 2)
                .await
                .expect_err("crash");
            assert_eq!(crashed.code(), RuntimeErrorCode::WorkerCrashed);
            assert_eq!(crashed.request_id(), "req_example00");
            assert!(!crashed.retryable());
            assert_eq!(
                supervisor
                    .health(&manifest.deployment)
                    .expect("health")
                    .generation,
                Some(2)
            );
            let response = supervisor
                .invoke(&manifest.deployment, worker_request(), 3)
                .await
                .expect("recovered invocation");
            assert_eq!(response.body_chunks, vec![b"2".to_vec()]);
            assert_eq!(
                supervisor
                    .health(&manifest.deployment)
                    .expect("health")
                    .generation,
                Some(3)
            );
            let terminations = factory.terminations.lock().expect("terminations");
            assert!(
                terminations
                    .iter()
                    .any(|(_, reason)| { *reason == WorkerTerminationReason::CrashRecovery })
            );
            assert!(
                terminations
                    .iter()
                    .any(|(_, reason)| { *reason == WorkerTerminationReason::CleanRecycle })
            );
        });
    }

    #[test]
    fn every_resource_limit_returns_a_stable_correlation_aware_code() {
        futures::executor::block_on(async {
            for (failure, expected) in [
                (
                    WorkerFailureKind::CpuLimitExceeded,
                    RuntimeErrorCode::CpuLimitExceeded,
                ),
                (
                    WorkerFailureKind::WallTimeExceeded,
                    RuntimeErrorCode::WallTimeExceeded,
                ),
                (
                    WorkerFailureKind::MemoryLimitExceeded,
                    RuntimeErrorCode::MemoryLimitExceeded,
                ),
                (
                    WorkerFailureKind::EgressDenied,
                    RuntimeErrorCode::EgressDenied,
                ),
            ] {
                let factory = Arc::new(Factory::default());
                *factory.forced_failure.lock().expect("forced failure") = Some(failure);
                let telemetry = Arc::new(Telemetry::default());
                let supervisor = supervisor_with_telemetry(factory, 10, telemetry.clone());
                let manifest = deployment("prj_abcdefgh", 1);
                supervisor
                    .load(manifest.clone(), bundle(), vec![], 1)
                    .await
                    .expect("load");
                let error = supervisor
                    .invoke(&manifest.deployment, worker_request(), 2)
                    .await
                    .expect_err("limit failure");
                assert_eq!(error.code(), expected);
                assert_eq!(error.request_id(), "req_example00");
                assert!(!error.retryable());
                {
                    let recorded = telemetry.invocations.lock().expect("telemetry invocations");
                    assert_eq!(recorded.len(), 1);
                    assert_eq!(
                        recorded[0].status,
                        FunctionInvocationStatus::Failed(expected)
                    );
                    assert_eq!(recorded[0].usage, worker_usage());
                }
            }

            let request_factory = Arc::new(Factory::default());
            let request_supervisor = supervisor(request_factory, 10);
            let mut request_manifest = deployment("prj_request00", 1);
            request_manifest.limits.request_bytes = 4;
            request_supervisor
                .load(request_manifest.clone(), bundle(), vec![], 1)
                .await
                .expect("load request limit");
            let mut oversized_request = worker_request();
            oversized_request.body = vec![0; 5];
            assert_eq!(
                request_supervisor
                    .invoke(&request_manifest.deployment, oversized_request, 2)
                    .await
                    .expect_err("request too large")
                    .code(),
                RuntimeErrorCode::RequestTooLarge
            );

            let response_factory = Arc::new(Factory::default());
            response_factory.response_bytes.store(5, Ordering::SeqCst);
            let response_supervisor = supervisor(response_factory, 10);
            let mut response_manifest = deployment("prj_response0", 1);
            response_manifest.limits.response_bytes = 4;
            response_supervisor
                .load(response_manifest.clone(), bundle(), vec![], 1)
                .await
                .expect("load response limit");
            assert_eq!(
                response_supervisor
                    .invoke(&response_manifest.deployment, worker_request(), 2)
                    .await
                    .expect_err("response too large")
                    .code(),
                RuntimeErrorCode::ResponseTooLarge
            );
        });
    }

    #[test]
    fn adversarial_limit_and_crash_failures_recycle_only_the_compromised_worker() {
        futures::executor::block_on(async {
            for (failure, expected) in [
                (
                    WorkerFailureKind::CpuLimitExceeded,
                    RuntimeErrorCode::CpuLimitExceeded,
                ),
                (
                    WorkerFailureKind::WallTimeExceeded,
                    RuntimeErrorCode::WallTimeExceeded,
                ),
                (
                    WorkerFailureKind::MemoryLimitExceeded,
                    RuntimeErrorCode::MemoryLimitExceeded,
                ),
                (
                    WorkerFailureKind::WorkerCrashed,
                    RuntimeErrorCode::WorkerCrashed,
                ),
            ] {
                let factory = Arc::new(Factory::default());
                *factory.forced_failure.lock().expect("forced failure") = Some(failure);
                let supervisor = supervisor(factory.clone(), 10);
                let manifest = deployment("prj_adversary", 1);
                supervisor
                    .load(manifest.clone(), bundle(), vec![], 1)
                    .await
                    .expect("load adversarial deployment");
                let error = supervisor
                    .invoke(&manifest.deployment, worker_request(), 2)
                    .await
                    .expect_err("adversarial invocation must fail");
                assert_eq!(error.code(), expected);
                assert_eq!(
                    supervisor
                        .health(&manifest.deployment)
                        .expect("replacement health")
                        .generation,
                    Some(2)
                );
                assert_eq!(factory.starts.lock().expect("starts").len(), 2);
                assert_eq!(
                    factory.terminations.lock().expect("terminations")[0].1,
                    WorkerTerminationReason::CrashRecovery
                );
            }

            let egress_factory = Arc::new(Factory::default());
            *egress_factory
                .forced_failure
                .lock()
                .expect("forced egress failure") = Some(WorkerFailureKind::EgressDenied);
            let egress_supervisor = supervisor(egress_factory.clone(), 10);
            let manifest = deployment("prj_egress000", 1);
            egress_supervisor
                .load(manifest.clone(), bundle(), vec![], 1)
                .await
                .expect("load egress deployment");
            assert_eq!(
                egress_supervisor
                    .invoke(&manifest.deployment, worker_request(), 2)
                    .await
                    .expect_err("egress must be denied")
                    .code(),
                RuntimeErrorCode::EgressDenied
            );
            assert_eq!(
                egress_supervisor
                    .health(&manifest.deployment)
                    .expect("egress health")
                    .generation,
                Some(1)
            );
            assert!(
                egress_factory
                    .terminations
                    .lock()
                    .expect("egress terminations")
                    .is_empty()
            );
        });
    }

    #[test]
    fn only_exact_declared_secret_versions_reach_the_worker_factory() {
        futures::executor::block_on(async {
            let factory = Arc::new(Factory::default());
            let telemetry = Arc::new(Telemetry::default());
            let runtime_supervisor =
                supervisor_with_telemetry(factory.clone(), 10, telemetry.clone());
            let mut manifest = deployment("prj_secrets00", 1);
            let reference = SecretVersionReference {
                name: "API_TOKEN".to_owned(),
                version: 2,
            };
            manifest.secret_versions = vec![reference.clone()];
            let secret = InjectedSecret::new(reference.clone(), "secret-runtime-value")
                .expect("injected secret");
            assert!(!format!("{secret:?}").contains("secret-runtime-value"));

            runtime_supervisor
                .load(manifest.clone(), bundle(), vec![secret], 1)
                .await
                .expect("load with declared secret");
            {
                let started = factory.started_secrets.lock().expect("started secrets");
                assert_eq!(started.len(), 1);
                assert_eq!(started[0][0].reference(), &reference);
                assert_eq!(
                    started[0][0].expose_to_worker_factory(),
                    "secret-runtime-value"
                );
            }
            runtime_supervisor
                .invoke(&manifest.deployment, worker_request(), 2)
                .await
                .expect("invoke with telemetry");
            {
                let invocations = telemetry.invocations.lock().expect("telemetry invocations");
                assert_eq!(invocations.len(), 1);
                assert_eq!(invocations[0].deployment, manifest.deployment);
                assert_eq!(invocations[0].region, "local");
                assert_eq!(invocations[0].request_id, "req_example00");
                assert_eq!(invocations[0].trace_id, "trc_example00");
                assert_eq!(invocations[0].status, FunctionInvocationStatus::Succeeded);
                assert_eq!(invocations[0].usage, worker_usage());
            }
            {
                let logs = telemetry.logs.lock().expect("telemetry logs");
                assert_eq!(logs.len(), 1);
                assert_eq!(logs[0].message, "configured secret=[REDACTED]");
                assert!(!logs[0].message.contains("secret-runtime-value"));
            }

            let other_factory = Arc::new(Factory::default());
            let other_supervisor = supervisor(other_factory.clone(), 10);
            let undeclared = InjectedSecret::new(
                SecretVersionReference {
                    name: "OTHER_TOKEN".to_owned(),
                    version: 1,
                },
                "must-not-reach-worker",
            )
            .expect("undeclared secret");
            assert_eq!(
                other_supervisor
                    .load(manifest, bundle(), vec![undeclared], 1)
                    .await
                    .expect_err("undeclared secret is rejected"),
                SupervisorError::InvalidSecretInjection
            );
            assert!(
                other_factory
                    .started_secrets
                    .lock()
                    .expect("started secrets")
                    .is_empty()
            );
        });
    }

    fn supervisor(factory: Arc<Factory>, max_invocations: u64) -> RegionalWorkerSupervisor {
        supervisor_with_telemetry(factory, max_invocations, Arc::new(Telemetry::default()))
    }

    fn supervisor_with_telemetry(
        factory: Arc<Factory>,
        max_invocations: u64,
        telemetry: Arc<Telemetry>,
    ) -> RegionalWorkerSupervisor {
        RegionalWorkerSupervisor::new(
            SupervisorConfig {
                region: "local".to_owned(),
                max_project_concurrency: 2,
                max_invocations_per_worker: max_invocations,
                max_worker_age_milliseconds: 60_000,
            },
            factory,
            telemetry,
        )
        .expect("supervisor")
    }

    fn deployment(project: &str, concurrency: u32) -> LoadDeploymentRequest {
        deployment_for(
            project.strip_prefix("prj_").expect("project prefix"),
            "abcdefgh",
            concurrency,
        )
    }

    fn deployment_for(project: &str, environment: &str, concurrency: u32) -> LoadDeploymentRequest {
        LoadDeploymentRequest {
            protocol_version: PROTOCOL_VERSION,
            deployment: DeploymentAddress {
                tenant: TenantScope::new(
                    ProjectId::parse(format!("prj_{project}")).expect("project"),
                    EnvironmentId::parse(format!("env_{environment}")).expect("environment"),
                ),
                function_name: "hello-world".to_owned(),
                version: 1,
            },
            bundle_digest: bundle_digest(),
            bundle_format: BundleFormat::Prebuilt,
            entrypoint: "index.ts".to_owned(),
            runtime_release: "v1.74.3".to_owned(),
            limits: RuntimeLimits {
                cpu_milliseconds: 100,
                wall_milliseconds: 1_000,
                memory_bytes: 64 * 1024 * 1024,
                request_bytes: 1024,
                response_bytes: 1024,
                concurrency,
                outbound_network: OutboundNetworkPolicy::DenyAll,
            },
            verify_jwt: true,
            secret_versions: vec![],
        }
    }

    fn bundle() -> Arc<[u8]> {
        Arc::from(b"test bundle".as_slice())
    }

    fn bundle_digest() -> String {
        let digest = Sha256::digest(b"test bundle");
        format!("sha256:{digest:x}")
    }

    fn worker_request() -> WorkerRequest {
        WorkerRequest {
            request_id: "req_example00".to_owned(),
            trace_id: "trc_example00".to_owned(),
            method: "POST".to_owned(),
            path_and_query: "/".to_owned(),
            headers: vec![],
            body: vec![],
        }
    }

    fn worker_log() -> WorkerLog {
        WorkerLog {
            timestamp_unix_milliseconds: 1_000,
            level: WorkerLogLevel::Info,
            message: "configured secret=secret-runtime-value".to_owned(),
        }
    }

    const fn worker_usage() -> WorkerResourceUsage {
        WorkerResourceUsage {
            cpu_milliseconds: 7,
            wall_milliseconds: 11,
            peak_memory_bytes: 4_096,
            outbound_requests: 1,
            outbound_bytes: 64,
        }
    }
}
