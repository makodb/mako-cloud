# Serving edge functions locally

`mako functions serve` runs a function in the same pinned Supabase Edge Runtime
release used by Mako's hosted runtime. Docker or Podman must be available; the
CLI starts the image by immutable digest and mounts both the function and Mako
main worker read-only.

JWT verification is on by default:

```bash
mako functions serve ./functions/hello-world \
  --project-id prj_abcdefgh \
  --environment-id env_abcdefgh \
  --api-url http://host.docker.internal:8787 \
  --jwks-file ./.local/jwks.json \
  --jwt-issuer http://localhost:8787/auth/v1 \
  --jwt-audience mako-app \
  --env-file ./.local/function.env \
  --secret-file ./.local/function.secrets
```

Invoke the function through its stable route:

```text
http://127.0.0.1:9000/prj_abcdefgh/functions/v1/hello-world
```

The CLI accepts `KEY=VALUE` files. `--env-file` is for ordinary configuration;
`--secret-file` identifies sensitive values. Only names explicitly present in
those files are mapped into the user worker. Secret values are supplied through
the child process environment, never placed in container arguments or dry-run
output. Runtime stdout and stderr redact exact active secret values before they
reach the terminal. Do not commit either local file when it contains credentials.

Use `--no-verify-jwt` only for a function configured as public. If a public
request supplies an `Authorization` header, the runtime still validates it; pass
the complete JWKS, issuer, and audience options if callers may send tokens. An
unverified token is never exposed as caller identity. Local signature and
tenant-claim checks match hosted ingress, but local serving cannot check remote
session revocation or authorization-epoch freshness unless the local identity
service supplying the JWKS is also running.

The function receives the hosted function-owned path (the public prefix is
removed), query string, standard method, headers, body stream, and request/trace
correlation headers. TypeScript, JavaScript, Fetch APIs, supported npm imports,
WebAssembly, outbound `fetch`, and streaming responses come from the pinned
Deno-compatible runtime. Regional routing, hosted quotas, and production egress
network enforcement are infrastructure differences; local resource limits are
still applied by the user worker.

The local worker runs with the same permissions a hosted deployment gets, so a
function that works here is not one the hosted sandbox will refuse: it may read
its own function directory, read the environment and secret names it was given,
and reach the `--api-url` origin. It may not write files, reach any other host
or port, load a module over the network, spawn a process, open a native
library, or read the machine it runs on. See
[what a function may do](edge-functions.md#what-a-function-may-do). A `fetch`
that a served function needs and the hosted sandbox denies fails the same way
in both places -- `Requires net access to ...` -- rather than passing locally
and failing after deployment. `--wall-time-ms` can lower the per-invocation
wall limit from its 300000 millisecond default and cannot raise it above that
hosted-compatible ceiling. The outer local runtime keeps a fixed shutdown grace
period beyond that limit so it can return the supervised function error instead
of racing the worker with an unrelated runtime-unavailable response.

Use `--dry-run` to inspect the redacted container command. Stop the command and
run it again after changing environment or secret files; source modules are
reloaded according to the pinned runtime's worker lifecycle.

## Importing the SDK

`import { createFunctionClientFromRequest } from "@mako-cloud/edge-sdk"` works
in a served function with no install step: the runtime supplies the built SDK
to the worker and maps that specifier onto it, the same way the hosted runtime
does. It is the only bare specifier a bundle may import -- see
[edge functions](edge-functions.md#the-sdk-is-supplied-by-the-runtime).

A served function reaching the data plane needs `--api-url` to name an address
the container can dial. `host.docker.internal` and `host.containers.internal`
reach the host, but **not** the host's loopback: a data plane bound to
`127.0.0.1` answers `connection refused`. Bind the data plane to an address the
container can reach, or -- with rootless Podman -- run the container with
`--network pasta:--map-host-loopback,169.254.1.2`, which is what
`crates/mako-smoke/tests/edge_function.rs` does.

## Deploying a function locally needs an object store

Serving is self-contained, but **deploying** is not. `mako functions deploy`
and the management API upload the bundle to the control plane, which stores the
artifact in the S3 object store `MAKO_OBJECT_STORE_ENDPOINT`,
`MAKO_OBJECT_STORE_ACCESS_KEY_REF`, and `MAKO_OBJECT_STORE_SECRET_KEY_REF`
name. Without a reachable, credentialed one, bundle upload and
`mako functions deployments create` answer
`503 function administration is unavailable`.

Two things surprise people here:

- `mako-local-bootstrap` does **not** need it. It constructs the function
  administration service in its own process with an in-memory object store, so
  the sample functions it deploys never touch S3. A bootstrapped tenant with a
  working function therefore proves nothing about whether your object store is
  usable.
- The compose object store in `infra/local/compose.yaml` starts with no S3
  identity of its own, so the platform's signed requests are refused
  (`InvalidAccessKeyId ... Available keys: 0`) until one is configured that
  matches the access and secret key the services resolve.

Deploying a function also needs a runtime supervisor the control plane can
authenticate to; `mako functions serve` cannot act as one. See
[running a hosted function locally](edge-functions.md#running-a-hosted-function-locally).
