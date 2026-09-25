# The Mako Cloud Dev Book

*How Mako Cloud is built, tested, deployed, operated, and released — for people who change the platform itself.*

This book is for contributors and operators of Mako Cloud: the Rust services and crates, the TypeScript packages and console, the storage engines, the deployment, the security posture, and the release process. If you build applications **on** Mako Cloud, read the [User Book](user-book.md) instead; this book assumes you know what it describes.

The authoritative sources this book explains are the code, [`api/openapi/mako-cloud-v1.yaml`](../api/openapi/mako-cloud-v1.yaml), the OpenSpec capability specs under [`openspec/specs/`](../openspec/specs/), and the machine-checked files that gate a release ([`release-gates.json`](release-gates.json), [`rollback-qualification.json`](rollback-qualification.json), [`production-rocksdb-qualification.json`](production-rocksdb-qualification.json), [`performance-baseline.json`](performance-baseline.json), [`requirements-traceability.md`](requirements-traceability.md), and [`evidence/`](evidence/)). Where prose and those disagree, they win — and the prose should be fixed in the same change.

## How to read this book

- **First day?** [Orientation](#orientation) and [Architecture](#architecture), then [Local development](#local-development).
- **Changing behavior?** [The spec-driven workflow](#the-spec-driven-workflow), [Testing and qualification](#testing-and-qualification), and the subsystem notes in [Architecture](#architecture).
- **Touching storage or startup?** [Storage](#storage) and [Conventions](#conventions) — fail closed is not optional.
- **Deploying or on call?** [Deployment](#deployment), [Operations](#operations), and the [Runbooks](#runbooks).
- **Cutting a release?** [Release engineering](#release-engineering).

## Table of contents

1. [Orientation](#orientation)
2. [Architecture](#architecture)
3. [Storage](#storage)
4. [Local development](#local-development)
5. [Configuration reference](#configuration-reference)
6. [Testing and qualification](#testing-and-qualification)
7. [The spec-driven workflow](#the-spec-driven-workflow)
8. [Conventions](#conventions)
9. [Security](#security)
10. [Deployment](#deployment)
11. [Operations](#operations)
12. [Runbooks](#runbooks)
13. [Release engineering](#release-engineering)
14. [The sample applications as platform gates](#the-sample-applications-as-platform-gates)
15. [The design system](#the-design-system)
16. [Appendix: binaries and tools](#appendix-binaries-and-tools)
17. [Appendix: evidence files](#appendix-evidence-files)
18. [Glossary](#glossary)

---

## Orientation

### What Mako Cloud is, for a contributor

Mako Cloud is an RxDB-native application backend: project authentication, document-level policies, RxDB replication, file storage, Supabase-style edge functions, and a cloud management plane. Production state lives in **exclusively owned local databases on a single node** — a server-side SQLite database for the control plane and local RocksDB databases for the data plane, edge gateway, and telemetry store. MongoDB and SQL compatibility, active-active writes, shared database directories, automatic failover, and horizontal scaling of one database are explicitly out of scope. Everything in this book follows from that choice and from one rule: **fail closed**.

### The repository

The repo is a dual workspace: a Cargo workspace (`crates/*`, `services/*`) for the backend and an npm workspace (`packages/*`, `apps/*`, `examples/*`) for SDKs, CLI, console, and samples.

| Path | What lives there |
| --- | --- |
| `crates/` | Domain logic with no HTTP transport — where behavior and most tests live ([crate map](#the-crates)) |
| `services/` | The four deployable binaries that compose crates onto the HTTP transport |
| `packages/` | `api-types`, `management-sdk`, `rxdb-client` (`@mako-cloud/rxdb`), `edge-sdk`, `cli`, `ui` |
| `apps/console` | The developer and operator console (React + Vite) |
| `examples/` | `local-first` (the reference RxDB app) and `rational` (the money manager that finds platform gaps) |
| `api/openapi/` | `mako-cloud-v1.yaml`, the public wire contract |
| `openspec/` | Capability specs and change proposals |
| `docs/` | This book, the User Book, the traceability matrix, machine-read gate files, and evidence |
| `infra/` | `local/` compose stack and dashboards; `production/` the storage StatefulSet template; `edge-runtime/` the runtime pin; `ansible/`, `proxmox/`, `public-beta/` the beta VM |
| `scripts/` | Validators, qualification runners, benchmark and release tooling, the local preparation script |
| `security/` | `threat-model.json`, the machine-checked threat-model registry |
| `config/` | Example JSON configuration documents for the data and control planes |

### Toolchain and everyday commands

Node ≥ 24, npm ≥ 11, Rust 1.97 (edition 2024). `unsafe_code` is forbidden workspace-wide; clippy runs with `-D warnings`.

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --lib --bins        # unit tests, mostly inline #[cfg(test)]
cargo test --workspace --tests             # integration targets under crates/*/tests/
cargo test -p mako-gateway quota::tests::hard_limits_rate_limits_and_retry_advice_are_stable
cargo test -p mako-gateway -- --list       # inline tests sit in a `tests` submodule per file

# TypeScript
npm run format:check && npm run lint && npm run typecheck
npm run test:unit
npm run test:integration
npm run test:unit -w @mako-cloud/cli       # one workspace
npm run test:e2e -w @mako-cloud/console    # Playwright

# Generated API types — regenerate whenever the OpenAPI document changes
npm run generate:api
npm run generate:api:check
```

`npm run build:user-book-docx` renders the User Book to `dist/user-book.docx` — a Word document with a table of contents, page numbers, and working section links — with pandoc; `scripts/build-user-book-docx.py` needs nothing else and is the one place the rendering is tuned. The Markdown stays the source the validators check.

TypeScript unit tests are `node --test` over built output, so their scripts run `npm run build` first. To run a single file, build the workspace once and then `node --test packages/cli/test/<name>.test.mjs`. No workspace defines a plain `test` script, so root `npm test` matches nothing — use `test:unit` / `test:integration`, as CI does.

Concurrent Cargo builds from several shells or agents contend on `target/`; give each its own `CARGO_TARGET_DIR`. The smoke suites do not rebuild service binaries — build them first (`cargo build --workspace --bins`) or point `MAKO_SMOKE_BINARY_DIR` at a build.

### The CI pipeline

`.github/workflows/ci.yml` runs on every pull request and push to `main`:

| Job | What it runs |
| --- | --- |
| **Format and lint** | `generate:api:check`, `validate:docs`, `validate:release-gates`, `validate:rollback`, `validate:no-collection`, Rational parity and UI-kit validators and their tests, `cargo fmt --check`, `cargo clippy -D warnings`, Biome format and lint, `tsc -b` |
| **Dependency audit** | `npm audit --audit-level=high`, `cargo audit` |
| **Unit tests** | `cargo test --workspace --lib --bins`, `npm run test:unit` |
| **Integration tests** | `cargo build --workspace --bins`, `cargo test --workspace --tests`, `npm run test:integration` |
| **End-to-end smoke** | `scripts/run-e2e-smoke-qualification.sh`, the developer CLI smoke, the local-first live browser suite; uploads `docs/evidence/e2e-smoke-qualification.json` |
| **Edge function end-to-end** | `MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-e2e` in Docker; uploads `docs/evidence/edge-e2e-qualification.json` |
| **Public beta infrastructure (offline)** | Builds the console, then the Proxmox plan/apply/teardown tests, `validate:public-beta-infrastructure`, `ansible-lint`, Caddy/containers/services validators, `validate:observability`, `validate:alert-metrics`, `scan:public-beta-secrets` |
| **Build artifacts** | Release builds of the three public service binaries and every TypeScript package |

Every job is a gate. A validator that only reads a file is still a gate: `validate:docs` fails the build on a broken link in this book.

---

## Architecture

### Planes and services

```text
                 browser / CLI / SDKs                      application clients (RxDB, edge SDK)
                          │                                             │
                          ▼                                             ▼
   ┌──────────────────────────────┐   internal RPC   ┌──────────────────────────────┐
   │  mako-control-plane :8081    │◀────────────────▶│  mako-data-plane :8080       │
   │  management, developer auth, │                  │  application auth, documents,│
   │  operator, provisioning,     │                  │  policies, replication,      │
   │  functions admin, workers    │                  │  file storage, mail intents  │
   │  ── SQLite ──                │                  │  ── RocksDB ──               │
   └──────────────┬───────────────┘                  └──────────────┬───────────────┘
                  │ resolve / schedule                              │ verify
                  ▼                                                 ▼
   ┌──────────────────────────────┐                  ┌──────────────────────────────┐
   │  mako-edge-gateway :8082     │──── protocol v1 ─▶│  edge runtime supervisor     │
   │  /{ref}/functions/v1/…       │                  │  (pinned Supabase Edge       │
   │  ── RocksDB (quota, cache) ──│                  │   Runtime, :9000)            │
   └──────────────────────────────┘                  └──────────────────────────────┘
                  │  usage / logs / events
                  ▼
   ┌──────────────────────────────┐
   │  mako-telemetry-query :9465  │   ingest + query; ── RocksDB ──
   └──────────────────────────────┘
```

| Service | Listens (local default) | Owns | Composes |
| --- | --- | --- | --- |
| `mako-control-plane` | `127.0.0.1:8081` | The control SQLite database: developer identities and lifecycle, wait list, operator entitlements and sessions, teams, projects, provisioning, control audit, idempotency, mail outbox, function metadata, webhooks, schedules, custom domains, allowed origins, email templates | `mako-control-plane`, `mako-provisioning`, `mako-billing`, `mako-audit`, `mako-object-store` (function bundles, data-job artifacts), the mail, webhook, schedule, domain-verification, and function-log workers |
| `mako-data-plane` | `127.0.0.1:8080` | Tenant RocksDB: application users, credentials and signing keys, documents, policies, indexes, RxDB sync state, buckets and object metadata, mail intents | `mako-identity`, `mako-documents`, `mako-policy`, `mako-sync`, `mako-file-storage`, `mako-auth-providers`, `mako-gateway` (quota, token verification) |
| `mako-edge-gateway` | `127.0.0.1:8082` | Its own RocksDB (quota counters, replay guard, route cache) | `mako-edge-gateway`, `mako-edge-runtime` (supervision), `mako-edge-runtime-protocol`, `mako-gateway` |
| `mako-telemetry-query` | `127.0.0.1:9465` | Its own RocksDB (retained observability records and cross-check checkpoints) | The telemetry store, redactor, and ingest/query routes |

Every service exposes `/readyz`; the control plane additionally serves `/metrics`. All listeners bind loopback; a reverse proxy (Caddy on the beta) is the only component that terminates public HTTPS.

### The request path, end to end

1. A **management** request (console, CLI, SDK) reaches the control plane with a developer session or automation token. The control plane authenticates it against SQLite, authorizes the team role or token permission, records audit and idempotency, and — for tenant resources such as collections, policies, keys, users, buckets, and sign-in settings — forwards an `Install*`/`IdentityAdmin*` command to the data plane over internal RPC. The data plane is the source of truth for those; the control plane keeps no copy.
2. An **application** request (RxDB client, edge SDK) reaches the data plane with `X-Mako-Key` and a bearer token. The gateway crate verifies the Ed25519 JWT, tenant binding, session, revocation freshness, and authorization epochs; charges quota; then the domain crate evaluates the active policy inside the same conditional transaction that commits any write.
3. A **function invocation** reaches the edge gateway, which verifies the caller (unless the function is public), resolves the active deployment and its allowed origins and verified domains against the control plane (cached), charges the tenant's invocation quota, and forwards over protocol v1 to the supervisor holding the deployment. The verified caller travels on `x-mako-caller-authorization`; the request's own `Authorization` is not forwarded.
4. **Workers** in the control plane run on their own threads: mail (developer and application outboxes), webhooks (every two seconds), schedules (every five seconds), custom-domain verification (every minute), function-log collection, and provisioning sweeps. Each reads the data plane over internal RPC where it must (`read_change_feed`, mail drain/acknowledge, function-secret resolution) and never blocks a request.
5. **Telemetry**: the data plane and edge gateway emit usage, auth-event, replication-error, and log records through `mako-telemetry-client` — buffered, bounded, dropped and counted under pressure — into the telemetry store, which the control plane queries for the observability routes and the bill.

### The crates

| Crate | Purpose |
| --- | --- |
| `mako-api` | Shared public and internal API contracts: `TenantScope`, `ApiErrorEnvelope`, `ErrorCode`, `RetryAdvice`, observability record types, telemetry constants |
| `mako-config` | Typed, layered service configuration with safe, field-addressed startup diagnostics |
| `mako-service-runtime` | The bounded, fail-closed HTTP transport shared by every service ([below](#no-async-runtime-the-service-transport)) |
| `mako-internal-rpc` | Authenticated, versioned, loopback-only service-to-service protocol with replay guards and an encrypted response journal ([below](#internal-rpc)) |
| `mako-storage` | The semantic ordered key-value boundary (`KvAdapter`, `KvSnapshot`), the deterministic `MemoryAdapter`, the RocksDB and SQLite adapters, readiness, backup/restore, and the `mako-storage-ops` / `mako-control-storage-ops` binaries |
| `mako-documents` | Versioned JSON document, schema, index, change-log, sequencer, and query engine |
| `mako-policy` | Deterministic document-policy compilation and evaluation |
| `mako-identity` | Application-user identity, Argon2id passwords, sessions, refresh families, signing keys, project credentials, administration |
| `mako-auth-providers` | External sign-in: provider settings, sealed client secrets, signed state, the OAuth 2.0 / OIDC round trip |
| `mako-sync` | RxDB pull, push, checkpoint, and live-stream service |
| `mako-file-storage` | Buckets and objects governed by the policy engine, encrypted at rest, metered |
| `mako-gateway` | Shared gateway concerns: token verification, revocation cache, quota engine and policy source, service-credential bypass, replication charging |
| `mako-control-plane` | The management and operator domain: teams, projects, provisioning, developer registration and identity, operator authentication and control center, functions administration, webhooks, schedules, custom domains, allowed origins, email templates and application mail, explorer and data jobs, billing surfaces, observability queries |
| `mako-provisioning` | Durable, idempotent project provisioning workflows |
| `mako-billing` | Plans, entitlements, exceptions, platform rate limits, the rate card and rating |
| `mako-audit` | Append-only audit event contracts and persistence, the telemetry redactor, the threat-model test |
| `mako-object-store` | Tenant-scoped immutable object storage boundary (S3-compatible client and an in-memory stub) |
| `mako-http-client` | A bounded HTTPS client for the few outbound calls the platform makes (identity providers) |
| `mako-edge-runtime-protocol` | Versioned messages exchanged with the replaceable edge-runtime supervisor; embeds and validates the runtime pin |
| `mako-edge-runtime` | Regional supervision of isolated edge-function workers |
| `mako-edge-gateway` | Public edge-function routing and authentication boundary |
| `mako-telemetry-client` | The emitting half of telemetry: buffered, bounded, non-blocking |
| `mako-benchmarks` | The performance harness behind `npm run benchmark` |
| `mako-local-bootstrap` | Seeds a complete local tenant into the two stores; refuses to run outside a local environment |
| `mako-qualification-fixture` | Creates and validates the protected hosted qualification fixture |
| `mako-smoke` | The harness that drives the **real service binaries** from a test, and the smoke suites themselves |

### The shape of a service

Each service follows the same shape:

- `graph.rs` — a `*Graph` that opens storage and dependencies and reports `readiness()`;
- `*_http.rs` modules — each registers routes onto a shared `HttpRouter`;
- `lib.rs` — a `*_router()` that composes those modules;
- `main.rs` — load config → open graph → **refuse to start unless readiness passes** → serve.

The control plane's modules map one-to-one onto product areas: `management_http` (teams, projects, environments), `collection_http`, `policy_http`, `credential_http`, `identity_admin_http` (application users), `function_http`, `function_schedule_http`, `webhook_http`, `custom_domain_http`, `allowed_origins_http`, `auth_settings_http`, `email_template_http`, `storage_bucket_http`, `explorer_http`, `data_job_http`, `workspace_http`, `observability_http`, `developer_auth_http`, `operator_auth_http`, `operator_http`, `developer_metrics_http` (`/metrics`), and `internal_http` (the loopback routes other services and the reverse proxy call). The data plane has `auth_http`, `auth_provider_http`, `document_http`, `replication_http`, `storage_http`, `service_user_http`, `explorer_http`, and `internal_http`. The edge gateway has `http` (invocation on both path shapes) and `internal_http` (the scheduler's invoke route).

Adding an endpoint means: domain logic in the crate, a route module in the service, an entry in `api/openapi/mako-cloud-v1.yaml`, regenerated types, a management-SDK method, a CLI command (the parity test insists), a traceability row, and a paragraph in the User Book.

### No async runtime: the service transport

There is no tokio, hyper, or axum. `mako-service-runtime` is built on `tiny_http` and OS threads with explicit caps: in-flight requests (default 128), request body (1 MiB default), request target (8 KiB), header count (128) and bytes (32 KiB), and a shutdown grace (20 s default) during which admission stops, bounded work drains, and the engine closes. `SIGINT`/`SIGTERM` start that drain. A handler panic is caught and answered with a safe `internal` envelope. The transport owns `/readyz` (a `ReadinessProbe` evaluated per request), the request-id header (`x-mako-request-id`), and the CORS middleware that answers listed origins on application routes.

The few async domain APIs are bridged with `futures::executor::block_on` — **one `block_on` per handler**. A helper that itself calls `block_on` inside a handler that is already inside one panics, and the panic surfaces as an opaque `500`; keep helpers `async` and let the handler be the single bridge. Do not introduce an async runtime or web framework to solve a local problem — extend the transport instead.

### Internal RPC

`mako-internal-rpc` is how services talk to each other: HTTP on loopback only, every request signed with the deployment's internal secret (`x-mako-internal-signature`, `-timestamp`, `-nonce`, `-version`, `-caller`), versioned (`INTERNAL_PROTOCOL_VERSION`), replay-guarded (`RocksInternalReplayGuard` persists nonces), and with an encrypted response journal so an idempotent command replayed after a lost response returns the stored answer. Typed clients name each direction: `ControlToDataClient` (install collections, indexes, policies, buckets, quota policies, allowed origins, custom domains, auth providers; identity administration; drain and acknowledge application mail; read the change feed; data-job import/export pages), `ControlToEdgeClient` (schedule invocations), `EdgeToControlClient` (resolve a function and its route), and `EdgeToDataClient` (verify identity, resolve function secrets). Bodies are capped (`MAX_INTERNAL_BODY_BYTES`). A non-loopback dependency address is a configuration error.

### Contracts and generated code

`api/openapi/mako-cloud-v1.yaml` is authoritative for the public wire; checked Rust and TypeScript types are authoritative for internal boundaries. `packages/api-types/src/generated/schema.ts` is generated by `npm run generate:api` — never hand-edit it (Biome ignores it), and `generate:api:check` in CI proves it matches. `packages/management-sdk` exports one typed method per operation and lists every operation id; `packages/cli/test/parity.test.mjs` proves every non-excluded operation has a command. `crates/mako-control-plane/src/management_access.rs` proves console and API RBAC parity.

### Identity domains, enforced

Identity domains are strictly separated in code as on the wire: management (developer session / automation token), project auth (application-user session scoped to one project and environment), `/service/` (scoped secret credential), `/v1/operator/` (a separate operator identity), and `/v1/developer-auth/` (hosted registration, with wait-list tokens in a dedicated audience rejected by management routes). Tenant identity comes from the verified credential and must match every `projectId`/`environmentId` path parameter; nothing infers authorization from a document field or a public key. Every keyspace prefix carries the encoded tenant, so caller-controlled bytes cannot escape their key range.

### Keyspaces

Both engines sit behind an opaque byte key/value encoding. The control plane's records live under versioned prefixes such as `control/authentication-identities/v1`, `control/developer-roles/v1`, `control/operator-auth/v1`, `control/operator-control-center/v1`, `control/webhooks/v1`, `control/function-schedules/v1`, `control/custom-domains/v1`, `control/allowed-origins/v1`, `control/email-templates/v1`, `control/application-mail-outbox/v1`, and `control/developer-workspace/v1`; tenant data uses collision-safe tenant-prefixed keys (`crates/mako-storage` key codec, property-tested); the telemetry store uses `\0mako/telemetry/v1/` and `\0mako/telemetry-checkpoint/v1/`. A new keyspace is a new versioned prefix, additive, never a rewrite of an existing one — rollback depends on it.

### Telemetry

The telemetry store (`services/mako-telemetry-query`) is a small RocksDB-backed service with an ingest route (at most 256 records per request, authenticated with a per-deployment credential from `MAKO_TELEMETRY_AUTHORIZATION_FILE` or `$CREDENTIALS_DIRECTORY/telemetry-authorization`), a query route the control plane calls for the observability signals, and a health route. Records are redacted at the store, retained for `MAKO_TELEMETRY_RETENTION_SECONDS`, and the store cross-checks the data plane's per-minute `quota` summaries against the usage records it kept (a difference past an absolute floor and five percent is a degraded health record). `mako-telemetry-client` is the emitting half every observing service shares: records are buffered without blocking (at most 4096), drained by a worker in batches of 256 with a stable offset across retries, and dropped and counted when the store is down — emitting can never fail a request a tenant paid for with a quota charge.

### The edge runtime: pin and protocol

Mako pins Supabase Edge Runtime `v1.74.3` at source commit `47d04fdd22e33ea3fd904576cf3248d963d903a9` and pulls the multi-platform OCI image only by digest:

```text
docker.io/supabase/edge-runtime@sha256:c52405002a890ca9fcf77978671c57f3a988e03174afb277f84ac65bc917013c
```

`infra/edge-runtime/runtime-pin.json` is the machine-readable source of truth; `mako-edge-runtime-protocol` embeds and validates it so an invalid or mismatched pin fails runtime startup. The 2026-08-06 evaluation chose `v1.74.3` over `v1.74.0` for a runtime-safety fix (malformed Node ECDH inputs return JavaScript errors instead of panicking the host) and rejected a standalone Deno worker because Edge Runtime already supplies isolation, worker lifecycle, npm compatibility, and resource control. Runtime upgrades require a new digest, source commit, compatibility report, sandbox tests, and rollback image; moving tags are never deployment inputs.

#### Boundary

Supabase documents Edge Runtime's configuration and APIs as beta. Mako therefore exposes none of its main-worker routes, errors, service paths, or `EdgeRuntime.userWorkers` object to gateways, control-plane code, SDKs, or user functions:

```text
control plane ---- lifecycle ----\
                                  > Mako runtime protocol v1 -> supervisor -> pinned Edge Runtime
edge gateway ----- invocation ---/
```

Only the supervisor adapter knows the upstream API. Replacing Edge Runtime must not change the public function URL or protocol v1.

#### Transport contract

Production transports use authenticated HTTP on the private service network; local development uses loopback HTTP. Every call carries `x-mako-runtime-protocol: 1`, `x-mako-request-id`, W3C `traceparent`, and `x-mako-trace-id`; a missing or unsupported version fails closed with `protocol_mismatch`. These headers are never forwarded to user-selected outbound destinations.

| Operation | Idempotency and result |
| --- | --- |
| `health` | Reports protocol, pinned release/commit, region, and readiness |
| `load` | Idempotently loads one `(project, environment, function, version)` and the exact bundle digest; a different manifest at that address is a conflict |
| `probe` | Runs the deployment health check without changing routing; returns only a state and sanitized diagnostic code |
| `retire` | Stops new admissions, drains bounded in-flight work, terminates the worker; idempotent |
| `invoke` | Targets an explicit immutable version; never resolves the active version itself |

Deployment control messages use the strict JSON types in `mako-edge-runtime-protocol`; unknown fields are rejected. Bundle bytes use a bounded binary body whose SHA-256 digest must match the manifest. Secret metadata contains only name/version references; decrypted values travel on a separate sensitive channel immediately before worker creation. Invocation preserves the original method, path/query, allowed headers, bounded body stream, status, allowed response headers, and bounded response stream. The gateway resolves the active version before invoking, so promotion is one atomic metadata switch. `x-mako-caller-authorization` carries the verified caller for protected functions, is stripped before user outbound fetches, and never appears in diagnostics; public functions omit it rather than synthesizing an identity.

#### Trust and failure rules

- The transport authenticates both workloads and authorizes the caller for the addressed region; tenant identity comes from authenticated workload context and must match the message body.
- Bundle digests, runtime release, entrypoints, tenant identifiers, limits, and secret references are validated before worker creation.
- Protocol v1 has no unrestricted egress mode. A worker receives either `deny_all` or a bounded `allow_list` carrying `hosts` and nothing else; the worker sandbox enforces it through its own permission set. `deny_all` denies the function's own destinations but not the platform API origin injected as `MAKO_API_URL` — the first-party SDK is built to call it, the function never chose it, and a function that could not reach it could neither read nor write a document.
- The supervisor passes only explicitly attached environment names and values to the user runtime; the main runtime's environment is never copied wholesale.
- User exceptions and upstream error strings are mapped to stable `RuntimeErrorCode` values; public responses receive Mako's normal error envelope and correlation id.
- A worker crash, limit event, health failure, or protocol mismatch affects only the addressed deployment. Automatic retries are allowed only before a request reaches user code unless the caller supplied an application idempotency key.
- Logs exclude bodies, authorization headers, environment values, and secret values.

#### Upstream adapter mapping

For this pin, the supervisor's main worker creates a user worker with `EdgeRuntime.userWorkers.create`, explicitly supplies memory/wall/CPU limits and selected environment variables, and calls `worker.fetch` with an abort signal. Mako owns health and lifecycle endpoints; upstream routes such as `/_internal/health`, `/_internal/metric`, and `/_internal/upload` are not part of the protocol and are never reachable from the public gateway.

#### Worker permissions

`EdgeRuntime.userWorkers.create` forwards its `permissions` object to Deno's own permission set, where the encoding is easy to read backwards: **an empty list is a grant without restriction, and `null` is the absence of a grant.** A populated list is the restriction. Every capability a function must not hold is therefore passed as `null`, never as `[]` — Rational finding #18 is what happens otherwise.

| Grant | Value | Effect |
| --- | --- | --- |
| `allow_env` | the attached secret names plus the `MAKO_*` values the supervisor injects | `Deno.env.get` of any other name is refused |
| `allow_net` | explicit `host:port` grants: the platform API origin always, plus each host the deployment's egress allowlist declares, pinned to port 443 | `fetch`, `Deno.connect`, `WebSocket`, and DNS to anything else are refused |
| `allow_read` | the worker's own directory | every other path is refused |
| `allow_write` | `null` | the function cannot write anywhere, including `/tmp` |
| `allow_import` | `null` | a module fetched over the network is never loaded; the bundle validator already refuses remote specifiers, and this closes the runtime half |
| `allow_run`, `allow_ffi`, `allow_sys` | `null` | withheld even though this image also blocks subprocesses outright and exposes no `Deno.dlopen`, so a widened image surface does not become a widened sandbox |

The SDK is reached through the worker's inline import map, which resolves the one first-party specifier onto a file inside the worker directory, so it needs no network or import grant. `mako-cloud functions serve` gives the local worker the same grants.

Protocol v1's `outboundNetwork` allowlist variant carries `hosts` and nothing else. It once also defined `maxRequestsPerInvocation`, and the supervisor refused any manifest carrying the variant rather than half-enforce it: a per-invocation request count cannot be enforced from inside an isolate the tenant controls — tenant code can reach the network through `fetch`, `WebSocket`, or `Deno.connect`, and can replace any counter installed beside it. The count left the contract for that reason, and a manifest still naming it is refused by deserialization. The host bound is real: the supervisor unions each declared host, pinned to port 443, into `allow_net` beside the API origin. A request count can return only together with a mandatory network proxy that can actually enforce it.

---

## Storage

### The split, and why it is not configurable

Two engines, deliberately:

- **Control plane → server-side SQLite** (WAL, full synchronous durability, one process holding an external lock). It owns developer credentials and lifecycle, wait list, operator entitlements and sessions, teams, projects, provisioning, control audit, idempotency, mail outbox, function metadata, and every control-plane worker's records.
- **Data plane, edge gateway, telemetry → local RocksDB `OptimisticTransactionDB`**, one process per database directory. The data plane's owns application users, tenant credentials and signing keys, documents, policies, indexes, RxDB sync state, buckets and object metadata, and mail intents.

Both sit behind `mako-storage`'s `KvAdapter`/`KvSnapshot` boundary over an opaque byte key/value encoding, with a deterministic `MemoryAdapter` for tests. There is intentionally **no backend selector, remote connector, or storage credential in configuration** — a new production backend requires a new change, not a config flag. The 2026-08-06 audit found one dormant vendor-facing surface (`crates/mako-storage/src/distributed.rs`) that nothing called; it was removed rather than kept as an unsupported choice. Production startup constructs the RocksDB or SQLite implementation directly.

Engine separation prevents a tenant RocksDB outage from becoming the control authentication authority — control readiness depends on SQLite, not tenant RocksDB, and tenant-data operations fail with their scoped dependency error when the data plane is unavailable. It is **not** replication, automatic failover, or host/disk high availability: on the beta both engines live on one VM and one data disk.

### The adapter contract

The document engine requires point reads, lexicographically ordered bounded scans, stable snapshots, atomic batches, atomic conditional writes, and durable restart behavior. `KvAdapter` exposes exactly those (`get`, `scan` with `KeyRange`/`ScanDirection`/bounded items, `snapshot`, `write` of a `WriteBatch`, `compare_and_write` with `KeyCondition`s), plus `health` and capability reporting; `Durability` names the strongest acknowledgement the adapter makes. Batch and scan sizes are bounded by configuration (`maximum_batch_operations`, `maximum_scan_items`); transactions have a lock timeout and an expiration.

Optimistic transactions use snapshot-based `get_for_update` conflict detection and pass the shared atomic conditional-race suite. The optimistic binding was selected because its safe native checkpoint API captures the live synchronous database; the pessimistic wrapper does not expose it.

The shared conformance suite (`crates/mako-storage/tests/adapter_conformance.rs`) runs every adapter — memory, RocksDB, SQLite — through the same semantics, including conditional races and stable snapshots. `sqlite_differential.rs` checks SQLite against the memory adapter operation by operation.

### The readiness contract

The data plane does not become ready merely because an adapter opened. `check_storage_readiness` collects the adapter's semantic capabilities and health result, compares its strongest acknowledgement to the configured durability requirement, and requires a positive restart-durability verification for WAL or sync operation. A missing capability is reported by name; unhealthy or degraded health, a health-check error, weaker durability, or unverified durability independently makes readiness false. The safe result contains capability names, durability mode, health class, and retryability — no vendor error text, keys, values, or document data. The deterministic in-memory adapter is intentionally rejected because it does not claim restart durability.

### The atomicity fault model

The semantic adapter treats one conditional mutation as condition reads plus a staged batch, one atomic commit point, and acknowledgement. The deterministic adapter exposes named failure locations in order: `BeforeWrite`, `BeforeConditionCheck`, `AfterConditionCheck`, `BeforeBatchStage`, `AfterBatchStage`, `BeforeCommit`, `AfterCommit`.

Failures through `BeforeCommit` return an error with the original state intact; the staged map is private and cannot be observed. A failure at `AfterCommit` models a process or transport loss after the storage commit but before the caller receives acknowledgement; recovery may observe the entire new state, and retrying the original revision condition then conflicts. No failure point permits only a document, index, change record, or idempotency result subset to become visible.

`crates/mako-storage/tests/fault_injection.rs` injects an I/O error at every point and verifies the exact pre-commit state for the first six, the exact full-commit state for the post-commit ambiguity, retry behavior, and the acknowledged path. Real adapters must map their transaction boundary to the same visibility outcomes.

The SQLite adapter maps the model to `BEGIN IMMEDIATE`, guarded reads, mutations, and one `COMMIT`. Synchronous durability is the minimum and WAL recovery is tested by abruptly terminating a helper process after an acknowledged write (`mako-sqlite-crash-helper`, `control_sqlite_recovery.rs`). Dedicated read transactions provide snapshots. Bounded busy time returns a retryable timeout; corruption, full disk, read-only files, identity mismatch, unsupported schema, and critical reserve pressure fail closed with sanitized errors. Graceful shutdown rejects new work, drains bounded operations, truncates the WAL, verifies integrity, synchronizes the database and directory, and releases the exclusive process lock.

### Production RocksDB

Each stateful service database is opened by exactly one process from one explicitly provisioned persistent volume (`ProductionRocksDb::open` with a `ProductionVolumeIdentity` naming the service and database id). Startup verifies ownership and format markers, the exclusive lock, capacity reserve (warning and critical free-byte thresholds), synchronous durability, engine health, the semantic contract, and the acknowledged high water; it never creates a database on a failed open or initializes a fallback. Stateless gateways may scale; a RocksDB owner may not be replicated or share its directory.

The **sequencer** allocates commit positions; a position that is neither committed nor aborted is a *gap* that holds committed high water back (readers, pulls, and live streams see only proven high water). Lease expiry classifies a gap from durable transaction evidence — abort only when the mutation transaction is proven absent, commit only when every atomic artifact is present. `production_crash.rs` and the soak suite exercise this recovery.

### Control-plane SQLite

The adapter preserves the opaque byte-key/value encoding in `mako_kv(key BLOB PRIMARY KEY, value BLOB) WITHOUT ROWID`, which made the RocksDB→SQLite migration byte-exact and keeps repository and API behavior stable. Metadata records a database identity and format version. Every connection enables WAL, full synchronous durability, foreign keys, `trusted_schema=OFF`, a bounded busy timeout, and bounded automatic checkpointing (`wal_autocheckpoint_pages`, `maximum_wal_bytes`). One process holds the configured external lock; a periodic integrity probe runs on `integrity_interval_seconds`.

Production startup requires an **existing** database — a blank fallback is never created — and refuses an unexpected application identity, a missing or mismatched deployment identity, an older or newer schema, symlinked or unsafe paths, an integrity failure, critical capacity pressure, or an incomplete migration. Live, lock, migration, backup staging, backup publish, restore, and reserve paths are mutually non-overlapping and reject symlink components.

**Migration** is offline from a stopped, fenced RocksDB checkpoint: `mako-control-storage-ops migrate` inventories the entire keyspace in byte order using framed key/value lengths and BLAKE3, copies into a temporary SQLite database, verifies the same count, checksum, and prefix inventory plus SQLite integrity, synchronizes, and atomically publishes. The protected receipt binds source checkpoint, release, configuration, identities, formats, counts, checksums, and timestamps. An existing live target, an unfenced or empty source, an incomplete target, or a mismatched result is refused. Once SQLite has accepted a production write, release selection accepts only binaries that declare support for its format ([release operations](#release-operations-on-the-vm)).

**Backup** uses the online backup API into an offline database, verifies identity, schema, integrity, inventory, high water, and SHA-256, authenticates the manifest, publishes atomically, copies off the VM, and verifies again. **Restore** requires a nonexistent offline target, matching identity and release, permitted age, a valid manifest, digest, inventory, and high water, and explicit no-overwrite promotion ([procedures](#control-plane-sqlite-operations)).

### Retention and compaction

`RetentionJob` runs one tenant-scoped pass across structured logs, append-only audit detail, raw usage detail, and each configured document collection, returning eligible and removed counts per data class. Always run `RetentionMode::DryRun` first: it performs no writes and does not advance the checkpoint-expiry barrier. Apply mode removes expired log, audit, and usage detail, then compacts revisions, change records, mutation-idempotency receipts, and eligible tombstones through each collection's explicitly supplied committed position.

Invariants: audit and log id digest guards remain after detail expires, so an old id cannot be reused; usage digest guards remain while raw detail is removed, so a retry cannot increment retained aggregates twice; a document retention barrier is advanced before history is deleted and checkpoints below it are expired first (which is where a client's `checkpoint_expired` comes from); tombstones are conditionally deleted only if their exact snapshotted value is still current, so compaction never deletes a concurrent resurrection; a job rejects duplicate collections and any collection whose trusted tenant differs from the job tenant; compaction never advances beyond committed high water and never weakens configured durability. A failed apply pass is safe to retry.

### Where this is tested

`crates/mako-storage/tests/`: `adapter_conformance.rs`, `fault_injection.rs`, `key_codec_properties.rs` (Proptest, 1 792 cases per run), `production_startup.rs`, `production_backup_restore.rs`, `production_crash.rs`, `sqlite_adapter.rs`, `sqlite_differential.rs`, `control_sqlite_recovery.rs`, `ops_cli.rs`. The soak and production qualifications ([Release engineering](#storage-soak-qualification)) run these for 25 consecutive cycles.

---

## Local development

Mako Cloud services run directly from the Rust and TypeScript workspaces during development. Docker Compose supplies discoverable infrastructure dependencies; Docker's `mako-cloud-local` network resolves services by the names in `infra/local/compose.yaml`.

### Prerequisites

- Node.js and npm versions accepted by `package.json` (Node ≥ 24, npm ≥ 11)
- The Rust version declared in `Cargo.toml` (1.97)
- OpenSSL 3
- Docker Compose v2, or Podman Compose with a working OCI runtime (needed for the compose dependencies and for anything that runs the pinned edge runtime)

For rootless Podman on a single-UID host, use an isolated graph root on a local filesystem and the documented `overlay.ignore_chown_errors=true` option. Do not weaken application-level runtime isolation tests or edit host container settings from project setup scripts. On a host whose home directory is on a network filesystem, rootless Podman cannot pull the pinned image into the default graph root (`lsetxattr ... operation not supported`) — point `XDG_DATA_HOME` at a local disk.

### Prepare local state

```bash
./scripts/local/prepare.sh
cp .env.example .env
```

The preparation script creates the ignored RocksDB, SQLite, backup, migration, restore, and reserve directories below `.local/`, generates a local CA plus a server certificate below `.local/certs/`, and generates the development secrets below `.local/secrets/`. Do not commit those private keys or trust the local CA system-wide. Configuration accepts secret references, never inline values, so `.env.example` points at the generated files with `file:` references. Those paths are relative to the repository root — run the services from there. Re-running the script leaves existing secrets and certificates alone.

### Run the services

`ServiceConfig` reads the **process** environment; nothing auto-loads `.env`, so export it yourself:

```bash
set -a; . ./.env; set +a
cargo run --bin mako-data-plane      # 127.0.0.1:8080
cargo run --bin mako-control-plane   # 127.0.0.1:8081, in a second shell
cargo run --bin mako-edge-gateway    # 127.0.0.1:8082, when you need functions
```

`.env.example` deliberately leaves `MAKO_BIND_ADDR` unset so each service uses its own default port; exporting it pins every service to one address. Each service refuses to start unless its readiness passes, so a successful start means storage, identity, and dependencies are actually usable. Confirm with:

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/readyz
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8081/readyz
```

The control plane and data plane each hold an exclusive lock on their own database directory, so only one process per database can run at a time.

### Seed a working tenant

A freshly started stack has no project, so nothing can authenticate against it. `mako-local-bootstrap` seeds one — developer identity, team, project, environment, public project key, signing key, collection, and a permissive development document policy — while the services are stopped:

```bash
set -a; . ./.env; set +a
cargo run --bin mako-local-bootstrap
```

It prints the created identifiers and the public project key as JSON; the key's secret is shown only at creation, and a repeated run rotates the credential and reports the new one. Two properties are deliberate and load-bearing:

- **It refuses to run unless the resolved environment is local.** It bypasses controls a hosted environment enforces for good reasons, including activating a developer without wait-list review and activating an allow-all document policy. It is a development tool, never a provisioning path.
- **It writes only to stores it exclusively owns while it runs**, so the services must be stopped. Starting it while a service holds a database lock fails with an explicit message.

It exists because hosted developer registration requires mail delivery over authenticated TLS SMTP and an operator wait-list decision, none of which a local environment has. With a runtime supervisor reachable ([below](#running-the-edge-runtime-locally)) it also deploys its sample functions through the real administrative path; without one it skips that step and says so.

### Start and stop dependencies

```bash
docker compose -f infra/local/compose.yaml up -d
docker compose -f infra/local/compose.yaml down     # named volumes and .local/data are retained
```

Local endpoints are bound only to loopback:

| Dependency | Image | Endpoint |
| --- | --- | --- |
| Mailpit (SMTP + UI) | `axllent/mailpit` | SMTP `127.0.0.1:1025`, UI http://127.0.0.1:8025 |
| S3-compatible object store | `chrislusf/seaweedfs` | http://127.0.0.1:8333 (S3), `:8888`, `:9333` |
| OpenTelemetry collector | `otel/opentelemetry-collector-contrib` | OTLP gRPC `127.0.0.1:4317`, OTLP HTTP `:4318`, Prometheus exposition `:9464` |
| Prometheus | `prom/prometheus` | http://127.0.0.1:9090 (loads `infra/local/prometheus-rules`) |
| Grafana | `grafana/grafana` | http://127.0.0.1:3000 (provisioned **Mako Cloud** dashboards) |

Rust services running on the host use `.env` loopback URLs. Services later added to the Compose network should use the Compose names `mailpit`, `object-store`, `otel-collector`, `prometheus`, and `grafana` for discovery.

The compose object store starts with **no S3 identity of its own**, so the platform's signed requests are refused (`InvalidAccessKeyId ... Available keys: 0`) until one is configured that matches the access and secret key the services resolve. `mako-local-bootstrap` does not need it (it uses an in-memory store); `mako-cloud functions deploy` and the management API do.

For local mail, set the plaintext SMTP mode described under [Application and developer mail](#application-and-developer-mail); Mailpit shows what was sent.

### Build the console

`apps/console/web-dist` is Vite build output and is not committed. The public-beta infrastructure validators inspect that bundle for its same-origin configuration and hosted authentication, and the release build packages it, so build it before running either:

```bash
npm run build --workspace @mako-cloud/console
npm run dev --workspace @mako-cloud/console          # a dev server against a running control plane
```

Both validators report exactly the build command if the directory is missing.

### Running the edge runtime locally

`mako-cloud functions serve` runs one function in the pinned runtime and answers directly — what a developer wants while writing code ([User Book](user-book.md#serving-a-function-locally)). The **hosted path** — gateway → control plane resolution → supervisor holding a registered deployment — needs a supervisor the control plane can authenticate to, which `serve` cannot be (it generates a random `MAKO_RUNTIME_AUTHORIZATION`). Run the runtime on the contract a deployment uses; `infra/ansible/roles/dependencies/files/quadlet/mako-edge-runtime.container` is the authoritative form:

- `MAKO_RUNTIME_AUTHORIZATION` must equal the internal auth secret the services use;
- `MAKO_RUNTIME_REGION` must equal `MAKO_REGION`, or the control plane reports the function unavailable in that region;
- mount `packages/cli/runtime/main` at `/home/deno/functions/main` and publish container port 9000.

The edge gateway reaches the data plane at `MAKO_DATA_PLANE_ENDPOINT`, the control plane at `MAKO_CONTROL_PLANE_ENDPOINT`, and the runtime's main worker at `MAKO_RUNTIME_ENDPOINT` (defaults 8080, 8081, 9000; all loopback), and listens on `MAKO_BIND_ADDR` (default 8082), so it can run beside a stack on other ports.

The automated version is `crates/mako-smoke/tests/edge_function.rs`:

```bash
MAKO_RUN_EDGE_RUNTIME_TESTS=1 MAKO_EDGE_TEST_ENGINE=podman npm run test:edge-e2e
```

It starts the pinned runtime, deploys a function through the administrative path, brings up the data plane, control plane, and edge gateway, and asserts the function's own response comes back through the gateway; that an undeployed function is not served; that a bare project reference does not resolve; and — with a second sample function — the SDK import, a supplied service secret, an encoded document id read back through `/service/`, and the request-id reuse refusal. That sample calls back into the data plane from inside the container, so the runtime container needs a route to the host's loopback: rootless Podman's default pasta networking does not provide one, so the suite passes `--network pasta:--map-host-loopback,169.254.1.2`; `MAKO_EDGE_TEST_NETWORK` replaces that value for another engine or host layout, and an empty value leaves the engine default alone. `MAKO_EDGE_TEST_ENGINE` selects `docker` or `podman`; `MAKO_EDGE_TEST_ENGINE_PREFIX_JSON` supplies a JSON array of arguments that must precede the subcommand (an isolated graph root, say). Without `MAKO_RUN_EDGE_RUNTIME_TESTS=1` the test reports why it is skipping and passes, matching the other edge suites. The suite requires ports 8080, 8081, 8082, and 9000, and says so when one is taken.

### Scratch space

The smoke harness, the qualification runners, and the benchmarks allocate RocksDB volumes under `MAKO_STORAGE_TMPDIR` (and Cargo under `TMPDIR`). On a shared host `/tmp` is often small or full; point both at a roomy local disk (for example `/var/tmp/mako-qual`) and never delete another session's `mako-*` scratch directories.

---

## Configuration reference

Every service loads the same validated configuration model (`crates/mako-config`) before opening listeners or storage. Defaults target local development. Set `MAKO_CONFIG_FILE` to an optional JSON file, then use environment variables for deployment-specific overrides; **environment values always win**. `config/mako.local.json.example` documents every JSON section for the data plane and `config/mako.control.local.json.example` the control plane's SQLite sections.

Unknown JSON fields, malformed addresses or URLs, invalid regions, empty paths, out-of-range limits, incomplete TLS pairs, and insecure production public URLs stop startup with a field-addressed error code:

```text
configuration error CONFIG_INVALID_VALUE at server.bind_address: must be an IP address and port
```

Successful startup emits only service, deployment, listener, storage path, and whether TLS, internal, and object-store authentication are configured. It never prints secret values.

### Secret references

Configuration accepts references, never inline secret values: `env:VARIABLE_NAME` reads an existing process environment variable; `file:/mounted/path` reads a UTF-8 secret file up to 64 KiB and removes one trailing newline. Resolved values (`SecretString`) are redacted from `Debug`, `Display`, and startup summaries. Production startup requires an internal-auth secret reference and an HTTPS public URL. The production control plane and data plane additionally require paired object-store access-key and secret-key references: the control plane for function bundles and data-job artifacts, the data plane for application file storage. Keep referenced environment variables and files out of source control.

### Deployment and server

| Variable | JSON | Meaning |
| --- | --- | --- |
| `MAKO_ENVIRONMENT` | `deployment.environment` | `local`, `development`, `staging`, or `production`. Production tightens every rule below |
| `MAKO_REGION` | `deployment.region` | Lowercase region slug (`local`, `us-east-1`, `us-east-1-beta`) |
| `MAKO_BIND_ADDR` | `server.bind_address` | IP and port; per-service defaults `127.0.0.1:8080` / `:8081` / `:8082` |
| `MAKO_PUBLIC_URL` | `server.public_url` | The public origin; must be HTTPS in production. Its host is the platform hostname a custom domain may not equal or sit under |
| `MAKO_TLS_CERT_PATH`, `MAKO_TLS_KEY_PATH` | `server.tls_*` | A complete pair or neither |
| `MAKO_MAX_REQUEST_BYTES` | `limits.max_request_bytes` | Transport body cap (default 1 MiB) |
| `MAKO_SHUTDOWN_GRACE_SECONDS` | `limits.shutdown_grace_seconds` | Drain window on `SIGTERM` |

### Storage (RocksDB services)

| Variable | Meaning |
| --- | --- |
| `MAKO_ROCKSDB_PATH` | The database directory. Relative paths resolve from the working directory only outside production; production requires a normalized absolute path outside known ephemeral filesystems |
| `MAKO_ROCKSDB_MAX_BATCH_OPERATIONS`, `MAKO_ROCKSDB_MAX_SCAN_ITEMS` | Positive, bounded |
| `MAKO_ROCKSDB_LOCK_TIMEOUT_SECONDS`, `MAKO_ROCKSDB_TRANSACTION_EXPIRATION_SECONDS` | Positive, bounded |
| `MAKO_ROCKSDB_BACKUP_DESTINATION`, `MAKO_ROCKSDB_BACKUP_RETENTION_COUNT` | Separate from the database path |
| `MAKO_ROCKSDB_DISK_WARNING_FREE_BYTES`, `MAKO_ROCKSDB_DISK_CRITICAL_FREE_BYTES` | Warning must exceed critical; production minimum for critical is 64 MiB. Startup is unready below critical |

A production control plane rejects legacy `storage.rocksdb_path`; the other services still require it. Durability and backend kind are not configurable.

### Control storage (control plane only)

| Variable | JSON (`control_storage.*`) |
| --- | --- |
| `MAKO_CONTROL_SQLITE_PATH`, `MAKO_CONTROL_SQLITE_LOCK_PATH`, `MAKO_CONTROL_SQLITE_IDENTITY` | `database_path`, `lock_path`, `database_identity` |
| `MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE`, `_BACKUP_STAGING`, `_BACKUP_PUBLISH`, `_RESTORE_WORKSPACE`, `_RESERVE_PATH` | `migration_workspace`, `backup_staging`, `backup_publish`, `restore_workspace`, `reserve_path` — mutually non-overlapping, no symlink components |
| `MAKO_CONTROL_SQLITE_MAX_BATCH_OPERATIONS`, `_MAX_SCAN_ITEMS`, `_BUSY_TIMEOUT_SECONDS`, `_TRANSACTION_EXPIRATION_SECONDS`, `_SHUTDOWN_TIMEOUT_SECONDS` | The corresponding limits |
| `MAKO_CONTROL_SQLITE_WAL_AUTOCHECKPOINT_PAGES`, `_MAX_WAL_BYTES`, `_INTEGRITY_INTERVAL_SECONDS`, `_BACKUP_RETENTION_COUNT`, `_DISK_WARNING_FREE_BYTES`, `_DISK_CRITICAL_FREE_BYTES` | WAL, integrity, backup, and capacity thresholds |

### Dependencies

| Variable | JSON (`dependencies.*`) | Used by |
| --- | --- | --- |
| `MAKO_DATA_PLANE_ENDPOINT` | `data_plane_address` (default `127.0.0.1:8080`) | Control plane and edge gateway → data plane internal RPC; must be loopback |
| `MAKO_CONTROL_PLANE_ENDPOINT` | `control_plane_address` (default `127.0.0.1:8081`) | Edge gateway → control plane (function resolution, custom domains); must be loopback; other services ignore it |
| `MAKO_EDGE_GATEWAY_ENDPOINT` | `edge_gateway_address` (default `127.0.0.1:8082`) | Control plane's function scheduler → gateway; must be loopback; other services ignore it |
| `MAKO_RUNTIME_SUPERVISOR_ENDPOINT` | `runtime_supervisor_address` (default `127.0.0.1:9001`) | Control plane → the edge runtime supervisor (deploy, health check, test) |
| `MAKO_RUNTIME_ENDPOINT` | `runtime_address` (default `127.0.0.1:9000`) | Edge gateway → the edge runtime's main worker (invocations); must be loopback; other services ignore it |
| `MAKO_TELEMETRY_QUERY_ENDPOINT` | `telemetry_query_address` | Control plane queries, data plane and gateway emit |
| `MAKO_OTLP_ENDPOINT` | `otlp_address` | Metrics export |
| `MAKO_SMTP_ENDPOINT` | `smtp_address` | Legacy relay address; developer/application mail uses `MAKO_DEVELOPER_SMTP_*` |
| `MAKO_OBJECT_STORE_ENDPOINT` | `object_store_endpoint` | S3-compatible store (bundles, artifacts, application objects) |
| `MAKO_DNS_RESOLVER` | `dns_resolver` (`host:port`) | Control plane only: the resolver the custom-domain verifier asks for `TXT` records. It is the host's resolver, so it need not be loopback. Unset, the first `nameserver` of `/etc/resolv.conf` is read **once at startup**, falling back to `127.0.0.53:53`; a resolv.conf that changes later does not change a running service |

### Secrets

| Variable | JSON (`secrets.*`) |
| --- | --- |
| `MAKO_INTERNAL_AUTH_SECRET_REF` | `internal_auth` — signs internal RPC, seals provider secrets and webhook signing secrets, is the fallback mail-encryption key; required by every service |
| `MAKO_OBJECT_STORE_ACCESS_KEY_REF`, `MAKO_OBJECT_STORE_SECRET_KEY_REF` | `object_store_access_key`, `object_store_secret_key` — required together; production requires them on the control and data planes |

### Developer registration and mail (control plane)

`developer_registration.enabled` (`MAKO_DEVELOPER_REGISTRATION_ENABLED`) defaults to `false`. Enabling it requires bounded token and session lifetimes (`MAKO_DEVELOPER_ACCESS_TTL_SECONDS`, `_REFRESH_TTL_SECONDS`, `_VERIFICATION_TTL_SECONDS`, `_RECOVERY_TTL_SECONDS`), layered rate limits (`_GLOBAL_RATE_LIMIT`, `_SOURCE_RATE_LIMIT`, `_EMAIL_RATE_LIMIT`, `_TOKEN_RATE_LIMIT`, `_RATE_WINDOW_SECONDS`), password-work and pending-outbox limits (`_MAX_PASSWORD_WORK`, `_MAX_PENDING_OUTBOX`), finite retention (`_MAIL_RETENTION_SECONDS`, `_DECISION_RETENTION_SECONDS`, `MAKO_DEVELOPER_DELIVERED_MAIL_RETENTION_SECONDS`), outbox behavior (`_OUTBOX_BATCH`, `_OUTBOX_LEASE_SECONDS`, `_OUTBOX_MAX_ATTEMPTS`, `_OUTBOX_MAX_BACKOFF_SECONDS`), a protected mail-encryption secret (`MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF`; the internal-auth secret when unset), and a complete SMTP set: `MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME`, `_PORT`, `_TLS_MODE`, `_USERNAME`, `_PASSWORD_REF`, `_SENDER`, `_TIMEOUT_SECONDS`. The public origin is the validated HTTPS server URL.

Production accepts only verified TLS modes (`starttls` or `wrapper`); missing mail material stops startup. `MAKO_DEVELOPER_SMTP_TLS_MODE=plaintext` speaks unencrypted SMTP and is the one mode in which the username and password reference may be omitted (they are still required together when either is set). It is for the local compose stack's Mailpit and for tests only; a production configuration that names it fails with `CONFIG_INVALID_VALUE at developer_registration.smtp_tls_mode`:

```bash
export MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME=127.0.0.1
export MAKO_DEVELOPER_SMTP_PORT=1025
export MAKO_DEVELOPER_SMTP_TLS_MODE=plaintext
export MAKO_DEVELOPER_SMTP_SENDER="Mako Local <no-reply@localhost>"
```

The same relay, encryption key, lease, attempt, backoff, and retention settings carry [application mail](user-book.md#application-mail). Without a relay the control plane starts, serves the template API, and drains nothing; intents wait on the data plane until a relay is configured.

### Operator authentication (control plane)

`MAKO_OPERATOR_PASSWORD_AUTH_ENABLED` (default `false`), `MAKO_OPERATOR_SESSION_TTL_SECONDS` (3600), `MAKO_OPERATOR_MUTATION_FRESHNESS_SECONDS` (300 — how recent a password verification must be for a guarded mutation), `MAKO_OPERATOR_ATTEMPT_WINDOW_SECONDS`, `MAKO_OPERATOR_SOURCE_RATE_LIMIT`, `MAKO_OPERATOR_IDENTITY_RATE_LIMIT`, `MAKO_OPERATOR_BASE_BACKOFF_SECONDS`, `MAKO_OPERATOR_MAX_BACKOFF_SECONDS`, and `MAKO_OPERATOR_BREAK_GLASS_BEARER_ENABLED` (default `false`; fail-closed). The control center additionally reads `MAKO_OPERATOR_DIAGNOSTIC_ORIGINS` (the HTTPS origins diagnostic links may point at), `MAKO_OPERATOR_BACKUP_EVIDENCE_JSON`, and the two recovery gates `MAKO_OPERATOR_RECOVERY_CREATE_ENABLED` and `MAKO_OPERATOR_RECOVERY_PROMOTE_ENABLED` (both default `false`).

### The telemetry store

`mako-telemetry-query` reads its own small set: `MAKO_TELEMETRY_QUERY_BIND` (loopback), `MAKO_TELEMETRY_REGION`, `MAKO_TELEMETRY_RETENTION_SECONDS`, `MAKO_TELEMETRY_DATABASE_PATH` (absolute), `MAKO_TELEMETRY_DATABASE_ID`, `MAKO_TELEMETRY_AUTHORIZATION_FILE` (or `$CREDENTIALS_DIRECTORY/telemetry-authorization`), `MAKO_DISK_WARNING_FREE_BYTES`, and `MAKO_DISK_CRITICAL_FREE_BYTES`. Any invalid value refuses startup.

### Public-beta configuration

The VM renders one production JSON document per Mako service. All application, telemetry, object-store, mail, and runtime-supervisor listeners remain on loopback; Caddy is the only component permitted to terminate public HTTPS. The public URL in every document is exactly `https://cloud-test.makodb.com`. Secret fields are `file:/run/credentials/...` references populated by systemd and are never copied into the rendered JSON or Ansible logs. The rendered configuration and redaction result are retained in `docs/evidence/public-beta-production-configuration.json`; validate all static assets with `npm run validate:public-beta-local` before convergence. Admission modes and the desired-state selector are described under [Deployment](#the-public-beta-vm).

---

## Testing and qualification

### The philosophy

Rust tests are predominantly inline `#[cfg(test)]` next to the code, in a `tests` submodule per file, with `crates/*/tests/` reserved for conformance, fault-injection, crash/recovery, and property suites (`mako-storage`, `mako-documents`, `mako-sync`, `mako-audit`). TypeScript unit tests are `node --test` over built output. Browser suites (Playwright) prove client behavior against an in-browser fake or an intercepted API; they would pass with the server completely broken, which is why the **smoke suites** exist: `crates/mako-smoke` drives the **real service binaries** over HTTP with nothing stubbed.

A gate that silently skips stops being a gate. Suites that need something the host may not have (a container engine, ports) say why they are skipping and pass **only** when the opt-in variable is absent; a missing binary fails with an actionable message.

### The layers

| Layer | Command | What it proves |
| --- | --- | --- |
| Rust unit | `cargo test --workspace --lib --bins` | Domain behavior, every refusal path, inline next to the code |
| Rust integration | `cargo test --workspace --tests` | Storage conformance, fault injection, crash recovery, key-codec properties, document invariants, multi-client sync, the threat model, and every smoke suite |
| TypeScript unit | `npm run test:unit` | SDK behavior (`rxdb-client`, `edge-sdk`, `management-sdk`, `ui`), CLI command groups against a loopback mock, the operation inventories |
| TypeScript integration | `npm run test:integration` | CLI compatibility and adversarial suites |
| Console e2e | `npm run test:e2e -w @mako-cloud/console` | Every console area against an intercepted `/v1/` |
| Sample browser suites | `npm run test:browser -w @mako-cloud/example-local-first` / `-rational` | Client behavior against an in-browser fake |
| Live browser suites | `npm run test:browser-live -w …` | The same scenarios against real binaries behind a same-origin proxy |
| Smoke | `npm run test:e2e-smoke`, `cargo test -p mako-smoke --test <name>` | The server half of every protocol, with real processes |

### The smoke suites

`crates/mako-smoke/tests/`:

| Suite | Covers |
| --- | --- |
| `happy_path.rs` | Bootstrap, application sign-up and sign-in, push, pull, and the negative control without a session (`npm run test:e2e-smoke`; writes `docs/evidence/e2e-smoke-qualification.json`) |
| `developer_cli.rs` | The built `mako-cloud` CLI through a console workflow (skips with a notice when `packages/cli/dist` is absent) |
| `database_service.rs` | Document and `/service/` routes: encoded ids, request-id reuse, idempotency |
| `explorer_proxy.rs` | Personal and team projects through the production Caddy routes: grants, browsing, queries, edits, revocation, scope enforcement, and custom-domain denial |
| `auth_providers.rs` | Provider start/callback/exchange and magic links against a loopback provider stub |
| `file_storage.rs` | Buckets, objects, policy, conditional uploads, public buckets, totals, removal (with `ObjectStoreStub`) |
| `webhooks.rs` | A real endpoint receiving and verifying signed deliveries |
| `custom_domains.rs` | The domain lifecycle against a loopback DNS stub (`DnsStub`) |
| `edge_function.rs` | The hosted function path through the pinned runtime (opt-in, `npm run test:edge-e2e`; writes `docs/evidence/edge-e2e-qualification.json`) |
| `personal_space.rs`, `project_transfer.rs` | Team ownership semantics |
| `control_outage.rs` | Control routes stay available while tenant-data routes fail closed during a data-plane outage |
| `telemetry_pipeline.rs` | Metering end to end into the telemetry store |
| `sample_app.rs`, `rational.rs` | The Rational model over HTTP (`npm run test:rational-smoke`; writes `docs/evidence/rational-smoke-qualification.json`) |

Each run allocates ephemeral ports and a temporary directory under `MAKO_STORAGE_TMPDIR`, so runs do not collide with a development stack or with each other (the edge suite is the exception: the gateway's ports are compiled in). `MAKO_SMOKE_BINARY_DIR` overrides where binaries are found.

The explorer proxy suite requires Caddy and Python with Jinja2. It renders the production Caddy template with loopback listeners and upstream ports, then starts real services with temporary storage. Set `MAKO_SMOKE_CADDY` and `MAKO_SMOKE_PYTHON` if those executables are outside `PATH`, then run `cargo test -p mako-smoke --test explorer_proxy`. CI installs these dependencies before running the integration targets. The route validator also checks that document explorer requests reach the data plane while grant requests reach the control plane. Neither route is exposed on custom domains.

### Qualification suites

These are evidence-producing suites run before a release, not part of the normal edit loop. Each one's latest result is summarized under [Release engineering](#qualification-reports).

| Command | Scope |
| --- | --- |
| `npm run test:auth-security` | Argon2id, enumeration-safe flows, JWT boundaries, signing-key rotation, refresh replay, revocation, credentials, client refresh |
| `npm run test:policy-security` | Differential policy decisions, visibility transitions, epochs, conflict non-disclosure, privileged bypass, edge callers |
| `npm run test:rxdb-chaos` | Concurrent offline clients, dropped-response retries in 128 orderings, tombstones, epoch resets, stream gaps, schema mismatch, checkpoint expiry, restart, refresh |
| `npm run test:tenant-boundaries` | Property-tested key encoding and every tenant boundary (≥ 3 968 generated cases) |
| `MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-security` | The real pinned image: isolation, secrets, limits, egress, compatibility, supply chain |
| `npm run test:storage-soak` | 25 complete storage and document cycles on the storage class under test (`MAKO_STORAGE_TMPDIR`, `MAKO_STORAGE_SOAK_ITERATIONS`) |
| `bash scripts/run-production-rocksdb-qualification.sh` | The production topology's durability, recovery, backup, and performance thresholds |
| `npm run test:rollback` | The six rollback drills (`scripts/run-rollback-qualification.sh`) |
| `npm run benchmark` | The component performance baseline (`scripts/run-performance-benchmarks.sh`) |
| `npm run test:operator-auth` | Operator password authentication |
| `npm run test:public-beta-*` | Hosted qualification against the beta VM (streaming, operator browser, hosted benchmark) |

### Validators

Validators gate CI and read checked-in files; run the relevant one after editing docs, spec scenarios, or `infra/`.

| Command | Checks |
| --- | --- |
| `validate:docs` | This book and the User Book exist, are indexed by `docs/README.md`, contain the sections each product area is documented under, and have no broken relative links (every Markdown file under `docs/` and the root README) |
| `validate:traceability` | `docs/requirements-traceability.md` maps every spec scenario to exactly one automated test, ids and titles match the specs, cited files exist, mocked-backend evidence is labelled |
| `validate:release-gates` | `docs/release-gates.json` shape, thresholds, blockers, and bindings |
| `validate:rollback` | `docs/rollback-qualification.json` names six passing drills whose procedures are in this book and whose evidence exists; the beta stays blocked |
| `validate:performance`, `validate:production-release`, `validate:production-storage` | The baseline report's shape; production thresholds against the baseline; the StatefulSet template's single-replica retained volumes |
| `validate:observability` | Grafana dashboard inventory and shape, forbidden high-cardinality labels, and every alert rule's expression, hold, severity, and runbook link |
| `validate:alert-metrics` | Every alert expression names a metric something publishes, or is listed with a reason in the script's `UNPRODUCED` map — which may only shrink |
| `validate:no-collection` | No payment-provider integration exists and nothing that enforces limits reads the balance |
| `validate:rational-parity`, `validate:ui-kit` | Rational's Monarch parity matrix stays ≥ 90 %; no raw form controls outside the design system |
| `validate:public-beta-*`, `validate:proxmox-image`, `scan:public-beta-secrets` | The beta's cloud-init, Ansible, Caddy, containers, systemd units, retained evidence, image pin, and absence of secrets |
| `validate:operator-auth-assets` | The operator authentication assets |

### Evidence

Suites that qualify something write machine-readable evidence under `docs/evidence/` (see the [appendix](#appendix-evidence-files)). A report states the host and scope it exercised; a passing local report never qualifies a different cloud storage class, container runtime, region, or cost model, and CI uploads the smoke and edge evidence as build artifacts rather than committing it.

---

## The spec-driven workflow

### OpenSpec

The project uses OpenSpec (`openspec/`, schema `spec-driven`). Capability specs live under `openspec/specs/<area>/<capability>/spec.md` — currently 29 capabilities across `billing`, `cloud`, `functions`, `identity`, `operations`, `samples`, `security`, `storage`, and `sync`. Work is proposed as a **change** under `openspec/changes/<name>/` with `proposal.md`, `design.md`, `tasks.md`, and spec deltas, then archived under `openspec/changes/archive/<date>-<name>/` after apply.

Skills are installed for Claude Code (`.claude/skills/`) and Codex (`.agents/skills/`): `/openspec-propose`, `/openspec-explore`, `/openspec-update-change`, `/openspec-apply-change`, `/openspec-archive-change`, `/openspec-sync-specs`. Proposing is planning-only — the propose workflow does not touch project code, and implementation waits for an explicit apply.

### Traceability

`docs/requirements-traceability.md` maps every spec scenario (`#### Scenario:` headings, numbered per capability prefix — `CP`, `BM`, `BP`, `BI`, `ER`, `PA`, `DP`, `DE`, `RR`, `DR`, `LS`, `CL`, `DC`, `AF`, `AP`, `DW`, `SF`, `CD`, `AD`, `RA`, `DS`) to its primary automated test. `npm run validate:traceability` enforces one-to-one coverage, exact titles, and that cited files exist; evidence under `apps/console/test-e2e/`, `examples/local-first/test/`, or `examples/rational/test/` must be marked `(mocked backend)`. When you add or rename a scenario or its test, update the matrix in the same change.

### Adding a capability or an endpoint

1. Propose the change (spec deltas, design, tasks). Say what is out of scope.
2. Domain logic and its inline tests in the crate; fail-closed refusals first.
3. A route module in the service, registered in `lib.rs`; readiness if the route has a new dependency.
4. The OpenAPI entry, then `npm run generate:api`; a typed method in `packages/management-sdk`; a `mako-cloud` command (the parity test will insist); console surface where the User Book promises one.
5. A smoke or integration test that drives the real binaries when the behavior crosses a service boundary.
6. Traceability rows; the User Book chapter and, for operational surface, this book; `npm run validate:docs` and `validate:traceability`.
7. If the change touches storage, startup, release, or a security boundary: the threat model registry and its review section, the rollback drill it affects, and the release-gate evidence it changes.

### Docs are part of the change

`docs/README.md` is the index and `npm run validate:docs` checks it. Qualification reports state the host and scope they exercised — do not generalize a passing local report to another environment, and do not relax a release gate in `docs/release-gates.json` as a side effect. When code that this book or the User Book describes changes, change the paragraph in the same commit.

---

## Conventions

- **Fail closed.** Startup refuses on mismatched database identity, wrong schema version, unsafe or symlinked paths, integrity failure, critical disk pressure, or incomplete migration. Readiness gates serving; degraded dependencies produce scoped errors rather than silent fallbacks. A missing scope, ambiguous identity, unavailable policy state, configuration drift, unhealthy regional placement, or unsupported capability is a refusal. Preserve this when touching startup or readiness paths.
- **Secrets are references, never inline values** — `env:NAME` or `file:/path`. Resolved values are redacted from `Debug`/`Display` and startup summaries. Anything that issues a secret shows it exactly once.
- **Config errors are addressed**: stable code + field path + non-sensitive explanation.
- **Errors are envelopes.** Public failures use `ApiErrorEnvelope` with a stable code, safe message, request id, retry advice, and bounded safe details — never vendor text, keys, values, document bodies, or existence/count information the caller is not entitled to.
- **Audit before privilege.** A privileged or security-sensitive action appends actor, target, reason, result, request, and correlation evidence first; if the append fails, the action does not happen.
- **One `block_on` per handler**; helpers stay `async`.
- **Keyspaces are additive and versioned.** Rollback depends on a prior binary being able to ignore what it does not know.
- **No new production storage backend, async runtime, or web framework** without a change that says why.
- **Biome** formats JS/TS/JSON at 100 columns, double quotes, semicolons, trailing commas. TypeScript is strict with `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, and `verbatimModuleSyntax`. `rustfmt` and `clippy -D warnings` are gates.
- **Write code that reads like the surrounding code**: match its comment density, naming, and idiom. Commit messages in this repository read as a sentence about what changed for whom.

---

## Security

### The threat model

Version 1, reviewed 2026-08-05. This section covers the Mako Cloud MVP. Security, data-platform, identity, and edge-runtime owners review it before each production release and after a material architecture or trust-boundary change. The machine-checked registry is [`security/threat-model.json`](../security/threat-model.json); `crates/mako-audit/tests/threat_model.rs` fails if the registry loses required categories, has duplicate or malformed ids, omits control ownership or prevention/detection/response/verification, references an unknown boundary, identity, or data class, or drifts out of this review section (every id below must appear here).

#### Scope and assumptions

In scope are the browser/Node RxDB client, public and internal APIs, project auth, document policy evaluation, the document and replication engines, the control plane, operators, the ordered transactional KV boundary, infrastructure dependencies, edge-function deployment and execution, CI, and release artifacts.

Hosted developer registration adds an untrusted signup/mail boundary and a distinct wait-list authority. Verification and recovery tokens are single-purpose digests, pending access uses a separate audience, protected requests check current persistent lifecycle and epoch, operator review uses a separate permission and atomic audited decisions, and verified-TLS mail leaves an encrypted bounded outbox. Applicant identifiers, source addresses, tokens, passwords, and review reasons are excluded from metric labels and ordinary logs.

Cloud and persistent-volume providers are assumed to enforce their documented physical and account isolation. Neither storage engine is trusted merely because it opens: control-plane SQLite must prove private path ownership, exclusive locking, database identity, schema and integrity, capacity, synchronous durability, and the required adapter semantics; tenant RocksDB must prove its ownership and semantic readiness independently. End-user devices, networks, tenant-authored documents, queries, policies, schemas, and function code are untrusted. A valid identity does not imply authorization to another project, environment, document, management action, or function secret.

The control plane is the sole owner of the local SQLite file and its WAL, lock, migration, restore, and backup staging paths. The browser is presentation only: it creates no SQLite, IndexedDB, Dexie, RxDB, or local-storage authority. Availability against provider-wide catastrophe and malicious cloud-provider administrators is outside the MVP guarantee; these remain deployment risks and do not weaken tenant isolation or fail-closed requirements.

### Security invariants

1. Every storage key and internal call carries a validated project and environment scope; caller-controlled bytes cannot escape their encoded key range.
2. The same active document policy governs direct reads/writes, queries, indexes, pull, live streams, push conflict responses, trusted user-context calls, and support impersonation. Denial is the default.
3. A storage adapter cannot serve traffic until it proves every semantic capability required by the document engine.
4. Secret values, password material, protected document bodies, and unauthorized existence/count information never enter public errors, diagnostics, logs, traces, metrics labels, or audit metadata.
5. Tenant function code runs in a project-bound isolate with bounded resources and explicit secrets, data, and egress capabilities. Cross-invocation state is absent or proven clean.
6. Privileged and security-sensitive actions create append-only actor, target, reason, result, request, and correlation evidence.
7. Missing scope, ambiguous identity, unavailable policy state, configuration drift, unhealthy regional placement, and unsupported runtime/storage capabilities fail closed.

#### Trust boundaries

| ID | Boundary | Required controls at entry |
|---|---|---|
| `TB-01` | Untrusted clients and the internet → public gateways | TLS, request/schema limits, authentication where required, generic auth errors, rate limits |
| `TB-02` | Gateway → control and data planes | Authenticated workload identity, explicit project/environment scope, route-level least privilege |
| `TB-03` | Data services → ordered transactional KV | Capability handshake, encoded tenant prefixes, bounded scans, transaction and durability conformance |
| `TB-04` | Services → SMTP, object storage, telemetry, and other dependencies | Scoped credentials, configured endpoints, deadlines, output redaction, no implicit egress |
| `TB-05` | Edge supervisor → untrusted tenant function isolate | Fresh/clean isolate, capability injection, resource ceilings, default-deny network policy |
| `TB-06` | Team members/support operators → management and operator paths | Team RBAC, step-up/JIT access, case and reason binding, immutable audit, visible impersonation |
| `TB-07` | Source/dependencies → release artifact | Protected review, locked/audited dependencies, isolated release identity, provenance/signature verification |
| `TB-08` | Control-plane desired state → regional runtime state | Versioned configuration, idempotent reconciliation, authorized promotion/rollback, drift detection |
| `TB-09` | Developer console → capability-authenticated explorer data routes | Short-lived signed scope/mode/operation grant, current epoch/nonce, policy preview or audited administrative authorization |
| `TB-10` | Control plane and offline recovery tools → local SQLite authority | Exclusive lock and identity, private service-owned paths, authenticated manifests, schema/integrity/capacity gates |

No internal network position is itself trusted. Crossing a boundary requires both identity and authorization for the exact operation and scope.

#### Privileged identities

| ID | Identity | Permitted privilege and constraint |
|---|---|---|
| `PI-01` | Provisioning controller | Reconcile only the project/environment named by its durable step; no arbitrary document reads |
| `PI-02` | Public gateway service | Authenticate and route; cannot bypass policy evaluation or mint management grants |
| `PI-03` | Data-plane service | Access assigned environment key ranges; cannot mint operator or release credentials |
| `PI-04` | Control-plane service | Mutate management state; cannot directly read customer document bodies |
| `PI-05` | Edge runtime supervisor | Create constrained isolates and inject only the selected project/function capabilities |
| `PI-06` | Break-glass support operator | Approved, time/case-bound, phishing-resistant MFA, enhanced audit, no silent impersonation |
| `PI-07` | Release automation | Publish signed artifacts from protected commits; no production data access |
| `PI-08` | Explorer administrative capability | One tenant collection, explicit operations and reason hash, current developer authority, maximum five-minute lifetime |
| `PI-09` | Control storage migration/recovery operator | Inspect fenced artifacts and publish only verified empty SQLite migration/restore targets using root-readable authentication material and explicit offline confirmation |

Service identities use short-lived workload credentials. Storage, object, mail, telemetry, and secret-store permissions are split by service role. Human shared accounts and permanent support grants are prohibited. Emergency access expires automatically and triggers review.

#### Sensitive data classes

| ID | Data | Handling rule |
|---|---|---|
| `SD-01` | Credentials, signing keys, API keys, function secrets | Reference instead of embedding; envelope encryption; redact; rotate/revoke |
| `SD-02` | Passwords, sessions, tokens, identity links | Argon2id; hashed refresh state; audience/project binding; minimal retention |
| `SD-03` | Documents, revisions, indexes, change records, tombstones | Tenant key isolation; uniform policy checks; retention compatible with supported offline clients |
| `SD-04` | Schemas, policies, quotas, and project configuration | Validate, version, authorize, atomically activate, retain rollback/audit history |
| `SD-05` | Audit and security events | Append-only and integrity protected; restrict reads; never include document bodies/secrets |
| `SD-06` | Function source, bundles, deployment state, invocation logs | Digest-bound artifacts; isolated execution; redacted and bounded logs |
| `SD-07` | Membership, billing, support, and case records | Management RBAC; purpose limitation; operator-access audit |
| `SD-08` | Metrics, traces, IPs, request and operational metadata | Bounded labels; pseudonymous IDs; short retention; no payload or secret values |
| `SD-09` | Operator incidents, recovery jobs, projections, and activity exports | Metadata-only; reason/case bound; integrity protected; expiring exports; no document bodies or secrets |
| `SD-10` | Explorer grants, data jobs, import uploads, export artifacts | Tenant/digest bound, encrypted, expiring, bounded, and absent from browser persistence |
| `SD-11` | SQLite control database, migration receipts, backup manifests | Private files, authenticated manifests, byte-exact inventories, integrity checks, no values or customer identifiers in evidence |

Encryption at rest and in transit is required in hosted environments. Backup retention must not silently extend revoked-secret availability or tombstone guarantees. Destructive project lifecycle operations are explicit workflows with retention and recovery state.

#### Abuse cases and control ownership

The detailed prevention, detection, response, boundary, identity, asset, and verification mappings are normative in the JSON registry. This table is the human review index.

| ID | Abuse case | Primary owner | Expected secure outcome |
|---|---|---|---|
| `AC-01` | Cross-tenant point/range access | data-platform | Reject before storage access; record scope mismatch without target data |
| `AC-02` | Policy bypass through sync, conflicts, indexes, or diagnostics | security | Uniform decision path; never return protected body/existence/count |
| `AC-03` | Credential stuffing and account enumeration | identity | Generic response plus bounded, observable throttling |
| `AC-04` | Session replay or token theft | identity | Detect refresh reuse, revoke family, preserve project/audience isolation |
| `AC-05` | Management/support privilege escalation | security | Deny outside team role or JIT case grant; audit every attempt |
| `AC-06` | Edge isolate escape or cross-project reuse | edge-runtime | Terminate isolate/node, rotate exposed capabilities, preserve other tenants |
| `AC-07` | SSRF or unauthorized edge egress | edge-runtime | Default-deny destination, revalidate DNS/redirects, block provider metadata |
| `AC-08` | Resource exhaustion through API, sync, query, or functions | platform | Bound offender and shed its work without noisy-neighbor propagation |
| `AC-09` | Storage adapter misrepresents semantics | data-platform | Fail readiness and block release/writes until conformance passes |
| `AC-10` | Replay, rollback, or partial activation of security configuration | control-plane | Preserve monotonic, atomic, authorized version state and repair workflow |
| `AC-11` | Audit deletion, forgery, or payload injection | security | Detect continuity/sink failure, preserve evidence, prevent sensitive fields |
| `AC-12` | Compromised dependency or tampered release | release-engineering | Block unverified digest, revoke signer, rebuild trusted provenance |
| `AC-13` | Secret/document leakage through errors or telemetry | security | Safe error schema and redaction; rotate and contain on canary detection |
| `AC-14` | Stale offline client resurrects deleted/unauthorized state | sync | Reject stale write or demand full resync without leaking conflict state |
| `AC-15` | Duplicate delivery repeats writes or side effects | data-platform | Atomically return stored scoped idempotency outcome |
| `AC-16` | Auth email abuse and redirect phishing | identity | Allowlisted redirect, single-use expiry, quota, generic response |
| `AC-17` | Global operator inventory leaks data or triggers unbounded work | control-plane | Metadata-only bounded scans, signed scoped cursors, redaction, durable denied audit |
| `AC-18` | Stale or forged operator action targets changed state | security | Reject stale version/action binding and require fresh password verification/review |
| `AC-19` | Unsafe observability deep link exfiltrates credentials | platform | Retain only allowlisted HTTPS origins and safe bounded identifiers |
| `AC-20` | Recovery restores or promotes unverified state | data-platform | Independent gates, verified evidence, durable state, successful verification before promotion |
| `AC-21` | Entitlement change removes last recoverable administrator | identity | Reject unsafe changes and preserve controlled bootstrap recovery |
| `AC-22` | Activity export/projection leaks data or hides integrity gaps | security | Distinct permission, bounded redacted expiry, count/checksum and gap evidence |
| `AC-23` | Forged, replayed, stale, or mode-confused explorer grant | security | Reject on signature, authority, nonce, epoch, scope, operation, mode, or lifetime mismatch |
| `AC-24` | Browse/query leaks hidden rows, tombstones, counts, or cursor state | data-platform | Snapshot and signed cursor binding, policy-fill paging, history permission, no scan fallback |
| `AC-25` | Explorer mutation bypasses document invariants | data-platform | Preview never commits; admin reuses conditional sequenced mutation path and enhanced audit |
| `AC-26` | Import/export artifact crosses scope or exposes partial output | data-platform | Tenant-bound digest grants, dry run, quotas, durable progress, finalized artifacts only |
| `AC-27` | Connect/sync/recovery diagnostics expose secrets or expand authority | platform | Non-document probes, bounded aggregates, safe manifests, isolated stepped-up restore only |
| `AC-28` | Control SQLite file or backup theft | security | Private paths and authenticated off-VM artifacts; contain host and rotate credentials on exposure |
| `AC-29` | Malicious/corrupt SQLite schema becomes authoritative | data-platform | Reject identity/schema/integrity mismatch and restore only into an empty verified target |
| `AC-30` | Migration substitution or partial copy | release-engineering | Fence the source and require release-bound, byte-exact inventory/checksum proof before atomic publication |
| `AC-31` | Rollback resurrects obsolete RocksDB control state | release-engineering | Refuse pre-SQLite binaries after SQLite writes without a separately verified reverse migration |
| `AC-32` | Browser persistence is treated as portal authority | security | No browser database authority; revalidate roles and sessions at the SQLite-backed server |
| `AC-33` | Tenant RocksDB outage is misdiagnosed or bypassed | platform | Keep SQLite-backed control routes available and fail only tenant-data operations with scoped errors |

#### Developer data explorer boundary

Explorer capabilities cross `TB-09` and live only in browser memory. The control plane revalidates active developer status, team membership and data permission, active project/environment, active collection, requested mode, and any selected active application user. The data plane accepts only the signed `mako-control-plane` to `mako-data-plane-explorer` audience, then rechecks the persistent nonce and current authorization epoch for the exact tenant, collection, mode, and operation. Grants expire within five minutes and cannot be exchanged for project credentials.

The console opens collections directly for members with data-admin permission, both in personal and team projects. It automatically issues and renews five-minute administrative grants using the standard reason `Browse and manage documents in the cloud console`, with no mode picker or reason form. Credentials stay in memory, and switching scope revokes the previous grant and discards late responses. Server permission checks, application document policies, and auditing remain in force. API and CLI policy preview remains available.

Policy preview uses the selected application user's current trusted claims and active policy and cannot commit. Administrative access requires data-admin permission and a reason hash and establishes a separate audited privileged authorizer for every operation. Primary-key browse runs inside the document engine over a stable snapshot, fills through policy-hidden rows, and binds cursors to tenant, collection, mode, schema, query, snapshot, epoch, and expiry. Indexed queries never fall back to collection scans. History, import, export, artifact access, backup inventory, and isolated restore each require separate permissions and bounded contracts. Audit and telemetry contain identifiers, fingerprints, index names, outcomes, and reason hashes, never capabilities, raw reasons, email addresses, document bodies, selectors, or artifact contents.

#### Operator control-center boundary

The operator control center is a metadata-only administrative surface. Global reads use signed, scope-bound cursors and bounded storage scans. `tenant_read` remains a compatibility grant for new read-only views, while incident management, recovery, security administration, and activity export use separate permissions. Missing or stale provider data is visibly `unknown`, `stale`, or `unavailable`; absence is never interpreted as healthy.

High-impact workflows require a password verification no older than five minutes plus an action binding over the action, target, and reviewed resource version. Requests also carry an idempotency key, explicit confirmation, reason, and optional case reference. Recovery creation and promotion have independent feature gates. Promotion requires verified backup evidence and successful post-restore verification; provider and executor interfaces do not accept arbitrary commands or paths. Routine operator routes cannot return customer document bodies. Diagnostic links must use a configured HTTPS origin allowlist and safe identifiers. Activity exports require a distinct permission, preserve their filter digest and integrity checksum, are bounded by the retained projection, and expire after one hour.

#### Control SQLite boundary and correlated failures

The control-plane process crosses `TB-10` through one vendor-neutral adapter. A fixed application id, configured database identity, supported schema, exclusive process lock, `trusted_schema=OFF`, integrity probe, capacity reserve, WAL limit, and synchronous writes gate authority. Database and lock paths are normalized and may not be symlinks or overlap any RocksDB, migration, backup, restore, or reserve path. SQLite has no listener and is never proxied by Caddy. Migration and recovery tools operate offline, create only new targets, authenticate their manifests, and publish through an atomic rename after inventory and integrity verification.

File permissions reduce but do not eliminate `AC-28`: a host-root or disk-snapshot compromise can copy the database and password hashes. Detection therefore includes permission drift, artifact verification, and canary scanning; response contains the VM, rotates affected credentials and sessions, and restores only from a trusted artifact. `AC-29` fails startup on a wrong application id, database identity, unsupported schema, failed integrity check, or raw SQLite error; startup never creates a blank production authority.

For `AC-30`, a stopped RocksDB checkpoint is fenced by digest and bound to an exact migration plan, release, configuration, paths, identity, and format. Complete-keyspace framed checksums and prefix inventories must match the temporary SQLite target before fsync and atomic publication. A protected receipt makes retries idempotent and rejects source drift or an existing ambiguous target. For `AC-31`, once SQLite accepts a post-cutover write, release selection requires declared support for its format. The old checkpoint remains evidence, not an active fallback.

`AC-32` is enforced by static console tests and session tests: operator authentication remains in an `HttpOnly` cookie, the developer token remains short-lived in session storage, and no browser database or durable local authority is introduced. During `AC-33`, control readiness and SQLite-backed authentication, wait-list, audit, incident, team, and project metadata remain available. Tenant reads, mutations, credentials, application-user administration, replication, and recovery actions continue to require the data plane and fail closed with an explicit unavailable provider rather than treating missing data as empty or healthy.

#### Verification and release gates

`crates/mako-audit/tests/threat_model.rs` is the first security control test. The verification ids (`SEC-…`) in each abuse case become executable tests as their owning subsystem is implemented; a mapped test may not be deleted without replacing the mapping and reviewing the residual risk.

Production release gates require formatting/lint and Rust/Node lockfile audits; unit, integration, cross-tenant, redaction, storage-conformance, policy-path, auth-abuse, edge-isolation/egress, and recovery tests relevant to the release; no unsupported storage/runtime capability; reviewed migration, rollback, key rotation, tombstone retention, and incident-response impact; and signed artifacts and provenance tied to the reviewed commit. Security regressions are release blocking. A flaky security test is treated as an unresolved control failure, not skipped indefinitely.

### Detection and response

Security events use bounded schemas with project/environment, actor class, operation, decision, policy/config version, request/correlation id, and reason code — never secret or document payloads. Alerts cover cross-scope attempts, authorization anomalies, auth spraying/reuse, support grants/impersonation, sandbox/egress violations, audit gaps, configuration divergence, storage conformance failures, unusual tenant resource saturation, secret canaries, and release verification failures.

Response favors containment that does not damage unrelated tenants: deny the request, revoke the narrow identity/token family/capability, disable the affected route/function/version, quarantine a runtime node, or stop writes when consistency is uncertain. Evidence is preserved in the protected audit sink. Customer notification, credential rotation, repair, and retrospective scope follow the incident classification.

#### Public-beta ingress boundary

For `cloud-test.makodb.com`, the default and current security posture is no public application listener. Caddy is disabled on VM `124`, and an active, boot-persistent Proxmox bridge rule drops destination TCP 80/443 independently of the guest. Management SSH is key-only and limited to discovered operator addresses; all Mako, RocksDB, rootless dependency, Prometheus, and Grafana listeners are private or loopback-only. The admission stop retains the VM, disks, backups, releases, logs, and evidence so containment does not become an unreviewed destructive action.

Restricted HTTPS admission, when approved, still crosses `TB-01` and cannot bypass Mako authentication, document policy, tenant binding, quotas, request limits, or audit. Only the checked OpenAPI and function route allowlists may be proxied; health and `/_internal/v1/` routes remain unavailable. No ACME failure may create a plaintext application fallback. Unrestricted admission requires a trusted certificate, completed renewal and hosted qualification, a passing 30-day gate, and an explicit approval bound to the selected release digest.

### Residual risk and review triggers

Initial quotas, token lifetimes, retention windows, and edge resource profiles remain tunable and require load/abuse-test evidence before hosted launch. New identity providers, storage adapters, query operators, policy context fields, regions, dependency endpoints, edge runtime APIs, operator capabilities, or data export/import paths trigger threat-model review. So do any incident, control failure, or architectural change that adds a privileged identity, sensitive data class, or boundary.

### Developer registration and the wait list

Mako Cloud **authentication identities** own email verification and credentials. Developer admission is an optional **role** on that identity, independent of operator entitlement. Both remain independent of the application users a project authenticates; a matching email address does not link those identity classes.

Self-registration creates an `unverified` identity and queues one verification message. A valid, single-use verification token moves that identity only to `waitlisted`. A wait-listed sign-in gets the `mako-developer-waitlist` audience, which can call only the coarse self-status endpoint, refresh, recovery, and sign-out; management routes accept only `mako-management`. An operator with the separate `waitlist_review` permission can list applicants — including their own developer application — and atomically approve or reject one with a private reason and idempotency key. Approval advances the durable developer authorization epoch, revokes pending developer sessions, and requires a fresh developer sign-in; it leaves operator entitlement and sessions unchanged and does not create a team, project, membership, or quota grant. Every protected management request loads the current developer role and epoch, so an old or manually signed claim cannot bypass review.

Password change or recovery advances the shared credential epoch and invalidates both developer and operator sessions without changing either role. Account-wide suspension or deletion denies both; developer rejection or developer-only disablement does not remove operator access. Browser refresh credentials are rotating, digest-stored, `Secure`, `HttpOnly`, `SameSite=Strict` cookies; access tokens stay in memory or tab-scoped session storage; verification and recovery tokens arrive in URL fragments the console removes before rendering.

Registration, resend, and recovery-request responses are generic. Password work has bounded concurrency. Global, source-digest, normalized-email-digest, and token-attempt limits have finite retention. Applicant values do not become metric labels or ordinary log fields. The public status response contains only the caller's developer id and `waitlisted`. Verification, recovery, approval, and rejection mail is encrypted at rest in the control-plane outbox; the worker uses authenticated SMTP with verified TLS, deterministic delivery ids, leases, bounded retries, dead letters, and terminal-record retention. Registration fails closed if mail is missing, unready, or the pending outbox reaches its configured bound. A notification failure after an operator decision does not roll the committed lifecycle back.

The account/role authority split is recorded field by field: the shared identity owns `normalized_email`, the password hash, verification, and the credential epoch; the developer role owns `status`, review summary, and the developer epoch; the operator entitlement owns `operator_epoch` and permissions. Password recovery advances the credential epoch and invalidates both kinds of session; developer approval/rejection/disable advances the developer epoch and invalidates only developer sessions; operator replacement/revocation advances the operator epoch only. SQLite backup, restore, and compatible rollback capture every one of those keyspaces, and a RocksDB-only binary is never a rollback target after SQLite writes.

### Operator identity

The hosted console serves the control center at `/operator` as the canonical operator workspace; the staged HTML gate and `/operator/legacy` were retired, and an unknown or retired operator route fails closed with `404`. Operators sign in with the password of their verified, security-active authentication identity, which must also hold an explicit operator entitlement (`/v1/operator-auth/sessions`). The browser receives only a `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/v1` cookie; a session expires within one hour; privileged mutations require a password verification from the preceding five minutes (`verifyCurrentOperatorPassword`) and answer `operator_step_up_required` otherwise. A generic rejection does not identify whether the email, password, lifecycle, entitlement, or service state caused the failure.

Read permissions are `overview_read`, `tenant_read`, `operations_read`, `incident_read`, `backup_read`, `fleet_read`, `security_read`, and `activity_read`; mutation permissions are `incident_manage`, `recovery_manage`, `security_manage`, and `activity_export`, alongside `provisioning_repair`, `quota_override`, `abuse_response`, `support_access`, and `waitlist_review`. The observer role is read-only; responder adds incident management; security-administrator manages operator security; recovery-administrator manages recovery; administrator holds the complete bundle. The last entitlement holding `security_manage` cannot remove that permission. Entitlements are bootstrapped and changed only through the protected loopback plan/apply flow ([operator runbook](#runbook-operator-password-authentication)); break-glass bearer tokens are incident-only, read-only, and fail closed by default.

Guarded workflows carry an operation key, reviewed resource version, reason, optional case reference, confirmation, and an action binding (SHA-256 over the action, target, and reviewed version), so proof for one operation or a stale page cannot authorize another. Recovery creation and promotion are independently disabled by default (`MAKO_OPERATOR_RECOVERY_CREATE_ENABLED`, `MAKO_OPERATOR_RECOVERY_PROMOTE_ENABLED`); recovery jobs accept a verified backup reference and protected logical target, use an explicit state machine, and call only typed executor methods. Activity exports expire after one hour and contain no document bodies or secrets. Projection rebuilds are idempotent and record source count, projected count, and checksum evidence; offline backfill uses `mako-operator-projection` against an explicit snapshot copy.

---

## Deployment

### Topology and the availability contract

Mako Cloud separates the control plane, data plane, edge-function plane, and telemetry store. The control plane owns one local SQLite database; data-plane, edge-gateway, and telemetry state remain in separate RocksDB databases. Every stateful database has **one process and one retained persistent volume**. Stateless gateways and edge workers may scale independently, but no two processes may open one database.

This topology has a single storage writer and no automatic failover, active-active writes, shared RocksDB directory, replicated durability, or zero-downtime storage-node replacement. A process restart on an intact volume causes a brief write outage. A lost node or volume causes a write outage until an operator restores and explicitly promotes a verified checkpoint. Never scale a stateful owner above one replica or mount its database claim in two pods.

The recovery objectives are release thresholds, not claims of replication: recovery point — newest eligible verified backup no more than 15 minutes old; same-volume recovery — ready within 5 minutes; replacement-volume restore and explicit promotion — ready within 30 minutes. A restored environment can lose acknowledged writes newer than the selected backup's manifest high water. Qualification reports must record measured RPO and RTO.

### Prerequisites

- Immutable service images built from one validated revision.
- TLS for public endpoints and authenticated private service traffic.
- A deployment secret reference for internal service authentication.
- One encrypted, retained, `ReadWriteOnce` volume per stateful service.
- A separate encrypted backup destination with retention and access controls.
- A pinned, qualified edge-runtime image digest.
- Prometheus-compatible metrics, alert routing, and access-controlled logs.

[`infra/production/storage-statefulsets.yaml`](../infra/production/storage-statefulsets.yaml) is the checked storage-topology example. Its CSI provisioner and image names are placeholders; replace them with deployment-owned values but retain encrypted storage, `reclaimPolicy: Retain`, volume expansion, `ReadWriteOnce`, claim retention, and `replicas: 1`. Run `npm run validate:production-storage` after rendering changes. Do not apply the file unchanged to a cluster.

### Stateful rollout

1. Provision each RocksDB retained volume with `mako-storage-ops provision`, the exact service identity, and explicit `PROVISION` confirmation. Provision the control SQLite volume and private directories separately; production control startup requires a pre-created or verified migrated SQLite authority.
2. Mount the database and backup paths at the absolute paths in typed service configuration. Never select a memory backend or create a database on failed open.
3. Start exactly one replica. Readiness remains false until the ownership and format markers, exclusive lock, capacity reserve, synchronous durability, engine health, semantic contract, and acknowledged high water are verified. SQLite additionally requires its application/database identity, supported schema, integrity, WAL/capacity limits, and exclusive lock.
4. Admit traffic only after the service and gateway readiness checks pass.
5. Take and verify a checkpoint backup before and after a storage-affecting rollout.

Use a graceful termination window long enough to stop admission, drain bounded work, checkpoint SQLite or flush RocksDB, and close the engine. Keep the volume claim when a StatefulSet is deleted or scaled.

### Stateless and edge rollout

Gateways can have multiple replicas after their token-verification, revocation-freshness, quota, and downstream health dependencies are ready. Function traffic resolves an immutable healthy version before invoking the pinned runtime. A promotion changes one version pointer atomically; a failed deployment leaves the prior version active. Selected edge regions may fail over only to another healthy selected region; they do not make the stateful owner multi-region. Data writes stop while the owning node or volume is unavailable.

### Configuration and secrets

Follow the [configuration reference](#configuration-reference). Production public URLs must use HTTPS, storage and backup paths must be absolute and separate, and secrets must use `env:` or `file:` references. Store project signing keys, service credentials, backup authentication material, and function secrets in a deployment secret system; never place plaintext values in manifests.

Developer-registration releases are installed with registration disabled. Apply and verify the identity migration, existing active access, backup/restore, authenticated SMTP, pending isolation, operator review, and a fresh active sign-in before enabling it. Preserve additive identity and mail-outbox keyspaces during rollback ([runbook](#runbook-developer-registration-and-mail)).

### The public beta VM

The current beta target is the single Ubuntu 24.04 VM `124` at `130.245.173.11` on a Proxmox host, with the exact public origin `https://cloud-test.makodb.com`. It uses a service-owned control SQLite path and distinct service-owned tenant RocksDB paths on the VM data disk (`/srv/mako-data`). Authenticated SQLite backups and signed RocksDB checkpoints are copied to a destination outside the VM failure domain every five minutes. This deployment is not a Kubernetes or distributed-KV deployment and does not provide HA.

The pieces:

| Piece | Where | Does |
| --- | --- | --- |
| Plan | `infra/proxmox/public-beta/{request,vm-settings,image-pin}.json`, `plan.schema.json` | The hash-bound VM plan; `npm run test:proxmox-plan` validates schema and collision fixtures |
| Cloud-init | `infra/proxmox/public-beta/cloud-init`, `npm run render:proxmox-cloud-init` | First boot: users, key-only SSH, disks |
| Apply / teardown | `npm run proxmox:public-beta` (`scripts/proxmox/public-beta-apply.js`), `npm run plan:public-beta-teardown` | Read-only planner, hash-bound VM apply, non-executing teardown planner; each writes evidence |
| Host firewall and admission stop | `scripts/proxmox/install-public-beta-host-firewall.sh`, `public-beta-admission-stop.sh` | The Proxmox bridge rule that drops TCP 80/443 to the VM independently of the guest |
| Backup target | `scripts/proxmox/install-public-beta-backup-target.sh`, `-retention.sh` | The off-VM checkpoint destination and its retention |
| Convergence | `infra/ansible/playbooks/public-beta.yml`, roles `base`, `storage`, `dependencies`, `configuration`, `runtime`, `observability`, `backup`, `firewall`; `infra/ansible/group_vars/public_beta.yml` | The pinned Ansible playbook that renders configuration, installs releases, the rootless Podman dependencies (`mako-dependencies.target`), the edge runtime quadlet, systemd units, checkpoint timers, Prometheus, Grafana, Alertmanager, and Caddy |
| Release | `npm run build:public-beta-release` (`scripts/build-public-beta-release.js`) | The immutable release directory — service binaries, console bundle, the edge runtime's main worker — named by digest and recorded in `docs/evidence/public-beta-release-manifest.json` |
| Release operations | `/usr/local/sbin/mako-release-operation` (`infra/ansible/roles/runtime/files/`) | `inspect`, `snapshot-current`, `upgrade DIGEST`, `rollback DIGEST` on a running stateful environment |
| Admission guard | `mako-public-preview-admission` and its one-minute timer; `scripts/public-beta-preview-approval.js` | Selects one of four rendered Caddy configurations atomically and falls back to `pre_gate` |

`infra/ansible/group_vars/public_beta.yml` is the non-secret desired-state selector. Its safe defaults are `mako_public_admission_mode: disabled`, `mako_caddy_enabled: false`, no tester CIDRs, no admission approval, and no ACME contact. Do not work around those defaults by editing a generated Caddyfile or adding a plaintext listener. Certificate configuration requires an operator-provided ACME contact and explicit approval for the restricted pre-gate; HSTS is enabled only after the production chain, hostname, redirect, route allowlist, and renewal checks pass.

**Admission** has four explicit values: `disabled`, `pre_gate`, `risk_accepted_preview`, and `approved_beta`. Preview is not beta approval and does not change any release-gate threshold. It requires a digest-bound approval from `npm run public-beta:preview-approval` naming the exact operator, release, plan, and blocker set. The acceptance has no calendar expiry and remains active until an operator pauses it, the exact release or blocker binding changes, or a non-waivable safeguard fails. Only incomplete SMTP delivery, latency targets, and the 30-day capacity/cost evidence may be accepted; trusted HTTPS with HSTS, the exact route allowlist, application security, backup and recovery, zero acknowledged-write loss, zero integrity failures, readiness, and the emergency stop are never waivable. Convergence renders all four Caddy configurations; the guard selects one atomically and falls back to `pre_gate`. Never hand-edit its symlink or a rendered Caddyfile.

Transactional developer mail and operator alerts both use the verified Resend domain but remain separate delivery paths. The control plane loads its authenticated SMTP password through its own systemd credential. Alertmanager loads a protected `alert-smtp-password` credential, requires STARTTLS on port 587, and never stores the credential value in its rendered YAML or retained evidence. Application mail readiness does not by itself prove operator-alert delivery.

Beta third-party credentials and the operative copies of secrets live outside the repository (`.local/public-beta-secrets/*.env` locally, systemd credentials on the guest) and are never committed; `npm run scan:public-beta-secrets` proves it.

#### Deploying a release to the VM

1. `npm run validate:public-beta-local` — offline; checks local assets and fixtures only, never contacts Proxmox, applies a plan, enables admission, or executes teardown.
2. `npm run build --workspace @mako-cloud/console`, then `npm run build:public-beta-release` to produce the candidate and its digest.
3. Select the digest in `infra/ansible/group_vars/public_beta.yml` (`mako_release_digest`), install it immutably, and converge the pinned playbook with Caddy disabled. The normal deployment path must not recreate the VM, replace either disk, delete a release, or alter DNS.
4. On a running stateful environment, change releases **only** through `sudo mako-release-operation upgrade DIGEST --confirm=UPGRADE_RELEASE:DIGEST` ([release operations](#release-operations-on-the-vm)); it must not overwrite `/opt/mako/current` or create an empty storage fallback.
5. Require verified checkpoints, active volume markers, HTTP 200 from all four private readiness endpoints, active backup timers, and the Proxmox admission-stop service after the operation.
6. After deploy, **probe the new code path** — a ready endpoint proves the process is up, not that the change is live — and expect brief `502` windows while the five-minute checkpoint timers hold the services.

The candidate carries the service binaries, the console bundle, and the edge runtime's main worker — the supervisor that authenticates deployments, supplies the SDK module to every user worker, and sets each worker's permissions. The release operation installs that worker and restarts the runtime when it differs, so what runs tenant code is named by the release digest; the provisioning role's copy is for first boot only. After SQLite accepts control writes, release selection rejects binaries that do not declare compatibility with the active control SQLite format.

Public HTTPS remains a separate gate: a deployed and ready VM is not approval to enable ingress.

---

## Operations

### Production RocksDB operations

#### Provisioning

```bash
mako-storage-ops provision \
  --database-path=/var/lib/mako/rocksdb \
  --service=mako-data-plane \
  --database-id=mako-data-plane-us-east-1 \
  --confirm=PROVISION
```

Use `--dry-run` first. `--accept-matching-marker` makes deployment initialization idempotent only when owner and format match; it never accepts an unmarked, non-empty, wrong-owner, or wrong-format path. Startup remains unready if the volume is absent, locked, corrupt, read-only, below its critical reserve, or fails sequencer/semantic recovery; it never initializes a fallback database.

#### Capacity expansion

Alert on the configured warning reserve and stop discretionary writes before the critical reserve. Check pending compaction bytes, delayed write rate, and backup headroom together, and measure each btrfs subvolume separately (`du -x` stops at subvolume boundaries): the backup staging and publish roots are on the same disk as the databases, and a leak there starves the data plane exactly as data growth would. Expand the existing retained claim through the CSI provider; do not replace its path. Confirm the filesystem sees the expansion, the critical alert clears, compaction pressure drains, and a new verified backup completes. Do not delete SST or WAL files manually.

#### Graceful shutdown and same-volume restart

Remove the owner from routing and wait for in-flight mutations. Let the service complete synchronous writes and graceful shutdown before terminating the process. Reattach the same claim to exactly one replacement. Readiness waits for RocksDB recovery, ownership/format checks, semantic checks, sequencer gap recovery, and acknowledged-high-water verification. If the old process may still run, fence it at the orchestrator and node level before attaching the volume.

#### Checkpoint backup

The deployment supplies at least 32 bytes of signing material in a protected file and encryption/access control for the backup destination. The key is not a command argument and never appears in manifests or diagnostics.

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

Success means the immutable uploaded copy was read back, authenticated, and all file digests verified. The checkpoint is staged under `--staging-root` as `{backup-id}.incomplete`, copied into the destination, and the staging copy is **removed once the published artifact verifies**; the next backup also sweeps any `*.incomplete` leftover a crashed run left behind, so the staging root holds at most one checkpoint. (Before 2026-09-19 nothing removed the staging copy: every five-minute checkpoint on the beta left one behind, 11 492 of them filled the 256 GiB data disk, and the data plane failed closed at its critical reserve — see the [public beta runbook](#runbook-public-beta-environment).) Monitor backup failures, age, and the data disk's free bytes together. Inspect or verify without opening a database:

```bash
mako-storage-ops inspect --artifact=/var/lib/mako-backups/data-plane/DP_ID --signing-key-file=/run/secrets/mako-backup-signing-key
mako-storage-ops verify  --artifact=/var/lib/mako-backups/data-plane/DP_ID --signing-key-file=/run/secrets/mako-backup-signing-key
```

On the beta the data-plane and control-plane timers create signed checkpoints every five minutes and remotely reverify them on the Proxmox-host destination; a backup is successful only when `mako_storage_backup_remote_verified` is 1 and age remains below 900 seconds.

#### Empty-target restore and node replacement

Fence and stop the original owner. Provision an offline, existing, empty target; never restore in place. Run a dry run and then confirm:

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

Restore authenticates the manifest, validates every file, opens a private staged database, checks tenant inventory, and recovers each sequencer through the manifest high water. It then writes a `Promotable` marker. It cannot serve until the old owner is fenced and an operator runs:

```bash
mako-storage-ops promote \
  --database-path=/var/lib/mako/replacement-rocksdb \
  --service=mako-data-plane \
  --database-id=mako-data-plane-us-east-1 \
  --confirm=PROMOTE
```

(`mako-storage-ops fence … --confirm=FENCE` marks a volume fenced.) Attach the replacement to one process and wait for full readiness. Preserve the old volume and failed staging paths for investigation; never point traffic at a blank replacement merely because RocksDB can create it.

#### Corruption response and binary rollback

On a corruption or repeated I/O signal, stop routing and mutations, fence the owner, preserve logs and the volume, and do not run repair commands against the only copy. Verify the newest backup and restore into an isolated empty volume. Escalate possible tenant-boundary mismatches as a security incident.

For binary rollback, stop writes, take and verify a checkpoint, and confirm the previous binary supports the recorded database format. Restart it against the same fenced volume and require readiness before traffic. If the volume cannot be used, restore a backup produced by the compatible format, verify, and explicitly promote. Never roll back by selecting memory storage or an empty database.

### Control-plane SQLite operations

**Readiness, contention, WAL, and capacity.** Keep the control service stopped if identity, schema, integrity, migration, or capacity checks fail. Inspect with `mako-control-storage-ops inspect-sqlite --database … --identity …`; do not copy, edit, vacuum, or recreate the live file. Busy failures are retryable only within the caller's bounded policy; investigate long transactions before changing timeouts. On WAL pressure, stop new mutations, allow graceful shutdown and checkpoint, and verify integrity. On critical capacity, preserve the root reserve for recovery, stop mutation traffic, and add space before restarting.

**Offline migration and cutover.** Pause public control mutations; checkpoint the old control RocksDB; verify and copy it off the VM; stop and fence the control plane; calculate the immutable checkpoint digest; write the fence marker; create a plan bound to the exact source, temporary/live target, lock, database identity, release digest, and configuration digest; inspect the source; then run the confirmed `mako-control-storage-ops migrate --plan …`. Require matching byte count, framed BLAKE3 checksum, prefix inventory, SQLite integrity, receipt digest, and domain probes before selecting SQLite. Never allow an empty fallback. The beta's cutover evidence is `docs/evidence/public-beta-control-sqlite-cutover.json`.

**Backup and restore.** Run `mako-control-storage-ops backup` with the live database, dedicated staging and publish directories, the exact release digest, a safe backup id, retention, and the protected signing-key file. Verify the locally published and off-VM copies. `restore` only to a nonexistent offline target with matching identity/release and maximum age; inspect it before `promote`, which refuses any existing live target.

**Corruption and rollback.** Stop the service, preserve database/WAL/SHM and logs read-only, identify the newest authenticated backup meeting high-water and age policy, restore to an empty target, verify domain invariants, and explicitly promote during a maintenance window. After SQLite accepts any production write, select only releases that declare support for the current SQLite format; a RocksDB-only release is not a rollback target, and returning to RocksDB requires a separately designed and verified reverse migration.

### Release operations on the VM

`mako-release-operation` runs as root on the guest and knows the four services, the two checkpoint timers, the RocksDB components, the ports, the control database and lock, and the runtime unit.

```bash
sudo mako-release-operation inspect
sudo mako-release-operation upgrade  DIGEST --confirm=UPGRADE_RELEASE:DIGEST
sudo mako-release-operation rollback DIGEST --confirm=ROLLBACK_RELEASE:DIGEST
sudo mako-release-operation prune                                            # housekeeping; also runs after every switch
```

A release is ~93 MB of immutable binaries, console bundle, and runtime worker under `/opt/mako/releases/<digest>`, and it runs on the **same** data volume as every release before it — a switch fences and re-activates the databases in place; nothing is copied per release, and the checkpoint the switch takes lands in the bounded 14-slot backup ring the timers fill anyway. What did accumulate was the releases themselves (the beta reached 104). Every successful switch therefore ends with `mako-release prune --keep 5`, which keeps the selected release, the last-known-good release, and the five newest by build time and removes the rest along with their configuration snapshots under `/etc/mako/release-configs/`. Pruning is housekeeping after the recorded switch, never part of it: a pruning failure is reported and leaves the switch complete. `npm run test:release-retention` exercises the manager against a private root; `validate:public-beta-infrastructure` asserts the order of events.

First verify Caddy is inactive and the Proxmox `mako-vm124-admission-stop.service` is active and enabled; `inspect` shows the current and last-known-good immutable digests. The operation drains all four services, pauses backup timers, checkpoints and fences every RocksDB path as its service owner, rejects incompatible or empty state, snapshots the outgoing release's three service JSON documents under its digest (rollback restores the target release's snapshot before starting its binaries, so a newer additive configuration section cannot strand the prior release), atomically changes `/opt/mako/current`, installs the release's edge-runtime main worker and restarts the runtime when it differs, waits for all private readiness endpoints, and resumes the timers. If any check fails, public admission remains off and the operation evidence is retained under `/var/lib/mako-release-operations/` for diagnosis. Operator keyspaces are additive and must remain untouched.

### Observability

#### Dashboards

The local stack provisions read-only Grafana dashboards in the **Mako Cloud** folder (`infra/local/grafana`); open `http://127.0.0.1:3000` after starting the compose stack.

| Dashboard | Primary operator question |
| --- | --- |
| Service Health | Are services ready, and is request latency healthy by service and region? |
| Saturation | Are concurrency slots or work queues approaching capacity? |
| Replication Lag | How far are RxDB clients behind committed high water? |
| Error Rates | Which service and safe error class is failing? |
| Sequencer Gaps | Are unresolved commit positions blocking visibility? |
| Revocation Freshness | Can each gateway prove its revocation cache is fresh? |
| Live Streams | Are SSE streams connected, buffered safely, or forcing resynchronization? |
| Index Builds | Which index versions are progressing or failing? |
| Function Workers | Are isolates saturated, recycling, or failing invocations? |
| Mako Production RocksDB | Is the owned database ready, locked, durable, within capacity, backed up, and recovering within objective? |
| Control-plane SQLite | Is control authority ready and intact, and are transactions, WAL, capacity, migration, backup, and restore healthy? |

The operator control center links these dashboards as bounded, allowlisted diagnostics rather than embedding arbitrary queries in the browser.

#### Metric contract

Services export OTLP metrics to the collector, which exposes Prometheus-compatible series on port 9464; the control plane also serves `/metrics` directly. Tenant-scoped series use `project_id` and `environment_id`; regional service series use `service` and `region`. Bounded domain labels such as `collection_id`, `index_name`, `outcome`, `reason`, and `error_class` are used only where a dashboard needs them. Actor, request, trace, session, document, raw URL, email, token, and secret values must never become metric labels; request and trace identifiers belong in structured logs and traces, where the shared redactor and access controls apply. Producers must preserve the exact `mako_*` names the provisioned dashboards query or update dashboard and producer together. `npm run validate:observability` validates the dashboard inventory, JSON shape, data-source binding, provisioning mount, forbidden labels, and every alert rule's runbook link.

Worker counters on the control plane's `/metrics`: `mako_webhook_deliveries_total{outcome}`, `mako_webhook_endpoint_pauses_total`, `mako_webhook_worker_failures_total`; `mako_function_schedule_runs_total{outcome}`, `mako_function_schedule_worker_failures_total`; `mako_custom_domain_checks_total{outcome}`, `mako_custom_domain_revocations_total`, `mako_custom_domain_publish_failures_total`; `mako_application_mail_{stored,delivered,retried,dead_lettered,worker_failures}_total`; `mako_operator_control_center_{requests,failures,unavailable}_total` and its latency counter. Control SQLite exports `mako_control_sqlite_*` readiness, integrity, schema, file/WAL size, free-space thresholds, active/oldest transaction, busy, checkpoint, migration, backup, restore, and restart signals, none of which contain paths, keys, values, emails, credentials, or customer identifiers.

#### Alerts and the unproduced-metric gate

Prometheus loads the release-blocking alert inventory from `infra/local/prometheus-rules/mako-cloud-alerts.yaml`. Every rule links to a runbook in this book, and `validate:observability` checks each link resolves to a heading here.

Production storage is *specified* to export bounded service/volume gauges and counters for open/readiness and lock state, available and threshold bytes, write stops and delayed rate, compaction and flush work, background/I/O/corruption errors, backup result and age, restore verification, and recovery duration. **None of those `mako_storage_*` series is published yet.** Only the control plane serves `/metrics`; the data plane, edge gateway, and telemetry-query expose none, and the deployment's textfile collectors publish backup and health series only. Fifteen of the checked alert rules therefore watch metrics with no producer, ten of them critical, covering tenant isolation, audit write failure, corruption, sequencer gaps, and restore verification. A Prometheus expression over an absent series does not error: a threshold comparison never fires, and an `unless on()` absence check fires forever.

`npm run validate:alert-metrics` checks every alert expression against the metric names the workspace and the deployment actually publish. Rules with no producer are listed in that script's `UNPRODUCED` map with a reason. The list may only shrink: the gate fails if an entry starts being published, if it names a rule that no longer exists, or if a new untracked rule watches an absent metric. Delete an entry when its producer lands, and never add one to make the gate pass. The open change `openspec/changes/observable-data-plane-and-restore-verification` is where the producers land.

#### Project logs

A function's printed output is collected off the request path: the control plane reads each deployed function's runtime-supervisor buffer on a short cadence and carries new lines into the retained telemetry store, where they are served by the project logs endpoint with the same retention and tenant scoping as every other signal. The supervisor's buffer is bounded and in-memory; the telemetry copy is durable. Log text is written by customer code, so it is scrubbed **at the store** — no producer can bypass it — masking configured secrets, bearer and JWT values, password and cookie assignments, platform credential formats, and email addresses (local part masked). It is best-effort by design.

### Application and developer mail

The control plane's mail worker thread delivers two outboxes with one discipline: the developer outbox (wait-list, verification, recovery, approval, rejection) and the application outbox (verification, recovery, invitation, magic link, drained from the data plane's intents over internal RPC). Both are encrypted at rest under the developer-mail key (`MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF`, else the internal-auth secret) with associated data binding each record to its id, tenant, and kind; both share the lease, attempt, backoff, and retention settings (`MAKO_DEVELOPER_OUTBOX_*`, `MAKO_DEVELOPER_DELIVERED_MAIL_RETENTION_SECONDS`, dead letters kept four times as long). On the beta both go through Resend's authenticated SMTP; locally, `MAKO_DEVELOPER_SMTP_TLS_MODE=plaintext` to Mailpit. Failure handling is the [registration and mail runbook](#runbook-developer-registration-and-mail).

Code: `crates/mako-control-plane/src/email_template.rs` (kinds, defaults, validation, rendering, authorization), `application_mail.rs` (the outbox record and store, the intent source, the worker), `services/mako-control-plane/src/email_template_http.rs`, `main.rs` (runs the worker), `smtp.rs` (builds the transports), and `mako-internal-rpc`'s drain/acknowledge contract, served by the data plane.

### The control-plane workers

| Worker | Cadence | Reads | Notes |
| --- | --- | --- | --- |
| Webhooks | every 2 s | `read_change_feed` per active endpoint (positions, revisions, event kinds only) | Intake, delivery of what is due (≤ 50 per endpoint per pass), retention. A data plane that cannot be reached is reported once a minute and retried |
| Schedules | every 5 s | Its own due index | ≤ 20 due schedules per pass; invokes over `POST /_internal/v1/edge/function-schedule-invoke` on the gateway (`MAKO_EDGE_GATEWAY_ENDPOINT`), 60 s timeout, 90 s lease |
| Custom-domain verification | every 60 s | The host resolver (`MAKO_DNS_RESOLVER`) | Publishes the verified list to the data plane and gateway; answers the proxy's `GET /_internal/v1/custom-domains/ask` |
| Mail | continuous, leased | Data-plane intents; both outboxes | See above |
| Function logs | short cadence | Each deployed function's supervisor buffer | Carries lines into the telemetry store |
| Provisioning sweep | periodic | Provisioning workflow records | Heals records stranded in `provisioning`; installs plan limits at activation |

### Operator control center operations

The control center at `/operator` is a metadata-only, audited workspace. The OpenAPI contract defines overview, tenant directory, Tenant 360, inventory, alerts, incidents, recovery jobs, activity and exports, projections, security, provisioning, quota overrides, abuse responses, support sessions, and wait-list review. Inventories use opaque cursors, a maximum page size of 100, stable key ordering, bounded time windows, explicit observation times, and `current`, `stale`, `unknown`, or `unavailable` source state; a provider failure affects only its section and never implies a healthy state. Diagnostic links are emitted only for origins in `MAKO_OPERATOR_DIAGNOSTIC_ORIGINS`.

Offline deployment-snapshot backfill and shadow validation use `mako-operator-projection`, which accepts only an explicit offline RocksDB snapshot, pages every scan, writes only to that snapshot copy, and emits source/projection counts and checksums (`docs/evidence/operator-control-center-projection-qualification.json`). If the control center is unsafe or misleading, stop public admission, take and verify a checkpoint, and use the immutable release rollback; the legacy page is no longer a runtime rollback mechanism (`docs/evidence/operator-control-center-legacy-retirement.json`). Qualification before enabling a mutation gate covers OpenAPI generation, Rust and console tests, threat-model tests, observability validation, permission-specific navigation, cross-tenant denials, stale step-up, provider outages, sensitive-URL checks, and production-like HTTPS routing.

### Retention jobs

Schedule `RetentionJob` per tenant with bounded collection targets, run `DryRun` first, retain the reports as operational telemetry, and alert on repeated failures or a growing eligible backlog. A failed apply pass is safe to retry ([invariants](#retention-and-compaction)).

---

## Runbooks

These procedures are linked from the Prometheus rules in `infra/local/prometheus-rules/mako-cloud-alerts.yaml`. Start with the alert labels and time range, keep request and trace correlation in access-controlled telemetry, and never paste tokens, function secrets, passwords, email addresses, or document bodies into incident notes.

### Runbook: storage contract failure

Severity: critical. Owner: storage on-call. The affected data plane must remain unready while required ordered scans, snapshots, atomic writes, conditional writes, or configured durability cannot be proven.

**Triage.** Identify the affected `service` and `region` from `MakoStorageContractFailed`; confirm the readiness failure and its safe diagnostic. Check adapter health latency, the advertised capability set, strongest acknowledged durability, and recent storage I/O or restart events. Compare the last acknowledged commit high water with the adapter instance being checked; do not infer that an empty or newly opened database is current.

**Containment.** Keep the instance out of readiness and drain new data-plane traffic from it. Stop automated failover if the destination has not independently proved both the adapter contract and possession of acknowledged state. Do not switch to another directory, weaken durability, or disable capability checks to restore availability.

**Recovery.** Repair connectivity, credentials, disk state, or the adapter implementation without changing the semantic contract. Run storage readiness and the adapter conformance suite against the exact candidate backend. Restore traffic only after readiness is continuously healthy and the acknowledged commit high water is present. Escalate to incident command if acknowledged data is absent, durability cannot be verified, or multiple regions fail simultaneously.

### Runbook: unresolved commit gaps

Severity: critical. Owner: data-plane on-call. A gap prevents committed high water from advancing, so RxDB pulls and live streams may lag without losing ordering guarantees.

**Triage.** Scope the alert by project and environment, then inspect unresolved gap count, oldest gap age, committed high water, and writer health. Determine whether each position belongs to an active lease, a transaction awaiting publication, a crashed writer, or a durable abort record. Check storage health before treating the issue as a sequencer-only fault.

**Containment.** Preserve the current high water; never skip, overwrite, or manually mark a position committed merely to reduce lag. Pause retention/compaction for the affected keyspace and limit new writes if the gap backlog is growing. Let reads continue only at the last proven committed high water; live clients may receive the normal resynchronization signal.

**Recovery.** Recover the owning writer or allow lease-expiry recovery to classify the position from durable transaction evidence. Record an abort only when the mutation transaction is proven absent; publish a commit only when every atomic mutation artifact is present. Verify contiguous high-water advancement, replication lag recovery, and restart behavior before closing. Escalate if durable evidence is contradictory or a gap survives its lease and recovery deadlines.

### Runbook: policy evaluation failures

Severity: warning, escalating to critical when broad or sustained. Owner: policy/security on-call. Evaluation timeout, unavailable state, and internal failure must continue to deny access rather than bypass policy.

**Triage.** Identify affected services and regions, then separate timeout, unavailable-policy, invalid-context, and evaluator-error outcomes. Check active policy versions, authorization epochs, compiler diagnostics, evaluator saturation, and recent policy activations. Confirm all affected public operations are failing closed and no service/operator bypass was implicitly enabled.

**Containment.** Halt further policy promotions in the affected environment. If a newly activated policy caused the incident, use the normal audited rollback to the last validated version. Scale or restart unhealthy evaluators only after preserving safe diagnostics and correlation identifiers.

**Recovery.** Re-run policy validation and example tests against the collection schema. Confirm point reads, indexed queries, replication, live delivery, and edge SDK calls produce identical decisions. Verify the authorization epoch and invalidation stream converge before declaring recovery. Never mitigate by enabling a public bypass or including protected document bodies in diagnostics.

### Runbook: authentication refresh replay

Severity: critical. Owner: identity/security on-call. The identity service should revoke the full token family when a rotated refresh credential is replayed.

**Triage.** Scope the alert to project/environment and inspect sanitized audit outcomes, affected family counts, source network class, and correlation identifiers. Confirm the replay path revoked the family and gateways received the ordered revocation event within the freshness bound. Determine whether the signal is an isolated client race within the configured grace window or credential theft; never retrieve or copy the raw refresh value.

**Containment.** Revoke affected sessions or the application user if family revocation did not converge. For a broad campaign, apply the auth endpoint throttle, suspend affected credentials, and engage the security incident process. Preserve append-only auth audit records and access-controlled request traces.

**Recovery.** Verify fresh sign-in succeeds while every token in the compromised family remains rejected. Confirm revocation-cache freshness in every gateway region. Review client refresh concurrency and grace configuration before changing it; do not expand grace to hide replay signals. Rotate project signing keys only if evidence shows key compromise, not merely refresh-token theft.

### Runbook: developer registration and mail

Use this for registration readiness, a growing wait list, mail delivery failures, emergency registration disablement, operator review, recovery, and rollback.

**Enablement.**
1. Keep `mako_developer_registration_enabled: false` while installing the release and running the developer identity migration.
2. Confirm identity/email-index parity, current active developer access, the separate operator boundary, a current checkpoint, and an empty-target restore result.
3. Install mode-`0600` controller sources for `developer-mail-encryption` and the authenticated SMTP password. Configure a verified-TLS relay, port, username, and sender as one atomic change.
4. Prove SMTP readiness and deliver verification, recovery, approval, and rejection fixtures. Prove TLS verification failure, transient retry, lease recovery, dead-letter alerting, token expiry, and absence of plaintext fallback. Retain only sanitized delivery ids and aggregate outcomes. For the public beta, Resend is used through the authenticated SMTP adapter; qualify the separate Alertmanager Resend path independently by sending a bounded certificate-expiry fixture — transactional-mail readiness is not alert-delivery evidence.
5. Set registration enabled, converge in `pre_gate`, and run registration through verification, pending product denial, operator approval/rejection, fresh active sign-in, restart, backup, restore, and rollback. Obtain a new exact-release public-preview approval before admission.

The hosted beta operator uses password sign-in at `https://cloud-test.makodb.com/operator`; there is no token-paste field. The developer identity must be active and verified and must have a separate `waitlist_review` entitlement. Every decision requires recent password verification, an explicit private reason, idempotency, and confirmation. Treat the detail API's committed lifecycle as authoritative even if notification mail is delayed. Approvals need the password cookie, not break-glass; recovery mail can be read through the mail provider's API when Mailpit is not in the path.

**Mail failure or outbox pressure.**
1. Disable new registration immediately if mail readiness is false, pending depth/age is growing, dead letters appear, or credential/TLS validation fails. Existing active sign-in remains available.
2. Do not replay lifecycle transitions or manually promote an applicant because mail failed. The durable worker retries only delivery. Approval/rejection remains committed.
3. Check aggregate worker outcomes, oldest pending age, dependency readiness, service restarts, and sanitized stable error classes. Never inspect decrypted bodies in routine incident response.
4. Correct the relay, credentials, sender authorization, DNS/provider policy, or network path. Prove readiness and one controlled fixture before restoring registration.
5. If the outbox bound remains reached, keep registration disabled, preserve a checkpoint, and investigate terminal-record cleanup and provider throughput. Do not raise limits without abuse and storage evidence.

**Recovery and rollback.** Password recovery is generic, single-use, expiry-bounded, advances the authorization epoch, and revokes every older developer and operator session; a wait-listed account stays wait-listed after recovery. Never use the developer bootstrap signer to manufacture an active account. For release rollback, disable registration, stop public admission, take and verify a checkpoint, and use the normal release rollback procedure. Preserve additive keyspaces and pending mail. Afterward, verify identity/index parity, lifecycle counts, positive epochs, bounded session/token/decision/outbox records, existing active access, operator review denial without permission, and a mail readiness fixture before reopening admission.

### Runbook: operator password authentication

Use this for routine operator sign-in, entitlement bootstrap and changes, password recovery, revocation, and incident-only break-glass access. Never put an email address, password, raw cookie, attempt key, private reason, internal signing secret, or bearer token in logs or retained evidence.

**Routine sign-in and revocation.** Routine operators open `https://cloud-test.makodb.com/operator` and use the password for their verified, security-active authentication identity, which must also have an explicit operator entitlement; its developer application may be absent, wait-listed, active, rejected, or developer-disabled. The browser receives only a `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/v1` cookie; a session expires within one hour; privileged mutations require a password verification from the preceding five minutes. A generic rejection does not identify the cause — check only aggregate metrics and sanitized audit classes. Password recovery/change, account-wide suspension/deletion, a credential-epoch advance, or an entitlement replacement/revocation invalidates existing operator sessions; developer approval, rejection, or developer-only disablement does not. Require a fresh password sign-in after a shared credential event; never restore an old cookie or session record.

**Protected entitlement plan and apply.** Run the release-owned client as root on the guest. The control-plane endpoint is loopback-only and the input and internal-auth files must be regular mode-`0600` files. The protected input is JSON with `operation` (`initial_bootstrap`, `grant`, `replace`, `revoke`, or `repair_bootstrap_developer_admission`), `targetEmail`, exact `permissions`, `privateReason`, `environmentBinding`, `idempotencyKey`, `activateWaitlisted`, and no `typedConfirmation` during planning.

```sh
umask 077
/opt/mako/current/bin/mako-operator-admin \
  --mode plan \
  --endpoint 127.0.0.1:8081 \
  --secret-file /etc/mako/credentials/internal-auth \
  --input /run/mako-operator-admin/request.json \
  > /run/mako-operator-admin/plan.json
```

Review the sanitized environment, opaque identity, separate developer/operator before-and-after states, and email/permission/request/operation digests. The client or deployment agent may carry the machine-generated `typedConfirmation` from plan to apply after a human gives a concise explicit approval; never ask the human to copy the opaque value. Keep the apply input unchanged and let the client read the mode-`0600` plan file:

```sh
/opt/mako/current/bin/mako-operator-admin \
  --mode apply \
  --endpoint 127.0.0.1:8081 \
  --secret-file /etc/mako/credentials/internal-auth \
  --input /run/mako-operator-admin/request.json \
  --plan-file /run/mako-operator-admin/plan.json
```

An identical replay returns `replayed: true`. A missing/unverified/ambiguous identity, changed environment or permission set, stale plan, or mismatched confirmation makes no change. Bootstrap and entitlement administration change operator state only; `activateWaitlisted: true` is rejected. The one-time `repair_bootstrap_developer_admission` operation additionally requires the protected `priorBootstrapIdempotencyKey`, empty permissions, and the exact original combined-bootstrap provenance; it accepts only an active developer record with no later review, plans `active → waitlisted`, and preserves credential, entitlement, operator epoch, and session records. The target must then be approved or rejected manually through `/operator` using the normal self-review safeguards.

**Incident-only break glass.** Hosted `operator_break_glass_bearer_enabled` defaults false. Enable it only through protected configuration in restricted `pre_gate`, with a recorded incident, and converge the service. The issuer requires a mode-`0600` incident-reason file, a non-empty least-privilege permission set, an expiry of no more than one hour, and a new mode-`0600` redacted evidence path:

```sh
/opt/mako/current/bin/mako-operator-session \
  --secret-file /etc/mako/credentials/internal-auth \
  --incident-reason-file /run/mako-operator-incident/reason \
  --issuer https://cloud-test.makodb.com/control-identity \
  --operator-id opr_OPERATOR_ID \
  --permissions tenant_read \
  --ttl-seconds 900 \
  --output /run/mako-operator-incident/session.jwt \
  --evidence-output /run/mako-operator-incident/issuance.json
```

Break-glass tokens can perform permitted reads only; mutations fail with `operator_step_up_required`, and that rule is not weakened during an incident. Disable bearer acceptance and converge immediately after recovery.

**Monitoring, recovery, and rollback.** Alert on sign-in failures/throttling, bootstrap failure, abnormal active-session growth, and any break-glass state that persists beyond the restricted operation. Keep public admission in `pre_gate` during bootstrap, password enablement, recovery tests, or rollback; audit/storage/readiness failures are fail-closed. Checkpoints include entitlements, epochs, protected session digests, attempts, and idempotency; restore only to an empty offline target and verify revoked/expired sessions do not regain authority. A format-compatible rollback leaves additive operator keyspaces untouched; if administration is required, use a bounded reason-bound break-glass token rather than changing storage directly. Do not remove a prior local token until the intended operator has completed one successful browser password sign-in against the exact candidate.

**Expired local-token cleanup.** Preview first, then `--apply` only after reviewing every reported path:

```sh
node scripts/cleanup-expired-operator-sessions.js --directory /home/OPERATOR/.local/operator-sessions
node scripts/cleanup-expired-operator-sessions.js --directory /home/OPERATOR/.local/operator-sessions --apply
```

The command accepts one explicit non-symlink directory, considers only regular mode-`0600` `.jwt` files whose bounded JWT claims have `mako-operator` audience and an elapsed `exp`, and never follows links or traverses subdirectories.

### Runbook: operator control-center source failure

Use this for stale/unavailable control-center cards, elevated request failures, or high read-model latency.

1. Confirm the affected card's provider, observation time, freshness, and bounded tenant/time scope. Unknown or unavailable is not healthy.
2. Check `mako_operator_control_center_requests_total`, `_failures_total`, `_unavailable_total`, and the total-latency counter. Keep request correlation in protected logs.
3. Check the service-health, error-rate, RxDB, and RocksDB dashboards for the same bounded interval. Follow only HTTPS diagnostic links generated by the server allowlist.
4. If a single source failed, leave successful sections visible and diagnose that dependency. Do not resolve an incident because an alert source disappeared or became stale.
5. For mutation failures, reload the durable resource before retrying. Preserve the operation key for an ambiguous retry; obtain a fresh password step-up if prompted. Never bypass version, target, backup-verification, or promotion gates.
6. If the workspace itself is unsafe or misleading, stop public admission, take and verify a checkpoint, and follow the release rollback procedure. Select the last compatible release; do not recreate `/operator/legacy` in place or delete additive incident, recovery, activity, workflow, or projection records.
7. Before restoring admission, verify operator password sign-in, `/operator`, `/operator/operations`, exact-project provisioning, quota, abuse, support-session and wait-list workflows, step-up retry, audit continuity, and that `/operator/legacy` remains absent on the current release.

Escalate immediately for tenant-scope mismatches, unsafe links, possible response leakage, recovery promotion without verified evidence, activity integrity gaps, or loss of the last recoverable security administrator.

### Runbook: developer data workspace incident

1. Identify the project/environment, request id, operation, mode, and safe grant/target fingerprints from audit. Never request a capability, document body, import file, exported data, public-key value, raw reason, or user email.
2. Disable only the affected rollout gate: workspace, explorer administration, data jobs, sync detail, or restore. Legacy collection/user/function/credential/observability routes remain the rollback path.
3. For suspected stale or stolen explorer authority, advance the affected developer epoch. For collection, policy, schema, index, project, environment, or application-user lifecycle drift, advance all tenant explorer epochs. Confirm the old nonce now fails and produces a denied audit.
4. For import/export incidents, cancel future work, preserve the job id and safe manifest/digest, and verify exact committed/failed/skipped/exported counts. Do not claim cancellation rolled back committed import rows. Keep downloads disabled until the complete artifact digest verifies.
5. For object-store outages, leave jobs retryable and allow idempotent retention cleanup to resume. Never publish a partial export or bypass the immutable tenant-bound address.
6. For restore incidents, keep the target inaccessible. Promotion and overwrite are prohibited for developer requests. Escalate verification/isolation failure through the operator recovery and production RocksDB runbooks.
7. Before re-enabling, verify audit continuity/redaction, bounded labels, cross-tenant denial, capability expiry/revocation, HTTPS allowlists, browser-storage absence, and the affected quota.

If import/export jobs remain `queued` with no progress, check the deployed
control-plane revision and its data-job worker before recreating jobs. The worker
runs in the control-plane process on a two-second loop and reports dependency
failures as `data-job worker pass failed: class=worker_dependency`. Its global
scan must include the tenant-specific `data-jobs/<project>/<environment>` domain
segments; an exact scan of the parent domain finds no jobs. Each pass scans at
most 1,000 keys and selects at most 16 actionable jobs, continuing across passes
and wrapping after the last page. Secondary indexes, completed jobs, and imports
awaiting upload/confirmation must not permanently hide later queued work. The
scan cursor is process-local; restart begins a new sweep while durable job state
and import progress remain authoritative. Worker discovery preserves existing
job keys, so deploying the corrected control plane requires no storage migration;
retention expiration and normal retry rules still apply.

### Runbook: tenant-isolation signal

Severity: critical/P0. Owner: security incident commander. Any impossible cross-project or cross-environment observation is presumed to be a confidentiality or integrity incident until disproved.

**Triage.** Page security, storage, and the owning service teams immediately; record service, region, release, and alert time. Use tenant ids and correlation identifiers to establish scope. Determine whether the signal came from key decoding, trusted-scope mismatch, cursor/checkpoint validation, cache ownership, object storage, or worker isolation.

**Containment.** Remove the implicated service instances and release from traffic. Suspend only the affected tenant paths when scope is proven; otherwise isolate the region. Keep immutable audit and storage evidence. Revoke exposed credentials and sessions based on evidence. Do not delete or compact relevant records.

**Recovery.** Reproduce the boundary violation with sanitized fixtures and identify the failed invariant. Patch the root cause and run tenant-boundary, key-codec, gateway, storage, policy, and worker-isolation suites. Restore traffic in stages while watching the isolation counter and audit stream. Complete disclosure, credential rotation, and forensic retention steps under the security response policy before closure.

### Runbook: edge sandbox incident

Severity: critical. Owner: edge-runtime/security on-call. Signals include cross-project memory/environment access, secret leakage prevention, denied metadata egress, resource abuse, or work continuing beyond invocation lifetime.

**Triage.** Identify region and bounded `reason`; correlate the invocation through access-controlled request/trace ids without retrieving secret values. Confirm the supervisor recycled only the compromised project worker and that neighboring tenants remained healthy. Inspect immutable deployment version, runtime digest, configured limits, outbound policy, and secret-version selection.

**Containment.** Disable or roll back the implicated function version and drain its project worker pool. Block suspicious outbound destinations at the runtime boundary; never relax the egress policy to aid diagnosis. Rotate only secrets that may have been exposed, and preserve redacted runtime/audit evidence.

**Recovery.** Reproduce with a non-sensitive fixture against the exact pinned runtime image. Run adversarial isolation, secret-redaction, egress, resource-exhaustion, crash, and post-lifetime-work tests. Re-enable the deployment gradually and verify worker generations, recycle rate, invocation failures, and neighboring tenant health. Escalate to platform security if isolation was bypassed, secret material reached output, or work survived worker termination.

### Runbook: production RocksDB incidents

1. Remove the affected stateful service from routing. Do not restart it against another or empty path.
2. Identify the bounded `service` and `volume` alert labels. Do not copy storage keys, values, signing material, or tenant documents into incident notes.
3. Confirm exactly one owner exists. If ownership or node state is uncertain, fence the workload and node before touching the volume.
4. For capacity or compaction pressure, stop discretionary writes, expand the existing retained claim, and watch available bytes, write-stopped state, pending compaction bytes, and I/O counters.
5. For lock loss, I/O errors, or corruption, preserve the volume and logs. Do not delete `LOCK`, WAL, manifest, or SST files and do not repair the only copy.
6. If the volume is intact, restart the same binary or a format-compatible rollback binary on that volume and wait for RocksDB plus sequencer recovery.
7. If replacement is required, verify an eligible backup, restore only to an empty offline volume, review its tenant/high-water report, and promote only after the previous owner is fenced.
8. Keep traffic disabled if backup age exceeds the RPO, restore verification fails, the target is unexpectedly empty, or recovery exceeds its recorded high water. Escalate tenant mismatch as a security incident.
9. For the control-plane volume, verify operator entitlements and authorization epochs survive while expired or revoked operator sessions remain unusable. Never edit operator keyspaces directly.

Commands are under [Production RocksDB operations](#production-rocksdb-operations).

### Runbook: control-plane SQLite operations

Readiness, contention, WAL, and capacity; offline migration and cutover; backup and restore; corruption and rollback are the four procedures under [Control-plane SQLite operations](#control-plane-sqlite-operations). The rule that governs all four: keep the service stopped while identity, schema, integrity, migration, or capacity checks fail; never copy, edit, vacuum, or recreate the live file; restore only to a nonexistent offline target; and after SQLite has accepted a production write, select only releases that declare support for its format. SQLite and tenant RocksDB share one VM and data disk in the beta; complete VM or disk loss requires off-VM recovery and is not masked by the storage split.

### Runbook: public beta environment

This covers VM 124 at `130.245.173.11` for `cloud-test.makodb.com`. It is a single-VM, single-region beta with no HA, automatic failover, or availability guarantee. Keep unrestricted public admission disabled whenever evidence is incomplete or a critical alert is active.

**Access and first checks.** Use the generated inventory and pinned host key: `ssh -o UserKnownHostsFile=.local/ansible/public-beta-known-hosts mako-admin@130.245.173.11`. Direct root SSH and password authentication are disabled; recovery is through the Proxmox console. On the guest, inspect `mako-release-operation inspect`, the four Mako system services, the rootless `mako-dependencies.target`, the two checkpoint timers, and `/var/lib/mako-health/health.json`. Prometheus and Grafana are loopback-only; use an SSH tunnel.

**Developer console sign-in.** The preview console uses the same-origin management API and the control plane's short-lived signed developer sessions. From the protected workspace, issue a session into a mode-0600 file with `target/release/mako-control-session --secret-file .local/public-beta-secrets/internal-auth --output .local/console-sessions/developer.jwt --issuer https://cloud-test.makodb.com/control-identity --identity-id <dev_…> --email <address> --display-name "<name>" --ttl-seconds 3600`, open the console, and paste the file contents into the short-lived developer session field. The browser keeps the bearer in tab-scoped session storage. Delete the token file after transferring it. Initial hosted fixture creation must use a real persistent developer that completed email verification and operator approval, recorded (non-secret identity binding only) in `.local/qualification/public-beta-developer.json`; the qualification token helpers refuse missing, malformed, stale-epoch, wait-listed, rejected, or disabled identities. Exact-release streaming and benchmark requalification may then reuse the protected fixture in `.local/qualification/public-beta-fixture.json` with `--reuse-fixture true`.

**Deploy or converge.** [Deploying a release to the VM](#deploying-a-release-to-the-vm). Stage operator-password releases in restricted `pre_gate`: keep password auth disabled, retain only explicit reason-bound break-glass rollback access, run the protected loopback bootstrap plan/apply, then enable password auth and complete one browser sign-in. Require a new release/plan/blocker-bound public-preview approval before reopening admission.

**Risk-accepted public preview.** After producing exact-release qualification evidence, create and review the approval contract:

```sh
npm run public-beta:preview-approval -- plan --operator OPERATOR_IDENTITY --output .local/qualification/public-preview-plan.json
npm run public-beta:preview-approval -- approve --plan .local/qualification/public-preview-plan.json --confirm 'EXACT_CONFIRMATION_FROM_PLAN' --output .local/qualification/public-preview-approval.json
```

Converge with `mako_public_admission_mode: risk_accepted_preview`, the exact approval path, blocker digest, and all eight non-waivable safeguards asserted. Convergence starts Caddy in `pre_gate`; the guard checks the selected release, readiness, backup freshness, trusted TLS/HSTS, persistent acceptance, and immutable bindings before it atomically selects the preview configuration, and its one-minute timer falls back to `pre_gate` on drift. Inspect `/var/lib/mako-public-preview/last-guard.json`. To pause manually without stopping private services: `sudo /usr/local/sbin/mako-public-preview-admission pause` — this invalidates the installed acceptance, so create a fresh one to resume. Only the full release gate may select `approved_beta`; the Proxmox emergency stop is authoritative in every mode.

**Certificate or renewal failure.** Disable Caddy and apply the Proxmox admission stop first. Do not expose a plaintext fallback. Validate DNS A/AAAA state, ACME reachability, the configured contact, staging issuance, production chain, hostname, expiry, and renewal before re-enabling restricted ingress. HSTS remains off until a trusted production certificate and external route checks pass.

**Data disk full (data plane failed closed).** Symptom: `mako-data-plane` in a restart loop logging `data-plane storage could not be opened`, the other planes 503, the admission guard in `pre_gate` with `service readiness failed on port 8080`, `df /srv/mako-data` at 100 %. Find the consumer per subvolume (`du -sh /srv/mako-data/*/*`, not `du -x`); on 2026-09-19 it was 11 492 leaked checkpoint copies in `backup-staging/data-plane`. Pause the component's checkpoint timer, remove only the redundant `*.incomplete` staging copies (the published artifacts and the off-VM copies are the backups; the 8 GiB `.reserve/emergency-space` file is the last resort and stays), `systemctl reset-failed` and start the data plane, confirm all four `/readyz` answer 200, resume the timer and confirm the next checkpoint is remotely verified. The guard will have **invalidated the preview approval**; issue a fresh one and converge as under *Risk-accepted public preview* — a valid readiness alone does not reopen admission.

**Capacity and saturation.** Review the public-beta operations, production RocksDB, saturation, latency, and error dashboards. Preserve the warning and critical filesystem reserves from `group_vars/public_beta.yml`. If pressure persists, stop admission, retain the measurement window, and either reduce qualification load or make a reviewed VM sizing change. Sizing a single VM does not create HA.

**Backup and restore.** Timers create signed checkpoints every five minutes and reverify them on the Proxmox-host destination; a backup is successful only when `mako_storage_backup_remote_verified` is 1 and age is below 900 seconds. Restore only to a new empty offline path with service-owned credentials; verify signature, digests, format, tenant inventory, and acknowledged high water; promotion requires explicit confirmation. Never overwrite the only live path or substitute a VM snapshot for a Mako checkpoint.

**Service or dependency failure.** Keep ingress off. Inspect the unit journal and request correlations. Restart only the failed system or rootless-user unit. Do not clear a RocksDB lock by deleting files. If readiness does not recover within the objective, use release rollback or restore to an empty offline target.

**Release rollback.** [Release operations on the VM](#release-operations-on-the-vm): `sudo mako-release-operation rollback DIGEST --confirm=ROLLBACK_RELEASE:DIGEST`. Failure leaves ingress and backup timers stopped for inspection. Verify the selected release's expected auth method, entitlement epoch, password recovery invalidation, and bounded break-glass rollback before admission resumes.

**Suspected compromise.** Apply the emergency admission stop, preserve logs and immutable evidence, stop affected services, and revoke or rotate application credentials, internal authentication, object-store credentials (loaded by both the control plane and the data plane), backup keys, runtime-state keys, and certificates according to scope. Signing-key recovery is a safe roll-forward; never reactivate suspected or retired private material. Do not destroy the VM or backups until evidence retention and disclosure decisions are recorded.

**Emergency public-admission stop.** Run `scripts/proxmox/public-beta-admission-stop.sh` from the workspace. It disables Caddy and enforces the VM 124 TCP 80/443 bridge drop while retaining SSH, the VM, both disks, releases, evidence, and backups. Verify the host service and nftables table are active and that external 80/443 no longer connect.

**Confirmed teardown.** Teardown is separate from convergence and begins with the admission stop. Run the teardown planner only (`npm run plan:public-beta-teardown`); review its inventory of VM configuration, disks, off-VM backups, credentials, certificate state, DNS consequences, and retained evidence. Capture a final verified backup and evidence bundle. Deletion, credential revocation, certificate handling, DNS changes, backup disposal, and VM/disk removal each require the planner's typed confirmation. Never execute teardown from an ordinary deploy, repair, or convergence.

### Runbook: public beta operations alerts

These alerts describe the private beta VM. Keep public admission disabled while a critical alert is active; reach Prometheus and Grafana only through the SSH management path.

- **Service or Podman dependency alert:** record the release digest and request correlation, inspect the relevant system or rootless-user journal, restart only the failed unit, and require all four `/readyz` endpoints before considering recovery complete. Repeated restarts require rollback or an offline restore; never bypass readiness.
- **Operator authentication alerts:** keep admission in `pre_gate`, use only aggregate counters, follow the [operator runbook](#runbook-operator-password-authentication). Any lingering break-glass enablement or failed protected bootstrap requires review before admission.
- **Filesystem, RocksDB, backup, or recovery alerts:** stop admission and writes, preserve the sole live paths, verify the newest signed off-VM checkpoint, follow the [RocksDB runbook](#runbook-production-rocksdb-incidents). Never create an empty live path or use an in-memory fallback.
- **Certificate alerts:** leave Caddy admission off until staging and production issuance, hostname validation, expiry telemetry, and renewal have all passed.
- **Latency, HTTP errors, audit failures, host saturation:** retain the alert window, service logs, and resource graphs; audit failure is fail-closed and requires operator review before admission resumes.
- **In `risk_accepted_preview`:** the one-minute guard records sanitized state in `/var/lib/mako-public-preview/last-guard.json`. Expiry, release or plan drift, blocker drift, failed readiness, stale backups, or TLS/HSTS failure atomically selects `pre_gate`, and the same invalidated approval cannot reactivate the preview. Fix the condition and issue a new exact-binding approval. Do not manually replace the Caddy symlink or disable the timer.

---

## Release engineering

### Release gates

The machine-readable source is [`release-gates.json`](release-gates.json), validated by `npm run validate:release-gates`. A component or storage qualification does not by itself authorize customer traffic: each stage is a separate decision for an exact image, configuration, region, storage class, runtime, workload, and cost measurement window.

| Stage | Decision | Reason |
| --- | --- | --- |
| Internal | Pass for the recorded qualification host | Durability, recovery, security, capacity, component latency, and zero incremental cash spend on the existing host meet the internal thresholds |
| Single-region beta | **Blocked** | The exact VM storage and recovery/rollback drills pass, but trusted HTTPS, successful end-to-end load/saturation, and the 30-day capacity and provider/facility cost window are incomplete |
| Multi-region | Unsupported | One local database has one owner and no cross-region durability or automatic failover |

**Internal gate.** Zero acknowledged-write loss and integrity failure, at least 25 complete storage-soak cycles, all five security qualification families, a verified backup no older than 15 minutes, backup recovery-point exposure no greater than 15 minutes, same-volume recovery within 5 minutes, replacement restore within 30 minutes, and free capacity above the 2 GiB warning reserve. The retained observations pass: 25 cycles, 2 375 storage and document executions, no loss or integrity failures, 50-second backup age, zero checkpoint high-water exposure, 1.43-second same-volume recovery, 1.72-second replacement restore, and 1.706 TB free; all 11 component performance paths pass their checked budgets; the real pinned edge-runtime image and the auth, policy, RxDB, and tenant-boundary suites pass. The cost observation is deliberately narrow — already-owned host capacity, zero incremental cash spend — and cannot satisfy a beta gate.

**Single-region beta gate.** The internal zero-loss and security requirements plus: the same 15-minute backup/RPO and 5/30-minute restart/restore objectives on the actual encrypted retained RWO volume; at least 30 % capacity headroom at the qualified traffic target; end-to-end p95 budgets of 200 ms for auth, 250 ms for RxDB pull/push/live and warm functions, and 1.5 s for a cold function; fixed infrastructure cost ≤ USD 2 500/month and replication cost ≤ USD 50 per million operations at the beta workload; a 30-day measurement window using metering plus provider invoice data, with backups, logs, egress, retained volumes, and idle capacity included; and an operator-observed backup, restore, promotion, service/configuration rollback, and alert-response drill. Null observations fail the gate; local component measurements may guide sizing but cannot substitute for hosted HTTPS, storage-class, saturation, and invoice measurements.

**Multi-region gate.** Unsupported regardless of stateless edge routing. Before it can be evaluated, a separate OpenSpec change must provide and qualify a replicated durability design. The eventual gate requires zero acknowledged loss and integrity failures, region-loss RPO within 15 minutes, recovery within 30 minutes, cross-region p95 within 500 ms, fixed infrastructure ≤ USD 7 500/month, and replication cost ≤ USD 100 per million operations — ceilings for a future design, not claims about current capability.

**Procedure.** Build immutable artifacts and run CI. Run the auth, policy, RxDB chaos, tenant-boundary, real-container edge, and production RocksDB qualification suites. Retain machine-readable performance, storage, capacity, recovery, and cost observations for the exact candidate environment. Run `npm run validate:release-gates`; do not manually change a stage to pass while any required observation is null or any blocker remains. Record the approver, candidate digest, measurement window, and exception-free result in the deployment system before enabling traffic.

For the current VM, the plan hash, release/runtime/dependency/artifact digests, VM identity, storage layout, recovery drills, observability state, admission stop, and teardown inventory are retained in `docs/evidence/public-beta-*.json`. The gate validator binds observations to VM `124`, address `130.245.173.11`, origin `https://cloud-test.makodb.com`, and the exact selected release. The measurement window begins only when its machine-readable start record names the exact candidate and plan and must cover 30 complete days; facts that cannot be proven remain `null`, which keeps the qualified-beta gate blocked. `risk_accepted_preview` never fills those facts, changes a threshold, or marks that gate passed.

### Performance baseline

The machine-readable source is [`performance-baseline.json`](performance-baseline.json); `npm run validate:performance` checks its shape and coverage and `npm run validate:production-release` enforces the production thresholds against it.

| Path | Timed operation | p50 | p95 | Throughput |
| --- | --- | ---: | ---: | ---: |
| Write | Validate, synchronously commit one document, publish high water | 3.249 ms | 3.778 ms | 304 writes/s |
| Pull | Initial pull returning 100 visible documents | 7.158 ms | 9.750 ms | 140 pulls/s |
| Hidden-change scan | Scan 100 denied changes and advance the opaque checkpoint | 6.782 ms | 7.799 ms | 160 scans/s |
| Push conflict | Allocate/finalize a stale push and return the readable master | 3.823 ms | 4.861 ms | 249 conflicts/s |
| Live fan-out | Deliver one committed change to 25 sessions | 8.036 ms | 13.296 ms | 2 936 deliveries/s |
| Auth | Verify Ed25519 JWT, tenant, active session, and authorization epochs | 0.090 ms | 0.098 ms | 10 891 verifications/s |
| Policy | Evaluate three compiled read rules | 0.0010 ms | 0.0014 ms | 888 636 evaluations/s |
| Index build | Create, backfill 100 documents, catch up, fence, and activate | 38.660 ms | 49.453 ms | 25 builds/s |
| Control plane | Authorize an environment read and verify stored resource scope | 0.0052 ms | 0.0155 ms | 161 592 authorizations/s |
| Edge cold | Load a deployment, start an in-process worker, and invoke once | 0.0045 ms | 0.0073 ms | 173 213 paths/s |
| Edge warm | Invoke an already loaded in-process worker | 0.0014 ms | 0.0015 ms | 710 106 invocations/s |

Each row contains 25 release-profile samples on local RocksDB with `Sync` durability; pull, hidden scan, and index build use 100 seeded documents; live throughput counts each of 25 subscriber deliveries as one operation. The measurements are single-process component baselines: they exclude HTTP serialization, network latency, multi-node coordination, and distributed-store behavior. The edge rows measure the supervisor lifecycle through its public worker boundary with an in-process worker factory and deliberately do not claim OCI image startup or Deno user-code latency; hosted cold-start thresholds must come from the production runtime and deployment. The run used Linux 7.0.14-5-pve on x86_64, Rust/Cargo 1.97.1, Node.js 24.15.0, local RocksDB volumes on persistent Btrfs. These are an observed baseline, not release thresholds.

```bash
npm run benchmark
MAKO_BENCH_OUTPUT=/tmp/mako-performance.json MAKO_BENCH_ITERATIONS=50 \
MAKO_BENCH_DATASET_DOCUMENTS=250 MAKO_BENCH_LIVE_SUBSCRIBERS=50 npm run benchmark
```

The default output is `.playwright-tmp/performance-benchmark.json`. The runner always uses Cargo's release profile, a project-local Rust temporary directory, disabled incremental compilation, and the locked dependency graph.

### Production RocksDB qualification

Status: **PASS — eligible for the tested single-node topology**, recorded 2026-08-07; retained in [`production-rocksdb-qualification.json`](production-rocksdb-qualification.json).

Qualified topology: Linux 7.0.14-5-pve on x86_64, Rust/Cargo 1.97.1, Node.js 24.15.0; one RocksDB owner on local persistent Btrfs (`/dev/sda3`) with test volumes beneath `/var/tmp/mako-cloud-qualification`; `OptimisticTransactionDB`, synchronous writes, fsync, WAL verification, paranoid integrity checks; 1.706 TB free, release warning and critical reserves 2 GiB and 1 GiB. This result qualifies the tested filesystem and host class; repeat with `MAKO_STORAGE_TMPDIR` on the actual RWO production volume before a different storage class is rolled out.

Results: 25 consecutive local-storage cycles passed, each with 52 storage and 43 document tests (2 375 executions, zero failures, zero acknowledged-write losses, zero integrity failures); adapter conformance, conditional races, synchronous process-crash restart, startup readiness, I/O and capacity faults, lock contention, compaction/retention, and acknowledged-high-water recovery passed; authenticated checkpoint backup, read-back verification, corruption and stale-artifact rejection, tenant inventory, empty-target restore, explicit promotion/fencing, and interrupted restore passed; same-volume recovery 1.43 s and replacement restore plus verification 1.72 s including test-process startup; the backup tests verified a 50-second-old artifact under a 100-second policy and restored every acknowledged position present at checkpoint time; previous-format-compatible binary rollback read and wrote the same non-empty volume, and a missing path with create-if-missing disabled stayed empty; all 11 performance paths passed their budgets.

Thresholds: no acknowledged-write loss or integrity failure; a verified backup at most 15 minutes old and 15 minutes of recovery-point exposure; same-volume recovery within 5 minutes; verified replacement restore and promotion within 30 minutes; free capacity above the 2 GiB warning reserve, failing closed at the 1 GiB critical reserve; storage/RxDB p95 budgets of 50 ms for writes and 100 ms for pull, hidden scans, push conflicts, and live fan-out; 500 ms for index build; the remaining per-component budgets are in the JSON.

```bash
MAKO_STORAGE_TMPDIR=/path/on/the/volume/under/test bash scripts/run-production-rocksdb-qualification.sh
```

### Rollback qualification

Six rollback paths were exercised locally on 2026-08-07; the machine-readable result is [`rollback-qualification.json`](rollback-qualification.json), `npm run test:rollback` reproduces it, and `npm run validate:rollback` checks that the report names six passing drills whose procedures are below and whose evidence exists. This proves the checked state transitions and storage invariants; it does not qualify a cloud cluster, storage class, image registry, or operator team. The single-region beta gate therefore remains blocked until the same release is exercised in its deployment environment under operator observation — which the hosted drill on VM 124 (`docs/evidence/public-beta-release-rollback.json`, `public-beta-rollback-matrix.json`) has since satisfied for its own blocker.

Before every rollback, stop new admission, preserve request and audit context, identify the last-known-good immutable version, and change one dimension at a time. Stateful rollback additionally requires a verified checkpoint, exclusive volume fencing, known format compatibility, and readiness before traffic.

#### Service

Roll back every container for a service to the same previous immutable image. For a stateful service, stop writes and fence the current owner first. Preserve the one-replica topology, retained claim, service identity, database marker, database path, and backup destination. Wait for semantic recovery and readiness; never use an empty volume as a shortcut. If desired-state provisioning fails, compensate completed components, retain the safe diagnostic, then retry the same workflow idempotently. The qualification applies a previous image to the checked StatefulSets in memory, asserts that only the four image references changed, and tests provisioning compensation and resume.

#### Policy

Select a previously validated immutable policy version through the authorized rollback operation. Activation is atomic and advances the authorization epoch, forcing stale authorization state to be discarded. Do not edit policy history in place or activate an invalid draft. Verify a representative deny and allow case after rollback and inspect the audit event.

#### Function

Point the function atomically at a previous immutable version that is still healthy. A failed deployment must not replace the active version, and a version that later fails health checks must become ineligible for promotion. Test-invoke the selected version, check its secret-version bindings, and inspect sanitized logs before restoring traffic.

#### Schema

Schema recovery is compatibility-first, not a destructive document downgrade. An incompatible publication remains inactive and returns a migration-required outcome, leaving the last compatible schema active. Roll application traffic back to that schema or execute a reviewed forward migration. Never discard fields or rewrite stored documents merely to reduce a schema version.

#### Signing key

Treat signing-key rollback as a safe roll-forward. Generate and activate fresh encrypted private material, publish both the new and prior public keys during the maximum token lifetime, and retire the superseded public key only after the overlap expires. Never reactivate a retired or suspected-compromised private key. Confirm both JWKS overlap and eventual retirement while keeping private material out of logs and serialized records.

#### Storage adapter

The production adapter is local RocksDB. Stop writes, fence its sole owner, take and verify a checkpoint, and confirm the previous binary supports the recorded format. Open that previous binary on the same non-empty retained volume, require full readiness, and then resume traffic. If the volume is unusable, restore a verified compatible backup to an empty target and explicitly promote it. Never fall back to memory storage, a different adapter, or a newly created path.

#### Reproduce and record

```bash
npm run test:rollback
```

For a candidate beta deployment, repeat these procedures using the exact image, runtime pin, storage class, configuration, and secrets policy. Record operator, timestamps, recovery time, failed checks, and the post-rollback audit/readiness evidence in the release record before removing the beta blocker. On the VM the tested transition uses `mako-release-operation` ([release operations](#release-operations-on-the-vm)); per-operation records remain under `/var/lib/mako-release-operations/`.

### Qualification reports

Each report describes the host and scope it exercised. A passing local report does not qualify a different storage class, container runtime, region, or cost model.

#### Authentication security qualification

`npm run test:auth-security` — run before a release and after changing password policy, tokens, signing keys, sessions, credentials, gateway verification, or client refresh behavior. Covers configurable Argon2id hashing and automatic parameter upgrades; enumeration-safe sign-up, sign-in, verification, and recovery; required JWT claims and signature, issuer, audience, tenant, expiry, session, and authorization-epoch verification; signing-key encryption, JWKS overlap rotation, and retirement; hashed single-use refresh rotation, bounded concurrency grace, family replay detection, and revocation; ordered user/session disable, sign-out, deletion, and gateway revocation-cache invalidation; one-time public/service credential display, scoping, overlap rotation, and cross-project rejection; automatic RxDB-client token refresh and transition to authentication-required when refresh is revoked. **Latest:** the 2026-08-07 local run passed the identity, gateway, and control-plane Rust suites and all 14 RxDB client tests.

#### Policy security qualification

`npm run test:policy-security` — run before a release or after changing a policy input, enforcement path, authorization epoch, conflict response, or privileged credential. Penetration objectives: differential decisions (point reads, trusted queries, pulls, live streams, conflict responses, the edge SDK path, and operator impersonation pass through the same compiled decision and stable code); visibility transitions (visible-to-hidden emits only a synthetic tombstone; hidden-to-hidden advances without exposing protected state); authorization epochs; conflict non-disclosure (an unreadable stale master returns the same permission error as a denied write, with no master state, document id, stale revision, or protected body on the wire); privileged bypass (exact tenant, collection, operation, expiry, reason, and durable audit; audit failure prevents bypass); edge callers (the default client propagates only verified caller context; service access uses a separate explicit API and mandatory bypass audit headers). **Latest:** the 2026-08-07 local run passed the Rust policy, sync, and gateway suites and all edge SDK unit tests; the command exits on the first failed objective.

#### RxDB chaos qualification

`npm run test:rxdb-chaos` — run before a release and after changing mutation idempotency, checkpointing, live delivery, policy visibility, schemas, retention, or client recovery. Models concurrent offline clients writing from one assumed master and resolving a readable conflict; a response dropped after commit followed by duplicate retries whose five rows arrive in 128 generated orderings; hidden changes, visible-to-hidden tombstones, remote deletion, and policy-driven epoch reset; live-stream reconnects, explicit gaps, bounded-buffer overflow, service failover, and resumable cursors; incompatible schema versions and client migration hooks; checkpoint expiry after compaction and full-resync hooks; RocksDB restart with a previously issued checkpoint; refresh and revoked-refresh transition to authentication-required. Each invariant asserts stable outcomes and that duplicate or reordered retries create no additional revisions or change records. **Latest:** the 2026-08-07 local run passed the sync and document Rust suites and all 14 RxDB client tests.

#### Tenant-boundary qualification

`npm run test:tenant-boundaries` runs the production boundary code with deterministic randomized inputs; failures retain Proptest's minimal reproducer and seed.

| Boundary | Property or invariant | Randomized cases per run |
| --- | --- | ---: |
| Key encoding | Arbitrary bytes round-trip without collisions; tenant and collection prefixes cannot escape or overlap | 1 792 |
| Storage | Memory and RocksDB adapters preserve tenant scan isolation through the shared conformance suite | deterministic |
| Gateways | Identical reservation ids and limits remain independent for arbitrary distinct project/environment pairs | 256 |
| Management APIs | Function records and range scans for arbitrary distinct pairs remain disjoint | 512 |
| Structured logs | A log event cannot be appended through a different arbitrary trusted tenant | 256 |
| Object storage | Typed bundle paths retain their owning tenant, reject cross-tenant reads, fuzz invalid digests, preserve immutable writes | 1 024 |
| Edge workers | Identically named and versioned deployments resolve to distinct workers for arbitrary tenant pairs | 128 |

At least 3 968 generated cases per run, plus deterministic unit, conformance, fault-injection, and restart tests. **Latest:** on 2026-08-07 all selected suites passed locally — `mako-storage` 52 tests, `mako-gateway` 10, `mako-control-plane` 29, `mako-audit` 21, `mako-object-store` 3, `mako-edge-runtime` 6. The object-store result qualifies the typed boundary and in-memory reference; a hosted S3-compatible provider belongs in environment-specific release testing.

#### Edge security qualification

`npm run test:edge-security` deliberately refuses to pass unless `MAKO_RUN_EDGE_RUNTIME_TESTS=1`, and uses the pinned image by digest, never a tag. Covers exact tenant worker identities, concurrency admission, crash replacement, and isolation of healthy tenants; undeclared environment/secret access, cross-project secret canaries, log redaction, and explicit secret versions; CPU, wall-time, memory, request, response, egress, and post-invocation background-work containment; deny-by-default outbound access and no unauthorized regional fallback; real-runtime TypeScript, JavaScript, Fetch, npm, WebAssembly, streaming, and outbound-fetch compatibility; an immutable runtime source commit and OCI digest plus a high-severity npm dependency audit. For rootless Podman on a single-UID host, use graph storage on a local filesystem and pass Podman's `ignore_chown_errors=true` overlay option through `MAKO_EDGE_TEST_ENGINE_PREFIX_JSON`. **Latest:** on 2026-08-07 the pinned `v1.74.3` image at digest `sha256:c52405002a890ca9fcf77978671c57f3a988e03174afb277f84ac65bc917013c` passed both real-container integration tests under rootless Podman; the Rust protocol/supervisor/gateway suites passed and `npm audit --audit-level=high` reported zero vulnerabilities.

#### Storage soak qualification

`npm run test:storage-soak` — run after changing adapter transactions, acknowledgement behavior, sequencing, indexes, retention, or recovery. `MAKO_STORAGE_SOAK_ITERATIONS` overrides the default 25 cycles; `MAKO_STORAGE_TMPDIR` names a directory on the storage class being qualified. Each cycle includes every injected pre-commit I/O failure and the ambiguous post-commit crash point; memory and RocksDB conformance including conditional races and stable snapshots; sync-acknowledged close/reopen durability; concurrent revision and acknowledgement invariants across restart; unresolved sequencer gaps, abort/commit advancement, and restart recovery; snapshot index backfill, concurrent change-log catch-up, uniqueness failure, and atomic activation; retention barriers, expired checkpoints, historical revisions, idempotency records, and safe tombstone compaction. **Latest:** the 2026-08-06 portability soak completed 25 consecutive cycles with no failures on the workspace's persistent NFSv4.1 filesystem (52 storage and 43 document tests per cycle, 2 375 executions) — retained as extra evidence, not the production topology result, because production must not use a shared directory. The release-facing local Btrfs result is the [production RocksDB qualification](#production-rocksdb-qualification).

#### End-to-end smoke and edge end-to-end

`npm run test:e2e-smoke` and `MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-e2e` run in CI on every change and write `docs/evidence/e2e-smoke-qualification.json` and `edge-e2e-qualification.json` recording the host and scope. The smoke covers bootstrap, sign-up, sign-in, push, pull, and the negative control; it does not cover hosted registration and review, management administration, the object store, telemetry, or the console.

---

## The sample applications as platform gates

Rational (`examples/rational`) is the platform's standing proof that its capabilities hold up together. Every gap it hits is recorded in [`PLATFORM-FINDINGS.md`](../examples/rational/PLATFORM-FINDINGS.md) as symptom → platform change → regression test, and fixed **in the platform**, never worked around in the application. Forty-nine findings so far, all closed. Each row is one defect or gap, in the order found: **Found by** (the task or moment that exposed it), **Symptom** (what the developer saw), **Platform change** (what was changed and why that rather than something else; a rejected alternative is named), **Regression test** (the test that fails against the old behaviour — a row without one is not closed), and **Status**. A few rows are not platform defects: #3 is a design adjustment (RxDB's open-source build opens at most 13 collections per page), #15 a documentation gap, and #21, #27, and #29 defects in the platform's own tests and guards.

Rational is also a release gate. `crates/mako-smoke/tests/rational.rs` (`npm run test:rational-smoke`) publishes the project from `examples/rational/mako/` — the same files the bootstrap publishes — and walks a household's life over HTTP: three ways in, sharing by claim, an import, a rule, a receipt, an alert that leaves by signed webhook, and writes queued while a device was away; the hosted qualification runs it against the deployed source on every release. `npm run validate:rational-parity` refuses a Monarch parity matrix under ninety percent. `scripts/export-rational-app.mjs` publishes the application to its own repository, vendoring the design kit under `src/kit`; `npm run test:rational-export` tests the export.

Two constraints shape the sample and are worth knowing before changing the platform under it: the open-source RxDB build opens at most thirteen collections per page (Rational holds twelve, so a new document type joins an existing collection behind a `kind` discriminator), and the sample's schema is at version 3, published additively over the beta's version 2 — a platform change that would force a breaking schema publication is a platform finding, not a sample edit. The beta's Rational project ids and the bootstrap procedure (`--env-name production`, then export the site, then run-now the schedules) live with the beta operator notes, not in this repository.

The local-first example (`examples/local-first`) is the reference RxDB integration and the browser half of the replication protocol's proof; `npm run test:browser-live` is what CI runs against the real binaries.

---

## The design system

Every Mako web surface — the developer console and the Rational sample — is dressed by one kit, `@mako-cloud/ui` in `packages/ui`: tokens, components, icons, typeface, and charts, so a button in the console and a button in Rational are the same button, and a dark theme is the same dark theme.

- **Tokens.** Colour, radius, and type are named CSS variables declared once in `packages/ui/src/styles.css` with `light-dark()`, so each has a light and a dark value and the palette follows the document's colour scheme. Tailwind v4 reads them through `@theme inline`, so `bg-background`, `text-muted-foreground`, and `border-border` are the same colours the components use. The names are exported from `@mako-cloud/ui/tokens`.
- **Components.** shadcn/ui-style components built on Radix primitives: button, input, textarea, label and field, badge, card, separator, skeleton, dialog, sheet, dropdown menu, popover, tooltip, tabs, select (native and Radix), checkbox, switch, progress, table, alert, empty state, avatar, toast. Radix gives keyboard operation, focus management, and ARIA; every control has a visible focus ring in both themes.
- **Icons.** Lucide, imported by the application from `lucide-react`; one weight everywhere.
- **Type.** Inter, self-hosted through `@fontsource-variable/inter`, so no page depends on a font service and an offline-first app keeps its type offline. Money is set in tabular figures with the `money` utility.
- **Charts.** Wrappers over Recharts — `LineChart`, `AreaChart`, `BarChart`, `DonutChart`, `Sparkline` — using the palette's series colours, formatted axes, the kit's tooltip, an accessible name (`role="img"`), and no animation under reduced motion or when a browser suite marks the root with `data-testing`.

**Adopting it:** depend on `@mako-cloud/ui`, add `@tailwindcss/vite` to `vite.config.ts`, import `@mako-cloud/ui/styles.css` once in the entry module (it brings Tailwind, the typeface, the tokens, the base styles, and — through `@source "./"` — the kit's own component classes), write screen-specific layout with Tailwind utilities, and keep the theme with `useTheme(storageKey)` and `ThemeToggle`. Adoption is checked: `npm run validate:ui-kit` refuses a raw `<button>`, `<input>`, `<select>`, `<textarea>`, or `<dialog>` in `examples/rational/src` or `apps/console/src` (a file picker and a hidden input are the deliberate exceptions; a file still being moved can be allow-listed by path with its reason).

**The standalone export** copies `packages/ui/src` to `src/kit` in the Rational checkout, points `@mako-cloud/ui` at it with a `paths` entry and a `resolve.alias`, and writes the kit's dependencies into the generated `package.json` with the exact versions the workspace lock resolved; the stylesheet's `@source "./"` is relative to itself, so it compiles from `src/kit` there and `packages/ui/src` here with no change.

**Serving it:** the typeface is a file the application ships. On the beta the console is served by Caddy from the release directory, whose asset route names the kinds of file it serves; it now serves `css`, `js`, and `woff2`, and `npm run validate:public-beta-caddy` asserts all three are there. A new asset kind must be added to both the route and that assertion.

**Tests:** `packages/ui/test/tokens.test.mjs` converts every `light-dark()` pair from oklch to sRGB and checks each promised text-on-surface pair (`READABLE_PAIRS`) against the AA contrast ratio in both themes, plus that every token is mapped into the Tailwind theme; `components.test.mjs` checks the public surface; `scripts/test/validate-ui-kit.test.js` drives the validator; the applications' browser suites are the integration gate. Bundle sizes (gzipped, production): Rational ships 444 KB of script in six cacheable chunks (`index` 128, `charts` 95, `database` 73, `react` 59, `vendor` 57, `primitives` 30) and 11 KB of CSS; the console 223 KB in four chunks (`index` 128, `react` 59, `primitives` 19, `vendor` 18) and 12 KB of CSS; the Inter variable font adds 48 KB for Latin. Playwright and Vite have traps worth knowing: mark the root `data-testing` to disable chart animation, and vendor chunks are split by library so a cache survives an application-only change.

---

## Appendix: binaries and tools

| Binary or script | Where | Purpose |
| --- | --- | --- |
| `mako-data-plane`, `mako-control-plane`, `mako-edge-gateway`, `mako-telemetry-query` | `services/*` | The four deployable services |
| `mako-local-bootstrap` | `crates/mako-local-bootstrap` | Seeds a complete local tenant; refuses outside a local environment |
| `mako-storage-ops` | `crates/mako-storage/src/bin` | `provision`, `backup`, `inspect`/`verify`, `restore`, `fence`, `promote` for RocksDB volumes |
| `mako-control-storage-ops` | `crates/mako-storage/src/bin` | `inspect-sqlite`, `migrate`, `backup`, `restore`, `promote` for the control SQLite authority |
| `mako-storage-crash-helper`, `mako-sqlite-crash-helper` | `crates/mako-storage/src/bin` | Test helpers terminated abruptly after an acknowledged write |
| `mako-control-session` | `services/mako-control-plane/src/bin` | Issues a short-lived signed developer session (console sign-in on the beta; smoke and qualification helpers) |
| `mako-operator-admin` | `services/mako-control-plane/src/bin` | Protected operator entitlement plan/apply over loopback |
| `mako-operator-session` | `services/mako-control-plane/src/bin` | Incident-only break-glass operator token issuer |
| `mako-operator-projection` | `services/mako-control-plane/src/bin` | Offline control-center projection backfill and shadow validation |
| `mako-qualification-fixture` | `crates/mako-qualification-fixture` | Creates and validates the protected hosted qualification fixture |
| `mako-benchmarks` | `crates/mako-benchmarks` | The performance harness (`npm run benchmark`) |
| `mako-release` | `infra/ansible/roles/runtime/files/`, installed to `/usr/local/sbin` | `verify`, `install`, `select`, `mark-good`, `inspect`, `prune [--keep N]` for the immutable release root |
| `mako-release-operation` | `infra/ansible/roles/runtime/files/`, installed to `/usr/local/sbin` | `inspect`, `snapshot-current`, `prune`, `upgrade`, `rollback` on the VM |
| `mako-public-preview-admission` | Installed by the runtime role | The admission guard (`pause`, `activate`) |
| `scripts/local/prepare.sh`, `generate-certs.sh` | `scripts/local` | Local directories, CA, certificate, and secrets |
| `scripts/build-public-beta-release.js` | `scripts` | Builds the immutable release directory and manifest |
| `scripts/proxmox/*` | `scripts/proxmox` | Plan, apply, teardown planner, cloud-init rendering, inventory, firewall, admission stop, backup target |
| `scripts/public-beta-preview-approval.js` | `scripts` | Digest-bound preview approval plan/approve |
| `scripts/cleanup-expired-operator-sessions.js` | `scripts` | Bounded cleanup of expired local operator tokens |
| `scripts/export-rational-app.mjs`, `publish-rxdb-client.mjs` | `scripts` | Publish Rational to its repository; publish the built `@mako-cloud/rxdb` package to its [public distribution repository](https://github.com/makodb/mako-rxdb) |
| `scripts/run-*-qualification.sh`, `run-performance-benchmarks.sh` | `scripts` | The qualification runners behind the `test:*` scripts |
| `scripts/validate-*.js`, `scan-public-beta-secrets.js` | `scripts` | The CI validators |

## Appendix: evidence files

Machine-readable evidence lives in `docs/evidence/`. CI-produced files (`e2e-smoke-qualification.json`, `edge-e2e-qualification.json`, `rational-smoke-qualification.json`) are uploaded as build artifacts; the rest are committed deliberately as release records.

| File | Records |
| --- | --- |
| `account-role-lifecycle-qualification.json`, `public-beta-role-lifecycle-qualification.json` | The identity/role split qualification, locally and hosted |
| `control-plane-sqlite-qualification.json`, `control-plane-sqlite-benchmark.json`, `public-beta-control-sqlite-cutover.json` | SQLite authority qualification, benchmark, and the beta cutover |
| `operator-control-center-projection-qualification.json`, `operator-control-center-legacy-retirement.json` | Projection shadow rebuild counts/checksums; retirement of the legacy operator page |
| `public-beta-preflight-plan.json`, `public-beta-provisioning-result.json`, `public-beta-guest-bootstrap-verification.json`, `public-beta-guest-convergence.json`, `public-beta-network-bootstrap-repair.json`, `public-beta-network-exposure.json`, `public-beta-egress-firewall.json`, `public-beta-firewall-convergence.json`, `public-beta-rootless-dependencies.json`, `public-beta-systemd-supervision.json` | The VM plan and its provisioning, bootstrap, convergence, and network posture |
| `public-beta-release-manifest.json`, `public-beta-release-installation.json`, `public-beta-candidate-deployment.json`, `public-beta-current-deployment.json`, `public-beta-candidate-qualification.json` | Release digests and what is installed |
| `public-beta-production-configuration.json`, `public-beta-production-storage.json` | Rendered configuration (redacted) and storage layout |
| `public-beta-backup-qualification.json`, `public-beta-restore-qualification.json`, `public-beta-release-rollback.json`, `public-beta-rollback-matrix.json` | The operator-observed backup, restore, and rollback drills |
| `public-beta-observability.json`, `public-beta-mail-qualification.json`, `public-beta-developer-registration-qualification.json`, `public-beta-initial-operator-lifecycle-repair.json`, `public-beta-operator-browser-qualification.json` | Observability state, mail, registration, and operator qualification |
| `public-beta-hosted-qualification.json`, `public-beta-hosted-benchmark.json`, `public-beta-pre-admission-benchmark.json`, `public-beta-streaming-qualification.json`, `public-beta-https-qualification.json`, `public-beta-final-validation.json` | Hosted functional, benchmark, streaming, and HTTPS qualification |
| `public-beta-admission-stop.json`, `public-beta-public-preview.json`, `public-beta-public-preview-approval.json`, `public-beta-public-preview-guard.json`, `public-beta-manual-self-approval.json`, `public-beta-measurement-window.json`, `public-beta-teardown-plan.json` | Admission stop, preview approval and guard, the 30-day window start, and the teardown inventory |

## Glossary

- **Acknowledged high water** — the highest commit position the sequencer has proven; readiness and restore verify it.
- **Admission mode** — `disabled`, `pre_gate`, `risk_accepted_preview`, or `approved_beta`: what Caddy serves publicly.
- **Authorization epoch** — a per-environment and per-user counter that advances on policy or claim changes; tokens, checkpoints, and explorer grants are bound to it.
- **Fence** — mark a storage volume so no owner may serve it until promoted.
- **Gap** — a commit position neither committed nor aborted; holds high water back until classified.
- **Internal RPC** — the signed, loopback-only protocol between services.
- **Plane** — control (management, SQLite), data (tenant state, RocksDB), edge (functions), telemetry.
- **Project reference** — `{projectId}--{environmentId}`, the prefix functions are invoked under.
- **Promotable** — the marker a restored volume carries until an operator promotes it.
- **Release digest** — the immutable name of a release directory on the VM.
- **Runtime pin** — the exact Edge Runtime source commit and OCI digest the supervisor may run.
- **Sequencer** — the component that allocates and publishes commit positions per environment.
- **Supervisor** — the runtime's main worker that authenticates deployments, injects capabilities, and starts user workers.
- **Tombstone** — a document delivered as `_deleted: true`; also the synthetic record sent when a document leaves a caller's view.
