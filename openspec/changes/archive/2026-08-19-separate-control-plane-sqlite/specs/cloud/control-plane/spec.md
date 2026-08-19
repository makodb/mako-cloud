## ADDED Requirements

### Requirement: Control availability is independent of tenant RocksDB
The control plane SHALL start and authenticate developers and operators using its independent control authority without requiring data-plane RocksDB readiness. It SHALL continue to serve control-owned identity, wait-list, organization, project metadata, audit, incident, and recovery-coordination operations during a tenant data-plane outage. Operations that require unavailable tenant authority MUST return a stable scoped unavailable result and MUST NOT make the entire control API or portal unavailable.

#### Scenario: Developer signs in during a tenant database outage
- **WHEN** the SQLite control authority is healthy but a project data-plane RocksDB service is unavailable
- **THEN** the developer can authenticate and open the project workspace while tenant-data-dependent sections show an explicit unavailable state

#### Scenario: Operator diagnoses a data-plane outage
- **WHEN** tenant RocksDB readiness fails
- **THEN** an entitled operator can authenticate, inspect retained control metadata and health evidence, and invoke permitted recovery coordination without the failed database becoming an authentication dependency

#### Scenario: Control authority is unavailable
- **WHEN** the SQLite control authority cannot prove readiness
- **THEN** control-owned authentication and mutations fail closed while static portal assets and independent public health signaling remain available

