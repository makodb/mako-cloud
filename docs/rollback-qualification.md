# Rollback qualification

Mako Cloud exercised six rollback paths locally on August 7, 2026. The
machine-readable result is in [rollback-qualification.json](rollback-qualification.json),
and `npm run test:rollback` reproduces the evidence. This proves the checked
state transitions and storage invariants; it does not qualify a cloud cluster,
storage class, image registry, or operator team. The single-region beta gate
therefore remains blocked until the same release is exercised in its deployment
environment under operator observation.

Before every rollback, stop new admission, preserve request and audit context,
identify the last-known-good immutable version, and change one dimension at a
time. Stateful rollback additionally requires a verified checkpoint, exclusive
volume fencing, known format compatibility, and readiness before traffic.

## Service

Roll back every container for a service to the same previous immutable image.
For a stateful service, stop writes and fence the current owner first. Preserve
the one-replica topology, retained claim, service identity, database marker,
RocksDB path, and backup destination. Wait for semantic recovery and readiness;
never use an empty volume as a shortcut. If desired-state provisioning fails,
compensate completed components, retain the safe diagnostic, then retry the
same workflow idempotently.

The qualification applies a previous image to the checked StatefulSets in
memory, asserts that only the four image references changed, and tests
provisioning compensation and resume. A real rollout is still required for beta.

## Policy

Select a previously validated immutable policy version through the authorized
rollback operation. Activation is atomic and advances the authorization epoch,
forcing stale authorization state to be discarded. Do not edit policy history
in place or activate an invalid draft. Verify a representative deny and allow
case after rollback and inspect the audit event.

## Function

Point the function atomically at a previous immutable version that is still
healthy. A failed deployment must not replace the active version, and a version
that later fails health checks must become ineligible for promotion. Test invoke
the selected version, check its secret-version bindings, and inspect sanitized
logs before restoring traffic.

## Schema

Schema recovery is compatibility-first, not a destructive document downgrade.
An incompatible publication remains inactive and returns a migration-required
outcome, leaving the last compatible schema active. Roll application traffic
back to that schema or execute a reviewed forward migration. Never discard
fields or rewrite stored documents merely to reduce a schema version.

## Signing key

Treat signing-key rollback as a safe roll-forward. Generate and activate fresh
encrypted private material, publish both the new and prior public keys during
the maximum token lifetime, and retire the superseded public key only after the
overlap expires. Never reactivate a retired or suspected-compromised private
key. Confirm both JWKS overlap and eventual retirement while keeping private
material out of logs and serialized records.

## Storage adapter

The production adapter is local RocksDB. Stop writes, fence its sole owner, take
and verify a checkpoint, and confirm the previous binary supports the recorded
format. Open that previous binary on the same non-empty retained volume, require
full readiness, and then resume traffic. If the volume is unusable, restore a
verified compatible backup to an empty target and explicitly promote it. Never
fall back to memory storage, a different adapter, or a newly created path.

## Reproduce

Run from the repository root:

```bash
npm run test:rollback
```

For a candidate beta deployment, repeat these procedures using the exact image,
runtime pin, storage class, configuration, and secrets policy. Record operator,
timestamps, recovery time, failed checks, and the post-rollback audit/readiness
evidence in the release record before removing the beta blocker.

## Public-beta VM procedure and evidence

On VM `124`, first verify Caddy is inactive and the Proxmox
`mako-vm124-admission-stop.service` is active and enabled. Inspect the current
and last-known-good immutable digests with `sudo mako-release-operation inspect`.
Then use exactly one of:

```bash
sudo mako-release-operation rollback DIGEST \
  --confirm=ROLLBACK_RELEASE:DIGEST
sudo mako-release-operation upgrade DIGEST \
  --confirm=UPGRADE_RELEASE:DIGEST
```

The operation drains all four services, pauses backup timers, checkpoints and
fences every RocksDB path as its service owner, rejects incompatible or empty
state, atomically changes `/opt/mako/current`, waits for all private readiness
endpoints, and resumes the timers. If any check fails, public admission remains
off and the operation evidence is retained for diagnosis.

The tested release transition and the service, configuration, policy, function,
schema, signing-key, and storage matrix are in
`docs/evidence/public-beta-release-rollback.json` and
`docs/evidence/public-beta-rollback-matrix.json`. Per-operation records remain
on the guest under `/var/lib/mako-release-operations/`.
