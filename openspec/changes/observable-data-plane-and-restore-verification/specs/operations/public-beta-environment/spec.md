## MODIFIED Requirements

### Requirement: Backups and rollback are exercised off the live paths
The beta environment SHALL create authenticated checkpoint backups outside each
live database path when backups are enabled. It SHALL alert operators when a
backup fails or when the newest successful backup is older than its configured
objective, and it SHALL verify restore to an empty offline target. Backup age
is an operational objective. It MUST NOT be an admission or release-gate
condition.

Restore verification SHALL recur on a schedule and SHALL publish its outcome
and its age as observable signals, so that a stale or failing verification is
detectable without an operator going to look. A dated record of a past drill
SHALL NOT stand in for the recurring signal. The recurring verification SHALL
NOT promote anything it restores, and its outcome MUST NOT open or close public
admission in any mode: like backup age, it is reported through operator alerts.

Operators SHALL exercise service, configuration, policy, function, schema,
signing-key and format-compatible storage rollback for the exact candidate
release. Restore promotion and rollback MUST be operator-controlled and MUST
NOT overwrite the sole live copy automatically.

#### Scenario: Scheduled backup completes
- **WHEN** a healthy stateful database reaches its backup schedule
- **THEN** an authenticated checkpoint and manifest are stored outside the live path and verified before the backup is considered successful

#### Scenario: Backups fall behind
- **WHEN** backups fail or the newest successful backup exceeds its age objective
- **THEN** operators are alerted, and public admission is not changed by the backup state

#### Scenario: Operator performs the beta recovery drill
- **WHEN** an operator restores the selected backup to an empty target and executes the documented rollback matrix
- **THEN** recovery, readiness, tenant-boundary, audit and rollback evidence are recorded without destroying the original live volume

#### Scenario: Scheduled restore verification succeeds
- **WHEN** the recurring verification restores the newest published backup into an empty offline target
- **THEN** it checks the manifest, file digests, tenant keyspace boundaries, and acknowledged commit high water, publishes success and the time it ran, releases the target, and leaves every live copy untouched

#### Scenario: Restore verification fails or stops running
- **WHEN** the recurring verification fails, or has not completed within its configured interval
- **THEN** operators are alerted that the deployment is running without current recovery evidence, and public admission is not changed by the verification outcome

#### Scenario: A retained recovery signal cannot be gathered
- **WHEN** the component that publishes a recovery or migration signal cannot read the state it reports on
- **THEN** it publishes no value for that signal rather than a default, so a signal that is never observable is reported as missing instead of reading as a failure or as healthy
