## Why

No automated test has ever driven Mako Cloud's application happy path. Every existing end-to-end suite runs against a mock: `examples/local-first` uses an in-browser `FakeMakoBackend`, and all four `apps/console/test-e2e` specs intercept `**/v1/**` with `page.route`. The hosted qualification in `scripts/run-public-beta-hosted-qualification.js` probes only for rejection (400/400/400/503) against a `prj_beta` that was never provisioned. The result is that unauthenticated refusal is well proven while successful sign-up, sign-in, and document replication are not proven at all.

Investigating this surfaced a defect that explains why: collections created through the management API are written to control-plane SQLite by `CollectionAdminService`, but the data plane resolves collection metadata from its own RocksDB via `DocumentEngine`. Nothing propagates between the two stores, and the internal-RPC operation set has no collection operation. Every push or pull against a collection created the documented way therefore fails `collection was not found`. The happy path is not merely untested — it cannot currently succeed.

## What Changes

- Add collection propagation from the control plane to the owning data plane, so a collection created through the management API becomes resolvable for document reads and writes. This closes the defect above.
- Add a local bootstrap tool that deterministically seeds a test developer, organization, project, environment, public project key, and collection into the two owned stores, so a working tenant exists without the mail, TLS SMTP, and operator wait-list chain that currently blocks first-developer creation.
- Add an end-to-end smoke test that starts the real `mako-control-plane` and `mako-data-plane` binaries and drives application-user sign-up, sign-in, document push, and pull over HTTP, asserting the document round-trips.
- Publish the smoke test in both forms the repository already uses: a Rust integration test for the flow assertions, and a script wrapper that emits evidence JSON under `docs/evidence/`.
- Gate CI on the smoke test and record its scenarios in `docs/requirements-traceability.md`, including correcting rows that currently cite only mocked console specs as automated coverage.
- Repair the documented local quickstart: `.env.example` ships `MAKO_INTERNAL_AUTH_SECRET_REF` commented out, so `mako-data-plane` refuses to start with `protected key material is unavailable`, and no control-plane configuration is documented at all.

## Capabilities

### New Capabilities
- `operations/local-bootstrap-and-smoke`: deterministic local tenant bootstrap and automated end-to-end verification that the application happy path — sign-up, sign-in, document push, document pull — succeeds against the real service binaries.

### Modified Capabilities
- `cloud/control-plane`: collection administration must propagate collection metadata to the owning data plane and must not report a collection usable until that propagation is durable.

## Impact

- **Defect fix**: `services/mako-control-plane/src/collection_http.rs`, `crates/mako-control-plane/src/collection.rs`, `crates/mako-internal-rpc/src/contract.rs`, `services/mako-data-plane/src/internal_http.rs`.
- **New tooling**: a local-only bootstrap binary, refusing to run outside `MAKO_ENVIRONMENT=local`; a new Rust integration test target; a script wrapper producing `docs/evidence/`.
- **CI**: a new gating job that builds the binaries and runs the smoke test.
- **Docs**: `.env.example`, `config/`, `docs/local-development.md`, `docs/configuration.md`, `docs/README.md`, `docs/requirements-traceability.md`.
- **No production behavior change** beyond the collection propagation fix. The bootstrap tool is fail-closed to local environments and is never a production provisioning path.
