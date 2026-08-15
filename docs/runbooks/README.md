# Mako Cloud operator runbooks

- [Operator password authentication](operator-password-authentication.md)
- [Operator control-center source failures and rollback](operator-control-center.md)

These runbooks are linked from the Prometheus rules in `infra/local/prometheus-rules/mako-cloud-alerts.yaml`. Start with the alert labels and time range, keep request and trace correlation in access-controlled telemetry, and never paste tokens, function secrets, passwords, or document bodies into incident notes.

- [Storage contract failure](storage-contract-failure.md)
- [Unresolved commit gaps](unresolved-commit-gaps.md)
- [Policy evaluation failures](policy-evaluation-failures.md)
- [Authentication refresh replay](auth-refresh-replay.md)
- [Developer registration and mail](developer-registration-and-mail.md)
- [Developer data workspace](developer-data-workspace.md)
- [Tenant-isolation signal](tenant-isolation-signal.md)
- [Edge sandbox incident](edge-sandbox-incident.md)
- [Production RocksDB incidents](production-rocksdb.md)
- [Control-plane SQLite migration, readiness, backup, restore, and compatible rollback](control-plane-sqlite.md)
