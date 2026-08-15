> Supersession note (2026-08-12): completed checkboxes below retain historical implementation
> evidence. Current eligibility, session-epoch, bootstrap, and developer-isolation requirements are
> governed by `separate-operator-developer-lifecycle` and are not reopened here.

## 1. Contract and Persistent Model

- [x] 1.1 Add OpenAPI schemas and exact same-origin routes for operator password sign-in, current-session inspection, sign-out, and current-session password step-up, including stable generic and `operator_step_up_required` errors.
- [x] 1.2 Regenerate management SDK types and route inventories, then update console/API/Caddy parity assertions for the new `/v1/operator-auth/` family.
- [x] 1.3 Add versioned control-plane RocksDB records and codecs for operator entitlements, stable opaque operator IDs, operator epochs, protected session digests, attempt budgets, revocations, idempotency, and audit links.
- [x] 1.4 Add restart-safe additive migrations, bounded record validation, expiry pruning, and state-size limits without allowing any second process to open the live RocksDB database.
- [x] 1.5 Test malformed records, duplicate email resolution, invalid permission sets, epoch monotonicity, record bounds, migration replay, and prior-release tolerance of the additive keyspaces.

## 2. Operator Entitlement and Authentication Core

- [x] 2.1 Implement the transactional entitlement service for exact permission grant, replacement, and revocation with private reason, idempotency, typed binding, operator-epoch advance, session revocation, and redacted audit events.
- [x] 2.2 Implement eligibility resolution requiring one active, email-verified developer identity with a non-empty explicit operator entitlement; prove registration, wait-list approval, membership, and developer sessions never imply it.
- [x] 2.3 Reuse the existing Argon2id password verifier and hash-upgrade path for operator sign-in, including dummy verification and constant-shape generic failures for unknown, invalid, inactive, unverified, and ineligible identities.
- [x] 2.4 Implement bounded per-source and keyed-identity attempt budgets, exponential backoff, expiry, generic `429` responses, and sanitized metrics and audit classifications.
- [x] 2.5 Implement cryptographically random operator credentials, keyed server-side digests, at-most-one-hour absolute expiry, protected session records, and bounded current-session responses.
- [x] 2.6 Implement operator cookie authentication that resolves current entitlement and both authorization epochs on every request and rejects developer, wait-list, application-user, malformed, expired, revoked, wrong-audience, and stale credentials.
- [x] 2.7 Implement sign-out and explicit per-session and per-identity revocation so later use fails immediately and session records remain bounded.
- [x] 2.8 Implement same-identity password step-up and five-minute mutation freshness without retaining the submitted password or allowing an identity switch.
- [x] 2.9 Integrate password change/recovery, developer disable/delete, entitlement changes, and security epoch advances with atomic revocation of every operator session.
- [x] 2.10 Add unit and concurrency tests for eligibility, enumeration resistance, attempt races, session digest secrecy, expiry boundaries, epoch drift, entitlement reduction, revocation, and step-up identity binding.

## 3. HTTP Boundary and Existing Operator APIs

- [x] 3.1 Implement bounded `/v1/operator-auth/` HTTP handlers with exact-origin validation before password work, JSON and body limits, generic public errors, request IDs, and no credential or raw-cookie logging.
- [x] 3.2 Set and expire the operator cookie with `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/v1`, no `Domain`, bounded `Max-Age`, and `Cache-Control: no-store`; return no JavaScript-readable credential.
- [x] 3.3 Extend the shared operator middleware to accept current password-backed cookies, enforce exact permissions, and return step-up-required before parsing or committing any mutating operator action.
- [x] 3.4 Preserve the existing reason, confirmation, idempotency, tenant binding, support-expiry, and audit rules for wait-list, provisioning, quota, abuse, and support operations after the authentication change.
- [x] 3.5 Update Caddy's generated exact route allowlist and headers for operator-auth endpoints while keeping private identity, password, storage, issuance, and internal administration endpoints unreachable.
- [x] 3.6 Add HTTP integration tests for successful lifecycle, generic failure parity, rate limits, login CSRF, state-changing CSRF, cookie scope and flags, cross-audience rejection, permission denial, step-up, sign-out, and fail-closed dependency errors.

## 4. Protected Entitlement Administration and Break Glass

- [x] 4.1 Add an internal-authenticated loopback-only control-plane plan/apply API for operator entitlement grant, replacement, revocation, and the explicitly confirmed initial wait-list activation bootstrap.
- [x] 4.2 Build the release-owned `mako-operator-admin` client with mode-`0600` protected input, exact environment and identity resolution, email and permission digests, idempotency, private reason, and content-bound typed confirmation.
- [x] 4.3 Ensure bootstrap apply atomically activates only an already verified target when explicitly planned, grants the exact permission set, and emits sanitized evidence without email, password, session, or private reason.
- [x] 4.4 Add ambiguity, unverified-target, stale-plan, environment-mismatch, permission-mismatch, replay, concurrent apply, audit-redaction, and crash-atomicity tests for the private workflow.
- [x] 4.5 Add `operator_break_glass_bearer_enabled` as a fail-closed hosted setting that defaults false and makes the existing operator authenticator reject file tokens when disabled.
- [x] 4.6 Require an incident reason and least-privilege permission set in the file-token issuer, record a redacted private issuance event, preserve the one-hour maximum, and define mutation freshness behavior for enabled incident mode.
- [x] 4.7 Test disabled bearer rejection, explicit enablement, expiry, revocation, wrong audience, missing incident context, and rollback access without restoring the browser paste workflow.

