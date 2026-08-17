# Edge functions

Mako runs short-lived TypeScript and JavaScript HTTP functions behind a stable
project route using a pinned Supabase Edge Runtime image and a Mako-owned
versioned protocol. Functions receive the Fetch `Request`/`Response` contract,
supported npm modules and WebAssembly, outbound fetch subject to policy, and
streaming responses.

## Deploy and invoke

1. Bundle and validate source into an immutable digest.
2. Create a function version referencing the exact bundle, runtime pin,
   entrypoint, selected regions, secret versions, limits, and JWT setting.
3. Health-check the version before promotion.
4. Promote by atomically switching the active version pointer.
5. Invoke through `/{projectRef}/functions/v1/{functionName}`.

JWT verification is on by default. A function can be public only through its
explicit webhook setting; routing, payload limits, quotas, and audit context
still apply. Rollback selects a previously healthy immutable version and does
not modify its bundle.

## Data access and secrets

`@mako-cloud/edge-sdk` forwards the verified caller to auth and document APIs by
default, so the same policies used by RxDB apply. Service-level maintenance is a
separate explicit client initialized with an attached scoped service secret and
produces a privileged-bypass audit record.

Function secret values are encrypted, attached by exact version, injected only
into the selected deployment, and never returned after creation. Logs redact
known active values. Ordinary environment and sensitive secret files remain
separate in local development.

## Isolation and regions

Each project deployment has a distinct worker identity. CPU, wall time, memory,
request/response size, concurrency, egress, and post-request work are bounded;
violations recycle only the affected worker. The gateway may fail over only to
another healthy region selected for that project. If none is healthy, it fails
instead of entering an unauthorized region.

Logs and metrics include sanitized status, latency, resource use, immutable
version, region, request ID, and trace ID. They exclude bodies, authorization
headers, environment values, and secret values.

## Local development

Follow [local function serving](local-functions.md). It uses the same immutable
runtime digest and request/environment mapping. Hosted regional routing,
revocation freshness, quotas, and production egress infrastructure are explicit
differences. The internal upstream boundary is documented in
[edge runtime protocol](edge-runtime-protocol.md).

## Running a hosted function locally

Local serving (`mako functions serve`) and hosted invocation are different paths. Serving runs the
pinned runtime for one function and answers directly, which is what a developer wants while writing
code. Hosted invocation goes through the gateway, which resolves the function against the control
plane and forwards to a supervisor holding a **registered deployment**.

`mako functions serve` cannot act as the hosted supervisor. It generates a random
`MAKO_RUNTIME_AUTHORIZATION` for its container, so the control plane cannot authenticate to it and
no deployment can be registered. To exercise the hosted path locally, run the runtime on the same
contract a deployment uses — see
`infra/ansible/roles/dependencies/files/quadlet/mako-edge-runtime.container` for the authoritative
form. The parts that matter:

- `MAKO_RUNTIME_AUTHORIZATION` must equal the internal auth secret the services use.
- `MAKO_RUNTIME_REGION` must equal `MAKO_REGION`, or the control plane reports the function
  unavailable in that region.
- Mount `packages/cli/runtime/main` at `/home/deno/functions/main` and publish container port 9000.

With that supervisor listening, `mako-local-bootstrap` deploys its sample function through the real
administrative path — create, bundle, deploy an immutable version, health-check, promote — and the
`deploy` step is what registers the deployment with the supervisor. Without a supervisor the
bootstrap skips function deployment and says so.

Invocation addresses the project reference, which encodes the environment:

```text
GET /{projectId}--{environmentId}/functions/v1/{functionName}
```

A bare project id never resolves.

On a host whose home directory is on a network filesystem, rootless Podman cannot pull the pinned
image into the default graph root (`lsetxattr ... operation not supported`). Use an isolated graph
root on a local filesystem, as described in [local development](local-development.md).

## Tested evidence

Run `MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-security` with a working
Docker or Podman runtime. It exercises the real pinned image, compatibility,
cross-project canaries, secret redaction, egress, limits, crash containment,
regional routing, and supply-chain audit. See
[edge qualification](edge-security-qualification.md).
