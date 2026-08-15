## 1. Persistent Developer Identity and Migration

- [x] 1.1 Add validated developer identity, lifecycle, authorization-epoch, credential, session, token, review, idempotency, rate-limit, and mail-outbox domain models with explicit bounds and redacted debug behavior.
- [x] 1.2 Add control-plane RocksDB keyspaces and transactional repository operations for identity records, normalized-email uniqueness, lifecycle indexes, digested tokens and sessions, decision journals, and bounded outbox state.
- [x] 1.3 Implement the explicit `unverified`, `waitlisted`, `active`, `rejected`, and `disabled` state machine with legal-transition checks and an authorization-epoch advance on every authority-changing mutation.
- [x] 1.4 Implement a versioned resumable migration that validates normalized-email uniqueness, backfills existing developer identities as verified and active, initializes epochs, and refuses ambiguous collisions before registration can be enabled.
- [x] 1.5 Add repository and migration tests for atomic uniqueness, legal and illegal transitions, concurrent compare-and-set behavior, restart recovery, existing-identity preservation, collision refusal, and bounded cleanup.

## 2. Hosted Credentials, Tokens, Sessions, and Abuse Controls

- [x] 2.1 Implement developer password validation and the approved memory-hard hash and verification path with bounded concurrency, constant-safe comparisons, redaction, and transparent parameter-upgrade support.
- [x] 2.2 Implement random single-use email-verification and password-recovery token issuance, digest-only persistence, expiry, replacement, consumption, replay rejection, and cleanup.
- [x] 2.3 Implement short-lived active and wait-list access-token audiences plus rotating digest-stored refresh sessions with identity, lifecycle, epoch, issuer, audience, session, expiry, and cookie-policy binding.
- [x] 2.4 Update developer authentication to load current persistent lifecycle and epoch on every protected request, accept only the appropriate audience, and revoke incompatible sessions on approval, rejection, disablement, recovery, or sign-out.
- [x] 2.5 Restrict `mako-control-session` bootstrap and recovery tokens to a persisted currently active developer and current epoch so protected signing material cannot bypass wait-list activation.
- [x] 2.6 Implement bounded global, source-digest, normalized-email-digest, token-attempt, password-work, and outbox-growth controls with enumeration-safe stable outcomes and finite retention.
- [x] 2.7 Add adversarial authentication tests for duplicate-email enumeration, timing and response parity, password limits, token expiry/replay, refresh rotation/replay, audience confusion, stale status claims, epoch revocation, cookie attributes, throttling, and secret redaction.

## 3. Registration and Operator Review Workflows

- [x] 3.1 Implement enumeration-safe registration that atomically creates one unverified identity and verification outbox item only when registration and mail readiness are available.
- [x] 3.2 Implement bounded verification resend and token consumption that invalidates superseded material and transitions a verified self-registration only to `waitlisted`.
- [x] 3.3 Implement sign-in, refresh, sign-out, and coarse self-status workflows that issue wait-list authority to pending applicants and normal developer authority only to active identities.
- [x] 3.4 Implement enumeration-safe password-recovery request and completion with single-use tokens, epoch advance, and all-session revocation.
- [x] 3.5 Implement operator-authorized wait-list listing and detail lookup with deterministic ordering, stable cursor pagination, bounded filters and pages, and separate private review details.
- [x] 3.6 Implement idempotent atomic approve and reject operations with explicit wait-list-review permission, bounded reason, request correlation, compare-and-set state, epoch advance, session revocation, audit, and decision notification outbox writes.
- [x] 3.7 Add workflow tests for registration retries, verification replacement, pending isolation, approval and rejection, losing concurrent decisions, changed idempotency digests, fresh-sign-in enforcement, no automatic membership, operator denial, and restart-safe recovery.

## 4. Durable Mail Delivery

- [x] 4.1 Define a bounded mail-delivery boundary and redacted verification, recovery, approval, and rejection templates whose links and tokens are purpose-, origin-, identity-, and expiry-bound.
- [x] 4.2 Implement the persistent outbox worker with transactional claim leases, deterministic delivery identifiers, retry backoff, lease recovery, terminal dead-letter state, retention, and no repeated lifecycle mutation.
- [x] 4.3 Implement the production authenticated SMTP adapter with protected credential files, TLS verification, sender configuration, request timeouts, response bounds, readiness, and safe error mapping.
- [x] 4.4 Add mail and outbox tests for configuration failure, transient and permanent delivery failure, ambiguous retry, worker crash and restart, lease expiry, duplicate suppression, token secrecy, message bounds, and redacted observability.

## 5. Public and Operator API Contracts

