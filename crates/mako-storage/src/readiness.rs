use std::fmt;

use crate::{
    AdapterCapabilities, Capability, Durability, HealthReport, HealthStatus, KvAdapter,
    StorageErrorKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessStatus {
    Ready,
    NotReady,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadinessFailure {
    MissingCapability(Capability),
    HealthCheckFailed {
        kind: StorageErrorKind,
        retryable: bool,
    },
    HealthNotHealthy(HealthStatus),
    InsufficientDurability {
        required: Durability,
        strongest: Durability,
    },
    DurabilityNotVerified,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageReadiness {
    pub status: ReadinessStatus,
    pub capabilities: AdapterCapabilities,
    pub health: Option<HealthReport>,
    pub required_durability: Durability,
    pub failures: Vec<ReadinessFailure>,
}

impl StorageReadiness {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status == ReadinessStatus::Ready
    }
}

impl fmt::Display for StorageReadiness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_ready() {
            write!(
                formatter,
                "storage ready: durability={:?} verified=true capabilities={:?}",
                self.capabilities.strongest_durability,
                self.capabilities.supported()
            )
        } else {
            write!(formatter, "storage not ready")?;
            for failure in &self.failures {
                write!(formatter, "; {}", failure_message(failure))?;
            }
            Ok(())
        }
    }
}

/// Evaluates every semantic requirement before data-plane traffic is enabled.
pub async fn check_storage_readiness(
    adapter: &dyn KvAdapter,
    required_durability: Durability,
) -> StorageReadiness {
    let capabilities = adapter.capabilities();
    let mut failures = capabilities
        .missing_document_engine_requirements()
        .into_iter()
        .map(ReadinessFailure::MissingCapability)
        .collect::<Vec<_>>();

    if capabilities.strongest_durability < required_durability {
        failures.push(ReadinessFailure::InsufficientDurability {
            required: required_durability,
            strongest: capabilities.strongest_durability,
        });
    }

    let health = match adapter.health().await {
        Ok(health) => {
            if health.status != HealthStatus::Healthy {
                failures.push(ReadinessFailure::HealthNotHealthy(health.status));
            }
            if required_durability > Durability::Memory && !health.durability_verified {
                failures.push(ReadinessFailure::DurabilityNotVerified);
            }
            Some(health)
        }
        Err(error) => {
            failures.push(ReadinessFailure::HealthCheckFailed {
                kind: error.kind,
                retryable: error.retryable,
            });
            None
        }
    };

    StorageReadiness {
        status: if failures.is_empty() {
            ReadinessStatus::Ready
        } else {
            ReadinessStatus::NotReady
        },
        capabilities,
        health,
        required_durability,
        failures,
    }
}

fn failure_message(failure: &ReadinessFailure) -> String {
    match failure {
        ReadinessFailure::MissingCapability(capability) => {
            format!("missing capability {capability:?}")
        }
        ReadinessFailure::HealthCheckFailed { kind, retryable } => {
            format!("health check failed kind={kind:?} retryable={retryable}")
        }
        ReadinessFailure::HealthNotHealthy(status) => {
            format!("health status is {status:?}")
        }
        ReadinessFailure::InsufficientDurability {
            required,
            strongest,
        } => format!("durability {strongest:?} is weaker than required {required:?}"),
        ReadinessFailure::DurabilityNotVerified => {
            "restart durability has not been verified".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use tempfile::TempDir;

    use super::*;
    use crate::{MemoryAdapter, RocksDbAdapter, RocksDbConfig};

    #[test]
    fn memory_adapter_cannot_make_the_data_plane_ready() {
        let report = block_on(check_storage_readiness(
            &MemoryAdapter::new(),
            Durability::Sync,
        ));

        assert!(!report.is_ready());
        assert!(
            report
                .failures
                .contains(&ReadinessFailure::MissingCapability(
                    Capability::DurableRestart
                ))
        );
        assert!(
            report
                .failures
                .contains(&ReadinessFailure::DurabilityNotVerified)
        );
        assert!(report.to_string().contains("DurableRestart"));
    }

    #[test]
    fn rocksdb_reports_ready_after_health_and_durability_checks() {
        let directory = TempDir::new().expect("temporary directory");
        let adapter = RocksDbAdapter::open(RocksDbConfig::new(directory.path().join("rocksdb")))
            .expect("open RocksDB");
        let report = block_on(check_storage_readiness(&adapter, Durability::Sync));

        assert!(report.is_ready(), "{report}");
        assert!(report.failures.is_empty());
        assert_eq!(
            report.health.expect("health report").status,
            HealthStatus::Healthy
        );
    }
}
