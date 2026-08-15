## Context

> Supersession note (2026-08-12): `separate-operator-developer-lifecycle` retains the shipped
> password-session design and historical qualification evidence, but replaces every dependency on
> active developer admission and every combined bootstrap mutation with independent identity,
> developer-role, and operator-entitlement state.

The hosted console currently accepts a manually issued `mako-operator` bearer token, validates it in browser session storage, and sends it to `/v1/operator/` routes. Developer password authentication and lifecycle state already live in the control plane's service-owned RocksDB database, while operator authorization is deliberately separate and currently derives short-lived tokens from the internal deployment secret. See `proposal.md` for the usability motivation and `specs/identity/operator-authentication/spec.md` for the behavioral contract.

The design must preserve the existing audience and permission boundary, exact Caddy route allowlist, generic credential failures, transactional audit behavior, exclusive RocksDB ownership, backup/restore guarantees, and fail-closed public-preview release process. It must not create public operator registration or let browser code read a privileged session credential.

## Goals / Non-Goals

**Goals:**

- Reuse one verified, security-active authentication identity's password while issuing a distinct operator authority independent of developer admission.
- Make routine browser login and sign-out understandable without token minting or copying.
- Keep operator permission assignment non-public, explicit, least-privilege, reversible, and immediately enforceable.
- Make every session revocable and every mutation require recent verification plus the existing action safeguards.
- Bootstrap and qualify the initial public-beta operator without embedding an email address or credential in source, release artifacts, logs, or evidence.

**Non-Goals:**

- Public operator signup, invitations, self-promotion, or a web UI for changing operator entitlements.
- Treating developer, wait-list, project-member, or application-user sessions as operator sessions.
- Adding TOTP, WebAuthn, enterprise SSO, or a general external identity provider in this change.
- Changing the meaning of the existing six operator permissions or removing action-specific reasons, confirmations, idempotency, and audits.
- Extending operator sessions beyond one hour without another successful password verification.

## Decisions

### 1. Share credential verification, not authorization or sessions

Operator sign-in locates the shared authentication identity by the normalized email index and uses the existing Argon2id password verifier and opportunistic hash upgrade. Eligibility requires verified email, account-security state `active`, and an explicit operator entitlement; absent, wait-listed, active, rejected, and developer-disabled role states are presentation only. Success creates a new operator identity context and never returns or upgrades a developer JWT.

The entitlement record will bind a stable opaque `opr_...` identifier to the developer identity, an exact non-empty subset of the existing operator permissions, an operator authorization epoch, and lifecycle timestamps. Operator requests will resolve the current record on every authorization decision rather than trusting only a session's permission snapshot.

Alternatives considered:

- Reusing a developer JWT with an `operator` claim collapses the existing boundary and makes ordinary developer-session theft sufficient for administration; rejected.
- Maintaining a second password creates another recovery and credential lifecycle for the same human; rejected for this beta.
- Keeping pasted file tokens as the primary flow preserves the usability problem; retained only for explicitly enabled break-glass recovery.

### 2. Use opaque server-side operator sessions in an HttpOnly cookie

`POST /v1/operator-auth/sessions` will accept bounded JSON `{email, password}` only from the configured origin. On success the control plane generates a cryptographically random credential, stores only a keyed digest with the operator/developer IDs, operator epoch, effective-permission snapshot, issuance time, last-password-verification time, and expiry, and sets a `Secure; HttpOnly; SameSite=Strict; Path=/v1` cookie. The response contains only the bounded operator profile, permissions, verification freshness, and expiry.

The same bounded shape is returned by `GET /v1/operator-auth/sessions/current`. `DELETE /v1/operator-auth/sessions/current` transactionally revokes the stored session and expires the cookie. No refresh credential is issued; the absolute expiry is at most one hour. The control plane will prune bounded expired session and attempt records using the existing maintenance model.

All state-changing operator-auth requests require exact `Origin`, JSON where applicable, and the existing request-size and in-flight limits. Caddy forwards the cookie only to the exact control-plane routes; unrelated services ignore it. Operator APIs accept the cookie through the new authenticator and continue to enforce explicit permissions. Developer and wait-list bearer tokens fail before the handler.

