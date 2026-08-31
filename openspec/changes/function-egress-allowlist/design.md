# Design: Function Egress Allowlist and Plaid-Connected Rational

## Context

See proposal.md for motivation. The mechanics being changed — with a correction to where enforcement actually lives, from tracing the code:

- **Enforcement is the TypeScript supervisor shipped into the pinned container**, `packages/cli/runtime/main/supervisor.ts`, which creates workers through `EdgeRuntime.userWorkers.create`. `allow_net` today is exactly one `host:port` — `networkGrants` (`supervisor.ts:1422`) throws on any mode but `deny_all` and returns `[originGrant(MAKO_API_URL)]`; the manifest validator `isLimits` (`supervisor.ts:802`) refuses a policy record carrying anything but `{mode: "deny_all"}`. The Rust `crates/mako-edge-runtime` crate is a reference model with no production `IsolatedWorker` — its `outbound_requests`/`outbound_bytes` are unpopulated telemetry fields, not limits.
- **The policy type** is `OutboundNetworkPolicy` in `crates/mako-edge-runtime-protocol/src/lib.rs:143` — `DenyAll` and `AllowList { hosts, max_requests_per_invocation }`. Its `valid_outbound_host` (`lib.rs:519`) already rejects IP literals and hard-denies `localhost` and cloud metadata names, and bounds lists to 64; `max_requests_per_invocation` is validated but observed by nothing.
- **The single `DenyAll` hardcode** is the control plane's manifest builder: `crates/mako-control-plane/src/runtime_backend.rs:255`, inside `RuntimeDeploymentClient::manifest` — the only `RuntimeLimits` field not derived from the deployment record. The edge gateway carries invocations only; deployment loading goes control plane → supervisor directly.
- **Function metadata** is `FunctionConfiguration` (`crates/mako-control-plane/src/function.rs:99`), stored as `deny_unknown_fields` JSON blobs in the generic `mako_kv` SQLite table and frozen per version; the flow is CLI (`configurationFromArgs`, `packages/cli/src/commands/functions.ts:111`) → OpenAPI `FunctionConfiguration` (`mako-cloud-v1.yaml:10419`, `additionalProperties: false`) → `FunctionRecord`/`FunctionVersionRecord` → `FunctionDeploymentSpec` → manifest.
- **Local serving** has its own grant literal: `packages/cli/runtime/main/index.ts:159` hardcodes `allow_net: [originGrant(MAKO_API_URL)]`, with the container launched by `packages/cli/src/serve.ts` (env map at `:178`, unknown-flag refusal at `:504`).
- Rational's document policies are default-deny allow-lists per collection; the service credential is privileged and bypasses them. Its `institution-sync` function already owns the connection/sync/alert seams (`examples/rational/functions/institution-sync/index.ts`), with idempotence by `(account, external_id)`.

## Goals / Non-Goals

**Goals:**
- Egress a deployment declares is enforced by the sandbox's own permission model — the same `allow_net` mechanism that already confines the worker, extended rather than bypassed.
- The default is unchanged: no declaration, no egress. Nothing existing redeploys differently.
- Rational's Plaid path is an ordinary use of public capability — no sample-app privileges, per the standing constraint.
- Every CI-gating suite stays hermetic (no external network).

**Non-Goals:**
- No per-invocation request counting, no bandwidth shaping, no egress proxy in this change. Aggregate outbound accounting stays whatever it is today.
- No wildcard or suffix host matching; no non-443 ports; no plain HTTP.
- No Plaid production (Trial plan) mode, no Plaid inbound webhooks — sync is schedule-driven; webhooks are a follow-up once egress exists.
- No UI for editing egress in the console; CLI/OpenAPI only for now.

## Decisions

**1. Hosts-only in the protocol; the count leaves the contract.**
`OutboundNetworkPolicy::AllowList` loses `max_requests_per_invocation` (becoming `{mode: "allow_list", hosts: [...]}`): the field is validated today and observed by nothing, and a field the platform accepts but does not enforce is a promise it cannot keep — the exact stance the current supervisor refusal encodes. Alternative considered: enforce the count in a mandatory network proxy — rejected for this change as a large infrastructure build serving no requesting workload; the protocol change leaves room to reintroduce a count later *with* a proxy. Breaking for the internal protocol only, with zero deployed instances (the supervisor refuses the variant today), so no migration exists in practice. The protocol's existing `valid_outbound_host` rules (no IP literals, hard-denied `localhost`/metadata names, ≤64 hosts) stay as the inner bound.

