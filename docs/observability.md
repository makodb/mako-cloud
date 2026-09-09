# Operational dashboards

The local stack provisions read-only Grafana dashboards in the **Mako Cloud** folder. Open `http://127.0.0.1:3000` after starting `infra/local/compose.yaml`; no manual import is required.

| Dashboard | Primary operator question |
| --- | --- |
| Service Health | Are services ready, and is request latency healthy by service and region? |
| Saturation | Are concurrency slots or work queues approaching capacity? |
| Replication Lag | How far are RxDB clients behind committed high water? |
| Error Rates | Which service and safe error class is failing? |
| Sequencer Gaps | Are unresolved commit positions blocking visibility? |
| Revocation Freshness | Can each gateway prove its revocation cache is fresh? |
| Live Streams | Are SSE streams connected, buffered safely, or forcing resynchronization? |
| Index Builds | Which index versions are progressing or failing? |
| Function Workers | Are isolates saturated, recycling, or failing invocations? |
| Mako Production RocksDB | Is the owned database ready, locked, durable, within capacity, backed up, and recovering within objective? |
| Control-plane SQLite | Is control authority ready and intact, and are transactions, WAL, capacity, migration, backup, and restore healthy? |

The operator control center links these existing dashboards as bounded,
allowlisted diagnostics rather than embedding arbitrary queries in the
browser. Its own aggregate request, failure, unavailable-source, and total
latency counters are guarded by the `mako-operator-control-center` Prometheus
rules and the [control-center runbook](runbooks/operator-control-center.md).

## Metric contract

Services export OTLP metrics to the collector, which exposes Prometheus-compatible series on port 9464. Tenant-scoped series use `project_id` and `environment_id`; regional service series use `service` and `region`. Bounded domain labels such as `collection_id`, `index_name`, `outcome`, `reason`, and `error_class` are used only where the dashboard needs them.

Actor, request, trace, session, document, raw URL, email, token, and secret values must never become metric labels. Request and trace identifiers belong in structured logs and traces, where the shared redactor and access controls apply. Producers must preserve the exact `mako_*` metric names referenced by the provisioned dashboard queries or update the dashboard and producer together.

Run `npm run validate:observability` to validate the dashboard inventory, JSON shape, data source binding, provisioning mount, and forbidden high-cardinality labels.

Production storage is specified to export bounded service/volume gauges and counters for open/readiness and lock state, available and threshold bytes, write stops and delayed rate, pending/running compaction and flush work, background/I/O/corruption errors, backup result and age, restore verification, and recovery duration. Tenant identifiers, keys, values, and signing material are excluded.

**None of those `mako_storage_*` series is published yet.** Only the control plane serves a `/metrics` endpoint; the data plane, edge gateway, and telemetry-query expose none, and the deployment's textfile collectors publish backup and health series only. Fifteen of the checked alert rules therefore watch metrics with no producer, ten of them critical, covering tenant isolation, audit write failure, corruption, sequencer gaps, and restore verification. A Prometheus expression over an absent series does not error: a threshold comparison simply never fires, and an `unless on()` absence check fires forever. Both are silent to a reader of the rule file.

Run `npm run validate:alert-metrics` to check every alert expression against the metric names the workspace and the deployment actually publish. Rules with no producer are listed in that script's `UNPRODUCED` map with a reason. The list may only shrink: the gate fails if an entry starts being published, if it names a rule that no longer exists, or if a new untracked rule watches an absent metric. Delete an entry when its producer lands, and never add one to make the gate pass.

Control SQLite separately exports `mako_control_sqlite_*` readiness, integrity,
schema, file/WAL size, free-space thresholds, active/oldest transaction, busy,
checkpoint, migration, backup, restore, and restart signals. These series do not
contain paths, keys, values, emails, credentials, or customer identifiers.

Prometheus also loads the checked release-blocking alert inventory from `infra/local/prometheus-rules`. Every rule links to an operator procedure in the [runbook index](runbooks/README.md); the same validation command checks that inventory and those links.

## Project logs

A function's printed output is collected off the request path: the control
plane reads each deployed function's runtime-supervisor buffer on a short
cadence and carries new lines into the retained telemetry store, where they
are served by the project logs endpoint with the same retention and tenant
scoping as every other signal. The supervisor's own buffer is bounded and
in-memory; the telemetry copy is the durable one.

Because log text is written by customer code, it is scrubbed before storage
— at the store itself, so no producer can bypass it. The scrub masks
configured secrets, bearer and JWT values, password and cookie assignments,
platform credential formats, and email addresses (the local part is masked,
the domain kept). It is best-effort by design: it removes token-, password-,
and email-shaped text, not every possible secret, and applications should
still avoid printing sensitive values.
