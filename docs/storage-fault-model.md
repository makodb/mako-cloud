# Storage atomicity fault model

The semantic adapter treats one conditional mutation as condition reads plus a staged batch, one atomic commit point, and acknowledgement. The deterministic adapter exposes the following named failure locations in order:

1. `BeforeWrite`
2. `BeforeConditionCheck`
3. `AfterConditionCheck`
4. `BeforeBatchStage`
5. `AfterBatchStage`
6. `BeforeCommit`
7. `AfterCommit`

Failures through `BeforeCommit` return an error with the original state intact. The staged map is private and cannot be observed. A failure at `AfterCommit` models a process or transport loss after the storage commit but before the caller receives acknowledgement; recovery may observe the entire new state. Retrying the original revision condition then conflicts. No failure point permits only a document, index, change record, or idempotency result subset to become visible.

`crates/mako-storage/tests/fault_injection.rs` injects an I/O error at every point. It verifies the exact pre-commit state for the first six, the exact full-commit state for the post-commit ambiguity, retry behavior, and the successful acknowledged path. Real adapters must map their transaction boundary to the same visibility outcomes and pass the shared conformance/restart suite.

The control-plane SQLite adapter maps this model to `BEGIN IMMEDIATE`, guarded reads, mutations, and one `COMMIT`. Synchronous durability is the minimum and WAL recovery is tested by abruptly terminating a helper process after an acknowledged write. Dedicated read transactions provide snapshots. Bounded busy time returns a retryable timeout; corruption, full disk, read-only files, identity mismatch, unsupported schema, and critical reserve pressure fail closed with sanitized errors. Graceful shutdown rejects new work, drains bounded operations, truncates the WAL, verifies integrity, synchronizes the database and directory, and releases its exclusive process lock.

SQLite and RocksDB directories remain on the same public-beta VM and data disk. Engine separation prevents a tenant RocksDB outage from becoming the control authentication authority, but it is not replication, automatic failover, or host/disk high availability.
