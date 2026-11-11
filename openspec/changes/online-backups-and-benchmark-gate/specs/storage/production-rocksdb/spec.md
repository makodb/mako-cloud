## ADDED Requirements

### Requirement: Backups are taken online by the owning process
The process that owns a production RocksDB database SHALL create each backup
checkpoint in-process, while it continues to serve, at a point consistent with
every adapter mutation and every sequence-allocating commit group. Taking a
backup MUST NOT stop the owner, fail its readiness, or reject requests.

The checkpoint MUST be created on the filesystem that holds the live database,
so that database files are linked rather than copied. The owner MUST refuse a
checkpoint target on another device before pausing any writes.

Writes MAY pause while the checkpoint is created. That pause MUST be bounded by
a configured barrier wait, and its duration MUST be recorded for every backup. A
checkpoint that cannot acquire the barrier within the bound MUST be abandoned
without blocking writes any further, and MUST be reported as a failed,
retryable backup.

A checkpoint MUST be requested only through an authenticated, loopback-only
private interface, and at most one may exist at a time. A checkpoint left by an
interrupted backup MUST be removed before another is created and when the owner
starts.

Copying, sequencer recovery, inventory, signing, publication, transfer and
verification SHALL run outside the owner against a copy of the checkpoint, and
MUST NOT open the live database. Online backups MUST produce the same manifest
format, and satisfy the same restore proofs, as any other backup.

#### Scenario: Backup runs while the database serves traffic
- **WHEN** a scheduled backup runs while clients read, write and push
- **THEN** the owner stays ready, no request fails because of the backup, and the recorded write pause is within the configured bound

#### Scenario: A push is in flight when the checkpoint is requested
- **WHEN** a sync push has leased positions but has not yet aborted its remainder and recovered its high water
- **THEN** the checkpoint waits for that commit group to finish, within the bound, so the artifact never contains a partially committed push

#### Scenario: The barrier cannot be acquired in time
- **WHEN** in-flight commit groups hold the barrier past the configured bound
- **THEN** the checkpoint is abandoned, writes resume, the backup is recorded as failed and retryable, and the next schedule tries again

#### Scenario: The checkpoint target is on another filesystem
- **WHEN** the configured checkpoint directory is not on the live database's device
- **THEN** the owner refuses the request before pausing writes and never copies the database under the write lock

#### Scenario: A backup is interrupted
- **WHEN** the worker or the owner stops between checkpoint creation and publication
- **THEN** the live database is unaffected, and the leftover checkpoint and staging copy are removed before the next backup proceeds

#### Scenario: An online backup is restored
- **WHEN** an operator restores a verified backup that was taken while the owner was serving pushes
- **THEN** restore verification proves the manifest, digests, tenant boundaries and every acknowledged position through the recorded high water, as it does for any other backup
