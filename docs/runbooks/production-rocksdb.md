# Production RocksDB incident runbook

1. Remove the affected stateful service from routing. Do not restart it against
   another or empty path.
2. Identify the bounded `service` and `volume` alert labels. Do not copy storage
   keys, values, signing material, or tenant documents into incident notes.
3. Confirm exactly one owner exists. If ownership or node state is uncertain,
   fence the workload and node before touching the volume.
4. For capacity or compaction pressure, stop discretionary writes, expand the
   existing retained claim, and watch available bytes, write-stopped state,
   pending compaction bytes, and I/O counters.
5. For lock loss, I/O errors, or corruption, preserve the volume and logs. Do
   not delete `LOCK`, WAL, manifest, or SST files and do not repair the only copy.
6. If the volume is intact, restart the same binary or a format-compatible
   rollback binary on that volume and wait for RocksDB plus sequencer recovery.
7. If replacement is required, verify an eligible backup, restore only to an
   empty offline volume, review its tenant/high-water report, and promote only
   after the previous owner is fenced.
8. Keep traffic disabled if backup age exceeds the RPO, restore verification
   fails, the target is unexpectedly empty, or recovery exceeds its recorded
   high water. Escalate tenant mismatch as a security incident.
9. For the control-plane volume, verify operator entitlements and authorization epochs survive while
   expired or revoked operator sessions remain unusable. Never edit operator keyspaces directly;
   follow the [operator password-authentication runbook](operator-password-authentication.md).

Detailed commands and rollback steps are in
[Production RocksDB deployment and operations](../production-rocksdb-operations.md).
