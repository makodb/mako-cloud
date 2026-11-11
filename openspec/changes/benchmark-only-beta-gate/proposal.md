## Why

The single-region beta gate cannot pass as written, so the only way to serve
the public is a risk-accepted preview, and that preview closes for good on any
failed safeguard. It last closed on 2026-10-26, after one data-plane readiness
failure, and stayed closed for sixteen days until a new approval was issued.

Three of the gate's four outstanding requirements are not engineering problems
that a fix can close:

- **The 30-day measurement window** is bound to one exact release, and any
  deploy restarts it. The window started on 2026-10-12 on `b58934d8`; the VM
  has been upgraded several times since. Shipping fixes and completing the
  window are mutually exclusive.
- **The 15-minute backup-age objective** needs data-plane backups every five
  minutes. Every data-plane backup stops the data plane for the whole run:
  checkpoint, several hash passes, retention re-hashing, off-VM transfer and
  read-back (`mako-checkpoint-backup-orchestrate`: `systemctl stop
  "$application"`). Rational's client already documents the resulting "one blip
  in six minutes". Scheduled backups were suspended on 2026-09-25, so the
  objective cannot be met without a five-minute outage cycle.
- **Cost thresholds** require provider invoices or facility evidence that the
  owned lab hardware does not produce, and the validator requires them to stay
  null until then, which blocks the gate permanently.

The remaining requirement, the hosted benchmark (latency, capacity, zero
acknowledged-write loss, zero integrity failures), is real engineering work. It
is also the one measurement that says whether the service is fit to serve.

## What Changes

- **Online data-plane backups.** The data-plane process creates the RocksDB
  checkpoint itself, on the live database's filesystem so files are linked
  rather than copied, behind a barrier that waits briefly for in-flight sync
  pushes. The barrier has a bounded wait. An external worker copies the
  checkpoint and does everything else (sequencer recovery on the copy,
  inventory, signing, publishing, off-VM transfer, read-back) while the service
  keeps serving. The orchestrator no longer stops the data plane.
- **Release operations pause and restore the backup schedule.** They no longer
  require backups to be disabled, and they still never create a checkpoint.
- **The beta gate becomes the hosted benchmark.** The latency, capacity, loss
  and integrity thresholds are unchanged. Each gated latency needs at least 100
  steady-state samples, and the run must happen with online backups running.
  The validator gains a `passed` result.
- **Removed from the gate:** the 30-day window, the backup-age and
  recovery-point objectives, and the cost thresholds. They may still be recorded
  as informational evidence.
- **Backups never open or close public admission.** The preview guard drops its
  backup-age and backup-verification checks and the `backupFreshnessRequired`
  setting. It also drops the 30-day and cost entries from its list of blockers
  an operator may waive. Backup failure and age raise alerts instead.
- **Removed requirement:** *Approved test preview can suspend scheduled
  backups*. The exception existed only because backups were disruptive and
  gated admission; neither is true after this change.

## Capabilities

### New Capabilities

### Modified Capabilities

- `storage/production-rocksdb`: backups are taken online by the owning process,
  with a bounded write pause and no full copy under the write lock.
- `operations/public-beta-environment`: the beta gate is decided by the hosted
  benchmark. Backup state never changes admission. Release operations pause and
  restore backup schedules. The test-preview backup suspension exception is
  removed.

## Impact

- **Code:**
  - `crates/mako-storage`: split checkpoint creation from finalization, add a
    commit-group barrier, refuse cross-device checkpoints.
  - `crates/mako-sync`: push holds the commit-group guard.
  - `crates/mako-internal-rpc` and `services/mako-data-plane`: new loopback
    checkpoint route.
  - `mako-storage-ops`: new worker entry point.
- **Infra:**
  - The backup orchestrator and worker template.
  - The storage role: the data-plane live path moves to `live/`, with a
    sibling `checkpoints/` directory.
  - `mako-release-operation`, the admission guard, and the preview context
    template.
  - `group_vars/public_beta.yml`.
- **Gate:**
  - `docs/release-gates.json`, `scripts/validate-release-gates.js`,
    `scripts/public-beta-preview-approval-lib.js`.
  - The hosted benchmark harness, plus their tests.
- **Docs:** the dev-book gate, backup, release-operation and admission
  runbooks.
- **Cutover:** removing the 30-day blocker changes the blocker digest. The
  installed preview approval stops matching at the next convergence, and
  admission closes unless the same convergence installs either a new preview
  approval or a beta approval backed by a passing benchmark.

**What is given up:**

- **No enforced recovery point.** A backup that stalls no longer closes
  anything; only an alert says so.
- **No sustained-operation evidence before beta.** A slow leak like the
  2026-09-19 staging leak, which took about forty days to fill the data disk,
  would be caught by monitoring rather than by a gate.
- **No cost ceiling.**

These are acceptable for a beta that charges nobody and carries no production
tenants. They should be revisited before general availability.
