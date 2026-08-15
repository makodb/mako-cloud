## 1. Contracts and Security Boundaries

- [x] 1.1 Update the threat model and abuse-case registry for global operator reads, incident management, recovery jobs, entitlement administration, activity export, and observability deep links.
- [x] 1.2 Define the operator overview, tenant inventory, Tenant 360, operations, incident, backup, fleet, security, and activity API schemas with bounded filters, cursors, freshness, and partial-failure fields.
- [x] 1.3 Define separate read and mutation permissions for every new operator resource and map existing operator roles to least-privilege defaults.
- [x] 1.4 Define the step-up authentication contract, validity window, action binding, and safe reauthentication response used by high-impact workflows.
- [x] 1.5 Add contract tests proving unauthorized permissions, stale step-up, malformed scope, and cross-tenant requests return safe failures and durable denied audit events.

## 2. Operator Read-Model Foundation

- [x] 2.1 Add shared opaque-cursor, stable-sort, bounded-page, bounded-time-window, freshness, and provider-status types for operator inventories.
- [x] 2.2 Add narrow provider interfaces for telemetry summaries, current alerts, fleet health, backup evidence, release state, and approved diagnostic links, with deterministic test fakes.
- [x] 2.3 Add server-side redaction and response validation that rejects secrets, document payloads, unsafe URLs, and unbounded high-cardinality fields from operator read models.
- [x] 2.4 Add additive RocksDB keyspaces and versioned records for tenant search indexes, operation history, incident overlays, recovery jobs, and unified activity projection.
- [x] 2.5 Implement idempotent index backfill and projection rebuild commands with count/checksum validation and restart-safe progress.
- [x] 2.6 Add realistic-cardinality performance tests for global inventories and prove requests cannot trigger unrestricted RocksDB scans.

## 3. Tenant Directory and Tenant 360 APIs

- [x] 3.1 Implement normalized bounded tenant search by organization, project, environment, developer email, and identifiers with lifecycle, health, region, and plan/quota filters.
- [x] 3.2 Implement cursor-paginated operator-safe tenant summaries with stable ordering and audited read access.
- [x] 3.3 Implement the Tenant 360 summary and topology endpoints with independently loadable lifecycle, provisioning, usage, and quota sections.
- [x] 3.4 Implement Tenant 360 sync, authentication, function, backup, safe-error, operator-activity, and support-history sections through scoped provider calls.
- [x] 3.5 Add partial-provider failure and stale-data behavior so unavailable sections never blank successful sections or appear healthy.
- [x] 3.6 Add integration tests for pagination under concurrent writes, email normalization, scope retention, partial failure, redaction, and cross-tenant isolation.

## 4. Overview and Diagnostic Integration

- [x] 4.1 Implement the global overview aggregator for tenant lifecycle, service readiness, request health, alerts, sync, storage, backups, registration/mail, and release state.
- [x] 4.2 Implement Prometheus-compatible summary queries with enforced time ranges, bounded labels, deadlines, and explicit stale/unknown results.
- [x] 4.3 Implement allowlisted Grafana, log, trace, and runbook link generation using only safe identifiers and bounded time filters.
- [x] 4.4 Add overview provider health metrics, latency/error instrumentation, and alerting for stale or failed operator read sources.
- [x] 4.5 Add tests that provider outages degrade only their section and that missing telemetry is never represented as healthy or resolved.

## 5. Alerts and Incident Management

- [x] 5.1 Implement current-alert ingestion and normalization with stable fingerprints, severity, affected scope, observation times, and runbook references.
- [x] 5.2 Implement durable incident, acknowledgement, assignment, annotation, and resolution records with optimistic state versions.
- [x] 5.3 Implement alert and incident list/detail APIs with cursor pagination, bounded filters, ordered timelines, and permission checks.
- [x] 5.4 Ensure stale alert-provider state cannot implicitly resolve active alerts or incidents and expose the source freshness visibly.
- [x] 5.5 Add tests for repeated observations, alert disappearance, stale input, concurrent acknowledgement, assignment, resolution, and immutable history.

## 6. Contextual Operational Workflows

- [x] 6.1 Implement a guarded operator mutation envelope with idempotency key, reviewed resource version, reason, case reference, confirmation data, and step-up proof where required.
- [x] 6.2 Add paginated failed, stalled, and pending provisioning inventories with safe error classes, attempt history, and available repair actions.
- [x] 6.3 Update provisioning repair to revalidate reviewed state, preserve existing API compatibility, and return durable action and audit references.
- [x] 6.4 Add list/detail/history/replace/revoke behavior for quota overrides, including bounded expiry and current effective value.
- [x] 6.5 Add list/detail/history/restore behavior for abuse responses with explicit affected project or environment scope.
- [x] 6.6 Add list/detail/history/revoke behavior for support sessions and preserve exact project, environment, permission, reason, and expiry checks.
- [x] 6.7 Add tests for stale-state rejection, retry idempotency, conflicting operation keys, expiry, revocation propagation, cross-context payload rejection, and audit completeness.

## 7. Backup and Recovery Control

