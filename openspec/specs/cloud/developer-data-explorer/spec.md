# Developer Data Explorer Specification

## Purpose

Provide authorized project members with a safe, bounded, and auditable way to inspect and operate application documents without exposing service credentials or weakening Mako Cloud's document-policy guarantees.

## Requirements

### Requirement: Project-scoped explorer authorization
The system SHALL authorize every Data Explorer request against the current developer identity, team membership, project, environment, collection, requested access mode, and requested operation. Developer-account status, operator status, or membership in another team MUST NOT grant document access.

#### Scenario: Authorized member opens a collection
- **WHEN** a developer with the required project data permission opens a collection in an active environment
- **THEN** the system allows only the explorer modes and operations granted to that membership for the selected project, environment, and collection

#### Scenario: Member changes the requested project identifier
- **WHEN** an authenticated developer substitutes a project or environment outside their authorized membership
- **THEN** the system denies the request without reading or revealing the other tenant's data and records the denied attempt

### Requirement: Explicit explorer access modes
The Data Explorer SHALL provide separate policy-preview and administrative access modes with persistent, visually distinct mode indicators. Policy preview MUST evaluate the active document policies as a selected application user and MUST be read-only except for non-committing mutation simulation. Administrative mode MUST require an explicit data-administration permission, reason, short-lived access grant, and audited privileged bypass.

#### Scenario: Developer previews an application user
- **WHEN** an authorized developer selects an application user for policy preview
- **THEN** explorer reads are filtered by that user's current verified claims and active policy version, mutation previews do not commit, and the selected preview identity remains visible

#### Scenario: Developer enters administrative mode
- **WHEN** a developer with data-administration permission supplies a valid reason and confirms administrative access
- **THEN** the system creates a short-lived project/environment-scoped grant, displays a persistent administrative-mode warning, and audits use of the grant

#### Scenario: Administrative grant expires
- **WHEN** an administrative explorer request uses an expired or revoked grant
- **THEN** the system denies the request and requires the developer to re-enter administrative mode without silently falling back to another access mode

### Requirement: Service credentials remain server-side
Explorer access SHALL use a management-authorized, short-lived capability validated by the data service. The browser MUST NOT receive, derive, store, or transmit a project service credential, internal workload credential, signing key, or unrestricted bearer token.

#### Scenario: Browser opens administrative mode
- **WHEN** the console obtains permission to perform an administrative document operation
- **THEN** it receives only a narrowly scoped, expiring explorer capability and no reusable service credential is present in the response, browser storage, URL, or client-visible logs

### Requirement: Bounded document lookup and browse
The explorer SHALL support exact primary-key lookup and deterministic, cursor-paginated browsing by the collection's canonical primary-key order. It SHALL also support equality and bounded-range predicates and deterministic sorting through active declared indexes. Every page MUST use a consistent snapshot, enforce a result limit, and preserve project, environment, collection, access mode, query, and schema-version scope in its opaque cursor.

#### Scenario: Developer browses a collection without a custom index
- **WHEN** an authorized developer requests the first bounded page without filters
- **THEN** the system returns current non-deleted documents in canonical primary-key order and an opaque cursor when more documents exist

#### Scenario: Developer runs an indexed query
- **WHEN** an authorized developer supplies supported filters and sorting satisfied by an active index
- **THEN** the system returns one deterministic policy-filtered or administratively authorized page and identifies the index used

#### Scenario: Cursor scope is changed
- **WHEN** a cursor issued for one collection, mode, query, or schema version is reused with different scope
- **THEN** the system rejects the cursor without returning documents

### Requirement: Query planning and safe rejection
Before execution, the explorer SHALL be able to explain whether a query is supported, which active index will satisfy it, its deterministic ordering, and the enforced limit. A query requiring an unbounded scan MUST be rejected with a safe required-index shape and MUST NOT silently scan the collection.

#### Scenario: Query lacks an eligible index
- **WHEN** a developer builds a filter or sort that no active index can satisfy
- **THEN** the explorer explains the required index shape, links an authorized member to index creation, and does not execute the query

### Requirement: Schema-aware document representation
The explorer SHALL present the active JSON schema, primary-key definition, schema version, current revision token, deletion state, and a formatted JSON representation for each visible document. Create and update input MUST be parsed and validated against the active schema before the console allows submission, while the server remains authoritative.

#### Scenario: Draft violates the active schema
- **WHEN** a developer edits a document draft that omits a required field or changes the primary key
- **THEN** the explorer identifies the schema error and the server rejects any submitted invalid mutation without changing document, index, or change-log state

### Requirement: Conditional document mutations
Administrative mode SHALL support create, update, and delete using a unique idempotency key and the expected current revision for update or delete. The explorer MUST display the proposed change and access mode before confirmation. A stale revision MUST not overwrite the current document and MUST return a conflict suitable for explicit reload, comparison, and retry.

