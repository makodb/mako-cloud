# Project Authentication Specification

## Purpose

Provide isolated application-user identity and session management for each Mako Cloud project so RxDB clients and edge functions can authenticate requests securely.

## Requirements

### Requirement: Project-scoped application users
Application-user identities SHALL belong to one project environment and MUST NOT grant access to another project environment. Control-plane developer identities SHALL remain separate from application-user identities.

#### Scenario: Same email is used in two projects
- **WHEN** an email address registers independently in two projects
- **THEN** each project receives a distinct user identity and neither session is valid for the other project

### Requirement: Email and password lifecycle
Each project SHALL be able to enable email/password sign-up, sign-in, email verification, password recovery, and password change. Password material MUST be processed using a configurable modern password policy and MUST never be returned or logged.

#### Scenario: User signs up successfully
- **WHEN** sign-up is enabled and a new user submits a valid email and password
- **THEN** the service creates a project-scoped user and follows that project's email-verification setting before granting a full session

#### Scenario: Password recovery is requested
- **WHEN** any syntactically valid email requests recovery
- **THEN** the public response is indistinguishable for existing and non-existing users while an existing eligible user receives a single-use expiring recovery link

### Requirement: Signed access tokens
Successful authentication SHALL issue a short-lived signed JWT access token containing at least issuer, audience, subject, project, environment, role, issued-at, expiry, session identifier, and authorization-epoch claims. Verifiers SHALL be able to retrieve active public verification keys from a project-scoped JWKS endpoint.

#### Scenario: Valid access token is presented
- **WHEN** a data or function request presents an unexpired correctly signed token for the target project and environment
- **THEN** the gateway supplies its verified identity and trusted claims to downstream authorization

#### Scenario: Token targets another project
- **WHEN** a correctly signed token is presented to a different project
- **THEN** the request is rejected before document or function execution

### Requirement: Rotating refresh sessions
Authentication SHALL issue opaque refresh credentials that rotate on successful use. Reuse of an invalidated predecessor MUST trigger replay protection for the affected session family.

#### Scenario: Session refresh succeeds
- **WHEN** a valid current refresh credential is exchanged
- **THEN** the service invalidates it and returns a new access token and replacement refresh credential

#### Scenario: Rotated token is replayed
- **WHEN** a previously consumed refresh credential is presented outside the allowed concurrency grace window
- **THEN** the service revokes the session family and requires fresh authentication

### Requirement: Session and user revocation
Users SHALL be able to sign out one session or all their sessions. Project administrators SHALL be able to disable or delete an application user and revoke all sessions, with revocation taking effect for new data and function requests without waiting for access-token expiry.

#### Scenario: Administrator disables a user
- **WHEN** a project administrator disables an application user
- **THEN** refresh fails, gateways reject that user's session identifiers, and active replication streams are closed or reset

### Requirement: Trusted and user-editable metadata
The identity service SHALL distinguish administrator-controlled app metadata from user-editable profile metadata. Only verified token claims and administrator-controlled metadata MAY be used as trusted policy inputs.

#### Scenario: User edits profile metadata
- **WHEN** a user changes their display name or other user-editable metadata
- **THEN** that change does not grant roles or permissions controlled by app metadata

### Requirement: Public and privileged project credentials
Each project environment SHALL have rotatable public credentials for identifying browser/client requests and separately rotatable secret service credentials for trusted server operations. Possession of a public credential MUST NOT bypass user authentication or document policies.

#### Scenario: Public credential is used without a user session
- **WHEN** an unauthenticated client presents a valid public project credential
- **THEN** it receives only operations explicitly permitted to the anonymous role

#### Scenario: Service credential is rotated
- **WHEN** an administrator completes credential rotation
- **THEN** the retired credential stops authorizing new requests after the configured overlap window

### Requirement: Application-user administration
Authorized project administrators SHALL be able to list, inspect, create, invite, disable, restore, update trusted metadata for, and delete application users through management APIs and the console. Every privileged action SHALL produce an audit event.

#### Scenario: Developer without user-admin permission attempts deletion
- **WHEN** a project member lacking application-user administration permission attempts to delete a user
- **THEN** the operation is denied and audited

### Requirement: Abuse and enumeration protection
Public auth endpoints SHALL enforce configurable rate limits and SHALL avoid responses that disclose whether an account exists, except where an authenticated administrator is authorized to know.

#### Scenario: Repeated failed sign-ins occur
- **WHEN** a client exceeds the configured failed-attempt threshold
- **THEN** the service throttles subsequent attempts without revealing whether the email is registered

### Requirement: Authentication audit trail
The service SHALL record sign-up, sign-in, refresh replay, password recovery, credential change, session revocation, and administrator user-management events with project, actor, outcome, request identifier, and timestamp, while excluding passwords and raw tokens.

#### Scenario: Authentication fails
- **WHEN** a sign-in attempt fails
- **THEN** an audit event records the failure class without storing submitted credentials

