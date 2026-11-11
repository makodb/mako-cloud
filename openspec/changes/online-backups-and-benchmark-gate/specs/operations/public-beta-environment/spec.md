## ADDED Requirements

### Requirement: The single-region beta gate is decided by the hosted benchmark
The single-region beta gate SHALL pass for a release exactly when the hosted
benchmark run against that release on the beta deployment meets every
threshold, and the deployment's trusted-HTTPS, route-allowlist,
application-security and recovery-drill qualifications remain valid. The
benchmark thresholds are:

- p95 latency within the recorded budgets for auth, RxDB pull, RxDB push, live
  delivery, warm functions and cold functions;
- the required capacity headroom at target load;
- no failed requests at target and headroom concurrency;
- zero acknowledged-write loss;
- zero integrity failures.

Each gated latency MUST be computed from at least 100 steady-state samples of
that path. A path with fewer samples is unmeasured and MUST block the gate. The
run MUST execute with the production backup schedule enabled and MUST span at
least two completed online backups.

The gate MUST NOT require a calendar measurement window, a backup-age or
recovery-point objective, or cost observations. Those MAY be recorded as
informational evidence. Thresholds MUST NOT be changed to make a run pass. A
gate result applies only to the release and plan it measured.

#### Scenario: The benchmark meets every threshold
- **WHEN** a benchmark run on the deployed release meets every threshold with enough samples, while online backups complete during the run
- **THEN** the gate records the release as passed, and an authorized operator may approve rate-limited public beta admission for that release

#### Scenario: A path exceeds its budget
- **WHEN** any gated p95 exceeds its budget, or the run records a failed request, lost write or integrity failure
- **THEN** the gate is blocked, and each blocker names the failing path and its measured value

#### Scenario: A path has too few samples
- **WHEN** a gated path has fewer than 100 steady-state samples
- **THEN** that path is reported as unmeasured and the gate is blocked

#### Scenario: Informational observations are incomplete
- **WHEN** no 30-day window, backup age or cost observation has been recorded
- **THEN** none of them is a blocker, and the gate result depends only on the benchmark and the retained qualifications

#### Scenario: A new release is deployed after a passing run
- **WHEN** a release operation selects a different release
- **THEN** admission is not changed by the deployment, and that release's own benchmark run decides its gate result

## MODIFIED Requirements

### Requirement: Backups and rollback are exercised off the live paths
The beta environment SHALL create authenticated checkpoint backups outside each
live database path on a recurring schedule, without stopping the owning
service. It SHALL alert operators when a backup fails or when the newest
successful backup is older than its configured objective, and it SHALL verify
restore to an empty offline target. Backup age is an operational objective. It
MUST NOT be an admission or release-gate condition.

Operators SHALL exercise service, configuration, policy, function, schema,
signing-key and format-compatible storage rollback for the exact candidate
release. Restore promotion and rollback MUST be operator-controlled and MUST
NOT overwrite the sole live copy automatically.

#### Scenario: Scheduled backup completes
- **WHEN** a healthy stateful database reaches its backup schedule
- **THEN** an authenticated checkpoint and manifest are stored outside the live path and verified before the backup is considered successful, while the owning service stays ready

#### Scenario: Backups fall behind
- **WHEN** backups fail or the newest successful backup exceeds its age objective
- **THEN** operators are alerted, and public admission is not changed by the backup state

#### Scenario: Operator performs the beta recovery drill
- **WHEN** an operator restores a backup taken online to an empty target and executes the documented rollback matrix
- **THEN** recovery, readiness, tenant-boundary, audit and rollback evidence are recorded without destroying the original live volume

### Requirement: Release operations do not create backups
Upgrade and rollback commands SHALL run without creating data-plane RocksDB or
control-plane SQLite backup checkpoints. This behavior MUST apply to the
ordinary invocation without a per-deployment skip flag or renewed approval.

Before stopping application services or changing the selected release, a
release operation MUST:

- record the enabled and active state of each backup timer;
- stop and disable those timers;
- stop any running backup job.

Stopping an online backup MUST leave the live database untouched; its leftover
checkpoint and staging copy are removed by the next backup. After the selected
release passes readiness, the operation SHALL restore each timer to its
recorded state. A failed operation MUST leave the timers stopped and record
their prior state. Unknown backup state MUST fail closed.

Storage compatibility, nonempty-path checks, exclusive ownership, fencing,
configuration snapshots and readiness checks MUST remain enforced. Existing
databases and retained backup artifacts MUST remain intact.

Successful operation evidence MUST record:

- `checkpointSkipped: true`;
- `checkpointVerified: false`;
- `checkpointPolicy: "paused-for-deployments"`;
- the backup schedule state before and after the operation.