#### Scenario: Developer updates the current revision
- **WHEN** an authorized administrator confirms a schema-valid update against the current revision
- **THEN** the system commits one new durable revision, updates affected indexes and change history atomically, and displays the resulting revision and audit reference

#### Scenario: Another writer commits first
- **WHEN** an administrative update names a revision that is no longer current
- **THEN** the system leaves the current document unchanged and offers an explicit comparison or reload without automatically merging or retrying the write

#### Scenario: Delete is confirmed
- **WHEN** an authorized administrator confirms deletion of the current revision
- **THEN** the system commits a tombstone revision and makes its replication and retention consequences clear

### Requirement: Policy mutation simulation
Policy-preview mode SHALL evaluate create, update, or delete drafts against the selected application user's verified policy context without committing storage changes. The result SHALL identify allow or deny, operation, policy version, and safe diagnostics without exposing inaccessible prior document content.

#### Scenario: Previewed update is denied
- **WHEN** a selected application user's active policy denies a proposed update
- **THEN** the explorer reports the denial and policy version without committing the update or revealing document fields the selected user cannot read

### Requirement: Revision and tombstone inspection
Members with explicit document-history permission SHALL be able to inspect bounded revision metadata and retained tombstones for a selected document. Historical views MUST remain collection-scoped, MUST distinguish current from historical state, and MUST not provide a policy-preview user with revisions they could not read under the applicable authorization state.

#### Scenario: Administrator inspects a deleted document
- **WHEN** an authorized administrator enables retained-tombstone visibility and selects a deleted primary key
- **THEN** the explorer clearly labels the tombstone, revision, deletion position, and retention status without treating it as a live document

### Requirement: Audited document access
Administrative reads, queries, exports, imports, mutation previews, committed mutations, and history access SHALL create audit events containing developer actor, project, environment, collection, operation, access-grant identifier, reason, target or query fingerprint, outcome, request identifier, and timestamp. Audit events MUST NOT embed document bodies, exported content, credentials, or secrets.

#### Scenario: Administrator queries documents
- **WHEN** a developer runs an administrative query
- **THEN** the audit trail records the actor, scope, query fingerprint, index, result class, reason, grant, and outcome without storing returned document content

### Requirement: Asynchronous document export
An authorized member SHALL be able to export a bounded query result or collection snapshot as a versioned JSON Lines artifact with a manifest describing scope, schema, snapshot, count, digest, and generation time. Export jobs MUST enforce quotas, run against a consistent snapshot, report progress, support cancellation, encrypt artifacts at rest, issue short-lived downloads, and delete artifacts after the published expiry.

#### Scenario: Export completes
- **WHEN** an authorized administrative user starts an export within project limits
- **THEN** the job produces an integrity-addressed artifact and manifest, records the access in audit, and offers a short-lived tenant-bound download

#### Scenario: Export would exceed a limit
- **WHEN** an export exceeds its row, byte, duration, or concurrent-job quota
- **THEN** the system stops safely, reports the applicable limit, produces no partial downloadable artifact, and records the outcome

### Requirement: Validated document import
An authorized administrator SHALL be able to upload a versioned JSON Lines import, run a non-committing dry run, review schema and conflict results, select create-only, update-existing, or upsert behavior, and then explicitly start an asynchronous import. Imports MUST use per-document conditional mutations and idempotency, enforce quotas, report progress and bounded errors, support cancellation, and MUST NOT leave partial state for an individual document.

#### Scenario: Dry run finds invalid documents
- **WHEN** an uploaded import contains schema violations, primary-key mismatches, or unsupported metadata
- **THEN** the system reports bounded row-level diagnostics and does not write any imported documents

#### Scenario: Confirmed import encounters concurrent changes
- **WHEN** documents change after dry run and before their import mutations commit
- **THEN** the configured conflict strategy is applied per document without overwriting an unreviewed current revision

### Requirement: Explorer limits and safe failures
The system SHALL enforce document-size, page, predicate, sort, upload, artifact, job-concurrency, duration, and rate limits and SHALL charge applicable project usage. Storage, schema, policy, and dependency failures MUST return safe errors without document content, policy internals, storage keys, credentials, or cross-tenant counts.

#### Scenario: Query exceeds configured limits
- **WHEN** a developer requests more predicates, rows, or bytes than allowed
- **THEN** the system rejects or safely truncates according to the documented contract, identifies the applicable limit, and returns no out-of-scope data

### Requirement: Accessible explorer interaction
The explorer SHALL support keyboard and assistive-technology use for access-mode selection, query construction, tables, document viewing, validation, comparisons, confirmations, and job progress. Loading, empty, denied, conflict, stale, deleted, failed, and completed states MUST be visually and programmatically distinct.

#### Scenario: Developer resolves a conflict with keyboard controls
- **WHEN** a developer uses only a keyboard after a conditional update conflict
- **THEN** focus and announcements identify the conflict and allow the developer to compare, reload, cancel, or prepare a new explicit update
