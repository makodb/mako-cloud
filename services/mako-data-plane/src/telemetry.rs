//! Decide when a tenant's stored size is worth measuring again.
//!
//! Shipping records is shared with every other service and lives in
//! `mako-telemetry-client`. What is specific to this plane is the storage it
//! owns, and knowing when measuring it is worth the walk.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use mako_api::{
    EnvironmentId, ObservabilityPayload, ObservabilityRecord, ProjectId,
    QUOTA_CHECKPOINT_WINDOW_MILLISECONDS, QuotaResource, TenantScope,
};
use mako_gateway::{GatewayQuotaEngine, GatewayQuotaPolicySource, GatewayQuotaResource};
use mako_storage::{KvAdapter, TenantKeyspace};
use mako_telemetry_client::TelemetryEmitter;

/// How often a tenant's stored size is re-measured.
///
/// Stored size only moves when a tenant writes, so nothing is gained by
/// sampling one that is idle: its last sample is still true. Writes mark a
/// tenant due and the sample is taken off the request path, because measuring
/// walks the tenant's range and a document write must not pay for that.
const STORAGE_SAMPLE_INTERVAL: Duration = Duration::from_secs(300);
/// Bounded so a burst across many tenants cannot grow this without limit.
const MAX_PENDING_SAMPLES: usize = 4_096;

/// What a tenant can be due to have re-measured.
///
/// Both are levels rather than counts of events, so both are sampled rather
/// than emitted per request, and each is marked by the events that can change
/// it: writes move stored size, and identity lifecycle moves the user count.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Measurement {
    StoredBytes,
    ApplicationUsers,
}

/// One tenant, identified the way its keyspace is.
type TenantKey = (String, String);
/// What is due for a tenant, and when it was noticed.
type SampleKey = (Measurement, TenantKey);

pub struct StorageSampler {
    region: String,
    due: Mutex<HashMap<SampleKey, Instant>>,
    last_sampled: Mutex<HashMap<SampleKey, Instant>>,
}

