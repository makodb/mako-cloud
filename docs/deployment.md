# Deployment

Mako Cloud separates the control plane, data plane, and edge-function plane.
The control plane owns one local SQLite database; data-plane, edge-gateway, and
telemetry state remain in separate RocksDB databases. Every stateful database
has one process and one retained persistent volume. Stateless gateways and edge
workers may scale independently, but no two processes may open one database.

## Prerequisites

- Immutable service images built from one validated revision.
- TLS for public endpoints and authenticated private service traffic.
- A deployment secret reference for internal service authentication.
- One encrypted, retained, `ReadWriteOnce` volume per stateful service.
- A separate encrypted backup destination with retention and access controls.
- A pinned, qualified edge-runtime image digest.
- Prometheus-compatible metrics, alert routing, and access-controlled logs.

[`infra/production/storage-statefulsets.yaml`](../infra/production/storage-statefulsets.yaml)
is the checked storage-topology example. Its CSI provisioner and image names are
placeholders; replace them with deployment-owned values. Do not apply the file
unchanged to a cluster.

## Stateful rollout

1. Provision each RocksDB retained volume with `mako-storage-ops provision`, the
   exact service identity, and explicit `PROVISION` confirmation. Provision the
   control SQLite volume and private directories separately; production control
   startup requires a pre-created or verified migrated SQLite authority.
2. Mount the database and backup paths at the absolute paths in typed service
   configuration. Never select a memory backend or create a database on failed
   open.
3. Start exactly one replica. Readiness remains false until the ownership and
   format markers, exclusive lock, capacity reserve, synchronous durability,
   engine health, semantic contract, and acknowledged high water are verified.
   SQLite additionally requires its application/database identity, supported
   schema, integrity, WAL/capacity limits, and exclusive lock.
4. Admit traffic only after the service and gateway readiness checks pass.
5. Take and verify a checkpoint backup before and after a storage-affecting
   rollout.

Use a graceful termination window long enough to stop admission, drain bounded
work, checkpoint SQLite or flush RocksDB, and close the engine. Keep the volume claim when a
StatefulSet is deleted or scaled. Scaling a stateful owner above one replica or
mounting its files `ReadWriteMany` is invalid.

## Stateless and edge rollout

Gateways can have multiple replicas after their token-verification,
revocation-freshness, quota, and downstream health dependencies are ready.
Function traffic resolves an immutable healthy version before invoking the
pinned runtime. A promotion changes one version pointer atomically; a failed
deployment leaves the prior version active.

Selected edge regions may fail over only to another healthy selected region.
They do not make the stateful RocksDB owner multi-region. Data writes stop while
the owning node or volume is unavailable.

## Configuration and secrets

Follow [service configuration](configuration.md). Production public URLs must
use HTTPS, storage and backup paths must be absolute and separate, and secrets
must use `env:` or `file:` references. Store project signing keys, service
credentials, backup authentication material, and function secrets in a
deployment secret system; never place plaintext values in manifests.

## Release and rollback

Run the [release gates](release-gates.md) for the exact image, storage class,
region, and runtime. Before enabling external traffic, run the
[rollback qualification](rollback-qualification.md). Storage recovery uses the
same fenced volume or an authenticated empty-target restore followed by
explicit promotion; an empty replacement must never become ready.

Developer-registration releases are installed with registration disabled.
Apply and verify the identity migration, existing active access,
backup/restore, authenticated SMTP, pending isolation, operator review, and a
fresh active sign-in before enabling it. Preserve additive identity and
mail-outbox keyspaces during rollback. See the
[registration runbook](runbooks/developer-registration-and-mail.md).

## Tested evidence

- `npm run validate:production-storage` checks two distinct single-replica
  retained volumes and rejects shared claims or scaled owners.
- Production startup and recovery are exercised in
  `crates/mako-storage/tests/production_startup.rs`.
- Backup, restore, promotion, and corruption rejection are exercised in
  `crates/mako-storage/tests/production_backup_restore.rs`.
- The retained topology measurements are in
  [production qualification](production-rocksdb-qualification.md).

## Tested public-beta VM flow

The current beta target is the single Ubuntu 24.04 VM `124` at
`130.245.173.11`, with the exact public origin
`https://cloud-test.makodb.com`. It uses a service-owned control SQLite path and
distinct service-owned tenant RocksDB paths on the VM data disk. Authenticated
SQLite backups and signed RocksDB checkpoints are copied to a destination outside
the VM failure domain. This deployment is not a Kubernetes or distributed-KV
deployment and does not provide HA.

Run `npm run validate:public-beta-local` before any live operation. That command
is offline and deliberately separates validation from the read-only Proxmox
planner, hash-bound VM apply, public-admission approval, and non-executing
teardown planner. Build the candidate with
`npm run build:public-beta-release`, select its digest in
`infra/ansible/group_vars/public_beta.yml`, install it immutably, and converge
the pinned Ansible playbook with Caddy disabled. A running stateful environment
changes releases only through `mako-release-operation upgrade` or `rollback`;
it must not overwrite `/opt/mako/current` or create an empty storage fallback.
After SQLite accepts control writes, release selection rejects binaries that do
not declare compatibility with the active control SQLite format.

The reviewed environment procedure is the
[public-beta environment runbook](runbooks/public-beta-environment.md), and
retained machine-readable deployment evidence is under `docs/evidence/` with
the `public-beta-` prefix. Public HTTPS remains a separate gate: a deployed and
ready VM is not approval to enable ingress. A persistent
`risk_accepted_preview` may expose only the documented waivable blockers while
the qualified-beta gate remains blocked; it requires all non-waivable
safeguards, exact release/plan/blocker binding, automatic fallback, an explicit
manual pause, and a fresh approval after any fallback or binding change.
