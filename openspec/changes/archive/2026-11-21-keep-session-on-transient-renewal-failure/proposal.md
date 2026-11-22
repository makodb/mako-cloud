## Why

`sync/rxdb-replication` says the client integration "SHALL stop data delivery
promptly when a session is revoked or cannot be refreshed", and its scenario
says that on a revoked session "replication stops, closes the live stream".

The User Book draws a line the spec does not. A renewal either ends the
session or does not touch it, and the client tells the two apart by what the
service answered. Only a definitive refusal signs a user out. A renewal that
reached no verdict (a network fault, a timeout, a `408`, a `429`, any `5xx`)
keeps the stored session: pulls, pushes and object requests report a retryable
`unavailable`, and the live stream keeps reconnecting. That is what makes an
offline restart work. The book also says RxDB's replication has no notion of a
terminal failure, so the application stops pull and push replication itself
when the integration reports `authentication_required`; only the live stream,
which the package owns, stops on its own.

As written, the spec requires the opposite of the offline behaviour the book
describes and the client implements.

## What Changes

- The requirement "Session renewal and revocation" separates a definitive
  refusal from a renewal that reached no verdict, says who stops what, and
  names the coalesced renewal.
- The scenario "Refresh session is revoked" says the session is cleared, the
  live stream closes and the application is told to stop replication.
- A scenario "Renewal cannot reach the service" is added, with its
  traceability row (RR-20; the row after it moves to RR-21). The row for
  "Refresh session is revoked" also cites the live-stream test.

No code changes: `@mako-cloud/rxdb` already behaves this way and existing
tests cover both scenarios.

## Capabilities

### New Capabilities

### Modified Capabilities

- `sync/rxdb-replication`: a session ends only on a definitive refusal; a
  renewal without a verdict keeps the session and is retried.

## Impact

- `openspec/specs/sync/rxdb-replication/spec.md`
- `docs/requirements-traceability.md` (RR-19 cites one more test, RR-20 is
  new, the former RR-20 becomes RR-21)
