# Operator control center

The hosted console serves the control center as the canonical operator workspace.
The staged HTML gate and `/operator/legacy` page were retired after contextual
workflow parity, hosted usage, audit, deep-link, and release rollback qualification.
An unknown or retired operator route fails closed with `404`.

Offline deployment-snapshot backfill and shadow validation use
`mako-operator-projection`. The command accepts only an explicit offline
RocksDB snapshot, pages every scan, writes only to that snapshot copy, and emits
source/projection counts and checksums. The latest retained result is
[operator-control-center-projection-qualification.json](evidence/operator-control-center-projection-qualification.json).

The control center at `/operator` is a metadata-only, audited workspace for platform operators. It composes bounded control-plane records with bounded telemetry summaries. It does not grant routine access to customer document bodies, arbitrary Prometheus queries, infrastructure credentials, physical backup paths, or arbitrary restore commands.

## Routes and freshness

The OpenAPI contract in `api/openapi/mako-cloud-v1.yaml` defines overview, tenant directory, Tenant 360, inventory, incident, recovery, activity, export, projection, security, provisioning, quota, abuse, and support-session resources. Inventories use opaque cursors, a maximum page size of 100, stable key ordering, bounded time windows, explicit observation times, and `current`, `stale`, `unknown`, or `unavailable` source state. A provider failure affects only its section and never implies a healthy state.

Diagnostic links are emitted only for configured HTTPS origins in `MAKO_OPERATOR_DIAGNOSTIC_ORIGINS`. The service removes links outside that allowlist and rejects response fields associated with secrets, tokens, passwords, documents, selectors, keys, values, or raw email addresses.

## Permission and role inventory

Read permissions are `overview_read`, `tenant_read`, `operations_read`, `incident_read`, `backup_read`, `fleet_read`, `security_read`, and `activity_read`. Mutation permissions are `incident_manage`, `recovery_manage`, `security_manage`, and `activity_export`, alongside the existing `provisioning_repair`, `quota_override`, `abuse_response`, `support_access`, and `waitlist_review` permissions.

The observer role is read-only. The responder role adds incident management. The security-administrator role manages operator security, the recovery-administrator role manages recovery, and the administrator role contains the complete bundle. Existing `tenant_read` entitlements retain compatibility access to the new read-only views, but never gain a new mutation. The last entitlement holding `security_manage` cannot remove that permission.

The browser hides destinations that are not useful for the current entitlement; every API remains server-authoritative. Operator, developer, and application-user identities are separate. Document-content routes stay outside this workspace and require a separately verified, exact-scope, expiring support session.

## Guarded workflows

New high-impact writes contain an operation key, reviewed resource version, reason, optional case reference, confirmation, and an action binding. Password verification must be no older than five minutes. The action binding is SHA-256 over the action, target, and reviewed version, so proof for one operation or stale page cannot authorize another. Same-origin checks and the operator session's password step-up are also enforced at HTTP entry.

Recovery creation and promotion are independently disabled by default. Enable them only after qualification with `MAKO_OPERATOR_RECOVERY_CREATE_ENABLED=true` and `MAKO_OPERATOR_RECOVERY_PROMOTE_ENABLED=true`. Recovery jobs accept a verified backup reference and protected logical target, use an explicit state machine, and call only typed executor methods. They do not accept shell commands or physical paths.

Activity exports require `activity_export`, preserve only a filter digest and integrity metadata, expire after one hour, and contain no document bodies or secrets. Projection rebuilds are idempotent and record source count, projected count, and checksum evidence.

## Rollback

The legacy single-page route is no longer a runtime rollback mechanism. If the
control center is unsafe or misleading, stop public admission, take and verify a
checkpoint, and use the immutable release rollback procedure to select the last
compatible release. The rollback release contains the prior console bundle;
existing operator APIs remain available, and additive incident, recovery,
activity, index, and workflow records must be retained. Re-enable admission only
after operator authentication, exact-project provisioning, quota, abuse,
support-session, wait-list, audit, and deep-link checks pass on the selected
release. The hosted retirement evidence is retained in
`docs/evidence/operator-control-center-legacy-retirement.json`.

Qualification must cover OpenAPI generation, Rust and console tests, threat-model tests, observability validation, permission-specific navigation, cross-tenant denials, stale step-up, provider outages, sensitive-URL checks, and production-like HTTPS routing. Store shadow rebuild count/checksum evidence in `docs/evidence/` before enabling mutation gates.
