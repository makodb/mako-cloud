# Document policies

Every collection is default deny. Create, read, update, and delete require a
matching active allow rule, and any matching deny wins. Policies are compiled
against the collection schema into a deterministic, bounded evaluator with no
network, wall-clock, or arbitrary-code access.

## Evaluation context

Rules may use the verified user ID, role, trusted claims, project/environment,
operation, safe request metadata, prior document, and proposed document. They
must not treat user-editable profile metadata as trusted authorization input.

- Create evaluates the proposed state.
- Delete evaluates the prior state.
- Update evaluates both states and rechecks the current revision in the same
  conditional transaction that commits the write.
- Read policy applies consistently to point reads, indexed queries, pull, live
  delivery, readable conflicts, caller-aware edge SDK calls, and authorized
  support access.

Protected document bodies must not appear in denial details, unreadable
conflicts, logs, counts, or index diagnostics.

## Visibility and local data

A visible-to-hidden document change sends a synthetic tombstone containing only
the safe replication identity; hidden-to-visible sends the new state. Policy or
trusted-claim changes increment authorization epochs. The RxDB client then
pauses, securely clears affected replicated state, notifies the application,
and starts a new replication generation before rendering data again.

## Policy lifecycle

Create an immutable draft, validate syntax/types/cost against the active schema,
run representative examples, and atomically activate the complete version.
Activation advances the environment authorization epoch exactly once. Failed
validation or activation leaves the current policy unchanged.

Rollback selects a previously validated immutable version through the normal
audited action. It is an activation, so it also advances the authorization
epoch and requires client resets where visibility may have changed. Never edit
an active policy version in place.

## Privileged access

The default edge document client uses the caller's policies. Bypass requires an
explicit scoped service credential or time-bounded operator grant, the exact
tenant/collection/operation, a reason, and a successful durable audit append.
Audit failure prevents the bypass.

## Tested evidence

Run `npm run test:policy-security`. The suite covers differential decisions,
old/new visibility, epoch invalidation, conflict non-disclosure, privileged
bypass, and caller-aware edge access. See
[policy qualification](policy-security-qualification.md) and the
[policy failure runbook](runbooks/policy-evaluation-failures.md).
