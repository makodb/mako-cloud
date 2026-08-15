## 1. Repair the documented local quickstart

- [x] 1.1 Extend `scripts/local/prepare.sh` to generate internal-auth and object-store secrets into `.local/secrets/` with `0600` permissions, idempotently (do not regenerate on re-run)
- [x] 1.2 Update `.env.example` so the data plane starts unedited: supply `MAKO_INTERNAL_AUTH_SECRET_REF` and the object-store references as `file:` references to the generated secrets
- [x] 1.3 Make the control plane start from its shipped defaults: create its directories in `prepare.sh`, and resolve relative control-storage paths against the working directory outside production in `mako-config` (SQLite storage rejects relative paths, so the shipped relative defaults could never open). No redundant environment block was added to `.env.example`; `MAKO_BIND_ADDR` was unset there so the two services do not collide on one port
- [x] 1.4 Add `config/mako.control.local.json.example` mirroring the existing data-plane example
- [x] 1.5 Update `docs/local-development.md` and `docs/configuration.md` so the documented steps start both services on a clean checkout
- [x] 1.6 Verify by hand: clean checkout → documented steps only → both `/readyz` return 200

## 2. Propagate collections to the data plane

- [x] 2.1 Add collection operations and a `ManageCollections` permission to `IdentityAdminCommand` / `IdentityAdminOperation` / `IdentityAdminPermission` in `crates/mako-internal-rpc/src/contract.rs`
- [x] 2.2 Handle the new operations in `services/mako-data-plane/src/internal_http.rs`, writing collection metadata through the data plane's `DocumentEngine`; make the write idempotent on collection identifier
- [x] 2.3 Add a pending lifecycle state to collection records in `crates/mako-control-plane/src/collection.rs` so a collection is not reported active before propagation succeeds
- [x] 2.4 Drive propagation from `services/mako-control-plane/src/collection_http.rs` in pending → propagate → active order, returning a retryable diagnostic when the data plane cannot record the collection
- [x] 2.5 Add inline tests: successful propagation marks active; data-plane failure leaves pending with a retryable error; repeated propagation is idempotent
- [x] 2.6 Document the propagation contract and retryable failure on `createCollection` in `api/openapi/mako-cloud-v1.yaml` (the `creating` state was already in the schema enum), then run `npm run generate:api`

## 3. Local tenant bootstrap

- [x] 3.1 Create `crates/mako-local-bootstrap` with a binary target, depending on `mako-config`, `mako-storage`, `mako-control-plane`, `mako-identity`, `mako-documents`
- [x] 3.2 Refuse to run unless the resolved environment is local, and refuse with an addressed error if either store lock cannot be acquired
- [x] 3.3 Seed the control store: developer identity, organization, project, environment — deterministic identifiers, check-then-create for idempotency
- [x] 3.4 Seed the data store: public project key, the project signing key sessions require, and an active collection with a compatible schema
- [x] 3.5 Print the created project, environment, collection, and public project key in a machine-readable form the smoke test can consume
- [x] 3.6 Add inline tests: deterministic identifiers across runs, idempotent re-run, refusal outside a local environment

## 4. End-to-end smoke test

- [x] 4.1 Create `crates/mako-smoke` with `tests/happy_path.rs`; resolve binaries from `MAKO_SMOKE_BINARY_DIR` defaulting to the workspace target directory, failing with an actionable message when absent
- [x] 4.2 Allocate ephemeral ports, create a per-run temp directory honoring `MAKO_STORAGE_TMPDIR`, run the bootstrap, then start both services and poll `/readyz` with a bounded timeout
- [x] 4.3 Drive the happy path over HTTP: application-user sign-up → sign-in → document push → document pull, asserting the pulled document matches what was pushed
- [x] 4.4 Assert the negative control: the same document operations without the issued session credentials are refused
- [x] 4.5 Tear down cleanly — terminate both services and remove the temp directory even when assertions fail
- [x] 4.6 Confirmed the test fails loudly: with the binary directory missing it fails with an actionable message rather than skipping, and with collection propagation removed it fails on `collection was not found` — the exact defect this change fixes

## 5. Evidence wrapper and CI

- [x] 5.1 Add `scripts/run-e2e-smoke-qualification.sh` that builds the binaries, runs the smoke test, and writes `docs/evidence/e2e-smoke-qualification.json` stating host and scope
- [x] 5.2 Add the `test:e2e-smoke` npm script and register it in `package.json`
- [x] 5.3 Add a gating CI job that builds the binaries and runs the smoke test
- [x] 5.4 Confirmed the gate fails when the happy path breaks (verified locally in 4.6); the existing integration job also builds the binaries so the workspace test run covers the smoke target

## 6. Documentation and traceability

- [x] 6.1 Added traceability rows at archive, once the delta specs reached `openspec/specs/`: registered `operations/local-bootstrap-and-smoke` (LS) in the validator, inserted the 2 new `cloud/control-plane` scenarios with CP renumbering, and added 10 LS rows. Backed the rows with real tests rather than stretched citations — a new `mako-config` test for the relative-path fix, a new `mako-smoke` test for bootstrap re-run convergence and non-local refusal, and a new validator check that any mocked-backend citation must say so. 141 scenarios across 8 capability specs
- [x] 6.2 Annotated every `apps/console/test-e2e` citation as `(mocked backend)` and documented the distinction in the matrix preamble, so mocked coverage is no longer indistinguishable from end-to-end proof
- [x] 6.3 Document the bootstrap and smoke test in `docs/local-development.md`, and add any new doc to the `docs/README.md` index
- [x] 6.4 Run `npm run validate:docs` and `npm run validate:traceability`

## 7. Full verification

- [x] 7.1 `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] 7.2 `cargo test --workspace --lib --bins` and `cargo test --workspace --tests`
- [x] 7.3 `npm run format:check && npm run lint && npm run typecheck`, `npm run test:unit`, `npm run generate:api:check`
- [x] 7.4 Run the smoke test end to end and confirm the evidence file is produced
