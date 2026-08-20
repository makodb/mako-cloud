# Developer Registration Specification

## Purpose

Define secure self-service registration and operator-controlled activation for
Mako Cloud developer accounts without granting wait-listed applicants access to
tenant or product resources.

## Requirements

### Requirement: Public developer registration is bounded and enumeration-safe
The hosted console and API SHALL accept a normalized email address, display
name, and password for a Mako Cloud developer identity. Registration MUST apply
bounded inputs, password policy, request and per-identity rate limits, and
abuse-resistant generic responses. Repeated registration for the same
normalized email MUST NOT create multiple identities or reveal whether that
email is unverified, wait-listed, active, rejected, or disabled.

#### Scenario: New visitor registers
- **WHEN** a visitor submits valid registration details within the applicable limits
- **THEN** the system durably records one unverified developer identity, queues an email-verification message, and returns the same accepted response used for an existing email

#### Scenario: Existing email is submitted again
- **WHEN** registration is submitted for an email already associated with any developer lifecycle state
- **THEN** the system returns the generic accepted response without creating another identity or disclosing the existing state

#### Scenario: Registration is abusive or over limit
- **WHEN** a source, email, payload, or registration rate violates a configured bound
- **THEN** the system rejects or throttles the request with a stable safe response and does not create unbounded identity, token, session, audit, or mail state

### Requirement: Email ownership is verified before wait-list admission
The system SHALL verify email ownership with a random, single-use,
expiry-bounded token delivered only to the submitted address. Verification and
resend responses MUST be enumeration-safe; token material MUST be stored only
as a non-reversible digest and MUST NOT appear in logs, metrics, audit events,
URLs retained by the console, or later API responses. A successfully verified
self-registration SHALL enter `waitlisted`, never `active`.

#### Scenario: Applicant verifies within the token lifetime
- **WHEN** an unverified applicant submits the valid unused token before expiry
- **THEN** the same identity atomically becomes wait-listed, the token becomes unusable, and no tenant or product permission is granted

#### Scenario: Verification token is invalid, expired, or replayed
- **WHEN** a token is malformed, expired, already consumed, or does not match the identity
- **THEN** verification returns a stable safe failure and leaves the identity non-active without disclosing another applicant's state

#### Scenario: Applicant requests another verification message
- **WHEN** a resend request is within its bounds
- **THEN** the system returns a generic response and queues a replacement only when eligible while invalidating superseded verification material

### Requirement: Wait-listed identities have a separate least-privilege session
A verified wait-listed applicant SHALL be able to authenticate only to view
their own coarse wait-list status and sign out. Account recovery SHALL remain
available through the logged-out sign-in flow rather than being offered from
the authenticated pending-review page. A wait-list session MUST use a scope or audience that the normal
management authenticator rejects. It MUST NOT authorize organization, project,
environment, collection, credential, application-user, replication, document,
observability, operator, or edge-function operations. The status response MUST
NOT expose queue position, reviewer notes, other applicants, capacity plans, or
an estimated approval time.

#### Scenario: Wait-listed applicant signs in
- **WHEN** a verified wait-listed identity presents valid credentials
- **THEN** the system issues only a bounded wait-list session and the console shows a pending-review page with sign-out controls and no account-recovery link

#### Scenario: Wait-listed session calls a product route
- **WHEN** a wait-list session is presented to any management, data, replication, function, or operator route
- **THEN** the route rejects it before reading or mutating tenant state and records only a redacted correlated security outcome

#### Scenario: Applicant inspects wait-list status
- **WHEN** a valid wait-list session requests its status
- **THEN** the response identifies only the caller and a coarse pending state without revealing ordering or another applicant's information

### Requirement: Developer credentials and sessions follow hosted security controls
Developer passwords SHALL be protected with the repository's approved
memory-hard password hashing policy, and recovery SHALL be enumeration-safe,
single-use, and expiry-bounded. Access and refresh sessions MUST be bound to the
developer identity, lifecycle state, session identifier, audience, issuer,
expiry, and a revocable authorization epoch. Browser refresh credentials MUST
use protected same-origin cookies and access credentials MUST NOT be persisted
in URLs or browser local storage. Authentication MUST check current persistent
identity state rather than trusting a status claim alone.

