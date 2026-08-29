# Developer CLI

`mako` is the terminal counterpart of the developer console. Everything a
developer can do in a browser — sign in, own teams and projects, shape
collections and policies, issue keys, deploy functions, read logs, move data,
manage file storage — can be done from a shell, a script, or CI with the same
authorization and the same audit trail. It lives in `packages/cli`
(`@mako-cloud/cli`) and drives the management API through
`@mako-cloud/management-sdk`.

```bash
npm run build -w @mako-cloud/cli
node packages/cli/dist/main.js --help        # or, once linked: mako --help
```

Node 24 or newer is required. There are no other runtime dependencies.

## Parity with the console

The command tree is hand-written, but coverage is machine-checked:
`packages/cli/test/parity.test.mjs` loads `api/openapi/mako-cloud-v1.yaml`,
takes every operation outside the excluded prefixes, and fails when one has no
command or a command names an operation the API does not define. The exclusions
are printed by the test so a new one is a visible decision: `/v1/operator*`
(a separate identity), `/{projectRef}/functions` (invocation, an application
concern), and the application-runtime routes under an environment (`auth/*`,
`documents`, `service/*`, `replication/*`), which SDKs and RxDB clients call
with project credentials. Every other operation — 143 at the time of writing —
maps to a command.

## Signing in

```bash
mako auth login --endpoint https://cloud-test.makodb.com     # prompts for email and password
mako auth status
mako auth whoami
mako auth logout                                             # revokes the session server-side first
```

Passwords are typed at a prompt with echo off or read from a file
(`--password-file`); they are never accepted as an argument, so they never land
in shell history or process listings. Registration, email verification, and
password recovery are available as `mako auth register`, `verify-email`,
`resend-verification`, `recover-password`, and `reset-password`.

The session is stored per **profile** in a credential file readable only by
you: `$MAKO_CONFIG_DIR/credentials.json`, else
`$XDG_CONFIG_HOME/mako-cloud/credentials.json`, else
`~/.config/mako-cloud/credentials.json`. The directory is created `0700` and the
file `0600`; a file other users can read is refused with exit code 7 rather
than used. A profile records the endpoint, the access token, its expiry, your
email, and the refresh cookie the API sets on sign-in — the only thing that can
renew the session, which the CLI does automatically shortly before expiry.
`--profile <name>` (or `MAKO_PROFILE`) keeps several sign-ins apart; a profile
signed in to one endpoint is never sent to another.

Developer-auth calls (sign-in, refresh, sign-out, step-up) carry the endpoint
as their `Origin` header because the API's cross-site check requires it; the
CLI holds the refresh cookie deliberately, which is exactly what that check
exists to prevent a foreign site from doing.

## Running in CI

Set `MAKO_TOKEN` and `MAKO_ENDPOINT`. The token is used as given and never
written to disk. Automation tokens (`mako auth token create`) are the intended
credential: they are team-scoped, optionally narrowed to one project or
environment, and carry a fixed permission list (`project_read`,
`project_write`, `environment_read`, `environment_write`, `collection_write`,
`policy_write`, `function_deploy`, `audit_read`, `organization_read`). A
command the token's permissions do not allow fails with exit code 3 and the
API's message naming the permission. A developer session token can be supplied
the same way with `MAKO_TOKEN_KIND=developer_session`.

Actions the console gates behind a fresh password (data explorer grants and
other step-up actions) prompt on a terminal; without one, set
`MAKO_STEP_UP_PASSWORD_FILE` to a file holding the password, or the command
fails with exit code 3 and creates nothing.

## Output for people and for programs

On a terminal, lists render as tables and single resources as `key  value`
lines; progress and notices go to stderr. `--json` prints the API's response
verbatim (one document, or a page with `items` and `nextCursor`); `--all`
follows cursors and prints every page as one document. Nothing reaches stdout
that a script could not parse in JSON mode.

Exit codes:

