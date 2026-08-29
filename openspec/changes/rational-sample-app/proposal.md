## Why

The platform now has the surface a real application needs — auth with providers and magic links, policy-governed documents with RxDB replication, file storage, edge functions with schedules, webhooks, custom domains — but nothing in the repository uses more than a slice of it at once, and the one sample (`examples/local-first`) is a todo list on in-memory storage. The gaps a real product hits (sharing data between users, durable offline storage, provider sign-in from a browser, reading files, calling the API from an app's own domain) are exactly the ones no test exercises today. Rational, a Monarch-style money-management app, is the standing test bed: every feature it needs maps onto a platform capability, every problem it exposes becomes a platform fix with a regression test, and it stays deployed on the beta as the proof that the product works end to end.

## What Changes

- A new sample application, **Rational**, under `examples/rational`: households shared by several users with roles; accounts (manual, CSV import, and a simulated bank connection synced by a scheduled edge function); transactions with categories, tags, notes, splits, receipts, and duplicate detection; categorization rules applied in the client and nightly by a function; monthly budgets with rollover; recurring detection and upcoming bills; goals; net-worth history from scheduled snapshots; cash-flow and spending reports computed client-side over RxDB; alerts delivered to a household's webhook endpoint; sign-in by password, Google/GitHub, and magic link; fully offline-first on durable IndexedDB storage with conflict resolution and security resets; a "how it's built" page generated from the environment's API docs.
- A test harness for Rational: Playwright suites against a wire-mocked backend and against the real local stack (like `examples/local-first/test-live`), a smoke suite that boots the platform and drives Rational's flows through it, and `examples/rational/PLATFORM-FINDINGS.md` — a running log in which every platform problem the app exposes is recorded with its fix and regression test.
- Platform work Rational forces, done as part of this change:
  - `@mako-cloud/rxdb` gains provider sign-in and magic-link helpers (start, callback fragment handling, exchange, request, redeem), a session persistence for browsers, and a storage object client (put, get, list, delete under the same auth), so an app no longer hand-rolls those requests.
  - Applications can manage membership as trusted claims: a service-credential route lets a function set a user's administrator-controlled app metadata (the input policies trust) with a bypass reason and audit record, and the edge SDK exposes it. Today only a management session can, which makes app-managed sharing impossible.
  - Every environment gains an allowlist of browser origins: a browser app served from a listed origin can call that environment's application API cross-origin, on the platform's own hostname and on any custom domain serving it. Today the data plane sends no CORS at all, so every browser app must sit behind a same-origin proxy or own a DNS name — a project's ordinary API URL must work from an ordinary web host, with no special treatment for anyone.
  - The supported RxDB integration is proven on durable storage: the replication state (checkpoints, security state) survives an app restart on IndexedDB and resumes rather than re-syncing.
- Rational is created on the beta the way a developer creates any project — the repository owner's developer account, the ordinary project API URL, no custom domain and no bespoke routing — with its static bundle's origin in the environment's allowlist, and the beta is requalified with Rational's smoke in the suite.

## Capabilities

### New Capabilities
- `samples/rational-money-app`: what Rational must do for its users — sign-in, households, accounts, transactions, rules, budgets, recurring, goals, reports, receipts, alerts, offline behavior — and what it must prove about the platform (findings loop, live suites, beta deployment).

### Modified Capabilities
- `identity/project-auth`: a new requirement for the environment's browser-origin allowlist governing cross-origin access to the application API, and "Trusted and user-editable metadata" gains a supported path for an application's own function to set administrator-controlled app metadata under a service credential, audited, so trusted claims can be managed by the app rather than only by a developer session.
- `sync/rxdb-replication`: "Supported RxDB client integration" gains durable local storage as a supported configuration — replication resumes from persisted checkpoints after a restart — and the client library's sign-in helpers cover every application sign-in method.

## Impact

- New: `examples/rational` (React + Vite + RxDB on Dexie/IndexedDB, edge functions, test suites, docs), `docs/rational.md`.
- `packages/rxdb-client`: auth helpers for providers and magic links, browser session persistence, storage object client; unit tests and the console's Connect template unchanged.
- `packages/edge-sdk`, `api/openapi/mako-cloud-v1.yaml`, `services/mako-data-plane` (service route to set app metadata), `crates/mako-identity` (audited metadata write), regenerated types.
- Cross-origin access end to end: a per-environment `allowed-origins` setting in the control plane installed into the data plane like the quota policy; the data plane and gateway answer preflights and echo listed origins on application routes only (never management, operator, or service), on the platform hostname and on custom domains; Caddy forwards `OPTIONS`; console environment settings, CLI, SDK, docs.
- `crates/mako-smoke`: a Rational smoke suite; the beta qualification includes it.
- Beta: no DNS or infrastructure prerequisite — Rational uses the platform's ordinary project URL, and its static bundle is hosted like any application's would be.
- No breaking changes: every platform addition is additive; existing examples and clients keep working.