impl StorageSampler {
    #[must_use]
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            region: region.into(),
            due: Mutex::new(HashMap::new()),
            last_sampled: Mutex::new(HashMap::new()),
        }
    }

    /// Note that a tenant's stored size may have changed.
    pub fn mark(&self, tenant: &TenantScope) {
        self.mark_measurement(Measurement::StoredBytes, tenant);
    }

    /// Note that a tenant's application-user count may have changed.
    pub fn mark_users(&self, tenant: &TenantScope) {
        self.mark_measurement(Measurement::ApplicationUsers, tenant);
    }

    fn mark_measurement(&self, measurement: Measurement, tenant: &TenantScope) {
        let key = (measurement, tenant_key(tenant));
        if let Ok(last) = self.last_sampled.lock()
            && last
                .get(&key)
                .is_some_and(|at| at.elapsed() < STORAGE_SAMPLE_INTERVAL)
        {
            return;
        }
        let Ok(mut due) = self.due.lock() else {
            return;
        };
        if due.len() >= MAX_PENDING_SAMPLES {
            return;
        }
        due.entry(key).or_insert_with(Instant::now);
    }

    fn take_due(&self) -> Vec<SampleKey> {
        let Ok(mut due) = self.due.lock() else {
            return Vec::new();
        };
        due.drain().map(|(key, _)| key).collect()
    }

    fn record_sampled(&self, key: SampleKey) {
        if let Ok(mut last) = self.last_sampled.lock() {
            last.insert(key, Instant::now());
        }
    }

    /// Measure every tenant marked since the last pass and report each as
    /// usage. Returns how many were sampled.
    pub async fn sample_due(
        &self,
        adapter: &Arc<dyn KvAdapter>,
        emitter: &TelemetryEmitter,
        now_unix_seconds: u64,
    ) -> usize {
        let mut sampled = 0;
        for key in self.take_due() {
            let (measurement, tenant) = key.clone();
            let Ok(keyspace) = TenantKeyspace::new(tenant.0.as_bytes(), tenant.1.as_bytes()) else {
                continue;
            };
            let measured = match measurement {
                Measurement::StoredBytes => match keyspace.tenant_range() {
                    Ok(range) => adapter.stored_bytes(range).await.ok(),
                    Err(_) => None,
                },
                // One entry per user, so counting the range counts the users.
                Measurement::ApplicationUsers => match keyspace.normalized_email_owners_range() {
                    Ok(range) => adapter.count_keys(range).await.ok(),
                    Err(_) => None,
                },
            };
            let (Ok(project), Ok(environment)) = (
                ProjectId::parse(tenant.0.clone()),
                EnvironmentId::parse(tenant.1.clone()),
            ) else {
                continue;
            };
            let scope = TenantScope::new(project.clone(), environment.clone());
            // Measuring is also observing: a tenant whose storage answered is
            // healthy at this moment, and one whose storage did not is what a
            // developer asking "is my project healthy" needs to see.
            emitter.record(ObservabilityRecord {
                tenant: scope.clone(),
                timestamp_unix_milliseconds: now_unix_seconds.saturating_mul(1_000),
                payload: ObservabilityPayload::Health {
                    service: "mako-data-plane".to_owned(),
                    region: self.region.clone(),
                    status: if measured.is_some() {
                        mako_api::HealthState::Healthy
                    } else {
                        mako_api::HealthState::Degraded
                    },
                    diagnostic: measured
                        .is_none()
                        .then(|| "storage measurement failed".to_owned()),
                },
            });
            let Some(quantity) = measured else {
                // Left unmarked, so the next change marks it again rather than
                // recording a sample that never happened.
                continue;
            };
            let (resource, unit) = match measurement {
                Measurement::StoredBytes => (QuotaResource::StorageBytes, "bytes"),
                Measurement::ApplicationUsers => (QuotaResource::ApplicationUsers, "users"),
            };
            emitter.record(ObservabilityRecord {
                tenant: scope,
                timestamp_unix_milliseconds: now_unix_seconds.saturating_mul(1_000),
                payload: ObservabilityPayload::Usage {
                    resource,
                    quantity,
                    unit: unit.to_owned(),
                },
            });
            self.record_sampled(key);
            sampled += 1;
        }
        sampled
    }
}

fn tenant_key(tenant: &TenantScope) -> TenantKey {
    (
        tenant.project_id().as_str().to_owned(),
        tenant.environment_id().as_str().to_owned(),
    )
}

/// A checkpoint is only taken once a window has been closed this long, so a
/// request that read its clock just before the boundary has finished emitting
/// its usage record before the window it charged is summarized.
const QUOTA_CHECKPOINT_SETTLE: u64 = 10_000;
/// A window nobody managed to checkpoint within this horizon is abandoned:
/// the cross-check samples, it does not owe completeness.
const QUOTA_CHECKPOINT_HORIZON: u64 = 10 * 60_000;
/// Bounded so a burst across many tenants cannot grow this without limit.
const MAX_TRACKED_QUOTA_TENANTS: usize = 4_096;
/// Windows per tenant awaiting checkpoint; passes run every couple of
/// seconds, so more than a handful pending means the worker is stalled.
const MAX_PENDING_QUOTA_WINDOWS: usize = 8;

/// The metered resources whose quota counters are worth cross-checking: the
/// two replication charges, because they are exactly what the usage ledger
/// records per admitted request under these product names.
const CHECKPOINTED_RESOURCES: [(GatewayQuotaResource, QuotaResource); 2] = [
    (
        GatewayQuotaResource::ReplicationRequests,
        QuotaResource::ReplicationRequestsPerMinute,
    ),
    (
        GatewayQuotaResource::ReplicationBytes,
        QuotaResource::ReplicationBytesPerMonth,
    ),
];

/// Report what enforcement counted, so the ledger can be checked against it.
///
/// The usage ledger and the quota counters observe the same admitted requests
/// through different mechanisms: the counter is written transactionally with
/// admission, the ledger travels a bounded buffer that sheds under pressure.
/// If they disagree materially, one of billing or enforcement is wrong. This
/// side reads the counters for each closed minute a tenant was active in and
/// reports them as `Quota` records; the ledger's holder does the comparing.
pub struct QuotaCheckpointer {
    pending: Mutex<HashMap<TenantKey, BTreeSet<u64>>>,
}

