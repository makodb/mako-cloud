# Release gates

The machine-readable source is
[`release-gates.json`](release-gates.json), validated by
`npm run validate:release-gates`. A component or storage qualification does not
by itself authorize customer traffic: each stage is a separate decision for an
exact image, configuration, region, storage class, runtime, workload, and cost
measurement window.

## Current decisions

| Stage | Decision | Reason |
| --- | --- | --- |
| Internal | Pass for the recorded qualification host | Durability, recovery, security, capacity, component latency, and zero incremental cash spend on the existing host meet the internal thresholds. |
| Single-region beta | Blocked | The exact VM storage and recovery/rollback drills pass, but trusted HTTPS, successful end-to-end load/saturation, and the 30-day capacity and provider/facility cost window are incomplete. |
| Multi-region | Unsupported | One local RocksDB database has one owner and no cross-region durability or automatic failover. |

The internal cost observation is deliberately narrow: the qualification used
already-owned host capacity and incurred zero incremental cash spend. It does
not estimate allocated hardware, staffing, bandwidth, backup-provider, or cloud
cost and cannot satisfy a beta gate.

## Internal gate

Internal environments require zero acknowledged-write loss and integrity
failure, at least 25 complete storage-soak cycles, all five security
qualification families, a verified backup no older than 15 minutes, backup
recovery-point exposure no greater than 15 minutes, same-volume recovery within
5 minutes, replacement restore within 30 minutes, and free capacity above the
2 GiB warning reserve.

The retained observations pass those thresholds: 25 cycles, 2,375 storage and
document executions, no loss or integrity failures, 50-second backup age, zero
checkpoint high-water exposure, 1.43-second same-volume recovery, 1.72-second
replacement restore, and 1.706 TB free. All 11 component performance paths pass
their checked budgets. The real pinned edge-runtime image and the auth, policy,
RxDB, and tenant-boundary suites pass.

## Single-region beta gate

The beta retains the internal zero-loss and security requirements and adds:

- the same 15-minute backup/RPO and 5/30-minute restart/restore objectives on
  the actual encrypted retained RWO volume;
- at least 30% capacity headroom at the qualified traffic target;
- end-to-end p95 budgets of 200 ms for auth, 250 ms for RxDB pull/push/live and
  warm functions, and 1.5 seconds for a cold function;
- fixed infrastructure cost no greater than USD 2,500/month and replication
  cost no greater than USD 50 per million operations at the beta workload;
- a 30-day measurement window using metering plus provider invoice data, with
  backups, logs, egress, retained volumes, and idle capacity included;
- an operator-observed backup, restore, promotion, service/configuration
  rollback, and alert-response drill.

Null observations fail the gate. Local component measurements may guide sizing
but cannot substitute for hosted HTTPS, storage-class, saturation, and invoice
measurements.

## Multi-region gate

Multi-region remains unsupported regardless of stateless edge routing. Before
it can be evaluated, a separate OpenSpec change must provide and qualify a
replicated durability design. The eventual gate requires zero acknowledged loss
and integrity failures, region-loss RPO within 15 minutes, recovery within 30
minutes, cross-region p95 within 500 ms, fixed infrastructure at or below USD
7,500/month, and replication cost at or below USD 100 per million operations.
Those are ceilings for a future design, not claims about current capability.

## Evidence and procedure

1. Build immutable artifacts and run CI formatting, lint, unit, integration,
   dependency, API-generation, and documentation checks.
2. Run the auth, policy, RxDB chaos, tenant-boundary, real-container edge, and
   production RocksDB qualification suites.
3. Retain machine-readable performance, storage, capacity, recovery, and cost
   observations for the exact candidate environment.
4. Run `npm run validate:release-gates`. Do not manually change a stage to pass
   while any required observation is null or any blocker remains.
5. Record the approver, candidate digest, measurement window, and exception-free
   result in the deployment system before enabling traffic.

For the current VM, the plan hash, release/runtime/dependency/artifact digests,
VM identity, storage layout, recovery drills, observability state, admission
stop, and teardown inventory are retained in `docs/evidence/public-beta-*.json`.
The gate validator must bind observations to VM `124`, address
`130.245.173.11`, origin `https://cloud-test.makodb.com`, and the exact selected
release. A successful deploy, private readiness, or a short benchmark does not
fill a missing 30-day or provider-cost observation.

The measurement window begins only when its machine-readable start record names
the exact candidate and plan. It must cover 30 complete days and record resource
use, capacity, backups, availability, workload, egress, allocated host, power,
network, storage, and operational cost. Provider or facility evidence is
attached where available; facts that cannot be proven remain `null`, which
keeps the qualified-beta gate blocked. `risk_accepted_preview` never fills those
facts, changes a threshold, or marks that gate passed. It may expose only the
documented waivable blockers under an exact digest-bound approval lasting at
the operator's discretion without a calendar expiry, while every non-waivable
safeguard passes. Manual pause, release or blocker drift, or a safeguard failure
falls back to `pre_gate`; drift requires a new exact acceptance.
