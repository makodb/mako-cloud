## Why
Live profiling identified storage usage accounting reading 3.28 TB in 23 hours from a roughly 44 GiB database. Full tenant scans, cache pollution, and a cooldown race compete with document requests and durable audit/quota writes.

## What Changes
- Maintain exact logical byte totals through atomic write deltas, with sharded counters.
- Seed existing tenants using bounded, resumable reconciliation, without publishing partial totals.
- Gate queued samples at execution and retain dirty marks during cooldown.
- Exclude reconciliation from the request cache; configure an explicit bounded RocksDB cache.
- Deploy and measure the real cloud workload without weakening durability or audit.

## Capabilities
### New Capabilities
None.
### Modified Capabilities
- `billing/metering`: bounded reconciliation and accurate incremental storage samples under concurrent writes and restart.

## Impact
RocksDB adapter, data-plane telemetry, storage tests, operator documentation. No public API changes or changes to tenant documents. User explicitly approved implementation and deployment after investigation.
