# Operator Authentication Specification

## Purpose

Provide simple password-based operator access while preserving explicit privileged entitlements, isolated revocable sessions, least privilege, and auditable administration.

This capability is amended by `identity/account-role-lifecycle`: references below to active
developer eligibility and combined bootstrap activation are superseded. Historical scenarios remain
as implementation evidence; current authorization uses verified, security-active authentication
identity state plus an independent operator entitlement.

## Requirements

### Requirement: Operator eligibility is explicit and separate from ordinary developer access
The system SHALL allow operator authentication only for a verified, security-active authentication identity that has a separately stored, non-empty set of operator permissions. Developer registration, wait-list approval, project membership, and developer authentication MUST NOT grant operator permissions implicitly, and developer role state MUST NOT deny an otherwise eligible operator. No email address or permission set SHALL be hard-coded in application code.

#### Scenario: Entitled verified identity is eligible
- **WHEN** a verified, security-active authentication identity has an explicit operator entitlement
- **THEN** that identity is eligible to attempt operator password authentication for only the permissions in the entitlement

#### Scenario: Ordinary developer is not an operator
- **WHEN** an active developer without an operator entitlement submits correct credentials to operator sign-in
- **THEN** the system returns the same generic authentication failure used for invalid credentials and creates no operator session

#### Scenario: Wait-listed entitled identity is eligible
- **WHEN** a wait-listed developer role belongs to a verified, security-active authentication identity with an explicit operator entitlement
- **THEN** operator sign-in evaluates the entitlement without activating or otherwise changing the developer role

### Requirement: Operator password sign-in is same-origin and enumeration-safe
The system SHALL expose a bounded same-origin operator sign-in operation that accepts normalized email and password credentials, verifies them using the existing developer password policy and hash-upgrade behavior, and returns only generic failure responses. It MUST enforce per-source and per-identity attempt limits with bounded backoff, MUST NOT log credentials or raw session values, and MUST record redacted success, failure-class, and throttling audit events.

#### Scenario: Eligible operator signs in
- **WHEN** an eligible operator submits the correct email and password from the configured public origin within attempt limits
- **THEN** the system creates a separate operator session, returns a generic success response, and records a redacted successful-authentication audit event

#### Scenario: Invalid or ineligible sign-in is indistinguishable
- **WHEN** a request uses an unknown email, incorrect password, inactive identity, unverified email, missing entitlement, or malformed credential
- **THEN** the public response has the same status and generic message for every case and no operator session is created

#### Scenario: Repeated attempts are throttled
- **WHEN** operator sign-in attempts exceed a configured bound for a source or normalized identity
- **THEN** subsequent attempts receive a generic retryable rate-limit response for a bounded interval without revealing whether the identity exists

#### Scenario: Cross-origin sign-in is rejected
- **WHEN** an operator sign-in request does not prove the configured same origin
- **THEN** the system rejects it before credential verification and creates no session

### Requirement: Operator sessions are isolated, cookie-backed, and revocable
Successful password verification SHALL create an opaque operator session whose credential is stored only in a `Secure`, `HttpOnly`, `SameSite=Strict` cookie scoped to operator authentication and API paths. The server SHALL store only a protected digest of the credential, bind the session to the operator identity, permissions, authorization epoch, issuance time, last verification time, and absolute expiry, and limit the absolute lifetime to at most one hour. Developer, wait-list, application-user, expired, revoked, malformed, and wrong-audience credentials MUST NOT authorize operator routes.

#### Scenario: Operator route accepts a current operator session
- **WHEN** a request carries a current operator cookie with the required permission and current authorization epoch
- **THEN** the operator route authorizes the request as the bound operator identity and records that identity in its audit context

#### Scenario: Developer session cannot cross the boundary
- **WHEN** a developer or wait-list session is presented to an operator route
- **THEN** the route rejects it without treating the developer identity as an operator

#### Scenario: Script cannot read the operator credential
- **WHEN** the console signs in and runs ordinary client-side code
- **THEN** the raw operator credential is unavailable to JavaScript and is sent only by the browser to its scoped same-origin paths

