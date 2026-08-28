# Local development

Mako Cloud services run directly from the Rust and TypeScript workspaces during development. Docker Compose supplies discoverable infrastructure dependencies; Docker's `mako-cloud-local` network resolves services by the names in `infra/local/compose.yaml`.

## Prerequisites

- Node.js and npm versions accepted by `package.json`
- The Rust version declared in `Cargo.toml`
- OpenSSL 3
- Docker Compose v2, or Podman Compose with a working OCI runtime

For rootless Podman on a single-UID host, use an isolated graph root on a local
filesystem and the documented `overlay.ignore_chown_errors=true` option. Do not
weaken application-level runtime isolation tests or edit host container settings
from project setup scripts.

## Prepare local state

```bash
./scripts/local/prepare.sh
cp .env.example .env
```

The preparation script creates the ignored RocksDB, SQLite, backup, migration, restore, and reserve directories below `.local/`, generates a local CA plus a server certificate below `.local/certs/`, and generates the development secrets below `.local/secrets/`. Do not commit those private keys or trust the local CA system-wide.

Configuration accepts secret references, never inline values, so `.env.example` points at the generated files with `file:` references. Those paths are relative to the repository root — run the services from there. Re-running the script leaves existing secrets and certificates alone.

## Run the services

`ServiceConfig` reads the process environment; nothing auto-loads `.env`, so export it yourself:

```bash
set -a; . ./.env; set +a
cargo run --bin mako-data-plane      # 127.0.0.1:8080
cargo run --bin mako-control-plane   # 127.0.0.1:8081, in a second shell
```

`.env.example` deliberately leaves `MAKO_BIND_ADDR` unset so each service uses its own default port; exporting it pins every service to one address. Each service refuses to start unless its readiness passes, so a successful start means storage, identity, and dependencies are actually usable. Confirm with:

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/readyz
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8081/readyz
```

The control plane and data plane each hold an exclusive lock on their own database directory, so only one process per database can run at a time.

## Seed a working tenant

A freshly started stack has no project, so nothing can authenticate against it. `mako-local-bootstrap` seeds one — developer, team, project, environment, public project key, signing key, collection, and a development document policy — while the services are stopped:

```bash
set -a; . ./.env; set +a
cargo run --bin mako-local-bootstrap
```

It refuses to run outside a local environment, and it prints the public project key you need to call the data plane. See [end-to-end smoke verification](e2e-smoke.md) for the full flow and for the test that exercises it.

## Build the console

`apps/console/web-dist` is Vite build output and is not committed. The public-beta infrastructure validators inspect that bundle for its same-origin configuration and hosted authentication, and the release build packages it, so build it before running either:

```bash
npm run build --workspace @mako-cloud/console
```

Both report exactly this command if the directory is missing.

## Start dependencies

```bash
docker compose -f infra/local/compose.yaml up -d
```

Local endpoints are bound only to loopback:

- SMTP: `127.0.0.1:1025`
- Mailpit UI: http://127.0.0.1:8025
- S3-compatible object API: http://127.0.0.1:8333
- OpenTelemetry OTLP/gRPC: `127.0.0.1:4317`
- OpenTelemetry OTLP/HTTP: `127.0.0.1:4318`
- Prometheus: http://127.0.0.1:9090
- Grafana: http://127.0.0.1:3000

Rust services running on the host use `.env` loopback URLs. Services later added to the Compose network should use the Compose names `mailpit`, `object-store`, `otel-collector`, `prometheus`, and `grafana` for discovery.

## Stop dependencies

```bash
docker compose -f infra/local/compose.yaml down
```

Named infrastructure volumes and the service-specific paths below `.local/data/` are retained. Removing them is intentionally a separate, explicit operation.
