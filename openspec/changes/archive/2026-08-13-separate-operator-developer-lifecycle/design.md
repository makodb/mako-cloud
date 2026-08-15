## Context

The control plane currently stores credentials, email verification, developer lifecycle, and one `authorization_epoch` in `DeveloperAccount`. Operator entitlements are separate records, but operator sign-in and every operator-session check require `DeveloperIdentityStatus::Active` and bind the session to that developer epoch. Initial bootstrap can transition `Waitlisted` to `Active` in the same transaction as the entitlement grant. See `proposal.md` for why that coupling must be removed and `specs/identity/account-role-lifecycle/spec.md` for the resulting contract.

The hosted public beta contains an initial operator whose developer state was activated by that combined bootstrap and has now been repaired to `waitlisted` without changing operator access. Manual self-approval is intentionally pending while the review form is simplified and page-scoped batch approval is added. Storage remains service-owned RocksDB, migrations must be restart-safe and rollback-tolerant, privileged changes must remain auditable and idempotent, and a new release must pass the existing fail-closed public-preview process.

## Goals / Non-Goals

**Goals:**

- Give credential/security state, developer role state, and operator entitlement independent epochs and lifecycle transitions.
- Preserve one email/password login identity and the existing opaque identity ID while removing developer admission from operator eligibility.
- Preserve all current operator-session, step-up, permission, rate-limit, CSRF, audit, and wait-list review safeguards.
- Make a private review reason optional for individual approve/reject and add bounded current-page batch approval without weakening per-applicant authorization, idempotency, audit, notification, or role isolation.
- Repair only the initial developer activation caused by combined bootstrap, without interrupting operator access, so the user can approve that application through the console.
- Keep old data readable across an interrupted migration and keep rollback behavior explicit.

**Non-Goals:**

- Separate passwords, duplicate people, public operator signup, or automatic operator promotion.
- Weaken email verification, operator entitlement, recent-password, or account-security requirements.
- Add a general UI for operator-entitlement administration.
- Automatically approve the initial operator's developer application after repair.
- Rename public opaque `dev_...` identifiers in this change.

## Decisions

### 1. Split the stored aggregate into authentication and role records

Introduce a versioned authentication-identity record containing the stable identity ID, normalized email, display name, password hash, email-verification timestamp, account-wide security state, credential epoch, and authentication timestamps. Introduce a versioned developer-role record keyed by the same identity ID containing optional developer lifecycle state, developer authorization epoch, review summary, and role timestamps. Operator entitlement remains separately keyed by the identity ID.

The domain/store layer will return an aggregate view where existing call sites benefit from it, but authentication, developer authorization, and operator authorization will validate their own record and epoch. Existing names such as `DeveloperIdentityId` and compatibility response fields can remain aliases until a later API version; semantics and new internal fields will use `authentication_identity_id` where ambiguity would be unsafe.

This split supports a verified identity with no developer role and prevents a role transition from rewriting credential state. Keeping the current composite record and merely ignoring its status during operator login was rejected because its single epoch would still revoke operator sessions on developer decisions and could not represent an absent developer role correctly.

### 2. Use three independent revocation dimensions

- `credential_epoch` belongs to the authentication identity and advances on password change/recovery, compromise response, account-wide suspension/restoration, or identity deletion. Both developer and operator sessions bind to it.
- `developer_epoch` belongs to the developer role and advances on developer approval, rejection, role disablement, role restoration, or repair. Only developer sessions bind to it.
- `operator_epoch` remains on the entitlement and advances on permission replacement or revocation. Only operator sessions bind to it.

Developer sessions will store both credential and developer epochs. Operator sessions will store credential and operator epochs and will stop storing or comparing the developer epoch. Each authorization check still resolves current authoritative records rather than trusting only a session snapshot.

An account-wide security state has `active`, `suspended`, and `deleted` values. The existing developer `disabled` state becomes explicitly role-local. Legacy `deleted` records migrate to an account-wide deleted state as well as a non-active developer role; other legacy states migrate with an active authentication identity.

Using one shared epoch was rejected because it preserves cross-role revocation. Making password changes bump every role epoch was rejected because it obscures the cause and complicates audit, although the observable revocation result would be similar.

### 3. Make operator authentication depend only on identity security and entitlement

Operator password sign-in will resolve the authentication identity by normalized email, verify its password and email, require account-wide security state `active`, and require a valid non-empty operator entitlement. It will not load developer state for eligibility. Current-session authorization will compare credential and operator epochs and current permissions.

Developer sign-in and refresh will resolve the authentication identity plus developer role and continue to issue the active or wait-list audience appropriate to that role. A missing, rejected, or developer-disabled role cannot receive developer access but has no effect on operator authentication.

