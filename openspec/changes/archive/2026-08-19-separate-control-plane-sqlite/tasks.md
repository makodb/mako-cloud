## 1. Storage Contract and Configuration

- [x] 1.1 Update the storage threat model and authority inventory to distinguish SQLite-owned control state from RocksDB-owned application and tenant state, including browser non-authority and correlated-failure cases.
- [x] 1.2 Add a pinned SQLite Rust dependency and required build features without introducing a network database or runtime package-manager dependency.
- [x] 1.3 Define validated SQLite configuration for database, lock, migration, backup, transaction, scan, checkpoint, integrity, and capacity limits with production-safe defaults.
- [x] 1.4 Make production configuration require SQLite only for the control plane while retaining existing RocksDB requirements for data-plane, edge-gateway, and telemetry services.
- [x] 1.5 Add configuration tests for relative, ephemeral, overlapping, symlinked, missing-identity, out-of-range, legacy-RocksDB, and secret-bearing control storage inputs.

## 2. SQLite Adapter

- [x] 2.1 Add the SQLite adapter module, database identity, exclusive process lock, normalized path checks, connection initialization, and versioned metadata schema.
- [x] 2.2 Implement byte-exact point reads and bounded forward/reverse half-open scans with the existing key and response limits.
- [x] 2.3 Implement stable snapshots on dedicated SQLite read transactions and prove they cannot observe later commits.
- [x] 2.4 Implement synchronous atomic batches and conditional compare-and-write with deterministic condition outcomes and no partial mutation.
- [x] 2.5 Implement explicit read/write transactions with bounded lifetimes, guarded reads/scans, commit, rollback, busy handling, and serializable write behavior.
- [x] 2.6 Implement durability mapping, WAL checkpoint policy, file and parent-directory synchronization, health reporting, capacity checks, and safe SQLite error classification.
- [x] 2.7 Implement adapter capabilities and semantic readiness checks without returning database paths, keys, values, SQL text, credentials, or raw vendor errors.
- [x] 2.8 Add graceful shutdown that rejects new work, drains bounded transactions, checkpoints WAL, verifies health, releases the lock, and leaves restart-safe files.

## 3. Adapter Qualification

- [x] 3.1 Run the complete vendor-neutral adapter conformance suite against SQLite, including point, range, snapshot, batch, conditional, transaction, durability, and bounds behavior.
- [x] 3.2 Add differential operation-trace tests proving SQLite and RocksDB return identical key/value and conflict outcomes for control-plane workloads.
- [x] 3.3 Add concurrent wait-list, entitlement, idempotency, provisioning, audit, and session mutation tests that prove one serializable winner and bounded busy failures.
- [x] 3.4 Add crash/restart, acknowledged-write, WAL recovery, interrupted checkpoint, process-lock, guest-like restart, and graceful-shutdown tests.
- [x] 3.5 Add corruption, truncated file, unexpected identity, unsupported/newer schema, missing database, read-only path, full disk, critical reserve, and overgrown WAL failure tests.
- [x] 3.6 Add property tests for arbitrary byte keys, byte ordering, half-open ranges, batch limits, condition combinations, and deterministic error redaction.

## 4. Control-Plane Composition and Failure Isolation

- [x] 4.1 Replace the control-plane production `StorageOwner` with a SQLite owner while preserving test-only injected memory storage and removing production control RocksDB selection.
- [x] 4.2 Compose every control-owned repository, audit sink, projection, identity lifecycle, operator store, provisioning store, function metadata store, and outbox over the single SQLite adapter.
- [x] 4.3 Preserve private data-plane application-identity administration and prove application users, project credentials, signing keys, documents, policies, indexes, and RxDB state remain RocksDB-owned.
- [x] 4.4 Decouple control-plane process and route readiness from data-plane RocksDB readiness while retaining scoped fail-closed errors for operations that require data-plane authority.
- [x] 4.5 Add integration tests proving developer registration, verification, password login/recovery, wait-list status, operator login/step-up, organizations, projects, incidents, and audit remain available during data-plane outage.
- [x] 4.6 Add integration tests proving tenant-data reads, writes, credentials, application-user administration, replication, and recovery actions fail with scoped stable errors when their data-plane dependency is unavailable.
- [x] 4.7 Verify public HTTP routes, SDK types, response schemas, cookie behavior, authorization epochs, idempotency, audit correlation, and tenant boundaries remain backward compatible.

