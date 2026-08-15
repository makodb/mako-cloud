## MODIFIED Requirements

### Requirement: Deployment preserves the local RocksDB production contract
Each RocksDB-owned Mako service SHALL have one exclusively owned persistent local RocksDB path with synchronous durability, fail-closed readiness, capacity reserves, and no memory or empty-database fallback. The control plane SHALL instead have one exclusively owned SQLite database path with its required durability, migration identity, readiness, capacity reserves, and no browser, memory, empty-database, or RocksDB fallback. Every stateful path MUST be separate and survive ordinary service and VM restarts. The deployment MUST NOT claim multi-region durability, automatic storage failover, or uninterrupted availability during VM or disk loss.

#### Scenario: VM restarts normally
- **WHEN** the beta VM reboots with its persistent disks intact
- **THEN** RocksDB-owned services reopen their original databases, the control plane reopens its original SQLite authority, and each passes its own recovery checks before accepting affected traffic

#### Scenario: Persistent path is unavailable
- **WHEN** a RocksDB-owned service cannot open its configured path or prove acknowledged-high-water recovery
- **THEN** that service remains unready instead of creating an empty database or selecting another backend, while an independently healthy control plane can report and coordinate the incident

#### Scenario: SQLite control path is unavailable
- **WHEN** the control plane cannot open its configured SQLite path or prove schema, integrity, migration, and durable high-water readiness
- **THEN** control traffic remains unavailable instead of creating an empty database or falling back to RocksDB

## ADDED Requirements

### Requirement: Public-beta control storage is migrated and operated separately
The public-beta deployment SHALL provision a protected control SQLite path, migration workspace, backup staging and publish paths, service identity, configuration, monitoring, and recovery workflow independently of all RocksDB paths. Cutover MUST use a stopped and fenced control-plane RocksDB checkpoint, verified SQLite target, immutable release binding, and explicit operator promotion. Backup schedules, age alerts, restore drills, capacity evidence, and release qualification MUST identify SQLite and RocksDB results separately.

#### Scenario: Existing beta control state is cut over
- **WHEN** the exact migration-capable release has stopped control writes and verified the fenced source and target inventories
- **THEN** the deployment atomically selects SQLite, restarts the control plane, verifies developer and operator authentication plus control APIs, and retains the source checkpoint without reopening it

#### Scenario: Control migration qualification fails
- **WHEN** migration, readiness, authentication, authorization, audit, backup, restore, or failure-isolation evidence is incomplete or failing
- **THEN** the deployment does not promote the SQLite target or claim a qualified control-storage cutover

#### Scenario: Post-cutover code rollback is requested
- **WHEN** an operator requests release rollback after SQLite has accepted new writes
- **THEN** release tooling permits only a format-compatible SQLite-capable target and refuses a pre-SQLite binary without an explicit verified reverse migration
