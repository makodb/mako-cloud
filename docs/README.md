# Mako Cloud documentation

These guides describe the tested MVP surface. The OpenAPI document is the
authoritative public wire contract, and the checked Rust and TypeScript types
are authoritative for internal boundaries. Run `npm run validate:docs` after
changing this index or a published guide.

## Start here

| Area | Guide | Primary tested flow |
| --- | --- | --- |
| Local development | [Local development](local-development.md) and [configuration](configuration.md) | Workspace preparation, typed configuration, retained RocksDB directories, and local dependencies |
| Deployment | [Deployment](deployment.md) | Mixed SQLite/RocksDB volumes, fail-closed startup, readiness, graceful shutdown, and qualification |
| Operations | [Control-plane SQLite](control-plane-sqlite.md), [Production RocksDB operations](production-rocksdb-operations.md), [observability](observability.md), [rollback qualification](rollback-qualification.md), and [runbooks](runbooks/README.md) | Backup, restore, rollback, capacity, recovery, alerts, and incident response |
| Security | [Threat model](threat-model.md) and [qualification reports](#qualification-evidence) | Tenant boundaries, auth, policies, edge isolation, and dependency auditing |
| Developer identity | [Developer registration and wait list](developer-registration.md) | Self-registration, verification, pending isolation, operator review, recovery, and durable mail |
| End-to-end smoke | [Smoke verification](e2e-smoke.md) | Local tenant bootstrap, then sign-up, sign-in, push, and pull against the real service binaries |
| Public API | [API guide](api.md) | Versioned OpenAPI, generated clients, identity domains, errors, and idempotency |
| RxDB | [RxDB client](rxdb-client.md) and [reference application](../examples/local-first/README.md) | Pull, push, live SSE, conflicts, tombstones, refresh, resets, and full resync |
| Developer data | [Developer data workspace](developer-data-workspace.md) | Explorer grants, policy preview, administration, jobs, Connect, sync diagnostics, and isolated recovery |
| Document policies | [Policy guide](document-policies.md) | Default deny, state-aware writes, visibility changes, activation, and rollback |
| Project auth | [Authentication guide](project-auth.md) | Sign-up, sign-in, refresh rotation, revocation, JWKS rotation, and administration |
| Edge functions | [Hosted edge functions](edge-functions.md), [local serve](local-functions.md), and [runtime protocol](edge-runtime-protocol.md) | Bundle, deploy, promote, invoke, roll back, isolate, limit, and observe |

## Storage and reliability

- [Control-plane SQLite authority](control-plane-sqlite.md)
- [Production RocksDB architecture](production-rocksdb-architecture.md)
- [Storage readiness](storage-readiness.md)
- [Storage fault model](storage-fault-model.md)
- [Retention jobs](retention-jobs.md)
- [Production qualification](production-rocksdb-qualification.md)
- [Performance baseline](performance-baseline.md)
- [Release gates](release-gates.md)
- [Rollback qualification](rollback-qualification.md)

## Qualification evidence

- [Requirements traceability](requirements-traceability.md)
- [Authentication security](auth-security-qualification.md)
- [Policy security](policy-security-qualification.md)
- [RxDB chaos](rxdb-chaos-qualification.md)
- [Tenant boundaries](tenant-boundary-qualification.md)
- [Edge security](edge-security-qualification.md)
- [Storage soak](storage-soak-qualification.md)
- [Rollback qualification](rollback-qualification.md)

Qualification reports describe the host and scope they exercised. A passing
local report does not silently qualify a different cloud storage class,
container runtime, region, or cost model.

The concrete single-VM beta procedure is the
[public-beta environment runbook](runbooks/public-beta-environment.md). Its
public origin is `https://cloud-test.makodb.com`; availability, certificate,
measurement-window, and operator-approval gates remain explicit even when the
private services are healthy.
