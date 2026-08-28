## Why

The `mako` command today does exactly one thing: `mako functions serve` runs a function locally in the pinned edge runtime. Everything else a developer can do — sign in, create a team or project, publish a schema, activate a policy, issue keys, deploy a function, read logs, export data — exists only as a click in the developer console. A cloud database whose whole management surface is browser-only cannot be scripted, cannot run in CI, cannot be driven from an editor or an agent, and cannot be reproduced from a shell history. Supabase, Fly, and Vercel each ship a CLI that reaches everything their dashboard reaches; developers expect the same here.

The pieces already exist. `@mako-cloud/management-sdk` wraps every management, developer-auth, and operator operation in the OpenAPI document (199 operations), the console consumes it, and the CLI package is already an npm workspace member with a `mako` binary. What is missing is the command layer, a safe way to hold credentials outside a browser tab, and a guarantee that the CLI keeps pace with the API instead of drifting behind it.

## What Changes

- Grow `@mako-cloud/cli` from one command into a complete developer CLI: a noun–verb command tree (`mako auth`, `teams`, `projects`, `envs`, `collections`, `indexes`, `policies`, `users`, `keys`, `functions`, `logs`, `observability`, `activity`, `usage`, `bill`, `explorer`, `data`, `workspace`, `sync`, `backups`) that reaches every operation the developer console can reach, driven through the management SDK. `mako functions serve` keeps its name and behavior.
- Add developer authentication for a terminal: `mako auth login` (email and password, including registration, email verification, and password recovery as the hosted flow allows), profiles per endpoint stored in a mode-0600 file under the user's config directory, refresh on use, `MAKO_TOKEN` for CI (an automation token or a session token, never written to disk), and step-up prompts where the console asks for a password again (data explorer grants and other sensitive actions).
- Give scripts stable output: `--json` emits the wire types verbatim, human output is tables and sentences on a terminal, one-time secrets print once and are never logged, errors carry the API's stable code and retry advice, and exit codes are fixed per failure class.
- Compose the multi-step flows the console performs into single commands: `mako functions deploy` (bundle, upload, create deployment, health check, promote), `mako data export` / `mako data import` (data jobs with upload and download grants, dry-run, confirm), `mako projects create --wait` and other lifecycle waits.
- Enforce parity: a unit test maps every developer-facing operation in the OpenAPI document to a command and fails when one is unmapped, the same way the traceability matrix guards spec scenarios. Operator operations are deliberately excluded from this change and from the parity set.
- Document the CLI (`docs/cli.md`), add traceability rows, and cover each command group with unit tests over a mocked management endpoint plus an integration run against the local stack.

## Capabilities

### New Capabilities
- `cloud/developer-cli`: the terminal counterpart of the developer console — authentication and credential storage, complete command coverage of the developer management surface, machine-readable output, confirmation of destructive actions, and composed deploy and data-transfer flows.

### Modified Capabilities
- none — the CLI consumes existing management, developer-auth, and edge-function contracts unchanged. Widening what automation tokens may do (today: `organization_read`, `project_read`, `project_write`, `environment_read`, `environment_write`, `collection_write`, `policy_write`, `function_deploy`, `audit_read`, optionally narrowed to a project or environment) is a control-plane question left to its own change; with a developer session the CLI reaches everything, with an automation token it reaches what the token permits and says so.

## Impact

- `packages/cli` (new command modules, credential store, output layer, parity test), `packages/management-sdk` only if a wrapper is found missing during apply, `docs/cli.md`, `docs/README.md`, `docs/requirements-traceability.md`, `.github/workflows/ci.yml` (the CLI's integration test joins the existing integration job). No Rust changes, no OpenAPI changes, no data-plane deploy; the beta needs no upgrade for the CLI itself, only the npm package to be built and, when the user decides, published.