## 5. Verified RocksDB-to-SQLite Migration

- [x] 5.1 Define the protected migration plan and receipt schemas with source checkpoint identity, release binding, formats, record counts, prefix inventories, framed checksums, timestamps, and verification results.
- [x] 5.2 Implement deterministic complete-keyspace inventory and framed BLAKE3 checksum readers for fenced RocksDB checkpoints and SQLite targets.
- [x] 5.3 Implement the offline migration command with explicit source checkpoint, temporary target, final target, expected release/configuration binding, and non-secret plan/inspect modes.
- [x] 5.4 Copy byte-exact keys and values into a new temporary SQLite database, run schema/integrity/conformance and control-domain invariants, checkpoint and fsync it, and atomically publish only a verified target.
- [x] 5.5 Make migration idempotent and restart safe, refusing source drift, a mismatched receipt, an existing live target, an unfenced source, an incomplete target, or ambiguous path ownership.
- [x] 5.6 Add migration fixtures covering all control key prefixes plus realistic developer/operator/project/audit state and prove identifiers, password hashes, epochs, revocations, sessions, decisions, outbox items, and idempotency outcomes are preserved.
- [x] 5.7 Add failure injection for interrupted scan/copy/checkpoint/fsync/rename, checksum and inventory mismatch, corrupt source/target, insufficient space, unsupported formats, and attempted empty fallback.
- [x] 5.8 Extend release inspection and rollback guards so post-cutover selection accepts only releases compatible with the current SQLite format and refuses pre-SQLite binaries without a separately verified reverse migration.

## 6. SQLite Backup, Restore, and Recovery

- [x] 6.1 Define the SQLite backup manifest and high-water evidence separately from RocksDB checkpoint manifests.
- [x] 6.2 Implement transactionally consistent SQLite backup, WAL checkpoint handling, integrity verification, file digests, protected staging, authenticated off-VM publication, retention, and age/failure telemetry.
- [x] 6.3 Implement offline empty-target restore with manifest/digest/integrity/schema/inventory/epoch/revocation/audit/high-water verification and explicit atomic promotion.
- [x] 6.4 Add inspect, backup, restore, verify, and promote CLI operations that reject live-target overwrite, ambiguous paths, stale evidence, and incompatible releases.
- [x] 6.5 Add backup/restore qualification proving pending applicants, active developers, operator authority, revoked sessions, organizations/projects, provisioning state, audit history, and mail outbox state survive recovery.
- [x] 6.6 Test stale, incomplete, corrupt, wrong-identity, wrong-release, and insufficient-high-water backups plus interrupted restore and repeated promotion.

## 7. Portal and Operator Experience

- [x] 7.1 Extend operator overview providers and read models with bounded SQLite readiness, schema, migration, contention, WAL, capacity, backup, and restore status.
- [x] 7.2 Make overview and Tenant 360 sections preserve SQLite-backed control information while marking unavailable data-plane providers explicitly stale or unavailable.
- [x] 7.3 Update developer workspaces to remain navigable after control authentication while presenting scoped unavailable states for tenant-data-dependent sections.
- [x] 7.4 Add browser tests proving developer and operator authentication, step-up, wait-list review, incident navigation, and recovery coordination work while data-plane routes are unavailable.
- [x] 7.5 Add regression tests proving the portal creates no authoritative SQLite, IndexedDB, Dexie, RxDB, local-storage, or browser database and retains its existing sessionStorage/HttpOnly-cookie security boundaries.

## 8. Deployment, Observability, and Security

