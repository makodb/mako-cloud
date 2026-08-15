# Production RocksDB deployment and operations

## Supported topology and availability contract

Each stateful service owns one local RocksDB database on one retained,
encrypted, expandable persistent volume. `mako-data-plane` and
`mako-control-plane` each run with exactly one replica and distinct
`ReadWriteOnce` claims. Stateless gateways may scale independently.

This initial topology has a single storage writer and no automatic failover,
active-active writes, shared RocksDB directory, replicated durability, or
zero-downtime storage-node replacement. A process restart on an intact volume
causes a brief write outage. A lost node or volume causes a write outage until
an operator restores and explicitly promotes a verified checkpoint. Never scale
a stateful owner above one replica or mount its database claim in two pods.

The initial objectives are:

- recovery point: newest eligible verified backup no more than 15 minutes old;
- same-volume recovery: ready within 5 minutes;
- replacement-volume restore and explicit promotion: ready within 30 minutes.

These are release thresholds, not claims of replication. A restored environment
can lose acknowledged writes newer than the selected backup's manifest high
water. Qualification reports must record measured RPO and RTO.

## Deployment and provisioning

`infra/production/storage-statefulsets.yaml` is the checked topology template.
Replace its example CSI provisioner and image with deployment-owned values, but
retain encrypted storage, `reclaimPolicy: Retain`, volume expansion,
`ReadWriteOnce`, claim retention, and `replicas: 1`. Run
`npm run validate:production-storage` after rendering changes.

The init container explicitly provisions an empty volume. Manually, use:

```bash
mako-storage-ops provision \
  --database-path=/var/lib/mako/rocksdb \
  --service=mako-data-plane \
  --database-id=mako-data-plane-us-east-1 \
  --confirm=PROVISION
```

Use `--dry-run` first. `--accept-matching-marker` makes deployment initialization
idempotent only when owner and format match; it never accepts an unmarked,
non-empty, wrong-owner, or wrong-format path. Startup remains unready if the
volume is absent, locked, corrupt, read-only, below its critical reserve, or
fails sequencer/semantic recovery. It never initializes a fallback database.

## Capacity expansion

Alert on the configured warning reserve and stop discretionary writes before the
critical reserve. Check pending compaction bytes, delayed write rate, and backup
headroom together. Expand the existing retained claim through the CSI provider;
do not replace its path. Confirm the filesystem sees the expansion, the critical
alert clears, compaction pressure drains, and a new verified backup completes.
Do not delete SST or WAL files manually.

## Graceful shutdown and same-volume restart

Remove the owner from routing and wait for in-flight mutations. Let the service
complete synchronous writes and graceful shutdown before terminating the pod.
Reattach the same claim to exactly one replacement. Readiness waits for RocksDB
recovery, ownership/format checks, semantic checks, sequencer gap recovery, and
acknowledged-high-water verification. If the old process may still run, fence it
at the orchestrator and node level before attaching the volume.

## Checkpoint backup

The deployment supplies at least 32 bytes of signing material in a protected
file and encryption/access control for the backup destination. The key is not a
command argument and never appears in manifests or diagnostics.

```bash
mako-storage-ops backup \
  --database-path=/var/lib/mako/rocksdb \
  --service=mako-data-plane \
  --database-id=mako-data-plane-us-east-1 \
  --backup-id=dp-20260806T120000Z \
  --staging-root=/var/lib/mako-backup-staging \
  --destination=/var/lib/mako-backups/data-plane \
  --retention=14 \
  --signing-key-file=/run/secrets/mako-backup-signing-key
```

Success means the immutable uploaded copy was read back, authenticated, and all
file digests verified. Monitor backup failures and age. Inspect or verify without
opening a database:

```bash
mako-storage-ops inspect --artifact=/var/lib/mako-backups/data-plane/DP_ID --signing-key-file=/run/secrets/mako-backup-signing-key
mako-storage-ops verify --artifact=/var/lib/mako-backups/data-plane/DP_ID --signing-key-file=/run/secrets/mako-backup-signing-key
```

## Empty-target restore and node replacement

Fence and stop the original owner. Provision an offline, existing, empty target;
never restore in place. Run a dry run and then confirm:

```bash
mako-storage-ops restore \
  --artifact=/var/lib/mako-backups/data-plane/DP_ID \
  --target=/var/lib/mako/replacement-rocksdb \
  --service=mako-data-plane \
  --database-id=mako-data-plane-us-east-1 \
  --maximum-age-seconds=900 \
  --signing-key-file=/run/secrets/mako-backup-signing-key \
  --dry-run

mako-storage-ops restore ...same-options... --confirm=RESTORE
```

Restore authenticates the manifest, validates every file, opens a private staged
database, checks tenant inventory, and recovers each sequencer through the
manifest high water. It then writes a `Promotable` marker. It cannot serve until
the old owner is fenced and an operator runs:

```bash
mako-storage-ops promote \
  --database-path=/var/lib/mako/replacement-rocksdb \
  --service=mako-data-plane \
  --database-id=mako-data-plane-us-east-1 \
  --confirm=PROMOTE
```

Attach the replacement to one pod and wait for full readiness. Preserve the old
volume and failed staging paths for investigation; never point traffic at a
blank replacement merely because RocksDB can create it.

## Corruption response and rollback

On a corruption or repeated I/O signal, stop routing and mutations, fence the
owner, preserve logs and the volume, and do not run repair commands against the
only copy. Verify the newest backup and restore into an isolated empty volume.
Escalate possible tenant-boundary mismatches as a security incident.

For binary rollback, stop writes, take and verify a checkpoint, and confirm the
previous binary supports the recorded database format. Restart it against the
same fenced volume and require readiness before traffic. If the volume cannot be
used, restore a backup produced by the compatible format, verify, and explicitly
promote. Never roll back by selecting memory storage or an empty database.