- [x] 5.1 Add OpenAPI wire models, stable error responses, exact methods, request and response bounds, cookie behavior, and route inventory for the `/v1/developer-auth/` registration, verification, session, recovery, and wait-list-status family.
- [x] 5.2 Add OpenAPI wire models and exact protected routes for bounded `/v1/operator/developer-waitlist` list, detail, approve, and reject operations with reason and idempotency requirements.
- [x] 5.3 Register the public developer-auth handlers in the production control-plane graph with dependency-specific readiness, request IDs, rate limits, audit correlation, and exact audience enforcement.
- [x] 5.4 Register the operator wait-list handlers behind the separate operator authenticator and explicit review permission, with no developer, application-user, or wait-list-token fallback.
- [x] 5.5 Regenerate the management SDK types and clients and add contract tests for every new public and operator operation, error, cursor, lifecycle state, and cookie response.
- [x] 5.6 Add route-level integration tests for malformed and over-limit bodies, wrong methods, unknown routes, enumeration parity, audience confusion, CSRF/origin handling, operator authorization, concurrency, and dependency failure.

## 6. Developer and Operator Console Experiences

- [x] 6.1 Implement the hosted developer-auth adapter for same-origin registration, verification, sign-in, refresh, recovery, sign-out, active sessions, and wait-list sessions without URL or local-storage credential persistence.
- [x] 6.2 Add accessible create-account, check-email, verify-email, sign-in, forgot-password, and reset-password routes and forms with generic enumeration-safe messaging and clear application-user/developer separation.
- [x] 6.3 Add the wait-list status screen and route guard so a pending session can view only its coarse status and sign-out controls, keeps recovery in the logged-out flow, and cannot render or call product workflows.
- [x] 6.4 Implement a hosted beta operator-auth adapter that accepts only a separately issued short-lived operator-audience token with wait-list-review permission and keeps it tab-scoped.
- [x] 6.5 Add an operator wait-list queue with bounded filters and pagination, applicant detail, explicit approve and reject confirmations, required private reason, idempotent submission, conflict handling, and authoritative committed status.
- [x] 6.6 Add console unit, accessibility, and browser end-to-end tests covering public registration, pending isolation, verification and recovery, approval and rejection, operator/developer token separation, notification failure messaging, and fresh sign-in after approval.

## 7. Deployment, Security, Recovery, and Documentation

- [x] 7.1 Add registration feature flags, public origin, token/session lifetimes, rate limits, retention, mail/outbox limits, and secret-file configuration with production validation that defaults registration to disabled.
- [x] 7.2 Update Caddy to proxy only the generated developer-auth and protected operator routes in each admission mode, preserve request and cookie security headers, and keep private identity, mail, metrics, storage, and issuance listeners unreachable.
- [x] 7.3 Add aggregate bounded-cardinality metrics, dashboards, and alerts for registration, verification, sign-in, recovery, queue depth and age, decisions, throttles, outbox delivery, dead letters, and dependency readiness without identity or source labels.
- [x] 7.4 Extend semantic health, checkpoint backup, empty-target restore, guest-restart, release rollback, and migration checks to cover identity/index parity, lifecycle counts, epochs, sessions, tokens, decision journals, and outbox state.
- [x] 7.5 Update API, authentication, privacy, threat-model, configuration, deployment, operator, mail-failure, registration-disablement, recovery, rollback, and public-preview runbooks plus requirements traceability.
- [x] 7.6 Add offline infrastructure validators for disabled-by-default registration, protected mail credentials, authenticated TLS mail, exact Caddy routes, operator separation, alert inventory, secret scanning, and release-manifest inclusion.

## 8. Qualification and Public-Beta Deployment

- [x] 8.1 Run format, lint, type, unit, integration, API generation, SDK, console, security, documentation, infrastructure, production RocksDB, backup/restore, rollback, and strict OpenSpec validation suites for the exact candidate.
- [x] 8.2 Build a checksummed immutable release, move public admission to `pre_gate`, install it through the safe release operation, run the identity migration with registration disabled, and prove existing developer and operator recovery access.
- [x] 8.3 Qualify the configured Resend authenticated SMTP path on the exact deployed release by delivering verification, recovery, approval, and rejection messages and proving retry, duplicate suppression, token expiry, TLS verification, plaintext-fallback denial, and redacted evidence; separately configure authenticated Alertmanager delivery and prove a certificate-expiry alert reaches the operator destination.
- [x] 8.4 With registration already enabled and mail readiness true, use ordinary self-service test identities with no email-specific bypass to exercise hosted registration, verification, pending sign-in and product denial, operator queue, approval, rejection, fresh active sign-in, onboarding without automatic membership, abuse limits, restart, backup, restore, and rollback.
- [x] 8.5 Verify externally that only documented HTTPS routes are reachable, the browser renders the registration and wait-list flows, internal ports remain isolated, mail and credentials are not disclosed, and emergency admission stop still works.
- [x] 8.6 Update exact-release deployment and release-gate evidence without treating wait-list deployment as qualified-beta approval, obtain a fresh digest-bound public-preview risk acceptance, and reactivate guarded public preview only after every non-waivable safeguard passes.
- [x] 8.7 Replace calendar-expiring preview approval with an exact-release-bound persistent acceptance, add an explicit manual pause, retain fail-closed safeguard drift handling, deploy it, and verify restart-safe public admission plus sanitized evidence.
