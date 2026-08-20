## Context

The control plane owns collection, index, and policy definitions in SQLite. The data plane owns documents in RocksDB and answers every query. `separate-control-plane-sqlite` established that split; `finish-project-creation-path` carried collection metadata and policy activation across it. Indexes were the remaining piece, and their absence was invisible because the engine that builds them is thoroughly unit-tested — just never called.

## Goals / Non-Goals

- **Goal:** an index a developer creates becomes one a query can use, without further action.
- **Goal:** the state the API reports is the state that decides whether a query succeeds.
- **Non-Goal:** rebuilding an index after schema migration, or scheduling builds across tenants.

## Decisions

### The build runs inside the propagating call, in bounded pages

`backfill_index` writes at most one bounded page and commits its progress marker with its entries, so it is resumable by construction. The install handler loops it to completion, catches up to the committed high water, then activates.

The alternative was a background worker in the data plane, which today has none. A worker needs a durable pending-build queue and cross-tenant enumeration to find work, and it would leave the developer polling an index that no request is driving. Running the build where it was asked for is simpler and has no state to reconcile after a restart.

The cost is that a collection large enough for the build to exceed the internal-RPC timeout leaves the index `building`. That is not a stuck state: reading the index advances the build too, and every page already committed is kept, so polling converges. A deployment that outgrows this wants the worker, and the resumable page loop is what it would be built from.

### Reported state comes from the data plane

The control plane's local record is the definition of record; it is not evidence that a query can be answered. `InspectIndex` reports the data plane's state and both the single-index read and the listing use it, so they cannot disagree with each other or with what a query does.

## Risks / Trade-offs

- **A listing costs one internal call per index.** Collections carry few indexes, and the alternative — a listing that says `building` while the single read says `active` — is worse than the round trips.
- **Propagation can fail after the local record is written.** Creation then fails and the definition exists only in the control plane. Re-issuing the same create converges, because recording the definition in the data plane is idempotent and the build resumes.
