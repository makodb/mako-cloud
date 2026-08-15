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

## Tested evidence

Run `MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-security` with a working
Docker or Podman runtime. It exercises the real pinned image, compatibility,
cross-project canaries, secret redaction, egress, limits, crash containment,
regional routing, and supply-chain audit. See
[edge qualification](edge-security-qualification.md).
