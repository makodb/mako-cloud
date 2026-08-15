## Context

See `proposal.md` for motivation and
`specs/identity/developer-registration/spec.md` for the behavior contract. The
hosted console currently accepts a short-lived developer JWT generated from a
protected workspace utility. The control plane verifies its issuer, audience,
expiry, verified-email flag, and active status but does not yet own a hosted
password, verification, recovery, refresh-session, or wait-list lifecycle.

Developer identities are a control-plane concern and are distinct from project
application users, whose credentials and sessions remain data-plane owned. The
control-plane RocksDB path is already the sole production state for management
resources. The public beta has trusted same-origin HTTPS and exact Caddy route
allowlists. Resend is configured for transactional developer mail through the
authenticated SMTP adapter and registration is enabled, but the complete
transactional-mail and developer-lifecycle qualification is still outstanding.
Certificate-expiry notifications use a separate Alertmanager delivery path and
remain unqualified. A changed immutable release also invalidates the current
public-preview approval.

## Goals / Non-Goals

**Goals:**

- Make self-registration safe to expose before automatic tenant admission is
  appropriate.
- Make `waitlisted` a real authorization boundary enforced from current durable
  state at every protected management request.
- Give platform operators an efficient, audited, race-safe review workflow.
- Keep an explicitly accepted public preview available until an operator pauses
  it, without weakening exact-release or non-waivable safety bindings.
- Preserve restart, backup, restore, redaction, route, and immutable-release
  contracts already used by the public beta.

**Non-Goals:**

- Automatically ranking applicants, assigning queue positions, estimating an
  approval date, or making approval decisions from profile data.
- Creating an organization, project, quota grant, billing account, or tenant
  membership during approval.
- Reusing project application-user identity, passwords, sessions, or signing
  keys for developer access.
- Adding social login, enterprise SSO, MFA enrollment, CAPTCHA, billing, or a
  general campaign-email system in this change.
- Replacing the separately authenticated platform-operator boundary.

## Decisions

### 1. Keep hosted developer identity in the control-plane authority

Add a production developer identity service over the control-plane-owned
RocksDB adapter. Its records include a stable developer id, normalized-email
unique index, display name, approved password hash, lifecycle status,
email-verification time, authorization epoch, timestamps, and decision
metadata. Separate keyspaces hold digested verification/recovery tokens,
digested refresh sessions, bounded idempotency results, rate-limit state, and a
mail outbox.

This preserves the existing trust split: application users remain in the data
plane, while developer accounts and operator actions stay in the control plane.
Putting hosted developer credentials in the data plane would couple management
availability to project auth and confuse the two user populations. An external
identity-as-a-service provider was also considered, but it would introduce a
new authority, webhook, account-linking, and deployment dependency while still
requiring Mako-owned wait-list state and current-status checks.

### 2. Use an explicit lifecycle state machine and compare-and-set decisions

The initial state machine is:

`unverified -> waitlisted -> active`

`unverified` and `waitlisted` may move to `rejected`; `active` may move to
`disabled` through existing operator lifecycle controls. Approval and rejection
are transactional compare-and-set operations from `waitlisted`. Each decision
stores the operator id, bounded private reason, request id, idempotency key,
timestamp, prior state, new state, and resulting authorization epoch in the
same commit as the lifecycle mutation, audit event, and notification outbox
record. Exact idempotency retries return the original safe result; changed input
conflicts. Concurrent decisions therefore have one winner.

Approval deliberately does not create tenant membership. On the next fresh
active sign-in, a developer with no memberships enters the normal organization
creation/onboarding flow. Automatically creating a tenant at approval would
mix capacity allocation with identity review and make accidental approvals
more expensive to undo.

### 3. Separate wait-list and active session audiences

Hosted authentication issues short-lived access JWTs plus rotating opaque
refresh credentials. Refresh credentials are stored only as digests and sent
to the browser in `Secure`, `HttpOnly`, same-origin cookies with a narrow path
and an appropriate `SameSite` policy. The console holds access tokens only in
memory or tab-scoped session storage and removes them on sign-out; sensitive
tokens never enter query strings or local storage.

