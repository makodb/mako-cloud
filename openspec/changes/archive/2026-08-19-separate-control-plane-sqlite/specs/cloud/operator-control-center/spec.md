## MODIFIED Requirements

### Requirement: Fleet overview with freshness
The system SHALL provide a global overview containing bounded summaries of tenant and environment lifecycle, control-database readiness, service readiness, request health, active alerts, RxDB synchronization, RocksDB capacity and recovery signals, backup freshness, registration and mail delivery, and deployed release state. Control-owned navigation, authentication, incident state, and recovery coordination MUST remain usable from the independent SQLite authority when a tenant RocksDB provider is unavailable. Every summary MUST identify its observation time and MUST distinguish healthy, degraded, unavailable, stale, and unknown data.

#### Scenario: Operator reviews current platform state
- **WHEN** an entitled operator opens the overview and all providers are available
- **THEN** the system presents current global summaries, active exceptions, observation times, and drill-down links without customer document content

#### Scenario: One overview provider is unavailable
- **WHEN** a telemetry or operational provider cannot supply one overview section
- **THEN** the system marks only that section unavailable or stale, preserves the remaining overview, and does not represent missing data as healthy

#### Scenario: Tenant RocksDB service has failed
- **WHEN** a tenant data-plane RocksDB service is unavailable but the SQLite control authority is healthy
- **THEN** the operator can sign in, inspect affected tenants and retained incident evidence, and access permitted recovery coordination while data-dependent panels show a scoped unavailable state

