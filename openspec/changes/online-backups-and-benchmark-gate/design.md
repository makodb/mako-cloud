## Context

**How a data-plane backup works today**

- The worker runs `mako-storage-ops backup`.
  - It opens the live RocksDB database as a full read-write owner
    (`ProductionRocksDb::open`).
  - It creates a `rocksdb::checkpoint::Checkpoint` under
    `operation_lock.write()` (`crates/mako-storage/src/rocks.rs:149-170`).
  - It reopens the copy, recovers sequencers, inventories and hashes files, then
    writes a signed manifest. The manifest records identity, format, file
    digests and per-tenant acknowledged high water.
- Every adapter `write` and `compare_and_write` holds `operation_lock.read()`,
  so a checkpoint is atomic with respect to each mutation.
- RocksDB's exclusive `LOCK` prevents a second process from opening a running
  database. The orchestrator therefore stops the data plane for the whole worker
  run (`mako-checkpoint-backup-orchestrate:156`). That run covers the
  checkpoint, about five full hash passes, retention re-hashing of up to fifteen
  artifacts, rsync to the off-VM target and back, and remote inspection.
- The worker's timeout is ten minutes. No evidence records how long a real run
  takes.
- The control plane's SQLite database is already backed up online.
- Edge-gateway and telemetry-query have RocksDB databases but no backup timer.

**Three facts shape the design**

1. **Only the owning process can take a consistent checkpoint while it runs.**
   `create_backup` is already `pub`. A test checkpoints a database that is open
   and being written (`crates/mako-storage/tests/production_backup_restore.rs`).
2. **The staging root is a separate Btrfs subvolume from
   `rocksdb/data-plane`.** RocksDB checkpoints hard-link SST files and fall back
   to a full copy on `EXDEV`. An in-process checkpoint into today's staging root
   would copy the whole database while holding `operation_lock.write()`,
   stalling every write for the duration.
3. **A sync push is not one atomic write.** It leases N positions, commits rows
   one at a time, then aborts the remainder and recovers its high water
   (`crates/mako-sync/src/push.rs:61-93`).
   - A checkpoint taken between rows captures leased positions that are neither
     committed nor aborted.
   - Sequencer recovery stops at the first gap, so the manifest's high water is
     the contiguous prefix and nothing acknowledged is lost.
   - However, nothing in production classifies an orphaned gap. That tenant's
     high water would stay behind it after a restore.
   - A crash has the same exposure today; a backup should not add to it.

**What the gate validates today**

- `scripts/validate-release-gates.js` requires `single-region-beta` to be
  `blocked` with exactly two blockers, a latency blocker and a 30-day blocker.
  It has no path to `passed`.
- The preview guard checks backup verification and, unless
  `backupFreshnessRequired` is false, backup age of 900 seconds or less.
- The guard's list of blockers an operator may waive includes the 30-day and
  cost patterns.

## Goals / Non-Goals

**Goals**

- A data-plane backup never stops the data plane, fails readiness, or fails a
  request.
- Backups pause writes only for a short, bounded, measured interval. They never
  pause writes for a full copy.
- An online backup restores with the same proofs as an offline one: digests,
  tenant boundaries and acknowledged high water. It never contains a partially
  committed push.
- Release operations coexist with an enabled backup schedule.
- The beta gate can pass, and it is decided by the hosted benchmark.
- Backup state never opens or closes public admission.

**Non-Goals**

- Backups for edge-gateway or telemetry-query.
- Changing any latency, capacity, loss or integrity threshold.
- Changing preview stickiness, the readiness fallback, or how `approved_beta`
  is selected.
- Classifying orphaned sequencer gaps left by crashes. That exposure is
  unchanged and noted under Risks.
- Storage metrics endpoints and recurring restore verification. Those are
  `observable-data-plane-and-restore-verification`.
- Incremental storage accounting. Restored databases still restart
  reconciliation from scratch, as they do today.

## Decisions

### The owner takes the checkpoint; a worker finishes it

`create_backup` splits into two parts:

- **`create_checkpoint`**, run inside the data plane:
  1. Acquire the commit-group barrier.
  2. Take `operation_lock.write()`.
  3. Call `Checkpoint::create_checkpoint` into
     `.online-checkpoints/<backup-id>.checkpoint`.
  4. Release both locks.
  5. Return the barrier wait and creation durations.
