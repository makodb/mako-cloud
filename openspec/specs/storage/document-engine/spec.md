# Document Engine Specification

## Purpose

Provide a deterministic, revisioned JSON document engine over an internal ordered key-value contract, using exclusively owned local RocksDB for production, so replication and trusted server code can safely persist and query application data.

## Requirements

### Requirement: Backing-store conformance contract
The document engine SHALL depend on an internal backing-store contract that provides byte-key point reads and writes, lexicographically ordered bounded scans, stable read snapshots, durable atomic write batches, and an atomic conditional-write primitive. Production SHALL satisfy that contract with local RocksDB optimistic transactions opened on one exclusively owned persistent volume. The service MUST refuse data-plane traffic when the configured adapter cannot prove those semantics.

#### Scenario: Unsupported adapter is rejected
- **WHEN** a storage adapter lacks stable snapshots or atomic conditional writes
- **THEN** the document service fails readiness with a diagnostic naming the unsupported capability

#### Scenario: Production writes use synchronous RocksDB durability
- **WHEN** the production service commits a document mutation even if a call site requests weaker durability
- **THEN** RocksDB acknowledges it only after the document, revision, indexes, idempotency outcome, change record, and sequencer state satisfy synchronous durability

#### Scenario: Production storage is not ready
- **WHEN** the owned RocksDB volume is missing, locked, corrupt, read-only, below its critical capacity threshold, or fails a required semantic check
- **THEN** the document service remains unready and serves no data-plane traffic

#### Scenario: Production restarts on its owned volume
- **WHEN** the service restarts after acknowledging document mutations
- **THEN** it reopens the same volume and verifies the document state, change log, sequencer recovery, and acknowledged high water before becoming ready

#### Scenario: Production storage cannot open
- **WHEN** the configured production RocksDB path cannot be opened and verified
- **THEN** the service fails closed without creating or selecting a temporary, in-memory, or empty database

### Requirement: Tenant and project keyspace isolation
The engine SHALL scope every document, index entry, schema, revision, and change record to exactly one project and environment. A request MUST NOT address storage outside the project and environment selected by its trusted server context.

#### Scenario: Cross-project identifier is supplied
- **WHEN** a request authenticated for one project supplies a collection or document identifier from another project
- **THEN** the engine returns a not-found or authorization error without reading or revealing the other project's data

### Requirement: Versioned collection schemas
An administrator SHALL be able to create a collection with a collection identifier, JSON schema, primary-key definition, and positive schema version. The engine MUST validate every accepted document against the active schema and MUST reject primary-key changes.

#### Scenario: Valid document is written
- **WHEN** a document contains the declared primary key and satisfies the active collection schema
- **THEN** the engine accepts the write and records the schema version with the stored revision

#### Scenario: Invalid document is rejected
- **WHEN** a document violates the active schema or changes the primary key of an existing document
- **THEN** the engine rejects the write without changing document, index, or change-log state

### Requirement: Atomic document revisions
Every accepted mutation SHALL assign an opaque revision token and commit the document state, affected index entries, and change record atomically. A conditional mutation that names an assumed revision MUST succeed only when that revision is still current.

#### Scenario: Concurrent conditional updates
- **WHEN** two writers update the same current revision concurrently
- **THEN** exactly one update commits and the other receives the current revision as a conflict

#### Scenario: Failed commit leaves no partial state
- **WHEN** an atomic backing-store write fails
- **THEN** no new document revision, index entry, or change record is observable

### Requirement: Ordered collection change log
The engine SHALL assign each committed mutation a stable, totally ordered change position within its project environment and SHALL support deterministic iteration by `(changePosition, documentId)`. A caller SHALL be able to capture a high-water position and enumerate all changes after a checkpoint through that high water without gaps or duplicates.

#### Scenario: Equal-time writes remain deterministic
- **WHEN** multiple document writes commit with indistinguishable wall-clock timestamps
- **THEN** their change positions still define one stable order for checkpoint iteration

#### Scenario: Iteration uses a fixed high water
- **WHEN** new writes occur while a caller iterates changes through a captured high-water position
- **THEN** the current iteration excludes those later writes and a subsequent iteration can retrieve them

### Requirement: Tombstone deletion semantics
Deleting a document SHALL create a revisioned state with `_deleted: true` rather than immediately removing its replication history. Tombstones MUST remain available until the configured retention policy proves that supported clients can no longer require them.

#### Scenario: Document is deleted
- **WHEN** an authorized delete commits
- **THEN** the change log contains a tombstone revision that can be pulled and streamed like any other mutation

#### Scenario: Tombstone is compacted safely
- **WHEN** a tombstone exceeds retention and no supported checkpoint can depend on it
- **THEN** background compaction may remove its obsolete data without changing newer checkpoint results

### Requirement: Indexed document access
Trusted server components SHALL be able to get a document by primary key and query documents through declared single-field or compound indexes using equality, bounded range, deterministic sort, cursor pagination, and a result limit. Queries that cannot be satisfied safely by an active index MUST be rejected rather than silently performing an unbounded scan. A one-sided range over an index's leading field, with no equality predicate before it, SHALL be served: it is how a caller walks a collection when there is no query for "everything", and refusing it would leave no way to do so at all. A rejection SHALL name which rule the query broke, not merely that it was invalid.

#### Scenario: Indexed query is executed
- **WHEN** trusted server code submits a supported predicate and sort matching an active index
- **THEN** the engine returns a deterministic page and an opaque cursor for the next page

#### Scenario: Query lacks an eligible index
- **WHEN** a query would require an unbounded collection scan
- **THEN** the engine rejects it with an error identifying the required index shape

#### Scenario: A range with no equality walks the collection
- **WHEN** trusted server code ranges over the leading field of a single-field index with no equality predicate
- **THEN** every document of the collection is returned across pages, rather than the query being refused for having no leading component

### Requirement: Online index lifecycle
Administrators SHALL be able to create and remove non-unique, unique, and compound indexes. A building index MUST NOT be used for queries or uniqueness decisions until its backfill and concurrent-write catch-up complete atomically.

#### Scenario: Unique index finds duplicate data
- **WHEN** a unique-index build encounters duplicate indexed values
- **THEN** the build enters a failed state with non-sensitive diagnostics and does not become active

#### Scenario: Writes occur during index build
- **WHEN** documents change while an index is backfilling
- **THEN** the completed index includes those changes before it is marked active

### Requirement: Durable acknowledgements and snapshot reads
An acknowledged mutation SHALL survive process restart according to the backing store's declared durability mode. Multi-key reads used for a query or replication batch SHALL observe a consistent snapshot.

#### Scenario: Process restarts after acknowledgement
- **WHEN** the service acknowledges a durable mutation and then restarts
- **THEN** the committed revision and its change record remain observable

#### Scenario: Concurrent write occurs during a page read
- **WHEN** a document changes while a query page is being assembled
- **THEN** the page reflects one consistent snapshot rather than a mixture of revisions
