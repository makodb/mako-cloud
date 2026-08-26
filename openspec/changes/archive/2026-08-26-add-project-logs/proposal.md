## Why

The observability contract already promises project logs: the control-plane spec's "Usage, quotas, logs, and health" requirement names them, `/observability/logs` exists in the OpenAPI contract and management SDK, and the telemetry store accepts and redacts `ProjectLog` records. But nothing emits them — and it turned out nothing could: the runtime supervisor's in-memory buffer holds only synthetic lifecycle entries (`deployment_loaded`, `invocation_completed`), because a worker isolate's console output never leaves the isolate. Its printed text went to container stdout, unattributed, unreadable by any tenant surface. A developer debugging yesterday's failure had nothing to read anywhere.

Log lines are also the one telemetry payload written by customer code, so they can carry anything: end-user emails, tokens, passwords. Collecting them without a scrubbing rule would make the platform a retainer of that data.

## What Changes

- The runtime supervisor writes a console shim next to each source-deployed bundle and points the worker at it: the shim patches `console` in the worker isolate, forwards every line to the real console so container logs stay whole, and ships captured lines back on an internal response header the supervisor strips and retains in its per-deployment buffer. Workers have no network permission, so the response is the only channel a line can travel.
- The control plane collects each deployed function's supervisor buffer off the request path, keeping a durable per-function high-water mark, and emits new lines as `ProjectLog` records over the existing telemetry pipeline — giving them the same retention, tenant scoping, and query surface every other signal has.
- Log text is scrubbed before it is stored: the existing central redaction (configured secrets, bearer and JWT values, password/cookie assignments, Mako credential prefixes) is extended with email-address masking, and the telemetry store applies the scrub at ingest so no producer can bypass it.
- The scrub is best-effort by design and documented as such: it masks token-, password-, and email-shaped text, not every possible secret.

**Deliberately excluded:** log-based alerting, retention configuration per project, and a dedicated console page — the console already shows live function logs from the supervisor, and the retained stream is served by the existing observability endpoint.

## Capabilities

- `cloud/control-plane` (modified): the logs half of "Usage, quotas, logs, and health" becomes real and gains the scrubbing guarantee.