## 5. Hosted Console and SDK Experience

- [x] 5.1 Replace the hosted operator token adapter with a same-origin cookie adapter that inspects, creates, and deletes server-side sessions using credentialed requests.
- [x] 5.2 Replace the `/operator` bearer-token form with accessible email and password inputs, generic failure and throttling states, pending state, current operator profile, and normal sign-out.
- [x] 5.3 Render operator panels and actions only when their exact current permissions are present while keeping server authorization authoritative.
- [x] 5.4 Add a password-only step-up dialog that clears input after submission, never switches identity, and retries the pending mutation with its original idempotency key only after success.
- [x] 5.5 Clear applicant, tenant, support, quota, abuse, and other privileged client state before returning to sign-in on expired, revoked, stale-epoch, or unauthorized responses.
- [x] 5.6 Remove routine token-paste code and session-storage operator credentials from the production bundle while retaining test-only adapters where needed.
- [x] 5.7 Add unit and Playwright coverage for password login, restored cookie session, sign-out, generic failures, throttling, permission-shaped UI, step-up and retry, revocation during use, no token input, and absence of a JavaScript-readable operator credential.
- [x] 5.8 Make full-span operator panels use the available desktop workspace width and add measured browser regression coverage without changing the mobile stacking behavior.

## 6. Production Configuration, Observability, and Operations

- [x] 6.1 Add validated production settings for password-operator enablement, session lifetime, step-up freshness, attempt budgets, cookie policy, and disabled-by-default break glass, with safe `pre_gate` rollout defaults.
- [x] 6.2 Wire the operator-auth service graph through the existing control-plane password verifier, entitlement/session repositories, audit sink, maintenance worker, readiness checks, and service-owned RocksDB path.
- [x] 6.3 Add bounded sanitized metrics and alerts for sign-in outcomes, throttling, active sessions, revocations, step-up requirements, bootstrap outcomes, and break-glass state without email, source address, attempt keys, or credentials.
- [x] 6.4 Update systemd, Ansible, environment examples, Caddy validation, release manifest inputs, and configuration-plan sanitization for every new setting and protected bootstrap input.
- [x] 6.5 Update operator, registration, password recovery, incident access, deployment, backup/restore, and rollback runbooks with the password flow and explicit authority-revocation behavior.
- [x] 6.6 Add a safe cleanup procedure for expired local token files and prove it never removes protected deployment secrets or unrelated operator material.

## 7. Security, Durability, and Release Qualification

- [x] 7.1 Extend auth and tenant-boundary qualifications with developer/operator session isolation, cookie and CSRF defenses, enumeration timing classes, brute-force bounds, permission enforcement, session fixation resistance, and audit redaction.
- [x] 7.2 Extend operator API qualifications so every mutating route proves five-minute password freshness plus its pre-existing permission, reason, confirmation, idempotency, and audit contract.
- [x] 7.3 Extend production RocksDB, restart, checkpoint, empty-target restore, replacement restore, and release rollback qualifications for entitlements, epochs, idempotency, bounded attempts, and non-resurrection of expired or revoked sessions.
- [x] 7.4 Add exact-release hosted browser qualification for login, session restoration, all six permission families, step-up, sign-out, authorization loss, no token form, HTTPS, security headers, and exact public routes.
- [x] 7.5 Run formatting, type checks, unit, integration, Playwright, security, storage, service, Caddy, infrastructure, release-manifest, and strict OpenSpec validation; retain only sanitized evidence.
- [x] 7.6 Keep the outer local edge runtime alive beyond the supervised function wall limit and prove adversarial timeout qualification returns the stable contained error without relaxing the assertion.

## 8. Public-Beta Migration and Activation

- [x] 8.1 Build, validate, stage, and offline-install a new immutable release while leaving the current public release selected and its admission guard active.
- [x] 8.2 Pause public admission, promote the exact candidate through the fail-closed release operation, and converge in restricted `pre_gate` with password operator auth initially disabled and bounded break-glass access still available for rollback.
- [x] 8.3 Create a protected bootstrap plan for the verified `msmummy@gmail.com` identity with all six existing operator permissions, present its exact typed confirmation, and wait for explicit operator approval before apply.
- [x] 8.4 Apply the confirmed bootstrap once, prove active developer lifecycle and entitlement state without exposing the email or private reason in evidence, and verify idempotent replay returns the committed result.
- [x] 8.5 Enable password operator auth in restricted `pre_gate` and verify all operator APIs with controlled fixtures without collecting or recording the intended operator's password.
- [x] 8.6 Disable routine bearer acceptance, retain the prior local token without accepting it on public routes until successful public browser password login, and prove password recovery, entitlement reduction, revocation, and documented break-glass rollback behavior.
- [x] 8.7 Rerun exact-release hosted security, durability, streaming, benchmark, mail, registration, operator-auth, browser, recurring-guard, and recovery qualifications; update measurement and release-gate evidence.
- [x] 8.8 Generate a new release-, plan-, and blocker-bound public-preview approval request, wait for the exact required confirmation, activate the persistent preview guard only after approval, and verify repeated timer evaluations.
- [x] 8.9 After the successor change repairs the historical combined bootstrap and the intended operator manually self-approves through `/operator`, verify password sign-in without sharing or recording the password, remove any prior local token only after success, and verify externally that developer and operator statuses remain independently authoritative.
- [x] 8.10 If any bootstrap, login, authorization, recovery, release, or admission check fails, keep or return admission to `pre_gate`, preserve the protected state and evidence, and execute the documented immutable rollback rather than weakening authentication.