- **`finalize_backup`**, run by `mako-storage-ops` as `mako-data-plane`:
  1. Copy the checkpoint into `backup-staging/data-plane/<id>.incomplete`.
  2. Delete the linked checkpoint, which releases the pinned SST files.
  3. Run today's existing steps on the staged copy: sequencer recovery,
     inventory, manifest, signing, publish, retention, transfer and read-back.

The manifest format stays at version 1, so restore, verify and promote do not
change. The worker never opens the live database.

Alternatives considered:

- **A RocksDB secondary or read-only instance.** It cannot coordinate with
  `operation_lock` or the push barrier, so it cannot produce a checkpoint
  consistent with them. It also needs catch-up semantics the codebase does not
  have.
- **A read-only Btrfs snapshot of the subvolume.** It is only crash-consistent,
  needs root, and still captures partial pushes.
- **Keep stopping the service but shorten the stop.** The stop would still have
  to cover checkpoint and fence. It is less work, but every backup would remain
  an outage.

### The checkpoint stays on the live database's filesystem

The data plane links each checkpoint into
`<database>/.online-checkpoints/<backup-id>.checkpoint`, a directory inside its
own database directory. That puts it on the database's filesystem by
construction, with no new configuration and no data migration.

- **Why inside the database directory is safe.** RocksDB only reads the files
  it names at the top of its directory; it already ignores the volume marker
  that sits there. A test proves the database reopens, keeps serving, and
  never copies the directory into an offline checkpoint.
- **The device check stays.** Before taking the barrier,
  `create_online_checkpoint` compares the device of the target's parent with
  that of the live database, so a filesystem mounted at that path would fail
  the backup instead of turning hard links into a full copy under the write
  lock.

The first draft moved the live database to `rocksdb/data-plane/live/` with a
sibling `checkpoints/`. That was rejected during implementation. The
data-plane unit's `ExecStartPre` provisions any empty database path, so if the
new path were configured before the data moved, the service would start on a
new, empty database. A live-data move on the production VM is not worth that
risk when a subdirectory gives the same guarantee.

### A commit-group barrier keeps partial pushes out of checkpoints

- **The barrier:** the storage owner holds a `CommitGroupBarrier` built from a
  `Mutex` and a `Condvar`. A push enters it before `lease` and holds an owned,
  `Send` token until `recover_high_water` returns; dropping the token leaves the
  group. The token is not a borrowed lock guard, so it can be held across the
  `await`s that `futures::executor::block_on` drives. Mako Cloud has no async
  runtime and this change does not add one.
- **The audit:** implementation must audit every other writer that leases
  sequence positions across more than one adapter write, and put each under the
  same guard. A single-batch document mutation does not need it.
- **The bounded wait:** `create_checkpoint` closes the barrier, so new commit
  groups wait at entry, then waits up to a configured bound (default 2 seconds)
  for the open groups to drain. New pushes queue behind a waiting checkpoint, so
  the wait itself is a write pause. That is why it is bounded.
- **On timeout:** the checkpoint is abandoned, writes resume, and the backup
  fails as retryable. The next schedule tries again. Repeated failures surface
  as backup-age alerts, not as an outage.

`operation_lock` remains a `std::sync::RwLock` for single writes. The barrier
is a separate primitive because a commit group spans several writes and
`await`s.

### The checkpoint is requested over the signed internal RPC

- **Route:** `POST /_internal/v1/data/storage/checkpoint` on the data plane's
  loopback listener.
- **Caller:** a new `BackupWorker` entry in the closed internal RPC allowlist.
  It uses the deployment internal-auth key, a synthetic tenant, the existing
  ±30 s skew window and the existing replay guard.
- **Request and response:** the body carries the backup id. The response
  carries the checkpoint path and the two durations.
- **Concurrency:** one checkpoint at a time. A concurrent request gets a
  conflict. Leftover `.online-checkpoints/*.checkpoint` directories from an
  interrupted backup are removed before each new checkpoint and at startup.
- **Where the worker lives:** `mako-online-backup` is a binary in the
  data-plane service package. It cannot live in `mako-storage-ops`, because
  `mako-internal-rpc` depends on `mako-storage` and that would be a cycle.

The precedent is `mako-operator-admin`, which signs as `OperatorAdmin` against
a loopback-only endpoint. A Unix signal was rejected because it gives no
response or error channel. A new socket transport was rejected because the
internal RPC already has authentication, replay protection and an allowlist.

### Release operations pause the schedule instead of requiring it off

`mako-release-operation upgrade|rollback` now does this:

