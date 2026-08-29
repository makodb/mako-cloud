## Context

See proposal.md for motivation. What shapes the approach:

- The console already implements most product areas at the environment level (collections and schemas, policies, application users, credentials, functions, observability with retained logs, a document explorer, an environment workspace with overview, sync, backups, connect, and settings). The `cloud/developer-database-workspace` spec already requires a persistent workspace with these destinations; it is implemented only below the environment and reached only by deep links.
- The management API serves 92 operations; 21 SDK methods are never called by the console, several of them user-facing (retained logs, index state, data-job detail, signing-key initialization, team rename, plan and credits).
- Teams and personal spaces now exist; every project has an owner with a plan, a bill, and installed quota policy. Audit events are already queryable per environment through the observability API.
- The platform has no object storage for applications, no external sign-in, no outbound webhooks, no scheduler, and no custom-domain routing. The object store used for function bundles is an internal dependency, not a tenant surface.
- Constraints that do not move: fail-closed startup and readiness; no async runtime; tenant identity from verified credentials; secrets as references; docs and traceability as part of every change; each applied change requalified on the beta before its evidence is committed.

## Goals / Non-Goals

**Goals:**
- A developer signing in sees a product: what they have, what to do next, and how to reach every area in at most two selections.
- Phase 1 changes only the console and consumes only existing APIs, so it ships in days and needs no data-plane deploy.
- Phases 2 and 3 are each an ordinary applied change with its own proposal-level design already settled here, so they can be scheduled independently and in any order after phase 1.

**Non-Goals:**
- Redesigning the visual language of existing screens; the shell frames them, it does not rewrite them.
- SQL, table editors, or relational features — this is a document database with RxDB replication, and the console says so.
- Multi-region, custom compute sizes, or self-service infrastructure changes.
- Payment collection — money stays informational, as the billing change decided.

## Decisions

**1. A single navigation shell owns layout; screens stay as they are.**
A `ConsoleShell` component renders the top context bar (team / project / environment switchers, identity) and the left destination list, and mounts the existing screen for the current route. Routes gain no new path shapes beyond the home (`/`), project home (`/projects/{id}`), and new destination segments; every existing deep link keeps working because the shell reads the same router state. *Alternative rejected:* a rewrite of the screens into a new component tree — most of the 12k lines are behavior we would only re-test.

**2. Home and project home are read-only aggregations of existing endpoints.**
Home lists projects via `listTeams` → `listProjects` per team (personal space included), then per selected environment the workspace summary, usage, and audit-events endpoints fill the cards lazily with per-card unavailability. No new "dashboard" API: the summaries the workspace spec already requires are the source, and a per-card fetch keeps one failing project from blanking the page. *Alternative rejected:* a server-side dashboard endpoint — it would duplicate the workspace summary and force a control-plane deploy for a console change.

**3. Destinations the deployment lacks are shown, disabled, with a reason.**
Storage, providers, webhooks, schedules, and domains appear in the shell from phase 1 as "not available on this deployment" until their phase ships. Developers see the product's shape; nothing pretends to work. The shell learns availability from a capability list the console already receives with workspace navigation (`getWorkspaceNavigation`), extended with these areas.

**4. Onboarding is a client-side flow over existing calls, with progress stored per developer.**
Create project → wait for active → show keys → connection check is exactly what the environment "Connect" screen already does, sequenced. Progress lives in the developer's tab-scoped browser storage (session storage) keyed by identity, because the console holds no durable local authority; dismissing it is local, and a lost tab simply shows the guide again where the server state warrants it. *Alternative rejected:* server-side onboarding state — nothing else needs it, and a developer on a new browser simply sees the guide again.

**5. Activity is the existing audit-events signal, read by the developer.**
The observability audit-events endpoint already serves tenant-scoped audit records to authorized members. The feed is that endpoint, newest first, with actor and target rendered. Team-level activity unions the team's projects' feeds client-side, bounded. *Alternative rejected:* a new activity store — it would be a second audit trail.

