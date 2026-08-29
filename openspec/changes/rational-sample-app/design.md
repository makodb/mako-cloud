## Context

See proposal.md — Why. What an application can use today, as surveyed before this plan: `@mako-cloud/rxdb` (password auth, RxDB pull/push/live, recovery and security-reset coordinators), `@mako-cloud/edge-sdk` inside functions (caller-forwarded and service-credential document clients), raw HTTP for storage objects and for provider/magic-link sign-in, indexed predicate queries only (no aggregation, count, or search), document policies over `identity.user_id`, `claims.*`, `request.*`, `old`/`new` with default deny, no CORS from the data plane, application mail limited to the four auth templates, and management-only configuration of collections, indexes, policies, buckets, functions, schedules, webhooks, and domains. `examples/local-first` is the only application sample and runs RxDB on in-memory storage. Rational must be a realistic product on exactly that surface, and where the surface is insufficient the fix goes into the platform, not into a workaround.

## Goals / Non-Goals

**Goals:**
- Every Rational feature maps to a named platform capability and exercises it through the public client packages, never through management APIs at runtime.
- Sharing, offline durability, provider sign-in, file access, and same-domain API access from a browser are proven by tests that run against the real stack, locally and on the beta.
- The findings loop is a first-class artifact: a defect found by Rational is not worked around in Rational.

**Non-Goals:**
- Real bank aggregation (Plaid-style); the simulated institution is deterministic test data served by an edge function.
- Server-side aggregation, reporting APIs, or full-text search in the platform; Rational computes reports client-side, and a finding that this is insufficient is recorded, not solved here.
- Application-initiated email; alerts go to webhooks, and in-app notifications are documents.
- Multi-currency conversion; each household has a base currency and accounts carry their own currency without conversion.
- A native mobile client.

## Decisions

**1. Rational is a React + Vite single-page app on RxDB with Dexie storage, in `examples/rational`.**
React because the console already uses it (toolchain, lint, and Playwright setup exist); RxDB with the Dexie/IndexedDB storage because the point is durable offline behavior, which the memory storage of `examples/local-first` cannot prove. One RxDB database per household, one collection per document type, replication per collection. *Alternative rejected:* extending `examples/local-first` — its single collection and vanilla rendering would not carry a product-sized app.

**2. Households are the tenancy unit inside the environment, enforced by policies over trusted claims.**
Every document carries `household_id`. Each user's administrator-controlled app metadata carries `households: { "<id>": "owner"|"editor"|"viewer" }`, which the platform surfaces as verified token claims; policies read `claims.households[new.household_id]` (or `old.…` on read/delete) and the role decides the operation. Membership changes are written by Rational's own `households` edge function under a service credential through the new app-metadata route (decision 4) and take effect on the next token; the app refreshes its session after a membership change, and the platform's authorization-epoch reset clears a removed member's local data. *Alternative rejected:* a membership document consulted by policy — policies cannot join, so membership must live in the claims the token carries.

**3. Document model (collections, one schema version each, primary keys as fields).**
`households`, `memberships` (a readable projection of the claims for UI, written by the function), `accounts`, `transactions`, `splits` embedded in the transaction, `categories`, `tags`, `rules`, `budgets` (one per category per month, `id = <household>:<category>:<yyyy-mm>`), `recurrences`, `goals` with embedded contributions, `net_worth_snapshots` (one per day), `alert_settings`, `alerts`, `institution_connections`, `import_batches`. Amounts are integer minor units with a currency code. Every collection has an index on `(household_id, updated_at)` for the function-side queries, and `transactions` additionally on `(household_id, account_id, date)` and `(household_id, date)`. No collection exceeds the 1 MiB request bound per document; receipts are objects in a `receipts` bucket at `households/<id>/transactions/<txn>/<file>` under a bucket rule that reads `identity`/`claims` the same way. *Alternative rejected:* one collection per household — collections are management-created and policies are per collection; households are data.

**4. Platform: a service-credential route to set app metadata, exposed by the edge SDK.**
`POST …/service/users/{userId}/app-metadata` with `X-Mako-Service-Key`, a bypass reason, and a JSON merge body; audited as a privileged bypass like service document access; refused without the service credential or over a public route. The edge SDK gains `createServiceClient(...).users.setAppMetadata(userId, patch, reason)`. The identity store already separates trusted app metadata from profile metadata, so the change is a route and an audit record, not a model change. *Alternative rejected:* a management automation token inside the function — it would grant the function the whole management surface for one write.

**5. Platform: per-domain CORS allowlist.**
`CustomDomain` gains `allowedOrigins` (exact origins, ≤ 16); the control plane installs them with the hostnames into the data plane, and the data plane and gateway, on a request carrying `X-Mako-Custom-Domain`, answer `OPTIONS` preflights and add `Access-Control-Allow-Origin` (echoing the matched origin), `-Allow-Methods`, `-Allow-Headers` (`authorization, content-type, x-mako-key, idempotency-key`), `-Expose-Headers` (`etag, x-mako-request-id`), `-Max-Age`, and `Vary: Origin` only when the origin is listed. Caddy's custom-domain site forwards `OPTIONS`. The platform hostname never emits CORS. Rational's SPA is served from its own hostname and calls the API on the custom domain, which is the topology a real product has. *Alternative rejected:* serving application static files from the custom domain — a hosting feature the platform does not have and that this change should not smuggle in.

