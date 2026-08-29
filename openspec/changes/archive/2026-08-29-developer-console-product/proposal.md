## Why

A developer who signs in today lands on a bare list — "Your projects", "Teams" — and a project page that shows environments, provisioning state, and data-plane health. Nothing tells them what to do next, and nothing looks like the cloud database they signed up for. Yet most of the product already exists behind that page: collections and schemas, policies, application users, credentials and keys, functions with retained logs, observability, a document explorer, replication diagnostics, backups, billing. It is reachable only by deep environment URLs and shown without a persistent navigation, so the console reads as empty while the platform underneath is not.

Measured against the product developers compare us to — Supabase — there are also real gaps: no application file storage, no social sign-in or magic links, no email templates, no database webhooks, no scheduled functions, no custom domains, no per-project API docs, no project settings (rename, transfer, delete), and no usage, billing, or activity visible where the developer is working.

## What Changes

**Phase 1 — the console becomes a product (existing APIs only).**
- A home dashboard after sign-in: projects across the personal space and every team as cards with state, region, plan, and headline usage; recent activity; a first-run onboarding flow that creates a project and walks the developer to a working client in one session.
- A project home: overview, environments, API URL and keys, quickstart, usage and health at a glance, and the same navigation the environment level has.
- A persistent environment sidebar — Database, Explorer, Auth, Functions, Sync, Logs, Observability, API & Keys, Backups, Settings — replacing deep-link-only reachability. Every existing screen is mounted in it.
- Surfacing what the backend already serves but the console never shows: retained project logs, index build state, data-job detail, JWT signing-key initialization, team renaming, and team-level plan and credits.

**Phase 2 — settings, money, and history where developers work.**
- Project settings: rename, transfer between the personal space and teams the developer administers, and deletion with grace, all audited.
- Usage and billing in the console at project and team level, and a developer-facing activity feed derived from the audit trail.

**Phase 3 — the product areas that are missing.**
- Application file storage: buckets per environment, policy-governed object access from application sessions, metered and billed.
- Auth providers: social sign-in, magic links, and per-environment email templates for application users.
- Database webhooks: durable, signed HTTP deliveries on collection changes, with retries and delivery logs.
- Scheduled functions: cron-triggered invocations of deployed functions, with history.
- Custom domains for a project's API and functions, with managed certificates.
- Generated per-project API documentation and quickstarts from the environment's live collections and keys.

Nothing here is **BREAKING**: every existing route, screen, and API keeps working; new surfaces are added around them.

## Capabilities

### New Capabilities
- `cloud/developer-console`: the console shell — home dashboard, project home, environment navigation, onboarding, in-console usage/billing/activity, project settings.
- `storage/application-file-storage`: buckets and objects for application data, governed by policies and metered.
- `identity/auth-providers`: social sign-in, magic links, and email templates for application users.
- `sync/database-webhooks`: signed HTTP deliveries on collection changes with retries and logs.
- `functions/scheduled-functions`: cron-triggered function invocations with history.
- `operations/custom-domains`: developer-managed custom domains with certificates.
- `cloud/api-documentation`: generated per-project API docs and quickstarts.

### Modified Capabilities
- `cloud/developer-database-workspace`: the persistent workspace becomes the environment sidebar of the console shell; the project overview gains keys, usage, and health at a glance; workspace context requirements extend to the home and project levels.
- `cloud/control-plane`: project lifecycle gains rename, transfer, and developer-initiated deletion with grace.

## Impact

- Console: a new shell (navigation, home, project home, onboarding) around the existing screens; new screens for settings, billing/usage, activity, storage, auth providers, webhooks, schedules, domains, and docs.
- Control plane: project rename/transfer/delete, activity feed derivation from control audit, and — in phase 3 — new stores and services for storage buckets, auth providers, webhooks, schedules, and domains, with new OpenAPI operations, regenerated types, SDK methods, Caddy allowlist entries, and traceability rows.
- Data plane and edge: object access for storage, provider callbacks and magic-link issuance for auth, change-feed consumers for webhooks, timer-driven invocations for schedules.
- Billing: storage and webhook delivery become metered resources on the existing ledger.
- Phase 1 needs no backend change beyond what exists; phases 2 and 3 each ship as their own applied change under this proposal's design.