A wait-listed identity receives an access token with a dedicated wait-list
audience and only self-status, refresh, and sign-out scope. Account recovery
remains available from the logged-out sign-in flow and is not linked from the
authenticated pending-review screen. The normal
management authenticator accepts only the active developer audience, then
loads the current persistent identity state and compares the authorization
epoch before building a developer principal. Approval, rejection, disablement,
password recovery, and other authority-changing transitions advance the epoch
and revoke incompatible refresh sessions. Approval requires a fresh sign-in;
an old pending token never upgrades in place.

The protected `mako-control-session` utility remains useful for bootstrap and
recovery, but its tokens must identify a persisted, currently active developer
and current epoch. Signature possession alone cannot manufacture an active
identity or bypass wait-list review. Continuing to trust only the JWT's
embedded `status=active` claim was rejected because it would make revocation
and wait-list enforcement dependent on token expiry.

### 4. Define one bounded same-origin public auth family and one operator family

The public control-plane routes are versioned under
`/v1/developer-auth/` for registration, verification and resend, session
creation/refresh/deletion, recovery request/completion, and self wait-list
status. The operator family is under `/v1/operator/developer-waitlist` with
bounded list, detail, approve, and reject operations. Exact paths and wire
models are generated through the repository OpenAPI workflow, and Caddy admits
only those published routes. Private lookup, token, mail, and session-issuance
operations remain unproxied.

The console uses the same-origin routes. Its logged-out router adds create
account, verify email, recover account, and sign-in views; a wait-list session
can render only the pending page with status and sign-out controls. The existing `/operator` area adds queue,
detail, approve, and reject screens behind the separate operator auth provider.
This retains console/API parity and avoids a browser-only privileged backend.

For the beta deployment, a hosted operator adapter accepts only a separately
issued, short-lived operator-audience token carrying the explicit wait-list
review permission. The credential remains tab-scoped and is verified by the
operator backend on every request; developer and wait-list tokens are rejected.
Issuance remains a protected operator procedure until a phishing-resistant
operator identity provider is deployed. This makes the review interface usable
without weakening the existing separation or creating public operator signup.

### 5. Commit identity transitions and a durable mail outbox together

Registration, verification, recovery, approval, and rejection write bounded
outbox entries in the same RocksDB transaction as identity state. A supervised
worker claims entries with a lease, calls a production authenticated SMTP
adapter, and records a redacted delivery outcome with bounded retry and dead
letter state. Provider credentials are protected deployment files; message
templates receive single-purpose tokens only at delivery construction and no
message body is logged.

The initial public-beta provider is Resend through its authenticated SMTP
endpoint. This does not add a provider-specific HTTP API to the control plane;
provider behavior remains behind the existing mail transport boundary.
Alertmanager certificate-expiry notifications are a separate delivery path and
must be configured and qualified independently. Transactional-mail readiness
does not prove that operator-alert delivery works.

Registration readiness is false when a usable mail adapter is not configured,
so the service does not accumulate accounts that cannot verify. Once a state
transition and outbox item commit, a transient provider failure does not roll
the transition back; the worker retries idempotently and alerts operators. A
direct synchronous SMTP call inside the identity transaction was rejected
because ambiguous network failures would either hold storage locks or duplicate
state changes.

### 6. Make enumeration and abuse resistance part of the protocol

Registration, verification resend, sign-in failure, and recovery request use
stable generic public responses. Input sizes, token attempts, password work,
mail production, and outbox growth are bounded. Rate limits use privacy-safe
keyed digests of normalized email and source information with configured
retention rather than raw values in metrics. Global shedding prevents a large
number of unique inputs from exhausting RocksDB or password-hashing resources.

The initial version does not require a third-party CAPTCHA. CAPTCHA can be
added later behind an abuse-verification boundary if measured traffic shows it
is necessary; making it mandatory now would add another availability and
privacy dependency without evidence.

### 7. Preserve existing identities through a forward-compatible migration

Introduce a versioned, resumable control-plane migration that creates the new
indexes and records before registration is enabled. Existing developer
identities are backfilled as `active`, email-verified, and assigned an initial
authorization epoch; no existing account is silently wait-listed. The migration
validates normalized-email uniqueness and stops on ambiguous collisions rather
than selecting a winner. New self-registrations use `unverified` only after the
migration marker is complete.

Backups, restore, release rollback, and semantic recovery gain checks for
identity/index parity, lifecycle counts, authorization epochs, token/outbox
bounds, and absence of an active identity created without verification or
operator approval. Older binaries must ignore the additive keyspaces safely;
rollback disables new registration first and preserves all identity/outbox
records rather than deleting applicants.

