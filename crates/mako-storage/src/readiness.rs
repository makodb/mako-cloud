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

    /// What the adapter wants an operator to read while storage is not ready,
    /// such as the recovery procedure for a corruption signal. It is the
    /// adapter's own safe text: no vendor error, key, value, or path.
    #[must_use]
    pub fn operator_diagnostic(&self) -> Option<&str> {
        if self.is_ready() {
            return None;
        }
        self.health
            .as_ref()
            .and_then(|health| health.safe_diagnostic.as_deref())
    }

    /// A service's readiness detail, followed by the operator diagnostic when
    /// storage has one. The detail a service publishes is where an operator
    /// first looks, so that is where the recovery procedure is named.
    #[must_use]
    pub fn with_operator_diagnostic(&self, detail: impl Into<String>) -> String {
        let mut detail = detail.into();
        if let Some(diagnostic) = self.operator_diagnostic() {
            detail.push(';');
            detail.push_str(diagnostic);
        }
        detail
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
            if let Some(diagnostic) = self.operator_diagnostic() {
                write!(formatter, "; {diagnostic}")?;
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
    fn a_not_ready_report_carries_the_adapters_operator_diagnostic() {
        let report =
            |status, diagnostic: Option<&str>, failures: Vec<ReadinessFailure>| StorageReadiness {
                status: if failures.is_empty() {
                    ReadinessStatus::Ready
                } else {
                    ReadinessStatus::NotReady
                },
                capabilities: MemoryAdapter::new().capabilities(),
                health: Some(HealthReport {
                    status,
                    latency: std::time::Duration::ZERO,
                    durability_verified: true,
                    safe_diagnostic: diagnostic.map(str::to_owned),
                }),
                required_durability: Durability::Sync,
                failures,
            };
        let unavailable = vec![ReadinessFailure::HealthNotHealthy(
            HealthStatus::Unavailable,
        )];

        let directed = report(
            HealthStatus::Unavailable,
            Some("see=runbook"),
            unavailable.clone(),
        );
        assert_eq!(directed.operator_diagnostic(), Some("see=runbook"));
        assert_eq!(
            directed.with_operator_diagnostic("dependencies_not_ready:storage"),
            "dependencies_not_ready:storage;see=runbook"
        );
        assert_eq!(
            directed.to_string(),
            "storage not ready; health status is Unavailable; see=runbook"
        );

        let silent = report(HealthStatus::Unavailable, None, unavailable);
        assert_eq!(silent.operator_diagnostic(), None);
        assert_eq!(
            silent.with_operator_diagnostic("dependencies_not_ready:storage"),
            "dependencies_not_ready:storage"
        );
        assert_eq!(
            silent.to_string(),
            "storage not ready; health status is Unavailable"
        );

        // A ready report directs nobody anywhere, whatever the adapter said.
        let ready = report(HealthStatus::Healthy, Some("note"), Vec::new());
        assert_eq!(ready.operator_diagnostic(), None);
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
