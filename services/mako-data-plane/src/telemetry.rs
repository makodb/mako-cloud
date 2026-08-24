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

pub struct StorageSampler {
    due: Mutex<HashMap<(String, String), Instant>>,
    last_sampled: Mutex<HashMap<(String, String), Instant>>,
}

impl Default for StorageSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageSampler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            due: Mutex::new(HashMap::new()),
            last_sampled: Mutex::new(HashMap::new()),
        }
    }

    /// Note that a tenant's stored size may have changed.
    pub fn mark(&self, tenant: &TenantScope) {
        let key = tenant_key(tenant);
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

    fn take_due(&self) -> Vec<(String, String)> {
        let Ok(mut due) = self.due.lock() else {
            return Vec::new();
        };
        due.drain().map(|(key, _)| key).collect()
    }

    fn record_sampled(&self, key: (String, String)) {
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
            let Ok(keyspace) = TenantKeyspace::new(key.0.as_bytes(), key.1.as_bytes()) else {
                continue;
            };
            let Ok(range) = keyspace.tenant_range() else {
                continue;
            };
            let Ok(bytes) = adapter.stored_bytes(range).await else {
                // Leave it unmarked so the next write marks it again rather
                // than recording a sample that never happened.
                continue;
            };
            let (Ok(project), Ok(environment)) = (
                ProjectId::parse(key.0.clone()),
                EnvironmentId::parse(key.1.clone()),
            ) else {
                continue;
            };
            emitter.record(ObservabilityRecord {
                tenant: TenantScope::new(project, environment),
                timestamp_unix_milliseconds: now_unix_seconds.saturating_mul(1_000),
                payload: ObservabilityPayload::Usage {
                    resource: QuotaResource::StorageBytes,
                    quantity: bytes,
                    unit: "bytes".to_owned(),
                },
            });
            self.record_sampled(key);
            sampled += 1;
        }
        sampled
    }
}

fn tenant_key(tenant: &TenantScope) -> (String, String) {
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

            let sampler = StorageSampler::new();
            let emitter = emitter();

            // Nothing is measured until a write says the size may have moved.
            assert_eq!(sampler.sample_due(&adapter, &emitter, 1).await, 0);

            sampler.mark(&tenant());
            assert_eq!(sampler.sample_due(&adapter, &emitter, 1).await, 1);
            assert_eq!(emitter.buffered(), 1);

            // A tenant that keeps writing is not re-measured on every write:
            // measuring walks its range, and the answer barely moves.
            sampler.mark(&tenant());
            sampler.mark(&tenant());
            assert_eq!(
                sampler.sample_due(&adapter, &emitter, 2).await,
                0,
                "a tenant was re-measured inside the sampling interval"
            );
            assert_eq!(emitter.buffered(), 1);
        });
    }
}