| code | meaning |
| ---- | ------- |
| 0 | success |
| 1 | the API refused or failed in a way not classified below |
| 2 | usage error, or a destructive command refused without confirmation |
| 3 | not signed in, credential rejected, or step-up not possible |
| 4 | not found |
| 5 | conflict or precondition refused (409, 412, 422) |
| 6 | `--wait` reached its deadline |
| 7 | the credential store is unsafe |

Errors print `error <code>: <message> | request <id>` and, when the API says
so, `retry after <n>s` or `retryable`.

## Secrets and confirmations

Commands that issue a secret — public and service keys, automation tokens,
function secrets, invitation tokens — print it exactly once: as a delimited
block on a terminal, as the `secret` field with `--json`, or into a file
created `0600` with `--secret-file <path>`. It never appears in progress
lines, errors, or the credential file.

Commands that delete, suspend, retire, revoke, activate a policy, promote or
roll back a deployment, restore a backup, or mutate a document require
`--yes` or, on a terminal, the resource's identifier typed back. Without
either they refuse with exit code 2 before sending anything.

## Scoping

Environment-scoped commands take `--project <id>` (`-p`) and `--env <id>`
(`-e`), or `MAKO_PROJECT_ID` and `MAKO_ENVIRONMENT_ID` from the environment.
Identifiers that name the resource acted on are positional.

## Composed flows

- `mako projects create <name> --region <r> [--team <id>] --wait` polls until
  the project is active or failed (deadline `--timeout`, default 600 s; exit
  6 on the deadline, after printing the id and the last observed state).
  Without `--team` the project lands in your personal space.
- `mako functions deploy <dir> --name <fn>` bundles the directory, uploads it,
  creates a deployment, runs the health check, and promotes it unless
  `--no-promote`. Each step prints the identifier it produced and the
  lower-level command that resumes from there, so a failed run is never a
  mystery.
- `mako data export --collection <id> --output <file>` creates an export job,
  waits for it, obtains a download grant, and streams the artifact to the
  file. `mako data import --collection <id> --input <file>` obtains an upload
  grant, streams the file with its digest, runs the dry run, shows the result,
  and confirms only with `--yes` or a typed confirmation.
- `mako explorer …` issues a scoped explorer grant for the one call, performs
  it with the capability held in memory, and revokes the grant afterwards —
  also on failure. The capability is never printed or stored.
- `mako functions serve <dir>` runs a function locally in the pinned edge
  runtime, as before (see [local-functions.md](local-functions.md)).
- Storage: `mako storage buckets create <id> [--access policy|public]
  [--max-object-bytes n] [--content-type t ...] [--rules <@file|-|json>]`
  declares a bucket (see [file-storage.md](file-storage.md)); `update` sends
  only the options given. `mako storage buckets delete <id>` is refused while
  the bucket still holds objects unless `--delete-objects` confirms their
  loss. `mako storage objects list <id>` pages a bucket's objects by prefix
  (`--all` follows the cursor) and `objects delete <id> <path>` removes one.
