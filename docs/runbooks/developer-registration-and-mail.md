# Developer registration and mail operations

Use this runbook for registration readiness, a growing wait list, mail delivery failures, emergency
registration disablement, operator review, recovery, and rollback. Never copy email addresses,
tokens, password hashes, session values, SMTP credentials, or reviewer reasons into tickets, logs,
metrics, or qualification evidence.

## Enablement

1. Keep `mako_developer_registration_enabled: false` while installing the release and running the
   developer identity migration.
2. Confirm identity/email-index parity, current active developer access, the separate operator
   boundary, a current checkpoint, and an empty-target restore result.
3. Install mode-`0600` controller sources for `developer-mail-encryption` and the authenticated SMTP
   password. Configure a verified-TLS relay, port, username, and sender as one atomic change.
4. Prove SMTP readiness and deliver verification, recovery, approval, and rejection fixtures. Prove
   TLS verification failure, transient retry, lease recovery, dead letter alerting, token expiry, and
   absence of plaintext fallback. Retain only sanitized delivery IDs and aggregate outcomes.
   For the public beta, Resend is used through the authenticated SMTP adapter.
   Qualify the separate Alertmanager Resend path independently by sending a
   bounded certificate-expiry fixture and verifying the provider delivery event;
   transactional-mail readiness is not alert-delivery evidence.
5. Set registration enabled, converge in `pre_gate`, and run registration through verification,
   pending product denial, operator approval/rejection, fresh active sign-in, restart, backup,
   restore, and rollback. Obtain a new exact-release public-preview approval before admission.

The hosted beta operator uses password sign-in at `https://cloud-test.makodb.com/operator`; there is
no token-paste field. The developer identity must be active and verified and must have a separate
`waitlist_review` entitlement. Every decision also requires recent password verification, an
explicit private reason, idempotency, and confirmation. Treat the detail API's committed lifecycle
as authoritative even if notification mail is delayed. Follow the
[operator password-authentication runbook](operator-password-authentication.md) for protected
bootstrap, entitlement changes, revocation, and incident-only break glass.

## Mail failure or outbox pressure

1. Disable new registration immediately if mail readiness is false, pending depth/age is growing,
   dead letters appear, or credential/TLS validation fails. Existing active sign-in remains available.
2. Do not replay lifecycle transitions or manually promote an applicant because mail failed. The
   durable worker retries only delivery. Approval/rejection remains committed.
3. Check aggregate worker outcomes, oldest pending age, dependency readiness, service restarts, and
   sanitized stable error classes. Never inspect decrypted bodies in routine incident response.
4. Correct the relay, credentials, sender authorization, DNS/provider policy, or network path. Prove
   readiness and one controlled fixture before restoring registration.
5. If the outbox bound remains reached, keep registration disabled, preserve a checkpoint, and
   investigate terminal-record cleanup and provider throughput. Do not raise limits without abuse and
   storage evidence.

## Recovery and rollback

Password recovery is generic, single-use, expiry-bounded, advances the authorization epoch, and
revokes every older developer and operator session. A wait-listed account stays wait-listed after recovery. Never use the
developer bootstrap signer to manufacture an active account; management authorization checks the
persisted active lifecycle and current epoch.

For release rollback, disable registration, stop public admission, take and verify a checkpoint, and
use the normal release rollback procedure. Preserve additive keyspaces and pending mail. Afterward,
verify identity/index parity, lifecycle counts, positive epochs, bounded session/token/decision/outbox
records, existing active access, operator review denial without permission, and a mail readiness
fixture before reopening admission.
