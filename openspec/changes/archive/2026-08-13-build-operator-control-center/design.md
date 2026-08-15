## Context

The existing operator surface is a single page backed by exact project lookup and direct forms for provisioning repair, quota override, abuse response, support-session creation/revocation, and wait-list review. Operator authentication and per-action permissions already exist, as do durable control-plane records for several workflows, Prometheus alert rules, Grafana dashboards, backup evidence, release qualification evidence, and multiple audit event streams.

The new capability crosses the console, control-plane service, RocksDB-backed control state, observability providers, backup tooling, and operator authentication. It must preserve tenant isolation and the existing threat-model rule that the control plane cannot directly read customer document bodies. See `proposal.md` for motivation and `specs/cloud/operator-control-center/spec.md` for observable behavior.

## Goals / Non-Goals

**Goals:**

- Give operators fleet-to-tenant diagnosis paths before exposing contextual actions.
- Make freshness, partial failure, tenant scope, and authorization explicit in every view.
- Reuse existing telemetry and operational evidence through bounded server-side read models.
- Preserve existing operator endpoints while introducing coherent inventory, history, and guarded workflow resources.
- Make all new privileged access least-privilege, step-up protected where appropriate, idempotent, and auditable.

**Non-Goals:**

- Providing routine operator access to customer document bodies.
- Replacing Grafana, Prometheus, logs, traces, or runbooks as deep diagnostic systems.
- Building customer-facing data exploration, billing, subscriptions, or cost attribution.
- Adding multi-region storage, automatic RocksDB failover, or a new monitoring vendor.

## Decisions

### 1. Build an operator read-model layer in the control plane

The browser will call bounded operator APIs that compose durable control-plane records with safe summaries from telemetry, backup, release, and health providers. Responses will include section-level `observedAt`, freshness state, source status, and safe error classification. Tenant-scoped reads will require an explicit project/environment scope at every provider boundary.

The read-model layer will expose separate resources for overview, tenants, tenant sections, alerts/incidents, operations, backups, fleet, security, and activity rather than one unbounded mega-response. Tenant 360 will load independent sections so one unavailable provider does not blank the page.

**Alternative considered:** Query Prometheus and Grafana directly from the browser. Rejected because it exposes infrastructure credentials and query capability, complicates tenant filtering, and cannot consistently apply operator authorization, redaction, bounds, or audit.

### 2. Use cursor pagination and bounded filters for every inventory

Tenant, operation, alert, session, backup, and activity inventories will use opaque cursors, stable sort keys, server-enforced page limits, bounded time windows, and allowlisted filters. Email lookup will normalize an exact address or constrained prefix server-side and return only fields allowed to the current permission. No endpoint will provide an unrestricted RocksDB or telemetry scan.

**Alternative considered:** Offset pagination and client-side filtering. Rejected because concurrent writes cause duplicates or omissions and client filtering requires transferring more sensitive metadata than needed.

### 3. Treat the console as a contextual shell, not a form collection

The operator application will gain nested routes and a persistent navigation shell. Overview exceptions lead to filtered inventories; tenant results lead to Tenant 360; alerts lead to affected tenants and runbooks; workflow actions appear beside the current resource and its history. Route state will carry only safe identifiers, filters, and time windows. Permission-aware navigation improves usability but server authorization remains authoritative.

The initial route hierarchy is:

```text
/operator
  /overview
  /tenants
  /tenants/:projectId/:section?
  /operations/:kind?
  /incidents/:incidentId?
  /backups/:jobId?
  /fleet/:resourceId?
  /security/:section?
  /activity
```

**Alternative considered:** Incrementally add more panels to the current page. Rejected because it cannot preserve navigable context, scalable inventory views, or clear separation among diagnosis, history, and mutation.

### 4. Keep operational sources behind narrow provider interfaces

The control plane will define narrow providers for service/fleet health, telemetry summaries, current alerts, backup and recovery evidence, deployed release state, and approved deep links. The local deployment will implement these against the existing Prometheus-compatible endpoint, checked dashboard/runbook inventory, backup manifests, release evidence, and service composition state. Tests will use deterministic fakes.

Approved Grafana and runbook URLs will be generated from configuration and allowlisted templates. The server will add bounded identifiers and time ranges; it will never forward arbitrary operator-supplied destinations or raw queries.

**Alternative considered:** Copy all time-series and logs into the control-plane RocksDB database. Rejected because it duplicates the observability stack, creates retention and scale problems, and broadens the sensitive-data footprint.

### 5. Persist operator-owned state separately from observed alert state

External alert state is read-only observed data keyed by a stable alert fingerprint. Acknowledgement, assignment, annotations, and incident lifecycle are operator-owned records stored durably in the control-plane RocksDB keyspace. Incidents reference alert fingerprints and affected resources, allowing the console to retain a timeline even after a telemetry alert resolves. A stale alert provider never implicitly resolves an incident.

Provisioning actions, overrides, abuse responses, support sessions, restore jobs, and operator entitlement changes retain append-only history rather than overwriting their prior state.

**Alternative considered:** Require Alertmanager before adding incident workflows. Rejected for the initial deployment because existing Prometheus rules are sufficient for observation, while durable Mako-specific annotations and incident state still need an authoritative store.

