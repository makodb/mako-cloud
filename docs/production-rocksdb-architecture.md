# Production RocksDB architecture

Mako Cloud uses local RocksDB `OptimisticTransactionDB` as its sole production ordered
key-value store. Each stateful service database is opened by exactly one process
from one explicitly provisioned persistent volume. Stateless gateways may scale;
a RocksDB database owner may not be replicated or share its directory.

## Distributed-surface audit

The 2026-08-06 source, export, test, configuration, documentation, and service
call-site audit found one dormant vendor-facing surface:
`crates/mako-storage/src/distributed.rs` and its re-exports from
`crates/mako-storage/src/lib.rs`. It contained connector identity, endpoint,
namespace, option, and credential types plus self-contained tests. No service,
configuration type, environment example, factory, or non-storage crate called
it, and no vendor SDK was present. The surface is therefore removed rather than
kept as an unsupported production choice.

The following semantic pieces remain intentionally:

- `KvAdapter`, `KvSnapshot`, and the atomic write/transaction contract used by
  domain crates;
- deterministic `MemoryAdapter` tests and fault injection;
- `RocksDbAdapter` as the local durable implementation;
- shared conformance, safe storage errors, readiness checks, capability reports,
  and collision-safe tenant key encoding.

Production startup constructs the RocksDB implementation directly. Configuration
has no backend selector, remote storage endpoint, vendor namespace, connector
option, or storage credential. Adding another production backend requires a new
change and must not happen through dormant configuration.

Optimistic transactions use snapshot-based `get_for_update` conflict detection
and pass the shared atomic conditional-race suite. This binding is selected
because its safe native checkpoint API captures the live synchronous database;
the pessimistic wrapper does not expose that API.