- Email templates: `mako email-templates list` shows the four kinds an
  environment sends to its application users and which are still the built-in
  default; `set <kind> --subject <text> --body <@file|-|text>` saves a
  plain-text template (see [application-mail.md](application-mail.md); an
  unknown `{{variable}}` or a stray brace is refused with the API's message);
  `reset <kind>` returns to the default; `preview <kind>` renders the stored
  template with placeholder data, or `--subject`/`--body` render unsaved text
  before it is saved.
- Sign-in settings: `mako auth-settings get` shows an environment's external
  providers (client ids and whether a secret is installed, never the secret),
  redirect allowlist, and magic-link settings; `mako auth-settings set
  --input <@file|-|json>` replaces them whole (see
  [auth-providers.md](auth-providers.md)). A provider given without
  `clientSecret` keeps the installed one. The secret is sent once, in the
  request, and appears in no output.

## Hosted deployment

The public beta's Caddy admission allows every management route by an explicit
path allowlist and does not gate them on a browser origin; a probe from a
plain HTTP client reaches the control plane (401 without a credential, 400
for a malformed identifier) and only the developer-auth routes answer 403
without the `Origin` header the CLI sends. Nothing on the host changes for the
CLI.

## Tests

`npm run test:unit -w @mako-cloud/cli` runs the command groups against a
loopback mock of the management API (`packages/cli/test/harness.mjs`) with
assertions on requests, headers, output, exit codes, and secret handling, plus
the parity test. `cargo test -p mako-smoke --test developer_cli` boots the
local data and control planes, mints a developer session, and drives the built
CLI through a console workflow end to end; it skips with a notice when
`packages/cli/dist` has not been built.

## Command reference

<!-- generated: mako-cli-reference -->

Generated from the command registry; every command also answers `--help` with its full option list, and every one takes the global options `--endpoint`, `--profile`, `--config-dir`, `--json`, `--all`, `--yes`, `--wait`, `--timeout`.

### `mako activity`

| command | does |
| ------- | ---- |
| `mako activity` | Audited actions in this environment, newest first |

### `mako auth`

| command | does |
| ------- | ---- |
| `mako auth login` | Sign in with email and password and store the session for this profile |
| `mako auth logout` | Revoke the stored session server-side and remove it |
| `mako auth recover-password --email <email>` | Send a password recovery mail |
| `mako auth register` | Register a developer account with the hosted registration flow |
| `mako auth resend-verification --email <email>` | Send the verification mail again |
| `mako auth reset-password <token>` | Set a new password with the token from the recovery mail |
| `mako auth status` | Show which credential and endpoint commands will use |
| `mako auth token create --team <team-id> --name <name> --permission <permission> --expires-in <duration>` | Issue an automation token for a team; its secret is shown once *(prints a secret once)* |
| `mako auth token list --team <team-id>` | List a team's automation tokens |
| `mako auth token revoke <token-id> --team <team-id>` | Revoke an automation token; automation using it loses access immediately *(confirmed)* |
| `mako auth token rotate <token-id> --team <team-id> --replacement-id <token-id> --expires-in <duration>` | Replace an automation token with a new one; the old token stops working *(confirmed, prints a secret once)* |
| `mako auth verify-email <token>` | Confirm an email address with the token from the verification mail |
| `mako auth waitlist-status` | Show whether a registration is still on the wait-list |
| `mako auth whoami` | Show the signed-in developer, their personal space, and teams |

### `mako auth-settings`

| command | does |
| ------- | ---- |
| `mako auth-settings get` | Show the environment's sign-in providers (without secrets), redirect allowlist, and magic-link settings |
| `mako auth-settings set --input <@file\|-\|json>` | Replace the environment's sign-in settings from a JSON document; a provider without clientSecret keeps the installed one |

### `mako backups`

| command | does |
| ------- | ---- |
| `mako backups list` | Verified recovery points for an environment |
| `mako backups restore-requests create --project <project-id> --input <@file\|-\|json>` | Restore a verified backup into a new isolated environment (step-up verified) *(confirmed)* |
| `mako backups restore-requests list` | Restore requests for a project and their verification state |

### `mako collections`

| command | does |
| ------- | ---- |
| `mako collections create <collection-id> --schema <@file\|-\|json>` | Create a collection with its first schema |
| `mako collections get <collection-id>` | Show a collection, its active schema, and its state |
| `mako collections list` | List the environment's collections and their active schema versions |
| `mako collections migrations create <collection-id> --input <@file\|-\|json>` | Plan a schema migration to a target schema version |
| `mako collections migrations get <collection-id> <migration-id>` | Show a schema migration and its state |
| `mako collections migrations update <collection-id> <migration-id> --state <state>` | Move a schema migration to another state |
| `mako collections schema publish <collection-id> --schema <@file\|-\|json> --schema-version <n>` | Publish a new schema version; reports migration_required when documents do not fit |

### `mako data`

| command | does |
| ------- | ---- |
| `mako data export --output <path\|->` | Export a collection snapshot as JSON Lines: create the job, wait, download |
| `mako data import` | Import JSON Lines: upload with its digest, dry-run, then confirm |
| `mako data jobs cancel <job-id>` | Cancel a queued or running data job (committed rows are kept) *(confirmed)* |
| `mako data jobs get <job-id>` | Show one data job; --wait polls it to a terminal state |
| `mako data jobs list` | List import and export jobs of an environment |

### `mako email-templates`

| command | does |
| ------- | ---- |
| `mako email-templates get <kind>` | Show one email template's subject and body as they are in effect |
| `mako email-templates list` | List the environment's application email templates; defaults are marked until customized |
| `mako email-templates preview <kind>` | Render an email template with placeholder data; --subject or --body previews unsaved text |
| `mako email-templates reset <kind>` | Reset an email template to the built-in default |
| `mako email-templates set <kind>` | Customize an email template's subject and plain-text body; unknown {{variables}} are refused |

### `mako envs`

| command | does |
| ------- | ---- |
| `mako envs create <name>` | Create an environment in a project (--wait for provisioning) |
| `mako envs delete <env-id>` | Start an environment's deletion grace period *(confirmed)* |
| `mako envs get <env-id>` | Show an environment |
| `mako envs list` | List a project's environments |
| `mako envs restore <env-id>` | Restore a suspended environment or one in its deletion grace period |
| `mako envs suspend <env-id>` | Suspend an environment; it stops serving *(confirmed)* |

### `mako explorer`

| command | does |
| ------- | ---- |
| `mako explorer browse <collection-id>` | Page through a collection in primary-key order over a stable snapshot |
| `mako explorer get <collection-id> <document-id>` | Read one document under a short-lived explorer grant |
| `mako explorer history <collection-id> <document-id>` | List the retained revisions and tombstones of one document |
| `mako explorer mutate <collection-id> --mutation <@file\|-\|json>` | Commit a create, update, or delete with administrative access *(confirmed)* |
| `mako explorer plan <collection-id> --query <@file\|-\|json>` | Show which index a query would use, or the index it needs |
| `mako explorer query <collection-id> --query <@file\|-\|json>` | Run an indexed query and page through its results |
| `mako explorer simulate <collection-id> --mutation <@file\|-\|json>` | Validate a mutation against the schema, revision, and policy without committing |

### `mako functions`

| command | does |
| ------- | ---- |
| `mako functions create <name>` | Create a function with its configuration; deploy code with `functions deploy` |
| `mako functions delete <name>` | Delete a function and every deployment it has *(confirmed)* |
| `mako functions deploy <directory> --name <function-name>` | Upload a function directory, create a version, check its health, and promote it (promotion needs --yes or a typed confirmation) |
| `mako functions deployments create <name> --bundle <digest>` | Create a deployment version from an uploaded bundle digest |
| `mako functions deployments delete <name> <version>` | Delete a deployment version that is not active *(confirmed)* |
| `mako functions deployments get <name> <version>` | Show one deployment version |
| `mako functions deployments health <name> <version>` | Run the health check on a deployment version and record the outcome |
| `mako functions deployments list <name>` | List a function's immutable deployment versions |
| `mako functions deployments promote <name> <version>` | Make a healthy deployment version the active one *(confirmed)* |
| `mako functions deployments rollback <name> <version>` | Switch the active version back to a previously healthy one *(confirmed)* |
| `mako functions get <name>` | Show a function, its configuration, and its active version |
| `mako functions list` | List the functions in an environment |
| `mako functions logs <name>` | Read a function's sanitized logs, newest page first |
| `mako functions secrets create <name>` | Create a function secret and show its value once *(prints a secret once)* |
| `mako functions secrets get <name>` | Show a function secret's version and state, never its value |
| `mako functions secrets retire <name>` | Retire a function secret so no new deployment can attach it *(confirmed)* |
| `mako functions secrets rotate <name>` | Rotate a function secret and show the new value once *(confirmed, prints a secret once)* |
| `mako functions serve` | Run a function locally in the pinned edge runtime |
| `mako functions test <name>` | Invoke a function through the management test route and show the response |
| `mako functions update <name> --config <json>` | Replace a function's configuration |

### `mako indexes`

| command | does |
| ------- | ---- |
| `mako indexes create <collection-id> --name <name> --version <n>` | Create an index; it is built in the background and reported active when ready |
| `mako indexes delete <collection-id> <name> <version>` | Delete one index version *(confirmed)* |
| `mako indexes get <collection-id> <name> <version>` | Show one index version, its fields, progress, and any failure |
| `mako indexes list <collection-id>` | List a collection's indexes and their build state |

### `mako keys`

| command | does |
| ------- | ---- |
| `mako keys get <credential-id>` | Show a credential's kind, scope, and state (never its secret) |
| `mako keys public create` | Issue a public key for client apps; the secret is shown once *(prints a secret once)* |
| `mako keys retire <credential-id>` | Retire a credential; requests signed with it are refused from then on *(confirmed)* |
| `mako keys rotate <credential-id> --replacement-id <credential-id> --overlap <seconds>` | Issue a replacement credential and retire this one after the overlap *(confirmed, prints a secret once)* |
| `mako keys service create` | Issue a service credential scoped to collections and operations; shown once *(prints a secret once)* |
| `mako keys signing init` | Create the environment's first JWT signing key |
| `mako keys signing list` | List the environment's JWT signing keys and their states |
| `mako keys signing rotate --overlap <seconds>` | Rotate the JWT signing key; the old key verifies tokens until the overlap ends *(confirmed)* |

### `mako logs`

| command | does |
| ------- | ---- |
| `mako logs` | Retained, scrubbed log lines from functions, the data plane, and sync (newest first) |

### `mako observability`

| command | does |
| ------- | ---- |
| `mako observability audit` | Append-only administration history for this environment, newest first |
| `mako observability auth-events` | Sanitized application authentication outcomes, without credentials |
| `mako observability function-metrics` | Invocation, error, latency, and compute counts per function version and region |
| `mako observability health` | Regional data-plane service status and sanitized diagnostics |
| `mako observability index-state` | Index build state events per collection index, newest first |
| `mako observability logs` | Retained, scrubbed log lines from functions, the data plane, and sync (newest first) |
| `mako observability quotas` | Consumption against enforced limits, with the retry time when work was throttled |
| `mako observability replication-errors` | RxDB replication failures with retry guidance and correlation identifiers |
| `mako observability usage` | Retained usage samples per resource (flows sum their records; levels average their samples) |

### `mako policies`

| command | does |
| ------- | ---- |
| `mako policies activate <collection-id> <version>` | Activate a policy version; every client is authorized by it from then on *(confirmed)* |
| `mako policies draft <collection-id> --input <@file\|-\|json>` | Create a policy draft from {version, rules} |
| `mako policies get <collection-id>` | Show the active policy of a collection, or one version with --version |
| `mako policies rollback <collection-id> <version>` | Make an earlier policy version active again *(confirmed)* |
| `mako policies test <collection-id> <version> --examples <@file\|-\|json>` | Evaluate a policy version against example requests |
| `mako policies validate <collection-id> <version>` | Validate a policy version and report its diagnostics |

### `mako projects`

| command | does |
| ------- | ---- |
| `mako projects create <name> --region <region>` | Create a project in your personal space or a team (--wait for provisioning) |
| `mako projects delete <project-id>` | Start a project's deletion grace period *(confirmed)* |
| `mako projects get <project-id>` | Show a project |
| `mako projects list` | List projects in one team, or in every team you belong to |
| `mako projects rename <project-id> <name>` | Rename a project |
| `mako projects restore <project-id>` | Restore a suspended project or one in its deletion grace period |
| `mako projects suspend <project-id>` | Suspend a project; its environments stop serving *(confirmed)* |
| `mako projects transfer <project-id>` | Move a project to a team you administer, or to your personal space *(confirmed)* |

### `mako storage`

| command | does |
| ------- | ---- |
| `mako storage buckets create <bucket-id>` | Create a storage bucket; without rules a policy bucket refuses every request |
| `mako storage buckets delete <bucket-id>` | Delete a storage bucket; one that still holds objects is refused unless --delete-objects confirms their loss *(confirmed)* |
| `mako storage buckets get <bucket-id>` | Show a storage bucket, its limits, its rules, and its totals |
| `mako storage buckets list` | List the environment's storage buckets with their object counts and totals |
| `mako storage buckets update <bucket-id>` | Change a storage bucket's access, limits, content types, or rules; only given options are sent |
| `mako storage objects delete <bucket-id> <path>` | Delete one object from a bucket by its path *(confirmed)* |
| `mako storage objects list <bucket-id>` | List a bucket's objects in path order; --all follows the cursor to the end |

### `mako sync`

| command | does |
| ------- | ---- |
| `mako sync summary` | Aggregate RxDB synchronization diagnostics for a window (default: the last hour) |

### `mako teams`

| command | does |
| ------- | ---- |
| `mako teams bill <team-id>` | Show a team's bill for the current month or a closed period |
| `mako teams create <name>` | Create a team |
| `mako teams delete <team-id>` | Start a team's deletion grace period; its projects lose access immediately *(confirmed)* |
| `mako teams get <team-id>` | Show a team |
| `mako teams invitations accept <invitation-id> <token>` | Join a team with an invitation id and its token |
| `mako teams invitations create <team-id> --email <email> --role <role>` | Invite a developer to a team; the invitation token is shown once *(prints a secret once)* |
| `mako teams list` | List the teams you belong to, including your personal space |
| `mako teams members list <team-id>` | List a team's members and their roles |
| `mako teams members remove <team-id> <developer-id>` | Remove a member from a team *(confirmed)* |
| `mako teams members update <team-id> <developer-id> --role <role>` | Change a member's role |
| `mako teams rename <team-id> <name>` | Rename a team |
| `mako teams restore <team-id>` | Restore a team from its deletion grace period |

### `mako usage`

| command | does |
| ------- | ---- |
| `mako usage` | Retained usage samples per resource (flows sum their records; levels average their samples) |

### `mako users`

| command | does |
| ------- | ---- |
| `mako users create --email <email>` | Create an application user directly, without an invitation |
| `mako users delete <user-id>` | Delete a user *(confirmed)* |
| `mako users disable <user-id>` | Disable a user; their sessions stop working until restored *(confirmed)* |
| `mako users get <user-id>` | Show an application user, their metadata, and their sessions |
| `mako users invite --email <email>` | Invite an application user; they finish signing up themselves |
| `mako users restore <user-id>` | Restore a disabled user |
| `mako users revoke-session <user-id> <session-id>` | Revoke one session of a user *(confirmed)* |
| `mako users revoke-sessions <user-id>` | Revoke every session of a user *(confirmed)* |
| `mako users search` | Search application users by id or email (bounded; no credential material) |
| `mako users update-metadata <user-id> --input <@file\|-\|json>` | Replace a user's trusted and profile metadata |

### `mako workspace`

| command | does |
| ------- | ---- |
| `mako workspace check` | Probe DNS, TLS, routes, key, schema, and replication without reading documents |
| `mako workspace connect` | Public RxDB connection metadata: endpoint, active public key id, compatibility |
| `mako workspace nav` | Workspace destinations and whether your memberships permit each |
| `mako workspace summary` | Overview sections for an environment, each with its own freshness |
