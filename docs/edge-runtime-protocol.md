# Edge runtime pin and internal protocol

## Decision

Mako pins Supabase Edge Runtime `v1.74.3` at source commit
`47d04fdd22e33ea3fd904576cf3248d963d903a9` and pulls the multi-platform OCI
image only by digest:

```text
docker.io/supabase/edge-runtime@sha256:c52405002a890ca9fcf77978671c57f3a988e03174afb277f84ac65bc917013c
```

The machine-readable source of truth is
`infra/edge-runtime/runtime-pin.json`. The `mako-edge-runtime-protocol` crate
embeds and validates it so an invalid or mismatched pin fails runtime startup.

The evaluation was performed on 2026-08-06:

- `v1.74.0` is the latest entry on the upstream GitHub Releases page and has a
  verified signature.
- `v1.74.3` is the latest upstream source tag and published multi-platform
  image. Compared with `v1.74.0`, it includes a runtime-safety fix that returns
  JavaScript errors for malformed Node ECDH inputs instead of panicking the
  runtime host. That makes the patch tag preferable for untrusted workloads.
- A standalone Deno worker was not selected. It would require Mako to build and
  qualify isolation, worker lifecycle, npm compatibility, and resource-control
  behavior already supplied by Edge Runtime.

This pin is not an approval to expose traffic. The compatibility and adversarial
suites in tasks 11.15 and 11.16 remain release gates. Runtime upgrades require a
new digest, source commit, compatibility report, sandbox tests, and rollback
image. Moving tags are never deployment inputs.

## Boundary

Supabase documents Edge Runtime's configuration and APIs as beta. Mako therefore
does not expose its main-worker routes, errors, service paths, or
`EdgeRuntime.userWorkers` object to gateways, control-plane code, SDKs, or user
functions.

```text
control plane ---- lifecycle ----\
                                  > Mako runtime protocol v1 -> supervisor -> pinned Edge Runtime
edge gateway ----- invocation ---/
```

Only the supervisor adapter knows the upstream API. Replacing Edge Runtime must
not change the public function URL or this protocol's v1 behavior.

## Transport contract

Production transports use authenticated HTTP/2 on the private service network;
local development may use loopback HTTP. Every call carries
`x-mako-runtime-protocol: 1`, `x-mako-request-id`, W3C `traceparent`, and
`x-mako-trace-id`. A missing or unsupported protocol version fails closed with
`protocol_mismatch`. These headers must never be forwarded to user-selected
outbound destinations.

The semantic operations are:

| Operation | Idempotency and result |
| --- | --- |
| `health` | Reports protocol, pinned release/commit, region, and readiness. |
| `load` | Idempotently loads one `(project, environment, function, version)` and the exact bundle digest; a different manifest at that address is a conflict. |
| `probe` | Runs the deployment health check without changing active routing and returns only a state and sanitized diagnostic code. |
| `retire` | Stops new admissions, drains bounded in-flight work, terminates the worker, and is idempotent. |
| `invoke` | Targets an explicit immutable version; it never resolves the active version itself. |

Deployment control messages use the strict JSON types in
`mako-edge-runtime-protocol`. Unknown fields are rejected. Bundle bytes use a
bounded binary body whose SHA-256 digest must match the manifest. Secret
metadata contains only name/version references; decrypted values travel on a
separate sensitive channel immediately before worker creation.

Invocation preserves the original HTTP method, path/query, allowed headers,
bounded body stream, status, allowed response headers, and bounded response
stream. The gateway resolves the active version before invoking so promotion is
one atomic metadata switch. The sensitive `x-mako-caller-authorization` header
contains the verified caller credential for protected functions, is stripped
before user outbound fetches, and must never appear in diagnostics. Public
functions omit it rather than synthesizing an identity.

## Trust and failure rules

- The transport authenticates both workloads and authorizes the caller for the
  addressed region. Tenant identity comes from authenticated workload context
  and must match the message body.
- Bundle digests, runtime release, entrypoints, tenant identifiers, limits, and
  secret references are validated before worker creation.
- Protocol v1 has no unrestricted egress mode. A worker receives either
  `deny_all` or a bounded host allowlist with a per-invocation request count;
  the worker sandbox or its mandatory network proxy enforces that policy.
- The supervisor passes only explicitly attached environment names and values
  to the user runtime. The main runtime's environment is never copied wholesale.
- User exceptions and upstream error strings are mapped to stable
  `RuntimeErrorCode` values. Public responses receive Mako's normal error
  envelope and correlation ID, not supervisor internals.
- A worker crash, limit event, health failure, or protocol mismatch affects only
  the addressed deployment. Automatic retries are allowed only before a request
  reaches user code unless the caller supplied an application idempotency key.
- Logs exclude request/response bodies, authorization headers, environment
  values, and secret values. Only sanitized diagnostic codes cross the control
  protocol.

## Upstream adapter mapping

For this pin, the supervisor's main worker creates a user worker with
`EdgeRuntime.userWorkers.create`, explicitly supplies memory/wall/CPU limits and
selected environment variables, and calls `worker.fetch` with an abort signal.
Mako owns health and lifecycle endpoints; upstream example routes such as
`/_internal/health`, `/_internal/metric`, and `/_internal/upload` are not part of
the Mako protocol and are never reachable from the public gateway.