1. Records each backup timer's enabled and active state.
2. Stops and disables the timers.
3. Stops any running backup job. Stopping an online backup is safe: the live
   database is untouched, and a leftover checkpoint or staging copy is swept by
   the next run.
4. Switches the release and waits for readiness.
5. Restores each timer to its recorded state.

If the operation fails, the timers stay stopped for inspection and their prior
state is recorded. Unknown state still fails closed. The operation still never
creates a checkpoint, and `checkpointPolicy` becomes `paused-for-deployments`.

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
  Today's evidence has 10 sign-ins, 4 push batches and a single pull, which is
  not a p95. Pull is measured in steady state. Recovery-to-ready and
  pull-after-recovery are still recorded, but they belong to the recovery drill
  rather than the latency gate.
- **Backups during the run:** the run happens with the production backup
  schedule enabled and must span at least two completed online backups. A
  backup that hurts latency or fails requests therefore fails the gate.
- **What is no longer gated:**
  - `release-gates.json` loses the beta thresholds and observations for the
    measurement window, backup age, recovery-point exposure and cost.
  - The internal gate is unchanged.
  - The measurement-window evidence file may keep being recorded as
    informational.
- **Validator:** `passed` is allowed when every threshold holds with no
  blockers. `blocked` must list only failing or unmeasured benchmark paths, or
  alert-delivery blockers.

### Backup state never moves admission

The guard drops these checks:

- `mako_storage_backup_remote_verified`;
- the backup-age check;
- `backupFreshnessRequired`, along with its context field and the
  `mako_public_preview_require_fresh_backup` group variable.

The waivable-blocker patterns drop the 30-day and cost entries, so an approval
naming them no longer validates.

The `backupAndRecovery` safeguard keeps its name, because it appears in
approval records. It is satisfied by a passing recovery drill, which after this
change must include a restore from an online backup. Backup failure and backup
age become alerts only.

## Risks / Trade-offs

- **Writes pause briefly on every backup.** A memtable flush plus hard links
  under the write lock, plus up to the barrier bound. This is measured on every
  backup and must stay inside the benchmark's budgets, because the benchmark
  runs with backups enabled.
- **A long push can starve backups.** If a push holds the shared guard past the
  bound, every attempt times out. Backup age grows and alerts. Admission is
  unaffected, by design.
- **Pinned SSTs use disk.** A checkpoint holds SST files until the worker
  deletes it. There is at most one checkpoint, and leftovers are swept.
  Monitoring must cover `.online-checkpoints`: the 2026-09-19 incident was
  exactly this kind of leak, in staging.
- **Orphaned sequencer gaps after a crash remain unclassified.** The barrier
  keeps backups from creating them, but a crash still can. This is out of scope,
  and it is worth its own change.
- **No enforced recovery point, no sustained-operation window, no cost
  ceiling.** These are accepted in the proposal for a beta that charges nobody.
  Reintroducing any of them later should be a gate condition that does not
  depend on deploy frequency.

## Migration Plan

1. Land the storage, sync, RPC and worker changes behind the existing timer
   configuration. Backups stay suspended on the beta.
2. Deploy the release with a release operation, and converge the backup role,
   which creates `.online-checkpoints` and installs the data-plane worker
   drop-in.
3. Re-enable backups on the beta (`mako_backup_enabled: true`) and run them for
   at least 24 hours. Require zero readiness failures, and record write-pause
   statistics.
4. Run the recovery drill from an online backup taken under concurrent pushes.
5. Change the gate, validator, guard and approval library. Run the hosted
   benchmark with backups enabled.
6. In the same convergence that installs the new guard and context, install
   either a beta approval (if the benchmark passed) or a new preview approval
   for the new blocker set. The old approval's blocker digest no longer
   matches, so skipping this step closes admission.

Rollback: re-enable the stop-based orchestrator path and suspend timers. The
manifest format is unchanged, so backups taken online remain restorable.

## Open Questions

- **The auth budget.** Auth latency is measured on password sign-in, which
  includes deliberate Argon2id cost. The 200 ms budget may be intended for
  token refresh or verification instead. Thresholds are unchanged here.
- **The barrier bound and staleness alert.** The default barrier bound
  (2 seconds) and the backup-age alert threshold should be confirmed against
  the measured push durations on the beta.
- **Ordering with the restore-verification change.**
  `observable-data-plane-and-restore-verification` also edits the orchestrator
  and the requirement *Backups and rollback are exercised off the live paths*.
  Whichever change archives second must rebase onto the other, and must not
  reintroduce the backup-age objective.
