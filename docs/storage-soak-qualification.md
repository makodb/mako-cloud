# Storage soak qualification

Run `npm run test:storage-soak` after changing adapter transactions,
acknowledgement behavior, sequencing, indexes, retention, or recovery.
`MAKO_STORAGE_SOAK_ITERATIONS` overrides the default 25 complete cycles and
must be a positive integer. Set `MAKO_STORAGE_TMPDIR` to a directory on the
storage class being qualified; RocksDB test volumes are created beneath it.

Each cycle includes:

- every injected pre-commit I/O failure and the ambiguous post-commit/pre-acknowledgement crash point;
- memory and RocksDB adapter conformance, including conditional races and stable snapshots;
- sync-acknowledged RocksDB close/reopen durability;
- concurrent document revision and acknowledgement invariants across restart;
- unresolved sequencer gaps, abort/commit advancement, and RocksDB restart recovery;
- snapshot index backfill, concurrent change-log catch-up, uniqueness failure, and atomic activation;
- retention barriers, expired checkpoints, historical revisions, idempotency records, and safe tombstone compaction.

## Latest qualification

The 2026-08-06 portability soak completed 25 consecutive cycles with no failures
on the workspace's persistent NFSv4.1 filesystem. Each cycle ran 52 storage
tests and 43 document tests, for 2,375 successful test executions. This is
retained as extra evidence, not as the local production topology result because
production must not use a shared RocksDB directory.

The release-facing local Btrfs result is recorded in
[`production-rocksdb-qualification.md`](production-rocksdb-qualification.md).
