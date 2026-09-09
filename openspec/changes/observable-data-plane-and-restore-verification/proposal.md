## Why

Fifteen of the forty-six checked alert rules watch metrics that no process publishes, and ten of them are critical. Nothing in the Rust workspace emits a single `mako_storage_*` series, and only the control plane serves a `/metrics` route at all, so the data plane, edge gateway, and telemetry-query are entirely unobserved. Prometheus does not report this: a threshold over an absent series simply never fires, so tenant isolation violations, audit write failures, RocksDB corruption, I/O errors, sequencer gaps, auth refresh replay, and edge sandbox incidents would all pass unnoticed on a deployment now carrying real users. The single rule that fails the other way, `MakoRecoveryEvidenceMissing`, uses `unless on()` and so fires forever, and it has been paging every four hours for weeks while its nine silent siblings sat green.

## What Changes

- Add a bounded `/metrics` route to `mako-data-plane`, `mako-edge-gateway`, and `mako-telemetry-query`, following the shape and label-cardinality discipline of the existing control-plane exporter. Between them they publish the seventeen series the checked alert rules name.
- Add a recurring offline restore verification that publishes `mako_storage_restore_verification_success`. It restores the newest published backup into an empty offline target, verifies the manifest, file digests, tenant keyspace boundaries, and acknowledged commit high water, reports the result, then reaps the target. It never promotes and never touches a live copy.
- Fix `mako_control_sqlite_migration_verified`, which is stuck at `0` by construction rather than reporting anything. The backup orchestrator unit strips `CapabilityBoundingSet` and `AmbientCapabilities`, so it runs without `CAP_DAC_READ_SEARCH` and cannot traverse the `drwx------` directory the migration receipt sits in. The receipt is present; the probe has never once seen it.
- Teach the Prometheus scrape configuration and the Ansible observability role about the new endpoints.
- Retire entries from the `UNPRODUCED` map in `scripts/validate-alert-metric-producers.js` as each producer lands. That gate already fails when a tracked rule becomes live, so the list cannot silently go stale.

No alert is weakened, retargeted at a weaker series, or deleted to match the data that happens to exist. The rules describe the failures worth catching; the producers are what is missing.

## Capabilities

### New Capabilities

- `operations/service-telemetry`: what every deployable service must expose for operations to see it. The bounded metrics endpoint and its authorization and cardinality rules, the requirement that a published alert rule may only watch a series something actually publishes, and the requirement that absent telemetry is itself detectable rather than silent.

### Modified Capabilities

- `storage/production-rocksdb`: the existing requirement that storage health is observable gains scenarios that make it falsifiable. Today it lists the series production "SHALL expose" and nothing checks that any of them exist. It must also state that these are exposed by the service that owns the data, not only named in a rule file.
- `operations/public-beta-environment`: restore verification becomes a recurring, published signal rather than a one-off operator drill. The current requirement demands verified restore to an empty offline target and makes verified backup and recovery a non-waivable public-preview safeguard, yet the only evidence is a single drill from 2026-08-13. Promotion stays operator-controlled and manual, which is unchanged.

## Impact

- **Services**: `services/mako-data-plane`, `services/mako-edge-gateway`, `services/mako-telemetry-query` gain a route module and an exporter; `crates/mako-storage`, `crates/mako-sync`, `crates/mako-documents`, `crates/mako-audit`, `crates/mako-gateway`, and `crates/mako-edge-runtime` gain the counters and gauges those routes read. No new dependency, no async runtime, and the transport is the existing bounded one.
- **Deployment**: `infra/local/prometheus.yml` and the Ansible `observability` role learn the new scrape targets; the `backup` role gains the recurring verification unit and timer, and the orchestrator unit's sandbox is corrected for the receipt probe. The full public-beta playbook is not safely runnable, so this must converge through a scoped playbook, as the alerting change did.
- **Gates**: `scripts/validate-alert-metric-producers.js` shrinks as producers land. `docs/observability.md` currently documents the gap and will need to describe what is published instead. `docs/requirements-traceability.md` gains rows for the new capability's scenarios.
- **Operators**: `MakoRecoveryEvidenceMissing` stops being a permanent page, and nine critical rules become capable of firing for the first time. Expect previously invisible conditions to surface once the producers land.
