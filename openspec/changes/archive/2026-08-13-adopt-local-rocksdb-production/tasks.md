## 1. Reconcile the Active MVP Plan

- [x] 1.1 Update the `build-mako-cloud-mvp` proposal and design to name local RocksDB as the sole production KV backend and document the single-node availability and scaling limits.
- [x] 1.2 Replace the distributed-parity scenario in the active document-engine delta with production RocksDB durability, readiness, restart, and no-fallback scenarios while preserving the internal semantic adapter contract.
- [x] 1.3 Replace active MVP tasks 2.9, 2.10, 13.9, and 13.10 with production RocksDB configuration, qualification, backup, and restore tasks, retaining completion only where matching evidence already exists.
- [x] 1.4 Update traceability and qualification documents so no release status, test matrix, or runbook claims that a distributed adapter is required or supplied.

## 2. Remove Distributed Production Wiring

- [x] 2.1 Audit all distributed adapter types, exports, tests, configuration, documentation, and service call sites and record which internal semantic adapter pieces remain.
- [x] 2.2 Remove the vendor-facing distributed connector, adapter identity, connection endpoint, namespace, option, and credential types after confirming no supported caller remains.
- [x] 2.3 Remove distributed backend selection and secret resolution from typed configuration, environment examples, startup diagnostics, and service factories.
- [x] 2.4 Keep `KvAdapter`, the deterministic memory adapter, the RocksDB adapter, and their shared semantic conformance suite without exposing vendor types above storage.
- [x] 2.5 Add compile-time and configuration tests proving production cannot select a memory or distributed backend and contains no distributed endpoint or credential surface.

## 3. Production RocksDB Startup and Volumes

- [x] 3.1 Add typed production RocksDB settings for an absolute database path, bounded scan/batch limits, transaction timeouts, backup destination reference, retention, and disk safety thresholds.
- [x] 3.2 Reject relative, empty, known-ephemeral, unsafe-limit, and weaker-than-synchronous production storage configurations with non-sensitive diagnostics.
- [x] 3.3 Add versioned volume ownership and database-format markers and a provisioning path that creates them only on an explicitly initialized empty volume.
- [x] 3.4 Wire every stateful production service directly to its own `RocksDbAdapter` path with one declared replica and no fallback database.
- [x] 3.5 Gate readiness on ownership-marker validation, exclusive database open, RocksDB health, synchronous durability, and the required semantic capability checks.
- [x] 3.6 Add startup integration tests for a valid volume, missing volume, wrong owner, wrong format, read-only path, lock contention, corrupt open, and attempted empty fallback.
- [x] 3.7 Update deployment and provisioning assets to assign, retain, and protect one persistent volume per stateful service and reject configurations that share a path or scale its owner above one replica.

## 4. Synchronous Durability and Recovery

- [x] 4.1 Make synchronous durability, fsync, paranoid checks, and WAL verification mandatory for production RocksDB regardless of weaker call-site requests.
- [x] 4.2 Add graceful shutdown and same-volume restart recovery that completes RocksDB recovery, sequencer gap recovery, and acknowledged-high-water verification before readiness.
- [x] 4.3 Expose safe storage health for lock state, available capacity, write stalls, compaction pressure, I/O errors, corruption signals, and sequencer recovery.
- [x] 4.4 Add deterministic tests for process crashes, injected I/O failures, disk exhaustion, lock loss, interrupted compaction, and restart after acknowledged writes.
- [x] 4.5 Run the shared adapter conformance and durability soak suites against the exact production RocksDB configuration.

## 5. Backup and Restore

- [x] 5.1 Define versioned backup identifier, service/database identity, file inventory, digest, tenant inventory, format version, creation time, and acknowledged-high-water manifest types.
- [x] 5.2 Implement RocksDB checkpoint creation into a private staging path and generate a complete manifest only after the checkpoint is closed and every file is hashed.
- [x] 5.3 Add an authenticated manifest envelope with deployment-supplied signing material and ensure secrets and stored values never enter logs, diagnostics, or manifests.
- [x] 5.4 Add a backup transport boundary that uploads immutable checkpoint artifacts, reads them back for verification, applies retention, and reports backup age and failure metrics.
- [x] 5.5 Implement empty-target offline restore with manifest authentication, format and digest validation, atomic staging, and isolation of failed restores from the active volume.
- [x] 5.6 Verify restored tenant keyspaces and recover every document sequencer through the manifest's recorded acknowledged high water before writing the promotable ownership marker.
- [x] 5.7 Add operator-facing backup, inspect, restore, verify, fence, and promote commands with dry-run output and explicit destructive confirmations.
- [x] 5.8 Add tests for successful backup/restore, missing and modified files, forged and stale manifests, wrong service/format, non-empty targets, cross-tenant keys, missing acknowledged positions, and interrupted restore.

## 6. Operations and Release Qualification

- [x] 6.1 Add dashboards and alerts for RocksDB readiness, locks, disk thresholds, stalls, compaction, I/O and corruption, backup success/age, restore verification, and recovery duration.
- [x] 6.2 Publish production deployment, capacity expansion, graceful shutdown, checkpoint backup, empty-target restore, node replacement, corruption response, and rollback runbooks.
- [x] 6.3 Document the single-node write outage, recovery-point, recovery-time, no-automatic-failover, no-active-active, and no-shared-directory limitations in deployment and operator guidance.
- [x] 6.4 Run crash/restart, disk/I/O, compaction/retention, tenant isolation, backup corruption, restore, load, soak, and performance qualification on representative persistent storage and retain the reports.
- [x] 6.5 Replace distributed-adapter release gates with measured local RocksDB durability, integrity, backup age, restore, capacity, latency, recovery-point, and recovery-time thresholds.
- [x] 6.6 Exercise same-volume restart, backup restore to a replacement volume, explicit promotion, and previous-binary rollback without accepting traffic from an empty database.
- [x] 6.7 Run the full Rust and TypeScript formatting, linting, unit, integration, security, traceability, documentation, and release-validation suites and record the final OpenSpec status.
