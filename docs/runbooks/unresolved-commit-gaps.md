# Unresolved commit gaps

Severity: critical. Owner: data-plane on-call. A gap prevents committed high water from advancing, so RxDB pulls and live streams may lag without losing ordering guarantees.

## Triage

1. Scope the alert by project and environment, then inspect unresolved gap count, oldest gap age, committed high water, and writer health.
2. Determine whether each position belongs to an active lease, a transaction awaiting publication, a crashed writer, or a durable abort record.
3. Check storage health before treating the issue as a sequencer-only fault.

## Containment

1. Preserve the current high water; never skip, overwrite, or manually mark a position committed merely to reduce lag.
2. Pause retention/compaction for the affected keyspace and limit new writes if the gap backlog is growing.
3. Let reads continue only at the last proven committed high water; live clients may receive the normal resynchronization signal.

## Recovery

1. Recover the owning writer or allow lease-expiry recovery to classify the position from durable transaction evidence.
2. Record an abort only when the mutation transaction is proven absent; publish a commit only when every atomic mutation artifact is present.
3. Verify contiguous high-water advancement, replication lag recovery, and restart behavior before closing.
4. Escalate if durable evidence is contradictory or a gap survives its lease and recovery deadlines.
