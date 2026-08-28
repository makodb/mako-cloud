## Context

See proposal.md for motivation. What shapes the approach:

- `packages/cli` is 549 lines: `main.ts` dispatches straight into `serve.ts` (`mako functions serve <directory> --project-id … --environment-id …`, with a pinned Deno supervisor under `runtime/`). There is no argument framework, no credential handling, and no network client.
- `packages/management-sdk` exposes `MakoManagementClient` (bearer developer session or automation token; every `/v1/teams`, `/v1/projects`, observability, functions, explorer, data-job, workspace, sync, and backup operation), `MakoDeveloperAuthClient` (register, verify email, sign in with email and password, refresh, sign out, password recovery, wait-list status), and `MakoOperatorClient`. The console holds the developer session in tab-scoped storage and obtains step-up grants with `verifyCurrentDeveloperPassword` before explorer grants and other sensitive actions.
- Identity domains are strictly separated (management, project auth, service, operator, developer-auth). A developer session reaches the whole console surface; an automation token is team-scoped with a fixed permission list and optional project or environment narrowing, and cannot reach credentials, application users, the explorer, data jobs, billing, or membership.
- Secrets are shown once by the API (`AutomationTokenIssue.secret`, credential issue responses, explorer grant capabilities marked "keep in memory only"). Destructive management calls require a confirmation header; lifecycle changes are asynchronous (202 then polling).
- Errors on the wire are addressed: stable code, message, and retry advice (`retry.afterMs`). Node >= 24; TypeScript strict; `node --test` over built output; Biome.

## Goals / Non-Goals

**Goals:**
- Everything a developer can do in the console can be done with `mako`, from a shell, in CI, or by an agent, with the same authorization and the same audit trail.
- Scripts can rely on the CLI: stable JSON, stable exit codes, stable command names, and a test that fails when the API grows an operation the CLI does not expose.
- Credentials are at least as safe as in the browser: never in shell history, never in logs, never world-readable, never persisted when supplied by the environment.

**Non-Goals:**
- Operator commands (`/v1/operator/*`): a separate identity and audience, left for a later change.
- Application-side commands (sign up application users as themselves, run replication): that is the RxDB client's job.
- A device or browser-handoff login flow: the developer-auth API has none; adding one is a control-plane change.
- Widening automation-token permissions; single-file binaries; shell completion beyond what the framework gives for free.

## Decisions

**1. A hand-written command tree over the SDK, with a machine-checked parity manifest.**
Commands are ordinary TypeScript modules calling `MakoManagementClient`, so each one can shape arguments and output for humans. Coverage is guaranteed by `packages/cli/src/parity.ts`: a table from OpenAPI `operationId` to command path, and a unit test that loads `api/openapi/mako-cloud-v1.yaml`, takes every operation outside the operator and application-runtime paths, and fails on any operation absent from the table or any table entry with no registered command. *Alternative rejected:* generating commands from the OpenAPI document — the generated surface would be correct and unusable (no composed flows, no prompts, no sensible defaults).

**2. Argument parsing with `node:util` `parseArgs`; no CLI framework dependency.**
The workspace avoids dependencies it does not need; `parseArgs` handles the noun–verb tree with a small dispatcher, and the help text is generated from the same command registry the parity test reads. *Alternative rejected:* commander or yargs — more features than the tree needs and another supply-chain surface for a tool that handles credentials.

**3. Profiles in a mode-0600 credential file; environment token wins and is never written.**
`$XDG_CONFIG_HOME/mako-cloud/credentials.json` (or `MAKO_CONFIG_DIR`) holds profiles keyed by name, each with endpoint, developer session (`accessToken`, `expiresAt`, identity), and nothing else. The file and its directory are created 0600/0700 and refused if group- or world-readable. `MAKO_TOKEN` (a developer session token or an automation token) and `MAKO_ENDPOINT` override the profile and are never persisted. Sessions are refreshed through the developer-auth client when within a margin of expiry; a wait-listed session is reported, not stored as usable. *Alternative rejected:* the OS keychain — no portable dependency-free access from Node, and CI has no keychain anyway.

**4. Step-up is a prompt, never a flag holding a password.**
Where the console re-asks for the password (explorer grants, other step-up-gated actions), the CLI prompts on the TTY with echo off; without a TTY it reads `MAKO_STEP_UP_PASSWORD_FILE` if set and otherwise fails with the addressed error. Grant tokens and explorer capabilities live in process memory only for the duration of the command.

