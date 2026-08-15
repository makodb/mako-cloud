# Retention and compaction jobs

`RetentionJob` runs one tenant-scoped retention pass across structured logs, append-only audit detail, raw usage detail, and each configured document collection. Every run returns eligible and removed counts per data class.

Always run `RetentionMode::DryRun` first. Dry-run performs no writes and, for document history, does not advance the checkpoint-expiry barrier. Apply mode removes expired log/audit/usage detail, then compacts revisions, change records, mutation-idempotency receipts, and eligible tombstones through each collection’s explicitly supplied committed position.

Important invariants:

- Audit and log ID digest guards remain after detail expires, so an old ID cannot be reused.
- Usage digest guards remain while raw detail is removed, so a retry cannot increment retained aggregates twice.
- A document retention barrier is advanced before history is deleted; checkpoints below it are expired first.
- Tombstones are conditionally deleted only if their exact snapshotted value is still current, preventing compaction from deleting a concurrent resurrection.
- A job rejects duplicate collections and any collection whose trusted tenant differs from the job tenant.
- Compaction never advances beyond committed high water and never silently weakens configured durability.

Schedule jobs per tenant with bounded collection targets, retain their reports as operational telemetry, and alert on repeated failures or a growing eligible backlog. A failed apply pass is safe to retry: all detail records are immutable/idempotent, and document barriers only move forward.
