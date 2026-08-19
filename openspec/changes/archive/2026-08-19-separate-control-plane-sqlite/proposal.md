## Why

The developer and operator portal must remain available for authentication, diagnosis, and recovery when a tenant RocksDB service is unhealthy. Keeping portal identities and control metadata on the same storage engine family as the tenant data plane creates correlated failure risk and makes the operator console less useful during the incidents it is meant to resolve.

## What Changes

- Introduce a dedicated server-side SQLite control database for all control-plane-owned durable state, including developer identities, verification and recovery state, wait-list decisions, operator entitlements and sessions, organizations, projects, environments, provisioning state, control-plane audit records, and other control metadata.
- Keep project application users, project credentials, signing keys, documents, policies, indexes, RxDB checkpoints/change logs, and other data-plane authority in the existing service-owned RocksDB databases.
- Make control-plane startup, developer/operator authentication, and the operator console independent of data-plane RocksDB readiness; affected tenant operations degrade explicitly while diagnosis and recovery controls remain available.
- Add versioned SQLite schema migrations, integrity/readiness checks, bounded transactions, WAL and synchronous durability settings, backup/restore, observability, capacity safeguards, and an offline RocksDB-to-SQLite migration with count/checksum verification.
- Stage cutover through an immutable release that can inspect and migrate the prior control-plane RocksDB without permitting an empty database fallback. Retain the fenced pre-cutover RocksDB checkpoint for forensic recovery, while post-cutover rollback is limited to releases that support the SQLite schema.
- Preserve existing public HTTP APIs, SDK contracts, identity lifecycle behavior, authorization epochs, audit semantics, and tenant boundaries.
- **BREAKING (operations):** production control-plane storage changes from a RocksDB directory to a SQLite database and requires a verified maintenance-window migration; a pre-SQLite binary cannot be selected after new SQLite writes without an explicit reverse-migration design.

## Capabilities

### New Capabilities

- `storage/control-plane-sqlite`: Dedicated SQLite authority, schema lifecycle, durability, readiness, migration, backup, recovery, and observability for control-plane-owned state.

### Modified Capabilities

- `storage/production-rocksdb`: Narrow the RocksDB-only production contract to data-plane, edge, telemetry, and other explicitly RocksDB-owned state; exclude control-plane-owned metadata.
- `cloud/control-plane`: Require control APIs and portal authentication to remain independently available and degrade tenant-data-dependent sections explicitly during data-plane outages.
- `identity/developer-registration`: Persist developer registration and lifecycle state transactionally in the SQLite control authority and preserve it across migration and recovery.
- `identity/operator-authentication`: Keep operator authentication, entitlements, session revocation, and password step-up available without data-plane RocksDB readiness.
- `cloud/operator-control-center`: Require the console to load from independent control state and present actionable data-plane outage information instead of failing with the tenant database.
- `operations/public-beta-environment`: Provision, migrate, back up, monitor, restore, and qualify the SQLite control database separately from every RocksDB path.

## Impact

- Rust storage abstractions and control-plane service composition gain a production SQLite implementation and migration tooling.
- Control-plane repositories move from the generic RocksDB adapter to a SQLite-backed transactional authority; data-plane, edge-gateway, and telemetry RocksDB ownership remains unchanged.
- Cargo gains a pinned SQLite dependency and CI gains SQLite migration, concurrency, corruption, backup/restore, and failure-isolation suites.
- Public-beta configuration, systemd hardening, filesystem layout, backup jobs, release management, dashboards, alerts, runbooks, and retained evidence change for the new storage topology.
- Existing API clients and console URLs require no compatibility change.
