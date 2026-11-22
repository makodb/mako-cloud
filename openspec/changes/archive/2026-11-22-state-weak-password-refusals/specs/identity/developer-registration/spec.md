## MODIFIED Requirements

### Requirement: Public developer registration is bounded and enumeration-safe
The hosted console and API SHALL accept a normalized email address, display
name, and password for a Mako Cloud developer identity. Registration MUST apply
bounded inputs, password policy, request and per-identity rate limits, and
abuse-resistant generic responses. Repeated registration for the same
normalized email MUST NOT create multiple identities or reveal whether that
email is unverified, wait-listed, active, rejected, or disabled. The password
policy MUST be checked with the rest of the payload, before any identity is
looked up, and a password it refuses MUST be answered as an invalid request in
the same way whether or not the email is registered.

#### Scenario: New visitor registers
- **WHEN** a visitor submits valid registration details within the applicable limits
- **THEN** the system durably records one unverified developer identity, queues an email-verification message, and returns the same accepted response used for an existing email

#### Scenario: Existing email is submitted again
- **WHEN** registration is submitted for an email already associated with any developer lifecycle state
- **THEN** the system returns the generic accepted response without creating another identity or disclosing the existing state

#### Scenario: Registration is abusive or over limit
- **WHEN** a source, email, payload, or registration rate violates a configured bound
- **THEN** the system rejects or throttles the request with a stable safe response and does not create unbounded identity, token, session, audit, or mail state

#### Scenario: Password does not satisfy the policy
- **WHEN** registration is submitted with a password the policy refuses, once for a registered email and once for an email nobody registered
- **THEN** both requests receive the same invalid-request refusal, no identity is created, and neither response is the accepted response or an unavailable-service response

### Requirement: Developer credentials and sessions follow hosted security controls
Developer passwords SHALL be protected with the repository's approved
memory-hard password hashing policy, and recovery SHALL be enumeration-safe,
single-use, and expiry-bounded. Access and refresh sessions MUST be bound to the
developer identity, lifecycle state, session identifier, audience, issuer,
expiry, and a revocable authorization epoch. Browser refresh credentials MUST
use protected same-origin cookies and access credentials MUST NOT be persisted
in URLs or browser local storage. Authentication MUST check current persistent
identity state rather than trusting a status claim alone. A new password that
does not satisfy the password policy MUST be refused as an invalid request
before the recovery token is examined, and the refusal MUST NOT consume the
token.

#### Scenario: Active developer signs in after approval
- **WHEN** an active developer presents valid credentials after the operator decision
- **THEN** the system issues a normal developer session that is accepted by authorized management routes

#### Scenario: Stale token claims active status
- **WHEN** a token claims the identity is active but persistent state or its authorization epoch is not active and current
- **THEN** every protected route rejects the token without falling back to the embedded status claim

#### Scenario: Password recovery completes
- **WHEN** an eligible developer uses a valid unused recovery token before expiry
- **THEN** the password changes atomically, all older sessions are revoked, and the recovery token cannot be replayed

#### Scenario: Recovery password does not satisfy the policy
- **WHEN** recovery is completed with a password the policy refuses, once with a valid unused token and once with a value that is not a token
- **THEN** both requests receive the same invalid-request refusal, and the valid token still completes a recovery with an acceptable password
