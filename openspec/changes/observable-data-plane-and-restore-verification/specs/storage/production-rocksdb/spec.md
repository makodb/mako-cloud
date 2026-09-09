## MODIFIED Requirements

### Requirement: Local storage health and capacity are observable
Production SHALL expose non-sensitive metrics and alerts for database open
state, lock failures, disk capacity, write stalls, compaction pressure, I/O
errors, corruption signals, backup success and age, restore verification, and
sequencer recovery. Each of those series SHALL be published by the service that
owns the data it describes, and naming a series only in an alert rule SHALL NOT
satisfy this requirement. Operators SHALL have tested procedures for capacity
expansion, graceful shutdown, backup, restore, and corruption response.

#### Scenario: Disk approaches the configured safety threshold
- **WHEN** free capacity falls below the warning or critical threshold
- **THEN** operators receive an alert identifying the affected service and volume before RocksDB exhausts the filesystem

#### Scenario: RocksDB reports possible corruption
- **WHEN** an integrity check or database operation reports corruption
- **THEN** the service fails readiness, stops accepting mutations, and directs operators to the documented recovery procedure

#### Scenario: Every named storage signal has a producer
- **WHEN** the published series are compared against the storage signals this requirement names
- **THEN** each named signal is produced by the service owning that database, and a signal that appears only in an alert rule is reported as missing

#### Scenario: Corruption is signalled while nothing is publishing it
- **WHEN** a database reports an I/O error or a corruption signal and its owning service publishes no metrics
- **THEN** the condition is treated as unobserved rather than as absent, and the gap is reported as such rather than read as a healthy database
