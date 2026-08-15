# Operator password authentication

Use this runbook for routine operator sign-in, entitlement bootstrap and changes, password recovery,
revocation, and incident-only break-glass access. Never put an email address, password, raw cookie,
attempt key, private reason, internal signing secret, or bearer token in logs or retained evidence.

## Routine sign-in and revocation

1. Routine operators open `https://cloud-test.makodb.com/operator` and use the password for their
   verified, security-active authentication identity. The identity must also have an explicit
   operator entitlement; its developer application may be absent, wait-listed, active, rejected,
   or developer-disabled.
2. The browser receives only a `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/v1` cookie. A session
   expires within one hour; privileged mutations require a password verification from the preceding
   five minutes.
3. A generic rejection does not identify whether the email, password, lifecycle, entitlement, or
   service state caused the failure. Check only aggregate metrics and sanitized audit classes.
4. Password recovery/change, account-wide suspension/deletion, a credential-epoch advance, or an
   entitlement replacement/revocation invalidates existing operator sessions. Developer approval,
   rejection, or developer-only disablement does not. Require a fresh password sign-in after a
   shared credential event; never restore an old cookie or session record.

## Protected entitlement plan and apply

Run the release-owned client as root on the guest. The control-plane endpoint is loopback-only and
the input and internal-auth files must be regular mode-`0600` files. The protected input is JSON with
`operation` (`initial_bootstrap`, `grant`, `replace`, `revoke`, or
`repair_bootstrap_developer_admission`), `targetEmail`, exact
`permissions`, `privateReason`, `environmentBinding`, `idempotencyKey`, `activateWaitlisted`, and no
`typedConfirmation` during planning.

```sh
umask 077
/opt/mako/current/bin/mako-operator-admin \
  --mode plan \
  --endpoint 127.0.0.1:8081 \
  --secret-file /etc/mako/credentials/internal-auth \
  --input /run/mako-operator-admin/request.json \
  > /run/mako-operator-admin/plan.json
```

Review the sanitized environment, opaque identity, separate developer/operator before-and-after
states, and email/permission/request/operation digests. The client or deployment agent may carry the
machine-generated `typedConfirmation` from plan to apply after a human gives a concise explicit
approval; never ask the human to copy the opaque value. Keep the apply input unchanged and let the
client read the mode-`0600` plan file:

```sh
/opt/mako/current/bin/mako-operator-admin \
  --mode apply \
  --endpoint 127.0.0.1:8081 \
  --secret-file /etc/mako/credentials/internal-auth \
  --input /run/mako-operator-admin/request.json \
  --plan-file /run/mako-operator-admin/plan.json
```

An identical replay returns `replayed: true`. A missing/unverified/ambiguous identity, changed
environment or permission set, stale plan, or mismatched confirmation makes no change. Bootstrap
and entitlement administration change operator state only; `activateWaitlisted: true` is rejected.

The one-time `repair_bootstrap_developer_admission` operation additionally requires the protected
`priorBootstrapIdempotencyKey`, empty permissions, and the exact original combined-bootstrap
provenance. It accepts only an active developer record with no later review, plans
`active → waitlisted`, and preserves credential, entitlement, operator epoch, and session records.
Obtain concise human approval after plan and before apply. The target must then be approved or
rejected manually through `/operator` using the normal self-review safeguards.

## Incident-only break glass

Hosted `operator_break_glass_bearer_enabled` defaults false. Enable it only through protected
configuration in restricted `pre_gate`, with a recorded incident, and converge the service. The
issuer requires a mode-`0600` incident-reason file, a non-empty least-privilege permission set, an
expiry of no more than one hour, and a new mode-`0600` redacted evidence path:

```sh
/opt/mako/current/bin/mako-operator-session \
  --secret-file /etc/mako/credentials/internal-auth \
  --incident-reason-file /run/mako-operator-incident/reason \
  --issuer https://cloud-test.makodb.com/control-identity \
  --operator-id opr_OPERATOR_ID \
  --permissions tenant_read \
  --ttl-seconds 900 \
  --output /run/mako-operator-incident/session.jwt \
  --evidence-output /run/mako-operator-incident/issuance.json
```

Break-glass tokens can perform permitted reads only. Mutations fail with
`operator_step_up_required`; do not weaken that rule during an incident. Disable bearer acceptance
and converge immediately after recovery. Remove the token only with the bounded cleanup procedure
below or another explicit, reviewed deletion.

## Monitoring, recovery, and rollback

- Alert on sign-in failures/throttling, bootstrap failure, abnormal active-session growth, and any
  break-glass state that persists beyond the restricted operation.
- Keep public admission in `pre_gate` during bootstrap, password enablement, recovery tests, or
  rollback. Audit/storage/readiness failures are fail-closed.
- Checkpoints include entitlements, epochs, protected session digests, attempts, and idempotency.
  Restore only to an empty offline target; verify revoked/expired sessions do not regain authority.
- A format-compatible rollback leaves additive operator keyspaces untouched. If administration is
  required, use a bounded reason-bound break-glass token rather than changing RocksDB directly.
- Release operations snapshot the three service JSON documents under the outgoing immutable release
  digest. Rollback requires and restores the target release's protected snapshot before starting its
  binaries, so a newer additive configuration section cannot strand the prior release.
- Do not remove a prior local token until the intended operator has completed one successful browser
  password sign-in against the exact candidate.

## Expired local-token cleanup

Preview first, then use `--apply` only after reviewing every reported path:

```sh
node scripts/cleanup-expired-operator-sessions.js \
  --directory /home/OPERATOR/.local/operator-sessions
node scripts/cleanup-expired-operator-sessions.js \
  --directory /home/OPERATOR/.local/operator-sessions --apply
```

The command accepts one explicit non-symlink directory, considers only regular mode-`0600` `.jwt`
files whose bounded JWT claims have `mako-operator` audience and an elapsed `exp`, and never follows
links or traverses subdirectories. Deployment credentials, protected admin inputs/evidence, and
unrelated files are outside its eligible set.