All operator-auth failures remain generic, and the dummy-password, rate-limit, cookie, one-hour lifetime, five-minute mutation freshness, origin, and audit behaviors remain unchanged.

### 4. Remove lifecycle mutation from operator entitlement administration

`InitialBootstrap`, `Grant`, `Replace`, and `Revoke` will plan and mutate only operator entitlement state. The `activate_waitlisted` input and `lifecycle_result` output will be removed or retained only as a rejected backward-compatibility field during a bounded transition. Plans and results will instead include separate `developer_status_before`, `developer_status_after`, `operator_status_before`, and `operator_status_after` fields; for entitlement operations the developer fields must match.

Entitlement grant eligibility requires a verified, security-active authentication identity, not an active developer role. Initial bootstrap remains singleton/idempotent, but no longer receives special authority over the developer lifecycle.

Keeping combined bootstrap as an optional flag was rejected because it leaves the unsafe coupling available and makes future operations ambiguous.

### 5. Treat self-review as an ordinary wait-list decision with explicit audit context

The authenticated operator principal will carry both the stable operator ID and authentication identity ID. The wait-list decision service will compare the principal identity with the target and record a bounded `self_review` classification when they match. It will not reject or relax the operation.

Permission enforcement, recent-password middleware, confirmation, idempotency binding, optimistic concurrency, mail outbox work, and audit commit remain identical to other reviews. An omitted or blank private reason is normalized to absence; a supplied reason retains the existing validation. Developer approval advances only `developer_epoch`; the operator entitlement and session stay valid, allowing the console response to complete normally.

A ban on self-review was considered but rejected because the user explicitly wants to exercise the same manual admission path and the beta's initial operator is the only reviewer. Automatic self-approval was rejected because it recreates the coupling being removed.

### 6. Extend bounded administration views with independent state

Wait-list API rows/details will add an operator-entitlement summary containing only `none` or `active` and, where useful, a stable operator ID; they will not expose entitlement reasons or unrelated permissions. Operator session/profile responses will add developer status as an independent nullable field for clarity, without using it for authorization. Protected administration plans/results and audit events will name the affected role and show the unchanged companion state.

The console will render separate “Developer status” and “Operator access” labels in review details. An entitled operator's own wait-list entry remains actionable. UI indicators are explanatory only; the server remains authoritative.

### 7. Add a narrowly proven repair for the prior combined bootstrap

Add a private plan/apply operation `repair-bootstrap-developer-admission`. Planning succeeds only when all of these hold: the environment binding matches; the target identity is active as a developer; an active entitlement exists; durable bootstrap idempotency/audit provenance shows that the same operation activated the developer; and no later developer review decision exists. The plan binds identity, environment, current role epochs/statuses, provenance digest, and requested transition.

Apply uses one RocksDB transaction to compare the plan state, move only the developer role to `waitlisted`, advance only `developer_epoch`, write a role-specific audit/idempotency record, and preserve credential and operator records byte-for-byte. Exact replays return the prior result. The operation is not exposed through public HTTP and cannot target an ordinary approved developer.

The tooling may retain a machine-consumed content-bound confirmation, but the deployment workflow will ask the human only for a short explicit go-ahead and will pass the generated binding itself. The user will not be asked to copy an opaque confirmation string.

### 8. Migrate additively and invalidate only sessions whose schema cannot be proven

On startup, a restart-safe migration will read each legacy `DeveloperAccount`, create authentication-identity and developer-role records with deterministic values, validate indexes, and mark completion only after all records are consistent. Reads during migration prefer new records and can reconstruct from legacy data until the migration marker commits. Legacy keys remain untouched for rollback.

Existing developer sessions are migrated or invalidated if they cannot be bound safely to both new epochs. Existing operator sessions are invalidated once because their stored developer epoch cannot prove the new credential-only binding; operators sign in again after deployment. Entitlements and their operator epochs are preserved. New releases never write role transitions back into legacy composite records after migration.

Backup/restore and rollback qualification will cover both keyspaces, incomplete migration resumption, independent epoch behavior, and non-resurrection of invalidated sessions. A rollback to a release that understands only combined state requires pausing public admission first because that code would again deny operator access to a wait-listed developer.

### 9. Reconcile the still-unarchived predecessor artifacts

Before either change is archived, revise `add-operator-password-signin` so it no longer claims that an active developer is required, a wait-listed identity is ineligible, or bootstrap activates developer state. Its completed implementation tasks remain historical evidence; the new change owns the migration and corrective implementation. This prevents later archive order from reintroducing contradictory requirements.

### 10. Keep optional reasons storage-compatible and orchestrate bounded batches in the console

