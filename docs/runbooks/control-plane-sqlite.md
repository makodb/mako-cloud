# Control-plane SQLite operations

## Readiness, contention, WAL, and capacity

Keep the control service stopped if identity, schema, integrity, migration, or capacity checks fail. Inspect with `mako-control-storage-ops inspect-sqlite --database … --identity …`; do not copy, edit, vacuum, or recreate the live file. Busy failures are retryable only within the caller's bounded policy. Investigate long transactions before changing timeouts. On WAL pressure, stop new mutations, allow graceful service shutdown/checkpoint, and verify integrity. On critical capacity, preserve the root reserve for recovery, stop mutation traffic, and add space before restarting.

## Offline migration and cutover

Pause public control mutations; checkpoint the old control RocksDB; verify and copy it off the VM; stop/fence the control plane; calculate the immutable checkpoint digest; write the fence marker; create a plan bound to exact source, temporary/live target, lock, database identity, release digest, and configuration digest; inspect the source; then run the confirmed `migrate` operation. Require matching byte count, framed BLAKE3 checksum, prefix inventory, SQLite integrity, receipt digest, and domain probes before selecting SQLite. Never allow an empty fallback.

## Backup and restore

Run `backup` with the live database, dedicated staging/publish directories, exact release digest, safe backup id, retention, and protected signing-key file. Verify the locally published and off-VM copies. Restore only to a nonexistent offline target with matching identity/release and maximum age. Inspect it before `promote`. Promotion refuses any existing live target.

## Corruption and rollback

On corruption, stop the service, preserve database/WAL/SHM and logs read-only, identify the newest authenticated backup meeting high-water and age policy, restore to an empty target, verify domain invariants, and explicitly promote during a maintenance window. After SQLite accepts any production write, select only releases that declare support for the current SQLite format. A RocksDB-only release is not a rollback target; returning to RocksDB requires a separately designed and verified reverse migration.

SQLite and tenant RocksDB share one VM/data disk in the beta. Complete VM or disk loss requires off-VM recovery and is not masked by this storage split.
