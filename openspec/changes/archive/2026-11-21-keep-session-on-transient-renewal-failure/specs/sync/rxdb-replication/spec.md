## MODIFIED Requirements

### Requirement: Session renewal and revocation
The client integration SHALL renew expiring access tokens through the auth service, with at most one renewal in flight, shared by every caller that needs it. It SHALL decide whether a session has ended only from what the service answered.

A definitive refusal of the refresh credential SHALL clear the stored session and surface an authentication-required state. The live stream SHALL close on it instead of reconnecting, and the integration SHALL report the state so that the application stops pull and push replication, which RxDB otherwise repeats.

A renewal that reached no verdict (a network fault, a timeout, throttling, or a server error) MUST NOT end the session. The stored session SHALL be kept, pulls, pushes and object requests SHALL report a retryable unavailable error, and the live stream SHALL keep reconnecting, until a later renewal succeeds or the service refuses the credential.

#### Scenario: Access token expires during live sync
- **WHEN** the refresh session remains valid
- **THEN** the integration obtains a new access token and resumes from its last checkpoint without duplicating writes

#### Scenario: Refresh session is revoked
- **WHEN** token renewal reports a revoked session
- **THEN** the stored session is cleared, the live stream closes instead of reconnecting, and the integration surfaces an authentication-required state on which the application stops replication

#### Scenario: Renewal cannot reach the service
- **WHEN** token renewal fails without a verdict from the service
- **THEN** the stored session is kept, the request reports a retryable unavailable error instead of an authentication failure, and renewal succeeds again once the service answers
