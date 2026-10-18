## ADDED Requirements

### Requirement: Storage accounting is bounded and consistent
Storage measurement MUST NOT repeatedly scan all tenant data during normal sampling. Initial reconciliation SHALL advance with bounded work while accepting ordinary writes. Published totals MUST include committed inserts, replacements, and deletions exactly once, exclude failed writes, and MUST NOT represent partial reconciliation as zero or a complete total. Restart and rollback recovery MUST preserve correctness without restoring tenant data.

#### Scenario: Concurrent writes during reconciliation
- **WHEN** records are inserted, replaced, or deleted before and after the current reconciliation position
- **THEN** the completed sample equals the logical size of committed tenant data

#### Scenario: Failed and repeated operations
- **WHEN** a conditional write fails or one batch changes the same key multiple times
- **THEN** accounting reflects only the final committed data

#### Scenario: Restart after accounting progress
- **WHEN** a database is reopened after a clean shutdown or an untrusted writer history
- **THEN** verified progress resumes or bounded reconciliation restarts without publishing a stale total

#### Scenario: Writes arrive during sampling cooldown
- **WHEN** a write marks an environment while measurement is running or cooling down
- **THEN** another sample waits for the interval and the final dirty mark is not lost

#### Scenario: Large environment initialization
- **WHEN** reconciliation exceeds one work budget
- **THEN** it returns pending, yields resources to requests, and subsequent normal samples use counters after initialization completes
