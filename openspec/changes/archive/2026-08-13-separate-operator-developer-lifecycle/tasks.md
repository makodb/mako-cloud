## 1. Identity and Role Storage

- [x] 1.1 Inventory every `DeveloperAccount` status and `authorization_epoch` read/write, session binding, keyspace, migration, backup, restore, and audit consumer; record the required authority for each use.
- [x] 1.2 Add validated versioned authentication-identity and developer-role records with active/suspended/deleted account-security state, optional developer state, credential epoch, and developer epoch.
- [x] 1.3 Add additive RocksDB keyspaces, indexes, serialization compatibility, and atomic store operations for authentication identities and developer roles while retaining legacy keys for rollback.
- [x] 1.4 Implement a deterministic restart-safe legacy-account backfill with completion markers, duplicate/index conflict detection, and fail-closed invariant validation.
- [x] 1.5 Update the domain/store aggregate APIs so credential workflows use authentication identity state and developer workflows use developer role state without exposing password or recovery material.
- [x] 1.6 Add storage tests for identities with neither, either, pending-plus-operator, and both roles; interrupted migration resumption; legacy deleted/disabled mappings; conflict rejection; and redacted debug output.

## 2. Independent Developer Authorization

- [x] 2.1 Change developer registration and email verification to create/update authentication identity and developer role atomically, including the existing-email and absent-role cases.
- [x] 2.2 Bind developer access and refresh sessions to both credential and developer epochs, and validate the current account-security and developer-role states on every refresh/authorization decision.
- [x] 2.3 Make approval, rejection, developer-only disablement/restoration, and wait-list transitions advance only the developer epoch and revoke only affected developer sessions.
- [x] 2.4 Make password change/recovery and account-wide suspension/deletion advance the credential epoch and invalidate both developer and operator sessions without changing either role grant.
- [x] 2.5 Add developer workflow tests proving role-local transitions, account-wide revocation, absent-role denial, recovery behavior, and unchanged operator records.

## 3. Independent Operator Authentication and Entitlements

- [x] 3.1 Change operator sign-in eligibility to require a verified security-active authentication identity and explicit entitlement while ignoring absent, wait-listed, active, rejected, and developer-disabled role states.
- [x] 3.2 Change operator session records and authorization checks to bind credential and operator epochs only, migrate or reject legacy session records safely, and preserve expiry, revocation, step-up, permission, cookie, origin, and generic-error behavior.
- [x] 3.3 Change operator entitlement grant, replacement, revocation, and initial bootstrap to mutate only entitlement state and require no active developer role.
- [x] 3.4 Remove or fail closed on the legacy `activate_waitlisted` bootstrap option and replace combined lifecycle outputs with explicit before/after developer and operator state.
- [x] 3.5 Add cross-product operator tests for all developer states, missing/disabled entitlements, account-wide suspension, password recovery, permission replacement, developer decision isolation, and redacted enumeration-safe failures.

## 4. Self-Review and Repair

- [x] 4.1 Carry the stable authentication identity ID in authenticated operator principals and audit context without weakening the operator ID or permission boundary.
- [x] 4.2 Classify self-review in wait-list decisions while retaining recent-password verification, `waitlist_review`, private-reason validation, confirmation, idempotency, optimistic concurrency, outbox, and audit requirements.
- [x] 4.3 Add tests that self-approval and self-rejection affect only developer state, conflicting races commit once, failed safeguards change neither role, and the current operator session remains valid after approval.
- [x] 4.4 Implement the private `repair-bootstrap-developer-admission` plan/apply service with environment, identity, role-state, epoch, provenance, and idempotency bindings.
- [x] 4.5 Prove the repair accepts only the prior combined-bootstrap activation, moves only active developer state to wait-listed, preserves credential/entitlement/operator-session bytes and epochs, replays harmlessly, and rejects arbitrary or subsequently reviewed developers.
- [x] 4.6 Extend protected admin CLI/runbook handling so a concise human approval authorizes the agent/tool to supply the machine-generated content binding without asking the human to copy an opaque string.

## 5. APIs, Console, and Audit Presentation

