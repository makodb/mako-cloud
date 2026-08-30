## MODIFIED Requirements

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
