## Why

The single-region beta gate cannot pass as written, so the only way to serve
the public is a risk-accepted preview, and that preview closes for good on any
failed safeguard. It last closed on 2026-10-26, after one data-plane readiness
failure, and stayed closed for sixteen days until a new approval was issued.

Three of the gate's four outstanding requirements are not engineering problems
that a fix can close:

- **The 30-day measurement window** is bound to one exact release, and any
  deploy restarts it. Shipping fixes and completing the window are mutually
  exclusive.
- **The 15-minute backup-age objective** needs data-plane backups every five
  minutes. Every data-plane backup stops the data plane for the whole run, and
  the data-plane database is now 47 GB, so a full-copy backup takes far longer
  than the schedule allows. Scheduled backups have been off since 2026-09-25.
- **Cost thresholds** require provider invoices or facility evidence that the
  owned lab hardware does not produce, and the validator requires them to stay
  null until then, which blocks the gate permanently.

The remaining requirement, the hosted benchmark (latency, capacity, zero
acknowledged-write loss, zero integrity failures), is real engineering work. It
is also the one measurement that says whether the service is fit to serve.

## What Changes

- **The beta gate becomes the hosted benchmark.** The latency, capacity, loss
  and integrity thresholds are unchanged. Each gated latency needs at least 100
  steady-state samples. The validator gains a `passed` result.
- **Removed from the gate:** the 30-day window, the backup-age and
  recovery-point objectives, and the cost thresholds. They may still be recorded
  as informational evidence.
- **Backups never open or close public admission.** The preview guard drops its
  backup-age and backup-verification checks and the `backupFreshnessRequired`
  setting. It also drops the 30-day and cost entries from its list of blockers
  an operator may waive. Backup failure and age raise alerts instead.
- **Removed requirement:** *Approved test preview can suspend scheduled
  backups*. The exception existed only because backup freshness gated
  admission, which it no longer does.

Data-plane backups are unchanged: when enabled they stop the data plane, and on
the beta they stay disabled. An earlier version of this change also took
data-plane backups online; that part was withdrawn.

## Capabilities

### New Capabilities

### Modified Capabilities

- `operations/public-beta-environment`: the beta gate is decided by the hosted
  benchmark. Backup state never changes admission. The test-preview backup
  suspension exception is removed.

## Impact

- **Gate:** `docs/release-gates.json`, `scripts/validate-release-gates.js`,
  `scripts/public-beta-benchmark-gate-lib.js`,
  `scripts/public-beta-preview-approval-lib.js`, the hosted benchmark harness,
  `scripts/validate-rollback-qualification.js`, and their tests.
- **Infra:** the admission guard, the preview context template, and
  `group_vars/public_beta.yml`.
- **Docs:** the dev-book's admission, backup runbook and release-gate chapters.
- **Cutover:** changing the blocker set changes the blocker digest. The
  installed preview approval must be replaced in the same step that installs the
  new guard and context, or admission closes.

**What is given up:**

- **No enforced recovery point.** A backup that fails or never runs no longer
  closes anything; only an alert says so.
- **No sustained-operation evidence before beta.** A slow leak like the
  2026-09-19 staging leak would be caught by monitoring rather than by a gate.
- **No cost ceiling.**

These are acceptable for a beta that charges nobody and carries no production
tenants. They should be revisited before general availability.
