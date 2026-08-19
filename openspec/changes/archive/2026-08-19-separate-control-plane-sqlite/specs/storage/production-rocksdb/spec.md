## MODIFIED Requirements

### Requirement: Local RocksDB is the production storage backend
Every production service whose authority is explicitly defined as RocksDB-owned SHALL store its ordered key-value state in a local RocksDB optimistic transactional database on a dedicated persistent volume. Control-plane-owned identity and management state MUST instead use the dedicated SQLite control authority and MUST NOT open or duplicate that state in RocksDB. The platform MUST NOT accept a distributed-adapter backend, endpoint, namespace, or credential in production configuration. Each RocksDB path MUST have exactly one owning service process, while tenant separation continues to be enforced by encoded project and environment keyspaces.

#### Scenario: Production service starts with local storage
- **WHEN** a production data-plane, edge, telemetry, or other explicitly RocksDB-owned service is configured with an available dedicated RocksDB volume
- **THEN** it opens that database as its sole production key-value backend and reports the owned path without exposing secret or tenant data

#### Scenario: Production control plane starts
- **WHEN** the production control-plane service starts with its configured SQLite control authority
- **THEN** it does not require or open a control-plane RocksDB path while tenant RocksDB ownership remains unchanged

#### Scenario: Distributed backend configuration is supplied
- **WHEN** production configuration supplies a distributed adapter, remote endpoint, or adapter credential
- **THEN** startup fails with a non-sensitive configuration diagnostic instead of contacting or falling back to that backend

#### Scenario: A second process opens an owned database
- **WHEN** another process attempts to open a RocksDB path already owned by a running service
- **THEN** the second process fails readiness and cannot serve reads or writes
