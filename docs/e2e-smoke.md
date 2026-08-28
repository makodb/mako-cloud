# End-to-end smoke verification

The smoke test proves that Mako Cloud's application happy path works against the real service binaries: an application user signs up, signs in, pushes a document, and pulls it back.

It exists because every other end-to-end suite in this repository runs against a mock. The reference application under `examples/local-first` ships an in-browser fake backend, and the console specs under `apps/console/test-e2e` intercept every `/v1/` request. Those suites prove client behavior; none of them prove a server implements the protocol. This one starts the actual processes and speaks HTTP to them, with nothing stubbed.

## Run it

```bash
npm run test:e2e-smoke
```

That builds the workspace binaries, runs the test, and writes `docs/evidence/e2e-smoke-qualification.json` recording the host and the exercised scope. To run the test alone against binaries you already built:

```bash
cargo build --workspace --bins
cargo test -p mako-smoke --test happy_path
```

`MAKO_SMOKE_BINARY_DIR` overrides where the binaries are found, for example to exercise a release build. A missing binary fails the test with an actionable message rather than skipping, because a gate that silently skips stops being a gate.

Each run allocates ephemeral ports and a temporary directory under `MAKO_STORAGE_TMPDIR`, so runs do not collide with a development stack or with each other.

## What it covers

- Local tenant bootstrap
- Application-user sign-up and sign-in
- Document replication push
- Document replication pull returning the pushed document with its content intact
- The negative control: the same document operations without the issued session are refused, which is what proves the successful run depended on real authentication

## What it does not cover

- Hosted developer registration, email verification, and operator wait-list review
- Management API project, environment, and collection administration
- Edge functions, the object store, telemetry, and the console user interface

## The reference application against a real backend

`examples/local-first` ships two backends behind one seam. Its default suite runs against
`FakeMakoBackend`, an in-browser implementation of the protocol, which proves the RxDB adapters are
correct but would pass with the server completely broken. `LiveMakoBackend` runs the same six
scenarios against real service binaries:

```bash
npm run test:browser-live
```

Offline writes, conflict resolution, remote tombstones, token refresh, live-stream reconnect, and
access revocation are all exercised over real HTTP, with remote edits coming from a second
authenticated application user rather than a staged fixture.

The application is served from an origin that also proxies `/v1` to the data plane, because the data
plane emits no CORS headers and is only reachable same-origin — the same topology a deployment gets
from its reverse proxy.

## Local tenant bootstrap

`mako-local-bootstrap` seeds a complete, usable tenant into the two owned stores: a developer identity, team, project, environment, public project key, project signing key, collection, and a permissive development document policy.

It exists because hosted developer registration requires mail delivery over authenticated TLS SMTP and an operator wait-list decision, none of which a local environment has. Without it there is no way to reach a working project locally.

```bash
set -a; . ./.env; set +a
cargo run --bin mako-local-bootstrap
```

It prints the created identifiers and the public project key as JSON. The key's secret is shown only at creation; a repeated run rotates the credential and reports the new one.

Two properties are deliberate and load-bearing:

- **It refuses to run unless the resolved environment is local.** It bypasses controls a hosted environment enforces for good reasons, including activating a developer without wait-list review and activating an allow-all document policy. It is a development tool, never a provisioning path.
- **It writes only to stores it exclusively owns while it runs**, so the services must be stopped. Starting it while a service holds a database lock fails with an explicit message.
