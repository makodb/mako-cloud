## Context

See proposal.md for why. The constraints that shape the approach:

The control plane already has the pattern to copy. `add_developer_metrics_route` registers `GET /metrics` on the shared router, refuses a query string and a payload, pulls a health snapshot from each store it reports on, and renders a bounded text body. It uses one `block_on` at the route boundary and nothing deeper, which is the rule everywhere in this workspace, and it publishes about a hundred and thirty series without a single unbounded label.

The services that need this have no exporter at all, and their metrics are not merely unregistered. The counters do not exist: no crate increments an I/O error, a corruption signal, a tenant isolation violation, or an audit write failure today. This is not wiring a route to existing state; most of the state has to be created alongside it.

The alert rules are the specification of what to publish. Seventeen series are named across the fifteen dead rules, and those names are already load-bearing in the Grafana dashboards. They are the contract, not a starting point for renaming.

Two deployment constraints. The full public-beta playbook cannot be run: the checked-in variables are a gate-closed baseline, so a converge without the live-only overrides stops and disables Caddy. No role in the playbook is tagged, so `--tags` cannot narrow it. Scoped playbooks are the established way around this, and the alerting change used one. Separately, `/etc/prometheus/prometheus.yml` currently scrapes one Mako target; the new endpoints have to be added there and bound to loopback like every other listener.

## Goals / Non-Goals

**Goals:**

- The seventeen series the alert inventory names are published by the service that owns the data each describes.
- Restore verification recurs, publishes its outcome and its age, and never promotes.
- A series that cannot be gathered is omitted rather than defaulted, so a probe that can never see its subject is reported as missing rather than as a failure.
- The tracked-gap list in the producer gate shrinks to empty as this lands.

**Non-Goals:**

- No new alert rules beyond what absent telemetry itself requires. The existing fifteen become capable of firing; that is the win.
- No OTLP push path for these services. The collector already scrapes; adding a second delivery mechanism is a separate decision.
- No histogram or high-cardinality request telemetry beyond the two series the latency and error-rate rules name.
- No automatic promotion of a verified restore, ever. The spec forbids it and the verification runs offline into a target it then reaps.

## Decisions

**Scrape, not push, and one route per service.** The control plane is scraped on loopback and the collector is already configured for that shape. Alternatives considered: emit through the OTLP collector that the local compose stack runs, which would centralise the exporters, but it adds a delivery dependency between a service and its own observability and it fails in exactly the case observability matters. A service that is degraded must still be able to say so.

**Publish from the owner of the data.** Each series is rendered by the service holding the database it describes, not gathered centrally. This is what makes `Every named storage signal has a producer` checkable, and it is why the migration-receipt gauge is broken today: a component was asked to report on state it structurally cannot read.

**Fix the receipt probe by moving it, not by widening the sandbox.** The orchestrator strips all capabilities and cannot traverse the control plane's private directory. Granting `CAP_DAC_READ_SEARCH` would fix the symptom and hand a root process the ability to read every file on the host, for one existence check. Every other value in that exporter already arrives through a status file written by the privileged component that owns the data, and the receipt probe is the lone exception reaching directly into a protected path. It moves to the same pattern.

**Omit rather than default.** A gauge that cannot be gathered publishes nothing. The current `0` is indistinguishable from a genuine failure, and had an alert been attached to it, it would have paged continuously for a condition that was never true. This is the fail-closed convention applied to telemetry: absence is visible, a false healthy value is not.

**Keep the gap list as the ratchet.** `validate:alert-metric-producers` already fails when a tracked rule becomes live. Entries are deleted as producers land, one per task, so the change cannot be declared done while a rule is still blind, and the gate does the bookkeeping instead of a checklist.

## Risks / Trade-offs

- **Nine critical rules become able to fire for the first time, on a deployment carrying real users.** → Land producers one subsystem at a time and watch each for a full alert interval before the next. Expect genuine findings; a first firing is information, not necessarily a regression. Do not tune a threshold on the first page without establishing what the metric actually reads in steady state.
- **New counters on hot paths cost something.** → The series are counters and gauges, not histograms, apart from the one latency bucket the existing rule requires. Read them from state the services already keep where possible rather than adding instrumentation to a per-document path.
- **A recurring restore verification is expensive and touches backup artifacts.** → It restores into an empty offline target and reaps it, never promotes, and holds the same transfer lock the backup orchestration already uses so it cannot run against a checkpoint being written. Daily is the right cadence; backup age is already covered by a faster alert.
- **The exporter could leak a tenant identifier through a label.** → Labels come from a closed domain, mirroring the control-plane exporter, and the observability validator's forbidden-label check covers the dashboards that consume them. A tenant isolation counter reports that a violation occurred and its class, never which tenant.
- **Converging the deployment is the risky step, not the code.** → Scoped playbooks only, dry run with `--diff` first, and confirm the change set is what was intended before applying, exactly as the alert-recipient change did.

## Migration Plan

Additive throughout: a new route on services that currently expose none, a new timer, and a corrected probe. Nothing changes an existing wire contract.

Order matters for the deployment steps. Land and verify the exporters locally, then add the scrape targets, then converge the observability role so the collector learns the endpoints, then converge the backup role for the verification timer and the receipt fix. Each converge is a scoped playbook with a dry run first.

Rollback is per-step and cheap. Removing a scrape target or a timer returns the deployment to today's posture, and the tracked-gap entry is restored alongside so the gate stays honest about what is blind.

## Open Questions

- The retention and cadence of the recurring restore verification for the data plane specifically. A RocksDB checkpoint restore is materially more expensive than the SQLite one, and whether it verifies daily or weekly should be settled against a measured run rather than assumed. It does not change the specs, the approach, or the task breakdown.
