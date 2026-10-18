## Context
See proposal.md. RocksDB is exclusively owned by one process; all writes pass through the adapter and its operation lock. Billing samples are observability, not an admission dependency. Existing data must remain readable during reconciliation.

## Goals / Non-Goals
Preserve exact logical key-plus-value totals and synchronous durability while making steady-state sampling independent of tenant size. Preserve audit, quota, document revisions, and current data. Do not redesign import or scheduler admission.

## Decisions
- Register only canonical tenant ranges. Store a cursor, seeded byte total, and 64 signed delta shards outside tenant ranges in a versioned system namespace.
- Mutation transactions read previous values and update the appropriate delta shards in the same commit. Duplicate operations in one batch see their own writes; failed comparisons and retries cannot affect totals. Shards reduce unrelated-write contention. Sorted per-shard mutexes serialize colliding counters before optimistic transactions, avoiding retry exhaustion on unconditional writes; disjoint shards retain group-commit concurrency.
- While initializing, only writes at or before the committed cursor affect deltas. A bounded reconciliation step takes the existing operation lock exclusively, counts a page from current data, and atomically advances the cursor/base. Other writes retain shared-lock concurrency. No long-lived snapshot or separate local worker is required.
- Budget reconciliation globally by time, entries, bytes and tick interval; disable block-cache population and automatic scan readahead. A single value/block may exceed a soft step budget. Return pending until complete; never emit a partial total as a usage sample.
- Persist progress and counts. A clean-close sequence fence proves no older binary wrote after close; otherwise reset derived accounting metadata and reconcile conservatively. Tenant data is untouched. This supports safe rollback/roll-forward and crash recovery without trusting stale totals.
- A dirty tenant stays queued through cooldown, and eligibility is checked when dequeued. Errors and pending reconciliation remain queued. A write during measurement cannot immediately retrigger measurement.
- Use a 256 MiB RocksDB block cache. This is bounded per adapter and fits the measured host headroom; scans do not fill it.

## Risks / Trade-offs
- Bootstrap can take hours for large existing tenants → expose pending, retain prior ledger samples, throttle I/O; document requests continue.
- Reconciliation briefly excludes writes → bounded page budget and nonblocking lock acquisition, no full-tenant critical section.
- Shard collisions can cause optimistic retries → existing bounded retry behavior plus concurrency tests.
- Unclean stop or old binary writes invalidates accounting trust → restart derived accounting only; rebuild in bounded steps.

## Migration Plan
Run storage, telemetry, document/sync regressions and validators; build the matching Linux release. Capture live disk/request baseline. Pause existing schedules for deployment, use the immutable release upgrade workflow, verify readiness, restore schedules, and compare disk activity and request latency. Keep the prior release available; no data snapshot restore. Derived counters are fenced across rollback.
