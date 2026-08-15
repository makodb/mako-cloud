## Context

See `proposal.md` — Why. The constraints that shape this design:

- **Two exclusively-owned stores, no shared transaction.** The control plane owns server-side SQLite; the data plane owns RocksDB. `CollectionAdminService` is constructed on the control adapter (`services/mako-control-plane/src/graph.rs:379`) while `DocumentEngine` is constructed on the data adapter (`services/mako-data-plane/src/graph.rs:214`). Collection metadata written by one is unreadable by the other, and `active_collection_metadata` (`services/mako-data-plane/src/document_http.rs:396`) rejects the request when it is absent.
- **Services talk over `mako-internal-rpc` only** — authenticated, versioned, loopback-only, replay-guarded. The data plane already exposes `InternalRoute::IdentityAdmin`, through which the control plane drives tenant administration such as `CreateProjectCredential`.
- **One process per database directory.** Any tool writing directly to a store must hold that store's lock, so it cannot run while the owning service is up.
- **No async runtime, fail-closed startup, secrets are references.** These are workspace conventions the new code must not break.

## Goals / Non-Goals

**Goals:**
- A collection created through the management API becomes usable for document traffic in the same environment.
- A single command produces a working local tenant; a single command proves the happy path against the real binaries.
- The verification is gating in CI and honestly represented in the traceability matrix.

**Non-Goals:**
- Exercising the hosted registration chain (mail, TLS SMTP, wait-list, operator approval). The bootstrap deliberately bypasses it; verifying it stays a separate change.
- Verifying edge functions, the object store, telemetry, or the console UI. This change covers auth and document replication only.
- Any change to production provisioning. The bootstrap is a local-only tool, not a provisioning path.
- Multi-node or cross-region propagation. One control plane, one data plane.

## Decisions

### Propagate collections over the existing IdentityAdmin internal route

Add collection operations (create, and lifecycle update) to the `IdentityAdminCommand` family in `crates/mako-internal-rpc/src/contract.rs`, with a dedicated `ManageCollections` permission, handled by the data plane's existing internal route which writes metadata through its own `DocumentEngine`.

*Why:* that route already carries control-plane-to-data-plane tenant administration (`CreateProjectCredential` is not identity-specific either), and it comes with authentication, versioning, replay guarding, and the encrypted response journal already wired and tested.

*Alternative considered:* a new `InternalRoute::CollectionAdmin`. Cleaner naming, but it duplicates transport, guard, and authenticator wiring for no isolation benefit — both are the same trust relationship over the same channel. Rejected as cost without gain.

*Alternative considered:* having the data plane read collection metadata from control SQLite. Rejected outright — it violates the storage split, which the project states is deliberate and not configurable.

### Order the two-store write as pending → propagate → active

Collection creation records the collection in control SQLite in a pending state, propagates to the data plane, and only then marks it active. Propagation is idempotent, keyed by the collection identifier, so a retry after a crash converges. Failure leaves the collection pending with a retryable diagnostic.

*Why:* there is no transaction spanning the two stores, so some order must be chosen and some interleaving must be survivable. This one satisfies the spec requirement that a collection is never reported active while document operations would reject it, and it reuses the asynchronous-state model the control plane already uses for provisioning.

*Alternative considered:* propagate first, then write control SQLite. A crash in between leaves the data plane holding a collection no management surface knows about — invisible, unreclaimable, and harder to reconcile than a pending record.

### Bootstrap is a standalone binary that runs while services are stopped

A new `crates/mako-local-bootstrap` binary opens both stores directly, seeds developer, organization, project, environment, public project key, and collection, then exits. It refuses to run unless the resolved environment is local, and refuses if it cannot acquire either store's lock.

*Why:* seeding both stores is precisely what the running services will not permit concurrently, and requiring the services down makes the exclusivity rule enforce itself rather than being an honor system. Fixed identifiers make runs deterministic; check-then-create makes them idempotent.

*Alternative considered:* driving the management API over HTTP with a real developer session. That is the path the smoke test *should* eventually take, but it is blocked today by the mail/TLS-SMTP/wait-list chain the user chose not to take on here.

### Smoke test resolves binaries by path, allocates ports dynamically

The Rust integration test lives in a new `crates/mako-smoke` package, spawns both binaries with `std::process::Command`, and drives HTTP directly. Binary directory comes from `MAKO_SMOKE_BINARY_DIR`, defaulting to the workspace target directory; a missing binary is a failure with an actionable message, never a silent skip. Ports are allocated by binding ephemeral sockets and passing the result via `MAKO_BIND_ADDR`. Storage paths go under a per-run temporary directory honoring `MAKO_STORAGE_TMPDIR`.

*Why:* `CARGO_BIN_EXE_*` is only available to tests in the package defining the binary, and this test needs two binaries from two packages. Dynamic ports keep parallel and CI runs from colliding on fixed loopback ports. A gating test that skips when misconfigured would silently stop protecting the happy path — the exact failure mode this change exists to end.

### The script wrapper produces evidence, the Rust test owns the assertions

`scripts/run-e2e-smoke-qualification.sh` builds the binaries, runs the Rust test, and writes `docs/evidence/e2e-smoke-qualification.json` recording host, scope, and outcome. It adds no assertions of its own.

*Why:* it matches the existing `scripts/run-*-qualification.sh` pattern and the repository's rule that qualification reports state the host and scope they exercised, while keeping one source of truth for what "passing" means.

### Local secrets are generated, not shipped

`scripts/local/prepare.sh` generates the internal-auth and object-store secrets into `.local/secrets/` with restrictive permissions, and `.env.example` references them with `file:` references.

*Why:* the quickstart is broken today because the required references ship commented out. Filling in literal values would contradict the convention that secrets are references and never inline values; generating them keeps `prepare.sh` as the single documented preparation step.

## Risks / Trade-offs

- **Collection creation now depends on data-plane availability** → It already did in substance: a collection the data plane cannot serve is not a usable collection. The pending state makes the dependency explicit and retryable rather than silently producing a broken collection.
- **A crash between the pending write and propagation leaves a pending collection** → Propagation is idempotent and keyed by collection identifier, so a retry converges; the pending record is visible to management rather than orphaned.
- **Spawning two services makes the smoke test the most flake-prone test in the suite** → Dynamic ports, per-run temp directories, bounded readiness polling with a clear timeout diagnostic, and no dependence on Docker, mail, or the object store.
- **The bootstrap bypasses controls that exist for good reasons** → It is refused outside local environments, it never relaxes a control in code that a non-local environment uses, and it is documented as a development tool rather than a provisioning path.
- **The smoke test proves the tenant path, not the management path** → Deliberate, per the chosen scope. The collection propagation fix is nonetheless exercised through the real internal RPC, so the defect this change closes is genuinely covered.

## Migration Plan

No data migration is required. Collections created before this change exist only in control SQLite and were never usable by the data plane; the beta environment has no tenant collections, so there is nothing to backfill. Should any exist, re-driving creation for that collection propagates it, because propagation is idempotent.

Rollback is reverting the change: the propagation path is additive, and the bootstrap, smoke test, and CI job are new surfaces that no production path depends on.
