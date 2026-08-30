# Edge functions

Mako runs short-lived TypeScript and JavaScript HTTP functions behind a stable
project route using a pinned Supabase Edge Runtime image and a Mako-owned
versioned protocol. Functions receive the Fetch `Request`/`Response` contract,
supported npm modules and WebAssembly, outbound fetch subject to policy, and
streaming responses. A function's sandbox is deny-by-default: see
[what a function may do](#what-a-function-may-do).

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

The caller a function sees is the one the gateway verified. Your function is
handed that credential on the reserved `x-mako-caller-authorization` header,
which `createFunctionClientFromRequest` reads; the request's own
`Authorization` header is **not** forwarded, so a function that admits
anonymous callers cannot mistake an unverified bearer token for an identity.
A call with no verified caller reaches the function with none, and the caller
client then refuses rather than acting as somebody.

### The SDK is supplied by the runtime

A function imports `@mako-cloud/edge-sdk` and nothing else has to happen: the
runtime ships the built SDK beside its main worker, materializes it into the
worker's own directory, and maps that one specifier onto it. Nothing is
vendored into the bundle, no npm install runs, and the version a function gets
is the platform's.

It is the **only** bare specifier a bundle may import. Every other one must be
declared as a dependency mapping onto an uploaded module (`--dependency
<specifier>=<path>` on `mako functions deploy`); an import the platform cannot
resolve is refused at upload with an `unresolved_import` diagnostic rather than
failing when the worker boots. A bundle may not carry a module path beginning
with `__mako` or remap `@mako-cloud/edge-sdk` -- both are refused
(`reserved_module_path`, `reserved_dependency_specifier`), because those are
what the runtime injects.

Two environment values the runtime always injects locate the API and the
tenant: `MAKO_API_URL`, `MAKO_PROJECT_ID`, and `MAKO_ENVIRONMENT_ID`.

### One request id per data-plane request

Both clients send `X-Mako-Request-Id`, and the data plane keys a quota
reservation by it. Two data-plane requests carrying one request id are refused
with `409 conflict` naming the reuse, so a function that makes more than one
call derives a distinct id per call from the runtime's:

```ts
const requestId = request.headers.get("x-mako-request-id")!;
const read = createServiceClient({ ...options, requestId: `${requestId}r` });
const write = createServiceClient({ ...options, requestId: `${requestId}w` });
```

The same service client can set a user's administrator-controlled app metadata
-- the claims that user's next token carries -- through the audited
`…/service/users/{userId}/app-metadata` route. The credential must be scoped to
the reserved `users` target with `update`; the patch is a one-level JSON merge
(`null` removes a key); the reason defaults to the client's. A `households`
function that accepts an invitation would record the membership like this:

```ts
import { createServiceClient } from "@mako-cloud/edge-sdk";

const service = createServiceClient({
  endpoint: Deno.env.get("MAKO_API_URL")!,
  projectId: Deno.env.get("MAKO_PROJECT_ID")!,
  environmentId: Deno.env.get("MAKO_ENVIRONMENT_ID")!,
  serviceCredential: Deno.env.get("HOUSEHOLDS_SERVICE_KEY")!,
  reason: "household membership changed",
  requestId,
});

// Replaces the user's `households` claim whole; other keys are untouched.
const { authorizationEpoch } = await service.users.setAppMetadata(
  invitee.userId,
  { households: { ...invitee.households, [householdId]: "editor" } },
  `invitation ${invitationId} accepted`,
);
```

The write advances the user's authorization epoch, so the app should refresh
its session afterwards; the new `households` claim is on the refreshed token.
See [project authentication](project-auth.md) for the refusal and audit rules.

### Giving a function a credential you already hold

`HOUSEHOLDS_SERVICE_KEY` above is a scoped service credential stored as a
function secret. Create the credential, then store its value as a secret under
the name the function reads:

```bash
mako keys service create --id key_households \
  --collection memberships --operation read --operation update \
  --secret-file ./.local/households.key --project "$PROJECT" --env "$ENV"

mako functions secrets create HOUSEHOLDS_SERVICE_KEY \
  --value-file ./.local/households.key --project "$PROJECT" --env "$ENV"

rm ./.local/households.key
mako functions create households --secret HOUSEHOLDS_SERVICE_KEY --region local ...
```

`--value <v>` takes the value inline; `--value-file <path>` reads it from a
file and ignores one trailing newline, so it round-trips a `--secret-file` the
credential command wrote and keeps the value out of shell history. Over the
API this is `PUT
…/environments/{environmentId}/function-secrets/{secretName}` with
`{"value": "…"}`. Without it, the only way to give a function a credential was
to write it into the uploaded bundle, which leaves a secret at rest in a stored
artifact.

A supplied value is handled exactly like a generated one and is **never
returned** -- not by this call and not by any later read -- so the response
carries metadata only. Creating a secret that already exists is a conflict.

Function secret values are encrypted, attached by exact version, injected only
into the selected deployment, and never returned after creation. Logs redact
known active values. Ordinary environment and sensitive secret files remain
separate in local development.

## What a function may do

A worker is started with the capabilities a function needs and nothing else.
Everything below is refused at the runtime boundary, not by convention:

- **Network.** A function may open connections to the platform API origin the
  runtime injects as `MAKO_API_URL`, and to nothing else. `outboundNetwork:
  {mode: "deny_all"}` denies the function's own destinations -- another host,
  another port on the same host, a raw socket, a WebSocket, a DNS lookup -- and
  the injected `@mako-cloud/edge-sdk` keeps working, because the origin it talks
  to is the platform's, not one the function chose.
- **Files.** A function may read its own worker directory. It may not read
  anywhere else and may not write anywhere at all, including `/tmp`. Persist
  state in a document, an object, or a function secret.
- **Environment.** A function may read the secret names attached to its
  deployment and the `MAKO_*` values the platform injects. Any other name is
  refused rather than returned empty.
- **Modules.** Imports resolve inside the bundle, through a dependency mapping
  to another uploaded module, or to `@mako-cloud/edge-sdk`, which the runtime
  supplies. A module fetched over the network is never loaded: the bundle
  validator refuses a remote specifier and the worker holds no import grant.
- **Processes, native code, host identity.** A function may not spawn a
  process, open a native library, or read the machine it is running on.

`mako functions serve` applies the same grants locally, so a function that runs
locally is not one the hosted sandbox will refuse. The mechanics are in
[edge runtime protocol](edge-runtime-protocol.md#worker-permissions).

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

## Automated hosted invocation

`crates/mako-smoke/tests/edge_function.rs` performs the whole path: it starts the pinned runtime,
deploys a function through the administrative path, brings up the data plane, control plane, and
edge gateway, and asserts the function's own response comes back through the gateway. It also
asserts an undeployed function is not served and that a bare project reference does not resolve.

A second sample function covers the pattern this page documents end to end: it imports
`@mako-cloud/edge-sdk`, runs with a scoped service credential handed to it as a supplied secret
value, and creates and re-reads a document whose id contains `:` -- an id `encodeURIComponent`
escapes in the path. The suite then reads that document directly through the `/service/` route with
the same encoded id, and checks that reusing one request id across two service requests is refused
as a conflict.

That sample calls back into the data plane from inside the container, so the runtime container needs
a route to the host's loopback. Rootless Podman's default pasta networking does not provide one, so
the suite passes `--network pasta:--map-host-loopback,169.254.1.2`; `MAKO_EDGE_TEST_NETWORK`
replaces that value for another engine or host layout, and an empty value leaves the engine default
alone. Every service still binds loopback only.

```bash
MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-e2e
```

`MAKO_EDGE_TEST_ENGINE` selects `docker` or `podman`, and `MAKO_EDGE_TEST_ENGINE_PREFIX_JSON`
supplies arguments that must precede the subcommand — a JSON string array, used for an isolated
graph root on hosts where the default one cannot hold the image. Without
`MAKO_RUN_EDGE_RUNTIME_TESTS=1` the test reports why it is skipping and passes, matching the other
edge suites.

The suite requires ports 8080, 8081, 8082, and 9000, and says so when one is taken. The edge gateway
resolves its dependencies from compiled-in constants rather than configuration, so it cannot be
pointed at other ports and cannot run beside a development stack.

## Tested evidence

Run `MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-security` with a working
Docker or Podman runtime. It exercises the real pinned image, compatibility,
cross-project canaries, secret redaction, egress, limits, crash containment,
regional routing, and supply-chain audit. See
[edge qualification](edge-security-qualification.md).
