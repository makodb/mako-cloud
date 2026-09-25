use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use async_trait::async_trait;
use mako_api::TenantScope;
use mako_control_plane::{
    BackupSummary, Freshness, InventoryKind, ObservabilityBackend, ObservabilityPayload,
    ObservabilityQuery, ObservabilitySignal, OperatorProvider, OperatorProviderError,
    OperatorReadSection, ProductionObservabilityBackend, SafeDiagnosticLink,
};
use mako_storage::SqliteAdapter;
use serde_json::Value;

#[derive(Clone)]
pub(crate) struct ProductionOperatorProvider {
    observability: Arc<ProductionObservabilityBackend>,
    diagnostic_origins: BTreeSet<String>,
    backup_evidence: Vec<BackupSummary>,
    control_storage: SqliteAdapter,
}

impl ProductionOperatorProvider {
    pub(crate) const fn new(
        observability: Arc<ProductionObservabilityBackend>,
        diagnostic_origins: BTreeSet<String>,
        backup_evidence: Vec<BackupSummary>,
        control_storage: SqliteAdapter,
    ) -> Self {
        Self {
            observability,
            diagnostic_origins,
            backup_evidence,
            control_storage,
        }
    }

    pub(crate) fn developer_backups(
        &self,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Vec<mako_api::DeveloperBackupView> {
        let exact_scope = format!(
            "{}/{}",
            tenant.project_id().as_str(),
            tenant.environment_id().as_str()
        );
        self.backup_evidence
            .iter()
            .filter(|backup| {
                backup.project_id == *tenant.project_id()
                    && backup.integrity_verified
                    && backup.remotely_verified
                    && (backup.protected_target == exact_scope
                        || backup.protected_target == tenant.environment_id().as_str())
            })
            .take(100)
            .map(|backup| {
                let recovery_point = now_unix_seconds.saturating_sub(backup.age_seconds);
                mako_api::DeveloperBackupView {
                    backup_id: backup.backup_id.clone(),
                    tenant: tenant.clone(),
                    recovery_point_unix_seconds: recovery_point,
                    verified_at_unix_seconds: recovery_point,
                    retained_until_unix_seconds: now_unix_seconds.saturating_add(24 * 60 * 60),
                    last_restore_drill_unix_seconds: backup.last_restore_drill_at_unix_seconds,
                    recovery_objective_status: match backup.objective_met {
                        Some(true) => "met",
                        Some(false) => "missed",
                        None => "unknown",
                    }
                    .to_owned(),
                }
            })
            .collect()
    }

    async fn section(
        &self,
        tenant: &TenantScope,
        kind: InventoryKind,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<OperatorReadSection, OperatorProviderError> {
        let signal = signal(kind);
        let query = ObservabilityQuery {
            cursor: None,
            from_unix_milliseconds: Some(from_unix_seconds.saturating_mul(1_000)),
            until_unix_milliseconds: Some(until_unix_seconds.saturating_mul(1_000)),
            limit: 100,
            newest_first: false,
        };
        let page = self
            .observability
            .query(tenant, signal, &query)
            .await
            .map_err(|_| OperatorProviderError::Unavailable)?;
        let unavailable = page
            .items
            .iter()
            .filter(|record| {
                matches!(
                    record.payload,
                    ObservabilityPayload::Health {
                        status: mako_control_plane::HealthState::Unavailable,
                        ..
                    }
                )
            })
            .count();
        let degraded = page
            .items
            .iter()
            .filter(|record| {
                matches!(
                    record.payload,
                    ObservabilityPayload::Health {
                        status: mako_control_plane::HealthState::Degraded,
                        ..
                    }
                )
            })
            .count();
        let failures = page
            .items
            .iter()
            .filter(|record| {
                matches!(
                    record.payload,
                    ObservabilityPayload::ReplicationError { .. }
                        | ObservabilityPayload::AuthenticationEvent {
                            outcome: mako_control_plane::EventOutcome::Failed,
                            ..
                        }
                )
            })
            .count();
        let observed = page.retention.observed_at_unix_milliseconds / 1_000;
        let freshness = if unavailable > 0 {
            Freshness::Unavailable
        } else if degraded > 0 || failures > 0 {
            Freshness::Stale
        } else if page.items.is_empty() {
            Freshness::Unknown
        } else if until_unix_seconds.saturating_sub(observed) > 5 * 60 {
            Freshness::Stale
        } else {
            Freshness::Current
        };
        let mut metrics = BTreeMap::from([
            ("recordCount".to_owned(), Value::from(page.items.len())),
            ("unavailableCount".to_owned(), Value::from(unavailable)),
            ("degradedCount".to_owned(), Value::from(degraded)),
            ("failureCount".to_owned(), Value::from(failures)),
            (
                "retentionSeconds".to_owned(),
                Value::from(page.retention.retention_seconds),
            ),
        ]);
        match kind {
            InventoryKind::Sync => {
                for name in [
                    "liveStreamCount",
                    "pullOutcomeCount",
                    "pushOutcomeCount",
                    "laggingStreamCount",
                    "conflictCount",
                    "policyDenialCount",
                    "checkpointExpiryCount",
                    "resynchronizationCount",
                ] {
                    metrics.insert(name.to_owned(), Value::from(failures));
                }
            }
            InventoryKind::Fleet => {
                metrics.insert("instanceCount".to_owned(), Value::from(page.items.len()));
                metrics.insert("restartCount".to_owned(), Value::from(0));
                metrics.insert(
                    "dependencyFailureCount".to_owned(),
                    Value::from(unavailable),
                );
                metrics.insert("certificateExpiryCount".to_owned(), Value::from(0));
                metrics.insert("configurationDriftCount".to_owned(), Value::from(degraded));
            }
            InventoryKind::RocksDb => {
                metrics.insert("volumeCount".to_owned(), Value::from(page.items.len()));
                metrics.insert("capacityWarningCount".to_owned(), Value::from(0));
                metrics.insert("writeStallCount".to_owned(), Value::from(degraded));
                metrics.insert("compactionPressureCount".to_owned(), Value::from(degraded));
                metrics.insert("backgroundErrorCount".to_owned(), Value::from(failures));
                metrics.insert("corruptionErrorCount".to_owned(), Value::from(unavailable));
                metrics.insert("recoveryEvidenceCount".to_owned(), Value::from(0));
            }
            _ => {}
        }
        Ok(OperatorReadSection {
            id: format!(
                "{}:{}:{}",
                kind.as_str(),
                tenant.project_id().as_str(),
                tenant.environment_id().as_str()
            ),
            freshness,
            observed_at_unix_seconds: Some(observed),
            provider: "telemetry-query".to_owned(),
            message: (freshness != Freshness::Current).then(|| {
                "Telemetry is stale, missing, degraded, or unavailable; inspect the scoped source."
                    .to_owned()
            }),
            metrics,
            links: self.diagnostic_links(tenant, kind, from_unix_seconds, until_unix_seconds),
        })
    }

    fn diagnostic_links(
        &self,
        tenant: &TenantScope,
        kind: InventoryKind,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Vec<SafeDiagnosticLink> {
        let Some(origin) = self.diagnostic_origins.iter().next() else {
            return Vec::new();
        };
        let query = format!(
            "projectId={}&environmentId={}&kind={}&from={}&until={}",
            tenant.project_id().as_str(),
            tenant.environment_id().as_str(),
            kind.as_str(),
            from_unix_seconds,
            until_unix_seconds,
        );
        [
            ("Dashboard", "dashboard", "/d/mako-cloud"),
            ("Scoped logs", "logs", "/explore/logs"),
            ("Scoped traces", "traces", "/explore/traces"),
            (
                "Operator runbook",
                "runbook",
                "/runbooks/operator-control-center",
            ),
        ]
        .into_iter()
        .map(|(label, kind, path)| SafeDiagnosticLink {
            label: label.to_owned(),
            kind: kind.to_owned(),
            url: format!("{origin}{path}?{query}"),
        })
        .collect()
    }
}

#[async_trait]
impl OperatorProvider for ProductionOperatorProvider {
    async fn overview(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
        let ready = self.observability.dependency_ready().await;
        let mut sections = vec![OperatorReadSection {
            id: "telemetry-readiness".to_owned(),
            freshness: if ready {
                Freshness::Current
            } else {
                Freshness::Unavailable
            },
            observed_at_unix_seconds: Some(now_unix_seconds),
            provider: "telemetry-query".to_owned(),
            message: (!ready).then(|| {
                "The telemetry dependency is unavailable; no healthy state is implied.".to_owned()
            }),
            metrics: BTreeMap::from([("ready".to_owned(), Value::from(ready))]),
            links: Vec::new(),
        }];
        sections.push(match self.control_storage.health_signals() {
            Ok(signals) => OperatorReadSection {
                id: "control-sqlite".to_owned(),
                freshness: if signals.integrity_verified {
                    Freshness::Current
                } else {
                    Freshness::Unavailable
                },
                observed_at_unix_seconds: Some(now_unix_seconds),
                provider: "control-sqlite".to_owned(),
                message: (!signals.integrity_verified).then(|| {
                    "Control database integrity cannot be verified; control authority is unavailable."
                        .to_owned()
                }),
                metrics: BTreeMap::from([
                    ("schemaVersion".to_owned(), Value::from(signals.schema_version)),
                    (
                        "integrityVerified".to_owned(),
                        Value::from(signals.integrity_verified),
                    ),
                    ("databaseBytes".to_owned(), Value::from(signals.database_bytes)),
                    ("walBytes".to_owned(), Value::from(signals.wal_bytes)),
                    ("availableBytes".to_owned(), Value::from(signals.available_bytes)),
                    (
                        "activeOperations".to_owned(),
                        Value::from(signals.active_operations),
                    ),
                    (
                        "activeTransactions".to_owned(),
                        Value::from(signals.active_transactions),
                    ),
                    (
                        "oldestTransactionSeconds".to_owned(),
                        Value::from(signals.oldest_transaction_seconds),
                    ),
                    (
                        "busyFailures".to_owned(),
                        Value::from(signals.busy_failures),
                    ),
                    (
                        "checkpointFailures".to_owned(),
                        Value::from(signals.checkpoint_failures),
                    ),
                    (
                        "diskWarningFreeBytes".to_owned(),
                        Value::from(signals.disk_warning_free_bytes),
                    ),
                    (
                        "diskCriticalFreeBytes".to_owned(),
                        Value::from(signals.disk_critical_free_bytes),
                    ),
                    (
                        "migrationStatus".to_owned(),
                        Value::from(safe_status("MAKO_CONTROL_SQLITE_MIGRATION_STATUS")),
                    ),
                    (
                        "backupStatus".to_owned(),
                        Value::from(safe_status("MAKO_CONTROL_SQLITE_BACKUP_STATUS")),
                    ),
                    (
                        "restoreStatus".to_owned(),
                        Value::from(safe_status("MAKO_CONTROL_SQLITE_RESTORE_STATUS")),
                    ),
                ]),
                links: Vec::new(),
            },
            Err(_) => OperatorReadSection {
                id: "control-sqlite".to_owned(),
                freshness: Freshness::Unavailable,
                observed_at_unix_seconds: Some(now_unix_seconds),
                provider: "control-sqlite".to_owned(),
                message: Some(
                    "Control database health is unavailable; no healthy state is implied."
                        .to_owned(),
                ),
                metrics: BTreeMap::new(),
                links: Vec::new(),
            },
        });
        for (id, provider) in [
            ("request-health", "telemetry-query"),
            ("current-alerts", "alert-summary"),
            ("rxdb-sync", "telemetry-query"),
            ("rocksdb-storage", "storage-evidence"),
            ("backup-readiness", "backup-evidence"),
            ("registration-mail", "control-plane-metrics"),
            ("release-state", "deployment-evidence"),
        ] {
            sections.push(OperatorReadSection {
                id: id.to_owned(),
                freshness: Freshness::Unknown,
                observed_at_unix_seconds: Some(now_unix_seconds),
                provider: provider.to_owned(),
                message: Some(
                    "No bounded source sample is available; unknown does not imply healthy."
                        .to_owned(),
                ),
                metrics: BTreeMap::new(),
                links: Vec::new(),
            });
        }
        Ok(sections)
    }

    async fn tenant(
        &self,
        tenant: &TenantScope,
        kind: InventoryKind,
        from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
        if kind == InventoryKind::Backups {
            let tenant = tenant.clone();
            let evidence = self
                .backup_evidence
                .iter()
                .filter(|backup| backup.project_id == *tenant.project_id())
                .take(100)
                .collect::<Vec<_>>();
            if !evidence.is_empty() {
                return Ok(evidence
                    .into_iter()
                    .map(|backup| OperatorReadSection {
                        id: format!("backup:{}", backup.backup_id),
                        freshness: if backup.integrity_verified
                            && backup.remotely_verified
                            && backup.age_seconds <= 15 * 60
                        {
                            Freshness::Current
                        } else {
                            Freshness::Stale
                        },
                        observed_at_unix_seconds: Some(until_unix_seconds),
                        provider: "verified-backup-evidence".to_owned(),
                        message: None,
                        metrics: BTreeMap::from([
                            ("backupId".to_owned(), Value::from(backup.backup_id.clone())),
                            ("ageSeconds".to_owned(), Value::from(backup.age_seconds)),
                            ("sizeBytes".to_owned(), Value::from(backup.size_bytes)),
                            (
                                "integrityVerified".to_owned(),
                                Value::from(backup.integrity_verified),
                            ),
                            (
                                "remotelyVerified".to_owned(),
                                Value::from(backup.remotely_verified),
                            ),
                            (
                                "protectedTarget".to_owned(),
                                Value::from(backup.protected_target.clone()),
                            ),
                            (
                                "restoreDrillAt".to_owned(),
                                backup
                                    .last_restore_drill_at_unix_seconds
                                    .map_or(Value::Null, Value::from),
                            ),
                            (
                                "recoveryObjectiveMet".to_owned(),
                                backup.objective_met.map_or(Value::Null, Value::from),
                            ),
                        ]),
                        links: self.diagnostic_links(
                            &tenant,
                            kind,
                            from_unix_seconds,
                            until_unix_seconds,
                        ),
                    })
                    .collect());
            }
            return Ok(vec![OperatorReadSection {
                id: format!("backups:{}", tenant.project_id().as_str()),
                freshness: Freshness::Unknown,
                observed_at_unix_seconds: Some(until_unix_seconds),
                provider: "backup-evidence".to_owned(),
                message: Some(
                    "No backup evidence provider is configured for this tenant.".to_owned(),
                ),
                metrics: BTreeMap::from([
                    ("ageSeconds".to_owned(), Value::Null),
                    ("sizeBytes".to_owned(), Value::Null),
                    ("integrityVerified".to_owned(), Value::from(false)),
                    ("remotelyVerified".to_owned(), Value::from(false)),
                    ("protectedTargetCount".to_owned(), Value::from(0)),
                    ("restoreDrillAgeSeconds".to_owned(), Value::Null),
                    ("recoveryObjectiveMet".to_owned(), Value::Null),
                ]),
                links: self.diagnostic_links(&tenant, kind, from_unix_seconds, until_unix_seconds),
            }]);
        }
        self.section(tenant, kind, from_unix_seconds, until_unix_seconds)
            .await
            .map(|section| vec![section])
    }

    async fn global(
        &self,
        kind: InventoryKind,
        _from_unix_seconds: u64,
        until_unix_seconds: u64,
    ) -> Result<Vec<OperatorReadSection>, OperatorProviderError> {
        Ok(vec![OperatorReadSection {
            id: kind.as_str().to_owned(),
            freshness: Freshness::Unknown,
            observed_at_unix_seconds: Some(until_unix_seconds),
            provider: "bounded-global-summary".to_owned(),
            message: Some(
                "A global provider is not configured; use a tenant-scoped view for telemetry."
                    .to_owned(),
            ),
            metrics: BTreeMap::new(),
            links: Vec::new(),
        }])
    }
}

fn safe_status(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

const fn signal(kind: InventoryKind) -> ObservabilitySignal {
    match kind {
        InventoryKind::Operations
        | InventoryKind::Alerts
        | InventoryKind::Fleet
        | InventoryKind::RocksDb => ObservabilitySignal::Health,
        InventoryKind::Sync => ObservabilitySignal::ReplicationError,
        InventoryKind::Security => ObservabilitySignal::AuthenticationEvent,
        InventoryKind::Backups => ObservabilitySignal::Audit,
    }
}