**6. Platform: `@mako-cloud/rxdb` grows to cover the whole application surface.**
`MakoAuthClient` gains `startProviderSignIn(provider, redirectUrl)`, `completeProviderSignIn(locationFragment)`, `requestMagicLink(email, redirectUrl)`, `redeemMagicLink(token)`; a `BrowserAuthSessionPersistence` over `localStorage`; and `MakoStorageClient` with `put/get/list/delete` under the same session and public key. Replication persistence: `DexieReplicationStatePersistence` (checkpoint, security epochs, recovery state) so the coordinators survive restarts. *Alternative rejected:* a separate `@mako-cloud/app` package — the client is already the published application package, and one dependency is what a developer expects.

**7. Automation is three edge functions and two schedules, invoked as the platform invokes them.**
`institution-sync` (schedule every 15 minutes; reads connections with a service client, pulls deterministic statements from the simulated institution — itself a route on the same function — and writes transactions idempotently by `(account, external_id)`), `nightly` (schedule at 02:00 UTC; applies rules to uncategorized transactions, writes net-worth snapshots, detects recurrences), and `households` (HTTP; invite, accept, change role, remove — the only writer of memberships, using the app-metadata route). Alerts are evaluated in `nightly` and on each `institution-sync` pass, written as `alerts` documents, and the household's webhook endpoint (registered by the developer for the `alerts` collection) delivers them signed; the app shows the alert history from the collection. *Alternative rejected:* evaluating alerts in the browser — a device that is closed would never fire them.

**8. Duplicate detection and reports are client-side.**
Import duplicates are found in the app by `(account, date ± 0, amount, normalized description)` against local data before push; the nightly job repeats the check server-side for synced transactions with an indexed query per account and day. Reports (cash flow, spending by category, budgets, net worth) are computed over local RxDB data with memoized selectors and stamped "as of last sync". The absence of server aggregation is a recorded finding with a measured cost (report time at 50 000 transactions), not a platform change in this plan.

**9. Testing is four layers, and findings are logged where they are found.**
Unit (rules engine, dedupe, recurrence detection, budgets, reports — pure functions), Playwright against a wire-mocked backend (fast, every screen), Playwright against the real local stack (`test-live`, like the existing sample: bootstrap, data plane, control plane, edge gateway with the runtime container when available, Vite proxy for same-origin, and a second origin to prove CORS on a loopback custom domain with the DNS stub), and a smoke suite in `crates/mako-smoke` that boots the stack and drives Rational's flows over HTTP (sharing, import, rules, receipts, alerts) so the hosted qualification can include it. `examples/rational/PLATFORM-FINDINGS.md` records each finding as symptom → platform change (commit) → regression test.

**10. Beta deployment.**
Rational's SPA is served by a Caddy site block on the beta host for the app hostname (static files from the release, same converge as the console), and its API is the project's custom domain with the app origin allowlisted. Both DNS names are an operator prerequisite (recorded in the runbook); until they exist, the same topology is qualified locally with the DNS stub and `--resolve`, and the beta's Rational smoke runs on the platform hostname.

## Risks / Trade-offs

- [Claims-based sharing means a role change is visible only after a token refresh] → the app refreshes the session after every membership write and the function returns the new expiry; removal is enforced immediately server-side by the authorization epoch.
- [Policies over `claims.households[...]` push the policy language] → the first task validates the expression form against the compiler; if map lookup by field value is unsupported, that is finding #1 and a platform change (indexing a claim by a document field).
- [Dexie storage in Playwright is per browser context] → the live suite uses persistent contexts and asserts IndexedDB survives a reload.
- [Client-side reports at scale] → measured at 50 000 transactions with a seeded household; recorded as a finding with numbers.
- [Simulated institution is deterministic] → it can never surface real-world statement formats; the CSV import path with real-shaped fixtures covers that.
- [CORS widens the attack surface of custom domains] → exact-origin matching only, no wildcard, credentials mode limited to the headers the client uses, never on the platform hostname; covered by the smoke and by the Caddy validator.
- [Beta DNS is outside the repository] → the plan degrades to the platform hostname for the beta smoke and proves the custom-domain path locally.

## Migration Plan

Additive throughout: new sample, new client APIs, a new service route, a new optional field on custom domains (default empty, meaning no CORS — today's behavior). Deploy as one release after the local suites pass; requalify with Rational's smoke added to the hosted suite; rollback is the platform's existing release rollback, and Rational has no state to migrate.

## Open Questions

- The beta hostnames (proposed `rational.makodb.com` for the app and `api.rational.makodb.com` for the API) need DNS records the repository cannot create; the plan proceeds locally and on the platform hostname until they exist.
