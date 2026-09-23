# Function Egress Allowlist and a Plaid-Connected Rational

## Why

An edge function today runs with `outboundNetwork: deny_all`: its sandbox may reach the platform API origin and nothing else, and the supervisor refuses the protocol's allowlist variant rather than half-enforce it. That stance is correct — but it means no function can call a third-party API, which shuts out the most common reason to have server-side code at all: talking to an external service the browser must not hold credentials for. Rational made this concrete (finding #36): its institution sync is a simulator because a real aggregator was unreachable by construction. Plaid's sandbox is free and self-serve, so the one thing standing between Rational and a real bank-data integration is our own egress policy.

## What Changes

- **Declared outbound egress for functions.** A function deployment may declare a bounded list of external hosts it needs. The runtime grants the worker's sandbox exactly those hosts (plus the platform API origin, as today) and refuses everything else at the permission boundary. Undeclared stays `deny_all` — the default does not move.
- **The unenforceable bound is removed rather than asserted.** Protocol v1's allowlist variant carries `maxRequestsPerInvocation`, which cannot be enforced from inside an isolate the tenant controls. The manifest's egress declaration becomes hosts-only; the per-invocation count is dropped from the contract instead of being honoured on paper. **BREAKING** for the internal edge-runtime protocol only (no deployed manifest carries the variant today — the supervisor refuses it).
- **Egress declarations are validated fail-closed.** DNS names only (no IP literals), HTTPS port only, refusing the platform's own origins and internal service names, with a small per-deployment cap on list length.
- **Deployment surface end to end**: control-plane function metadata stores the declaration, the OpenAPI deployment schema and generated types carry it, the CLI accepts it (`mako-cloud functions deploy --allow-host <name>`), and `mako-cloud functions serve` applies the same grant locally so local runs match hosted refusals.
- **Rational connects to Plaid Sandbox.** New connection kind `plaid` beside the simulator: a function route creates a Link token, exchanges the public token, and stores the access token in a server-only collection no application user can read; the existing 15-minute sync walks Plaid connections with `/transactions/sync` cursors, reusing the same idempotent write and alert seams the simulator proved. Tests run against a wire-mocked Plaid; real sandbox credentials are an optional live mode.

## Capabilities

### New Capabilities

None — egress is a new behavior of the existing edge-runtime capability, not a new capability.

### Modified Capabilities

- `functions/edge-runtime`: adds a requirement that a deployment may declare bounded outbound egress, enforced at the worker permission boundary, validated fail-closed, defaulted to deny-all, and honoured identically by local serving; amends workload isolation to name the hosts-only contract (the per-invocation request count leaves the protocol).
- `samples/rational-money-app`: adds a requirement that Rational syncs real accounts through Plaid Sandbox using the egress allowlist — Link-token issuance, server-only token custody, cursor-based transaction sync without duplication, and refusal to expose aggregator credentials or tokens to any application user.

## Impact

- **Rust**: `crates/mako-edge-runtime-protocol` (the `OutboundNetworkPolicy` type and its validation), `crates/mako-control-plane` (`FunctionConfiguration` + the manifest builder in `runtime_backend.rs`, today's single `DenyAll` hardcode). The edge gateway carries invocations only, not manifests; the Rust `mako-edge-runtime` crate is a reference model, not the live enforcement.
- **API**: `api/openapi/mako-cloud-v1.yaml` function configuration schema; regenerated `packages/api-types`.
- **TypeScript**: `packages/cli/runtime/main/` — the supervisor shipped into the pinned container is where enforcement actually lives (`supervisor.ts` refusal and grant assembly, `index.ts` for local serving) — plus `packages/cli` command surface (`functions deploy`, `functions serve`). `packages/edge-sdk` behavior unchanged.
- **Sample app**: `examples/rational` functions (`plaid` routes inside `institution-sync` or a sibling function), collections/policies (`plaid_items` server-only), bootstrap, UI (Connections screen gains Plaid), wire-mocked and live tests.
- **Docs and gates**: `docs/edge-functions.md`, `docs/edge-runtime-protocol.md`, `docs/rational.md`, requirements-traceability matrix, edge-security qualification (egress suite must now prove both the grant and the refusal).
- **External prerequisite**: a Plaid Sandbox `client_id`/`secret` (self-serve signup, owned by the user) for live-mode runs; all CI-gating tests must pass without them.