**6. Project transfer is an ownership change, not a copy.**
Transfer rewrites the project record's owner, updates both owners' project indexes atomically in the control store, reinstalls quota policy on every environment from the new owner's plan (the same install path environment creation uses), and emits two audit events. Data-plane state is keyed by project and environment, not by owner, so nothing moves. Authorization requires administrator or owner on both sides; a personal space counts as administered by its developer. *Alternative rejected:* copy-and-delete — it would invalidate identifiers, keys, and clients.

**7. Application file storage rides the internal object store behind a tenant-scoped API in the data plane.**
Buckets and object metadata live in the environment's RocksDB keyspace (owner, content type, size, policy scope); bytes live in the existing object store under keys prefixed by project and environment, encrypted with the environment's key. Policies are the document-policy engine evaluated against a synthetic `object` document (path, owner, metadata), so a developer writes one policy language. Metering adds `object_storage_bytes` (a level, sampled like stored bytes) and `object_egress_bytes` (a flow) to the ledger and rate card. *Alternative rejected:* a separate storage service — it would need its own auth, policy, and metering, all of which exist.

Applied as follows: the data plane opens the same S3 store the control plane uses (a distinct bucket, `mako-application-objects-v1`) and requires its credentials; there was no encryption in the store, so the data plane encrypts every object with XChaCha20-Poly1305 under a key derived per tenant from its secret, and the store's address is the ciphertext digest. Bucket rules live inline on the bucket record (the data plane compiles them against the object schema and refuses a bucket whose rules do not compile) rather than in the collection policy store, which is keyed by collection and demands a collection record. A public bucket's reads need no credential at all, not merely no session. The store reads and writes whole objects, so the ceiling is 16 MiB and the transport gained a per-route body bound and a trailing `{path...}` route segment. The control plane keeps no bucket state; it forwards authorized developer actions to the data plane, which is the source of truth.

**8. Auth providers and magic links live in the data plane's identity service; templates in control.**
Provider configuration (client id, secret reference) and template text are control-plane records per environment installed to the data plane the way quota policy is; the OAuth callback and magic-link issuance run in the data plane where application-user sessions are minted. Provider identities are linked to application users by verified email plus provider subject, never by email alone. Templates are rendered with an allowlist of variables and no HTML from the developer beyond a safe subset. *Alternative rejected:* delegating to an external auth product — the platform's value is that identity, policy, and data share one authority.

Applied as follows, because the platform had no outbound TLS client and no mail path outside the control plane's developer outbox. Provider configuration is a control-plane record installed to the data plane like quota policy, with the client secret carried as ciphertext under a key both planes derive from the shared internal-auth secret — a value, never returned, rather than a reference (a per-environment reference into the host's environment or filesystem would let a tenant name deployment secrets). A small HTTPS client crate over the already-locked rustls and webpki-roots gives the data plane the token exchange and identity fetch; outside production it also speaks plain HTTP to loopback so stubs can stand in for providers. The browser flow never carries tokens in URLs: the application asks the data plane to start a flow and navigates to the provider; the callback binds the signed state to the environment, links or creates the user by verified email and provider subject, and sends the browser back to a redirect the developer registered with a one-time code the application exchanges for a session with its public key. Magic links, verification, recovery, and invitations are written by the data plane as mail intents (kind, recipient, variables) in the environment's keyspace; the control plane's mail worker drains them over an internal route, renders them with the environment's template or the built-in default, and sends them through its outbox, so mail delivery keeps one transport and one durability story. Templates stay in the control plane. A plaintext SMTP mode exists for local deployments only, so mailpit and the smoke suite's capturing stub can receive mail. A magic link is also how a user signs up: an address without a user gets one, pending until the link proves the address, and redemption activates it — the request answers identically either way. The control plane never learns a plain client secret after sealing it; an update that omits a provider's secret keeps the installed one by sending an empty seal the data plane substitutes, and a new provider without a secret is refused before anything is forwarded.

