# Account Role Lifecycle Specification

## Purpose

Define independent developer-admission and operator-authorization lifecycles for one authentication identity so each role can be granted, reviewed, and revoked without implicitly changing the other.

## Requirements

### Requirement: Authentication identity and role states are distinct
The system SHALL bind credentials and email verification to one stable authentication identity while storing developer lifecycle state and operator entitlement as independent, optional role state. It MUST be possible to represent an identity with neither role, either role, a pending developer application plus operator authority, or both active roles.

#### Scenario: Operator with a pending developer application
- **WHEN** an email-verified authentication identity has an active operator entitlement and a wait-listed developer application
- **THEN** the system reports the identity as an active operator and a wait-listed developer without converting either state

#### Scenario: Developer without operator authority
- **WHEN** an active developer identity has no operator entitlement
- **THEN** the system permits developer access and denies operator access without creating an operator entitlement

#### Scenario: Operator without developer admission
- **WHEN** an email-verified authentication identity has an active operator entitlement and no active developer role
- **THEN** the system permits operator access and does not create or activate a developer role

### Requirement: Operator eligibility does not depend on developer admission
The system SHALL determine operator eligibility from the shared authentication identity's email verification and account-wide security state plus an explicit active operator entitlement. Developer lifecycle states including absent, wait-listed, active, rejected, and developer-disabled MUST NOT independently grant or deny operator eligibility.

#### Scenario: Wait-listed operator signs in
- **WHEN** a wait-listed developer supplies correct credentials for an email-verified, security-active authentication identity with an active operator entitlement
- **THEN** the system creates the same bounded operator session it would create for an active developer

#### Scenario: Rejected developer retains operator eligibility
- **WHEN** a developer application is rejected for an identity that retains an active operator entitlement and no account-wide security restriction
- **THEN** existing and subsequent operator sessions remain governed by the operator entitlement rather than the developer rejection

#### Scenario: Entitlement is still mandatory
- **WHEN** an email-verified active developer without an operator entitlement submits correct credentials to operator sign-in
- **THEN** the system returns the generic operator-authentication failure and creates no operator session

### Requirement: Role administration has no cross-role side effects
The system SHALL apply developer decisions only to developer lifecycle state and operator entitlement decisions only to operator authorization state. A role-specific operation MUST NOT create, approve, reject, disable, revoke, or advance the authorization epoch of the other role, and its result and audit record SHALL report both unchanged and changed role state explicitly.

#### Scenario: Operator grant leaves developer wait-listed
- **WHEN** a protected administration operation grants operator permissions to a wait-listed developer identity
- **THEN** the operator entitlement becomes active and the developer application remains wait-listed

#### Scenario: Developer approval leaves operator entitlement unchanged
- **WHEN** a permitted operator approves a wait-listed developer application whose identity already has operator permissions
- **THEN** the developer lifecycle becomes active and the existing operator entitlement, permissions, and operator authorization epoch remain unchanged

#### Scenario: Operator revocation leaves developer active
- **WHEN** a protected administration operation revokes an active operator entitlement from an active developer
- **THEN** operator sessions are revoked and developer status and developer sessions remain unchanged

#### Scenario: Developer-only disable leaves operator authority unchanged
- **WHEN** an authorized operation disables only the developer role of an identity with an active operator entitlement
- **THEN** developer sessions are invalidated and operator entitlement and eligible operator sessions remain unchanged

### Requirement: Account-wide security actions remain shared
The system SHALL distinguish role-specific lifecycle decisions from account-wide authentication security actions. Password change or recovery, authentication-identity deletion, credential compromise response, and explicit account-wide suspension MUST revoke or invalidate all affected developer and operator sessions, while preserving role records unless the action explicitly removes them.

#### Scenario: Password recovery revokes both session types
- **WHEN** password recovery successfully changes credentials for an identity with developer and operator sessions
- **THEN** all prior developer and operator sessions for that authentication identity become invalid without changing its developer decision or operator permissions

#### Scenario: Account-wide suspension denies both roles
- **WHEN** an authentication identity is placed in an account-wide security-suspended state
- **THEN** both developer and operator authentication fail regardless of their independent role states

#### Scenario: Restoring authentication does not grant roles
- **WHEN** an account-wide security restriction is removed
- **THEN** the system resumes evaluating the previously stored developer and operator states independently and grants neither role implicitly

### Requirement: Operators can review their own developer application
The system SHALL allow an operator to approve or reject the developer application attached to the same authentication identity through the normal wait-list review operation. Self-review MUST require the same `waitlist_review` permission, recent password verification, explicit confirmation, idempotency, concurrency control, and durable audit record as review of another identity, and MUST NOT alter operator authority. A private review reason is optional under the same rules as every other wait-list decision.

#### Scenario: Operator manually approves own application
- **WHEN** an operator with `waitlist_review` permission and recent password verification submits a valid approval for their own wait-listed developer application
- **THEN** the system activates only the developer role, records the self-review relationship in the audit event, and leaves the operator entitlement unchanged

#### Scenario: Self-review lacks required safeguards
- **WHEN** an operator attempts to decide their own developer application without any required permission, freshness, confirmation, or idempotency safeguard
- **THEN** the system rejects the decision with the same stable error used for an equivalent review of another identity and changes neither role

#### Scenario: Concurrent self-review is deterministic
- **WHEN** two decisions race for the same operator's wait-listed developer application
- **THEN** exactly one valid transition commits, retries with the same idempotency input return its result, and conflicting input cannot alter the developer or operator state

