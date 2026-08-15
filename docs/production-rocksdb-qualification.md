# Production RocksDB qualification

Status: **PASS — eligible for the tested single-node topology**  
Recorded: 2026-08-07

Local RocksDB `OptimisticTransactionDB` is the sole supported production
key-value backend. The retained machine-readable result is
[`production-rocksdb-qualification.json`](production-rocksdb-qualification.json),
and `npm run validate:production-release` enforces its release thresholds
against the retained performance baseline.

## Qualified topology

- Linux 7.0.14-5-pve on x86_64, Rust/Cargo 1.97.1, Node.js 24.15.0.
- One RocksDB owner on local persistent Btrfs (`/dev/sda3`), with test volumes
  beneath `/var/tmp/mako-cloud-qualification`.
- `OptimisticTransactionDB`, synchronous writes, fsync, WAL verification, and
  paranoid integrity checks.
- 1.706 TB free at qualification time; the release warning and critical
  reserves are 2 GiB and 1 GiB respectively.

This result qualifies the tested filesystem and host class. Repeat the same
command with `MAKO_STORAGE_TMPDIR` on the actual RWO production volume before a
different storage class is rolled out. The additional 25-cycle NFSv4.1
portability result is retained in
[`storage-soak-qualification.md`](storage-soak-qualification.md), but NFS is not
the production topology and a shared RocksDB directory remains unsupported.

## Results

- 25 consecutive local-storage cycles passed, each with 52 storage and 43
  document tests: 2,375 executions, zero failures, zero acknowledged-write
  losses, and zero integrity failures.
- Shared adapter conformance, conditional races, synchronous process-crash
  restart, startup readiness, I/O and capacity faults, lock contention,
  compaction/retention, and acknowledged-high-water recovery passed.
- Authenticated checkpoint backup, read-back verification, corruption and stale
  artifact rejection, tenant inventory, empty-target restore, explicit
  promotion/fencing, and interrupted restore passed.
- Same-volume recovery completed in 1.43 seconds and replacement-volume restore
  plus verification completed in 1.72 seconds, including test-process startup.
- The backup tests verified a 50-second-old artifact under a 100-second test
  policy, and restored every acknowledged position present at checkpoint time.
- Previous-format-compatible binary rollback read and wrote the same non-empty
  volume; a missing path with create-if-missing disabled stayed empty.
- All 11 release-profile performance paths passed the retained latency budgets.
  Exact observations are in
  [`performance-baseline.json`](performance-baseline.json).

## Release thresholds

- No acknowledged-write loss or integrity failure is permitted.
- A successful verified backup must be at most 15 minutes old; the corresponding
  maximum backup-based recovery-point exposure is 15 minutes.
- Same-volume recovery must complete within 5 minutes; verified replacement
  restore and promotion must complete within 30 minutes.
- Free capacity must remain above the 2 GiB warning reserve and must fail closed
  at the 1 GiB critical reserve.
- Storage/RxDB p95 component budgets are 50 ms for writes and 100 ms for pull,
  hidden scans, push conflicts, and live fan-out. The index-build budget is
  500 ms; the remaining per-component budgets are encoded in the JSON report.

## Reproduce

```bash
MAKO_STORAGE_TMPDIR=/path/on/the/volume/under/test \
  bash scripts/run-production-rocksdb-qualification.sh
```

The topology intentionally provides no automatic failover, active-active
writes, shared database directory, or zero-downtime stateful replacement. Node
or volume loss causes a write outage until an operator verifies and explicitly
promotes a replacement.