- [x] 5.1 Update internal and public OpenAPI schemas and HTTP adapters with separate bounded developer-status and operator-entitlement fields, maintaining backward compatibility or explicit rejection for removed bootstrap fields.
- [x] 5.2 Extend wait-list list/detail responses with bounded operator status and extend operator session inspection with nullable developer status, without using presentation fields for authorization.
- [x] 5.3 Update the operator console to render separate “Developer status” and “Operator access” values and keep an entitled operator's own wait-list row fully reviewable.
- [x] 5.4 Update audit, logs, metrics, and qualification evidence so each event names the affected role, self-review is visible, unchanged companion state is explicit, and sensitive authentication data remains redacted.
- [x] 5.5 Add HTTP and browser tests for wait-listed operator sign-in, self-review UI, separate status labels, role-isolated session behavior, generic failures, CSRF/origin enforcement, and responsive layout.

## 6. Consistency and Qualification

- [x] 6.1 Reconcile `add-operator-password-signin` proposal, design, spec, and remaining task wording to remove the superseded active-developer eligibility and combined-bootstrap requirements without rewriting historical implementation evidence.
- [x] 6.2 Run formatting, linting, focused Rust and console tests, OpenAPI checks, full workspace tests, and strict OpenSpec validation; fix all regressions attributable to this change.
- [x] 6.3 Qualify fresh install, legacy migration, interrupted restart, checkpoint/backup, empty-target restore, replacement restore, session non-resurrection, and rollback with independent role records.
- [x] 6.4 Produce sanitized evidence proving each developer/operator state combination, developer-decision and entitlement isolation, password-recovery shared revocation, self-review safeguards, and repair refusal cases.

## 7. Public-Beta Deployment and Manual Admission

- [x] 7.1 Build and identify an immutable release candidate, update the release inventory, and stage it with public admission closed using the existing fail-closed deployment process.
- [x] 7.2 Back up the public-beta RocksDB state, install and converge the candidate, run migration/invariant checks, and prove the initial entitlement and independent role records survived.
- [x] 7.3 Qualify operator sign-in for a wait-listed fixture and cross-role isolation in restricted `pre_gate`, then obtain a concise human approval and activate the exact release under the persistent public-preview guard.
- [x] 7.4 Verify fresh external HTTPS, public registration, wait-list persistence, operator password sign-in, status presentation, service restart recovery, recurring guard health, and rollback readiness.
- [x] 7.5 Plan the provenance-bound repair for `msmummy@gmail.com`, present its exact state effects without an opaque copy-back token, and wait for a concise human approval before applying it.
- [x] 7.6 Apply the approved repair, verify `msmummy@gmail.com` remains an operator while appearing as a wait-listed developer, and verify no synthetic address is substituted for the real account.

## 8. Optional Review Reasons and Batch Approval

- [x] 8.1 Normalize omitted, null, empty, and whitespace-only wait-list reasons to one absent representation; retain 8-to-1,024-character safe-text validation when supplied; keep decision-record serialization compatible; bind the normalized value to idempotency; and audit bounded reason presence without exposing private contents.
- [x] 8.2 Make individual approve/reject reasons optional in the HTTP adapter, OpenAPI schema, generated API types, and management SDK while preserving permission, recent-password, origin/CSRF, confirmation, concurrency, idempotency, mail outbox, and role-isolation behavior.
- [x] 8.3 Update the operator console to label the individual reason optional and add accessible current-page selection, select-all-visible, one-confirmation batch approval for at most 25 applicants, distinct stable per-target idempotency, explicit committed/failed counts, authoritative refresh, and selection clearing on filter, pagination, or refresh.
- [x] 8.4 Add domain, HTTP, SDK, and browser tests for reason omission and normalization, invalid supplied reasons, individual self-approval without a reason, all-success batch approval, a stale partial failure, self-review within a batch, hidden-selection clearing, audit/outbox isolation, responsive layout, and accessible controls.
- [x] 8.5 Run formatting, linting, OpenAPI generation/checks, focused Rust and console tests, full workspace tests, production-release validation, and strict OpenSpec validation; fix all attributable regressions.
- [x] 8.6 Build and identify a new immutable release, stage it in restricted `pre_gate`, take and remotely verify fresh RocksDB backups, deploy and qualify optional-reason and batch behavior plus restart/rollback safety, update sanitized evidence, obtain concise approval for the exact new release binding, and reactivate the persistent public-preview guard.

## 9. Manual Developer Admission

- [x] 9.1 Have the user manually approve `msmummy@gmail.com` through `/operator` without a reason, then verify developer status is active, operator entitlement/session is unchanged, and exactly one durable self-review audit event records that no reason was supplied.
