## Context

The control-plane process already owns a separate RocksDB path and supplies one `KvAdapter` to developer identity, operator authentication, organizations, projects, provisioning, audit, functions, and control-center projections. The data plane separately owns application identity, documents, policy, and RxDB state. This service boundary is useful, but both authorities currently share the RocksDB engine and production storage lifecycle. The public beta runs on one VM and one data disk, so this change can reduce engine and database-file coupling but cannot provide host or disk high availability.

The existing vendor-neutral `KvAdapter` contract already defines point reads, byte-ordered bounded scans, stable snapshots, atomic batches and conditions, transactions, health, and durability. Preserving that boundary avoids rewriting every domain repository and keeps API behavior stable.

## Goals / Non-Goals

**Goals:**

- Replace control-plane RocksDB with a dedicated production SQLite database without changing public APIs or domain encodings.
- Keep portal authentication and control-owned incident/recovery functions available when tenant RocksDB is down.
- Migrate existing control state byte-for-byte with deterministic proof and no empty fallback.
- Give SQLite its own configuration, readiness, backup/restore, monitoring, and release-compatibility lifecycle.
- Preserve the current single-writer, single-region operational model while reducing correlated storage-engine failures.

**Non-Goals:**

- Moving project application users, credentials, signing keys, documents, policies, indexes, or RxDB state out of the data plane.
- Storing authoritative portal data in browser SQLite, IndexedDB, RxDB, local storage, or session storage.
- Normalizing every existing key/value record into relational domain tables during this migration.
- Adding multi-writer clustering, automatic failover, multi-region replication, or availability during complete VM/disk loss.
- Supporting rollback to a binary that only understands control-plane RocksDB after SQLite has accepted new writes.

## Decisions

### Implement SQLite behind the existing `KvAdapter`

Add a pinned Rust SQLite dependency and a `SqliteAdapter` that stores opaque byte keys and values in a `WITHOUT ROWID` table keyed by `BLOB PRIMARY KEY`. SQLite BLOB ordering provides the byte-lexicographic ranges expected by current repositories. A small metadata/migration schema records database identity, format version, migration history, and durable high water. Domain validation and key prefixes remain unchanged.

This compatibility layer gives every existing control repository the new engine at one composition point and makes byte-for-byte migration and differential conformance testing possible. Fully normalized relational tables were rejected for this change because they would combine failure isolation with a broad domain-model rewrite and make migration verification substantially harder. Normalization can be proposed later without changing the portal/storage boundary established here.

### Use a connection-per-operation model with SQLite-managed transactions

The adapter owns the normalized database path, immutable limits, an exclusive process lock, and a connection factory. Each connection applies required settings including WAL journaling, full synchronous durability, foreign keys, trusted-schema restrictions, bounded busy timeout, and bounded WAL checkpoint policy. Reads use short connections; stable snapshots own a read transaction on a dedicated connection; guarded writes and compare-and-write use `BEGIN IMMEDIATE`; explicit repository transactions own a dedicated connection until commit or rollback. Blocking SQLite work runs outside async executor-critical sections.

This model satisfies `Send + Sync` without sharing one connection across unrelated work, permits concurrent readers, and serializes writes through SQLite. A single mutex-protected connection was rejected because long snapshots and transactions would block all reads and create awkward lifetime coupling. An external database server was rejected because the requested beta topology is local and should not add another network dependency.

### Make control storage configuration service-specific

Add a control SQLite configuration containing absolute database, migration-workspace, backup-staging, and backup-publish paths plus transaction, scan, capacity, checkpoint, and integrity limits. Production control-plane startup requires that configuration and rejects its legacy RocksDB path after cutover. Data-plane, edge-gateway, and telemetry configuration continue to require their existing RocksDB settings. Tests may still inject the deterministic memory adapter, but no production backend-selection switch or fallback is exposed.

The control SQLite file, `-wal`, and `-shm` files live in a service-owned directory separate from every RocksDB directory. A lock/identity marker prevents a second process or an unexpected database from becoming authoritative.

### Preserve the repository key model during migration

The offline migration tool operates only on a stopped, checkpointed copy of the old control RocksDB. It scans the complete byte keyspace in deterministic order and writes the exact key/value pairs into a temporary SQLite database. It calculates a framed BLAKE3 checksum over key lengths, keys, value lengths, and values on both sides, records counts by recognized control prefix, runs adapter conformance plus identity/entitlement/project/audit invariants, checkpoints SQLite WAL, fsyncs files and the parent directory, and atomically renames the verified target into place.

