# RxDB chaos qualification

Run `npm run test:rxdb-chaos` before a release and after changing mutation idempotency, checkpointing, live delivery, policy visibility, schemas, retention, or client recovery.

The suite models:

- concurrent offline clients writing from one assumed master and resolving a readable conflict;
- a response dropped after commit, followed by duplicate retries whose five rows arrive in 128 generated orderings;
- hidden changes, visible-to-hidden synthetic tombstones, remote deletion, and policy-driven authorization-epoch reset;
- live-stream reconnects, explicit stream gaps, bounded-buffer overflow, service failover, and resumable cursors;
- incompatible schema versions and client migration hooks;
- checkpoint expiry after compaction and full-resync hooks;
- RocksDB service restart with a previously issued checkpoint;
- access-token refresh and revoked-refresh transition to authentication-required.

Each invariant asserts stable outcomes and that duplicate or reordered retries create no additional document revisions or change records.

## Latest qualification

The 2026-08-07 local run passed the sync and document Rust suites and all 14
RxDB client tests. Production RocksDB restart, authenticated backup/restore,
and persistent-storage fault drills also pass in the retained production
qualification. Multi-node storage remains outside the supported topology.