#### Scenario: Active developer signs in after approval
- **WHEN** an active developer presents valid credentials after the operator decision
- **THEN** the system issues a normal developer session that is accepted by authorized management routes

#### Scenario: Stale token claims active status
- **WHEN** a token claims the identity is active but persistent state or its authorization epoch is not active and current
- **THEN** every protected route rejects the token without falling back to the embedded status claim

#### Scenario: Password recovery completes
- **WHEN** an eligible developer uses a valid unused recovery token before expiry
- **THEN** the password changes atomically, all older sessions are revoked, and the recovery token cannot be replayed

### Requirement: Platform operators control wait-list decisions
Only a separately authenticated platform operator with the explicit wait-list
review permission SHALL list or inspect applicants and approve or reject a
verified wait-listed identity. Queue reads MUST use bounded filters, stable
cursor pagination, deterministic ordering, and redacted summaries. Approval and
rejection MUST require an operator-visible reason, request identifier, and
idempotency key and MUST be atomic under concurrent review.

#### Scenario: Operator reviews pending applicants
- **WHEN** an authorized operator opens the wait-list administration surface
- **THEN** the console obtains a bounded page of wait-listed identities through the protected operator API without exposing passwords, tokens, session credentials, or private reviewer notes outside the detail view

#### Scenario: Operator approves an applicant
- **WHEN** an authorized operator approves a currently wait-listed identity with a reason and unused idempotency key
- **THEN** the identity becomes active exactly once, existing wait-list sessions are revoked, an immutable audit event and notification are recorded, and a fresh sign-in is required before product access

#### Scenario: Operator rejects an applicant
- **WHEN** an authorized operator rejects a currently wait-listed identity with a reason and unused idempotency key
- **THEN** the identity becomes rejected, all sessions are revoked, no product permission is granted, and the private reviewer reason is not disclosed in the applicant status response

#### Scenario: Two operators decide concurrently
- **WHEN** conflicting approve and reject operations race for the same wait-listed identity
- **THEN** only one state transition commits and the losing operation returns a stable conflict without changing or duplicating the decision

#### Scenario: Unauthorized actor calls a review route
- **WHEN** a developer, wait-listed applicant, application user, or operator lacking wait-list review permission calls an operator wait-list route
- **THEN** the request is denied before applicant data is returned or changed and the denial is audited

### Requirement: Lifecycle changes revoke stale authority
The developer lifecycle SHALL distinguish at least `unverified`, `waitlisted`,
`active`, `rejected`, and `disabled`. Every transition that changes available
authority MUST advance a durable authorization epoch and revoke incompatible
sessions. An identity MUST NOT gain organization membership or resource
ownership merely by becoming active. Rejected and disabled identities MUST
remain non-active unless a later explicit, authorized transition is defined and
audited.

#### Scenario: Approved identity has an old wait-list session
- **WHEN** an approved developer reuses a session issued before approval
- **THEN** the session is rejected and the developer must perform a fresh sign-in to receive active authority

#### Scenario: Active identity is disabled
- **WHEN** an authorized operator disables an active developer identity
- **THEN** all current sessions become unusable before subsequent management operations and existing tenant data remains governed by normal ownership and recovery procedures

#### Scenario: Newly active developer enters the product
- **WHEN** a newly approved developer signs in successfully without an existing organization membership
- **THEN** the console offers the normal authorized organization-onboarding flow rather than silently assigning membership to an existing tenant

### Requirement: Registration mail is durable and fail-closed
Verification, recovery, approval, and rejection notices SHALL be represented by
a durable bounded outbox and delivered through the configured authenticated
mail adapter without embedding secrets in logs or evidence. Public registration
readiness MUST remain closed when mail is unconfigured or cannot accept durable
outbox work. A transient delivery failure after an atomic identity transition
MUST be retried without repeating the transition, minting a new credential, or
rolling an approved account back to wait-listed.

#### Scenario: Hosted mail is not configured
- **WHEN** the deployment lacks usable authenticated mail configuration
- **THEN** self-service registration is unavailable and does not create an identity that cannot receive verification while existing authenticated product access remains unaffected

#### Scenario: Approval notice delivery fails temporarily
- **WHEN** approval commits but the mail provider temporarily rejects delivery
- **THEN** the identity remains active, the outbox retries within bounded policy, and operators receive a redacted delivery-health signal

