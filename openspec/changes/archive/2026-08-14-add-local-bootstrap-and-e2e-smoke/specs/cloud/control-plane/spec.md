## MODIFIED Requirements

### Requirement: Collection and schema administration
Authorized project members SHALL be able to create and inspect collections, publish compatible schema versions, inspect migration state, and manage index lifecycle. Destructive or incompatible schema changes MUST require an explicit migration workflow rather than silently reinterpreting stored documents.

Creating a collection SHALL make that collection resolvable for document reads and writes in the environment's owning data plane. The control plane MUST NOT report a collection as active until its metadata is durably recorded in the data plane that serves the environment. When that propagation cannot be completed, the operation MUST fail with a retryable diagnostic and MUST NOT leave a collection that management surfaces report as usable while document operations reject it as missing.

#### Scenario: Compatible schema version is published
- **WHEN** validation proves existing documents and clients remain compatible
- **THEN** the version becomes available for new replication sessions

#### Scenario: Incompatible schema is submitted directly
- **WHEN** a change would invalidate stored documents or active clients
- **THEN** publication is blocked and the console directs the member to a migration workflow

#### Scenario: Created collection accepts document traffic
- **WHEN** an authorized member creates a collection and the control plane reports it active
- **THEN** replication push and pull for that collection in the same environment resolve the collection instead of rejecting it as not found

#### Scenario: Data plane cannot record the collection
- **WHEN** the owning data plane cannot durably record the new collection's metadata
- **THEN** creation fails with a retryable diagnostic and the collection is not reported as active by any management surface
