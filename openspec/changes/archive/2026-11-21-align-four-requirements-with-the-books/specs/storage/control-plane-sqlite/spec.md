## MODIFIED Requirements

### Requirement: RocksDB-to-SQLite migration is verified and restart safe
Cutover SHALL run offline from a consistent, fenced control-plane RocksDB checkpoint into a new temporary SQLite target. Migration MUST preserve every key and value byte-for-byte, record source and target identities, versions, counts, and deterministic checksums, validate control-domain inventories and the complete storage semantic contract, durably publish the target atomically, and be safe to inspect after interruption. A run that finds an incomplete temporary target MUST refuse until the target has been inspected and removed; a run that finds a completed migration SHALL answer from its protected receipt without copying again. The source checkpoint MUST remain immutable and retained until the migration retention decision is explicitly completed.

#### Scenario: Migration succeeds
- **WHEN** the fenced source checkpoint is complete and every copied record, checksum, domain inventory, and semantic check matches
- **THEN** the migration atomically publishes the SQLite database and a signed or protected receipt before the control plane can select it

#### Scenario: Migration is interrupted
- **WHEN** the migration process stops before durable publication
- **THEN** the incomplete target is never selected, the source remains unchanged, and a later run refuses until the incomplete temporary target has been inspected and removed

#### Scenario: Verification differs
- **WHEN** any record count, checksum, domain invariant, schema check, or semantic qualification differs between source and target
- **THEN** cutover fails closed and retains both the fenced source and diagnostic evidence without serving the target