**5. Output is for humans on a TTY and for machines with `--json`.**
`--json` prints the SDK's typed response verbatim (one document, or a page with `items` and `nextCursor`); `--all` follows cursors. Human output is tables for lists, key/value blocks for single records, and progress lines on stderr. Nothing is written to stdout that a script could not parse in JSON mode. Exit codes: 0 success, 1 API error, 2 usage error, 3 authentication or authorization, 4 not found, 5 conflict or refused, 6 timeout waiting, 7 credential store unsafe.

**6. One-time secrets print once, to stdout, and nowhere else.**
Commands that issue a secret (automation tokens, public and service credentials, function secrets) print it in a clearly delimited block on a TTY, or as the `secret` field in JSON mode; they never echo it in progress lines, errors, or verbose logs. `--secret-file <path>` writes it 0600 instead of printing. The redaction rule is applied to error rendering too.

**7. Destructive commands require `--yes` or an interactive confirmation typed as the resource name.**
Deletion, suspension, credential retirement, session revocation, policy activation on a collection with an active policy, and promotion of a function deployment prompt for the resource's name on a TTY and refuse without `--yes` otherwise. The management API's confirmation header is set only after that check.

**8. Composed flows are explicit sequences of existing calls, resumable by identifier.**
`mako functions deploy <dir>` bundles with the same bundler `functions serve` uses, uploads, creates the deployment, runs the health check, and promotes unless `--no-promote`; each step prints the identifier it produced so a failed run can be resumed with the lower-level commands. `mako data export` creates an export job, waits, obtains a download grant, and streams the artifact to a file; `mako data import` obtains an upload grant, streams the file, runs the dry run, and confirms only with `--yes` or after showing the dry-run result. `--wait` on lifecycle commands polls until active, failed, or a deadline (default 10 minutes) and exits 6 on the deadline.

**9. Tests are unit tests against a loopback mock of the management API, plus one integration run against the local stack.**
Each command group has a `node --test` file that starts an in-process HTTP server on 127.0.0.1, serves fixtures shaped by the generated types, and asserts requests, headers (bearer, confirmation, idempotency keys), output, and exit codes. `packages/cli/test/cli.integration.mjs` joins `npm run test:integration` and runs login, project creation with wait, a schema publish, a policy activation, a function deploy, a log read, and an export against the services started for the existing integration suite. Secret handling has its own test that greps the captured stdout, stderr, and credential file.

**10. The parity set is defined by path prefix, not by tag.**
Developer-facing operations are those under `/v1/teams`, `/v1/projects`, `/v1/developer-auth`, and `/v1/explorer` and data-job routes; excluded are `/v1/operator/*` and the application-runtime paths (`/auth/`, `/documents`, `/sync`, function invocation, `/service/`). The test prints the excluded prefixes so a new prefix is a visible decision.

## Risks / Trade-offs

- [Credential file on disk] → 0600 enforcement, refusal on unsafe modes, `MAKO_TOKEN` never persisted, `mako auth logout` revokes the session server-side before deleting it locally.
- [Parity test becomes a chore for every new endpoint] → that is the point; the table entry is one line, and the failure names the operation.
- [Composed flows hide which step failed] → every step prints its identifier and the exact lower-level command that resumes from it.
- [Human and JSON outputs drift] → both render from the same typed value; human rendering is a pure function tested per command.
- [Automation tokens cannot reach everything] → the CLI names the permission it needed in the exit-3 error, and `mako auth token create` lists the permissions that exist.

## Migration Plan

- Additive: the package keeps its name and binary; `mako functions serve` is unchanged. Build and unit tests run in the existing CI jobs; the integration test joins the integration job. Publishing the package is the user's call and is not part of the apply.
- Rollback is reverting the commit; nothing server-side changes.

## Open Questions

- Whether `mako` should offer `--profile` switching by endpoint automatically when `MAKO_ENDPOINT` differs from the stored profile (assumed: yes, endpoint is part of the profile key).
- Whether the beta's Caddy allowlist admits every management route the CLI will use from a non-browser origin (the same-origin rule applies to the console's routes; the CLI sends no `Origin`, which is the API's expected shape for tokens). To be verified during apply against the live admission files.