Alternatives considered:

- A JavaScript-readable operator JWT would recreate the token handling and XSS exposure this change removes; rejected.
- A stateless signed cookie is simpler but cannot provide immediate per-session revocation without relying only on epochs; rejected.
- A long-lived refresh token improves convenience but expands privileged credential lifetime and theft recovery; rejected.

### 3. Reuse generic password defenses with an operator-specific attempt budget

The sign-in path will perform constant-shape validation and return one generic failure for unknown email, wrong password, ineligible lifecycle, missing entitlement, or disabled authority. Per-source and keyed-normalized-identity counters will have bounded cardinality, exponential backoff, expiry, and sanitized metrics. A dummy password hash verification will preserve comparable work when no eligible identity is found. Passwords, normalized emails, raw cookies, and attempt keys will not enter logs or metrics.

Successful authentication resets only the applicable attempt state and records a redacted operator-auth audit event. Throttling returns a generic `429` with bounded `Retry-After`; infrastructure failure fails closed without falling back to developer authentication or file tokens.

### 4. Gate every privileged mutation on five-minute password freshness

`POST /v1/operator-auth/sessions/current/actions/verify-password` accepts only the current operator's password, never an email, and transactionally advances that session's `password_verified_at` after successful verification. It uses the same attempt budget and generic errors as sign-in.

The shared operator authorization middleware will distinguish read permission from mutation freshness. Every mutating `/v1/operator/` operation checks that the current session verified the same identity's password no more than five minutes earlier. A stale session returns a stable `operator_step_up_required` error before parsing or committing the mutation. After the console completes step-up, it retries the original operation with the same idempotency key; it never stores the password or changes the operator identity.

The existing reason, confirmation, permission, idempotency, tenant binding, and audit checks remain authoritative. UI feature hiding is convenience only; the server performs all enforcement.

### 5. Make security events converge through epochs and explicit revocation

Operator sessions carry the shared credential epoch and operator epoch observed at issuance. Password change/recovery, account-wide suspension/deletion, or a credential-security epoch advance invalidates all operator sessions. Developer approval, rejection, or developer-only disablement advances only the developer epoch and leaves operator sessions eligible. Entitlement replacement increments the operator epoch and revokes sessions before returning; revocation removes the entitlement and does the same. Each request compares stored session epochs with current authoritative records.

This couples the shared password's compromise recovery to operator access without merging authorization. It also avoids a stale permission snapshot surviving a grant reduction.

### 6. Keep entitlement administration behind a private, transactional control-plane operation

A release-owned `mako-operator-admin` client calls a loopback-only control-plane endpoint authenticated by the protected internal credential; it never opens the live RocksDB database directly. Its plan/apply workflow accepts a mode-`0600` input containing the target email, exact permission set, private reason, idempotency key, and environment binding. Planning resolves one authentication identity and prints a confirmation bound to the environment, identity, email digest, operator transition, unchanged developer status, and permission digest. Apply requires the exact typed confirmation and commits only entitlement, operator epoch, idempotency, and audit effects atomically.

The historical initial public-beta operation named `msmummy@gmail.com`, granted all six permissions, and also activated its wait-listed developer role. That completed evidence remains historical. Current bootstrap and ordinary entitlement operations require only a verified, security-active authentication identity and never change developer state. The successor change provides one provenance-bound repair for the historical combined activation. Replays with identical input return the committed result, while mismatches fail without mutation.

Future grants, permission replacement, and revocation use the same private workflow. A public operator-management API is deferred until a phishing-resistant identity and delegation design exists.

### 7. Replace the console adapter with credentialed same-origin operations

The hosted operator adapter will call session inspection on startup, submit email/password to sign in, use `credentials: include` for operator APIs, and call server-side sign-out. The `/operator` anonymous view contains email and password inputs and no token input. Authenticated UI renders only features in the current permission set and displays the stable opaque operator identity plus the account email returned by session inspection.