**2. Enforcement stays Deno `allow_net`, assembled in one shared place.**
`networkGrants` in `supervisor.ts` stops throwing on `allow_list` and unions the platform API origin with each declared `host:443` (reusing `originGrant`'s explicit-port normalization so granting a host never grants its neighbours); `isLimits` accepts the widened shape. Local serving calls the same exported grant logic from `index.ts` instead of its hardcoded literal, fed by a `--allow-host` flag on `serve` passed into the container as an env list (the `MAKO_USER_ENV_NAMES` pattern). Everything else — fetch, `Deno.connect`, WebSocket, DNS — is refused by the runtime exactly as undeclared destinations are refused today. No second enforcement path to keep honest. The guard tests that pin today's literals (`packages/cli/test/serve.test.mjs:182-197`) and today's refusals (`crates/mako-smoke/tests/edge_function.rs:589`, the protocol's adversarial egress test) are updated in the same motion — the smoke suite gains a *grant* case beside its denial cases.

**3. Validation is layered: protocol inner bound, control-plane outer bound, both addressed.**
`FunctionConfiguration::validate` (which already sorts, dedups, and bounds regions and secret names) additionally refuses: IP literals (v4/v6), ports other than 443, wildcards or empty labels, hosts matching the platform's public origin or internal service names, duplicate entries, more than **8** hosts, names longer than 253 octets or with non-LDH labels. Refusals use the addressed configuration-error convention (`CONFIG_INVALID_VALUE at functions.<name>.allowedHosts[i]`). The cap of 8 is a documented constant tighter than the protocol's 64; raising it is a deliberate change, not a config flag. The manifest builder maps a non-empty stored list onto the protocol variant and leaves everything else `DenyAll`.

