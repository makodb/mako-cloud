## MODIFIED Requirements

### Requirement: Conditional document mutations
Administrative mode SHALL support create, update, and delete using a unique idempotency key and the expected current revision for update or delete. The console MUST display the proposed change before explicit submission. A stale revision MUST not overwrite the current document and MUST return a conflict suitable for explicit reload, comparison, and retry. A create or an update MUST carry the document content. A delete MAY omit it: the delete is then validated against the stored document's body and the tombstone keeps that body, while a delete that does carry content is validated against what it carries. When the stored document no longer satisfies the active schema, a delete without content SHALL be refused with a diagnostic that says so and names none of the stored document's fields.

#### Scenario: Developer updates the current revision
- **WHEN** an authorized administrator confirms a schema-valid update against the current revision
- **THEN** the system commits one new durable revision, updates affected indexes and change history atomically, and displays the resulting revision and audit reference

#### Scenario: Another writer commits first
- **WHEN** an administrative update names a revision that is no longer current
- **THEN** the system leaves the current document unchanged and offers an explicit comparison or reload without automatically merging or retrying the write

#### Scenario: Delete is confirmed
- **WHEN** an authorized administrator confirms deletion of the current revision
- **THEN** the system commits a tombstone revision and makes its replication and retention consequences clear

#### Scenario: Delete is requested without document content
- **WHEN** a delete names an existing document and its current revision and carries no content
- **THEN** the stored document's body is validated in its place, the delete proceeds as one that carried that body, and a document that does not exist is refused as an invalid request

#### Scenario: Create or update is requested without document content
- **WHEN** a create or an update carries no content
- **THEN** the request is refused as invalid and nothing is written
