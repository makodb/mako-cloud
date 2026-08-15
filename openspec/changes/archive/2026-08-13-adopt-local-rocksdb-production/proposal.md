## Why

Mako Cloud does not currently have a selected or supplied distributed-KV
backend, and keeping production blocked on that future integration adds
complexity without helping the initial product. Production should use the
already implemented and qualified local RocksDB transactional path, with its
single-node operational limits made explicit.

## What Changes

- **BREAKING** Make local RocksDB the only supported production document and
  control-plane backing store; remove production distributed-adapter selection,
  connector configuration, and qualification requirements.
- Retain the internal semantic `KvAdapter` boundary for deterministic tests and
  service layering, but remove the vendor-facing distributed connector surface
  and any readiness state that waits for an external adapter.
- Require a dedicated persistent volume per production storage node, exclusive
  database ownership, synchronous durability, startup locking, and fail-closed
  readiness when the database cannot be opened or verified.
- Replace distributed-store fault and disaster-recovery gates with local
  RocksDB restart, crash, backup, restore, integrity, and acknowledged-high-water
  recovery qualification.
- Define production as a single-writer/single-node storage topology for the
  initial release. Automatic multi-node failover, active-active writes, and
  horizontal storage scaling are not supported.
- Reconcile the still-active `build-mako-cloud-mvp` plan so its distributed
  adapter tasks and release blockers no longer remain in the MVP definition.

## Capabilities

### New Capabilities

- `storage/production-rocksdb`: Define the supported production RocksDB
  topology, durability, readiness, persistent-volume, backup/restore, recovery,
  and operational safety contract.

### Modified Capabilities

None. The related `storage/document-engine` capability is still being
introduced by the unarchived `build-mako-cloud-mvp` change rather than existing
as a main spec; its planning artifacts will be reconciled during implementation.

## Impact

- Affects `mako-storage`, service configuration and startup, readiness checks,
  provisioning, deployment manifests, local/production runbooks, qualification
  scripts, release gates, and storage documentation.
- Removes the `DistributedAdapterConnector`, `DistributedAdapter`, and their
  connection configuration if no non-production caller requires them.
- Removes the need for a vendor SDK, distributed test endpoint, adapter
  credentials, or network-fault controls.
- Requires production operators to provide persistent storage, node-level
  fencing, monitored backups, tested restores, sufficient disk capacity, and a
  maintenance window for node replacement or recovery.
- Does not change the public RxDB, auth, policy, management, or edge-function
  APIs, but reduces initial production availability and scale expectations to a
  documented single-node storage service.
