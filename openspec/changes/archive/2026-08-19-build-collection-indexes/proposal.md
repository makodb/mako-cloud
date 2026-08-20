## Why

Document queries are refused unless an active index covers them — a deliberate choice that keeps every query bounded. But no index could ever become active, so the document query API was unusable in a running system.

`createCollectionIndex` recorded the definition in the control plane's own store and returned `202 building`. The data plane, which holds the documents an index is built from and answers every query, was never told. `backfill_index`, `catch_up_index`, and `activate_index` exist in `mako-documents` and are covered by unit tests, but no service ever called them. An index stayed `building` forever, and `POST .../documents/query` answered `document query requires an active index` for the life of the deployment.

This is the same gap as collection metadata and policy activation: state created in the control plane that never crossed into the data plane that enforces it.

## What Changes

- Propagate a created index to the owning data plane over internal RPC under a new `InstallIndex` operation, and build it there.
- Report index state from the data plane on read, so `getCollectionIndex` and `listCollectionIndexes` describe the index a query would actually be answered against rather than the definition the control plane recorded.
- Build in bounded, resumable pages: backfill, then catch up to the committed high water, then activate atomically.

## Capabilities

### Modified Capabilities

- `cloud/control-plane`: Require a created index to become usable for document queries, and require reported index state to be the state that decides whether a query is answerable.

## Impact

- `crates/mako-internal-rpc`, `services/mako-data-plane`, `services/mako-control-plane`
- `crates/mako-smoke` — new `database_service` end-to-end test
- No wire change: the endpoints, their request bodies, and their status codes are unchanged.
