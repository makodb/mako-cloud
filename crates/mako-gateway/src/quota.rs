use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    num::{NonZeroU64, NonZeroUsize},
    sync::Arc,
    thread,
    time::Duration,
};

use async_trait::async_trait;
use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, StorageError,
    StorageErrorKind, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Serialize};

const COUNTER_DOMAIN_PREFIX: &str = "mako:gateway-quota:counter:v1";
const RESERVATION_DOMAIN_PREFIX: &str = "mako:gateway-quota:reservation:v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayQuotaResource {
    AuthenticationRequests,
    DocumentRequests,
    DocumentBytes,
    ReplicationRequests,
    ReplicationBytes,
    FunctionInvocations,
    PublicFunctionInvocations,
    FunctionRequestBytes,
    EgressRequests,
    EgressBytes,
}

impl GatewayQuotaResource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationRequests => "authentication_requests",
            Self::DocumentRequests => "document_requests",
            Self::DocumentBytes => "document_bytes",
            Self::ReplicationRequests => "replication_requests",
            Self::ReplicationBytes => "replication_bytes",
            Self::FunctionInvocations => "function_invocations",
            Self::PublicFunctionInvocations => "public_function_invocations",
            Self::FunctionRequestBytes => "function_request_bytes",
            Self::EgressRequests => "egress_requests",
            Self::EgressBytes => "egress_bytes",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayQuotaWindow {
    pub limit: NonZeroU64,
    pub window_milliseconds: NonZeroU64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayQuotaLimit {
    pub hard: Option<GatewayQuotaWindow>,
    pub rate: Option<GatewayQuotaWindow>,
}

impl GatewayQuotaLimit {
    fn is_empty(self) -> bool {
        self.hard.is_none() && self.rate.is_none()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GatewayQuotaPolicy(BTreeMap<GatewayQuotaResource, GatewayQuotaLimit>);

impl GatewayQuotaPolicy {
    pub fn new(
        limits: impl IntoIterator<Item = (GatewayQuotaResource, GatewayQuotaLimit)>,
    ) -> Result<Self, GatewayQuotaError> {
        let limits = limits.into_iter().collect::<BTreeMap<_, _>>();
        if limits.values().any(|limit| limit.is_empty()) {
            return Err(GatewayQuotaError::InvalidPolicy);
        }
        Ok(Self(limits))
    }

    #[must_use]
    pub fn limit(&self, resource: GatewayQuotaResource) -> Option<GatewayQuotaLimit> {
        self.0.get(&resource).copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayQuotaCharge {
    pub resource: GatewayQuotaResource,
    pub amount: NonZeroU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayQuotaDecision {
    Allowed,
    Throttled {
        resource: GatewayQuotaResource,
        retry_after_milliseconds: u64,
    },
    HardLimit {
        resource: GatewayQuotaResource,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct GatewayQuotaEngineConfig {
    pub durability: Durability,
    pub maximum_conflict_retries: NonZeroUsize,
    pub maximum_charges_per_request: NonZeroUsize,
}

#[derive(Clone)]
pub struct GatewayQuotaEngine {
    adapter: Arc<dyn KvAdapter>,
    config: GatewayQuotaEngineConfig,
}

impl GatewayQuotaEngine {
    #[must_use]
    pub fn new(adapter: Arc<dyn KvAdapter>, config: GatewayQuotaEngineConfig) -> Self {
        Self { adapter, config }
    }

    pub async fn check_and_reserve(
        &self,
        tenant: &TenantScope,
        reservation_id: &str,
        charges: &[GatewayQuotaCharge],
        policy: &GatewayQuotaPolicy,
        now_unix_milliseconds: u64,
    ) -> Result<GatewayQuotaDecision, GatewayQuotaError> {
        validate_request(
            reservation_id,
            charges,
            self.config.maximum_charges_per_request,
            now_unix_milliseconds,
        )?;
        let active = charges
            .iter()
            .filter_map(|charge| policy.limit(charge.resource).map(|limit| (*charge, limit)))
            .collect::<Vec<_>>();
        if active.is_empty() {
            return Ok(GatewayQuotaDecision::Allowed);
        }
        let request_digest = request_digest(charges)?;
        let reservation_key = reservation_key(tenant, reservation_id)?;
        if let Some(existing) = self.adapter.get(&reservation_key).await? {
            return if constant_time_eq(&existing, request_digest.as_bytes()) {
                Ok(GatewayQuotaDecision::Allowed)
            } else {
                Err(GatewayQuotaError::ReservationConflict)
            };
        }

        for attempt in 0..self.config.maximum_conflict_retries.get() {
            let mut states = Vec::new();
            for (charge, limit) in &active {
                if let Some(window) = limit.hard {
                    states.push(
                        self.counter_state(
                            tenant,
                            charge.resource,
                            CounterKind::Hard,
                            window,
                            now_unix_milliseconds,
                        )
                        .await?,
                    );
                }
                if let Some(window) = limit.rate {
                    states.push(
                        self.counter_state(
                            tenant,
                            charge.resource,
                            CounterKind::Rate,
                            window,
                            now_unix_milliseconds,
                        )
                        .await?,
                    );
                }
            }

            for (charge, limit) in &active {
                if let Some(window) = limit.hard
                    && would_exceed(&states, charge, CounterKind::Hard, window)?
                {
                    return Ok(GatewayQuotaDecision::HardLimit {
                        resource: charge.resource,
                    });
                }
            }
            for (charge, limit) in &active {
                if let Some(window) = limit.rate
                    && would_exceed(&states, charge, CounterKind::Rate, window)?
                {
                    let elapsed = now_unix_milliseconds % window.window_milliseconds.get();
                    return Ok(GatewayQuotaDecision::Throttled {
                        resource: charge.resource,
                        retry_after_milliseconds: window.window_milliseconds.get() - elapsed,
                    });
                }
            }

            let mut conditions = Vec::with_capacity(states.len() + 1);
            conditions.push(KeyCondition::Missing {
                key: reservation_key.clone(),
            });
            let mut batch = WriteBatch::with_capacity(states.len() + 1);
            batch.put(&reservation_key, request_digest.as_bytes());
            for state in &mut states {
                let charge = active
                    .iter()
                    .find(|(charge, _)| charge.resource == state.resource)
                    .map(|(charge, _)| *charge)
                    .ok_or(GatewayQuotaError::InvalidPolicy)?;
                state.used = state
                    .used
                    .checked_add(charge.amount.get())
                    .ok_or(GatewayQuotaError::CounterOverflow)?;
                conditions.push(state.condition());
                batch.put(
                    &state.key,
                    serde_json::to_vec(state).map_err(|_| GatewayQuotaError::Serialization)?,
                );
            }
            let write_result = match self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions,
                    batch,
                    durability: self.config.durability,
                })
                .await
            {
                Ok(result) => result,
                Err(error) if error.kind == StorageErrorKind::Conflict && error.retryable => {
                    concurrent_update_backoff(reservation_id, attempt);
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            match write_result {
                CompareAndWriteResult::Applied => return Ok(GatewayQuotaDecision::Allowed),
                CompareAndWriteResult::Conflict {
                    failed_condition: 0,
                    actual_value: Some(actual),
                } if constant_time_eq(&actual, request_digest.as_bytes()) => {
                    return Ok(GatewayQuotaDecision::Allowed);
                }
                CompareAndWriteResult::Conflict {
                    failed_condition: 0,
                    ..
                } => return Err(GatewayQuotaError::ReservationConflict),
                CompareAndWriteResult::Conflict { .. } => {
                    concurrent_update_backoff(reservation_id, attempt);
                    continue;
                }
            }
        }
        Err(GatewayQuotaError::ConcurrentUpdate)
    }

    async fn counter_state(
        &self,
        tenant: &TenantScope,
        resource: GatewayQuotaResource,
        kind: CounterKind,
        window: GatewayQuotaWindow,
        now: u64,
    ) -> Result<CounterState, GatewayQuotaError> {
        let window_start =
            now / window.window_milliseconds.get() * window.window_milliseconds.get();
        let key = counter_key(tenant, resource, kind, window_start)?;
        let existing = self.adapter.get(&key).await?;
        let state = existing
            .as_deref()
            .map(|bytes| {
                serde_json::from_slice::<StoredCounter>(bytes)
                    .map_err(|_| GatewayQuotaError::CorruptStore)
            })
            .transpose()?
            .unwrap_or(StoredCounter {
                resource,
                kind,
                window_start_unix_milliseconds: window_start,
                window_milliseconds: window.window_milliseconds.get(),
                used: 0,
            });
        if state.resource != resource
            || state.kind != kind
            || state.window_start_unix_milliseconds != window_start
            || state.window_milliseconds != window.window_milliseconds.get()
        {
            return Err(GatewayQuotaError::CorruptStore);
        }
        Ok(CounterState {
            key,
            previous: existing,
            resource: state.resource,
            kind: state.kind,
            window_start_unix_milliseconds: state.window_start_unix_milliseconds,
            window_milliseconds: state.window_milliseconds,
            used: state.used,
        })
    }
}

fn concurrent_update_backoff(reservation_id: &str, attempt: usize) {
    let base_microseconds = 50_u64 << attempt.min(5);
    let jitter =
        u64::from(blake3::hash(reservation_id.as_bytes()).as_bytes()[0]) * base_microseconds / 255;
    thread::sleep(Duration::from_micros(base_microseconds + jitter));
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CounterKind {
    Hard,
    Rate,
}

impl CounterKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::Rate => "rate",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredCounter {
    resource: GatewayQuotaResource,
    kind: CounterKind,
    window_start_unix_milliseconds: u64,
    window_milliseconds: u64,
    used: u64,
}

#[derive(Clone, Debug)]
struct CounterState {
    key: Vec<u8>,
    previous: Option<Vec<u8>>,
    resource: GatewayQuotaResource,
    kind: CounterKind,
    window_start_unix_milliseconds: u64,
    window_milliseconds: u64,
    used: u64,
}

impl CounterState {
    fn condition(&self) -> KeyCondition {
        self.previous.as_ref().map_or_else(
            || KeyCondition::Missing {
                key: self.key.clone(),
            },
            |value| KeyCondition::ValueEquals {
                key: self.key.clone(),
                value: value.clone(),
            },
        )
    }
}

impl Serialize for CounterState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        StoredCounter {
            resource: self.resource,
            kind: self.kind,
            window_start_unix_milliseconds: self.window_start_unix_milliseconds,
            window_milliseconds: self.window_milliseconds,
            used: self.used,
        }
        .serialize(serializer)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayQuotaPolicyError {
    Unavailable,
}

impl fmt::Display for GatewayQuotaPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("gateway quota policy is unavailable")
    }
}

impl Error for GatewayQuotaPolicyError {}

#[async_trait]
pub trait GatewayQuotaPolicySource: Send + Sync {
    async fn policy_for(
        &self,
        tenant: &TenantScope,
    ) -> Result<GatewayQuotaPolicy, GatewayQuotaPolicyError>;
}

#[async_trait]
impl GatewayQuotaPolicySource for GatewayQuotaPolicy {
    async fn policy_for(
        &self,
        _tenant: &TenantScope,
    ) -> Result<GatewayQuotaPolicy, GatewayQuotaPolicyError> {
        Ok(self.clone())
    }
}

#[derive(Debug)]
pub enum GatewayQuotaError {
    InvalidPolicy,
    InvalidRequest,
    ReservationConflict,
    CounterOverflow,
    ConcurrentUpdate,
    Serialization,
    CorruptStore,
    Storage(StorageError),
}

impl fmt::Display for GatewayQuotaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPolicy => "gateway quota policy is invalid",
            Self::InvalidRequest => "gateway quota request is invalid",
            Self::ReservationConflict => "gateway quota reservation identifier was reused",
            Self::CounterOverflow => "gateway quota counter exceeded its supported range",
            Self::ConcurrentUpdate => "gateway quota counters changed concurrently",
            Self::Serialization => "gateway quota state serialization failed",
            Self::CorruptStore => "gateway quota storage contains invalid state",
            Self::Storage(_) => "gateway quota storage operation failed",
        })
    }
}

impl Error for GatewayQuotaError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StorageError> for GatewayQuotaError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

fn validate_request(
    reservation_id: &str,
    charges: &[GatewayQuotaCharge],
    maximum_charges: NonZeroUsize,
    now: u64,
) -> Result<(), GatewayQuotaError> {
    let unique = charges
        .iter()
        .map(|charge| charge.resource)
        .collect::<BTreeSet<_>>();
    if now == 0
        || reservation_id.len() < 8
        || reservation_id.len() > 128
        || reservation_id
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'-' | b'.'))
        || charges.is_empty()
        || charges.len() > maximum_charges.get()
        || unique.len() != charges.len()
    {
        return Err(GatewayQuotaError::InvalidRequest);
    }
    Ok(())
}

fn would_exceed(
    states: &[CounterState],
    charge: &GatewayQuotaCharge,
    kind: CounterKind,
    window: GatewayQuotaWindow,
) -> Result<bool, GatewayQuotaError> {
    let state = states
        .iter()
        .find(|state| state.resource == charge.resource && state.kind == kind)
        .ok_or(GatewayQuotaError::InvalidPolicy)?;
    Ok(state
        .used
        .checked_add(charge.amount.get())
        .is_none_or(|prospective| prospective > window.limit.get()))
}

fn request_digest(charges: &[GatewayQuotaCharge]) -> Result<String, GatewayQuotaError> {
    #[derive(Serialize)]
    struct DigestInput {
        charges: Vec<(GatewayQuotaResource, u64)>,
    }
    let bytes = serde_json::to_vec(&DigestInput {
        charges: charges
            .iter()
            .map(|charge| (charge.resource, charge.amount.get()))
            .collect(),
    })
    .map_err(|_| GatewayQuotaError::Serialization)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn counter_key(
    tenant: &TenantScope,
    resource: GatewayQuotaResource,
    kind: CounterKind,
    window_start: u64,
) -> Result<Vec<u8>, GatewayQuotaError> {
    TenantKeyspace::system_key(
        tenant_domain(COUNTER_DOMAIN_PREFIX, tenant),
        format!("{}/{}/{window_start:020}", resource.as_str(), kind.as_str()),
    )
    .map_err(|_| GatewayQuotaError::InvalidRequest)
}

fn reservation_key(
    tenant: &TenantScope,
    reservation_id: &str,
) -> Result<Vec<u8>, GatewayQuotaError> {
    TenantKeyspace::system_key(
        tenant_domain(RESERVATION_DOMAIN_PREFIX, tenant),
        reservation_id,
    )
    .map_err(|_| GatewayQuotaError::InvalidRequest)
}

fn tenant_domain(prefix: &str, tenant: &TenantScope) -> Vec<u8> {
    format!(
        "{prefix}:{}:{}",
        tenant.project_id(),
        tenant.environment_id()
    )
    .into_bytes()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use futures::{executor::block_on, future::join_all};
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::{MemoryAdapter, RocksDbAdapter, RocksDbConfig};
    use proptest::prelude::*;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn hard_limits_rate_limits_and_retry_advice_are_stable() {
        block_on(async {
            let engine = engine();
            let tenant = quota_tenant("example00", "example00");
            let policy = GatewayQuotaPolicy::new([(
                GatewayQuotaResource::ReplicationRequests,
                GatewayQuotaLimit {
                    hard: Some(window(3, 10_000)),
                    rate: Some(window(2, 1_000)),
                },
            )])
            .expect("policy");
            let charge = [charge(GatewayQuotaResource::ReplicationRequests, 1)];
            for id in ["req_quota001", "req_quota002"] {
                assert_eq!(
                    engine
                        .check_and_reserve(&tenant, id, &charge, &policy, 1_100)
                        .await
                        .expect("decision"),
                    GatewayQuotaDecision::Allowed
                );
            }
            assert_eq!(
                engine
                    .check_and_reserve(&tenant, "req_quota003", &charge, &policy, 1_100)
                    .await
                    .expect("throttle"),
                GatewayQuotaDecision::Throttled {
                    resource: GatewayQuotaResource::ReplicationRequests,
                    retry_after_milliseconds: 900,
                }
            );
            assert_eq!(
                engine
                    .check_and_reserve(&tenant, "req_quota003", &charge, &policy, 2_100)
                    .await
                    .expect("new window"),
                GatewayQuotaDecision::Allowed
            );
            assert_eq!(
                engine
                    .check_and_reserve(&tenant, "req_quota004", &charge, &policy, 2_200)
                    .await
                    .expect("hard limit"),
                GatewayQuotaDecision::HardLimit {
                    resource: GatewayQuotaResource::ReplicationRequests,
                }
            );
            assert_eq!(
                engine
                    .check_and_reserve(&tenant, "req_quota001", &charge, &policy, 9_900)
                    .await
                    .expect("idempotent retry"),
                GatewayQuotaDecision::Allowed
            );
        });
    }

    #[test]
    fn multi_resource_denial_never_partially_reserves() {
        block_on(async {
            let engine = engine();
            let tenant = quota_tenant("example00", "example00");
            let policy = GatewayQuotaPolicy::new([
                (
                    GatewayQuotaResource::FunctionInvocations,
                    GatewayQuotaLimit {
                        hard: Some(window(2, 10_000)),
                        rate: None,
                    },
                ),
                (
                    GatewayQuotaResource::PublicFunctionInvocations,
                    GatewayQuotaLimit {
                        hard: None,
                        rate: Some(window(1, 1_000)),
                    },
                ),
            ])
            .expect("policy");
            let public = [
                charge(GatewayQuotaResource::FunctionInvocations, 1),
                charge(GatewayQuotaResource::PublicFunctionInvocations, 1),
            ];
            let private = [charge(GatewayQuotaResource::FunctionInvocations, 1)];
            assert_eq!(
                engine
                    .check_and_reserve(&tenant, "req_public01", &public, &policy, 1_100)
                    .await
                    .expect("public"),
                GatewayQuotaDecision::Allowed
            );
            assert!(matches!(
                engine
                    .check_and_reserve(&tenant, "req_public02", &public, &policy, 1_200)
                    .await
                    .expect("throttled"),
                GatewayQuotaDecision::Throttled { .. }
            ));
            assert_eq!(
                engine
                    .check_and_reserve(&tenant, "req_private1", &private, &policy, 1_300)
                    .await
                    .expect("private"),
                GatewayQuotaDecision::Allowed
            );
            assert!(matches!(
                engine
                    .check_and_reserve(&tenant, "req_private2", &private, &policy, 1_400)
                    .await
                    .expect("hard"),
                GatewayQuotaDecision::HardLimit { .. }
            ));
        });
    }

    #[test]
    fn reservation_conflicts_and_tenants_fail_safe() {
        block_on(async {
            let engine = engine();
            let tenant = quota_tenant("example00", "example00");
            let other = quota_tenant("other000", "other000");
            let policy = GatewayQuotaPolicy::new([(
                GatewayQuotaResource::EgressBytes,
                GatewayQuotaLimit {
                    hard: Some(window(1, 10_000)),
                    rate: None,
                },
            )])
            .expect("policy");
            assert_eq!(
                engine
                    .check_and_reserve(
                        &tenant,
                        "req_shared00",
                        &[charge(GatewayQuotaResource::EgressBytes, 1)],
                        &policy,
                        1_000,
                    )
                    .await
                    .expect("allowed"),
                GatewayQuotaDecision::Allowed
            );
            assert!(matches!(
                engine
                    .check_and_reserve(
                        &tenant,
                        "req_shared00",
                        &[charge(GatewayQuotaResource::EgressBytes, 2)],
                        &policy,
                        1_000,
                    )
                    .await,
                Err(GatewayQuotaError::ReservationConflict)
            ));
            assert_eq!(
                engine
                    .check_and_reserve(
                        &other,
                        "req_shared00",
                        &[charge(GatewayQuotaResource::EgressBytes, 1)],
                        &policy,
                        1_000,
                    )
                    .await
                    .expect("other tenant"),
                GatewayQuotaDecision::Allowed
            );
        });
    }

    #[test]
    fn concurrent_admission_never_oversubscribes() {
        block_on(async {
            let engine = engine();
            let tenant = quota_tenant("example00", "example00");
            let policy = GatewayQuotaPolicy::new([(
                GatewayQuotaResource::FunctionInvocations,
                GatewayQuotaLimit {
                    hard: Some(window(10, 10_000)),
                    rate: None,
                },
            )])
            .expect("policy");
            let charge = [charge(GatewayQuotaResource::FunctionInvocations, 1)];
            let results = join_all((0..20).map(|index| {
                let id = format!("req_concurrent{index:02}");
                let engine = &engine;
                let tenant = &tenant;
                let charge = &charge;
                let policy = &policy;
                async move {
                    engine
                        .check_and_reserve(tenant, &id, charge, policy, 1_000)
                        .await
                }
            }))
            .await;
            let allowed = results
                .iter()
                .filter(|result| matches!(result, Ok(GatewayQuotaDecision::Allowed)))
                .count();
            let denied = results
                .iter()
                .filter(|result| matches!(result, Ok(GatewayQuotaDecision::HardLimit { .. })))
                .count();
            assert_eq!(allowed, 10);
            assert_eq!(denied, 10);
        });
    }

    #[test]
    fn optimistic_rocksdb_commit_conflicts_are_retried() {
        let temporary = TempDir::new().expect("temporary RocksDB path");
        let engine = GatewayQuotaEngine::new(
            Arc::new(RocksDbAdapter::open(RocksDbConfig::new(temporary.path())).expect("RocksDB")),
            GatewayQuotaEngineConfig {
                durability: Durability::Sync,
                maximum_conflict_retries: NonZeroUsize::new(128).expect("non-zero"),
                maximum_charges_per_request: NonZeroUsize::new(8).expect("non-zero"),
            },
        );
        let tenant = quota_tenant("rocksdb00", "rocksdb00");
        let policy = GatewayQuotaPolicy::new([(
            GatewayQuotaResource::AuthenticationRequests,
            GatewayQuotaLimit {
                hard: Some(window(64, 10_000)),
                rate: None,
            },
        )])
        .expect("policy");
        let barrier = Arc::new(Barrier::new(16));
        let workers = (0..16)
            .map(|index| {
                let engine = engine.clone();
                let tenant = tenant.clone();
                let policy = policy.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    block_on(engine.check_and_reserve(
                        &tenant,
                        &format!("req_rocksdb{index:02}"),
                        &[charge(GatewayQuotaResource::AuthenticationRequests, 1)],
                        &policy,
                        1_000,
                    ))
                })
            })
            .collect::<Vec<_>>();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("quota worker"))
            .collect::<Vec<_>>();
        assert!(
            results
                .iter()
                .all(|result| matches!(result, Ok(GatewayQuotaDecision::Allowed))),
            "all RocksDB-backed concurrent reservations should eventually commit: {results:?}"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arbitrary_tenants_never_share_gateway_quota_reservations(
            project_a in "[a-z0-9_-]{8,24}",
            environment_a in "[a-z0-9_-]{8,24}",
            project_b in "[a-z0-9_-]{8,24}",
            environment_b in "[a-z0-9_-]{8,24}",
        ) {
            prop_assume!(project_a != project_b || environment_a != environment_b);
            block_on(async {
                let engine = engine();
                let first = quota_tenant(&project_a, &environment_a);
                let second = quota_tenant(&project_b, &environment_b);
                let policy = GatewayQuotaPolicy::new([(
                    GatewayQuotaResource::ReplicationRequests,
                    GatewayQuotaLimit {
                        hard: Some(window(1, 10_000)),
                        rate: None,
                    },
                )])
                .expect("policy");
                let charge = [charge(GatewayQuotaResource::ReplicationRequests, 1)];
                prop_assert_eq!(
                    engine
                        .check_and_reserve(&first, "req_boundary0", &charge, &policy, 1_000)
                        .await
                        .expect("first tenant"),
                    GatewayQuotaDecision::Allowed,
                );
                prop_assert_eq!(
                    engine
                        .check_and_reserve(&second, "req_boundary0", &charge, &policy, 1_000)
                        .await
                        .expect("second tenant"),
                    GatewayQuotaDecision::Allowed,
                );
                Ok(())
            })?;
        }
    }

    fn engine() -> GatewayQuotaEngine {
        GatewayQuotaEngine::new(
            Arc::new(MemoryAdapter::new()),
            GatewayQuotaEngineConfig {
                durability: Durability::Memory,
                maximum_conflict_retries: NonZeroUsize::new(64).expect("non-zero"),
                maximum_charges_per_request: NonZeroUsize::new(8).expect("non-zero"),
            },
        )
    }

    fn window(limit: u64, width: u64) -> GatewayQuotaWindow {
        GatewayQuotaWindow {
            limit: NonZeroU64::new(limit).expect("non-zero"),
            window_milliseconds: NonZeroU64::new(width).expect("non-zero"),
        }
    }

    fn charge(resource: GatewayQuotaResource, amount: u64) -> GatewayQuotaCharge {
        GatewayQuotaCharge {
            resource,
            amount: NonZeroU64::new(amount).expect("non-zero"),
        }
    }

    fn quota_tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(format!("prj_{project}")).expect("project"),
            EnvironmentId::parse(format!("env_{environment}")).expect("environment"),
        )
    }
}
