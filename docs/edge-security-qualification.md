# Edge security qualification

`npm run test:edge-security` deliberately refuses to pass unless `MAKO_RUN_EDGE_RUNTIME_TESTS=1`. Configure Docker or Podman first; the test uses the repository-pinned Supabase Edge Runtime image by digest and never substitutes a tag.

The suite covers:

- exact tenant worker identities, concurrency admission, crash replacement, and isolation of healthy tenants;
- undeclared environment/secret access, cross-project secret canaries, log redaction, and explicit secret versions;
- CPU, wall-time, memory, request, response, egress, and post-invocation background-work containment;
- deny-by-default outbound access and no unauthorized regional fallback;
- real-runtime TypeScript, JavaScript, Fetch, npm, WebAssembly, streaming, and outbound-fetch compatibility;
- an immutable runtime source commit and OCI digest plus a high-severity npm dependency audit.

For rootless Podman on a single-UID host, use graph storage on a local filesystem and pass Podman's `ignore_chown_errors=true` overlay option through `MAKO_EDGE_TEST_ENGINE_PREFIX_JSON`. The release runner must provide adequate subordinate IDs or an equivalent isolated configuration; do not weaken the runtime's application-level tests.

## Latest qualification

On 2026-08-07, the pinned `v1.74.3` image at digest
`sha256:c52405002a890ca9fcf77978671c57f3a988e03174afb277f84ac65bc917013c`
passed both real-container integration tests under rootless Podman with isolated
local graph storage and the documented single-UID overlay option. The Rust
protocol/supervisor/gateway suites passed, and `npm audit --audit-level=high`
reported zero vulnerabilities.
