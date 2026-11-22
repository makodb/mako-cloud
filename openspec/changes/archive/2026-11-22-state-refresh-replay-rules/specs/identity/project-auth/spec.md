## MODIFIED Requirements

### Requirement: Rotating refresh sessions
Authentication SHALL issue opaque refresh credentials that rotate on successful use, in a session family that lives at most thirty days. Reuse of an invalidated predecessor MUST trigger replay protection for the affected session family, however many rotations ago that credential was consumed. A concurrency grace window of five seconds SHALL apply to the credential consumed last and to no other: presented again inside the window it receives the family's current credential, so that a client racing its own rotation is not signed out. A credential that no session family issued MUST be refused without revoking anything.

#### Scenario: Session refresh succeeds
- **WHEN** a valid current refresh credential is exchanged
- **THEN** the service invalidates it and returns a new access token and replacement refresh credential

#### Scenario: Rotated token is replayed
- **WHEN** a previously consumed refresh credential is presented outside the allowed concurrency grace window
- **THEN** the service revokes the session family and requires fresh authentication

#### Scenario: Refresh races its own rotation
- **WHEN** the credential consumed last is presented again within five seconds of its rotation
- **THEN** the service answers with the family's current credential and revokes nothing

#### Scenario: An older consumed credential is presented
- **WHEN** a credential the family consumed more than one rotation ago is presented, even within five seconds of the latest rotation
- **THEN** the service revokes the session family, and the family's current credential is refused from then on

#### Scenario: An unknown credential is presented
- **WHEN** a refresh request carries a value that no session family issued
- **THEN** the service refuses it, and every session family stays as it was
