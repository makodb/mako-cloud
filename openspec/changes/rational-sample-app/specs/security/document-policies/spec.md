## MODIFIED Requirements

### Requirement: Deterministic policy context
Policies SHALL evaluate deterministic expressions over the operation, project and environment, verified role, user identifier, the address the caller's session authenticated as and whether the environment has confirmed the caller controls it, trusted JWT claims, prior document state, proposed document state, and request metadata explicitly designated safe for authorization. The address and its confirmation SHALL be separate inputs, because a rule that reads the address alone hands the document to whoever registered it first. A member of an object the schema declares open SHALL type as dynamic, like a trusted claim, since what lives under it belongs to the application and not to the schema. Policy evaluation MUST NOT perform network calls, depend on wall-clock timing beyond supplied verified values, or execute arbitrary user code.

#### Scenario: Update policy checks ownership
- **WHEN** an authenticated user proposes an update to a document whose owner identifier matches the verified user identifier
- **THEN** the policy can allow the update based on both prior and proposed states

#### Scenario: Policy references untrusted metadata
- **WHEN** a policy attempts to use user-editable metadata as a trusted authorization source
- **THEN** validation rejects the policy before activation

#### Scenario: A document scoped to a confirmed address
- **WHEN** a policy allows a read only where the document's address equals the caller's and the environment has confirmed it
- **THEN** the confirmed holder of that address reads it, and a caller with the same address unconfirmed, a caller with another address, and a caller with none are all refused