- [x] 8.1 Provision dedicated service-owned SQLite live, lock, migration, backup-staging, backup-publish, restore, and reserve paths separate from all RocksDB paths.
- [x] 8.2 Render production control SQLite configuration and remove the control-plane RocksDB environment/path without changing the other three RocksDB service configurations.
- [x] 8.3 Update systemd sandboxing, filesystem permissions, graceful shutdown, restart ordering, backup timers, and restore units for SQLite files, WAL, shared memory, locks, and temporary targets.
- [x] 8.4 Add non-sensitive SQLite metrics, dashboards, and alerts for readiness, schema, integrity, migration, transactions, busy time, checkpoints, WAL/file size, capacity, backups, restores, and service restarts.
- [x] 8.5 Update health collectors and release evidence to report SQLite and RocksDB ownership separately without exposing paths, keys, values, emails, credentials, or customer data.
- [x] 8.6 Verify default-deny firewall and Caddy behavior remain unchanged and no SQLite listener, browser database endpoint, private migration route, or internal storage port becomes public.
- [x] 8.7 Extend infrastructure, manifest, secret-scan, rollback, storage-topology, and evidence validators for the mixed SQLite/RocksDB deployment.

## 9. Documentation and Operational Controls

- [x] 9.1 Update architecture, configuration, deployment, API, security, authority-inventory, and storage documentation for the SQLite control/RocksDB tenant boundary.
- [x] 9.2 Publish runbooks for SQLite readiness failure, contention/WAL pressure, capacity, migration, cutover, backup/restore, corruption, compatible rollback, and explicit refusal of pre-SQLite rollback.
- [x] 9.3 Document the remaining single-VM/disk failure domain and make clear that engine separation is not replication, failover, or high availability.
- [x] 9.4 Update threat models and abuse cases for database-file theft, malicious/corrupt schema, migration substitution, rollback resurrection, browser non-authority, and data-plane outage diagnosis.

## 10. Repository and Production-Like Qualification

- [x] 10.1 Run Rust formatting, clippy with warnings denied, unit, integration, property, migration, backup/restore, and adapter differential/conformance suites.
- [x] 10.2 Run console formatting, linting, typechecking, unit, Playwright, API generation, security, observability, secret-scan, infrastructure, release, and rollback suites.
- [x] 10.3 Benchmark realistic authentication, wait-list, operator, project, audit, scan, write-contention, checkpoint, and recovery workloads and record limits/headroom without weakening release thresholds.
- [x] 10.4 Exercise service restart, guest restart, data-plane outage, SQLite corruption, full disk, WAL pressure, backup loss, empty-target restore, and compatible release rollback in a production-like environment.
- [x] 10.5 Run strict OpenSpec validation and retain machine-readable SQLite conformance, migration, failure-isolation, backup/restore, benchmark, and security evidence.

## 11. Public-Beta Cutover

- [x] 11.1 Build, checksum, install, and inspect an immutable SQLite-capable release while leaving the current control authority and public admission unchanged.
- [x] 11.2 Pause public control writes, checkpoint and remotely verify the current control RocksDB, stop and fence the control plane, and bind the migration plan to the exact source, target, release, and configuration.
- [x] 11.3 Execute the offline migration, verify counts/checksums/domain invariants/conformance, atomically publish SQLite, select the new configuration, and retain the immutable source checkpoint and receipt.
- [x] 11.4 Start the exact release and verify control readiness, developer registration/login/recovery, wait-list decisions, operator login/step-up, organizations/projects, audit, functions, idempotency, and guest-reboot persistence.
- [x] 11.5 Deliberately stop or isolate the data plane and prove the public portal still authenticates developers/operators, preserves control-owned sections, reports scoped outage state, and exposes permitted incident/recovery coordination.
- [x] 11.6 Exercise live SQLite backup, authenticated off-VM verification, empty-target restore, explicit promotion, and SQLite-compatible rollback without reopening the old RocksDB authority.
- [x] 11.7 Run exact-release hosted security, browser, streaming, load, contention, storage, recovery, firewall, and internal-port isolation qualification and update release gates without inferring unavailable evidence.
- [x] 11.8 Rebind the existing manually revocable public-preview decision to the exact release only after every non-waivable safeguard passes, then verify recurring fail-closed admission guard behavior and public routes.
- [x] 11.9 Record final mixed-storage topology, migration receipt, selected release, service/database ownership, backup/recovery, failure-isolation, and rollback-compatibility evidence.
