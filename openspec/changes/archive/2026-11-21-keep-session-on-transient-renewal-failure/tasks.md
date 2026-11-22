## 1. Specification

- [x] 1.1 Rewrite the requirement "Session renewal and revocation" in `sync/rxdb-replication` so that only a definitive refusal ends a session, reword the scenario "Refresh session is revoked", and add the scenario "Renewal cannot reach the service".

## 2. Evidence that the client already meets it

- [x] 2.1 Confirm in `packages/rxdb-client/test/refresh.test.mjs`, `pull.test.mjs` and `live.test.mjs` that a refused credential clears the session and ends the live stream, and that a renewal without a verdict keeps the session and yields a retryable error.

## 3. Traceability and validation

- [x] 3.1 In `docs/requirements-traceability.md`, cite the live-stream test beside RR-19, add row RR-20 for the new scenario, and renumber the following row to RR-21.
- [x] 3.2 Run `openspec validate keep-session-on-transient-renewal-failure --strict --no-interactive`, `npm run validate:traceability`, `npm run validate:docs`, and the `@mako-cloud/rxdb` unit tests.
