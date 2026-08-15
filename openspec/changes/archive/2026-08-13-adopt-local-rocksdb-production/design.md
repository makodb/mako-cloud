## Context

See `proposal.md` for motivation and
`specs/storage/production-rocksdb/spec.md` for the behavioral contract. Mako
already has a semantic `KvAdapter`, a deterministic memory implementation, and
a RocksDB optimistic-transaction implementation with synchronous durability, snapshots,
bounded scans, atomic batches, and conditional writes. It also has a
vendor-neutral distributed connector boundary, but no production connector,
vendor dependency, endpoint, credentials, or failure controls.

RocksDB permits one database owner at a path. It is not a shared network
service, so production stateful services cannot scale by opening the same files
from multiple processes or hosts. Tenant isolation remains logical inside each
database through the existing encoded project/environment keyspaces.

## Goals / Non-Goals

**Goals:**

- Make the existing RocksDB optimistic-transaction implementation the sole production
  `KvAdapter` selection.
- Give every stateful service exclusive ownership of a persistent database
  volume and make missing, locked, corrupt, or unsafe storage fail closed.
- Preserve synchronous acknowledgements and the existing atomicity, snapshot,
  change-log, idempotency, and sequencer invariants.
- Provide verifiable checkpoint backup, empty-target restore, crash recovery,
  capacity monitoring, and node-replacement procedures.
- Replace unattainable distributed-adapter release gates with qualification of
  the topology the product actually supports.

**Non-Goals:**

- Automatic failover, active-active or multi-writer storage, shared RocksDB
  files, horizontal storage sharding, or zero-downtime stateful replacement.
- Rebuilding replication, consensus, remote block storage, or a RocksDB network
  protocol inside Mako.
- Changing public RxDB, auth, policy, management, or function APIs.
- Providing application-level point-in-time restore in the first version; the
  unit of restore is an owned service database checkpoint.

## Decisions

### 1. Use one exclusively owned RocksDB volume per stateful service instance

Each production process that owns key-value state receives one explicit
persistent path and is deployed with one stateful replica for that database.
Different stateful services use different paths and volumes; they never open a
common RocksDB directory. Stateless gateways may still have multiple replicas,
and they reach the owning stateful service through its existing service API.

This matches RocksDB's locking and process model while preserving the existing
service boundaries. Consolidating every stateful component into one monolith
would reduce the number of volumes but would unnecessarily couple their release
and failure domains. Mounting one RocksDB directory on a shared filesystem or
opening it from multiple processes is rejected because file locking is not a
distributed consistency protocol.

### 2. Keep `KvAdapter`, but remove the production distributed connector layer

Higher layers continue depending on `KvAdapter`, which remains useful for
deterministic tests, fault injection, and keeping storage semantics explicit.
Production startup constructs `RocksDbAdapter` directly. The memory adapter is
test-only. Vendor-facing connector types, remote connection configuration,
adapter credentials, and distributed identity/capability selection are removed
once call-site inspection confirms they have no remaining test utility.

Keeping dormant distributed production types would leave misleading
configuration and readiness branches. Removing the entire semantic trait would
instead couple domain code to RocksDB APIs and discard the conformance and
failure-injection test boundary, so that alternative is rejected.

### 3. Make production storage configuration narrow and fail-fast

Typed production configuration contains an explicit database path, bounded
batch/scan limits, transaction lock and expiration timeouts, backup destination
reference, retention policy, and capacity thresholds. It has no backend kind,
remote endpoint, namespace, or connection secret. Production validation rejects
empty or known-ephemeral paths, relative paths, durability below `Sync`, and
unsafe zero or unbounded limits.

The deployed volume supplies encryption at rest and access control; Mako does
not put volume credentials in RocksDB configuration. Development may keep its
current explicit local directories and memory-backed tests, but a production
open failure never falls back to either.

### 4. Gate readiness on exclusive open and semantic verification

Startup opens `OptimisticTransactionDB` with create-if-missing controlled by provisioning,
fsync, paranoid checks, WAL tracking, and the configured transaction limits. A
provisioned volume carries a non-secret ownership marker containing service and
format identity. Startup checks that marker before creating or opening data.

After open, the existing readiness/conformance logic verifies capabilities and
health using a reserved system keyspace. The service remains unready on a lock,
I/O, capacity, integrity, marker, or semantic failure. It never renames the bad
path or silently initializes a replacement. This favors a visible outage over
acknowledging writes into an empty database.

### 5. Back up through RocksDB checkpoints with a signed inventory manifest

The storage crate gains an operations boundary that creates a RocksDB checkpoint
in a staging directory, closes the staging handle, computes file sizes and
digests, and emits a versioned manifest. The manifest records the owning
service, source database identity and format, creation time, backup identifier,
tenant keyspace inventory, sequencer high-water evidence where applicable, and
every checkpoint file digest.

The deployment's backup transport copies the immutable checkpoint and manifest
to encrypted, access-controlled backup storage. A backup becomes successful
only after the stored copy is read back and verified. Copying a live database
directory with ordinary filesystem tools is unsupported because it can produce
an incoherent artifact. Backup encryption and retention are deployment
responsibilities, while manifest correctness and restore eligibility are Mako
responsibilities.

