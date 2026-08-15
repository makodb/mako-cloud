# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Mako Cloud is an RxDB-native application backend: project authentication, document-level policies, Supabase-style edge functions, and a cloud management plane. Production state lives in exclusively-owned local databases on a single node — MongoDB/SQL compatibility, active-active writes, and horizontal scaling of one database are explicitly out of scope.

The repo is a dual workspace: a Cargo workspace (`crates/*`, `services/*`) for the backend, and an npm workspace (`packages/*`, `apps/*`, `examples/*`) for SDKs, CLI, and the console.

## Commands

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --lib --bins        # unit tests (mostly inline #[cfg(test)])
cargo test --workspace --tests             # integration targets under crates/*/tests/
cargo test -p mako-gateway quota::tests::hard_limits_rate_limits_and_retry_advice_are_stable
cargo test -p mako-gateway -- --list       # inline tests sit in a `tests` submodule per file

# TypeScript
npm run format:check && npm run lint && npm run typecheck
npm run test:unit
npm run test:integration
npm run test:unit -w @mako-cloud/cli              # one workspace
npm run test:e2e -w @mako-cloud/console           # Playwright

# Generated API types — must be regenerated when the OpenAPI doc changes
npm run generate:api
npm run generate:api:check
```

TypeScript unit tests are `node --test` over built output, so their scripts run `npm run build` first. To run a single file, build the workspace once and then `node --test packages/cli/test/<name>.test.mjs`. Note that no workspace defines a plain `test` script, so root `npm test` currently matches nothing — use `test:unit` / `test:integration`, as CI does.

### Local run loop

```bash
./scripts/local/prepare.sh                          # creates .local/data + .local/certs (ignored)
cp .env.example .env
docker compose -f infra/local/compose.yaml up -d    # mailpit, object store, otel, prometheus, grafana
cargo run --bin mako-data-plane                     # or mako-control-plane / mako-edge-gateway / mako-telemetry-query
```

`ServiceConfig` reads the **process** environment — nothing auto-loads `.env`, so export it yourself (`set -a; . ./.env; set +a`). All local endpoints bind loopback only. See `docs/local-development.md` and `docs/configuration.md`.

### Validators that gate CI

`npm run validate:docs`, `validate:traceability`, `validate:release-gates`, `validate:rollback`, plus the public-beta infrastructure set (`validate:public-beta-*`, `test:proxmox-*`, `scan:public-beta-secrets`). Run the relevant one after editing docs, spec scenarios, or `infra/`. The heavier `test:*-qualification` scripts are evidence-producing suites, not part of the normal edit loop.

## Architecture

### crates vs services

`crates/*` hold domain logic with no HTTP transport; `services/*` are the deployable binaries that compose them. Domain crates are where the behavior and most tests live — `mako-control-plane` (management, operator, developer lifecycle), `mako-identity`, `mako-documents`, `mako-policy`, `mako-sync` (RxDB pull/push/checkpoint/stream), `mako-storage`, `mako-gateway` (quota, token verification, revocation), `mako-provisioning`, `mako-audit`, `mako-object-store`, `mako-edge-gateway`, `mako-edge-runtime`.

Each service follows the same shape:

- `graph.rs` — a `*Graph` that opens storage and dependencies, and reports `readiness()`
- `*_http.rs` modules — each registers routes onto a shared `HttpRouter`
- `lib.rs` — `*_router()` composes those modules
- `main.rs` — load config → open graph → **refuse to start unless readiness passes** → serve

Adding an endpoint means: domain logic in the crate, a route module in the service, an entry in `api/openapi/mako-cloud-v1.yaml`, regenerated types.

### No async runtime

There is no tokio, hyper, or axum. `mako-service-runtime` is a bounded, fail-closed HTTP transport built on `tiny_http` and OS threads, with explicit caps on in-flight requests, body size, header count, and shutdown grace. `futures::executor::block_on` bridges the few async domain APIs. Do not introduce an async runtime or web framework to solve a local problem — extend the transport instead. `unsafe_code` is `forbid` workspace-wide.

### Storage split

Two engines, deliberately, and the split is not configurable:

- **Control plane → server-side SQLite** (WAL, full synchronous, one process holding an external lock). Owns developer credentials and lifecycle, wait-list, operator entitlements and sessions, organizations, projects, provisioning, control audit, idempotency, mail outbox, function metadata.
- **Data plane, edge gateway, telemetry → local RocksDB** `OptimisticTransactionDB`, one process per database directory. Owns application users, tenant credentials and signing keys, documents, policies, indexes, RxDB sync state.

Both sit behind `mako-storage`'s `KvAdapter`/`KvSnapshot` boundary over an opaque byte key/value encoding, with a deterministic `MemoryAdapter` for tests. There is intentionally no backend selector, remote connector, or storage credential in configuration — a new production backend requires a new change, not a config flag.

Services talk to each other over `mako-internal-rpc`: authenticated, versioned, loopback-only, with replay guards and an encrypted response journal.

### Contracts

`api/openapi/mako-cloud-v1.yaml` is authoritative for the public wire; checked Rust and TypeScript types are authoritative for internal boundaries. `packages/api-types/src/generated/schema.ts` is generated — never hand-edit it (Biome ignores it). Identity domains are strictly separated: management (developer session / automation token), project auth (application-user session scoped to one project+environment), `/service/` (scoped secret credential), `/v1/operator/` (separate operator identity, unreachable from application or developer tokens), `/v1/developer-auth/` (hosted registration, with wait-list tokens in a dedicated audience). Tenant identity comes from verified credentials and must match every `projectId`/`environmentId` path parameter — never infer authorization from a document field or a public key.

## Spec-driven workflow (OpenSpec)

This project uses OpenSpec (`openspec/`, schema `spec-driven`). Capability specs live under `openspec/specs/<capability>/`; work is proposed as a change under `openspec/changes/<name>/` with `proposal.md`, `design.md`, `tasks.md`, and spec deltas, then archived after apply. Skills are installed for both Claude Code (`.claude/skills/`) and Codex (`.agents/skills/`): `/openspec-propose`, `-explore`, `-update-change`, `-apply-change`, `-archive-change`, `-sync-specs`.

Proposing is planning-only — the propose workflow does not touch project code, and implementation waits for an explicit apply.

`docs/requirements-traceability.md` maps every spec scenario to its primary automated test; `npm run validate:traceability` enforces one-to-one coverage and that cited test files exist. When you add or rename a scenario or its test, update that matrix.

## Conventions worth knowing

- **Fail closed.** Startup refuses on mismatched database identity, wrong schema version, unsafe or symlinked paths, integrity failure, critical disk pressure, or incomplete migration. Readiness gates serving; degraded dependencies produce scoped errors rather than silent fallbacks. Preserve this when touching startup or readiness paths.
- **Secrets are references, never inline values** — `env:NAME` or `file:/path`. Resolved values are redacted from `Debug`/`Display` and startup summaries.
- **Config errors are addressed**: stable code + field path + non-sensitive explanation (`configuration error CONFIG_INVALID_VALUE at server.bind_address: ...`).
- **Docs are part of the change.** `docs/README.md` is the index and `npm run validate:docs` checks it. Qualification reports state the host and scope they exercised — do not generalize a passing local report to another environment, and do not relax a release gate in `docs/release-gates.json` as a side effect.
- Rust tests are predominantly inline `#[cfg(test)]` next to the code, with `crates/*/tests/` reserved for conformance, fault-injection, crash/recovery, and property suites (`mako-storage`, `mako-documents`, `mako-sync`, `mako-audit`).
- Biome formats JS/TS/JSON at 100 columns, double quotes, semicolons, trailing commas. TypeScript is strict with `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, and `verbatimModuleSyntax`.
- Node >= 24, npm >= 11, Rust 1.97 (edition 2024).