The individual wait-list decision request will make `reason` optional. The HTTP adapter and workflow normalize omitted, null, empty, and whitespace-only values to the existing stored empty string, while a non-empty value must still contain 8 to 1,024 safe characters. `DeveloperDecisionRecord` therefore remains serialization-compatible: validation accepts either the canonical empty representation or the existing validated non-empty representation. Idempotency digests use the normalized value so equivalent empty forms cannot conflict. Audit output adds only a bounded `reason_provided` indication; private contents remain under existing redaction rules.

The management SDK will accept an optional reason for individual approve/reject. The console removes HTML and client-side required/minimum constraints while retaining the maximum length and non-empty validation. It labels the field optional so omission is deliberate rather than accidental.

Batch approval is a console orchestration over the existing single-applicant approval endpoint, not a new all-or-nothing server mutation. Checkboxes select only applicants visible in the current bounded 25-row page; selection is cleared on filter, pagination, refresh, or authoritative reload. One confirmation names the selected count and one optional shared reason is applied to every target. The console processes each target with a distinct stable idempotency key, continues after individual failures, reports committed and failed counts, and reloads authoritative state. This preserves per-target optimistic concurrency, mail outbox, audit, self-review classification, developer-session revocation, and role isolation while keeping partial completion explicit.

A new bulk endpoint was rejected because it would add a second decision contract, complicated cross-record transaction and replay semantics, and broader failure modes for a page-size administrative convenience. Client-side fire-and-forget was also rejected; the console awaits every outcome and never describes the batch as atomic.

## Risks / Trade-offs

- [Splitting a live identity aggregate can strand credentials or roles] → Use deterministic restart-safe backfill, invariant validation, legacy read fallback during migration, exact backup/restore qualification, and fail closed on mismatched records.
- [A developer decision might accidentally revoke an operator session through a remaining old epoch check] → Centralize session validation by authority, add cross-product lifecycle tests, and search for all legacy `authorization_epoch` consumers before deployment.
- [Allowing self-review reduces separation of duties] → Keep the full wait-list permission, recent-password, confirmation, concurrency, idempotency, and audit controls; record whether an optional reason was supplied; visibly classify self-review; revisit dual control before adding more operators or production tenants.
- [Reason-free decisions can lose useful operational context] → Keep the field visible and optional, preserve validated private reasons when supplied, and always retain actor, target, decision, time, self-review classification, reason-presence, idempotency, and resulting role state in the durable audit event.
- [Page-scoped batch approval can partially complete] → State that the action is non-atomic, use independent idempotency per target, continue through failures, show committed and failed counts, clear stale/hidden selection, and reload authoritative status after every run.
- [The one-time repair could demote a legitimately approved developer] → Require durable combined-bootstrap provenance and absence of a later review, bind the exact current state in plan/apply, and make the operation unavailable on arbitrary identities.
- [A rollback release cannot authenticate a wait-listed operator] → Pause public admission before rollback, retain protected break-glass recovery, and either restore the repaired developer role from captured pre-state or deploy forward.
- [Operator-only identities lack a public credential-enrollment workflow] → Represent and authorize them correctly now; continue to use protected identity provisioning until a separate invitation/recovery design is proposed.

## Migration Plan

1. Add split records, independent epochs, migration validation, and compatibility reads without changing public behavior; qualify restart, backup, restore, and rollback locally.
2. Switch developer sessions to credential plus developer epochs and operator sessions to credential plus operator epochs; update eligibility, entitlement administration, APIs, audit, console state, and tests.
3. Reconcile the predecessor OpenSpec artifacts and build an immutable public-beta candidate.
4. Stage the candidate with public admission closed, back up RocksDB, run the migration, verify entitlement preservation, and qualify operator sign-in for wait-listed and active fixtures plus cross-role isolation.
5. Deploy through the existing exact-release public-preview gate after a concise human approval; invalidate old operator sessions and verify fresh login.
6. Plan and apply the provenance-bound repair for the initial operator after a separate concise human approval. Confirm that operator login remains valid and that the real email appears as wait-listed.
7. Implement optional individual reasons and bounded page-scoped batch approval, qualify individual/self/batch decisions and partial failures, then build and stage a new immutable release with public admission closed. Back up RocksDB, deploy and qualify the exact release, obtain concise approval for the new release binding, and reactivate the persistent public-preview guard.
8. Have the user sign in at `/operator` and manually approve their own developer application without a reason. Verify the developer status becomes active, operator access remains active, and one self-review audit event records that no reason was supplied.

Rollback pauses public admission, captures current role state, selects the prior immutable release through the normal release operation, and restores the initial operator's developer state to the captured pre-repair value if the old release requires it. Public admission remains closed until operator recovery and storage consistency are proven.