- [x] 7.1 Implement a backup inventory provider for age, size, integrity, remote verification, protected target, restore drills, and recovery-objective status.
- [x] 7.2 Implement the durable recovery-job state machine for request, preparation, restore, verification, promotion readiness, promotion, failure, cancellation, and retry-safe resumption.
- [x] 7.3 Add a recovery executor interface that wraps approved existing backup/restore procedures without accepting arbitrary commands or paths.
- [x] 7.4 Enforce verified backup, reviewed target state, impact preview, step-up authorization, and successful post-restore verification before promotion.
- [x] 7.5 Add independent feature gates for recovery-job creation and restore promotion, defaulting mutation off until qualification succeeds.
- [x] 7.6 Add failure-injection tests for invalid evidence, interrupted restore, failed verification, repeated callbacks, stale promotion, and rollback preservation.

## 8. RxDB, Fleet, and RocksDB Views

- [x] 8.1 Implement bounded global and tenant-scoped RxDB summaries for live streams, pull/push outcomes, lag, conflicts, policy denials, checkpoint expiry, and resynchronization.
- [x] 8.2 Implement service and instance inventory for region, version, readiness, restarts, dependencies, certificate expiry, and configuration drift.
- [x] 8.3 Implement RocksDB volume summaries for readiness, capacity, write stalls, compaction, background/corruption errors, backups, and recovery evidence.
- [x] 8.4 Add safe drill-down relationships among sync/storage exceptions, active alerts, affected tenants, dashboards, and runbooks.
- [x] 8.5 Validate all new metrics and queries against the bounded-label contract and add tests excluding document IDs, selectors, keys, values, tokens, and raw emails.

## 9. Operator Security and Activity

- [x] 9.1 Implement operator identity, role-derived entitlement, active/recent session, authentication-failure, throttling, and support-grant inventories.
- [x] 9.2 Implement stepped-up operator session revocation and entitlement-change workflows with last-recoverable-administrator protection.
- [x] 9.3 Normalize control, operator, authentication, provisioning, recovery, and incident events into an append-only activity projection with source-event integrity and gap detection.
- [x] 9.4 Implement cursor-paginated activity search by time, actor, action, target, tenant, case, request, and outcome.
- [x] 9.5 Implement expiring asynchronous activity exports with preserved filters, integrity metadata, distinct export permission, and no document bodies or secrets.
- [x] 9.6 Add tests for revocation bounds, entitlement races, projection rebuild, duplicate source events, integrity gaps, export expiry, and redaction.

## 10. Operator Console Shell

- [x] 10.1 Add nested operator routes and a persistent responsive navigation shell for overview, tenants, operations, incidents, backups, fleet, security, and activity.
- [x] 10.2 Add shared page, filter, data-table, cursor-pagination, freshness, partial-error, empty-state, status, and drill-down components.
- [x] 10.3 Make navigation and controls permission-aware while preserving server-authoritative authorization and safe direct-route failures.
- [x] 10.4 Keep operator identity, session expiry, sign-out, and active support-mode state visible throughout the workspace.
- [x] 10.5 Add keyboard, focus, screen-reader announcement, responsive-layout, and sensitive-URL regression tests for the shell and shared components.

## 11. Operator Console Pages and Workflows

- [x] 11.1 Build the overview page with exception-first cards, source freshness, affected counts, and links to filtered operator views and approved diagnostics.
- [x] 11.2 Build the tenant directory and Tenant 360 pages with independently loading sections and preserved filters/time context.
- [x] 11.3 Build alert inventory, incident detail, acknowledgement, assignment, annotation, and resolution experiences with an ordered timeline.
- [x] 11.4 Build provisioning, quota, abuse, and support inventories and move existing action forms into the selected tenant/workflow context.
- [x] 11.5 Build backup inventory and gated recovery-job detail, progress, verification, and promotion-confirmation experiences.
- [x] 11.6 Build RxDB sync, fleet, and RocksDB pages with safe aggregate drill-down and runbook/dashboard links.
- [x] 11.7 Build operator security, session revocation, entitlement administration, activity search, and activity export experiences.
- [x] 11.8 Add persistent support-mode indication and deny ordinary document-content routes unless a matching verified support session is active.
- [x] 11.9 Add end-to-end tests for permission-specific navigation, partial outages, tenant diagnosis, contextual actions, step-up return, idempotent retry, and support-mode boundaries.

## 12. Qualification and Rollout

- [x] 12.1 Extend OpenAPI documentation, operator procedures, runbooks, dashboard inventory, and permission/role documentation for every new view and workflow.
- [x] 12.2 Run Rust and console formatting, linting, unit, integration, accessibility, observability-contract, redaction, and cross-tenant test suites.
- [x] 12.3 Run index backfill and activity-projection shadow validation against a deployment snapshot and record count/checksum evidence.
- [x] 12.4 Deploy the read-only shell and views behind a feature gate while retaining the legacy operator page as a rollback path.
- [x] 12.5 Qualify contextual mutations, incident workflows, security administration, recovery-job creation, and restore promotion as separate gates with audit evidence.
- [x] 12.6 Verify production-like operator routes, API allowlists, HTTPS behavior, deep links, stale-provider handling, and rollback before enabling the new console by default.
- [x] 12.7 Remove the legacy page only after every existing workflow has parity, successful usage evidence, and a documented rollback path.
