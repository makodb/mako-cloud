## 1. Phase 1 — the shell (console only, existing APIs)

- [x] 1.1 Navigation shell: context bar (team, project, environment switchers, identity) and destination list; mount every existing environment screen inside it; deep links keep working.
- [x] 1.2 Availability: extend workspace navigation with the phase-3 areas marked unavailable, and render them disabled with a reason.
- [x] 1.3 Home dashboard: project cards across personal space and teams with state, region, plan, headline usage; per-card lazy loading and unavailability.
- [x] 1.4 Project home: environments, readiness, keys and API URL, quickstart, usage, health, activity, with the shell's destinations.
- [x] 1.5 First-run onboarding: create project → wait for active → keys → connection check; dismissible, resumable, stored per developer.
- [x] 1.6 Surface served capabilities: retained logs with filters, index build state, data-job detail, signing-key initialization, team rename, plan and credits.
- [x] 1.7 Usage per project and environment, bill and balance per team including the personal space, all carrying the non-payable notice.
- [x] 1.8 Activity feed from audit events at project and team level.
- [x] 1.9 Keyboard and assistive-technology pass over the shell; home and project URLs follow the workspace context rules.
- [x] 1.10 Console e2e: home with projects, empty-state onboarding, deep link inside the shell, logs and activity screens; docs and traceability rows; deploy and requalify.

## 2. Phase 2 — settings and ownership

- [x] 2.1 Control plane: rename project (audited) with OpenAPI, SDK, Caddy allowlist.
- [x] 2.2 Control plane: transfer project between owners — atomic owner change, both indexes, quota reinstall from the new plan, dual audit; refused if reinstall fails.
- [x] 2.3 Console: project settings with rename, transfer, and deletion-with-grace, each confirmed.
- [x] 2.4 Smoke: transfer a project from a personal space to a team against real services; console e2e for settings; docs, traceability, deploy, requalify.

## 3. Phase 3 — application file storage

- [ ] 3.1 Data plane: bucket and object metadata records per environment; object bytes in the object store under tenant-prefixed encrypted keys.
- [ ] 3.2 Policy evaluation for objects through the document-policy engine over a synthetic object document; public-bucket reads without a session.
- [ ] 3.3 Application API: upload, download, list, delete under a bucket; service-credential path with privileged audit; path validation.
- [ ] 3.4 Metering: object storage bytes (sampled) and object egress (flow) on the ledger and rate card; plan allowances and caps.
- [ ] 3.5 Console: buckets, objects, limits; management OpenAPI and SDK; Caddy allowlist; backup inventory gains objects.
- [ ] 3.6 Tests: policy denial, path escape refusal, metering on the bill; smoke through the real stack; deploy and requalify.

## 4. Phase 3 — auth providers and email templates

- [ ] 4.1 Control plane: provider configuration per environment with secret references; installation to the data plane like quota policy.
- [ ] 4.2 Data plane: OAuth and OIDC callback flow issuing application-user sessions, identity linking by verified email plus subject, authentication events.
- [ ] 4.3 Magic links: request, single-use spend, expiry, non-revealing responses.
- [ ] 4.4 Email templates per environment with allowlisted variables, validation, and preview; wired into verification, recovery, invitation, and magic-link mail.
- [ ] 4.5 Console: providers and templates screens; OpenAPI, SDK, allowlist; tests including a provider stub end to end; deploy and requalify.

## 5. Phase 3 — database webhooks

- [ ] 5.1 Control plane: endpoint records per environment with collection subscriptions and a signing-secret reference shown once.
- [ ] 5.2 Worker: change-stream consumer per subscription writing to a durable outbox; delivery loop with HMAC signatures, backoff, bounded retry window, per-endpoint pause.
- [ ] 5.3 Delivery log retained like function logs; redelivery.
- [ ] 5.4 Console: endpoints, subscriptions, delivery log, redeliver; OpenAPI, SDK, allowlist; tests including an endpoint that fails then recovers; deploy and requalify.

## 6. Phase 3 — scheduled functions

- [ ] 6.1 Control plane: schedule records with validated cron expressions in UTC targeting a function's active deployment.
- [ ] 6.2 Worker: due-time evaluation, invocation through the gateway with a scheduler credential, per-schedule lease for overlap skipping, run history.
- [ ] 6.3 Console: schedules on the function page with next run and history; OpenAPI, SDK, allowlist; tests for overlap skipping; deploy and requalify.

## 7. Phase 3 — custom domains

- [ ] 7.1 Control plane: domain records with DNS challenge, verification worker, verified-domain list publication.
- [ ] 7.2 Caddy: on-demand TLS with an `ask` endpoint answering from the verified list; routing of verified domains to the project's API and functions; converge and validators.
- [ ] 7.3 Console: add domain, show challenge, verification state, remove; OpenAPI, SDK; tests for unverified refusal and re-verification failure; deploy and requalify.

## 8. Phase 3 — generated API documentation

- [ ] 8.1 Console: reference pages generated from live collections, schemas, indexes, policies, and function routes with observation time.
- [ ] 8.2 Quickstarts per supported client with the environment's API URL and public key; never a service credential.
- [ ] 8.3 Console e2e over a schema-bearing collection; docs and traceability rows.