#### Scenario: Expired or revoked session fails closed
- **WHEN** an operator session is expired, revoked, has a stale authorization epoch, or refers to removed permissions
- **THEN** every operator route rejects it and the session inspection operation reports no active operator session

### Requirement: Operator session lifecycle is bounded and understandable
The system SHALL provide same-origin session inspection and sign-out operations. Inspection SHALL expose only the current operator profile, effective permissions, verification freshness, and expiry; sign-out SHALL revoke the server-side session and expire its cookie. Session continuation MUST NOT extend the one-hour absolute lifetime without successful password verification.

#### Scenario: Console restores a current session
- **WHEN** an operator revisits the console with a current operator cookie
- **THEN** session inspection returns the bounded profile, effective permissions, verification freshness, and expiry without exposing the credential

#### Scenario: Operator signs out
- **WHEN** an authenticated operator requests sign-out from the configured origin
- **THEN** the server revokes the session, expires the cookie, and later use of that credential fails

#### Scenario: Absolute expiry requires another password verification
- **WHEN** an operator session reaches its absolute expiry
- **THEN** the console requires a new email-and-password sign-in and no silent refresh extends the expired session

### Requirement: Mutating operator actions require recent password verification
Every operator action that approves or rejects a developer, repairs provisioning, changes a quota, records an abuse response, creates or revokes support access, or otherwise mutates privileged state SHALL require password verification within the preceding five minutes in addition to its existing permission, reason, idempotency, confirmation, and audit requirements. A bounded same-origin step-up operation SHALL verify the password for the already authenticated operator without accepting a different identity.

#### Scenario: Recently verified mutation succeeds
- **WHEN** an operator has the required permission, verified the same identity's password within five minutes, and supplies every action-specific safety field
- **THEN** the system executes the action once and records both the operator identity and verification freshness in the audit event

#### Scenario: Stale verification requires step-up
- **WHEN** an otherwise authorized operator attempts a mutating action more than five minutes after password verification
- **THEN** the action is not executed and the response tells the console that password step-up is required

#### Scenario: Step-up cannot switch identities
- **WHEN** an authenticated operator submits a password belonging to another identity to the step-up operation
- **THEN** verification fails generically, the existing session gains no freshness, and no operator identity or permissions change

### Requirement: Entitlement changes and identity security events revoke authority
Granting, replacing, or revoking operator permissions SHALL require a protected administrative procedure with an explicit target, exact permission set, private reason, idempotency key, and typed confirmation. Password change or recovery, account-wide suspension or deletion, operator entitlement change or revocation, and credential-epoch advance SHALL invalidate affected operator sessions before later requests can succeed. Developer-only approval, rejection, or disablement SHALL NOT revoke operator authority.

#### Scenario: Permissions are granted deliberately
- **WHEN** a protected administrator grants an exact allowed permission set to an active, verified identity with all required safety inputs
- **THEN** the system commits the entitlement and a redacted audit event without exposing password or session material

#### Scenario: Permission revocation takes effect immediately
- **WHEN** an operator permission is removed or the entire entitlement is revoked
- **THEN** existing sessions can no longer exercise the removed authority and all cached console state is treated as untrusted

#### Scenario: Password recovery revokes operator sessions
- **WHEN** an operator completes developer password recovery or password change
- **THEN** all prior operator sessions are revoked and the new password is required for another operator sign-in

### Requirement: Initial public-beta operator bootstrap is protected and repeatable
The deployment SHALL provide an idempotent, non-public bootstrap operation that grants an exact operator permission set without changing developer admission. The operation MUST resolve the target by normalized email without writing the email into source or non-secret evidence, require an exact environment and identity binding plus typed confirmation, refuse ambiguous or unverified targets, and emit only redacted audit and qualification evidence. The historical rollout used a combined operation for `msmummy@gmail.com`; only the provenance-bound repair defined by the successor capability may reverse that developer-side effect.

