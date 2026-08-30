# RxDB Replication Specification

## Purpose

Expose the document platform as a first-class RxDB replication backend with resumable pull, conflict-aware push, live updates, and secure handling of changing document visibility.

## Requirements

### Requirement: Supported RxDB client integration
The platform SHALL publish a versioned client integration that connects an RxDB collection to Mako Cloud using RxDB's pull handler, push handler, and live pull stream contracts. The integration SHALL be the supported application data interface for the MVP. The integration SHALL support durable local storage: replication checkpoints and security state SHALL be persisted alongside the application's data so that a restarted application resumes from where it stopped instead of pulling everything again, and a security reset SHALL clear that persisted state. The integration SHALL also cover every application sign-in method the platform offers — password, external provider, and magic link — and object storage access under the same session, so an application does not hand-build those requests.

#### Scenario: Application starts replication
- **WHEN** an application supplies a project endpoint, collection, schema version, public project key, and application-user session
- **THEN** the integration starts bidirectional replication and exposes RxDB replication state, conflicts, and errors to the application

#### Scenario: Application restarts on durable storage
- **WHEN** an application using durable local storage is closed and reopened
- **THEN** its documents are available before any network request, replication resumes from the persisted checkpoint, and only changes since that checkpoint are pulled

#### Scenario: Application signs in through a provider with the integration
- **WHEN** an application uses the integration's provider sign-in helper and the browser returns from the provider
- **THEN** the helper completes the exchange, stores the session with the configured persistence, and replication proceeds under it

### Requirement: Checkpointed pull replication
The pull API SHALL accept a collection, nullable opaque checkpoint, and bounded batch size and SHALL return authorized document states in deterministic change order plus the next checkpoint. The server MUST either fill the requested authorized-document batch or scan through the captured high-water position before returning a shorter batch, while advancing past non-visible changes without exposing them. A pull MAY name a filter — one field and the value it must equal — so a client that replicates one slice of a collection receives only that slice; a document that leaves the filter SHALL arrive as a deletion, since to that client it is gone.

#### Scenario: Initial pull
- **WHEN** a client pulls with a null checkpoint
- **THEN** it receives the first authorized batch and a checkpoint that can resume after the scanned change position

#### Scenario: Changes are not visible to the caller
- **WHEN** the change range contains documents the caller cannot read
- **THEN** the server omits those documents, reveals no protected fields, and advances safely until the batch is full or the high-water position is exhausted

#### Scenario: A document leaves the filtered slice
- **WHEN** a document a filtered client holds is changed so the filter no longer matches it
- **THEN** the client receives it as a deletion rather than never hearing of it again

### Requirement: Conflict-aware push replication
The push API SHALL accept bounded batches containing each new fork state and its optional assumed master state. It SHALL commit each authorized, non-conflicting row atomically and return the current readable master state for rows whose assumption is stale, allowing RxDB to run its configured client conflict handler.

#### Scenario: Assumed master matches
- **WHEN** an authorized pushed row names the current master state
- **THEN** the server commits the new state and reports no conflict for that row

#### Scenario: Assumed master is stale
- **WHEN** an authorized pushed row names an older master state
- **THEN** the server leaves the master unchanged and returns its current state as a conflict

#### Scenario: Push is not authorized
- **WHEN** policy denies a create, update, or delete in a push batch
- **THEN** that row is not committed and the client integration emits a non-retryable authorization error without returning protected master content

### Requirement: Idempotent retry behavior
Pull and push operations SHALL be safe to retry after timeouts or connection loss. Repeating a successfully committed pushed revision MUST NOT create an additional document revision or duplicate change event.

#### Scenario: Push response is lost
- **WHEN** a client retries the same pushed document revision after the first response is lost
- **THEN** the server returns the outcome of the original mutation without committing it again

### Requirement: Live change stream
The platform SHALL provide an authenticated live stream that emits authorized document states with checkpoints after committed changes. Reconnection or any detected stream gap MUST emit a resynchronization signal that causes checkpoint pull to run before live delivery resumes. One stream MAY carry several collections of one environment, each frame naming the collection it belongs to beside the event rather than inside it, so an application that opens many collections holds one connection instead of one per collection.

#### Scenario: Visible document changes
- **WHEN** a connected caller is authorized to read a newly committed document state
- **THEN** the live stream emits that state and its checkpoint in commit order

#### Scenario: Client reconnects
- **WHEN** a live connection reconnects after an unknown gap
- **THEN** the integration performs checkpoint catch-up before treating the stream as current

#### Scenario: One connection carries many collections
- **WHEN** a client opens an environment stream naming several collections
- **THEN** every collection's changes arrive on that one connection, each frame naming its collection, and each collection's checkpoint advances independently

### Requirement: Replicated deletions
Pull and live delivery SHALL represent deletions as `_deleted: true` document states that retain the collection primary key and replication metadata required by RxDB.

#### Scenario: Remote document is deleted
- **WHEN** a document visible to a client is deleted on the server
- **THEN** the client receives a tombstone and removes the live document according to RxDB deletion semantics

### Requirement: Visibility-transition safety
The replication service SHALL prevent stale local copies when a document changes from readable to unreadable. If the transition results from the document mutation, the affected client SHALL receive a synthetic tombstone; if it results from policy or trusted-claim changes, the server SHALL advance an authorization epoch and the client integration SHALL clear and securely reseed affected replicated state before resuming.

#### Scenario: Document update revokes its own visibility
- **WHEN** a document was readable under its prior state but is not readable under its new state
- **THEN** an already connected affected client receives a tombstone without receiving the new protected state

#### Scenario: User membership is revoked
- **WHEN** trusted claims change so a user loses access to previously replicated documents
- **THEN** the next request or stream event forces an authorization-epoch reset before further replication

### Requirement: Schema-version compatibility
Every replication session SHALL bind to a collection schema version. The server MUST reject incompatible clients with a non-retryable schema-mismatch response that identifies the required version without returning collection data.

#### Scenario: Client schema is outdated
- **WHEN** an RxDB client connects with a schema version the server no longer accepts
- **THEN** replication does not start and the integration reports that migration is required

### Requirement: Session renewal and revocation
The client integration SHALL renew expiring access tokens through the auth service and SHALL stop data delivery promptly when a session is revoked or cannot be refreshed.

#### Scenario: Access token expires during live sync
- **WHEN** the refresh session remains valid
- **THEN** the integration obtains a new access token and resumes from its last checkpoint without duplicating writes

#### Scenario: Refresh session is revoked
- **WHEN** token renewal reports a revoked session
- **THEN** replication stops, closes the live stream, and surfaces an authentication-required state

### Requirement: Bounded requests and backpressure
The service SHALL enforce configurable batch, payload, connection, and rate limits and SHALL return retry guidance for transient throttling. The client integration SHALL apply bounded buffering and reconnect backoff.

#### Scenario: Client exceeds a transient rate limit
- **WHEN** an otherwise valid replication request exceeds its project quota
- **THEN** the server returns a retryable throttling response with a retry delay and the client backs off