### 6. Extend permissions by data class and action

New read permissions will be separated for global overview, tenant inventory, operational history, alerts/incidents, backups, fleet, operator security, and activity export. Existing mutation permissions remain distinct. High-impact actions additionally require a recent step-up marker bound to the operator session, action class, and short validity window.

UI checks only control presentation. Every service and provider call rechecks the operator principal, exact scope, permission, and step-up status. The existing authenticated operator identity remains separate from developer membership and application-user identities.

**Alternative considered:** Grant all views and actions to every operator account. Rejected because operational metadata, security administration, and recovery authority have different sensitivity and job responsibilities.

### 7. Standardize guarded workflow envelopes

State-changing operator APIs will accept a shared envelope containing an idempotency key, reviewed resource version, reason, case reference, and confirmation metadata. Temporary records additionally require an expiry. The service validates current state, scope, permission, and step-up status before mutation, then returns the durable workflow record and audit reference.

Existing mutation endpoints will remain compatible while their implementations adopt the same validation and history model. New list/detail/revoke routes will supply the context missing from today's create-oriented flows.

**Alternative considered:** Rely on a browser confirmation alone. Rejected because retries, stale tabs, scripted clients, and compromised frontends require server-enforced safeguards.

### 8. Model recovery as a job with verification gates

The console will not execute ad hoc restore commands. It will create a recovery job referencing a verified backup, protected target, operator case, impact preview, and reviewed state. The job state machine will cover requested, preparing, restoring, verifying, ready-for-promotion, promoted, failed, and cancelled states. Promotion requires explicit step-up confirmation and successful automated verification.

The first implementation may wrap existing runbook commands behind an executor interface, but job state and evidence remain durable and visible. A feature gate can leave mutation disabled while still delivering backup inventory and restore-drill visibility.

**Alternative considered:** Add a one-click shell-command trigger. Rejected because it lacks durable state, safe retry semantics, evidence binding, and a promotion gate.

### 9. Present safe aggregates for RxDB and RocksDB

Sync pages will use bounded project/environment/collection aggregates already permitted by the metric contract. Fleet pages will use service/region/volume aggregates and safe failure classes. Raw document IDs, selectors, keys, values, tokens, emails, and other high-cardinality or sensitive fields will not appear in metrics or drill-down URLs.

Any support workflow that eventually reads document content remains outside the ordinary read model and must present a persistent support-mode banner tied to a verified, time-bounded support session.

### 10. Provide an append-only unified operator activity projection

Existing control, operator, authentication, provisioning, recovery, and incident events will feed a normalized activity projection containing source event ID, actor, action, target, scope, case, request correlation, outcome, timestamp, and integrity metadata. The projection stores no document bodies or secrets. Source events remain authoritative; the projection can be rebuilt and detects gaps rather than fabricating continuity.

Exports will be asynchronous for larger result sets, preserve the filter and generation time, expire automatically, and require a distinct export permission.

**Alternative considered:** Query every audit sink live for each page. Rejected because schemas, retention, and query behavior differ, making stable pagination and integrity-gap detection unreliable.

## Risks / Trade-offs

- **[Cross-source data can be temporally inconsistent]** → Include source observation times and freshness states, avoid cross-source transactional claims, and refresh sections independently.
- **[Global inventories can cause expensive RocksDB scans]** → Add bounded secondary indexes, stable cursors, minimum search constraints, page limits, and performance tests with realistic cardinality.
- **[An operator UI increases privileged attack surface]** → Perform threat-model review, split permissions, require server-side scope checks and step-up, redact every provider response, and add cross-tenant/adversarial tests.
- **[Telemetry outages can mislead operators]** → Surface stale/unknown states explicitly and never translate missing data into healthy or resolved state.
- **[Deep links can leak sensitive scope]** → Generate only allowlisted destinations with safe identifiers and bounded time ranges; prohibit arbitrary URLs and raw query forwarding.
- **[Recovery automation can amplify an operator mistake]** → Default restore mutation off until qualified, require verified inputs and state versions, separate restore from promotion, and retain rollback evidence.
- **[A large console can become difficult to deliver atomically]** → Land the shell and read-only views first, then contextual workflow histories, then gated recovery and security mutations while preserving existing routes.

## Migration Plan

1. Add keyspace versions and backfill/rebuild jobs for tenant search, workflow history, incident overlays, and unified activity projection; validate counts without changing current operator behavior.
2. Add provider interfaces, bounded read APIs, permissions, audit coverage, and deterministic contract tests. Keep new routes disabled outside qualification environments.
3. Deploy the navigation shell, overview, tenant directory, Tenant 360, alert inventory, backup inventory, fleet, and activity as read-only pages. Preserve links to the existing operator page as a rollback path.
4. Move existing provisioning, quota, abuse, and support actions into contextual pages after parity, stale-state, idempotency, and audit tests pass.
5. Enable incident mutations, session administration, and recovery job creation independently after step-up and operational qualification. Keep restore promotion separately gated.
6. Remove the legacy single-page route only after usage and audit evidence show that all existing workflows have equivalent replacements.

Rollback disables the new route and mutation gates while retaining newly written append-only records and indexes. Existing operator APIs remain available during rollback; migrations are additive and do not delete source records.