**4. DNS rebinding is accepted and bounded, not solved.**
Deno's net permission authorizes by *name*, so a declared host whose DNS later answers with a private address would be connected to. Mitigations in scope: the worker runs inside the container's network namespace (its loopback is not the host's), internal RPC is loopback-only on the host, and the validator refuses declaring the platform's own names. A resolver-pinning proxy is the complete fix and is deliberately out of scope; the risk and its bound are documented in `docs/edge-functions.md`. This is the main trade-off of decision 2 and is called out in Risks.

**5. The declaration rides `FunctionConfiguration`, defaulting safely on old rows.**
One new field `allowedHosts: string[]` on `FunctionConfiguration` — the config already carried on the function and frozen per version, so a config change never retroactively alters a deployed version. Because stored records are `deny_unknown_fields` JSON blobs in `mako_kv`, the Rust field MUST be `#[serde(default)]` or every already-stored function fails to deserialize on read; the OpenAPI schema (`additionalProperties: false`) makes it optional-with-default-empty for the same reason. Coordinated four-place change: YAML → generated types → Rust struct → CLI (`configurationFromArgs` + a repeatable `--allow-host <name>` on `mako functions deploy`, shown by `functions list/show`). No SQL migration — there is no functions table to alter.

**6. Rational: Plaid rides `institution-sync`; secrets stay server-side twice over.**
- `PLAID_CLIENT_ID` / `PLAID_SECRET` are ordinary function secrets; the deployment declares `sandbox.plaid.com`.
- New collection `plaid_items` with an **empty policy rule set** — default-deny means no application user can read or write it; only the service credential touches it. It holds `{connection_id, access_token, item_id, cursor, ...}` and is never opened by the client (not in the RxDB schema).
- `connections` gains kind `"plaid"` carrying only non-secret metadata (institution name, account link, status, last sync) — replicated and visible like the simulator's.
- Routes on `institution-sync`: `POST /plaid/link-token` (authenticated member; function calls Plaid `/link/token/create`), `POST /plaid/exchange` (member hands over `public_token`; function calls `/item/public_token/exchange`, writes `plaid_items` + `connections`, returns only the connection id). The sync pass branches on kind: simulator connections keep `statement()`; plaid connections call `/transactions/sync` with the stored cursor, mapping added → create, modified → update, removed → delete (a pending charge replaced by its posted form), reusing `transactionId(account, external_id)` and the existing alert seam. Cursor advances only after the page's writes commit, so a crashed pass replays and idempotence absorbs it.
- No Plaid credentials configured → `serviceIsReachable`-style probe hides the option in the UI and the routes answer with an addressed "not configured" error.

**7. The model moves to schema version 2, and the bootstrap is the upgrade tool.**
Widening `connections.kind` and adding `plaid_items` is a schema change, and the platform's rules are deliberately strict: a publish must increase the version (`SchemaVersionMustIncrease`), and replication requires the client's version to *equal* the collection's — allowing an older writer after a "compatible" publish would trust that every possible old-version document fits the new schema, which the compatibility report (a check over *stored* documents) cannot promise. So the whole model moves together: `mako/collections.json` becomes `schemaVersion: 2`, every function and the client name it, and the bootstrap gains the upgrade arm — a deployed collection at an older version gets the model's schema published at the model's version, failing loudly on `migration_required` rather than half-upgrading. A device holding the old local schema cannot open the new one (RxDB refuses a changed schema at the same local version, correctly); Rational answers with the local-first move — erase the replica and re-pull — accepting the loss of unsynced edits at the moment of upgrade as the honest cost of shipping a new model to a static site without per-version migration code. Alternative considered: per-collection schema versions in the app — more machinery than a sample should carry, and the lockstep bump is what the platform's exact-equality rule steers toward anyway.

**8. Test strategy separates the two claims.**
- *The platform enforces the declaration*: edge-runtime container tests (behind `MAKO_RUN_EDGE_RUNTIME_TESTS=1`) deploy a probe function with one declared host mapped into the container (`--add-host` to the pasta host-loopback address) and assert the permission layer: undeclared destinations fail with a permission-shaped refusal; the declared one passes the permission layer (a TLS or connection error from the mock is proof the grant existed — distinguishable from `PermissionDenied`). No external network.
- *Rational's Plaid logic is right*: cursor handling, added/modified/removed mapping, and token-custody shapes live in `functions/shared/plaid.ts` as pure functions, unit-tested under `node --test` against recorded sandbox-shaped fixtures. The wire-mocked Playwright suite fakes the two function routes at the browser boundary as it already fakes the platform.
- *It really works against Plaid*: an opt-in live mode (`PLAID_CLIENT_ID`/`PLAID_SECRET` present) drives `/sandbox/public_token/create` to skip the Link widget, then the real sync path — run on the beta during qualification of this change, never in CI.

## Risks / Trade-offs

- [DNS rebinding steers a declared name at an internal address] → decision 4: namespace isolation, loopback-only internal RPC, platform names undeclrable, risk documented; full fix (pinning proxy) named as future work.
- [A declared host becomes an exfiltration channel for a compromised function] → unchanged trust model made explicit in docs: egress hosts are part of the reviewed deployment, visible in `functions show`, and bounded to 8 named HTTPS hosts; secrets redaction and no-body logging already limit what leaks through observability.
- [Plaid API drift breaks the sample] → the sync uses only four stable endpoints; live mode is opt-in so drift can never break CI; fixtures record the shapes the code was built against.
- [Beta egress: the host firewall may block outbound 443 from the edge-runtime container] → verify during apply on the beta before qualifying; if blocked, opening outbound 443 to declared hosts is an infra task in this change, not a silent relaxation.
- [Cursor loss (deleted `plaid_items` doc) forces a full resync] → `/transactions/sync` with no cursor replays history; idempotent ids absorb it at the cost of one heavy pass.

## Migration Plan

1. Platform first: protocol type + supervisor acceptance + validation + plumbing + CLI, behind nothing — the default is unchanged, so this deploys like any release.
2. Requalify the beta (existing cycle); the edge-security qualification gains the grant/refusal probes.
3. Then Rational: model + policies + functions + UI + tests; deploy the function with `--allow-host sandbox.plaid.com`; live-verify on the beta with the user's sandbox keys.
4. Rollback: the field is optional everywhere; rolling back the runtime release returns to refusing allowlist manifests, and a deployed declaration simply stops being honoured (functions fall back to refusal, not to open egress) — fail-closed in both directions.

## Open Questions

- Whether the beta host's outbound firewall already permits 443 from the container — checked during apply (risk above covers both outcomes).