On `operator_step_up_required`, a modal requests only the current password, clears it immediately after the request, and retries the pending idempotent action after success. Any unauthenticated, revoked, or epoch-stale response clears privileged response and selection state before returning to sign-in. Generic public text does not distinguish wrong password from ineligibility.

### 8. Retain file-token support only as disabled-by-default break glass

The existing issuer remains release-owned for recovery, but hosted configuration defaults `operator_break_glass_bearer_enabled` to false and removes the browser paste UI. When disabled, the operator authenticator rejects bearer tokens. Enabling it requires protected configuration, an incident reason, and a service convergence; issuance requires a reason and records a redacted private audit event. Tokens remain least-privilege and at most one hour.

This supplies rollback and credential-store recovery access without leaving two routine public authentication paths. It is not a bypass for mutation step-up: break-glass sessions receive a distinct authentication-method marker, and any allowed mutation must satisfy an explicitly documented incident-mode freshness rule and the existing safeguards.

### 9. Treat all new records as additive production RocksDB state

Operator entitlements, session digests, attempt state, idempotency records, and audit links use versioned additive keyspaces owned only by `mako-control-plane`. Migrations are restart-safe and rollback-tolerant: the prior release ignores new prefixes, and the new release can resume pruning after rollback. Checkpoint, empty-target restore, replacement restore, and release rollback tests will cover entitlements, current epochs, revoked/expired-session non-resurrection, bounded attempt records, and initial bootstrap idempotency.

## Risks / Trade-offs

- [A developer password now protects both developer and operator entry] → Require an explicit entitlement, separate session, one-hour maximum lifetime, five-minute mutation freshness, rate limits, immediate epoch revocation, and plan a later phishing-resistant MFA change.
- [Password-only operator login is phishable] → Keep permissions least-privilege, retain short sessions and step-up, alert on anomalous attempts, and treat WebAuthn or SSO as the next security upgrade before broader production administration.
- [Cookie authentication introduces CSRF concerns] → Use `SameSite=Strict`, exact-origin checks, JSON-only mutations, exact Caddy routes, and no permissive cross-origin credential policy.
- [Account-wide security actions remove both kinds of access] → Distinguish them from developer-only decisions in UI/runbooks and retain controlled break-glass recovery.
- [Historical bootstrap combined wait-list activation with a powerful grant] → Prohibit that combination now and allow only the exact provenance-bound repair under protected plan/apply controls.
- [Rollback to the token-only console can lock out routine password access] → Preserve the disabled-by-default issuer and document a bounded break-glass enablement before rollback.

## Migration Plan

1. Add the versioned RocksDB records, operator-auth service, private entitlement workflow, OpenAPI routes, console adapter, and additive Caddy configuration while keeping password operator auth disabled and the current bearer flow available in `pre_gate`.
2. Run storage, auth-boundary, enumeration, rate-limit, cookie, CSRF, session-revocation, step-up, audit-redaction, restart, backup, restore, and rollback qualifications from the exact candidate.
3. On the public-beta guest, use the protected plan/apply workflow to grant the exact six-permission entitlement without altering developer admission. Retain the historical combined-bootstrap evidence and use the successor repair only for its exact proven target.
4. In restricted `pre_gate`, enable password sign-in, verify every operator API and permission family with controlled fixtures, verify recovery and revocation, and prove that developer/wait-list tokens and disabled break-glass tokens fail without collecting or recording the intended operator's password.
5. Switch the hosted console to password login, disable routine bearer acceptance, rerun exact-release controlled browser and external HTTPS qualification, create a new exact-release public-preview risk approval, and then activate public admission.
6. After public admission is active, have the intended operator complete one browser password sign-in without sharing or recording the password, finish the external operator-console checks, and only then remove any previously issued local operator token.

Rollback pauses public admission, revokes new operator sessions, enables a bounded reason-bound break-glass token only if administration is required, and selects the prior immutable release through the normal release operation. Additive keyspaces remain untouched. Before reopening, verify the selected release's expected auth method, entitlement state, storage recovery, and exact-release approval.