### 8. Make public-preview risk acceptance persistent but manually revocable

An operator may accept the documented residual blocker set for one exact
release and plan without a calendar expiry. The approval remains bound to the
operator identity, plan hash, release digest, blocker digest, and all eight
non-waivable safeguards. The admission guard continues evaluating those
bindings on its timer and fails closed to `pre_gate` when any binding or
safeguard drifts.

The operator can pause the preview explicitly at any time. Manual pause
atomically selects `pre_gate`, records sanitized evidence, and leaves Caddy,
private services, wait-list state, backups, and the emergency stop intact. A
new immutable release or changed blocker set still requires a new acceptance;
"persistent" removes periodic calendar renewal only and does not authorize an
unknown future release.

An unbounded approval that ignored release or health drift was rejected because
it would turn risk acceptance into a safety bypass. A periodically expiring
approval was also rejected for this preview because an unattended expiry looks
like an outage to users even when every measured safeguard remains healthy.

## Risks / Trade-offs

- [Public signup creates spam and resource pressure] → Use generic responses,
  bounded password work and payloads, layered keyed-digest rate limits, global
  shedding, outbox limits, and aggregate alerts; add CAPTCHA only from measured
  need.
- [A stale or manually signed token bypasses wait-list state] → Verify current
  lifecycle and epoch from persistent state on every protected request and bind
  the recovery utility to an existing active identity.
- [Mail failure strands applicants] → Gate initial registration readiness on
  configured mail, transactionally persist outbox work, retry with leases, and
  alert on age or dead-letter thresholds.
- [Operator approval is accidental or races another reviewer] → Require a
  specific permission, visible identity and reason, explicit confirmation,
  compare-and-set state, idempotency, and immutable audit.
- [Applicant data leaks through queue, logs, or metrics] → Restrict queue APIs to
  operator auth, bound every response, separate private review notes, redact
  ordinary logs, and use no applicant values as metric labels.
- [Schema migration locks out current preview access] → Backfill existing
  identities as active, validate uniqueness before switching readers, deploy
  registration disabled, and retain the protected console recovery path.
- [Approval notification fails after activation] → Keep the committed identity
  state, retry only the outbox delivery, and make the console status authoritative
  rather than treating email as the state transition.
- [A persistent public preview outlives operator attention] → Keep the
  independent emergency stop, exact-release and blocker bindings, recurring
  safeguard evaluation, visible active-mode evidence, and an explicit manual
  pause operation.

## Migration Plan

1. Implement the schema, identity service, mail outbox, APIs, SDK types, and
   console surfaces behind a disabled hosted-registration flag.
2. Build and deploy the immutable candidate with admission in `pre_gate`, run
   migration fixtures and the migration, backfill existing developer identities
   as active and verified, and verify identity/index parity plus current session
   behavior before enabling registration. The release digest change invalidates
   the prior public-preview approval by design.
3. Configure protected Resend authenticated SMTP credentials and sender
   identity, enable hosted registration only after mail readiness and security
   suites pass, verify Caddy exposes only the documented route families, and
   retain sanitized hosted evidence.
4. Qualify the configured Resend authenticated SMTP path for verification,
   recovery, approval, rejection, retry, token expiry, duplicate suppression,
   redaction, and plaintext-fallback denial. Separately configure an
   authenticated Alertmanager delivery path and prove certificate-expiry alert
   delivery, retaining only sanitized evidence.
5. With registration enabled, exercise end-to-end registration through pending
   product denial, approval, rejection, concurrent review, stale-session
   denial, fresh active sign-in, onboarding without automatic membership,
   abuse limits, restart, backup, empty-target restore, and rollback against the
   exact immutable candidate.
6. Verify external route isolation, browser behavior, restart-safe admission,
   backups, recovery, and rollback evidence without treating the risk-accepted
   preview as a qualified-beta approval.
7. Obtain a fresh exact digest-bound public-preview acceptance before restoring
   unrestricted preview access. Keep it active without calendar expiry until an
   operator pauses it or the guard detects release, approval, or safeguard
   drift.

Rollback first disables registration and leaves existing active sign-in
available. If the candidate must be rolled back, preserve the additive identity
and outbox keyspaces, select the compatible prior release through the existing
release operation, and keep public admission restricted until active-session
and migration invariants are reverified.
