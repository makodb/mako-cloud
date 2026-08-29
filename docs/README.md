# Mako Cloud documentation

These guides describe the tested MVP surface. The OpenAPI document is the
authoritative public wire contract, and the checked Rust and TypeScript types
are authoritative for internal boundaries. Run `npm run validate:docs` after
changing this index or a published guide.

## Start here

| Area | Guide | Primary tested flow |
| --- | --- | --- |
| Local development | [Local development](local-development.md) and [configuration](configuration.md) | Workspace preparation, typed configuration, retained RocksDB directories, and local dependencies |
| Billing | [Billing](billing.md) | Meters, plans, exceptions, credits, the informational bill, and the guard proving nothing collects |
| Deployment | [Deployment](deployment.md) | Mixed SQLite/RocksDB volumes, fail-closed startup, readiness, graceful shutdown, and qualification |
| Operations | [Control-plane SQLite](control-plane-sqlite.md), [Production RocksDB operations](production-rocksdb-operations.md), [observability](observability.md), [rollback qualification](rollback-qualification.md), and [runbooks](runbooks/README.md) | Backup, restore, rollback, capacity, recovery, alerts, and incident response |
| Security | [Threat model](threat-model.md) and [qualification reports](#qualification-evidence) | Tenant boundaries, auth, policies, edge isolation, and dependency auditing |
| Developer identity | [Developer registration and wait list](developer-registration.md) | Self-registration, verification, pending isolation, operator review, recovery, and durable mail |
| End-to-end smoke | [Smoke verification](e2e-smoke.md) | Local tenant bootstrap, then sign-up, sign-in, push, and pull against the real service binaries |
| Public API | [API guide](api.md) | Versioned OpenAPI, generated clients, identity domains, errors, and idempotency |
| RxDB | [RxDB client](rxdb-client.md) and [reference application](../examples/local-first/README.md) | Pull, push, live SSE, conflicts, tombstones, refresh, resets, and full resync |
| Developer console | [Developer console](developer-console.md) | Home dashboard, onboarding, project home, the environment shell, and the areas shown as not yet available |
| Developer data | [Developer data workspace](developer-data-workspace.md) | Explorer grants, policy preview, administration, jobs, Connect, sync diagnostics, and isolated recovery |
| Document policies | [Policy guide](document-policies.md) | Default deny, state-aware writes, visibility changes, activation, and rollback |
| Project auth | [Authentication guide](project-auth.md) | Sign-up, sign-in, refresh rotation, revocation, JWKS rotation, and administration |
| Edge functions | [Hosted edge functions](edge-functions.md), [local serve](local-functions.md), and [runtime protocol](edge-runtime-protocol.md) | Bundle, deploy, promote, invoke, roll back, isolate, limit, and observe |
| Developer CLI | [Developer CLI](cli.md) | Sign in from a terminal, reach every console operation, script-stable output and exit codes, composed deploy and data flows |
| Application file storage | [Application file storage](file-storage.md) | Buckets, policy-governed objects, encryption at rest, metering, limits |
| Application mail | [Application mail](application-mail.md) | Mail intents drained from the data plane, per-environment plain-text templates with allowlisted variables, the encrypted outbox, and plaintext SMTP for local relays |
| Scheduled functions | [Scheduled functions](scheduled-functions.md) | Cron schedules in UTC, the configured request, the next run, overlap skipping and the lease, missed due times, run-now, run history and its retention, and how a scheduled invocation looks to the function |
| Database webhooks | [Database webhooks](webhooks.md) | Registering endpoints, the secret shown once, the signed delivery body and its verification, retries, the retry window, pause and resume, redelivery, and retention |
| Custom domains | [Custom domains](custom-domains.md) | Adding a domain, the TXT verification record, the check cadence and the two-check failure rule, what a domain serves and what it never serves, on-demand certificates and the ask gate, and removal |
| Application sign-in providers | [Sign-in providers and magic links](auth-providers.md) | Google, GitHub, and OpenID Connect providers, the redirect allowlist, the start, callback, and exchange flow, magic links, and sealed secrets never returned |

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
