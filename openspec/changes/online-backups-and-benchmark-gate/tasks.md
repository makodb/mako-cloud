## 1. Online checkpoint primitive

- [x] 1.1 Split `ProductionRocksDb::create_backup` into an in-process `create_checkpoint`, which returns the checkpoint path, barrier wait and creation time, and a worker-side `finalize_backup` that copies the checkpoint to staging, deletes the linked checkpoint, then runs the existing recovery, inventory, manifest, signing and publish steps. Keep manifest version 1.
- [x] 1.2 Add the commit-group barrier: a blocking `Mutex`/`Condvar` gate owned by the storage owner, with no async runtime. Sync push enters it from `lease` through `recover_high_water` with an owned `Send` token, and `create_checkpoint` closes it and drains open groups within a configured bound (default 2 s). Abandon on timeout without further blocking writes.
- [x] 1.3 Audit every writer that leases sequence positions across more than one adapter write, and put each under the barrier. Done by construction: the commit-group token lives in `SequenceLease`, so every caller of `EnvironmentSequencer::lease` (sync push, single-document mutations, explorer writes) holds it until its lease is dropped.
- [x] 1.4 Refuse a checkpoint directory on a different device from the live database before taking the barrier.
- [x] 1.5 Allow one checkpoint at a time, and sweep leftover `checkpoints/*` before each checkpoint and at owner startup.

## 2. Private interface and worker

- [x] 2.1 Add a `BackupWorker` caller and `POST /_internal/v1/data/storage/checkpoint` to the closed internal RPC allowlist. Serve it on the data plane's loopback listener only, signed with the deployment internal-auth key, with skew and replay checks.
- [x] 2.2 Add a `mako-storage-ops` entry point that requests the checkpoint, runs `finalize_backup`, and never opens the live database.
- [x] 2.3 Export the backup duration, barrier wait and checkpoint creation time. `mako-online-backup` prints `mode=online barrier_wait_ms checkpoint_ms duration_ms` on its result line; a Prometheus series for them is left to `observable-data-plane-and-restore-verification`.

## 3. Layout, orchestration and release operations

- [x] 3.1 Keep the checkpoint on the live database's filesystem. Implemented as `<database>/.online-checkpoints` instead of moving the live path to `live/` (see design), so no data migration is needed; the backup role creates the directory and a data-plane worker drop-in grants it, the internal-auth credential and loopback.
- [x] 3.2 Remove the data-plane `systemctl stop`/`start` from `mako-checkpoint-backup-orchestrate` and call the new worker entry point. Keep the shared transfer lock.
- [x] 3.3 Change `mako-release-operation` so upgrade and rollback record the backup timers' state, stop the timers and any running backup, switch, verify readiness, and then restore the recorded state. On failure, leave the timers stopped. Fail closed on unknown state. Record `checkpointPolicy: "paused-for-deployments"`.
- [ ] 3.4 Alert on backup failure, backup age, and growth of `.online-checkpoints` and the staging roots.

## 4. Backup qualification

- [x] 4.1 Test checkpoints taken during concurrent writes, commit-group waits, barrier timeout, cross-device refusal, an interrupted backup with sweep, the database ignoring its checkpoint directory, finalize refusing another database's marker, the signed route (success, replay, wrong scope, local storage), release operations with timers enabled, and restore of an online backup proving tenant boundaries and acknowledged high water.
- [ ] 4.2 On the beta VM, re-enable backups (`mako_backup_enabled: true`) and run them every five minutes for at least 24 hours. Require zero readiness failures, and record write-pause p50, p95 and maximum.
- [ ] 4.3 Run the recovery drill from an online backup taken under concurrent pushes, and record it as the `backupAndRecovery` evidence.

## 5. Benchmark gate

- [x] 5.1 Remove the measurement-window, backup-age, recovery-point and cost thresholds and observations from the `single-region-beta` stage in `docs/release-gates.json`. Leave the internal gate unchanged.
- [x] 5.2 Change `scripts/validate-release-gates.js` to allow `passed` when every benchmark threshold holds and the retained qualifications are valid, to limit `blocked` to failing or unmeasured benchmark paths and alert-delivery blockers, and to drop the window and cost assertions.
- [x] 5.3 Change the hosted benchmark harness to collect at least 100 steady-state samples for each gated path, measure pull in steady state, run with the backup schedule enabled across at least two completed online backups, and record per-path sample counts. Report recovery timings separately.
- [x] 5.4 In the admission guard, remove the backup-verification and backup-age checks, `backupFreshnessRequired`, and the 30-day and cost waivable patterns. Remove `mako_public_preview_require_fresh_backup` from group vars and from the context template.
- [x] 5.5 Update `scripts/public-beta-preview-approval-lib.js` and the guard, approval and validator tests for the new blocker set, including a case proving that a failed backup leaves admission unchanged.

## 6. Cutover and documentation

- [ ] 6.1 Run the hosted benchmark on the deployed release with backups enabled, and record the gate result.
- [ ] 6.2 In the convergence that installs the new guard and context, install either a beta approval (if the gate passed) or a new preview approval for the new blocker set. Verify that the next guard tick keeps public admission open.
- [x] 6.3 Update the dev-book: the beta gate, the backup procedure and timings, release operations, the admission runbooks, and the removal of the test-preview suspension exception.

## 7. Coordination and validation

- [ ] 7.1 Reconcile with `observable-data-plane-and-restore-verification`, which also edits the orchestrator and the requirement *Backups and rollback are exercised off the live paths*. Whichever change archives second rebases onto the other and must not reintroduce the backup-age objective.
- [ ] 7.2 Reconcile with the unmerged `incremental-storage-accounting` work in `rocks.rs`. A restored database still restarts accounting reconciliation.
- [x] 7.3 Run `openspec validate online-backups-and-benchmark-gate --strict`, the Rust workspace tests, and the infra and gate validators.
