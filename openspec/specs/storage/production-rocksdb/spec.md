# Production RocksDB Specification

## Purpose

Define the safe and observable contract for running Mako Cloud production data
on exclusively owned, persistent local RocksDB databases.

## Requirements

### Requirement: Local RocksDB is the production storage backend
Every production stateful service SHALL store its ordered key-value state in a
local RocksDB optimistic transactional database on a dedicated persistent volume. The platform MUST
NOT accept a distributed-adapter backend, endpoint, namespace, or credential in
production configuration. Each database path MUST have exactly one owning
service process, while tenant separation continues to be enforced by encoded
project and environment keyspaces.

#### Scenario: Production service starts with local storage
- **WHEN** a production stateful service is configured with an available dedicated RocksDB volume
- **THEN** it opens that database as its sole production key-value backend and reports the owned path without exposing secret or tenant data

#### Scenario: Distributed backend configuration is supplied
- **WHEN** production configuration supplies a distributed adapter, remote endpoint, or adapter credential
- **THEN** startup fails with a non-sensitive configuration diagnostic instead of contacting or falling back to that backend

#### Scenario: A second process opens an owned database
- **WHEN** another process attempts to open a RocksDB path already owned by a running service
- **THEN** the second process fails readiness and cannot serve reads or writes

### Requirement: Production writes use synchronous durability
Production SHALL configure RocksDB for synchronous durable writes, paranoid
integrity checks, and verified write-ahead-log tracking. A mutation MUST be
acknowledged only after its atomic document, index, idempotency, change-log, and
sequencer state satisfies that durability mode. Production configuration MUST
NOT downgrade the minimum durability below synchronous.

#### Scenario: Acknowledged write survives restart
- **WHEN** a production mutation is acknowledged and the owning process restarts on the same persistent volume
- **THEN** the document revision, change record, idempotency outcome, and acknowledged commit high water remain observable

#### Scenario: Production requests weaker durability
- **WHEN** production configuration or a call site requests memory-only or asynchronous durability
- **THEN** the effective write remains synchronous or startup fails before traffic is accepted

### Requirement: Storage readiness fails closed
A stateful service MUST remain unready until its configured RocksDB path opens
successfully and the database proves the required point-read, ordered-scan,
snapshot, atomic-batch, conditional-write, health, and durability semantics.
The service MUST NOT create or select a temporary, in-memory, or empty fallback
database when the production path is unavailable or unhealthy.

#### Scenario: Persistent volume is unavailable
- **WHEN** the configured production path is absent, read-only, out of capacity, locked, or cannot be opened safely
- **THEN** readiness fails with an operator-safe diagnostic and no data-plane request is served

#### Scenario: Required storage semantic fails
- **WHEN** startup qualification cannot verify a required atomicity, snapshot, ordering, or durability semantic
- **THEN** the service remains unready and identifies the failed semantic without exposing stored values

### Requirement: Persistent volume lifecycle is explicit
Production provisioning SHALL assign every stateful service an explicit,
non-ephemeral storage path and SHALL preserve that volume across ordinary
process and service restarts. Destructive reinitialization, path replacement,
and volume deletion MUST require an explicit operator workflow and MUST NOT be
triggered by an ordinary restart or failed open.

#### Scenario: Service is restarted normally
- **WHEN** the deployment restarts a production stateful service
- **THEN** the replacement process mounts and opens the same volume before becoming ready

#### Scenario: Replacement starts without the prior volume
- **WHEN** a replacement process cannot mount the volume containing acknowledged state
- **THEN** it remains unready rather than initializing an empty production database

### Requirement: Consistent backup artifacts are verifiable
The platform SHALL create RocksDB backups from consistent checkpoints and SHALL
record a manifest containing the service identity, database format version,
creation time, file digests, tenant-scope inventory, and acknowledged commit
high-water evidence. Backup storage MUST be access controlled, encrypted by the
deployment environment, retained according to policy, and monitored for age and
failure.

#### Scenario: Scheduled backup succeeds
- **WHEN** a scheduled backup captures a healthy production database
- **THEN** the backup and manifest are durably stored and their file digests and high-water evidence validate before the backup is reported successful

#### Scenario: Backup is incomplete or stale
- **WHEN** a backup file, manifest entry, digest, or required recent backup is missing
- **THEN** the backup is not eligible for restore and an operator-visible alert is raised

### Requirement: Restore proves high water and tenant isolation
A restore SHALL target an empty, offline RocksDB path and MUST validate the
backup manifest, file digests, database format, tenant keyspace boundaries, and
acknowledged commit high water before the restored service can become ready.
Restore verification MUST prove that no acknowledged position is missing and no
tenant data is placed outside its encoded project and environment keyspace.

#### Scenario: Valid backup is restored
- **WHEN** an operator restores a verified backup to an empty replacement volume
- **THEN** recovery reconstructs the database, verifies its acknowledged high water and tenant boundaries, and only then allows the replacement service to become ready

#### Scenario: Restore loses acknowledged state
- **WHEN** restored data cannot prove every position through the backup's recorded acknowledged high water
- **THEN** verification fails and the restored service remains unavailable

#### Scenario: Restore contains a tenant-boundary violation
- **WHEN** restored keys or manifest inventory do not agree on project and environment scope
- **THEN** verification fails without exposing the mismatched tenant data

### Requirement: Recovery is single-node and operator controlled
Crash recovery on the same volume SHALL use RocksDB recovery followed by Mako
sequencer and acknowledged-high-water recovery. Node replacement SHALL use a
verified backup restore or an operator-approved transfer of the fenced original
volume. The platform MUST NOT advertise automatic failover, active-active
writes, shared-database multi-process access, or zero-downtime storage-node
replacement.

#### Scenario: Process crashes with an intact volume
- **WHEN** the owning process crashes and restarts with its intact volume
- **THEN** RocksDB and sequencer recovery complete before readiness and no acknowledged partial mutation becomes visible

#### Scenario: Storage node is permanently lost
- **WHEN** the original volume is unavailable after node loss
- **THEN** writes remain unavailable until an operator restores a verified backup and explicitly promotes the replacement

### Requirement: Local storage health and capacity are observable
Production SHALL expose non-sensitive metrics and alerts for database open
state, lock failures, disk capacity, write stalls, compaction pressure, I/O
errors, corruption signals, backup success and age, restore verification, and
sequencer recovery. Operators SHALL have tested procedures for capacity
expansion, graceful shutdown, backup, restore, and corruption response.

#### Scenario: Disk approaches the configured safety threshold
- **WHEN** free capacity falls below the warning or critical threshold
- **THEN** operators receive an alert identifying the affected service and volume before RocksDB exhausts the filesystem

#### Scenario: RocksDB reports possible corruption
- **WHEN** an integrity check or database operation reports corruption
- **THEN** the service fails readiness, stops accepting mutations, and directs operators to the documented recovery procedure

### Requirement: Release qualification targets the supported topology
Release qualification SHALL exercise local RocksDB conformance, synchronous
acknowledgement durability, crash and restart recovery, backup and restore,
corrupt and incomplete backups, disk and I/O failures, compaction and retention,
load, and acknowledged-high-water verification. A production distributed
adapter and distributed-network fault suite SHALL NOT be release prerequisites
while the supported production topology remains local RocksDB.

#### Scenario: Production release candidate is evaluated
- **WHEN** a release candidate completes the local RocksDB production qualification suite on representative persistent storage
- **THEN** its measured durability, recovery, integrity, capacity, latency, and backup/restore results determine release eligibility without a distributed-adapter result