### Requirement: Identity state is persistent, private, and recoverable
Developer identity, normalized-email uniqueness, credential, verification, recovery, session, lifecycle, idempotency, review, audit, and outbox state SHALL be stored transactionally in the control-plane-owned SQLite production authority and survive service and guest restart independently of tenant RocksDB. Backups, restore qualification, and the offline RocksDB-to-SQLite migration MUST cover this state without exposing secret material. Existing developer identities present before storage migration MUST retain their identifier, verification state, lifecycle, password hash, authorization epoch, sessions, wait-list history, and current access.

#### Scenario: Control plane restarts with pending applicants
- **WHEN** the service or VM restarts with its persistent SQLite storage intact
- **THEN** wait-list status, decisions, revocations, queued notices, and idempotency results recover without creating active, duplicate, or empty identities

#### Scenario: Existing deployment is migrated
- **WHEN** the verified storage migration processes a deployment containing existing developer identities
- **THEN** every identity and related credential, session, lifecycle, decision, audit, and outbox record retains its prior behavior and deterministic inventory in SQLite

#### Scenario: Tenant RocksDB is unavailable
- **WHEN** a developer registers, verifies, signs in, recovers a password, or inspects wait-list status while tenant RocksDB is unavailable
- **THEN** the control-owned identity lifecycle continues through SQLite without granting tenant data access or writing identity state to the failed database

### Requirement: Console and API expose the same wait-list lifecycle
The hosted console SHALL provide registration, verification, sign-in,
recovery, pending-status, and sign-out screens backed by the documented public
developer-auth API. The separate operator console SHALL provide bounded queue,
detail, approve, and reject screens backed by the documented operator API.
Console behavior MUST use the same validation, authorization, idempotency, and
audit rules as direct API calls and MUST distinguish developer identity from
project application-user identity.

#### Scenario: Visitor opens the hosted sign-in page
- **WHEN** self-service registration is ready
- **THEN** the page offers sign in and create-account paths and explains that new accounts require review before product access

#### Scenario: Operator approves from the console
- **WHEN** an authorized operator confirms approval with a reason
- **THEN** the console sends the same idempotent operator API operation, shows its committed result, and does not infer success from a notification attempt

### Requirement: Registration exposure is observable and deny-by-default
The reverse proxy SHALL expose only the documented public developer-auth and
wait-list self-service routes plus the already protected operator routes. It
MUST NOT expose private identity operations, raw storage, mail, metrics, or
session-issuance utilities. Registration, verification, authentication,
recovery, review, decision, mail-outbox, and denial signals SHALL be observable
with bounded cardinality and no raw email, password, token, reviewer note, IP
address, or session value in metric labels or ordinary logs. An operator MAY
explicitly accept the documented residual risks and select unrestricted
public-preview admission for one exact plan and blocker set without a calendar
expiry. That acceptance MUST remain revocable through an explicit manual pause
and MUST fail closed to source-restricted admission when its approval binding
or any non-waivable safeguard becomes invalid. Deploying a different release
MUST NOT by itself close admission; the approval records the release its
safeguards were measured on as provenance rather than as a binding.

#### Scenario: Public caller probes a private identity route
- **WHEN** an internet client requests an identity route outside the published allowlist
- **THEN** the proxy and service fail closed without invoking a private operation or revealing whether an identity exists

#### Scenario: Operator monitors wait-list health
- **WHEN** an authorized operator inspects registration health
- **THEN** the system reports aggregate rate, queue, decision, delivery, throttle, and failure signals without applicant identifiers or secrets in metrics

#### Scenario: Accepted preview remains open over time
- **WHEN** an operator has accepted the residual risks for the current plan and blocker set and every non-waivable safeguard remains valid
- **THEN** public-preview admission remains active without requiring periodic calendar-based renewal, and remains active across a release deployment

#### Scenario: Operator manually pauses public preview
- **WHEN** an authorized operator invokes the documented manual pause operation
- **THEN** public-preview admission atomically returns to source-restricted `pre_gate` without stopping private services or deleting wait-list state

#### Scenario: Persistent preview safety binding drifts
- **WHEN** the approval binding, TLS, readiness, backup, recovery, route-isolation, integrity, or emergency-stop safeguard becomes invalid
- **THEN** the guard returns admission to `pre_gate` even though the operator has not manually paused it

