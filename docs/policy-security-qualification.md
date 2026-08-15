# Policy security qualification

Run `npm run test:policy-security` before a release or after changing a policy input, enforcement path, authorization epoch, conflict response, or privileged credential.

The suite exercises these penetration objectives:

- Differential decisions: point reads, trusted queries, replication pulls, live streams, conflict responses, the edge SDK path, and operator impersonation all pass through the same compiled policy decision and stable code.
- Visibility transitions: visible-to-hidden changes emit only a synthetic tombstone; hidden-to-hidden changes advance without exposing protected state.
- Authorization epochs: trusted-claim and policy activation changes advance scoped counters and publish ordered invalidations.
- Conflict non-disclosure: an unreadable stale master returns the same permission error as a denied write, with no master state, document ID, stale revision, or protected body on the wire.
- Privileged bypass: service credentials and operator grants require exact tenant, collection, operation, expiry, reason, and durable audit authorization. Audit failure prevents bypass.
- Edge callers: the default data client propagates only verified caller context; service access uses a separate explicit API and mandatory bypass audit headers.

## Latest qualification

The 2026-08-07 local run passed the Rust policy, sync, and gateway suites and
all edge SDK unit tests. The command is deterministic and exits on the first
failed objective.
