# Developer registration and wait-list access

Mako Cloud authentication identities own email verification and credentials. Developer admission
is an optional role on that identity, independent of operator entitlement. Both remain independent
of the application users that a Mako project authenticates; a matching email address does not link
those identity classes.

## Lifecycle and authorization

Self-registration creates an `unverified` identity and queues one verification message. A valid,
single-use verification token moves that identity only to `waitlisted`. A wait-listed sign-in gets
the `mako-developer-waitlist` audience, which can call only the coarse self-status endpoint,
refresh, recovery, and sign-out. Management routes accept only `mako-management`.

An operator with the separate `waitlist_review` permission can list applicants—including their own
developer application—and atomically
approve or reject one with a private reason and idempotency key. Approval advances the durable
developer authorization epoch, revokes pending developer sessions, and requires a fresh developer
sign-in. It leaves operator entitlement and sessions unchanged and does not create a team,
project, membership, or quota grant. Every protected management request loads the current developer
role and epoch, so an old or manually signed claim cannot bypass review.

Password change or recovery advances the shared credential epoch and invalidates both developer and
operator sessions without changing either role. Account-wide suspension/deletion also denies both;
developer rejection or developer-only disablement does not remove operator access. Wait-list review
responses expose bounded `Developer status` and `Operator access` values separately.

The hosted routes are:

- `POST /v1/developer-auth/registrations`, verification/resend, sign-in/refresh/sign-out, and
  password-recovery operations;
- `GET /v1/developer-auth/wait-list-status` for a wait-list audience only;
- `GET/POST /v1/operator/developer-waitlist...` for separately authenticated review.

The exact request models and status/error responses are in [the OpenAPI contract](api.md). Browser
refresh credentials are rotating, digest-stored, `Secure`, `HttpOnly`, `SameSite=Strict` cookies.
Access tokens stay in memory or tab-scoped session storage. Email verification and recovery tokens
arrive in URL fragments and the console removes the fragment before rendering a result; no token is
placed in a query string or local storage.

## Privacy and abuse controls

Registration, resend, and recovery-request responses are generic and do not disclose whether an
email exists or its lifecycle. Password work has bounded concurrency. Global, source-digest,
normalized-email-digest, and token-attempt limits have finite retention. Applicant values do not
become metric labels or ordinary log fields. The public status response contains only the caller's
developer ID and `waitlisted`; it never exposes queue position, reviewer notes, capacity plans, or
an approval estimate.

Verification, recovery, approval, and rejection mail is encrypted at rest in the control-plane
RocksDB outbox. The worker uses authenticated SMTP with verified TLS, deterministic delivery IDs,
leases, bounded retries, dead letters, and terminal-record retention. Registration fails closed if
mail is missing, unready, or the pending outbox reaches its configured bound. A notification failure
after an operator decision does not roll the committed lifecycle back.

## Production configuration and rollout

`developer_registration.enabled` defaults to `false`. When enabled, production configuration must
also provide bounded lifetime/rate/outbox settings, a dedicated mail-encryption secret reference,
and a complete authenticated SMTP configuration with password secret reference. The public origin is
the validated HTTPS `server.public_url`. See [service configuration](configuration.md) and the
[operator runbook](runbooks/developer-registration-and-mail.md).

Deploy a new identity schema with registration disabled, verify migration/index parity and existing
active access, then configure and test mail before enabling registration. Caddy exposes only the
documented developer-auth and operator-review paths. A release change requires a fresh digest-bound
public-preview approval.

Rollback first disables registration. Preserve the additive authentication-identity, developer-role,
operator-entitlement, token, session, decision, and outbox keyspaces; do not delete applicants or
queued mail. Verify a checkpoint before binary rollback, and keep public admission restricted until
current role access, lifecycle/index parity, outbox bounds, and mail readiness are re-established.