impl QuotaCheckpointer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Note that an admitted request charged this tenant's counters at this
    /// instant, so the minute it landed in becomes worth summarizing.
    pub fn mark(&self, tenant: &TenantScope, now_unix_milliseconds: u64) {
        let window = now_unix_milliseconds / QUOTA_CHECKPOINT_WINDOW_MILLISECONDS
            * QUOTA_CHECKPOINT_WINDOW_MILLISECONDS;
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        let key = tenant_key(tenant);
        if !pending.contains_key(&key) && pending.len() >= MAX_TRACKED_QUOTA_TENANTS {
            return;
        }
        let windows = pending.entry(key).or_default();
        if windows.len() >= MAX_PENDING_QUOTA_WINDOWS {
            // Keep the newest windows: the oldest is the closest to falling
            // off the horizon unemitted anyway.
            windows.pop_first();
        }
        windows.insert(window);
    }

    fn take_closed(&self, now_unix_milliseconds: u64) -> Vec<(TenantKey, Vec<u64>)> {
        let Ok(mut pending) = self.pending.lock() else {
            return Vec::new();
        };
        let mut closed = Vec::new();
        pending.retain(|tenant, windows| {
            let mut due = Vec::new();
            windows.retain(|window| {
                let closed_at = window.saturating_add(QUOTA_CHECKPOINT_WINDOW_MILLISECONDS);
                if now_unix_milliseconds < closed_at.saturating_add(QUOTA_CHECKPOINT_SETTLE) {
                    return true;
                }
                if now_unix_milliseconds < window.saturating_add(QUOTA_CHECKPOINT_HORIZON) {
                    due.push(*window);
                }
                false
            });
            if !due.is_empty() {
                closed.push((tenant.clone(), due));
            }
            !windows.is_empty()
        });
        closed
    }

    /// Summarize every settled window a marked tenant charged, as one `Quota`
    /// record per cross-checked resource, timestamped at the window's start.
    /// Returns how many records were emitted.
    pub async fn checkpoint_due(
        &self,
        engine: &GatewayQuotaEngine,
        policies: &dyn GatewayQuotaPolicySource,
        emitter: &TelemetryEmitter,
        now_unix_milliseconds: u64,
    ) -> usize {
        let mut emitted = 0;
        for ((project, environment), windows) in self.take_closed(now_unix_milliseconds) {
            let (Ok(project), Ok(environment)) =
                (ProjectId::parse(project), EnvironmentId::parse(environment))
            else {
                continue;
            };
            let tenant = TenantScope::new(project, environment);
            // The tenant's installed policy names the limit the counters were
            // held to; without it the counter cannot be read, because the
            // window length is part of the counter's identity.
            let Ok(policy) = policies.policy_for(&tenant).await else {
                continue;
            };
            for record in checkpoint_records(engine, &policy, &tenant, &windows).await {
                emitter.record(record);
                emitted += 1;
            }
        }
        emitted
    }
}

/// One `Quota` record per cross-checked resource per window, carrying what
/// the counter held. Separate from the emitting loop so a test can look at
/// the records themselves rather than count buffer entries.
async fn checkpoint_records(
    engine: &GatewayQuotaEngine,
    policy: &mako_gateway::GatewayQuotaPolicy,
    tenant: &TenantScope,
    windows: &[u64],
) -> Vec<ObservabilityRecord> {
    let mut records = Vec::new();
    for window_start in windows {
        for (gateway_resource, resource) in CHECKPOINTED_RESOURCES {
            let Some(rate) = policy.limit(gateway_resource).and_then(|limit| limit.rate) else {
                continue;
            };
            if rate.window_milliseconds.get() != QUOTA_CHECKPOINT_WINDOW_MILLISECONDS {
                continue;
            }
            let Ok(consumed) = engine
                .rate_counter_used(tenant, gateway_resource, rate, *window_start)
                .await
            else {
                continue;
            };
            records.push(ObservabilityRecord {
                tenant: tenant.clone(),
                timestamp_unix_milliseconds: *window_start,
                payload: ObservabilityPayload::Quota {
                    resource,
                    limit: rate.limit.get(),
                    consumed,
                    retry_after_unix_milliseconds: None,
                },
            });
        }
    }
    records
}

