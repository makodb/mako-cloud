# Document Policies Specification

## Purpose

Enforce default-deny, document-level authorization for every path that can read or mutate application documents, including replication and edge functions.

## Requirements

### Requirement: Default-deny collection policies
Every collection SHALL deny create, read, update, and delete operations unless an active policy explicitly allows the operation for the verified caller and relevant document state. Creating a collection MUST NOT implicitly make its documents public.

#### Scenario: Collection has no active policies
- **WHEN** an anonymous or authenticated application user attempts any document operation
- **THEN** the operation is denied without revealing whether the target document exists

### Requirement: Deterministic policy context
Policies SHALL evaluate deterministic expressions over the operation, project and environment, verified role, user identifier, trusted JWT claims, prior document state, proposed document state, and request metadata explicitly designated safe for authorization. Policy evaluation MUST NOT perform network calls, depend on wall-clock timing beyond supplied verified values, or execute arbitrary user code.

#### Scenario: Update policy checks ownership
- **WHEN** an authenticated user proposes an update to a document whose owner identifier matches the verified user identifier
- **THEN** the policy can allow the update based on both prior and proposed states

#### Scenario: Policy references untrusted metadata
- **WHEN** a policy attempts to use user-editable metadata as a trusted authorization source
- **THEN** validation rejects the policy before activation

### Requirement: Explicit allow and deny semantics
Administrators SHALL be able to define operation-specific allow and deny policies. An explicit matching deny SHALL override matching allows, and the absence of a matching allow SHALL deny the operation.

#### Scenario: Allow and deny both match
- **WHEN** an update matches one allow policy and one deny policy
- **THEN** the update is denied

### Requirement: State-aware write authorization
Create policies SHALL evaluate the proposed state, delete policies SHALL evaluate the prior state, and update policies SHALL evaluate both states. The system MUST re-evaluate authorization inside the same conditional transaction that commits a write or otherwise prove the authorized state did not change.

#### Scenario: Ownership is changed by update
- **WHEN** a user may update their own document but proposes changing its owner to another user
- **THEN** the update policy evaluates the new state and denies the transfer unless explicitly allowed

#### Scenario: Document changes between check and commit
- **WHEN** the current document revision changes after initial policy evaluation
- **THEN** the write does not commit under the stale authorization decision

### Requirement: Read filtering across all data paths
The same active read policy SHALL filter checkpoint pull, live streams, trusted server queries acting as a user, conflict responses, and console impersonation. Unauthorized document content MUST NOT appear in payloads, error details, logs, counts, or index diagnostics accessible to the caller.

#### Scenario: Conflict exists on unreadable document
- **WHEN** a caller attempts a write but is not allowed to read the current master state
- **THEN** the response does not include that master state or reveal protected fields

#### Scenario: Edge function uses caller context
- **WHEN** an edge function performs a data query with the invoking user's context
- **THEN** the query returns only documents allowed by the same read policies used for replication

### Requirement: Visibility revocation propagation
Authorization SHALL account for old and new document visibility. A document mutation that removes a caller's read access SHALL yield a synthetic tombstone to clients that could read the prior state, and a mutation that grants access SHALL yield the new state. Changes to policy definitions or trusted user claims SHALL advance an authorization epoch so clients can securely reset cached state.

#### Scenario: Team field changes
- **WHEN** a document moves from a team the caller belongs to into another team
- **THEN** the caller receives a tombstone while newly authorized callers receive the new document state

#### Scenario: Policy becomes more restrictive
- **WHEN** a new policy version removes previously granted read access
- **THEN** affected replication sessions are required to reset before receiving further data

### Requirement: Versioned policy activation
Policy definitions SHALL be versioned, syntax-checked, type-checked against the collection schema, and testable against administrator-supplied examples before atomic activation. Failed validation or activation MUST leave the previously active policy set unchanged.

#### Scenario: Invalid policy is submitted
- **WHEN** a policy references a field absent from the collection schema
- **THEN** activation is rejected with a source diagnostic and the current policy version remains active

#### Scenario: New policy set activates
- **WHEN** a validated policy version is activated
- **THEN** all subsequent operations use that complete version and the authorization epoch advances once

### Requirement: Controlled privileged bypass
Only a verified secret service credential or an explicitly authorized platform operator action SHALL bypass document policies. Privileged bypass MUST be opt-in per request, unavailable to public credentials and end-user sessions, and recorded in the audit trail.

#### Scenario: Edge function uses default data client
- **WHEN** an authenticated edge function invocation accesses documents without explicitly selecting a service credential
- **THEN** the data call uses the invoking user's policies rather than privileged bypass

#### Scenario: Service credential performs maintenance
- **WHEN** trusted server code explicitly uses a valid service credential
- **THEN** the operation may bypass document policies and records the credential identity and reason in audit metadata

### Requirement: Safe policy failure
Policy timeout, evaluator failure, malformed context, or unavailable policy state SHALL fail closed for application-user operations.

#### Scenario: Policy evaluator is unavailable
- **WHEN** a replication request cannot obtain a valid authorization result
- **THEN** the request fails without returning or mutating documents

### Requirement: Authorization audit events
Denied writes, privileged bypasses, policy lifecycle operations, and authorization-epoch changes SHALL emit audit events containing actor, project, collection, operation, policy version, outcome, and request identifier without embedding protected document bodies.

#### Scenario: Write is denied
- **WHEN** policy denies a pushed update
- **THEN** the audit trail identifies the document key and policy result without storing the proposed document body