A protected migration receipt binds source checkpoint digest, source format, target database identity, target schema, release digest, counts, checksum, start/completion time, and verification results. Re-running against a matching complete receipt is idempotent. An incomplete temporary target may be discarded or resumed only after its receipt and source binding match; the live destination is never overwritten.

### Treat cutover as an operational compatibility boundary

The first production release supports creating, inspecting, migrating, backing up, restoring, and operating SQLite. The maintenance workflow stops public control writes, stops the control plane, creates and remotely verifies a final RocksDB checkpoint, migrates offline, selects SQLite configuration, starts the control plane, and runs exact-release authentication, authorization, wait-list, operator, project, audit, and data-plane-outage probes.

The old checkpoint remains fenced and immutable for the documented retention period. Once SQLite accepts a new write, rollback is limited to releases declaring compatibility with the selected SQLite schema. Automatic dual-write to RocksDB was rejected because it would retain RocksDB as a control mutation dependency and recreate the correlated failure this change removes. Reverse migration is not implicit; it would require a separate design and proof.

### Separate readiness from data-plane availability

Control-plane process readiness depends on SQLite, required control secrets, mail configuration where applicable, and compatible local control dependencies—not on tenant RocksDB readiness. Data-plane clients and telemetry providers report scoped unavailable/stale results. Operator overview aggregation retains successful SQLite-backed sections and explicitly marks failed providers. Recovery actions that truly require the data plane remain unavailable until their preconditions pass; authentication and incident coordination do not.

### Give SQLite independent protection and evidence

Backups use SQLite's consistent backup mechanism into an offline file, run integrity checks, checkpoint required WAL state, generate a protected manifest, and copy/verify it off the VM. Restore uses an empty target and explicit promotion. Metrics and alerts distinguish SQLite open/integrity/migration/transaction/WAL/capacity/backup signals from RocksDB signals. Secrets, email addresses, keys, values, and document content are excluded from metrics and evidence.

## Risks / Trade-offs

- [SQLite and RocksDB still share one VM/data disk] → Use separate service-owned directories and backups outside the VM; document that host/disk HA remains out of scope.
- [SQLite permits one writer at a time] → Keep transactions short, use bounded busy handling, test realistic concurrent auth/operator workloads, and expose contention metrics before cutover.
- [Opaque key/value rows do not gain relational query benefits] → Preserve existing semantics for a safer first migration; consider normalized read projections or domain tables in a later change.
- [WAL growth or checkpoint stalls can exhaust storage] → Bound transaction duration, configure checkpoint policy, monitor WAL/file size, retain capacity reserves, and fail mutations before critical exhaustion.
- [Engine ordering or transaction behavior differs] → Run the complete adapter conformance suite, differential traces against RocksDB, concurrent conflict tests, and crash/restart fault injection.
- [One-way cutover narrows rollback choices] → Deploy SQLite support before cutover, retain a fenced source checkpoint, allow only SQLite-compatible code rollback afterward, and require explicit reverse-migration design for any return to RocksDB.
- [A corrupt control database still disables authentication] → Add independent verified backups, empty-target restore drills, static health signaling, Proxmox console recovery, and a bounded break-glass procedure that never bypasses data authorization.

## Migration Plan

1. Land the SQLite adapter, conformance tests, configuration, migration tool, backup/restore support, telemetry, and SQLite-capable service composition while production remains on RocksDB.
2. Build an immutable release and qualify empty/new SQLite databases plus an offline migration from a production-like RocksDB snapshot.
3. Pause public control mutations, fence the control-plane service, create and remotely verify a final RocksDB checkpoint, and capture the selected release/configuration evidence.
4. Run the offline migration into a temporary target and require matching byte count/checksum, domain inventories, integrity, conformance, and readiness evidence.
5. Atomically publish and select SQLite, start the control plane, and verify developer registration/login/recovery, wait-list review, operator login/step-up, organizations/projects, audit, function metadata, and restart behavior.
6. Stop the data plane deliberately and prove the portal can still authenticate operators/developers, show scoped outage state, and expose permitted incident/recovery coordination.
7. Run SQLite backup, off-VM verification, empty-target restore, explicit promotion, guest-reboot, load, contention, and disk/WAL failure drills.
8. Re-enable the prior admission mode only after exact-release gates pass. Retain the fenced RocksDB checkpoint and migration receipt according to policy; do not select a pre-SQLite binary after new writes.

