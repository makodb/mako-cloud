## Context

**What the gate validates today**

- `scripts/validate-release-gates.js` requires `single-region-beta` to be
  `blocked` with exactly two blockers, a latency blocker and a 30-day blocker.
  It has no path to `passed`.
- The preview guard checks backup verification and, unless
  `backupFreshnessRequired` is false, backup age of 900 seconds or less.
- The guard's list of blockers an operator may waive includes the 30-day and
  cost patterns.

**Why backups cannot carry a gate here.** Each data-plane backup opens the
RocksDB database itself, so the orchestrator stops the data plane for the whole
run. The run copies and hashes the checkpoint several times, then transfers it
off the VM and reads it back. At 47 GB that cannot fit a five-minute schedule,
and 14 retained full copies would not fit the 256 GB data disk.

## Goals / Non-Goals

**Goals**

- The beta gate can pass, and it is decided by the hosted benchmark.
- Backup state never opens or closes public admission.

**Non-Goals**

- Changing any latency, capacity, loss or integrity threshold.
- Changing preview stickiness, the readiness fallback, or how `approved_beta`
  is selected.
- Changing how data-plane backups run, or enabling them. An online-backup
  design was implemented and then withdrawn: the database does not need online
  backups, and full copies of it are impractical on any schedule.

## Decisions

### The beta gate is the hosted benchmark

- **What counts:** a release passes when the hosted benchmark on that release
  meets every threshold. The thresholds are:
  - p95 latency for auth, pull, push, live delivery, warm function and cold
    function;
  - capacity headroom at target load;
  - no failed requests at target and headroom concurrency;
  - zero acknowledged-write loss;
  - zero integrity failures.
- **What else must stay valid:** the one-time qualifications that already
  pass, meaning trusted HTTPS and HSTS, the route allowlist, the security
  suites, the recovery drill, and registration and mail. Those are the
  non-waivable safeguards the preview already depends on.
- **Statistics:** each gated p95 comes from at least 100 steady-state samples.
  The October evidence had 10 sign-ins, 4 push batches and a single pull, which
  is not a p95. Pull is measured in steady state. Recovery-to-ready and
  pull-after-recovery are still recorded, but they belong to the recovery drill
  rather than the latency gate.
- **What is no longer gated:**
  - `release-gates.json` loses the beta thresholds and observations for the
    measurement window, backup age, recovery-point exposure and cost.
  - The internal gate is unchanged.
  - The measurement-window evidence file may keep being recorded as
    informational.
- **Validator:** `scripts/public-beta-benchmark-gate-lib.js` computes the
  result and the exact blocker list from the benchmark evidence, and both the
  validator and the approval library use it. `passed` is allowed when every
  threshold holds with no blockers. `blocked` must list only failing or
  unmeasured benchmark paths, or alert-delivery blockers, and a recorded status
  or blocker list that disagrees with the computation is rejected.

### Backup state never moves admission

The guard drops these checks:

- `mako_storage_backup_remote_verified`;
- the backup-age check;
- `backupFreshnessRequired`, along with its context field and the
  `mako_public_preview_require_fresh_backup` group variable.

The waivable-blocker patterns drop the 30-day and cost entries, so an approval
naming them no longer validates. The `backupAndRecovery` safeguard keeps its
name, because it appears in approval records; it is satisfied by the passing
recovery drill. Backup failure and backup age become alerts only.

This matters in practice. On 2026-11-11 a failed manual backup set the
verification metric to 0, and the previous guard closed public admission
within a minute.

## Risks / Trade-offs

- **No enforced recovery point, no sustained-operation window, no cost
  ceiling.** These are accepted in the proposal for a beta that charges nobody.
  Reintroducing any of them later should be a gate condition that does not
  depend on deploy frequency.
- **Blocker text carries measured values**, so each new benchmark run changes
  the blocker digest. The convergence that installs a new context must install
  a matching approval in the same step.

## Migration Plan

1. Change the gate, validator, guard and approval library.
2. In one step on the VM, with the guard timer stopped, install the new guard,
   the context with the new blocker digest, and the matching preview approval,
   then verify and activate.
3. Run the hosted benchmark on the deployed release and record the gate result.

## Open Questions

- **The auth budget.** Auth latency is measured on password sign-in, which
  includes deliberate Argon2id cost. The 200 ms budget may be intended for
  token refresh or verification instead. Thresholds are unchanged here.