impl Default for QuotaCheckpointer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU64, NonZeroUsize};

    use futures::executor::block_on;
    use mako_gateway::{
        GatewayQuotaCharge, GatewayQuotaEngineConfig, GatewayQuotaLimit, GatewayQuotaPolicy,
        GatewayQuotaWindow, PersistentQuotaPolicySource,
    };
    use mako_storage::{Durability, MemoryAdapter, WriteBatch};

    use super::*;

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_sampler0001").expect("project"),
            EnvironmentId::parse("env_sampler0001").expect("environment"),
        )
    }

    fn emitter() -> TelemetryEmitter {
        // Never flushed in these tests, so the endpoint is never contacted.
        TelemetryEmitter::new(
            "127.0.0.1:1".parse().expect("address"),
            "0123456789abcdef0123456789abcdef",
            "mako.test",
        )
    }

    #[test]
    fn stored_size_and_user_count_are_marked_and_sampled_independently() {
        block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let sampler = StorageSampler::new("local");
            let emitter = emitter();

            // A write says nothing about how many users a tenant has, and a
            // signup says nothing about how much it stores, so one must not
            // schedule the other's measurement.
            sampler.mark(&tenant());
            assert_eq!(sampler.sample_due(&adapter, &emitter, 1).await, 1);
            // Each sample carries a health record with its measurement.
            assert_eq!(emitter.buffered(), 2);

            sampler.mark_users(&tenant());
            assert_eq!(
                sampler.sample_due(&adapter, &emitter, 2).await,
                1,
                "a user-count change was not measured because storage had just been"
            );
            assert_eq!(emitter.buffered(), 4);
        });
    }

    #[test]
    fn a_marked_tenant_is_sampled_once_and_not_again_until_the_interval_passes() {
        block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let keyspace = TenantKeyspace::new(
                tenant().project_id().as_str().as_bytes(),
                tenant().environment_id().as_str().as_bytes(),
            )
            .expect("keyspace");
            let mut batch = WriteBatch::new();
            batch.put(
                keyspace.tenant_range().expect("range").start_inclusive,
                b"x",
            );
            adapter
                .write(batch, Durability::Memory)
                .await
                .expect("write");

            let sampler = StorageSampler::new("local");
            let emitter = emitter();

            // Nothing is measured until a write says the size may have moved.
            assert_eq!(sampler.sample_due(&adapter, &emitter, 1).await, 0);

            sampler.mark(&tenant());
            assert_eq!(sampler.sample_due(&adapter, &emitter, 1).await, 1);
            // Each sample carries a health record with its measurement.
            assert_eq!(emitter.buffered(), 2);

            // A tenant that keeps writing is not re-measured on every write:
            // measuring walks its range, and the answer barely moves.
            sampler.mark(&tenant());
            sampler.mark(&tenant());
            assert_eq!(
                sampler.sample_due(&adapter, &emitter, 2).await,
                0,
                "a tenant was re-measured inside the sampling interval"
            );
            // Each sample carries a health record with its measurement.
            assert_eq!(emitter.buffered(), 2);
        });
    }

    fn quota_engine(adapter: &Arc<dyn KvAdapter>) -> GatewayQuotaEngine {
        GatewayQuotaEngine::new(
            Arc::clone(adapter),
            GatewayQuotaEngineConfig {
                durability: Durability::Memory,
                maximum_conflict_retries: NonZeroUsize::new(8).expect("non-zero"),
                maximum_charges_per_request: NonZeroUsize::new(8).expect("non-zero"),
            },
        )
    }

    fn replication_policy() -> GatewayQuotaPolicy {
        let rate = |limit: u64| GatewayQuotaLimit {
            hard: None,
            rate: Some(GatewayQuotaWindow {
                limit: NonZeroU64::new(limit).expect("non-zero"),
                window_milliseconds: NonZeroU64::new(QUOTA_CHECKPOINT_WINDOW_MILLISECONDS)
                    .expect("non-zero"),
            }),
        };
        GatewayQuotaPolicy::new([
            (GatewayQuotaResource::ReplicationRequests, rate(120)),
            (
                GatewayQuotaResource::ReplicationBytes,
                rate(64 * 1024 * 1024),
            ),
        ])
        .expect("policy")
    }

    /// The checkpoint must say exactly what the counters hold for the window
    /// it names, and must not be taken before the window has settled: a
    /// summary racing the requests it summarizes would cry divergence where
    /// there is none.
    #[test]
    fn a_charged_window_is_checkpointed_after_it_settles_with_what_the_counter_holds() {
        block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let engine = quota_engine(&adapter);
            let policies = PersistentQuotaPolicySource::new(
                Arc::clone(&adapter),
                replication_policy(),
                Duration::from_secs(5),
            );
            let emitter = emitter();
            let checkpoints = QuotaCheckpointer::new();

            // Two admitted requests inside the same minute.
            for (reservation, at, bytes) in
                [("req_qcheck01", 60_000, 300), ("req_qcheck02", 90_000, 200)]
            {
                engine
                    .check_and_reserve(
                        &tenant(),
                        reservation,
                        &[
                            GatewayQuotaCharge {
                                resource: GatewayQuotaResource::ReplicationRequests,
                                amount: NonZeroU64::MIN,
                            },
                            GatewayQuotaCharge {
                                resource: GatewayQuotaResource::ReplicationBytes,
                                amount: NonZeroU64::new(bytes).expect("non-zero"),
                            },
                        ],
                        &replication_policy(),
                        at,
                    )
                    .await
                    .expect("charge");
                checkpoints.mark(&tenant(), at);
            }

            // The window closes at 120s but must settle before it is read.
            assert_eq!(
                checkpoints
                    .checkpoint_due(&engine, &policies, &emitter, 125_000)
                    .await,
                0,
                "a window was summarized before requests straddling its edge could land"
            );

            let records =
                checkpoint_records(&engine, &replication_policy(), &tenant(), &[60_000]).await;
            let consumed: Vec<_> = records
                .iter()
                .map(|record| match record.payload {
                    ObservabilityPayload::Quota {
                        resource, consumed, ..
                    } => (resource, consumed, record.timestamp_unix_milliseconds),
                    _ => panic!("a checkpoint emitted something other than a quota record"),
                })
                .collect();
            assert_eq!(
                consumed,
                vec![
                    (QuotaResource::ReplicationRequestsPerMinute, 2, 60_000),
                    (QuotaResource::ReplicationBytesPerMonth, 500, 60_000),
                ],
                "the checkpoint does not carry what enforcement counted"
            );

            assert_eq!(
                checkpoints
                    .checkpoint_due(&engine, &policies, &emitter, 130_000)
                    .await,
                2,
                "a settled window was not summarized"
            );
            assert_eq!(emitter.buffered(), 2);
            // Once summarized, the window is done; nothing repeats it.
            assert_eq!(
                checkpoints
                    .checkpoint_due(&engine, &policies, &emitter, 200_000)
                    .await,
                0
            );
        });
    }

    /// A window nobody summarized in time is abandoned rather than reported
    /// arbitrarily late: the cross-check samples recent minutes, and a stale
    /// summary would race the ledger's retention.
    #[test]
    fn a_window_past_the_horizon_is_dropped_without_a_checkpoint() {
        block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let engine = quota_engine(&adapter);
            let policies = PersistentQuotaPolicySource::new(
                Arc::clone(&adapter),
                replication_policy(),
                Duration::from_secs(5),
            );
            let emitter = emitter();
            let checkpoints = QuotaCheckpointer::new();

            checkpoints.mark(&tenant(), 60_000);
            assert_eq!(
                checkpoints
                    .checkpoint_due(
                        &engine,
                        &policies,
                        &emitter,
                        60_000 + QUOTA_CHECKPOINT_HORIZON
                    )
                    .await,
                0,
                "a stale window was summarized anyway"
            );
            assert_eq!(emitter.buffered(), 0);
        });
    }
}
