## 1. The receipt probe, which is the smallest whole fix

- [ ] 1.1 Move the migration-receipt check out of the backup orchestrator, which cannot traverse the control plane's private directory, and into the privileged component that owns that directory, following the status-file pattern every other value in that exporter already uses
- [ ] 1.2 Make the gauge omit itself when the state cannot be gathered, rather than publishing `0`, and add the check that catches the difference
- [ ] 1.3 Converge the backup role with a scoped playbook, dry run first, and confirm the gauge reports the receipt that has been present since 2026-08-13

## 2. Recurring restore verification

- [ ] 2.1 Add a verification runner that resolves the newest published backup itself, restores into an empty offline target, verifies manifest, file digests, tenant keyspace boundaries and acknowledged commit high water, then reaps the target
- [ ] 2.2 Make it take the same transfer lock the backup orchestration holds, so it can never run against a checkpoint mid-write, and assert it never promotes
- [ ] 2.3 Publish the outcome and the time it last succeeded as `mako_storage_restore_verification_success` and a companion age series
- [ ] 2.4 Install the service and timer in the backup role next to the existing checkpoint timer, and converge with a scoped playbook
- [ ] 2.5 Remove `MakoRocksDbRestoreFailed` and `MakoRecoveryEvidenceMissing` from the tracked gaps and confirm the producer gate now demands their removal rather than allowing it

## 3. Data plane exporter

- [ ] 3.1 Add the bounded `/metrics` route to `mako-data-plane`, mirroring the control-plane exporter's shape: no query, no payload, one `block_on` at the route boundary, closed-domain labels only
- [ ] 3.2 Add the storage signals the alert rules name: open state, lock held, write stopped, pending compaction bytes, I/O errors, corruption signals, recovery duration, storage contract readiness
- [ ] 3.3 Add the sequencer gap age series
- [ ] 3.4 Add the tenant isolation violation counter, reporting that a violation occurred and its class and never which tenant
- [ ] 3.5 Add the audit write failure counter and the policy evaluation counter
- [ ] 3.6 Assert the endpoint is refused over the public route, and that a value the service cannot determine is omitted rather than defaulted
- [ ] 3.7 Remove the corresponding tracked gaps and confirm the gate holds

## 4. Edge gateway and telemetry-query exporters

- [ ] 4.1 Add the bounded `/metrics` route to `mako-edge-gateway` with the auth refresh replay counter and the function sandbox incident counter
- [ ] 4.2 Add the bounded `/metrics` route to `mako-telemetry-query`
- [ ] 4.3 Add the shared HTTP transport series the latency and error-rate rules name, published by every service that serves the transport
- [ ] 4.4 Remove the remaining tracked gaps so the producer gate reports zero

## 5. Collection and absence detection

- [ ] 5.1 Add the new scrape targets to `infra/local/prometheus.yml` and to the deployment's Prometheus configuration, bound to loopback
- [ ] 5.2 Add the alert that fires when a collection target stops answering, so an unobserved service is distinguishable from a healthy quiet one
- [ ] 5.3 Converge the observability role with a scoped playbook and confirm every target reports up

## 6. Close the loop

- [ ] 6.1 Rewrite the observability section of `docs/dev-book.md` that currently documents the gap, so it describes what is published
- [ ] 6.2 Add traceability rows for the new capability's scenarios and cite the tests that cover them
- [ ] 6.3 Watch each newly live critical rule for a full alert interval before declaring the change done, and record what each reads in steady state
- [ ] 6.4 Confirm `validate:alert-metric-producers` reports every rule live and the tracked-gap list empty