**9. Webhooks consume the replication change stream in a control-plane worker with a durable outbox.**
Each environment's change stream is already what live replication reads; a worker subscribes per subscribed collection, writes deliveries to a durable outbox in the control store, and a delivery loop posts with HMAC signatures, exponential backoff, a bounded retry window, and per-endpoint pause on sustained failure. The outbox is the at-least-once guarantee across restarts. Delivery logs are retained like function logs. *Alternative rejected:* firing from the data plane's write path — a slow endpoint must never slow a write.

**10. Schedules are a control-plane timer driving the existing gateway invocation path.**
A worker evaluates cron expressions in UTC, invokes through the edge gateway with a scheduler credential so quotas and metrics apply as for any invocation, records each run, and skips on overlap using a per-schedule lease. *Alternative rejected:* a runtime-internal scheduler — invocations would bypass admission, metering, and the function metrics the console shows.

**11. Custom domains are Caddy on-demand TLS gated by a verified-domain list.**
The control plane stores domains with a DNS TXT challenge; a verifier worker checks the record, marks the domain verified, and publishes the allowlist Caddy's on-demand TLS `ask` endpoint consults, so certificates are issued only for verified domains and serving stops when verification is withdrawn. Routing maps a verified domain to the project's API and function routes. *Alternative rejected:* manual certificate upload — it shifts the hardest part to the developer.

**12. API documentation is generated in the console from live metadata.**
Collections, schemas, indexes, policies, function routes, and the environment's public key are all already served; the docs screen renders them into reference pages and client quickstarts client-side, stamped with the observation time. No server render, no stored docs. *Alternative rejected:* generating static docs on deploy — they would drift from the environment.

## Risks / Trade-offs

- [Home dashboard fans out one request per project] → cards load lazily with a bounded concurrency; the page is usable before summaries arrive, and a failing project marks only its card.
- [Showing disabled phase-3 destinations could read as broken] → each carries an explicit "not available on this deployment" state with a one-line reason; nothing is a dead link.
- [Transfer changes which plan limits apply to live traffic] → limits are reinstalled per environment before the owner change commits; if reinstall fails the transfer is refused and nothing is half-moved.
- [Object storage adds a second byte store to back up] → object keys are deterministic from tenant metadata, so recovery is "restore metadata, verify objects"; the backup spec gains objects in phase 3, and until then the area stays unavailable.
- [Webhook deliveries can amplify a chatty collection] → per-endpoint rate limits and a bounded outbox with visible backpressure; a paused endpoint is a state the console shows, not silent loss.
- [Provider secrets and magic links are new attack surface] → secrets are references never returned; links are single-use, short-lived, and bound to environment and email; both paths emit authentication events.
- [On-demand TLS can be abused to issue certificates for domains nobody verified] → the `ask` endpoint answers only from the verified list; unverified names are refused before any ACME request.

## Migration Plan

- Phase 1 is a console-only release: build, deploy the candidate, requalify (the operator browser qualification covers the shell), commit evidence. Rollback is the previous release; no data changes.
- Phase 2 adds control-plane operations (rename, transfer) and console screens; additive records, additive OpenAPI, Caddy allowlist entries via converge, requalification.
- Phase 3 ships each area as its own applied change: storage, providers, webhooks, schedules, domains, docs. Each adds records, workers, operations, allowlist entries, metering where applicable, and traceability rows; each is requalified independently. Areas stay marked unavailable in the shell until their change is live.

## Open Questions

- Which social providers ship first (Google and GitHub are assumed); others are configuration once the OAuth path exists.
- Whether public buckets should be served from a distinct hostname to isolate cookies; the default is the API host under `/storage/v1/`, which the custom-domains work can revisit.
