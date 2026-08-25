//! Decide when a tenant's stored size is worth measuring again.
//!
//! Shipping records is shared with every other service and lives in
//! `mako-telemetry-client`. What is specific to this plane is the storage it
//! owns, and knowing when measuring it is worth the walk.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use mako_api::{
    EnvironmentId, ObservabilityPayload, ObservabilityRecord, ProjectId, QuotaResource, TenantScope,
};
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

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
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
}
