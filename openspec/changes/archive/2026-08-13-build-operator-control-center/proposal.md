## Why

The operator console currently exposes a handful of high-impact actions after an exact project-ID lookup, but it does not give operators the fleet, tenant, alert, recovery, and audit context needed to understand platform state before acting. Mako Cloud needs a conventional, metadata-first operations console that makes routine diagnosis fast while keeping customer document access exceptional and tightly audited.

## What Changes

- Add a navigable operator control center with a global overview of service health, tenant lifecycle, active alerts, RxDB sync health, RocksDB capacity, backup freshness, registration/mail health, and deployed release state.
- Add a searchable, paginated tenant directory and an operator-safe Tenant 360 view covering topology, lifecycle, usage and quotas, sync, authentication, functions, backups, recent errors, audit history, and support history.
- Add alert and incident views with severity, ownership, acknowledgement, resolution, affected scope, timelines, and runbook links.
- Add operational work queues and histories for provisioning repair, quota overrides, abuse response, support sessions, and guarded backup/restore operations; relocate existing mutations into the relevant tenant or incident context.
- Add fleet and storage views for service instances, deployed versions, readiness, restarts, configuration drift, certificate status, RocksDB capacity, compaction pressure, write stalls, and recovery signals.
- Add security administration for operator roles and entitlements, session inventory and revocation, authentication anomalies, support grants, and searchable/exportable immutable activity history.
- Reuse existing Prometheus/Grafana telemetry for deep diagnostics while exposing bounded, operator-safe summaries and links in the console.
- Keep ordinary operator views metadata-only. Customer document bodies remain unavailable unless a separately authorized, time- and case-bound support session grants the exact access, with visible impersonation and enhanced audit.
- Exclude billing, subscription management, and customer-facing document exploration from this change.

## Capabilities

### New Capabilities

- `cloud/operator-control-center`: Provide fleet-wide operational awareness, searchable tenant context, safe incident and recovery workflows, operator security administration, and auditable platform actions.

### Modified Capabilities

None.

## Impact

- Expands the operator web application and its navigation, accessibility, and permission-aware presentation.
- Adds bounded operator read APIs and workflow APIs in the control plane for global summaries, tenant search, operational histories, alerts, incidents, backups, fleet state, operator sessions, and audit activity.
- Integrates safe aggregations from existing metrics, logs, alert rules, backup evidence, release metadata, and provisioning state without exposing high-cardinality secrets or document payloads.
- Extends operator authorization, step-up checks, reason/case binding, idempotency, audit, and redaction coverage for new privileged workflows.
- Requires threat-model review because it adds operator capabilities and new operational data views.
