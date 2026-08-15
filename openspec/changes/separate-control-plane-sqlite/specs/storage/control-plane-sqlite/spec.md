## Purpose

Define the independent server-side SQLite authority that preserves control-plane identity and metadata while tenant RocksDB services are degraded or unavailable.

## ADDED Requirements

### Requirement: SQLite is the sole control-plane storage authority
The production control plane SHALL store all control-plane-owned durable state in one exclusively owned server-side SQLite database. This state SHALL include developer identities and credentials, verification and recovery records, wait-list decisions, operator entitlements and sessions, organizations, projects, environments, provisioning state, control-plane audit records, idempotency outcomes, mail outbox state, function metadata, and other control metadata. Browser storage and tenant RocksDB databases MUST NOT become alternate authorities for that state.

#### Scenario: Control-plane state is committed
- **WHEN** an authorized control-plane operation mutates identity or management state
- **THEN** the mutation and its required audit, idempotency, index, and outbox records commit atomically in the SQLite authority before success is returned

#### Scenario: Tenant database is unavailable
- **WHEN** a data-plane RocksDB database is unhealthy or unavailable
- **THEN** the SQLite control authority remains open and authoritative without copying control state into the tenant database

#### Scenario: Browser storage is cleared
- **WHEN** a developer or operator clears browser storage or uses another browser
- **THEN** durable identity, entitlement, wait-list, project, and audit state remains unchanged in the server-side control authority

### Requirement: SQLite transactions preserve the storage semantic contract
The SQLite authority SHALL provide byte-exact point reads, bounded byte-ordered scans, stable snapshots, atomic batches, atomic conditional writes, serializable guarded transactions, and synchronous durable restart semantics required by existing control-plane repositories. Limits and timeout behavior MUST be bounded, and a successful response MUST NOT precede the configured durable commit.

#### Scenario: Concurrent decisions race
- **WHEN** concurrent transactions attempt incompatible wait-list, entitlement, provisioning, or idempotency mutations
- **THEN** at most one compatible serializable outcome commits and each caller receives a deterministic success, conflict, or retryable busy result

#### Scenario: Service restarts after acknowledgement
- **WHEN** a control mutation is acknowledged and the service or guest restarts
- **THEN** the mutation and every atomically related record remain observable after SQLite recovery

#### Scenario: Bounded scan is requested
- **WHEN** a repository scans a half-open key range with a permitted limit and direction
- **THEN** SQLite returns the same byte-ordered bounded result required by the vendor-neutral storage contract

### Requirement: Schema lifecycle and readiness fail closed
The control database SHALL carry an explicit format and migration version. Startup MUST validate path ownership, file type, schema version, required pragmas, transaction semantics, integrity, capacity thresholds, and migration completion before control readiness succeeds. It MUST NOT create an empty production database when a prior control authority or incomplete migration is expected.

#### Scenario: Supported schema opens cleanly
- **WHEN** the configured database has the expected identity, supported schema, healthy integrity result, and required durability settings
- **THEN** the control plane completes semantic readiness and may serve control traffic

#### Scenario: Schema is newer than the binary
- **WHEN** a binary opens a control database with a format or migration version it cannot safely understand
- **THEN** startup fails with a sanitized diagnostic and does not modify the database

#### Scenario: Expected database is missing
- **WHEN** production metadata indicates an existing or migrated deployment but the configured SQLite database is missing or empty
- **THEN** startup remains unready instead of initializing a blank control authority

### Requirement: RocksDB-to-SQLite migration is verified and restart safe
Cutover SHALL run offline from a consistent, fenced control-plane RocksDB checkpoint into a new temporary SQLite target. Migration MUST preserve every key and value byte-for-byte, record source and target identities, versions, counts, and deterministic checksums, validate control-domain inventories and the complete storage semantic contract, durably publish the target atomically, and be safe to inspect or resume after interruption. The source checkpoint MUST remain immutable and retained until the migration retention decision is explicitly completed.

#### Scenario: Migration succeeds
- **WHEN** the fenced source checkpoint is complete and every copied record, checksum, domain inventory, and semantic check matches
- **THEN** the migration atomically publishes the SQLite database and a signed or protected receipt before the control plane can select it

#### Scenario: Migration is interrupted
- **WHEN** the migration process stops before durable publication
- **THEN** the incomplete target is never selected, the source remains unchanged, and a later run safely resumes or recreates only the temporary target

#### Scenario: Verification differs
- **WHEN** any record count, checksum, domain invariant, schema check, or semantic qualification differs between source and target
- **THEN** cutover fails closed and retains both the fenced source and diagnostic evidence without serving the target

### Requirement: SQLite backup and recovery are independent
The platform SHALL create transactionally consistent SQLite backups, include the database and migration identity, integrity result, durable high-water evidence, file digests, and protected manifest, and verify copies outside the live path and VM failure domain. Restore SHALL target an empty offline path and MUST pass integrity, inventory, authorization-epoch, revocation, audit, and semantic readiness checks before explicit promotion.

#### Scenario: Scheduled control backup succeeds
- **WHEN** the live control database is healthy and reaches its backup schedule
- **THEN** a consistent verified backup and manifest are stored independently of RocksDB backups before success is reported

#### Scenario: Restored control database is promoted
- **WHEN** an operator restores a verified backup into an empty offline target
- **THEN** identities, wait-list state, operator authority, sessions, projects, audit history, and committed high water are verified before explicit promotion

#### Scenario: Backup is stale or invalid
- **WHEN** a backup is too old or fails digest, integrity, inventory, or high-water verification
- **THEN** it is ineligible for promotion and produces an actionable operator alert

### Requirement: Control storage is private and observable
The SQLite database, journals, temporary migration targets, backups, and receipts SHALL be accessible only to the control-plane and backup identities. The platform SHALL expose bounded non-sensitive metrics and alerts for open and readiness state, schema version, migration status, integrity checks, transaction contention, checkpoint progress, file and WAL size, disk capacity, backup age, restore verification, and failure-isolation probes without exposing keys, values, credentials, email addresses, or customer content.

#### Scenario: Disk or WAL pressure grows
- **WHEN** the database filesystem or write-ahead log crosses a configured warning or critical threshold
- **THEN** operators receive an alert and critical pressure causes mutation readiness to fail before unsafe exhaustion

#### Scenario: Unauthorized process reads the database
- **WHEN** a process outside the configured control or backup identity attempts to open a live, temporary, or backup SQLite file
- **THEN** filesystem and service isolation deny access and no control data is disclosed