A deployment MUST NOT claim a new verified recovery point merely because the
release switch succeeded.

#### Scenario: Ordinary upgrade runs without backups
- **WHEN** an operator runs the standard upgrade command, whether or not backup timers are enabled
- **THEN** the release switches after storage checks without creating a checkpoint, and any enabled timers are paused for the switch and restored to their recorded state once readiness passes

#### Scenario: Ordinary rollback runs without backups
- **WHEN** an operator runs the standard rollback command to the recorded compatible last-known-good release
- **THEN** storage fencing, configuration restoration and readiness checks run without creating a backup, and the backup schedule returns to its recorded state

#### Scenario: Backup activity conflicts with deployment
- **WHEN** a backup timer is enabled or a backup job is running as a release operation begins
- **THEN** the operation records the schedule, stops the timers and the running job without creating a backup, and proceeds; if the timer or job state cannot be read, it refuses to select another release and records no successful deployment

#### Scenario: The release fails readiness
- **WHEN** the selected release does not become ready
- **THEN** the backup timers remain stopped, their prior state is recorded, and no successful deployment is recorded

### Requirement: Public admission follows the single-region beta gate
Qualified public beta traffic SHALL be admitted only after the exact release
and deployment pass the single-region beta gate decided by the hosted
benchmark. Pre-gate qualification access MUST be limited to authorized testers
unless an authorized operator activates the distinct risk-accepted
public-preview mode. Neither preview mode, a reachable VM, nor a valid
certificate changes the release decision or permits the deployment to be
represented as a passing or qualified beta release.

Risk-accepted public preview MUST retain all of the following non-waivable
safeguards:

- publicly trusted HTTPS and verified HSTS;
- the exact public route allowlist;
- application authentication;
- document-policy enforcement;
- quotas and rate limits;
- audit;
- a passing recovery drill;
- zero acknowledged-write loss and zero integrity failures for the exact
  release;
- a tested emergency admission stop.

An operator MAY accept named benchmark-latency and alert-delivery blockers only
through a retained approval record. The record identifies the operator, the
exact plan hash, the active blockers, the acceptance time, and the release
digest those safeguards were measured on. It persists until an operator pauses
it, rather than expiring on a calendar.

Backup age, backup schedule state and the outcome of an individual scheduled
backup MUST NOT open or close public admission in any mode. They are reported
through operator alerts.

The preview approval MUST fail closed to source-restricted pre-gate admission
when its plan binding no longer matches, the recorded blocker set changes, or
any non-waivable safeguard fails. Deploying a different release MUST NOT by
itself close admission, and the deployed release MUST NOT be compared against
the approval to decide admission. Re-enabling preview requires a new approval
record.

Preview changes only the ingress source allowlist. It MUST NOT bypass
authentication, authorization, policy, quota, rate-limit, audit, route or
private-listener controls.

#### Scenario: Qualification evidence is incomplete
- **WHEN** the hosted benchmark has not passed for the deployed release and no valid risk-accepted preview approval exists
- **THEN** unrestricted public admission remains disabled while authorized qualification traffic may continue

#### Scenario: Operator accepts bounded public-preview risk
- **WHEN** every non-waivable safeguard passes and an authorized operator records an approval bound to the exact plan and current blocker set
- **THEN** the deployment admits rate-limited public-preview traffic without changing the blocked beta-gate result or describing the release as qualified beta

#### Scenario: Public-preview approval becomes stale or unsafe
- **WHEN** the preview approval's plan binding changes, the active blocker set differs, or a non-waivable safeguard fails
- **THEN** unrestricted ingress fails closed to source-restricted pre-gate admission and requires a new valid approval before preview can resume

#### Scenario: A scheduled backup fails during preview
- **WHEN** a backup fails or backups fall behind while preview admission is active
- **THEN** admission is unchanged and operators are alerted

#### Scenario: The beta gate passes
- **WHEN** the hosted benchmark passes for the exact deployed release and the retained qualifications remain valid
- **THEN** an authorized operator may enable rate-limited public beta admission and records the approval and release digest

## REMOVED Requirements

### Requirement: Approved test preview can suspend scheduled backups
**Reason**: The exception existed because data-plane backups stopped the
service and backup freshness gated preview admission. Online backups no longer
interrupt the service, and backup state no longer affects admission or the beta
gate, so no scoped exception is needed.

**Migration**: `mako_backup_enabled` remains an ordinary operations setting.
Remove `mako_public_preview_require_fresh_backup` and the
`backupFreshnessRequired` context field, and re-enable the beta's backup
schedule once online backups are qualified. Retained artifacts are unaffected.