The Rust RocksDB binding exposes native checkpoints for
`OptimisticTransactionDB` but not its pessimistic `TransactionDB` wrapper. The
implementation therefore uses optimistic transactions with snapshot-based
`get_for_update` conflict detection. The shared conditional-race and atomicity
suite remains the acceptance contract; this choice also keeps checkpoint
creation on the live acknowledged database instead of copying files or omitting
unflushed WAL state.

The initial manifest may use a deployment-held signing key or an authenticated
digest envelope already available to operators. The exact key provider is a
configuration integration detail; tests use deterministic signing material and
never embed a production key.

### 6. Restore only into an empty, offline target

The restore command first validates manifest authenticity, format compatibility,
file inventory and digests. It copies to a newly provisioned empty path, opens
the database offline, runs integrity and tenant-keyspace scans, recovers the
sequencer, and proves every position through the recorded acknowledged high
water is committed or aborted. Only a successful verification writes the
ownership marker that permits normal startup.

The source service is fenced before a restored replacement is promoted. A
failed restore remains isolated and never changes the active volume. In-place
restore is rejected because partial replacement makes rollback and evidence
ambiguous.

### 7. Treat node replacement as an operator-controlled outage

An intact volume may be reattached to one fenced replacement node. If it is
lost, operators restore the newest eligible backup, inspect the recovery report,
and explicitly promote the new owner. Deployment status and documentation state
that writes are unavailable during this process. No health check may promote a
blank volume merely because it opens successfully.

This is less available than a distributed backend but is the honest behavior of
the selected storage topology. Adding automated failover without a replicated
durability protocol would risk split brain and acknowledged-data loss.

### 8. Qualify and observe local production behavior

Qualification runs the shared semantic suite against RocksDB plus synchronous
restart durability, crash points, I/O failure, disk-full simulation, lock
contention, compaction/retention load, backup corruption, restore, tenant
inventory, and acknowledged-high-water tests. Performance runs on representative
persistent storage and records capacity and latency rather than reusing the
in-memory adapter.

Metrics and alerts cover open and lock state, disk bytes/free space, write
stalls, compaction backlog, I/O errors, corruption, backup result/age, restore
result, and sequencer recovery. Release gates reference these results and no
longer reference a distributed adapter or distributed-network fault suite.

### 9. Reconcile the active MVP change as part of implementation

`build-mako-cloud-mvp` is still active, so its proposal, document-engine delta,
design, task list, traceability matrix, qualification documents, and release
blockers still describe a production distributed adapter. Applying this change
updates those planning artifacts coherently: generic semantic-adapter behavior
remains where it describes the internal trait, while production-distributed
requirements and tasks are removed or replaced by local RocksDB production and
backup/restore work. This avoids completing the code while leaving the active
source plan contradictory.

## Risks / Trade-offs

- **[A host or volume failure causes a write outage]** → State the single-node
  availability contract, monitor backup age, keep restore drills current, and
  require explicit promotion of a verified replacement.
- **[The newest acknowledged writes can be newer than the latest backup]** →
  Define and measure recovery-point objectives, run frequent checkpoints, and
  never claim that restored high water exceeds the manifest's recorded value.
- **[A stateful service is accidentally scaled above one replica]** → Enforce
  one replica in deployment validation and rely on RocksDB locking and ownership
  markers as a second fail-closed boundary.
- **[An ephemeral or wrong volume looks like an empty valid database]** → Require
  an explicit provisioned ownership marker and reject production auto-creation
  except in the provisioning workflow.
- **[Disk exhaustion stalls or corrupts service behavior]** → Reserve capacity,
  alert before critical thresholds, bound compaction work, and stop mutations on
  critical health or integrity failures.
- **[Backups exist but cannot be restored]** → Verify every artifact after copy,
  run scheduled destructive restore drills in isolated paths, and use drill
  results as release evidence.
- **[Removing distributed types makes a later migration more work]** → Preserve
  the semantic `KvAdapter` and conformance suite; a future distributed backend
  returns as a separate evidence-backed change rather than dormant production
  configuration.

## Migration Plan

1. Reconcile the active MVP artifacts and release evidence so local RocksDB is
   the declared production topology and distributed tasks are no longer gates.
2. Remove distributed connector/configuration exports and production selection;
   keep the memory and RocksDB implementations behind `KvAdapter`.
3. Add strict production RocksDB configuration, ownership markers, direct
   startup wiring, exclusive-volume deployment validation, and fail-closed
   readiness.
4. Add checkpoint/manifest creation, verified backup transport hooks, offline
   empty-target restore, integrity scans, and acknowledged-high-water reports.
5. Add metrics, alerts, operator commands, runbooks, capacity procedures, and
   explicit single-node availability documentation.
6. Run local production conformance, crash/restart, disk/I/O, backup/restore,
   tenant-isolation, soak, and performance qualification on representative
   persistent storage.
7. Deploy one internal stateful instance per database with new persistent
   volumes, exercise backup and restore, then admit traffic only after release
   gates pass.

There is no distributed production dataset to migrate. Existing local RocksDB
development data remains readable because the document and key encodings do not
change. Before rollout, take and verify a checkpoint of any environment being
promoted to the production configuration.

Rollback retains the prior binary's RocksDB configuration and data format.
Fence writes, take a verified checkpoint, restore the previous deployment with
the same owned volume, and re-run readiness before traffic resumes. Do not roll
back by selecting an empty memory or distributed backend.
