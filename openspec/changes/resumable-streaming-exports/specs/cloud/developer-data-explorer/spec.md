## MODIFIED Requirements

### Requirement: Asynchronous document export
An authorized member SHALL be able to export a bounded query result or collection snapshot as versioned JSON Lines artifacts with a manifest describing scope, schema, snapshot, count, digest, and generation time. Export jobs MUST enforce quotas, run against a consistent snapshot, report progress, support cancellation, encrypt artifacts at rest, issue short-lived downloads, and delete artifacts after the published expiry.

#### Scenario: Export completes
- **WHEN** an authorized administrative user starts an export within project limits
- **THEN** the job produces an integrity-addressed artifact and manifest, records the access in audit, and offers a short-lived tenant-bound download

#### Scenario: Export includes documents near the document-size limit
- **WHEN** a collection snapshot contains valid documents up to the supported 1 MiB document limit and the job remains within its quotas
- **THEN** the export includes every document exactly once across bounded 2 MiB pages, with a bounded 4 MiB allowance in both the control-to-data HTTP client and the encrypted response journal
- **AND** pagination retains the same snapshot and excludes later writes, retrying a page replays its exact journaled response, and request-size and document-size limits remain unchanged

#### Scenario: Export would exceed a limit
- **WHEN** an export exceeds its row, byte, duration, or concurrent-job quota
- **THEN** the system stops safely, reports the applicable limit, produces no partial downloadable artifact, and records the outcome

#### Scenario: Collection exceeds one artifact limit
- **WHEN** a consistent collection export exceeds the single-object 512 MiB limit within published job and manifest capacity limits
- **THEN** the job writes bounded immutable parts with durable progress and publishes an ordered manifest only when all parts and the complete-output digest are verified
- **AND** an authorized download selects only a finalized manifest member and legacy clients cannot mistake one part for the complete export

#### Scenario: Worker or snapshot interruption
- **WHEN** an export worker retries after interruption
- **THEN** it resumes durable progress on the same live snapshot, or restarts a new coherent generation if that snapshot is gone
- **AND** active page reads renew the idle snapshot lifetime independently of developer sessions, mixed-snapshot output is never published, and cancellation or expiry retires all recorded artifacts

