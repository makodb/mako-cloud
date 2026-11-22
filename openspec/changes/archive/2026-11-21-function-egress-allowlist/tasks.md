# Tasks: Function Egress Allowlist and Plaid-Connected Rational

## 1. Protocol and runtime enforcement

- [x] 1.1 `crates/mako-edge-runtime-protocol`: drop `max_requests_per_invocation` from `OutboundNetworkPolicy::AllowList` and its `is_valid`, keep `valid_outbound_host` and the 64-host inner bound, update the adversarial egress test; update `docs/edge-runtime-protocol.md` — the refusal rationale becomes the acceptance contract
- [x] 1.2 `packages/cli/runtime/main/supervisor.ts`: widen the `outboundNetwork` type and `isLimits` to accept the hosts-only variant; `networkGrants` unions the `MAKO_API_URL` origin grant with each declared `host:443`; `deny_all` and undeclared deployments byte-for-byte unchanged; regenerate the runtime module
- [x] 1.3 Local parity: `index.ts` calls the shared grant logic instead of its hardcoded literal; `packages/cli/src/serve.ts` gains a repeatable `--allow-host` (flag parse, `rejectUnknownOptions`, env plumbing into the container); update the guard assertions in `packages/cli/test/serve.test.mjs`
- [x] 1.4 Smoke coverage (`crates/mako-smoke/tests/edge_function.rs`): beside the existing denial cases, a deployment declaring one host proves the grant exists at the permission layer (a non-permission failure from the unreachable host is the proof) while undeclared destinations — other host, other port, WebSocket, DNS — still refuse

## 2. Control plane and deployment surface

- [x] 2.1 `FunctionConfiguration` gains `allowedHosts` with `#[serde(default)]` (stored records are `deny_unknown_fields` blobs — old rows must keep deserializing, with a test loading a pre-change record); `validate` refuses IP literals, non-443 ports, wildcards, platform origins and internal names, duplicates, >8 hosts, and malformed names with addressed configuration errors — unit tests for every refusal
- [x] 2.2 Add optional `allowedHosts` to `FunctionConfiguration` in `api/openapi/mako-cloud-v1.yaml`; `npm run generate:api` and check
- [x] 2.3 `runtime_backend.rs` manifest builder maps a non-empty stored list onto the protocol variant (today's single `DenyAll` hardcode), with a test that a deployed declaration reaches the worker manifest verbatim and an empty one stays `DenyAll`
- [x] 2.4 CLI: repeatable `--allow-host <name>` on `mako-cloud functions deploy` via `configurationFromArgs`, declaration shown by `functions list`/`show` — unit tests for parse and display

## 3. Platform docs and qualification

- [x] 3.1 Update `docs/edge-functions.md`: the declared-egress capability, the unchanged deny-all default, the 8-host/HTTPS-only bounds, and the documented DNS-rebinding bound with its namespace/loopback mitigations
- [x] 3.2 Extend the edge-security qualification with the grant and refusal probes; add traceability rows for the five new `functions/edge-runtime` scenarios and run `npm run validate:traceability`
- [x] 3.3 Build, deploy, and requalify the platform release on the beta through the existing cycle before any Rational work depends on it (five cycles in the end — findings #38–#41 each forced another; release 9680415d deployed, requalification recording)

## 4. Rational server side

- [x] 4.1 Model: `plaid_items` collection with an empty policy rule set (default-deny, service-credential only), `connections` gains kind `"plaid"`; bootstrap scopes the institution-sync credential to `plaid_items`, stores `PLAID_CLIENT_ID`/`PLAID_SECRET` as function secrets when provided, and deploys `institution-sync` with `--allow-host sandbox.plaid.com`
- [x] 4.2 `functions/shared/plaid.ts`: pure cursor-protocol engine — added→create, modified→update, removed→delete, cursor advancement, and the non-secret connection projection — unit-tested under `node --test` against sandbox-shaped fixtures
- [x] 4.3 `institution-sync` routes `POST /plaid/link-token` and `POST /plaid/exchange` (authenticated member; access token written only to `plaid_items`; response carries only the connection id), and the sync pass branches on connection kind, reusing `transactionId` idempotence and the alert seam
- [x] 4.4 Unconfigured degradation: without Plaid secrets the routes answer an addressed "not configured" error and the sync pass skips plaid connections cleanly

## 5. Rational app and tests

- [x] 5.1 Connections screen offers "Connect through Plaid" only when the function reports itself configured; launches Plaid Link (sandbox) with the issued token and hands the public token to the exchange route
- [x] 5.2 Wire-mocked Playwright suite covers the link → exchange → first-sync story and asserts no token-shaped value ever reaches the browser
- [x] 5.3 Opt-in live suite (runs only with `PLAID_CLIENT_ID`/`PLAID_SECRET` set): `/sandbox/public_token/create` skips the Link widget, the real sync runs through the schedule, transactions land once, and a second sync adds nothing — never part of CI-gating suites
- [x] 5.4 Traceability rows for the four new `samples/rational-money-app` scenarios; `npm run validate:traceability`

## 6. Ship and verify

- [x] 6.1 Export and publish the standalone `shuaimu/rational` repo with the new function code and UI; its own suites pass without Plaid credentials
- [x] 6.2 Deploy the updated Rational functions to the beta project with the declared egress; verify outbound 443 from the edge-runtime container works on the host (open it deliberately if not — never silently). Functions deployed and green (sync 462ms, nightly 7181ms through the schedules); NOTE for 6.3: the runtime container binds outbound to loopback by design (finding #28), so declaring `sandbox.plaid.com` also requires deliberately widening the container network when the keys arrive — never silently
- [x] 6.3 Live verification on the beta with the user's Plaid Sandbox keys: the runtime container's loopback-bound outbound policy was deliberately widened (the tenant boundary now lives in the per-worker allowlist), the bootstrap installed the keys and declared `sandbox.plaid.com`, and First Platypus Bank linked end to end — run 1 imported 48 unique transactions in 14.45s through the real scheduler, run 2 held the cursor at 1.29s with nothing doubled. The opt-in local spec also passed against real Sandbox, after correcting its steady-state assumption (Plaid backfills history asynchronously; the invariant is uniqueness, not stasis)
- [x] 6.4 Update `docs/rational.md` and the findings log (#36 closes with this change); `npm run validate:docs`; full local gates green (findings #36–#43 all recorded; requalification evidence committed as 4c184c4)