#### Scenario: Initial operator is bootstrapped
- **WHEN** the authorized deployer supplies the protected target, exact permissions, reason, environment binding, idempotency key, and matching typed confirmation for the verified `msmummy@gmail.com` identity
- **THEN** the system atomically grants the requested operator entitlement, leaves developer status unchanged, and records sanitized evidence that one bound bootstrap succeeded

#### Scenario: Bootstrap replay is safe
- **WHEN** the same bootstrap request is replayed with the same idempotency key and exact payload
- **THEN** the system returns the already committed state without duplicating transitions or audit decisions

#### Scenario: Bootstrap refuses ambiguity
- **WHEN** the target is absent, duplicated, unverified, bound to another environment, or differs from the typed confirmation
- **THEN** the operation makes no lifecycle or entitlement change and emits no sensitive target data

### Requirement: Hosted operator console uses password authentication
The hosted console SHALL present an email-and-password operator sign-in form, restore and end sessions through the same-origin operator-auth operations, prompt for password step-up only when required, and never ask routine users to mint or paste a bearer token. It SHALL render generic authentication errors, clear privileged client state on sign-out or authorization failure, and show only features allowed by the current effective permissions.

#### Scenario: Operator opens the console without a session
- **WHEN** a user visits `/operator` without a current operator cookie
- **THEN** the console displays the operator email-and-password form and no file-token input

#### Scenario: Password login opens the permitted console
- **WHEN** an eligible operator completes password sign-in
- **THEN** the console displays the operator identity and only the operator functions permitted by the current session

#### Scenario: Authorization is lost while the console is open
- **WHEN** an operator request reports an expired, revoked, or stale session
- **THEN** the console clears privileged state and returns to operator sign-in without displaying protected response data

### Requirement: Break-glass tokens are not a routine public login mechanism
Any retained file-token operator issuer SHALL be disabled by default in hosted configuration, SHALL require protected-host access and an explicit incident reason to enable, SHALL issue a least-privilege credential for at most one hour, and SHALL produce a redacted audit event. The hosted console SHALL contain no bearer-token paste form; when break-glass bearer acceptance is disabled, operator APIs MUST reject those credentials.

#### Scenario: Normal deployment rejects file tokens
- **WHEN** the hosted deployment has break-glass bearer acceptance disabled and a file-issued operator token is presented
- **THEN** operator APIs reject it and password-backed operator sessions remain the only public operator authentication mechanism

#### Scenario: Incident access is explicitly enabled
- **WHEN** an authorized responder enables break-glass access with a reason and issues a bounded least-privilege token on a protected host
- **THEN** only the explicitly enabled bearer path accepts it until its bounded expiry or earlier revocation and the event is auditable without recording the token

### Requirement: Operator authentication survives qualified restart and recovery
Operator entitlements, authorization epochs, revocations, password-verification state, and protected session records SHALL use the control plane's SQLite ownership, backup, restore, and migration controls. Restart, exact-release upgrade, SQLite-compatible rollback, checkpoint restore, and empty-target recovery MUST preserve committed entitlements and MUST NOT resurrect expired or revoked sessions. Operator authentication and step-up MUST NOT require tenant data-plane RocksDB readiness.

#### Scenario: Restart preserves current authority
- **WHEN** the control plane restarts after committing an operator entitlement and a current session
- **THEN** the entitlement remains authoritative and the session is accepted only if it is still unexpired, unrevoked, credential-current, and operator-epoch-current

#### Scenario: Restore does not resurrect revoked access
- **WHEN** a qualified SQLite backup is restored after an operator session or entitlement has been revoked according to the recovery contract
- **THEN** post-restore reconciliation preserves the revocation boundary and no stale credential gains operator authority

#### Scenario: Operator signs in while tenant storage is down
- **WHEN** the control database is healthy and one or more tenant RocksDB services are unavailable
- **THEN** an eligible operator can establish and step up a cookie-backed operator session without bypassing normal password, entitlement, throttling, or audit controls
