## 1. Scrubbing

- [x] 1.1 Extend the central redactor with email masking, composed with the existing token/password/JWT/credential scrubbing, as a log-specific scrub.
- [x] 1.2 Apply the scrub at telemetry ingest for every ProjectLog record, so the store is the choke point no producer can bypass.

## 2. Capture and collection

- [x] 2.0 A console shim, written by the supervisor next to each source-deployed bundle, captures the worker's printed lines and ships them back on an internal response header the supervisor strips; a bundle carrying a module by the shim's name keeps its own file and goes uncaptured. (Discovered during implementation: the supervisor buffer held only lifecycle entries, because isolate console output never left the isolate.)

- [x] 2.1 A control-plane collector walks organizations, projects, environments, and deployed functions, and pages each function's supervisor logs forward from a durable cursor.
- [x] 2.2 A high-water timestamp persists per function, so neither a collector restart nor a supervisor restart (whose positional cursors die with its buffer) re-emits what was already shipped. (Simpler than the per-function cursor the proposal sketched: the supervisor's cursors are positions into a volatile buffer, so the timestamp mark is the only state worth keeping.)
- [x] 2.3 Emission rides the existing bounded telemetry emitter, off the request path, with scrubbing applied before buffering.

## 3. Proof

- [x] 3.1 Unit tests: the scrub masks emails, bearer values, JWTs, and password assignments while leaving ordinary text and correlation ids alone.
- [x] 3.2 Ingest test: a ProjectLog record carrying a token and an email is stored masked.
- [x] 3.3 Collector tests: cursor advance, high-water dedup, invalid-cursor reset.
- [x] 3.4 End-to-end: the edge-function qualification asserts an invoked function's printed line is served by the observability logs endpoint, scrubbed.
- [x] 3.5 Traceability rows for the new scenarios: CP-24 and CP-25 in the matrix, enforced by validate:traceability.