### Requirement: Private wait-list review reasons are optional
The system SHALL accept individual approval and rejection decisions when the private review reason is omitted, null, empty, or whitespace-only. It MUST normalize all of those forms to one absent-reason representation before idempotency binding. A supplied non-empty reason MUST be trimmed, contain between 8 and 1,024 characters, and pass the existing safe-text validation. The system MUST record a bounded indication of whether a reason was supplied and MUST NOT invent placeholder reason text or expose private reason contents through public responses, logs, metrics, or retained qualification evidence.

#### Scenario: Operator approves without a reason
- **WHEN** an operator submits an otherwise valid approval with the reason omitted or blank
- **THEN** the developer decision commits, the audit event records that no reason was supplied, and all permission, freshness, confirmation, idempotency, concurrency, outbox, and role-isolation safeguards remain enforced

#### Scenario: Optional reason is supplied
- **WHEN** an operator submits an otherwise valid decision with a non-empty reason between 8 and 1,024 safe characters
- **THEN** the system binds the normalized reason to idempotency and records it under the existing private audit handling

#### Scenario: Supplied reason is invalid
- **WHEN** an operator supplies a non-empty reason that is too short, too long, or fails safe-text validation
- **THEN** the system rejects the decision and changes no developer or operator state

#### Scenario: Empty reason forms are idempotently equivalent
- **WHEN** an omitted reason and a whitespace-only reason are retried with the same otherwise identical idempotency input
- **THEN** the system treats both as the same absent-reason request rather than reporting a conflicting operation

### Requirement: Operators can batch approve a bounded visible wait-list page
The operator console SHALL allow an operator to select between 1 and 25 wait-listed applicants visible on the current page and approve them with one explicit confirmation. The selection MUST be cleared whenever the page, filter, or authoritative list result changes so an unseen applicant cannot remain selected. The batch MAY use one optional shared private reason, but each target MUST execute the ordinary single-applicant approval independently with its own idempotency binding, optimistic concurrency decision, developer-session revocation, notification work, audit event, and role-isolation checks. Batch approval is not atomic across targets and MUST report bounded per-target success or failure before refreshing authoritative state. Batch rejection is outside this change.

#### Scenario: Selected applicants are batch approved
- **WHEN** an eligible operator selects multiple visible wait-listed applicants, optionally enters one valid shared reason, confirms the named count, and starts batch approval
- **THEN** the console submits one independently idempotent approval per selected identity, reports the number committed, and refreshes the wait-list from authoritative server state

#### Scenario: One batch target is stale
- **WHEN** one selected applicant is no longer wait-listed while other selected applicants remain eligible
- **THEN** the stale target fails without rolling back or suppressing valid approvals, and the console reports both committed and failed counts without inferring final status for the failed target

#### Scenario: Operator includes their own application
- **WHEN** an entitled operator includes their own wait-listed developer application in a confirmed batch
- **THEN** that target follows the same self-review safeguards, changes only developer state, and leaves the operator entitlement and eligible operator session unchanged

#### Scenario: Selection would include a hidden target
- **WHEN** filtering, pagination, or refresh changes the set of visible applicants
- **THEN** the console clears the batch selection before another approval can be confirmed

### Requirement: Role state is presented and audited independently
Private administration plans, apply results, operator session inspection, and wait-list review data SHALL expose bounded developer status and operator-entitlement status as separate fields. Audit events MUST identify the affected role and MUST NOT describe an operator grant as developer approval or a developer decision as an operator grant.

#### Scenario: Wait-list row identifies an entitled operator
- **WHEN** an operator views a wait-list entry whose authentication identity also has an operator entitlement
- **THEN** the response and console show the developer application as wait-listed and the operator entitlement as active without implying developer approval

#### Scenario: Administration plan shows isolated effect
- **WHEN** a protected operator-entitlement change is planned
- **THEN** the plan identifies the operator entitlement transition and the unchanged developer status before any mutation occurs

#### Scenario: Sensitive data remains hidden
- **WHEN** role state is returned or recorded for administration, UI, logs, metrics, or qualification evidence
- **THEN** passwords, raw sessions, recovery secrets, and unbounded credential material are absent and existing privacy and redaction rules remain enforced

### Requirement: Existing combined bootstrap state can be repaired safely
The system SHALL provide a protected, environment-bound, idempotent repair operation for identities whose developer role was activated only as a side effect of the former operator bootstrap. The operation MUST plan and confirm an exact active-to-wait-listed developer transition, preserve operator entitlement and sessions, refuse ineligible or changed input, and record a durable role-specific audit event.

#### Scenario: Initial operator is returned to the wait list
- **WHEN** the exact public-beta identity previously auto-activated by operator bootstrap is eligible for repair and the protected operation is confirmed
- **THEN** its developer status becomes wait-listed, its operator entitlement remains active with unchanged permissions and epoch, and it can sign in as an operator to review the application normally

#### Scenario: Repair replay is harmless
- **WHEN** the same repair operation is replayed with identical environment and idempotency bindings
- **THEN** the system returns the committed result without another lifecycle transition or duplicate audit effect

#### Scenario: Repair cannot demote arbitrary developers
- **WHEN** the target was not activated by the former combined bootstrap, no longer has the expected state, or the environment or confirmation binding differs
- **THEN** the repair fails closed and changes neither developer nor operator state
