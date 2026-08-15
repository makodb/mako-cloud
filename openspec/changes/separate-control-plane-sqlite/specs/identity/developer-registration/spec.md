## MODIFIED Requirements

### Requirement: Identity state is persistent, private, and recoverable
Developer identity, normalized-email uniqueness, credential, verification, recovery, session, lifecycle, idempotency, review, audit, and outbox state SHALL be stored transactionally in the control-plane-owned SQLite production authority and survive service and guest restart independently of tenant RocksDB. Backups, restore qualification, and the offline RocksDB-to-SQLite migration MUST cover this state without exposing secret material. Existing developer identities present before storage migration MUST retain their identifier, verification state, lifecycle, password hash, authorization epoch, sessions, wait-list history, and current access.

#### Scenario: Control plane restarts with pending applicants
- **WHEN** the service or VM restarts with its persistent SQLite storage intact
- **THEN** wait-list status, decisions, revocations, queued notices, and idempotency results recover without creating active, duplicate, or empty identities

#### Scenario: Existing deployment is migrated
- **WHEN** the verified storage migration processes a deployment containing existing developer identities
- **THEN** every identity and related credential, session, lifecycle, decision, audit, and outbox record retains its prior behavior and deterministic inventory in SQLite

#### Scenario: Tenant RocksDB is unavailable
- **WHEN** a developer registers, verifies, signs in, recovers a password, or inspects wait-list status while tenant RocksDB is unavailable
- **THEN** the control-owned identity lifecycle continues through SQLite without granting tenant data access or writing identity state to the failed database

