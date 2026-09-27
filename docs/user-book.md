# The Mako Cloud User Book

*How to build an application on Mako Cloud: the console, the CLI, the API, the SDKs, and every capability the platform offers.*

This book is for developers who build applications **on** Mako Cloud. If you want to change Mako Cloud itself — its services, storage, deployment, or operations — read the [Dev Book](dev-book.md) instead.

Mako Cloud is an RxDB-native application backend: project authentication for your application's users, document-level access policies, RxDB replication (pull, push, and a live stream), file storage, Supabase-style edge functions with schedules, database webhooks, transactional mail, custom domains, and a management plane you reach from a developer console, a CLI, and a typed management API. Production state lives on a single node in exclusively owned local databases; MongoDB and SQL compatibility are deliberately out of scope.

The public wire contract is [`api/openapi/mako-cloud-v1.yaml`](../api/openapi/mako-cloud-v1.yaml). Where this book and that document disagree, the OpenAPI document wins.

## How to read this book

- **New to the platform?** Read [Concepts](#concepts) and [Getting started](#getting-started), then the chapter for the capability you need.
- **Building the client?** [Application authentication](#application-authentication) and [Building a local-first app with RxDB](#building-a-local-first-app-with-rxdb) are the core; [Document policies](#document-policies) explains what the server will and will not hand your client.
- **Building server-side logic?** [Edge functions](#edge-functions), [Scheduled functions](#scheduled-functions), and [Database webhooks](#database-webhooks).
- **Operating a project?** [Teams, projects, and environments](#teams-projects-and-environments), [Observability for your project](#observability-for-your-project), [Plans, quotas, and billing](#plans-quotas-and-billing), and [Troubleshooting](#troubleshooting).

Every chapter ends with **Where this is tested** where the platform has an automated test of the behavior described, so a claim in this book is something you can point at.

## Table of contents

1. [Concepts](#concepts)
2. [Getting started](#getting-started)
3. [The developer console](#the-developer-console)
4. [The developer CLI](#the-developer-cli)
5. [Teams, projects, and environments](#teams-projects-and-environments)
6. [Collections, schemas, and indexes](#collections-schemas-and-indexes)
7. [Document policies](#document-policies)
8. [Working with documents over HTTP](#working-with-documents-over-http)
9. [Application authentication](#application-authentication)
10. [Sign-in providers and magic links](#sign-in-providers-and-magic-links)
11. [Building a local-first app with RxDB](#building-a-local-first-app-with-rxdb)
12. [The replication protocol](#the-replication-protocol)
13. [Application file storage](#application-file-storage)
14. [Edge functions](#edge-functions)
15. [Scheduled functions](#scheduled-functions)
16. [Database webhooks](#database-webhooks)
17. [Application mail](#application-mail)
18. [Allowed origins (CORS)](#allowed-origins-cors)
19. [Custom domains](#custom-domains)
20. [Managing application users](#managing-application-users)
21. [The data workspace](#the-data-workspace)
22. [Observability for your project](#observability-for-your-project)
23. [Plans, quotas, and billing](#plans-quotas-and-billing)
24. [The public API and SDKs](#the-public-api-and-sdks)
25. [Sample applications](#sample-applications)
26. [Troubleshooting](#troubleshooting)
27. [Limits at a glance](#limits-at-a-glance)
28. [Glossary](#glossary)

---

## Concepts

### Three kinds of people, three kinds of identity

Mako Cloud keeps three identity domains strictly apart. A credential from one never works on another, and a matching email address never links them.

| Identity | Who | How they authenticate | What they reach |
| --- | --- | --- | --- |
| **Developer** | You — the person or team building on Mako Cloud | Developer session (console or `mako-cloud auth login`) or a team **automation token** | The management API: teams, projects, environments, collections, policies, keys, functions, observability |
| **Application user** | The people who use *your* application | Password, an external provider (Google, GitHub, OpenID Connect), or a magic link, always scoped to one project **and** one environment | The application API: their own documents (as policies allow), replication, file storage, function invocations |
| **Operator** | Mako Cloud platform staff | A separate operator identity with its own entitlements | `/v1/operator/…`, never reachable from a developer or application token |

Two further credentials belong to an environment rather than to a person:

- A **public project key** (`mako_pk.…`) identifies and meters a client. It is safe to ship inside a browser or mobile bundle: it grants no access to protected data by itself.
- A **service credential** (`mako_sk.…`) is a scoped secret for trusted server-side code — typically an edge function. It reaches the `/service/` routes, which bypass document policies within the credential's exact collection and operation scope, and every use writes a privileged-bypass audit record. Never put one in client code.

### Teams and the personal space

Projects belong to **teams**. Every developer also has a **personal space**: an implicit one-member team, created the first time you create a project without naming a `teamId` and reused afterwards. It is listed among your teams with `kind: personal`, is billed and limited like any team, and refuses invitations, membership changes, and deletion.

Team roles are `owner`, `administrator`, `developer`, and `viewer`. As a rule of thumb: any member may read; a role that can change projects (`developer`, `administrator`, `owner`) may write; owners and administrators manage membership, credentials, and sign-in settings. Function secrets go with functions: a role that may deploy a function may also create and rotate the secrets it names. A developer may read, but not create or rotate, project keys and signing keys.

### Projects, environments, and regions

A **project** is created in a **region** — a lowercase slug naming a deployment the platform serves (`local` on a local stack; the public beta VM serves one region of its own) and holds one or more **environments** — for example `production` and `development`. Almost everything you configure — collections, policies, users, keys, functions, storage buckets, webhooks, sign-in settings, allowed origins — is **environment-scoped**. Custom domains are the exception: they belong to the project and each serves one environment.

Projects, environments, and teams move through a lifecycle: `provisioning` → `active`, with `suspended`, `failed`, `deletion_grace`, `deleting`, and `deleted` as the other states. Deletion starts a grace period during which the resource can be restored.

### How your application addresses the platform

Everything an application calls lives under the environment's API URL:

```text
https://<platform host>/v1/projects/{projectId}/environments/{environmentId}/…
```

Functions are invoked through a **project reference**, which encodes the environment:

```text
https://<platform host>/{projectId}--{environmentId}/functions/v1/{functionName}
```

A bare project id never resolves as a project reference. A [custom domain](#custom-domains) serves the same application routes on your own hostname, and functions there as `/functions/v1/{functionName}` (the hostname names the environment).

Tenant identity always comes from the verified credential and must match every `projectId` and `environmentId` in the path. The platform never infers authorization from a field inside a document or from a public key.

### Collections, documents, policies

Data is JSON **documents** in **collections**. A collection has a versioned **JSON Schema** and a **primary key** definition; indexes make queries possible (there is deliberately no collection scan). Every collection is **default deny**: a **document policy** — a small set of allow/deny rules over the caller's verified identity, trusted claims, and the document's old and new state — decides every create, read, update, and delete, on every path (point reads, indexed queries, replication pull, live stream, conflict responses, and calls from your edge functions alike).

### Local-first by construction

The supported client is `@mako-cloud/rxdb`. Your application reads from an on-device RxDB database, writes locally while offline, and replication carries changes both ways: **pull** and **push** for batches, a server-sent-event **live stream** for changes as they happen. When a policy or a user's claims change, the platform advances an **authorization epoch**; the client notices, clears what it should no longer hold, and starts a fresh replication generation.

### Identifiers

Identifiers are prefixed strings: `org_` teams (including your personal space), `dev_` developer identities, `inv_` team invitations, `atm_` automation tokens, `prj_` projects, `env_` environments, `mig_` schema migrations, `usr_` application users, `ses_` application sessions, `sch_` schedules, `run_` schedule runs, `whk_` webhook endpoints, `whd_` deliveries, `dom_` domains, `opr_` operators, `aml_` mail intents. Credential ids are the name you give them (`mako-cloud keys service create --id key_households`), collection ids are lowercase slugs, and document ids are whatever your primary key holds. Treat them as opaque; only their uniqueness is promised.

---

## Getting started

### 1. Get a developer account

Mako Cloud's hosted registration is a wait list. On the console's **Create account** page (`/create-account`) you enter an email and password; a verification mail arrives and its link (`/verify-email`) moves your account from `unverified` to `waitlisted`. While wait-listed you can sign in, but the session can only check your status (`/wait-list`), refresh, recover a password, and sign out — no management call succeeds until a platform operator approves your application. Approval requires a fresh sign-in.

The same flow is available from a terminal:

```bash
mako-cloud auth register            # prompts for email and password
mako-cloud auth verify-email <token from the mail>
mako-cloud auth waitlist-status
```

Registration, resend, and recovery responses are deliberately generic: they never say whether an address is already known. Password recovery invalidates every existing session.

On a local stack there is no mail and no operator, so `mako-local-bootstrap` seeds a ready-made developer, team, project, and environment instead — see [Running a stack locally](#running-a-stack-locally).

### 2. Sign in

In the console, sign in at `/login`. Your access token lives in tab-scoped session storage and is renewed through a rotating, `HttpOnly`, `SameSite=Strict` refresh cookie; closing the tab or signing out removes it.

From a terminal:

```bash
mako-cloud auth login --endpoint https://cloud-test.makodb.com    # prompts for email and password
mako-cloud auth whoami
```

`--endpoint` is the platform host that serves the management API. The CLI stores the session per profile in a credential file only you can read and renews it automatically. For CI, mint an automation token instead (see [Running in CI](#running-in-ci)).

### 3. Create a project and an environment

```bash
mako-cloud projects create "Todos" --region <region> --wait      # lands in your personal space; `local` on a local stack
mako-cloud envs create production --project prj_… --wait
```

`--wait` polls until provisioning is `active` (or `failed`, with the diagnostic). Add `--team org_…` to create the project in a team. The console offers the same on the home page, and its guided first run walks a new developer through exactly these steps.

Most environment-scoped commands below take `--project`/`-p` and `--env`/`-e`, or read `MAKO_PROJECT_ID` and `MAKO_ENVIRONMENT_ID` from the environment; the examples omit them.

### 4. Define a collection

A collection is created with its first schema version: a JSON Schema for the document body plus a primary-key definition (`--primary-key <field>`, or a `primaryKey` member inside the schema object as RxDB schemas carry):

```bash
cat > todos.schema.json <<'JSON'
{
  "type": "object",
  "required": ["id", "ownerId", "title", "updatedAt"],
  "properties": {
    "id": { "type": "string" },
    "ownerId": { "type": "string" },
    "title": { "type": "string" },
    "done": { "type": "boolean" },
    "updatedAt": { "type": "integer" }
  }
}
JSON
mako-cloud collections create todos --schema @todos.schema.json --primary-key id      # schema version 1
mako-cloud indexes create todos --name by_owner --version 1 --field ownerId --field updatedAt
```

Indexes build in the background and report `active` when ready. See [Collections, schemas, and indexes](#collections-schemas-and-indexes) for schema evolution and the query planner's rules.

### 5. Write and activate a policy

Without an active policy every request is refused. A minimal owner-only policy:

```bash
cat > todos-policy.json <<'JSON'
{
  "version": 1,
  "rules": [
    { "id": "owner-reads",  "effect": "allow", "operations": ["read"],   "expression": "old.ownerId == identity.user_id" },
    { "id": "owner-writes", "effect": "allow", "operations": ["create"], "expression": "new.ownerId == identity.user_id" },
    { "id": "owner-edits",  "effect": "allow", "operations": ["update"], "expression": "old.ownerId == identity.user_id && new.ownerId == identity.user_id" },
    { "id": "owner-deletes", "effect": "allow", "operations": ["delete"], "expression": "old.ownerId == identity.user_id" }
  ]
}
JSON
mako-cloud policies draft todos --input @todos-policy.json
mako-cloud policies validate todos 1
mako-cloud policies activate todos 1 --yes
```

A draft is immutable; validation compiles it against the collection's active schema; activation is atomic and advances the environment's authorization epoch exactly once. A rule can only name the document states its operations have: `new` exists for create and update, `old` for read, update and delete, so a delete is its own rule over `old` (a rule covering both update and delete that reads `new` fails validation with `state_unavailable`). [Document policies](#document-policies) has the full expression language.

### 6. Issue keys

```bash
mako-cloud keys signing init                     # the environment's first JWT signing key
mako-cloud keys public create --secret-file ./mako_pk.txt   # the public project key your client ships
```

The public key's value is shown exactly once, here or in the console's **API & Connect** page; nothing can read it back later, only rotate it.

### 7. Connect a client

**API & Connect** in the console shows the API URL, the active public key id, the supported RxDB range, and copyable quickstarts; `mako-cloud workspace connect` prints the same metadata and `mako-cloud workspace check` probes DNS, TLS, routes, the key, the schema, and the replication route without reading a document. Then, in your application:

```ts
import { MakoAuthClient, BrowserAuthSessionPersistence, normalizeMakoRxdbConfig } from "@mako-cloud/rxdb";
import { RXDB_VERSION } from "rxdb/plugins/utils";

const config = normalizeMakoRxdbConfig({
  endpoint: "https://cloud-test.makodb.com",
  projectId: "prj_…",
  environmentId: "env_…",
  collectionId: "todos",
  schemaVersion: 1,
  publicProjectKey: "mako_pk.…",
  rxdbVersion: RXDB_VERSION,
  runtime: "browser",
});
const auth = new MakoAuthClient(config, { persistence: new BrowserAuthSessionPersistence(config) });
await auth.signUp("person@example.com", "correct horse battery staple");
await auth.signInWithPassword("person@example.com", "correct horse battery staple");
```

Before a browser page on another origin can call the API, list that origin under the environment's **Allowed origins** ([details](#allowed-origins-cors)):

```bash
mako-cloud allowed-origins set --origin http://127.0.0.1:5173 --origin https://app.example.com
```

[Building a local-first app with RxDB](#building-a-local-first-app-with-rxdb) continues from here to a replicating collection.

### Running a stack locally

You can run the whole platform on your machine; the [Dev Book](dev-book.md#local-development) has the complete procedure. The short version:

```bash
./scripts/local/prepare.sh && cp .env.example .env
set -a; . ./.env; set +a
cargo run --bin mako-local-bootstrap        # once, with the services stopped: seeds a developer, project, env, key, collection, policy
cargo run --bin mako-data-plane             # 127.0.0.1:8080
cargo run --bin mako-control-plane          # 127.0.0.1:8081, in a second shell
```

The bootstrap prints the identifiers and the public project key as JSON. Point `mako-cloud auth login --endpoint http://127.0.0.1:8081` and your client's `endpoint: "http://127.0.0.1:8080"` at it; plain `http` is accepted only for `localhost` and `127.0.0.1`. The local data plane sends CORS headers only for listed origins, exactly like production, so either list your dev-server origin or proxy `/v1` same-origin as the sample applications do.

### Where this is tested

The end-to-end smoke suite (`npm run test:e2e-smoke`, `crates/mako-smoke/tests/happy_path.rs`) bootstraps a tenant against the real service binaries, signs an application user up and in, pushes a document, pulls it back, and proves the same operations are refused without the session. `cargo test -p mako-smoke --test developer_cli` drives the built CLI through a console workflow against the real services.

---

## The developer console

The console is what a developer sees from sign-in onward. It is organised in three levels — home, project, environment — each with the same navigation shape, so any product area of any environment is at most two selections from the home page. The active team, project, environment, and developer identity stay visible in the shell; URLs carry only safe identifiers and destination names; navigation is keyboard-operable and the current destination is announced to assistive technology through `aria-current`.

### Home

Signing in opens the home dashboard. The root separates **Personal projects** from **Teams**. Personal projects belong to your account. Shared projects are grouped by team under **Teams**. Each row shows lifecycle state, region, plan, and usage for the current period. Select a project name to open its workspace, or use **Browse data**, **Schema**, and **Connect** to open its active database environment. Each row loads its summaries independently, so a usage failure affects only that project. Project audit events are available from its **Activity** screen.

**Create project** opens the creation form. Personal projects have no search field, and there is one creation action on the page. Empty accounts use the first-project form instead. The workspace sidebar has **Personal projects**, **Usage and plan**, and a **Teams** section containing only shared teams. Each project row links to its **Billing** page. **Usage and plan** shows combined usage, shared allowances, base fees, credits, and balances for your personal projects and each team. The team switcher lists only shared teams. The header's **Docs** link opens the User Book.

A developer with no projects is offered a guided first run that creates the project and opens its workspace. The project page provides keys, the API URL, quickstart, and a connection check. Dismissing the guide is remembered for the browser tab; you can reopen it from the empty state.

### Project

A project's home summarises its environments and their readiness, the selected environment's API URL, public key, and quickstart, its usage against quota, data-plane health, and recent activity, and offers **Overview**, **Usage**, **Billing**, **Activity**, **Domains**, and **Settings** alongside the environment list.

**Settings** show the owner, region, identifiers, and lifecycle, and offer the three changes an owner may make: renaming the project, transferring it between the personal space and the teams the developer administers, and requesting deletion with its grace period. Each asks for confirmation and is audited. A transfer keeps the identifier, environments, data, policies, users, keys, and functions, holds every environment to the new owner's plan before the owner changes, and is recorded under both the previous and the new owner.

**Domains** are project-level: the section lists every custom domain with the environment whose API and functions it serves, its state — pending, verified, or failed — and when it was verified and last checked. Adding one takes a hostname and an environment and answers with the DNS TXT record that proves control of the name (name, type, value, each copyable), which stays available per row under "Show DNS record". "Verify now" checks the record immediately instead of at the next periodic check. A domain whose record later disappears is marked failed with why serving stopped, and a confirmed "Remove" stops serving the name and its certificate renewal. See [Custom domains](#custom-domains).

Routes: `/projects/{projectId}` and `/projects/{projectId}/{overview|usage|billing|activity|domains|settings}`.

### Environment

Inside an environment the sidebar groups destinations under **Database**, **Application**, **Observe**, and **Configure**. The project name and environment selector remain visible. On narrow screens, **Go to** selects a destination without a wide sidebar. Navigation follows the permissions returned by the platform.

| Destination | What it holds |
| --- | --- |
| **Overview** | Environment lifecycle, collection inventory, function and recovery-point counts, usage observations, recent activity, and database setup actions |
| **Data** | The [data explorer](#the-data-workspace): browse, query, history, policy preview, administrative mutations, import and export jobs |
| **Collections** | Collections, schema versions, migrations, and indexes |
| **Sync** | Aggregate RxDB replication diagnostics |
| **Users** | Application users: search, invite, create, disable, restore, metadata, sessions |
| **Policies** | Drafts, validation, examples, activation, rollback |
| **Functions** | Functions, deployments, secrets, logs, and — on a function's page — its [schedules](#scheduled-functions) |
| **Storage** | Buckets with object counts and stored bytes; each bucket's access, limits, and rules; its objects, listed by prefix, paged, and deletable |
| **Webhooks** | Endpoints with subscriptions, state, and failure count; registration (the signing secret shown once in a dismissable panel); details, settings, enable/disable, confirmed secret rotation, resume after a platform pause, confirmed delete, and the delivery log with redelivery |
| **Auth providers** | The environment's sign-in settings edited as one unit: providers with client ids, scopes, and enabled state; the redirect allowlist; magic links. A client secret is typed once, sent once, and never displayed again — the screen only says whether one is stored |
| **Email templates** | The four application emails with the variables each may use, a server-rendered preview with placeholder data, save per kind, and a confirmed reset to the built-in default |
| **Observability** | Usage, quotas, health, replication errors, auth events, function metrics, index states, audit |
| **Logs** | Retained, scrubbed function output |
| **Usage** | This month's meters against the plan |
| **Activity** | The audit trail as you may read it |
| **Backups** | Verified recovery points and isolated restore requests |
| **API & Connect** | Endpoint, active public key id, compatibility, the RxDB connect template, and the connection check |
| **API keys** | Public and service credential management, subject to your permissions |
| **API docs** | The environment's own generated API reference (below) |
| **Settings** | Allowed origins and environment lifecycle |

**Overview** loads collection inventory separately from summary metrics, so a telemetry failure does not hide your schemas. Each summary marks when it was observed and whether it is current, stale, or unavailable. **Refresh** reloads both sources. Usage shows the latest returned sample per resource within the displayed one-hour window, not a billing total. Partial counts and truncated observations are labelled. Recent activity shows at most eight audit events without free-form audit details. Lifecycle state describes the project and environment, not database service health.

**Allowed origins** live in Settings: the browser origins that may call this environment's application API from a page served somewhere else. The section shows what is allowed now ("None" when the list is empty) and edits the whole list one origin per line, matched exactly as the browser sends it, at most 16. The list is checked in the browser before anything is sent — a path, a query, a trailing slash, a default port, or plain `http` off loopback is refused with its reason — and Save replaces the whole list with an idempotency key.

**API docs** (`…/api-docs`) is the environment's own API reference, generated in the browser from what the console already reads and stamped with the time it was observed — nothing is rendered by the server or stored. It shows the API URL, the public key id, and the headers each route takes; the auth endpoints with request and response bodies; for each collection its document shape from the schema, its indexes, the operations the active policy allows (an allow rule whose expression is `true` makes an operation allowed, any other allow expression makes it conditional, an operation no allow rule names is denied, and deny rules are listed beside), and example create, read, query, update, delete, and RxDB pull, push, and stream requests built from a sample document; each function's route, active version, and an invocation; each bucket's upload, download, list, and delete; and copyable curl, JavaScript (`fetch`), and RxDB replication quickstarts. Only public key material can appear in a snippet; a service credential is refused before it can reach one.

Routes: `/projects/{projectId}/environments/{environmentId}/{overview|data|collections|sync|users|policies|functions|storage|webhooks|auth-providers|email-templates|observability|logs|usage|activity|backups|connect|credentials|api-docs|settings}`, plus `…/storage/{bucketId}`, `…/webhooks/{webhookId}`, and per-resource pages for collections, policies, functions, and users. Every deep link opens inside the shell with its context shown.

### Where this is tested

`apps/console/test-e2e/*.spec.ts` (Playwright, against an intercepted management API) covers the shell, home dashboard, project home, management workflows, data workspace, storage, webhooks, auth providers, email templates, function schedules, allowed origins, custom domains, API docs, usage and activity, the registration wait list, and the operator control center. These suites prove console behavior; the smoke suites prove the server.

---

## The developer CLI

`mako-cloud` is the terminal counterpart of the console. Everything a developer can do in a browser — sign in, own teams and projects, shape collections and policies, issue keys, deploy functions, read logs, move data, manage file storage, register webhooks, serve on custom domains — can be done from a shell, a script, or CI with the same authorization and the same audit trail. It lives in `packages/cli` (`@mako-cloud/cli`) and drives the management API through `@mako-cloud/management-sdk`.

```bash
npm run build -w @mako-cloud/cli
node packages/cli/dist/main.js --help        # or, once linked: mako-cloud --help
```

Node 24 or newer is required. There are no other runtime dependencies.

### Parity with the console

The command tree is hand-written, but coverage is machine-checked: `packages/cli/test/parity.test.mjs` loads the OpenAPI document, takes every operation outside the excluded prefixes, and fails when one has no command or a command names an operation the API does not define. The exclusions are printed by the test so a new one is a visible decision: `/v1/operator*` (a separate identity), `/{projectRef}/functions` (invocation, an application concern), and the application-runtime routes under an environment (`auth/*`, `documents`, `service/*`, `replication/*`), which SDKs and RxDB clients call with project credentials. Every other operation maps to a command.

### Signing in

```bash
mako-cloud auth login --endpoint https://cloud-test.makodb.com     # prompts for email and password
mako-cloud auth status
mako-cloud auth whoami
mako-cloud auth logout                                             # revokes the session server-side first
```

Passwords are typed at a prompt with echo off or read from a file (`--password-file`); they are never accepted as an argument, so they never land in shell history or process listings. Registration, email verification, and password recovery are `mako-cloud auth register`, `verify-email`, `resend-verification`, `recover-password`, and `reset-password`.

The session is stored per **profile** in a credential file readable only by you: `$MAKO_CONFIG_DIR/credentials.json`, else `$XDG_CONFIG_HOME/mako-cloud/credentials.json`, else `~/.config/mako-cloud/credentials.json`. The directory is created `0700` and the file `0600`; a file other users can read is refused with exit code 7 rather than used. A profile records the endpoint, the access token, its expiry, your email, and the refresh cookie the API sets on sign-in — the only thing that can renew the session, which the CLI does automatically shortly before expiry. `--profile <name>` (or `MAKO_PROFILE`) keeps several sign-ins apart; a profile signed in to one endpoint is never sent to another.

Developer-auth calls (sign-in, refresh, sign-out, step-up) carry the endpoint as their `Origin` header because the API's cross-site check requires it; the CLI holds the refresh cookie deliberately, which is exactly what that check exists to prevent a foreign site from doing.

### Running in CI

Set `MAKO_TOKEN` and `MAKO_ENDPOINT`. The token is used as given and never written to disk. Automation tokens (`mako-cloud auth token create`) are the intended credential: they are team-scoped, optionally narrowed to one project or environment, and carry a fixed permission list — `project_read`, `project_write`, `environment_read`, `environment_write`, `collection_write`, `policy_write`, `function_deploy`, `audit_read`, `organization_read`. A command the token's permissions do not allow fails with exit code 3 and the API's message naming the permission (`automation token lacks the environment_read permission this request needs`), or saying the request lies outside the project or environment the token is scoped to. Commands read before they write: `functions deploy` reads the function first, so a deploy token needs `environment_read` as well as `function_deploy`. A developer session token can be supplied the same way with `MAKO_TOKEN_KIND=developer_session`.

Actions the console gates behind a fresh password (data explorer grants and other step-up actions) prompt on a terminal; without one, set `MAKO_STEP_UP_PASSWORD_FILE` to a file holding the password, or the command fails with exit code 3 and creates nothing.

### Output for people and for programs

On a terminal, lists render as tables and single resources as `key  value` lines; progress and notices go to stderr. `--json` prints the API's response verbatim (one document, or a page with `items` and `nextCursor`); `--all` follows cursors and prints every page as one document. Nothing reaches stdout that a script could not parse in JSON mode.

| Exit code | Meaning |
| --- | --- |
| 0 | success |
| 1 | the API refused or failed in a way not classified below |
| 2 | usage error, or a destructive command refused without confirmation |
| 3 | not signed in, credential rejected, or step-up not possible |
| 4 | not found |
| 5 | conflict or precondition refused (409, 412, 422) |
| 6 | `--wait` reached its deadline |
| 7 | the credential store is unsafe |

Errors print `error <code>: <message> | request <id>` and, when the API says so, `retry after <n>s` or `retryable`.

### Secrets and confirmations

Commands that issue a secret — public and service keys, automation tokens, function secrets, webhook signing secrets, invitation tokens — print it exactly once: as a delimited block on a terminal, as the `secret` field with `--json`, or into a file created `0600` with `--secret-file <path>`. It never appears in progress lines, errors, or the credential file.

Commands that delete, suspend, retire, revoke, activate a policy, promote or roll back a deployment, restore a backup, or mutate a document require `--yes` or, on a terminal, the resource's identifier typed back. Without either they refuse with exit code 2 before sending anything.

### Scoping

Environment-scoped commands take `--project <id>` (`-p`) and `--env <id>` (`-e`), or `MAKO_PROJECT_ID` and `MAKO_ENVIRONMENT_ID` from the environment. Identifiers that name the resource acted on are positional. Every command takes the global options `--endpoint`, `--profile`, `--config-dir`, `--json`, `--all`, `--yes`, `--wait`, `--timeout`.

### Composed flows

Some commands run several API calls and print each identifier they produce with the lower-level command that resumes from there, so a failed run is never a mystery:

- `mako-cloud projects create <name> --region <r> [--team <id>] --wait` polls until the project is active or failed (deadline `--timeout`, default 600 s; exit 6 on the deadline, after printing the id and the last observed state). Without `--team` the project lands in your personal space.
- `mako-cloud functions deploy <dir> --name <fn>` bundles the directory, uploads it, creates a deployment, runs the health check, and promotes it unless `--no-promote`. `--dependency <specifier>=<path>` maps a bare import onto an uploaded module; `--allow-host <name>` declares an external HTTPS host (repeatable, at most 8).
- `mako-cloud data export --collection <id> --output <file>` creates an export job, waits for it, obtains a download grant, and streams the artifact to the file. `mako-cloud data import --collection <id> --input <file>` obtains an upload grant, streams the file with its digest, runs the dry run, shows the result, and confirms only with `--yes` or a typed confirmation.
- `mako-cloud explorer …` issues a scoped explorer grant for the one call, performs it with the capability held in memory, and revokes the grant afterwards — also on failure. The capability is never printed or stored.
- `mako-cloud functions serve <dir>` runs a function locally in the pinned edge runtime ([details](#serving-a-function-locally)).
- `mako-cloud storage buckets create <id> [--access policy|public] [--max-object-bytes n] [--content-type t ...] [--rules <@file|-|json>]` declares a bucket; `update` sends only the options given; `delete` is refused while the bucket still holds objects unless `--delete-objects` confirms their loss. `mako-cloud storage objects list <id>` pages a bucket's objects by prefix and `objects delete <id> <path>` removes one.
- `mako-cloud email-templates list|get|set|reset|preview` manages the four application mails; `set <kind> --subject <text> --body <@file|-|text>` refuses an unknown `{{variable}}` or a stray brace with the API's message.
- `mako-cloud auth-settings get` shows an environment's providers (client ids and whether a secret is installed, never the secret), redirect allowlist, and magic-link settings; `mako-cloud auth-settings set --input <@file|-|json>` replaces them whole. A provider given without `clientSecret` keeps the installed one.
- `mako-cloud allowed-origins get` prints the origins one per line (`(none)` when empty); `mako-cloud allowed-origins set --origin <url>…` replaces the list whole and `--none` clears it. Both say on stderr that the management and operator APIs never answer cross-origin.
- `mako-cloud webhooks create --url <https-url> --subscribe <collection>[:<insert,update,delete>] ...` registers an endpoint; the signing secret is printed exactly once, after a warning on stderr, or written to `--secret-file`. `rotate-secret`, `update`, `resume`, `deliveries [--state …]`, and `redeliver` follow.
- `mako-cloud schedules create --function <name> --cron "<expr>" [--name <text>] [--method <m>] [--path </p>] [--header k=v ...] [--content-type <t>] [--body <@file|-|text>] [--disabled]` attaches a five-field UTC cron schedule to a deployed function; `update`, `run-now`, `runs [--outcome …]`, and `delete` follow.
- `mako-cloud domains add --hostname <name> --env <environment-id>` registers a hostname (here `--env` names the environment served, not the command's scope) and prints the TXT record to publish; `verify`, `list`, `get`, and `remove` follow.

### Hosted deployment note

The public beta's reverse proxy allows every management route by an explicit path allowlist and does not gate them on a browser origin; only the developer-auth routes require the `Origin` header the CLI sends. Nothing on the host changes for the CLI.

### Command reference

Generated from the command registry; every command also answers `--help` with its full option list. *(confirmed)* marks a command that requires `--yes` or a typed identifier; *(prints a secret once)* marks one whose output contains a secret shown exactly once.

#### `mako-cloud activity`

| command | does |
| ------- | ---- |
| `mako-cloud activity` | Audited actions in this environment, newest first |

#### `mako-cloud allowed-origins`

| command | does |
| ------- | ---- |
| `mako-cloud allowed-origins get` | Show the browser origins allowed to call this environment's application API |
| `mako-cloud allowed-origins set` | Replace the browser origins allowed to call this environment's application API; --none allows no cross-origin access |

#### `mako-cloud auth`

| command | does |
| ------- | ---- |
| `mako-cloud auth login` | Sign in with email and password and store the session for this profile |
| `mako-cloud auth logout` | Revoke the stored session server-side and remove it |
| `mako-cloud auth recover-password --email <email>` | Send a password recovery mail |
| `mako-cloud auth register` | Register a developer account with the hosted registration flow |
| `mako-cloud auth resend-verification --email <email>` | Send the verification mail again |
| `mako-cloud auth reset-password <token>` | Set a new password with the token from the recovery mail |
| `mako-cloud auth status` | Show which credential and endpoint commands will use |
| `mako-cloud auth token create --team <team-id> --name <name> --permission <permission> --expires-in <duration>` | Issue an automation token for a team; its secret is shown once *(prints a secret once)* |
| `mako-cloud auth token list --team <team-id>` | List a team's automation tokens |
| `mako-cloud auth token revoke <token-id> --team <team-id>` | Revoke an automation token; automation using it loses access immediately *(confirmed)* |
| `mako-cloud auth token rotate <token-id> --team <team-id> --replacement-id <token-id> --expires-in <duration>` | Replace an automation token with a new one; the old token stops working *(confirmed, prints a secret once)* |
| `mako-cloud auth verify-email <token>` | Confirm an email address with the token from the verification mail |
| `mako-cloud auth waitlist-status` | Show whether a registration is still on the wait-list |
| `mako-cloud auth whoami` | Show the signed-in developer, their personal space, and teams |

#### `mako-cloud auth-settings`

| command | does |
| ------- | ---- |
| `mako-cloud auth-settings get` | Show the environment's sign-in providers (without secrets), redirect allowlist, and magic-link settings |
| `mako-cloud auth-settings set --input <@file\|-\|json>` | Replace the environment's sign-in settings from a JSON document; a provider without clientSecret keeps the installed one |

#### `mako-cloud backups`

| command | does |
| ------- | ---- |
| `mako-cloud backups list` | Verified recovery points for an environment |
| `mako-cloud backups restore-requests create --project <project-id> --input <@file\|-\|json>` | Restore a verified backup into a new isolated environment (step-up verified) *(confirmed)* |
| `mako-cloud backups restore-requests list` | Restore requests for a project and their verification state |

#### `mako-cloud collections`

| command | does |
| ------- | ---- |
| `mako-cloud collections create <collection-id> --schema <@file\|-\|json>` | Create a collection with its first schema (`--primary-key <field>` unless the schema carries `primaryKey`; `--schema-version`, default 1) |
| `mako-cloud collections get <collection-id>` | Show a collection, its active schema, and its state |
| `mako-cloud collections list` | List the environment's collections and their active schema versions |
| `mako-cloud collections migrations create <collection-id> --input <@file\|-\|json>` | Plan a schema migration to a target schema version |
| `mako-cloud collections migrations get <collection-id> <migration-id>` | Show a schema migration and its state |
| `mako-cloud collections migrations update <collection-id> <migration-id> --state <state>` | Move a schema migration to another state |
| `mako-cloud collections schema publish <collection-id> --schema <@file\|-\|json> --schema-version <n>` | Publish a new schema version; reports migration_required when documents do not fit |

#### `mako-cloud data`

| command | does |
| ------- | ---- |
| `mako-cloud data export --output <path\|->` | Export a collection snapshot as JSON Lines: create the job, wait, download |
| `mako-cloud data import` | Import JSON Lines: upload with its digest, dry-run, then confirm |
| `mako-cloud data jobs cancel <job-id>` | Cancel a queued or running data job (committed rows are kept) *(confirmed)* |
| `mako-cloud data jobs get <job-id>` | Show one data job; --wait polls it to a terminal state |
| `mako-cloud data jobs list` | List import and export jobs of an environment |

#### `mako-cloud domains`

| command | does |
| ------- | ---- |
| `mako-cloud domains add` | Add a custom domain that serves one environment's API and functions; prints the TXT record to publish |
| `mako-cloud domains get <domain-id>` | Show a custom domain, its DNS verification record, and why the last check failed |
| `mako-cloud domains list` | List the project's custom domains with the environment each serves, its state, and the last check |
| `mako-cloud domains remove <domain-id>` | Remove a custom domain; serving on its name stops and its certificate is no longer renewed *(confirmed)* |
| `mako-cloud domains verify <domain-id>` | Check a domain's DNS record now instead of at the next periodic check and show the outcome |

#### `mako-cloud email-templates`

| command | does |
| ------- | ---- |
| `mako-cloud email-templates get <kind>` | Show one email template's subject and body as they are in effect |
| `mako-cloud email-templates list` | List the environment's application email templates; defaults are marked until customized |
| `mako-cloud email-templates preview <kind>` | Render an email template with placeholder data; --subject or --body previews unsaved text |
| `mako-cloud email-templates reset <kind>` | Reset an email template to the built-in default |
| `mako-cloud email-templates set <kind>` | Customize an email template's subject and plain-text body; unknown {{variables}} are refused |

#### `mako-cloud envs`

| command | does |
| ------- | ---- |
| `mako-cloud envs create <name>` | Create an environment in a project (--wait for provisioning) |
| `mako-cloud envs delete <env-id>` | Start an environment's deletion grace period *(confirmed)* |
| `mako-cloud envs get <env-id>` | Show an environment |
| `mako-cloud envs list` | List a project's environments |
| `mako-cloud envs promote --from <env-id> --to <env-id>` | Copy collections, indexes, policies, and buckets from one environment to another; plans unless `--apply` |
| `mako-cloud envs restore <env-id>` | Restore a suspended environment or one in its deletion grace period |
| `mako-cloud envs suspend <env-id>` | Suspend an environment; it stops serving *(confirmed)* |

#### `mako-cloud explorer`

| command | does |
| ------- | ---- |
| `mako-cloud explorer browse <collection-id>` | Page through a collection in primary-key order over a stable snapshot |
| `mako-cloud explorer get <collection-id> <document-id>` | Read one document under a short-lived explorer grant |
| `mako-cloud explorer history <collection-id> <document-id>` | List the retained revisions and tombstones of one document |
| `mako-cloud explorer mutate <collection-id> --mutation <@file\|-\|json>` | Commit a create, update, or delete with administrative access *(confirmed)* |
| `mako-cloud explorer plan <collection-id> --query <@file\|-\|json>` | Show which index a query would use, or the index it needs |
| `mako-cloud explorer query <collection-id> --query <@file\|-\|json>` | Run an indexed query and page through its results |
| `mako-cloud explorer simulate <collection-id> --mutation <@file\|-\|json>` | Validate a mutation against the schema, revision, and policy without committing |

#### `mako-cloud functions`

| command | does |
| ------- | ---- |
| `mako-cloud functions create <name>` | Create a function with its configuration (`--region` required; `--no-verify-jwt`, `--secret`, `--allow-host`, `--cpu-ms`, `--wall-ms`, `--memory-bytes`, `--request-bytes`, `--response-bytes`, `--concurrency`); deploy code with `functions deploy` |
| `mako-cloud functions delete <name>` | Delete a function and every deployment it has *(confirmed)* |
| `mako-cloud functions deploy <directory> --name <function-name>` | Upload a function directory, create a version, check its health, and promote it (promotion needs --yes or a typed confirmation) |
| `mako-cloud functions deployments create <name> --bundle <digest>` | Create a deployment version from an uploaded bundle digest |
| `mako-cloud functions deployments delete <name> <version>` | Delete a deployment version that is not active *(confirmed)* |
| `mako-cloud functions deployments get <name> <version>` | Show one deployment version |
| `mako-cloud functions deployments health <name> <version>` | Run the health check on a deployment version and record the outcome |
| `mako-cloud functions deployments list <name>` | List a function's immutable deployment versions |
| `mako-cloud functions deployments promote <name> <version>` | Make a healthy deployment version the active one *(confirmed)* |
| `mako-cloud functions deployments rollback <name> <version>` | Switch the active version back to a previously healthy one *(confirmed)* |
| `mako-cloud functions get <name>` | Show a function, its configuration, and its active version |
| `mako-cloud functions list` | List the functions in an environment |
| `mako-cloud functions logs <name>` | Read a function's sanitized logs, newest page first |
| `mako-cloud functions secrets create <name>` | Create a function secret: generated and shown once, or stored from `--value <v>` / `--value-file <path>` (a service credential, say) and never shown *(prints a secret once)* |
| `mako-cloud functions secrets get <name>` | Show a function secret's version and state, never its value |
| `mako-cloud functions secrets retire <name>` | Retire a function secret so no new deployment can attach it *(confirmed)* |
| `mako-cloud functions secrets rotate <name>` | Rotate a function secret and show the new value once *(confirmed, prints a secret once)* |
| `mako-cloud functions serve` | Run a function locally in the pinned edge runtime |
| `mako-cloud functions test <name>` | Invoke a function through the management test route and show the response |
| `mako-cloud functions update <name> --config <json>` | Replace a function's configuration |

#### `mako-cloud indexes`

| command | does |
| ------- | ---- |
| `mako-cloud indexes create <collection-id> --name <name> --version <n> --field <path[:ascending\|descending]>… [--unique]` | Create an index; it is built in the background and reported active when ready |
| `mako-cloud indexes delete <collection-id> <name> <version>` | Delete one index version *(confirmed)* |
| `mako-cloud indexes get <collection-id> <name> <version>` | Show one index version, its fields, progress, and any failure |
| `mako-cloud indexes list <collection-id>` | List a collection's indexes and their build state |

#### `mako-cloud keys`

| command | does |
| ------- | ---- |
| `mako-cloud keys get <credential-id>` | Show a credential's kind, scope, and state (never its secret) |
| `mako-cloud keys public create` | Issue a public key for client apps; the secret is shown once *(prints a secret once)* |
| `mako-cloud keys retire <credential-id>` | Retire a credential; requests signed with it are refused from then on *(confirmed)* |
| `mako-cloud keys rotate <credential-id> --replacement-id <credential-id> --overlap <seconds>` | Issue a replacement credential and retire this one after the overlap *(confirmed, prints a secret once)* |
| `mako-cloud keys service create --collection <id>… --operation <op>…` | Issue a service credential scoped to collections and operations; shown once *(prints a secret once)* |
| `mako-cloud keys signing init` | Create the environment's first JWT signing key |
| `mako-cloud keys signing list` | List the environment's JWT signing keys and their states |
| `mako-cloud keys signing rotate --overlap <seconds>` | Rotate the JWT signing key; the old key verifies tokens until the overlap ends *(confirmed)* |

#### `mako-cloud logs`

| command | does |
| ------- | ---- |
| `mako-cloud logs` | Retained, scrubbed log lines from functions, the data plane, and sync (newest first) |

#### `mako-cloud observability`

| command | does |
| ------- | ---- |
| `mako-cloud observability audit` | Append-only administration history for this environment, newest first |
| `mako-cloud observability auth-events` | Sanitized application authentication outcomes, without credentials |
| `mako-cloud observability function-metrics` | Invocation, error, latency, and compute counts per function version and region |
| `mako-cloud observability health` | Regional data-plane service status and sanitized diagnostics |
| `mako-cloud observability index-state` | Index build state events per collection index, newest first |
| `mako-cloud observability logs` | Retained, scrubbed log lines from functions, the data plane, and sync (newest first) |
| `mako-cloud observability quotas` | Consumption against enforced limits, with the retry time when work was throttled |
| `mako-cloud observability replication-errors` | RxDB replication failures with retry guidance and correlation identifiers |
| `mako-cloud observability usage` | Retained usage samples per resource (flows sum their records; levels average their samples) |

#### `mako-cloud policies`

| command | does |
| ------- | ---- |
| `mako-cloud policies activate <collection-id> <version>` | Activate a policy version; every client is authorized by it from then on *(confirmed)* |
| `mako-cloud policies draft <collection-id> --input <@file\|-\|json>` | Create a policy draft from {version, rules} |
| `mako-cloud policies get <collection-id>` | Show the active policy of a collection, or one version with --version |
| `mako-cloud policies rollback <collection-id> <version>` | Make an earlier policy version active again *(confirmed)* |
| `mako-cloud policies test <collection-id> <version> --examples <@file\|-\|json>` | Evaluate a policy version against example requests |
| `mako-cloud policies validate <collection-id> <version>` | Validate a policy version and report its diagnostics |

#### `mako-cloud projects`

| command | does |
| ------- | ---- |
| `mako-cloud projects create <name> --region <region>` | Create a project in your personal space or a team (--wait for provisioning) |
| `mako-cloud projects delete <project-id>` | Start a project's deletion grace period *(confirmed)* |
| `mako-cloud projects get <project-id>` | Show a project |
| `mako-cloud projects list` | List projects in one team, or in every team you belong to |
| `mako-cloud projects rename <project-id> <name>` | Rename a project |
| `mako-cloud projects restore <project-id>` | Restore a suspended project or one in its deletion grace period |
| `mako-cloud projects suspend <project-id>` | Suspend a project; its environments stop serving *(confirmed)* |
| `mako-cloud projects transfer <project-id>` | Move a project to a team you administer, or to your personal space *(confirmed)* |

#### `mako-cloud schedules`

| command | does |
| ------- | ---- |
| `mako-cloud schedules create --function <name>` | Attach a UTC cron schedule to a deployed function; an invalid expression is refused at once |
| `mako-cloud schedules delete <schedule-id> --function <name>` | Remove a schedule and its run history *(confirmed)* |
| `mako-cloud schedules get <schedule-id> --function <name>` | Show a schedule, the request it sends, its next run, and its last run |
| `mako-cloud schedules list --function <name>` | List a function's cron schedules with their state, next run, and last run |
| `mako-cloud schedules run-now <schedule-id> --function <name>` | Queue one run of a schedule outside its cron times, recorded as manual; refused while a run is executing |
| `mako-cloud schedules runs <schedule-id> --function <name>` | List a schedule's runs, newest first, including ones skipped for overlap; --all follows the cursor to the end |
| `mako-cloud schedules update <schedule-id> --function <name>` | Change a schedule's expression, name, request, or enabled flag; only given options are sent |

#### `mako-cloud storage`

| command | does |
| ------- | ---- |
| `mako-cloud storage buckets create <bucket-id>` | Create a storage bucket; without rules a policy bucket refuses every request |
| `mako-cloud storage buckets delete <bucket-id>` | Delete a storage bucket; one that still holds objects is refused unless --delete-objects confirms their loss *(confirmed)* |
| `mako-cloud storage buckets get <bucket-id>` | Show a storage bucket, its limits, its rules, and its totals |
| `mako-cloud storage buckets list` | List the environment's storage buckets with their object counts and totals |
| `mako-cloud storage buckets update <bucket-id>` | Change a storage bucket's access, limits, content types, or rules; only given options are sent |
| `mako-cloud storage objects delete <bucket-id> <path>` | Delete one object from a bucket by its path *(confirmed)* |
| `mako-cloud storage objects list <bucket-id>` | List a bucket's objects in path order; --all follows the cursor to the end |

#### `mako-cloud sync`

| command | does |
| ------- | ---- |
| `mako-cloud sync summary` | Aggregate RxDB synchronization diagnostics for a window (default: the last hour) |

#### `mako-cloud teams`

| command | does |
| ------- | ---- |
| `mako-cloud projects bill <project-id>` | Show one project's current usage and allocated costs |
| `mako-cloud teams activity <team-id>` | A team's own audit trail: invitations, member and role changes, automation tokens (newest first) |
| `mako-cloud teams bill <team-id>` | Show a team's bill for the current month or a closed period |
| `mako-cloud teams create <name>` | Create a team |
| `mako-cloud teams delete <team-id>` | Start a team's deletion grace period; its projects lose access immediately *(confirmed)* |
| `mako-cloud teams get <team-id>` | Show a team |
| `mako-cloud teams invitations accept <invitation-id> <token>` | Join a team with an invitation id and its token |
| `mako-cloud teams invitations create <team-id> --email <email> --role <role>` | Invite a developer to a team; the invitation token is shown once *(prints a secret once)* |
| `mako-cloud teams list` | List the teams you belong to, including your personal space |
| `mako-cloud teams members list <team-id>` | List a team's members and their roles |
| `mako-cloud teams members remove <team-id> <developer-id>` | Remove a member from a team *(confirmed)* |
| `mako-cloud teams members update <team-id> <developer-id> --role <role>` | Change a member's role |
| `mako-cloud teams rename <team-id> <name>` | Rename a team |
| `mako-cloud teams restore <team-id>` | Restore a team from its deletion grace period |

#### `mako-cloud usage`

| command | does |
| ------- | ---- |
| `mako-cloud usage` | Retained usage samples per resource (flows sum their records; levels average their samples) |

#### `mako-cloud users`

| command | does |
| ------- | ---- |
| `mako-cloud users create --email <email>` | Create an application user directly, without an invitation |
| `mako-cloud users delete <user-id>` | Delete a user *(confirmed)* |
| `mako-cloud users disable <user-id>` | Disable a user; their sessions stop working until restored *(confirmed)* |
| `mako-cloud users get <user-id>` | Show an application user, their metadata, and their sessions |
| `mako-cloud users invite --email <email>` | Invite an application user; they finish signing up themselves |
| `mako-cloud users restore <user-id>` | Restore a disabled user |
| `mako-cloud users revoke-session <user-id> <session-id>` | Revoke one session of a user *(confirmed)* |
| `mako-cloud users revoke-sessions <user-id>` | Revoke every session of a user *(confirmed)* |
| `mako-cloud users search` | Search application users by id or email (bounded; no credential material) |
| `mako-cloud users update-metadata <user-id> --input <@file\|-\|json>` | Replace a user's trusted and profile metadata |

#### `mako-cloud webhooks`

| command | does |
| ------- | ---- |
| `mako-cloud webhooks create` | Register a webhook endpoint for collection events; the signing secret is shown once *(prints a secret once)* |
| `mako-cloud webhooks delete <webhook-id>` | Remove a webhook endpoint with its subscriptions and delivery log *(confirmed)* |
| `mako-cloud webhooks deliveries <webhook-id>` | List a webhook endpoint's recent deliveries, newest first; --all follows the cursor to the end |
| `mako-cloud webhooks get <webhook-id>` | Show a webhook endpoint, its subscriptions, and why it is paused if it is |
| `mako-cloud webhooks list` | List the environment's webhook endpoints with their state and failure counts |
| `mako-cloud webhooks redeliver <webhook-id> <delivery-id>` | Queue a new signed delivery of one event, logged as a redelivery of the original |
| `mako-cloud webhooks resume <webhook-id>` | Resume a webhook endpoint the platform paused after sustained failure |
| `mako-cloud webhooks rotate-secret <webhook-id>` | Replace a webhook endpoint's signing secret; the new one is shown once and the old one stops signing at once *(confirmed, prints a secret once)* |
| `mako-cloud webhooks update <webhook-id>` | Change a webhook endpoint's URL, subscriptions, description, or enabled flag; only given options are sent |

#### `mako-cloud workspace`

| command | does |
| ------- | ---- |
| `mako-cloud workspace check` | Probe DNS, TLS, routes, key, schema, and replication without reading documents |
| `mako-cloud workspace connect` | Public RxDB connection metadata: endpoint, active public key id, compatibility |
| `mako-cloud workspace nav` | Workspace destinations and whether your memberships permit each |
| `mako-cloud workspace summary` | Overview sections for an environment, each with its own freshness |

### Where this is tested

`npm run test:unit -w @mako-cloud/cli` runs the command groups against a loopback mock of the management API (`packages/cli/test/harness.mjs`) with assertions on requests, headers, output, exit codes, and secret handling, plus the parity test. `cargo test -p mako-smoke --test developer_cli` boots the local data and control planes, mints a developer session, and drives the built CLI through a console workflow end to end; it skips with a notice when `packages/cli/dist` has not been built.

---

## Teams, projects, and environments

### Teams and membership

```bash
mako-cloud teams create "Acme"
mako-cloud teams invitations create org_… --email colleague@example.com --role developer   # token shown once
mako-cloud teams invitations accept inv_… <token>                                       # run by the invitee
mako-cloud teams members list org_…
mako-cloud teams members update org_… dev_… --role administrator
mako-cloud teams members remove org_… dev_… --yes
```

In the console, Home's Teams section has **New team**. An invitation is handed out as a link, `/invitations/{id}#token=…`, shown once: the token rides in the fragment, which never reaches a server, and the teammate opens the link, signs in if they need to, and accepts. The member list marks your own row "(you)"; only an owner may change or remove an owner.

### Team activity

A team's own audit trail -- changes to the team, invitations, membership and role changes, automation tokens issued, rotated, and revoked, and project-level actions -- is kept apart from any environment's audit events. Any member may read it: the team page's **Activity** panel shows it newest first, `GET /v1/teams/{teamId}/activity` (`listTeamActivity`) answers it, and `mako-cloud teams activity <team-id>` prints it. With `changes=true` (`--changes`, the panel's default) events that only read something are left out; every visit to a team records a membership read. Each event's `details` names what was acted on, such as `target=dev_…`, when that is a plain identifier.

```bash
mako-cloud teams activity org_… --changes --since 7d
```

Roles: `owner`, `administrator`, `developer`, `viewer`. Deleting a team starts a grace period (`mako-cloud teams delete`, restorable with `mako-cloud teams restore`); its projects lose access immediately. Your personal space (`kind: personal`) appears in `mako-cloud teams list` and refuses invitations, membership changes, and deletion.

### Automation tokens

```bash
mako-cloud auth token create --team org_… --name ci-deploy --permission function_deploy --permission project_read --expires-in 30d
mako-cloud auth token list --team org_…
mako-cloud auth token rotate atm_… --team org_… --replacement-id atm_next --expires-in 30d --yes
mako-cloud auth token revoke atm_… --team org_… --yes
```

A token may be narrowed to one project or environment at creation (`AutomationScope.projectId` / `environmentId` over the API). Its secret is shown once.

### Projects

```bash
mako-cloud projects create "Todos" --region <region> [--team org_…] --wait
mako-cloud projects list [--team org_…]
mako-cloud projects rename prj_… "Todos v2"
mako-cloud projects transfer prj_… --team org_…      # or to your personal space
mako-cloud projects suspend prj_… --yes             # its environments stop serving
mako-cloud projects restore prj_…
mako-cloud projects delete prj_… --yes              # starts the grace period; restore undoes it before the deadline
```

Provisioning is asynchronous and idempotent: a project shows `provisioning` until every step completes, then `active`; a step that keeps failing leaves `failed` with a `failureDiagnostic`. The platform's own worker sweeps records stranded in provisioning and completes or compensates them. Plan limits are installed on every environment at activation and reinstalled on transfer to the new owner's plan.

### Environments

```bash
mako-cloud envs create development -p prj_… --wait
mako-cloud envs list -p prj_…
mako-cloud envs suspend env_… -p prj_… --yes
mako-cloud envs restore env_… -p prj_…
mako-cloud envs delete env_… -p prj_… --yes
```

Each environment has its own collections, policies, users, credentials, signing keys, functions, buckets, webhooks, schedules, sign-in settings, email templates, and allowed origins. Nothing is shared between environments of one project except the project's custom domains, each of which serves exactly one environment.

### Promoting an environment

What you build in Development -- collection schemas, their indexes, each collection's active policy, and storage bucket settings -- is brought to another environment with `envs promote`. It prints a plan and changes nothing until you add `--apply`:

```bash
mako-cloud envs promote -p prj_… --from env_dev… --to env_prod…                     # the plan
mako-cloud envs promote -p prj_… --from env_dev… --to env_prod… --collection todos  # only what you name
mako-cloud envs promote -p prj_… --from env_dev… --to env_prod… --apply             # make the changes
```

A missing collection is created at the source's schema version; an older one gets the source's schema published over it, which succeeds only when it is compatible -- otherwise the step fails with the issues and the target needs a [migration](#publishing-a-new-schema-version). A policy is activated in the target as a new version above both sides. A target that is ahead of the source is left alone and reported. Data, secrets, keys, webhooks, sign-in settings, allowed origins, and domains are never copied: they belong to each environment. What the target lacks of them is reported, though, each with the command that sets it: a signing key, sign-in settings that differ (email verification, magic links, redirect URLs, providers), allowed origins, customized email templates, webhook endpoints, and the secrets and schedules the source's functions need. A plan limited with `--collection` or `--bucket` leaves these out. Function code cannot be read back from an environment, so a function that differs is reported with the `functions deploy` command that brings it over. Running the plan again after `--apply` shows what is still different; nothing, once the target matches.

### Audit

Every management mutation is recorded in the control audit log with actor, target, reason where one was given, result, request id, and correlation — never a password hash, a token, or a document body. That includes the changes the data plane carries out for the control plane -- project keys, signing keys, buckets and their objects, sign-in settings, application users -- and every such change refused for the caller's role, recorded as `denied`. An action made with an automation token keeps the issuing developer as its actor and names the token in its details as `via=atm_…`, so a CI deploy is never mistaken for the person acting directly. Read it with `mako-cloud activity` / `mako-cloud observability audit` or in the console's **Activity**.

---

## Collections, schemas, and indexes

### What a collection is

A collection is a named set of JSON documents with a **versioned schema** and a **primary key**. Documents are validated against the active schema on every write, whether it arrives through the document API, an RxDB push, an edge function, an import job, or the data explorer.

```json
{
  "id": "todos",
  "schemaVersion": 1,
  "primaryKey": { "kind": "field", "field": "id" },
  "jsonSchema": { "type": "object", "required": ["id", "ownerId", "title"], "properties": { "…": {} } }
}
```

- `id` is the collection identifier used in every route and in RxDB configuration.
- `jsonSchema` is a JSON Schema object describing the document body. Fields the schema names are what policies may reference (`old.<field>`, `new.<field>`); an unknown field in a policy expression is a compile error, and a document that does not validate is refused with `invalid_request`.
- `primaryKey` is either `{ "kind": "field", "field": "id" }` or a composite `{ "kind": "composite", "key": "id", "fields": ["household_id", "slug"], "separator": "." }`, which mirrors RxDB's composite primary keys. The route's `{documentId}` is compared with this key after percent-decoding, so an id containing `:`, `@`, `/`, or a space is escaped in the path exactly as `encodeURIComponent` does; a malformed escape is refused.

A collection's ids are one namespace for the whole environment. An application that seeds deterministic documents from several devices should therefore derive ids that are both deterministic and distinct per tenant of the application (the Rational sample uses `grp_<household>.<slug>`).

### Publishing a new schema version

```bash
mako-cloud collections schema publish todos --schema @todos.v2.json --schema-version 2 --primary-key id
```

Publication is **compatibility-first**. The platform checks existing documents against the new schema and answers with a `CollectionCompatibilityReport` (`compatible`, `documentsChecked`, up to 100 `issues`). A compatible publication activates at once; an incompatible one stays inactive and returns `migration_required`, leaving the last compatible schema active. Nothing is ever downgraded destructively: fields are not discarded and stored documents are not rewritten merely to lower a version.

For an incompatible change, plan a **schema migration** (`mako-cloud collections migrations create <collection> --input @plan.json`, then `get` and `update --state …` to move it through its states). A migration moves `planned` → `running` → `completed` (or `failed`/`cancelled`). While it runs, bring the stored documents to the new shape through the current version — a backfill of a new required field, say; nothing rewrites them for you. Moving it to `completed` checks every stored document against the target schema: if all satisfy it, the target version becomes active; if not, the answer is `409 conflict` with `documentsChecked` and up to 20 `failingDocuments` in its details, the migration stays `running`, and the current version stays active. A migration cannot change the primary key, and applies only to the version it was planned from. Replicating clients learn about a new required version through `schema_mismatch` on pull or push and, with `@mako-cloud/rxdb`, through `MakoReplicationRecoveryCoordinator.onSchemaMigrationRequired` ([details](#schema-migration-and-full-resync)). Additive changes — a new optional field — are compatible and need no *data* migration: publishing one makes it active at once. Replication is still bound to an exact version, though, so from that moment every app replicating the collection at the previous version is refused with `schema_mismatch` until it moves to the new one (with `@mako-cloud/rxdb`, run the RxDB schema migration and create replication for that version). Changes an app had not synced stay on the device and are sent once it has moved. Plan a publish together with the app release that adopts it. The Rational sample publishes its schema version 3 additively over a live version 2.

### Indexes

There is deliberately **no collection scan**. Every query — from the document API, an edge function, the data explorer, or an import's conflict check — must be served by an active index whose leading fields match the query's equality predicates and sort order; otherwise the server answers with the minimal index shape the query needs rather than scanning.

```bash
mako-cloud indexes create transactions --name by_household_date --version 1 \
  --field household_id --field date:descending
mako-cloud indexes list transactions
mako-cloud indexes get transactions by_household_date 1
mako-cloud indexes delete transactions by_household_date 1 --yes
```

- An index has a `name`, a `version`, a `kind` (`non_unique` or `unique`), and ordered `fields`, each `path` with `ascending` or `descending` direction. Repeat `--field` in key order.
- Indexes build **in the background**: `state` moves `building` → `active`, with `progress` reporting the captured commit position, the last backfilled document, whether backfill is complete, and the caught-up position. Activation is fenced so a build never serves before it has caught up with concurrent writes.
- A `unique` build that finds duplicate values `failed` with `duplicate_values` and the count of affected values; a backfill error is `backfill_failed`. Delete the failed version and create another once the data is corrected.
- Index state events (`mako-cloud observability index-state`) show each transition.

A range over an index's leading field is how a job enumerates a collection: a query with no predicate is refused by design, so a scheduled function that must walk every document of a household queries `household_id == X` sorted by the index's second field and follows the cursor.

### The query shape

Queries share one shape everywhere (the document API, the edge SDK, the explorer):

```json
{
  "predicates": [ { "field": "household_id", "operator": "eq", "value": "hh_1" },
                  { "field": "date", "operator": "gte", "value": 20260101 } ],
  "sort": [ { "field": "date", "direction": "desc" } ],
  "cursor": null,
  "limit": 200
}
```

At least one and at most 16 predicates (`eq`, `gt`, `gte`, `lt`, `lte` over a string, number, boolean, or `null`), at most 16 sort fields, `limit` 1–1000, and an opaque `cursor` to continue. Results are policy-filtered: a document the caller may not read is never returned and never counted.

### Where this is tested

`crates/mako-documents` unit tests and `crates/mako-documents/tests/document_invariants.rs` cover schema validation, compatibility publication, index backfill and fencing, uniqueness failure, and query planning. `packages/cli/test/database.test.mjs` covers the CLI commands.

---

## Document policies

Every collection is **default deny**. Create, read, update, and delete each require a matching active **allow** rule, and any matching **deny** rule wins. Policies are compiled against the collection schema into a deterministic, bounded evaluator with no network, wall-clock, or arbitrary-code access, and the *same* compiled decision governs every path: point reads, indexed queries, replication pull, live delivery, push conflict responses, caller-aware edge SDK calls, and authorized support access.

### The shape of a policy

```json
{
  "version": 3,
  "rules": [
    { "id": "members-read",   "effect": "allow", "operations": ["read"],
      "expression": "claims.households[old.household_id] != null" },
    { "id": "editors-create", "effect": "allow", "operations": ["create"],
      "expression": "claims.households[new.household_id] == \"owner\" || claims.households[new.household_id] == \"editor\"" },
    { "id": "editors-update", "effect": "allow", "operations": ["update"],
      "expression": "old.household_id == new.household_id && (claims.households[new.household_id] == \"owner\" || claims.households[new.household_id] == \"editor\")" },
    { "id": "owners-delete",  "effect": "allow", "operations": ["delete"],
      "expression": "claims.households[old.household_id] == \"owner\"" },
    { "id": "never-hidden-imports", "effect": "deny", "operations": ["delete"],
      "expression": "old.kind == \"import_batch\"" }
  ]
}
```

Each rule has a stable `id`, an `effect`, one or more `operations`, and one boolean `expression` (at most 16 KiB). A policy set has a `version` and a `state`: `draft`, `validated`, `active`, or `retired`.

### Evaluation context

Rules may use the verified user id, role, trusted claims, project and environment, operation, safe request metadata, the prior document, and the proposed document. They must never treat user-editable profile metadata as trusted authorization input — and they cannot, because it is not in the context.

- **Create** evaluates the proposed state (`new.*`).
- **Delete** evaluates the prior state (`old.*`).
- **Update** evaluates both states and rechecks the current revision in the same conditional transaction that commits the write.
- **Read** applies consistently to point reads, indexed queries, pull, live delivery, readable conflicts, caller-aware edge SDK calls, and authorized support access.

Protected document bodies never appear in denial details, unreadable conflicts, logs, counts, or index diagnostics.

### Expression language

A rule is one boolean expression over the context. Paths name context values:

| Path | Meaning |
| --- | --- |
| `identity.user_id` | The verified application user id |
| `identity.role` | The user's role, from administrator-controlled trusted metadata |
| `identity.email`, `identity.email_verified` | The address the session authenticated as (lower-cased; `null` when none) and whether the environment confirmed it |
| `claims.<name>` | A trusted claim from the user's app metadata; dynamic, `null` when absent |
| `request.<name>` | Safe request metadata |
| `operation` | `create`, `read`, `update`, or `delete` |
| `project_id`, `environment_id`, `collection_id` | The tenant and collection being decided |
| `old.<field>`, `new.<field>` | Fields of the prior and proposed document, typed by the schema |

Literals are JSON strings, numbers, `true`, `false`, and `null`. Operators are `==`, `!=`, `<`, `<=`, `>`, `>=`, `&&`, `||`, `!`, and parentheses. Document fields take their type from the collection schema and an unknown field is a compile error; trusted claims are dynamic because the schema does not describe them, so a claim compares with any operand and resolves to `null` when absent.

#### Scoping a document to an address

`identity.email` and `identity.email_verified` are separate because they answer different questions, and a rule that hands a document to an address needs both:

```text
old.invitee_email == identity.email && identity.email_verified
```

Without the second term the rule hands the document to whoever registered the address first, which anyone may do in an environment that does not require verification. Both are verified token claims set when the session was issued; profile metadata cannot reach them.

#### Indexing trusted claims

`claims.<name>[<expression>]` looks a trusted claim up by a value computed at evaluation time, so a rule can select the claim entry that belongs to the document it is deciding on. Indexes chain: `claims.a[new.x][new.y]`.

- Only `claims.*` paths may be indexed. Indexing `identity`, `request`, `old`, or `new` is the compile error `index_not_allowed`.
- The index expression must be a string, a number, or another trusted claim. A boolean, `null`, array, or object index is `index_type_invalid`; a missing `]` is a `syntax_error` ("expected closing bracket").
- An object indexed by a string yields the member and an array indexed by a non-negative integer yields the element. Everything else yields `null`: an absent member, an out-of-range, negative, or fractional position, and a claim that is a scalar. Indexing never fails evaluation, so a missing membership simply fails the comparison it feeds.
- The result is dynamic and compares like any other claim, including `!= null` to test that an entry exists.
- Each index costs one node of the bounded evaluation budget, and its index expression is evaluated once.

An application that keeps each member's role per household in the trusted claims as `households: { "<household_id>": "owner" | "editor" | "viewer" }` allows a create when the caller holds a writing role for the document's household, and a read or delete to any member with `claims.households[old.household_id] != null`. Membership lives in the claims the token carries, so a change takes effect on the next token and advances the authorization epoch like any other trusted-claim change ([how a function sets those claims](#trusted-metadata-from-an-applications-function)).

### Policy lifecycle

```bash
mako-cloud policies draft todos --input @policy.json      # immutable draft, {version, rules}
mako-cloud policies validate todos 3                       # compile against the active schema; diagnostics
mako-cloud policies test todos 3 --examples @examples.json # evaluate representative requests
mako-cloud policies activate todos 3 --yes                 # atomic; advances the environment authorization epoch once
mako-cloud policies get todos [--version 2]
mako-cloud policies rollback todos 2 --yes                 # an activation of an earlier validated version
```

Create an immutable draft, validate syntax, types, and cost against the active schema, run representative examples, and atomically activate the complete version. Failed validation or activation leaves the current policy unchanged. Rollback selects a previously validated immutable version through the normal audited action; it is an activation, so it also advances the authorization epoch and requires client resets where visibility may have changed. Never edit an active policy version in place — there is no route that can.

A policy **example** names an `operation`, an `identity` (`userId`, `role`, `email`, `emailVerified`, `trustedClaims`), and optionally `oldDocument`, `newDocument`, and `requestMetadata`; the test route answers with each example's decision, so a policy can be checked against the cases that matter before anyone is authorized by it.

### Visibility and local data

A visible-to-hidden document change sends replicating clients a **synthetic tombstone** containing only the safe replication identity; hidden-to-visible sends the new state. Policy or trusted-claim changes increment authorization epochs. The RxDB client then pauses, securely clears affected replicated state, notifies the application, and starts a new replication generation before rendering data again ([details](#authorization-epoch-security-reset)).

### Privileged access

The default edge document client uses the caller's policies. Bypass requires an explicit scoped **service credential** or a time-bounded operator grant, the exact tenant, collection, and operation, a reason, and a successful durable audit append. If the audit record cannot be written, the bypass does not happen.

### Where this is tested

`npm run test:policy-security` covers differential decisions, old/new visibility, epoch invalidation, conflict non-disclosure, privileged bypass, and caller-aware edge access; the latest qualification is summarized in the [Dev Book](dev-book.md#policy-security-qualification). `crates/mako-policy` holds the compiler and evaluator tests, including every diagnostic code above.

---

## Working with documents over HTTP

Most applications never call these routes directly — `@mako-cloud/rxdb` and `@mako-cloud/edge-sdk` do — but the rules below explain behavior you will see through either.

### Headers

| Header | Who sends it | Meaning |
| --- | --- | --- |
| `X-Mako-Key: mako_pk.…` | Every application request | The public project key; identifies and meters the client |
| `Authorization: Bearer <access token>` | Authenticated requests | The application user's session |
| `X-Mako-Service-Key: mako_sk.…` | `/service/` routes only | A scoped service credential (never beside a bearer token) |
| `X-Mako-Bypass-Reason: <text>` | `/service/` reads | The audited reason for a policy bypass |
| `X-Mako-Request-Id: <id>` | Optional on application routes, **required** on `/service/` | Identifies one request; the data plane keys a quota reservation by it |
| `Idempotency-Key: <id>` | Mutations | Replay protection; for document mutations it **must equal** the body's `mutationId` |
| `X-Mako-Object-Attributes: k=v,…` | Object uploads | Application attributes stored with the object ([details](#an-object-that-belongs-to-more-than-its-uploader)) |

### Read, mutate, query

```text
GET  /v1/projects/{p}/environments/{e}/collections/{c}/documents/{documentId}
POST /v1/projects/{p}/environments/{e}/collections/{c}/documents/{documentId}     (mutateDocument)
POST /v1/projects/{p}/environments/{e}/collections/{c}/documents/query            (queryDocuments)
```

A mutation body:

```json
{
  "mutationId": "m_01J9…",              // 16–200 chars; equals the Idempotency-Key
  "operation": "update",                 // create | update | delete
  "expectedRevision": "3-9f1c",          // null for create
  "schemaVersion": 1,
  "body": { "id": "t1", "ownerId": "usr_…", "title": "Buy milk", "updatedAt": 1757000000000 }
}
```

The result carries `status` — `applied`, `replayed` (the same `mutationId` was already committed; the stored outcome is returned), or `conflict` (the `expectedRevision` is not current; `currentRevision` and, when readable, the current `document` are returned) — and the resulting `DocumentRecord`: `primaryKey`, `schemaVersion`, `revision`, `commitPosition`, `_deleted`, `body`.

Two rules are easy to miss:

- **`Idempotency-Key` must equal `mutationId`.** They are one identity; a mismatch is `409 conflict`, not a validation error.
- **One request id per request.** Presenting the same `X-Mako-Request-Id` on two different requests is `409 conflict` naming the reuse — not something to retry. A function that makes several calls derives a distinct id per call.

### The `/service/` routes

```text
GET/POST /v1/projects/{p}/environments/{e}/service/collections/{c}/documents/{documentId}
POST     /v1/projects/{p}/environments/{e}/service/collections/{c}/documents/query
GET/POST /v1/projects/{p}/environments/{e}/service/users/{userId}/app-metadata
```

These bypass document policies within the credential's scope and write a `service_bypass` audit record first; if the record cannot be written the request fails closed. They require `X-Mako-Service-Key` and `X-Mako-Request-Id`, and refuse (`unauthenticated`) any request that also carries a bearer token or public key. The reverse proxy answers `/service/` with `404` on the platform hostname and on every custom domain: the routes are reachable only from the loopback origin edge functions are given.

### Errors and retries

Every failure is the versioned `ApiErrorEnvelope`:

```json
{ "apiVersion": "v1",
  "error": { "code": "permission_denied", "message": "…", "requestId": "req_…",
             "retry": { "kind": "never" }, "details": [ … ] } }
```

Codes: `invalid_request`, `unauthenticated`, `operator_step_up_required`, `permission_denied`, `not_found`, `conflict`, `precondition_failed`, `schema_mismatch`, `checkpoint_expired`, `rate_limited`, `quota_exceeded`, `unavailable`, `internal`. Retry advice is `never`, `immediate`, or `after_delay` with `afterMs`. Clients must not parse arbitrary server text; log the request identifier, not credentials or document bodies. Honor the advice: retry throttling only after the returned delay, refresh an expiring session through the auth token endpoint, stop on permission, schema, or authentication errors, and treat `checkpoint_expired` and stream gaps as secure full-resync states.

### Where this is tested

`crates/mako-smoke/tests/database_service.rs` and `edge_function.rs` drive the document and `/service/` routes end to end, including the encoded-id, request-id-reuse, and idempotency rules; `services/mako-data-plane/src/document_http.rs` holds the route tests.

---

## Application authentication

Application users belong to exactly one project environment. Their identities, sessions, roles, and trusted claims are distinct from developer and operator identities; the same normalized email may identify unrelated users in different projects.

### Routes

```text
POST /v1/projects/{p}/environments/{e}/auth/signup       { email, password (8–1024 chars), redirectUrl? } → 202 { accepted: true, verificationRequired }
POST /v1/projects/{p}/environments/{e}/auth/verify-email { token } → 200 { verified: true }
POST /v1/projects/{p}/environments/{e}/auth/signin       { email, password } → AuthSession
POST /v1/projects/{p}/environments/{e}/auth/token        { refreshToken } → AuthSession (rotated)
POST /v1/projects/{p}/environments/{e}/auth/signout      (bearer) → 204
GET  /v1/projects/{p}/environments/{e}/auth/user         (bearer) → AuthUser
GET  /v1/projects/{p}/environments/{e}/auth/jwks         → JsonWebKeySet
POST /v1/projects/{p}/environments/{e}/auth/password-recovery        { email, redirectUrl } → 202 { accepted: true }
POST /v1/projects/{p}/environments/{e}/auth/password-recovery/redeem { token, password } → AuthSession
```

A **forgotten password** is recovered by mail. `password-recovery` answers the same whether or not the address has an account; for an active or invited user it mails the environment's `recovery` template with a single-use link to the registered `redirectUrl`, carrying `#password_reset_token=<token>` and valid for an hour. The app reads the token (`MakoAuthClient.passwordLinkFragment(location.hash)`), asks for the new password, and sends both to `password-recovery/redeem` (`redeemPasswordLink(token, password)`), which replaces the password, revokes every earlier session, and signs the user in. Opening the mailed link proves the address, so a user still pending verification becomes active.

All take `X-Mako-Key`. An `AuthSession` is `{ accessToken, refreshToken, expiresIn, user }`; an `AuthUser` is `{ id, email, status, authorizationEpoch }` with `status` one of `unverified`/`pending_verification`, `active`, `disabled`, `deleted`. Provider and magic-link routes are in the [next chapter](#sign-in-providers-and-magic-links).

### Session lifecycle

1. **Sign-up** validates the project password policy and follows the environment's [email-verification setting](#email-verification). Public responses do not reveal whether an account exists. Passwords are hashed with Argon2id and parameters upgrade automatically on sign-in.
2. **Sign-in** issues a short-lived Ed25519-signed access JWT and an opaque refresh credential. The JWT binds issuer, audience, subject, project, environment, role, session, expiry, and authorization epochs.
3. **Refresh** rotates the stored credential hash. The credential is single-use; a second spend outside a five-second concurrency grace window is a **replay** and revokes the whole refresh family. A refresh family lives at most thirty days.
4. **Sign-out**, administrator disable or delete, password recovery, or explicit session revocation publishes an ordered invalidation. Gateways fail closed if revocation freshness cannot be proven.

Use `GET …/auth/jwks` with the public project key to retrieve verification keys. A key rotation (`mako-cloud keys signing rotate --overlap <seconds>`) publishes the new active key while retaining the prior public key through the overlap so existing tokens remain verifiable; do not retire the old key until the maximum token lifetime and clock-skew window have elapsed.

### Credentials and metadata

Public project credentials may be embedded in browser applications but never authorize protected data. Secret service credentials are one-time-display, hashed, scoped, rotatable (`mako-cloud keys rotate … --overlap`), and restricted to explicit privileged routes.

Every application user carries two metadata documents (each at most 64 KiB, nesting depth 16):

- **Trusted metadata** (app metadata) is administrator-controlled. Its `role` and every other key become the `role` and `trusted_claims` of the user's next token, which policies read as `identity.role` and `claims.<name>`.
- **Profile metadata** is user-editable and is **never** a policy input; it must not assign a role or grant access.

Passwords, refresh credentials, private signing keys, and raw session tokens are excluded from APIs, logs, and audit records.

### Trusted metadata from an application's function

An application's own trusted code — an edge function holding an attached service secret — can set a user's trusted app metadata, so the memberships and roles the application manages become the claims that user's next token carries and policies trust:

```http
POST /v1/projects/{p}/environments/{e}/service/users/{userId}/app-metadata
X-Mako-Service-Key: mako_sk....
X-Mako-Request-Id: req_...
Content-Type: application/json

{ "reason": "invitation accepted", "appMetadata": { "households": { "hh_1": "editor" } } }
```

- **Credential.** Only a service credential scoped to the reserved `users` target with the `update` operation is accepted (`mako-cloud keys service create --collection memberships --collection users --operation read --operation update`). Document scopes name collections; `users` names the identity surface and is checked by the same gateway. A credential without it is refused with `permission_denied`. A request that carries a bearer token or public key beside the service key is refused with `unauthenticated` — there is no fallback and no route through which an application user can reach app metadata.
- **Body.** `reason` (1–512 printable characters) is the audited bypass reason and is required. `appMetadata` is a one-level JSON merge patch: a key set to `null` is removed; any other key replaces the stored value whole. The patch and the merged result are bounded like trusted metadata; a patch that would exceed them is refused before anything is written. `expectedAuthorizationEpoch` is optional and names the epoch the patch was composed against; the write is refused with `conflict` if the user's epoch has moved since.
- **Reading it back.** `GET` on the same path, with the reason in `X-Mako-Bypass-Reason` and a credential scoped to `users` with `read`, returns the user's app metadata and current authorization epoch. Because a patch replaces a key whole, a function that manages one member of a claim map composes the next value out of this one, and naming the epoch it read on the write keeps two concurrent changes from silently keeping whichever wrote last. The read is audited as `service_user_app_metadata_read`.
- **Audit.** Before the metadata is written a `service_bypass` record is appended (`service_user_app_metadata_update` on `application_user/{userId}`, reason code `service_bypass_verified`, the `bypass_reason` in the details). If that record cannot be written the request fails closed. It is written once the credential is verified, so an unknown user (`not_found`) is audited too.
- **Reaching the next token.** A change advances the user's authorization epoch exactly as an administrator's metadata edit does and invalidates developer explorer grants for the environment. The user's access token no longer verifies, the client refreshes, and the refresh issues a token whose `trusted_claims` (and `role`, when trusted metadata carries one) are the new values. A patch that changes nothing is audited but advances no epoch.

`@mako-cloud/edge-sdk` exposes the route as `createServiceClient(...).users.getAppMetadata(userId)` and `.setAppMetadata(userId, patch, { reason, expectedAuthorizationEpoch })` ([example](#the-service-client)).

### Client behavior

Use `MakoAuthClient` from `@mako-cloud/rxdb` and persist refresh state only in the platform's protected credential storage. On `authentication_required`, stop replication, close the live stream, clear protected local state as the application requires, and request a new sign-in. [Building a local-first app with RxDB](#building-a-local-first-app-with-rxdb) covers refresh, offline behavior, and the authorization-epoch reset.

### Where this is tested

`npm run test:auth-security` covers Argon2id upgrades, enumeration-safe flows, JWT boundaries, encrypted signing-key rotation, refresh replay, revocation freshness, project credentials, cross-project rejection, and RxDB client refresh behavior ([latest qualification](dev-book.md#authentication-security-qualification)). Refresh-replay incident response is the Dev Book's [auth replay runbook](dev-book.md#runbook-authentication-refresh-replay).

---

## Sign-in providers and magic links

Applications can let their users sign in the ways users expect: with Google, GitHub, or any OpenID Connect provider, and with a single-use link sent by email. Both paths end in the same application-user session that password sign-in issues, and both are configured per environment through the management API, the console's **Auth providers** screen, or `mako-cloud auth-settings`. The settings live in the data plane that mints application sessions; the control plane keeps no copy and forwards each authorized read or replacement to it.

### Configuring an environment

The environment's settings are one document, replaced whole:

```json
{
  "providers": [
    { "name": "google", "kind": { "type": "oidc", "issuer": "https://accounts.google.com" },
      "clientId": "1234.apps.googleusercontent.com", "clientSecret": "GOCSPX-…", "scopes": [], "enabled": true },
    { "name": "github", "kind": { "type": "git_hub" },
      "clientId": "Iv1.abcdef", "clientSecret": "…", "enabled": true },
    { "name": "okta-acme", "kind": { "type": "oidc", "issuer": "https://acme.okta.com" },
      "clientId": "0oa…", "clientSecret": "…", "scopes": ["groups"], "enabled": false }
  ],
  "redirectUrls": ["https://app.example.com/auth/callback", "http://localhost:5173/auth/callback"],
  "magicLinks": { "enabled": true, "linkTtlSeconds": 900 },
  "emailVerification": { "required": true }
}
```

- `providers` (at most 16): each has a `name` (lowercase letters, digits, hyphens; 2–64 characters) that appears in the application's URLs, a `kind` — `oidc` with the issuer whose `/.well-known/openid-configuration` the data plane discovers, or `git_hub` for GitHub's OAuth 2.0 — the `clientId` the provider issued, the `clientSecret`, optional extra `scopes` beyond `openid email profile` (ignored for GitHub), and `enabled`. A disabled provider is kept but refuses every flow.
- `redirectUrls` (at most 32): where a provider callback or a magic link may send the browser. Absolute `https` URLs, or `http` to `localhost` or a loopback address for local development; no fragment, no credentials. Matched **exactly** — never by prefix — so register every page that finishes a sign-in.
- `magicLinks`: whether passwordless sign-in by email is on, and how long a link lives (60–3600 seconds).
- `emailVerification` (optional; left out means off): whether a password sign-up must confirm its address before it can sign in. Turning it on needs at least one redirect URL. See [Email verification](#email-verification).

Register the provider's side with the callback URL `https://<your api origin>/v1/projects/{projectId}/environments/{environmentId}/auth/providers/{name}/callback`.

`PUT …/auth-settings` (`updateAuthSettings`, with an `Idempotency-Key`) installs the document and answers with the installed view; `GET …/auth-settings` reads it. Owners and administrators may replace the settings; the read needs the same credential-reading permission as the environment's keys. Every replacement advances `version` by one; the management API numbers the new version after the one it read, so a client never has to track it.

```bash
mako-cloud auth-settings get -p prj_… -e env_…
mako-cloud auth-settings set -p prj_… -e env_… --input @auth-settings.json
```

#### Secrets are sealed and never returned

A `clientSecret` is a value, not a reference: the control plane seals it with XChaCha20-Poly1305 under a key both planes derive from the deployment's shared internal secret, bound to the project, environment, and provider name so a sealed secret cannot be moved to another environment or provider, and only then sends it to the data plane. Neither plane ever returns it. The installed view shows `hasSecret` per provider instead.

Because the document is replaced whole, a provider given **without** `clientSecret` keeps the secret already installed under its name — so reading the settings, editing a client id or flipping `enabled`, and writing them back is safe. A provider that has no installed secret and is given none is refused with `invalid_request` before anything is sent to the data plane. Removing a provider removes its secret with it.

### The browser flow

The application never sees a provider token, and no token ever travels in a URL:

1. **Start.** With its public project key, the application calls `POST …/auth/providers/{name}/start` (`startProviderSignIn`) with the `redirectUrl` it wants the browser back at — one of the registered ones — and receives the provider's `authorizationUrl`. It navigates the browser there. The request is refused when the provider is not enabled or the redirect is not registered.
2. **Callback.** The provider sends the browser to `GET …/auth/providers/{name}/callback?state=…&code=…` (`completeProviderSignIn`). Nothing on this request is trusted until the signed `state` verifies as issued by this environment for this provider and not yet expired. The data plane then redeems the code with the provider, fetches the identity, and requires a **verified email**: the application user is matched by provider and subject first, then by verified email, and created when neither matches. The identity is linked to the user by provider subject — never by email alone — so a later sign-in with the same account finds the same user even if the email changes at the provider.
3. **Redirect with a one-time code.** The browser is sent (`302`) to the registered redirect with a short-lived, single-use code in the URL fragment: `https://app.example.com/auth/callback#code=…`. A refusal — the provider declined, the exchange failed, the email is not verified, the user is disabled — arrives the same way as `#error=…`. Fragments are not sent to servers and do not land in logs.
4. **Exchange.** The application calls `POST …/auth/providers/exchange` (`exchangeProviderSignIn`) with `{ "code": … }` and its public project key and receives the same `AuthSession` password sign-in returns. The code is spent on first use and expires after two minutes.

Every outcome — verified, refused by the provider, exchange failed, user disabled, code redeemed — is recorded as an authentication event visible in the environment's observability screens and `mako-cloud observability auth-events`.

### Magic links

With `magicLinks.enabled`:

1. `POST …/auth/magic-link` (`requestMagicLink`) with `{ "email", "redirectUrl" }` answers `202 { "accepted": true }` for any well-formed address, so the endpoint reveals nothing about who is registered. A magic link is also how a new user signs up: an address without a user gets one, pending until the link proves the address, and redemption activates it. The data plane writes a mail intent that the control plane's mail worker renders with the environment's `magic_link` template (or the built-in default) and sends; the link points at the registered redirect with a single-use token. A disabled or deleted user's address is accepted identically and mails nothing.
2. The application reads the token from the link and calls `POST …/auth/magic-link/redeem` (`redeemMagicLink`) with `{ "token" }` to receive an `AuthSession`.

A link is bound to the environment and the email it was sent to, expires after `linkTtlSeconds`, and is spent on first use: a second redemption, or one after expiry, is refused with `unauthenticated` and no session is issued.

### Email verification

With `emailVerification.required`:

1. `POST …/auth/signup` (`signUp(email, password, { redirectUrl })`) must name one of the registered `redirectUrls`; without one, or with any other, it is refused with `invalid_request`. It answers `202 { "accepted": true, "verificationRequired": true }`. A new account is created `unverified` and a mail intent is written, rendered with the environment's `verification` template (or the built-in default); the link is the redirect with `#verification_token=…` and lives 24 hours. An address that is already registered gets the same answer and no mail.
2. Until the link is redeemed, password sign-in for the account is refused like a wrong password. A magic link to the same address also proves it and activates the account.
3. The application reads the token (`MakoAuthClient.verificationFragment(location.hash)`) and calls `POST …/auth/verify-email` (`verifyEmail(token)`) with `{ "token" }`. It answers `200 { "verified": true }` and the account becomes `active`; no session is issued, so the user then signs in with their password. A spent, expired, or unknown token is refused with `unauthenticated`.

With the setting off, which is the default, sign-up activates the account at once, ignores `redirectUrl`, and answers `verificationRequired: false`.

Accounts created while verification was off stay active when it is turned on. A sign-up whose mail could not be queued answers `unavailable` after the account is stored; a retry is accepted without mail, so ask that user to sign in by magic link.

### Local development

Outside production the data plane also speaks plain HTTP to loopback providers, so a stub standing in for Google or GitHub can be exercised by the smoke suite; `http://localhost:…` and `http://127.0.0.1:…` redirects are admitted for the same reason. In production only `https` redirects and providers are reachable.

### Where this is tested

`services/mako-control-plane/src/auth_settings_http.rs` (sealing, keep-secret substitution, version numbering, validation), `services/mako-data-plane/src/auth_provider_http.rs` (installation and the start/callback/exchange and magic-link flows), `crates/mako-smoke/tests/auth_providers.rs` (the whole flow against a loopback provider stub, and email verification through a captured SMTP relay), and `packages/cli/test/auth-settings.test.mjs`.

---

## Building a local-first app with RxDB

`@mako-cloud/rxdb` is the supported application client. It connects a normal RxDB collection to Mako's authenticated pull, push, and SSE endpoints, manages the application user's session, and reads and writes bucket objects. It does not expose the underlying storage. It speaks only the application surface: it carries a public project key and an application-user session — never a developer session, an automation token, or a service credential — and never reaches the management or operator API.

The code below follows the browser-tested implementation in [`examples/local-first`](../examples/local-first/README.md), whose fake backend implements the same public wire protocol so it can test offline behavior without a hosted environment.

### Install and configure

The adapter is not yet published to npm. Install its built `v0.2.0` release from the public [distribution repository](https://github.com/makodb/mako-rxdb) over HTTPS, alongside RxDB 17 and RxJS 7:

```sh
npm install https://codeload.github.com/makodb/mako-rxdb/tar.gz/refs/tags/v0.2.0 rxdb@17 rxjs@7
```

`rxdb` (`>=17.0.0 <18.0.0`) and `rxjs` (`>=7.8.0 <8.0.0`) are peer dependencies. Node 20.19 or newer, or any modern browser; the package is ESM only and ships browser and Node builds behind one entry point. The archive includes the compiled package and depends on nothing else from this repository ([package README](../packages/rxdb-client/README.md)). Keep importing from `@mako-cloud/rxdb`; only the installation source differs. Commit your package lockfile to preserve the resolved archive and integrity hash.

Create one normalized configuration per replicated collection:

```ts
import { normalizeMakoRxdbConfig } from "@mako-cloud/rxdb";
import { RXDB_VERSION } from "rxdb/plugins/utils";

const config = normalizeMakoRxdbConfig({
  endpoint: "https://api.example.mako.cloud",
  projectId: "prj_abcdefgh",
  environmentId: "env_abcdefgh",
  collectionId: "todos",
  schemaVersion: 1,
  publicProjectKey: "mako_pk.example",
  rxdbVersion: RXDB_VERSION,
  runtime: "browser",
  pullBatchSize: 100,
  pushBatchSize: 100,
});
```

Production endpoints must use HTTPS; plain HTTP is accepted only for `localhost` and `127.0.0.1`. The adapter rejects an unsupported RxDB major before starting replication.

The console's **API & Connect** page emits versioned template 1 from `createMakoRxdbConnectTemplateV1`, compiled in the local-first example on every TypeScript qualification run. Its connection check submits only the public key id, collection/schema metadata, and RxDB version; it never sends a public-key secret or creates an application-user session.

### Authenticate an application user

`MakoAuthClient` manages access and refresh tokens. It never uses developer sessions or service credentials.

```ts
import { MakoAuthClient } from "@mako-cloud/rxdb";

const auth = new MakoAuthClient(config, { persistence: encryptedSessionPersistence });

await auth.restoreSession();
if (auth.currentSession() === null) {
  await auth.signInWithPassword(email, password);   // or auth.signUp(email, password) first
}
```

Implement `AuthSessionPersistence` (`load`, `save`, `clear`) over the platform's protected credential storage, or use `BrowserAuthSessionPersistence` in a web application. The public `MakoUserSession` intentionally omits the refresh token. `validAccessToken()` returns the stored access token while it has more than 30 seconds of validity left and renews it otherwise.

#### Session renewal

The refresh credential is single-use: the token route rotates it and treats a second spend outside its narrow grace window as a replay, which revokes the whole refresh family. The client therefore **coalesces** renewal. One refresh is in flight at a time; `refreshSession()` and `validAccessToken()` callers — across every replication scope, storage call, and live stream in the page — await that same request and resolve with the same session, so several scopes reacting to one `authorization_epoch_changed` signal cannot spend the rotated credential twice.

A refresh either ends the session or does not touch it, and the client tells those apart by what the service answered:

| Outcome | Stored session | Thrown | Flag and event |
| --- | --- | --- | --- |
| Rotated (`200`) | replaced | — | `refreshUnavailable` cleared; `session`, plus `refresh_recovered` if it had been set |
| `401 unauthenticated` (invalid or replayed credential), `403 permission_denied`, any other `4xx` | cleared | `MakoAuthenticationRequiredError` (`code: "unauthenticated"`, `retryable: false`) | `signed_out` |
| Network fault, timeout, `408`, `429` (`rate_limited` or `quota_exceeded`), any `5xx` | kept | `MakoAuthError` (`code: "refresh_unavailable"`, `retryable: true`) | `refreshUnavailable` set; `refresh_unavailable` on the transition |

Only a definitive refusal signs a user out. Under a transient failure the persisted session stays exactly as it was, `authenticationRequired` stays `false`, and `refreshUnavailable` becomes `true` until a later refresh succeeds. `validAccessToken()` then returns the current access token while it has not yet expired and throws the retryable `MakoAuthError` once it has — it never throws `MakoAuthenticationRequiredError` for a failure the service did not pronounce. A `429 quota_exceeded` is kept the same way even though its retry advice is `never`: the credential is intact and the refresh succeeds once the quota window resets.

This is what an offline restart looks like: `restoreSession()` returns the stored session, the expired access token cannot be renewed, and the application keeps reading and writing its local RxDB data with `refreshUnavailable` set; replication resumes on its own when the network returns. Pulls, pushes, and object requests report that state as a retryable `unavailable` rather than `unauthenticated`, so RxDB retries instead of the application tearing the session down. A live stream keeps reconnecting through it; the stream ends only on a verdict.

`subscribe(listener)` reports these transitions without polling and returns the function that stops the subscription:

```ts
const stop = auth.subscribe((event) => {
  switch (event.kind) {
    case "session":            // signed in, or a refresh rotated the session
    case "refresh_recovered":  hideOfflineBanner(); break;
    case "refresh_unavailable": showBanner("offline - working from local data"); break;
    case "signed_out":         requireSignIn(); break;
  }
});
```

Each event carries `session`: the session in force when it was emitted, or `null` once signed out. A listener that throws is ignored, so one subscriber cannot break another.

#### Sign in through a provider

```ts
// On the sign-in screen: navigate to the provider.
const { authorizationUrl } = await auth.startProviderSignIn("google", "https://app.example.com/auth/callback");
window.location.assign(authorizationUrl);

// On the redirect page: finish the exchange from the fragment the browser landed with.
const session = await auth.completeProviderSignIn(window.location.hash);
```

`startProviderSignIn(provider, redirectUrl)` returns `{ authorizationUrl, provider }`; the redirect must be one the environment registered, matched exactly. `completeProviderSignIn(fragment)` accepts `location.hash` with or without its leading `#`, exchanges `#code=…`, stores the session, and returns it. A provider refusal arrives as `#error=<reason>`; the helper throws `MakoAuthError` with that reason in `error.reason` (for example `provider_refused` or `email_not_verified`) without calling the service. Sessions obtained this way refresh, expose `validAccessToken()`, and are revoked exactly like password sessions.

#### Sign in with a magic link

```ts
await auth.requestMagicLink("person@example.com", "https://app.example.com/auth/magic");
// ...the mailed link lands on the redirect with `#magic_link_token=<token>`:
const session = await auth.redeemMagicLink(token);
```

`requestMagicLink` resolves when the service answers `202` and rejects with `MakoAuthError` on any other status; it tells the application nothing about whether the address is registered, by design. `redeemMagicLink(token)` trades the single-use token for a persisted session; a spent or expired token is refused with `unauthenticated`.

#### Recover a password, or finish an invitation

```ts
await auth.requestPasswordRecovery("person@example.com", "https://app.example.com/auth/callback");
// ...the mailed link -- a recovery, or an invitation -- lands on the redirect
// with `#password_reset_token=<token>`; ask for the new password, then:
const token = MakoAuthClient.passwordLinkFragment(window.location.hash);
if (token !== null) {
  history.replaceState(null, "", window.location.pathname + window.location.search);
  const session = await auth.redeemPasswordLink(token, newPassword);
}
```

`requestPasswordRecovery` answers the same whether or not the address has an account. `redeemPasswordLink(token, password)` sets the password -- an invited user's first one -- revokes every earlier session, and returns a persisted session; a spent or expired link is refused with `unauthenticated`, and a password the policy refuses with `invalid_request`. Check for the token before resuming a stored session: the link is for whoever it was mailed to. The reference app in `examples/local-first` shows both: a **Forgot password?** button and a page that asks only for the new password when opened from a link.

#### Handle the fragment on application load

Every redirect page can be reached by a provider callback, a magic link, or an ordinary navigation. `MakoAuthClient.signInFragment(fragment)` classifies `location.hash` so the application decides once, before rendering:

```ts
const fragment = MakoAuthClient.signInFragment(window.location.hash);
switch (fragment.kind) {
  case "provider_code": await auth.completeProviderSignIn(window.location.hash); break;
  case "magic_link":    await auth.redeemMagicLink(fragment.value); break;
  case "error":         showSignInError(fragment.value); break;
  case "none":          await auth.restoreSession(); break;
}
history.replaceState(null, "", window.location.pathname + window.location.search);
```

Clear the fragment after consuming it so a reload does not retry a spent code, and never log it.

#### Keep the session across reloads in a browser

`BrowserAuthSessionPersistence` stores the session in `localStorage` under `mako.auth.session.<projectId>.<environmentId>` (or a `key` you choose). When storage is unavailable (a sandboxed frame, a blocked or full store) it degrades to memory for the lifetime of the page and never throws; `durable` tells you which mode it is in. A malformed stored value is discarded rather than trusted. `clear()` removes the stored session, and `signOut()` calls it.

```ts
import { BrowserAuthSessionPersistence, MakoAuthClient } from "@mako-cloud/rxdb";
const auth = new MakoAuthClient(config, { persistence: new BrowserAuthSessionPersistence(config) });
```

`localStorage` is readable by any script on the origin, which is the trust boundary of a single-page application; keep third-party scripts off the origin that holds sessions.

### Connect an RxDB collection

Use `MakoCheckpoint` as the replication checkpoint type. Its `token` is an opaque, server-signed value; never inspect or construct it in application code.

```ts
import { MakoLivePullStream, MakoReplicationSignals, createMakoPullOptions, createMakoPushOptions, type MakoCheckpoint } from "@mako-cloud/rxdb";
import { replicateRxCollection } from "rxdb/plugins/replication";

const live = new MakoLivePullStream<Todo>(config, auth);
const replication = replicateRxCollection<Todo, MakoCheckpoint>({
  replicationIdentifier: "mako-todos-v1",
  collection: database.todos,
  pull: createMakoPullOptions(config, auth, { stream$: live.stream$ }),
  push: createMakoPushOptions(config, auth),
  live: true,
});

const signals = new MakoReplicationSignals<Todo>();
const subscriptions = signals.bind(replication);
live.start();
await replication.awaitInitialReplication();
```

Close the SSE stream, unsubscribe the signals, and cancel replication when the owning application scope is destroyed.

### Durable replication state with Dexie

RxDB's Dexie storage (`getRxStorageDexie` from `rxdb/plugins/storage-dexie`) keeps documents and RxDB's own replication checkpoint in IndexedDB, so a reopened application shows its data before any network request and pulls only what changed. The Mako-side state — the last checkpoint the live stream reached, the authorization-epoch security state, and the recovery state — lives next to it in `DexieReplicationStatePersistence`, an IndexedDB database of its own (default name `mako-replication-state`, one key namespace per project, environment, and collection):

```ts
import { DexieReplicationStatePersistence, MakoAuthorizationEpochCoordinator, MakoLivePullStream,
         MakoReplicationRecoveryCoordinator, createMakoPullOptions, createMakoPushOptions } from "@mako-cloud/rxdb";
import { createRxDatabase } from "rxdb";
import { replicateRxCollection } from "rxdb/plugins/replication";
import { getRxStorageDexie } from "rxdb/plugins/storage-dexie";

const database = await createRxDatabase({ name: "rational", storage: getRxStorageDexie() });
const durable = new DexieReplicationStatePersistence(config);

const security = new MakoAuthorizationEpochCoordinator("mako-todos-v1", securityHooks, { persistence: durable.security });
const recovery = new MakoReplicationRecoveryCoordinator(recoveryHooks, { persistence: durable.recovery });

const securityState = await security.initialize({ environment: environmentEpoch, user: userEpoch });
const recoveryState = await recovery.initialize();
if (recoveryState.kind !== "active") {
  // Finish the migration or full resync recorded by the previous run first.
}

const live = new MakoLivePullStream<Todo>(config, auth, { checkpoints: durable.checkpoint });
const replication = replicateRxCollection<Todo, MakoCheckpoint>({
  replicationIdentifier: securityState.replicationIdentifier,
  collection: database.todos,
  pull: createMakoPullOptions(config, auth, { stream$: live.stream$, checkpoints: durable.checkpoint }),
  push: createMakoPushOptions(config, auth),
  live: true,
});
live.start((await durable.checkpoint.load()) ?? undefined);
```

What each piece persists:

- `durable.checkpoint` — every checkpoint a pull returns or the live stream advances to, so `live.start(checkpoint)` resumes the SSE stream where it stopped. RxDB's own checkpoint in the Dexie storage stays authoritative for pulls; the persisted one only makes the resume precise.
- `durable.security` — the epochs, generation, and replication identifier. A restart under the same epochs reuses the persisted generation and identifier; a restart under different epochs clears the collection first.
- `durable.recovery` — `schema_migration_required` and `full_resync_required` survive a restart, and `markActive()` records the return to normal.

A security reset clears the persisted checkpoint and recovery state through the coordinator, and `durable.clear()` forgets everything for the collection; call it from a full-resync flow together with removing the collection. Any object implementing `ReplicationStateStore` (`get`, `set`, `delete`) can back the persistence, and `MemoryReplicationStateStore` is the deterministic choice for tests. `DexieReplicationStateStore` fails at construction when no IndexedDB implementation exists rather than silently keeping state in memory; pass `indexedDB` and `IDBKeyRange` to inject one.

### Policy and local data

Mako evaluates document policy for every pull, push, and live event. Pull and live use the same visibility rules. If a document was visible at an earlier checkpoint but is no longer visible, the server sends a synthetic tombstone so RxDB removes the stale local copy. A denied push is a non-retryable `permission_denied` error, and a conflict response includes a master document only when the current user may read it.

Client-side filters are a user-interface convenience, not an authorization boundary. Applications must also assume that previously synchronized data can remain in local storage until an authorization-epoch reset completes. Do not render a protected collection while a security reset or authentication-required state is active.

### Replicating one slice of a collection

A user who belongs to several households may read the documents of all of them, so a database per household would receive every household's documents and discard what it did not want. `filter` narrows the scope to the documents whose field holds one value:

```ts
const config = normalizeMakoRxdbConfig({ ...scope, collectionId: "transactions", filter: { field: "household_id", value: householdId } });
```

It is applied **after** the policy, so it can only narrow what the caller was already allowed to read; it is not an authorization boundary and the platform trusts nothing about it. The pull and the live stream both use it — the client sends the same filter to both. A document that leaves the filter comes back as a tombstone, exactly as one that leaves the policy's reach does. The checkpoint and the stream cursor are bound to the filter: resuming one under a different filter is refused rather than silently skipping changes. Changing an open database's filter therefore means a new replication identifier and a fresh local store, the same as changing its collection.

### One stream for many collections

A browser opens six connections to one host. An application with a dozen collections therefore cannot have a live stream each: later streams queue behind earlier ones, then the pulls and pushes queue behind those, and the application looks connected and syncs nothing. `MakoLiveStreamGroup` opens one connection for every collection it is given:

```ts
const group = createMakoLiveStreamGroup(configs, auth, { onResyncReason });
for (const config of configs) {
  await replicateRxCollection({
    collection: collections[config.collectionId],
    replicationIdentifier,
    pull: { ...createMakoPullOptions(config, auth), stream$: group.stream$(config.collectionId) },
    push: createMakoPushOptions(config, auth),
  });
}
```

Every collection on one connection must belong to one environment; the group refuses a configuration that mixes them. Each collection keeps its own policy, checkpoint, cursor, and filter. Every event names the collection it belongs to — beside `event` and `data`, not inside the payload — and a reconnect sends each collection's own cursor back. A collection whose events outrun the buffer resyncs on its own while the connection stays up for the rest; a frame the client cannot make sense of means the connection itself is untrustworthy, so every collection resyncs and it reconnects.

### Conflict handling

Mako follows RxDB's assumed-master-state protocol and returns readable master states as conflicts. Set the collection's `conflictHandler` from application semantics — for example, prefer the document with the larger application-managed version:

```ts
const conflictHandler = {
  isEqual(left: TodoState, right: TodoState) {
    return left.id === right.id && left.version === right.version && left._deleted === right._deleted;
  },
  async resolve({ newDocumentState, realMasterState }: ConflictInput) {
    return newDocumentState.version > realMasterState.version ? newDocumentState : realMasterState;
  },
};
```

Use a server-issued logical version, a hybrid logical clock, or another deterministic ordering value; do not rely on unsynchronized device wall clocks for business-critical resolution. The browser suite covers a concurrent offline edit where the newer remote state wins.

### Authorization-epoch security reset

Environment policy changes and user authorization changes increment epochs. Persist the epochs with the replication generation and route every mismatch through `MakoAuthorizationEpochCoordinator`:

```ts
const security = new MakoAuthorizationEpochCoordinator("mako-todos-v1", {
  async pauseReplication() { await replication.pause(); },
  async clearReplicatedCollection({ replicationRunning }) {
    // `replicationRunning: false` means nothing is open yet: clear by database name.
    await securelyRemoveLocalData({ byName: !replicationRunning });
  },
  onSecurityReset(event) { router.showAuthenticationBoundary(event.reason); },
  async startReplication(replicationIdentifier) { await createAndStartReplication(replicationIdentifier); },
});

await security.initialize({ environment: environmentEpoch, user: userEpoch });
await security.handleMismatch({ environment: nextEnvironmentEpoch, user: nextUserEpoch });
```

The required order is pause, securely clear, notify the application, and start with the newly generated replication identifier. Between clearing and starting, the coordinator also calls the persistence's optional `clearReplicationState()` so a durable checkpoint and recovery state never outlive the data they describe. Do not resume an old replication metadata store after clearing data. On access-token revocation, stop replication, clear protected local state, and require a new sign-in.

`initialize(epochs)` reuses the persisted state when the epochs match. When durable state was persisted under different epochs — the user was removed from a group while the application was closed — it runs `clearReplicatedCollection`, clears the persisted replication state, saves a new generation, and calls `onSecurityReset` before returning; nothing is running yet, so it does not pause, and the caller starts replication with the returned identifier. That case is why the clear is told whether replication was running: at startup there is no open collection, so an implementation that clears through a handle it holds does nothing at all and the previous generation's documents survive — the one outcome a security reset exists to prevent. With `replicationRunning: false`, remove the database by name.

Every transition runs alone. A live `authorization_epoch_changed` and the application's own epoch sync after a write arrive together routinely, and a second `initialize` or `handleMismatch` waits for the one in flight and then re-reads the settled state. Overlapping resets clear a database the other is replicating into, which RxDB reports as `DB8`.

### Schema migration and full resync

`schema_mismatch`, `checkpoint_expired`, stream gaps, and service failovers are explicit recovery states, not ordinary retry loops:

```ts
const recovery = new MakoReplicationRecoveryCoordinator({
  async pauseReplication() { await replication.pause(); },
  onSchemaMigrationRequired({ requiredSchemaVersion }) { migrationUi.open(requiredSchemaVersion); },
  onFullResyncRequired({ reason }) { resyncUi.confirmSecureReset(reason); },
});

replication.error$.subscribe((error) => void recovery.handleError(error));
const liveWithRecovery = new MakoLivePullStream(config, auth, { onResyncReason: (reason) => recovery.handleResyncReason(reason) });
```

For a schema mismatch, stop using the collection, run the application's RxDB migration or install the required schema, and create replication bound to that schema version. For a full resync, securely clear the affected collection and its replication metadata, create a new replication identifier, pull from the beginning, and call `markActive()` only after the application can safely read the collection again. With a `persistence` (`durable.recovery` above), the state survives a restart: call `initialize()` before starting replication and act on a restored non-`active` state.

### Files in buckets

`MakoStorageClient` reads and writes bucket objects ([Application file storage](#application-file-storage)) under the same public key and session:

```ts
import { MakoStorageClient } from "@mako-cloud/rxdb";

const storage = new MakoStorageClient(config, auth);
const path = `households/${id}/transactions/${txn}/receipt.png`;
const { etag, size } = await storage.put("receipts", path, file, { contentType: file.type, ifNoneMatch: "*" });
const object = await storage.get("receipts", path);            // { bytes, contentType, etag }, or null
const page = await storage.list("receipts", { prefix: `households/${id}/`, limit: 100 });
await storage.delete("receipts", path);
imageElement.src = storage.url("public-images", "logos/acme.png");
```

- `put` and `delete` require a session and refuse with `unauthenticated` before any request when none exists, or with a retryable `unavailable` when a session exists but its renewal cannot reach the service. `get` and `list` send the bearer when a session exists and go without it otherwise, so a public bucket answers anonymously and a policy bucket refuses.
- Bodies may be a `Blob`, `ArrayBuffer`, `Uint8Array`, or string; `contentType` is required and `ifNoneMatch` is forwarded verbatim as `If-None-Match`. `put` returns the quoted plaintext-digest ETag a later `get` answers with, and the stored size.
- Paths are percent-encoded segment by segment (`encodeObjectPath`), and a path that can never be valid — empty, `.` or `..` segments, control characters, more than 512 bytes — is refused locally.
- Failures are `MakoStorageError` with the API's `code`, `requestId`, `retry`, and `status`; a response without an error envelope maps to `unavailable` (5xx) or `internal` without echoing its body, and a network fault to a retryable `unavailable`.

`url()` builds the object's address for `<img src>` and links; it carries no credential, so it is only useful on a public bucket.

### Replication errors and retry behavior

Every pull, push, and live-stream failure is a `MakoReplicationError` carrying the service's `code`, `message`, `requestId`, `retry` advice, and `status`, plus the two values a caller acts on: `retryable`, and `retryAfterMilliseconds` for an `after_delay` advice. Two rules decide `retryable`, in this order:

1. **The service's advice outranks the status.** `retry: {kind: "never"}` is terminal whatever the status was, a `500` included. `immediate` and `after_delay` are retryable, and an `after_delay` carries its delay onto the error.
2. **A refused credential is terminal.** A `401`, or a `403` the service labelled `unauthenticated`, is classified `unauthenticated` and non-retryable even when its envelope advises a retry. Repeating a request with a credential the service has already rejected is a denial of service against your own backend.

| Failure | `code` | `retryable` |
| --- | --- | --- |
| `401`, or `403` with `unauthenticated`, with or without an envelope | `unauthenticated` | no |
| Any status whose envelope advises `never` — `403 permission_denied`, `409 schema_mismatch`, `409 checkpoint_expired`, `500 internal` | the envelope's code | no |
| Any status whose envelope advises `immediate` or `after_delay` — `429 rate_limited`, `503 unavailable` | the envelope's code | yes |
| `5xx` with no envelope (a proxy or gateway answered) | `unavailable` | yes, `immediate` |
| `408` or `429` with no envelope | `internal` | yes, `after_delay` 1 s |
| Any other `4xx` with no envelope | `internal` | no |
| Network fault, or a response body that is not the documented shape | `unavailable` | yes, `immediate` |
| A renewal that never reached a verdict: offline, `408`, `429`, `5xx` | `unavailable` | yes, `after_delay` 1 s |
| No session at all, or a renewal the service definitively refused | `unauthenticated` | no |

A `denied` push row keeps the policy error the service returned for it, ordinarily a non-retryable `permission_denied`.

#### One renewal, never a loop

A `401` is ordinarily nothing worse than an expired access token, so a pull, a push, and a stream connection each renew the session once and repeat the request exactly once with the renewed bearer. The renewal is the coalesced `refreshSession()`, so collections failing together share a single token request. A second refusal is definitive: the client remembers the refused token, raises the terminal `unauthenticated` error, and every later attempt carrying that same token fails on one request without renewing again. A refused push is repeated byte for byte under its original `Idempotency-Key`, so a renewal cannot duplicate a write. A renewal that could not reach a verdict is not a refusal: the session is kept and the retryable `unavailable` above is raised instead.

#### Stopping on a terminal error

RxDB's replication protocol has no notion of a non-retryable failure: it repeats a failed handler on its own `retryTime` for as long as replication runs. **An application must therefore stop replication itself when a terminal error arrives.** The live stream is owned by this package and does stop on its own — it errors `stream$` and closes rather than reconnecting against a verdict.

```ts
signals.activity$.subscribe((activity) => {
  if (activity === "authentication_required") {
    void replication.cancel();
    live.close();
    requireSignIn();
  }
});
```

`unauthenticated` means the session is gone, not that it is busy: cancel replication, close the live stream, and send the person through sign-in again. Do not renew or retry it in application code — the client already made the one renewal attempt worth making. After a new sign-in, create replication again (through `MakoAuthorizationEpochCoordinator` if the epochs moved) and start a new stream. Local data stays readable throughout.

Errors reach `error$` wrapped in RxDB's own `RC_PULL` / `RC_PUSH` error, which keeps only part of the original. `makoReplicationErrorFrom(error)` returns the `MakoReplicationError` inside one, or `null` for a failure this client did not raise; `MakoReplicationSignals` and `MakoReplicationRecoveryCoordinator.handleError` already unwrap it.

#### UI signals

`MakoReplicationSignals` exposes activity, received and sent documents, conflicts, throttling, security resets, and sanitized errors. Its error values do not contain tokens or arbitrary response bodies. Show `after_delay` throttles using their retry delay; do not retry errors whose advice is `never`. Treat `authentication_required`, `schema_migration_required`, and `full_resync_required` as blocking UI states rather than transient connectivity messages. A retryable `unavailable` from a renewal that could not reach the service is a connectivity message; pair it with `MakoAuthClient.refreshUnavailable` and its `refresh_unavailable` / `refresh_recovered` events.

### Where this is tested

`packages/rxdb-client` unit tests (renewal coalescing, the retry table, durable checkpoint resume, security-reset clearing); the local-first browser suite (`npm run test:browser -w @mako-cloud/example-local-first`) against an in-browser fake, and the same six scenarios against the real binaries (`npm run test:browser-live`): an offline write pushed after reconnect, a conflict resolved by the handler, a remote tombstone, token refresh, stream reconnect with `resync`, and access revocation clearing protected state. `npm run test:rxdb-chaos` models duplicate and reordered retries, tombstones, epoch resets, stream gaps, checkpoint expiry, and service restart ([details](dev-book.md#rxdb-chaos-qualification)).

---

## The replication protocol

The wire protocol `@mako-cloud/rxdb` speaks, for anyone writing another client or debugging one.

```text
POST /v1/projects/{p}/environments/{e}/collections/{c}/replication/pull
POST /v1/projects/{p}/environments/{e}/collections/{c}/replication/push
GET  /v1/projects/{p}/environments/{e}/collections/{c}/replication/stream        (SSE, one collection)
GET  /v1/projects/{p}/environments/{e}/replication/stream                         (SSE, many collections)
```

All carry `X-Mako-Key` and the application user's bearer token.

**Pull** takes `{ checkpoint: <Checkpoint> | null, schemaVersion, batchSize (1–1000), filter?: { field, value } }` and answers the next batch of documents visible to the caller, plus the new `checkpoint`. A checkpoint is a signed opaque string beginning `mcp1.`, bound to the tenant, collection, user, schema version, filter, and authorization epochs; a checkpoint whose history has been compacted away answers `409 checkpoint_expired`, and one issued under another schema version `409 schema_mismatch`. Documents whose visibility ended since the checkpoint arrive as tombstones (`_deleted: true`) carrying only the safe replication identity.

**Push** takes `{ schemaVersion, rows: [ { mutationId (≥16 chars), assumedMasterState | null, newDocumentState }, … ] }` (1–1000 rows) and answers one outcome per row: `accepted`, `conflict` (with `masterState` when the caller may read it), or `denied` (with the policy `error`). Outcomes are persisted per `mutationId`, so a retry after a dropped response replays the stored outcome and can never create a second revision.

**Stream** is server-sent events. Each event is one of:

| `event` | `data` | Meaning |
| --- | --- | --- |
| `documents` | `{ documents, checkpoint, cursor }` | Changes visible to the caller since the last event |
| `checkpoint` | `{ checkpoint, cursor }` | The checkpoint advanced with nothing to deliver |
| `heartbeat` | `{ cursor }` | Keep-alive, every 15 seconds |
| `resync` | `{ reason }` | The client must pull from its checkpoint again: `reconnected`, `stream_gap`, `checkpoint_expired`, `authorization_epoch_changed`, `service_failover` |

The server keeps a bounded buffer (1000 events) per stream; a client that falls behind receives `resync` with `stream_gap`. Reconnect with `Last-Event-ID` set to the last `cursor` to resume; the environment-scoped stream names the collection beside each event and takes each collection's cursor back. A policy or claim change arrives as `authorization_epoch_changed`, after which the client must run its security reset before pulling again.

A stream can also stop delivering without ending: the connection goes half-open when a device sleeps or changes networks, or a proxy between the browser and the service holds the events back. `@mako-cloud/rxdb` treats a stream that sends nothing — not even a heartbeat — for `silenceTimeoutMs` (default 45 000, three missed heartbeats) as dead: it drops the connection, asks RxDB to resync, and reconnects, so changes it missed arrive by pull.

---

## Application file storage

Applications store files — images, uploads, attachments — next to their documents, in **buckets** a developer creates per environment. Objects are governed by the same policy language documents use, metered like every other resource, and served by the data plane under `/v1/projects/{p}/environments/{e}/storage/{bucketId}/objects/{path}`.

### Buckets

A bucket declares:

- `access`: `policy` (every request is evaluated against the bucket's rules) or `public` (reads need no credential at all; writes are still evaluated).
- `maxObjectBytes`: the largest object it accepts, up to the platform ceiling of 16 MiB.
- `allowedContentTypes`: patterns such as `image/*` or `text/plain`; empty means any.
- `rules`: the document-policy language over the **object document** — `path`, `folder` (the path's first segment: `usr_1` for `usr_1/receipt.png`, empty at the top level), `bucket`, `owner_id`, `content_type`, `size_bytes`, `created_at`, `updated_at`, `attributes` — with `new.*` on `create`/`update`, `old.*` on `read`, `update`, `delete`, and `identity.*`, `claims.*`, `request.*` as for collections. A bucket with no rules refuses every policy-governed request; rules that do not compile are refused at configuration time.
- A per-user layout needs the path bound to its owner as well: the uploader is always the owner of what they upload, so `new.owner_id == identity.user_id` alone lets anyone create any path, including inside another user's folder. `new.owner_id == identity.user_id && new.folder == identity.user_id` keeps each user to paths under their own id; the console's starter rules do this.

```bash
mako-cloud storage buckets create receipts --access policy --max-object-bytes 8388608 \
  --content-type 'image/*' --content-type application/pdf --rules @receipts-rules.json
mako-cloud storage buckets list
mako-cloud storage buckets update receipts --rules @receipts-rules.v2.json
mako-cloud storage objects list receipts --all
mako-cloud storage objects delete receipts households/hh_1/receipt.png --yes
mako-cloud storage buckets delete receipts --delete-objects --yes
```

Developers manage buckets through the management API (`…/storage-buckets`), the console's **Storage** screen, and `mako-cloud storage buckets …`. The control plane holds no bucket state: it authorizes the developer and forwards to the data plane, which is the single source of truth for buckets, objects, and totals. Deleting a bucket that still holds objects is refused unless the developer confirms their loss (`deleteObjects=true`).

### An object that belongs to more than its uploader

The object document names who wrote the object and nothing else about what it is for, so a rule written over it alone can reach exactly one person. That is enough for an avatar and not enough for a receipt a household shares.

`X-Mako-Object-Attributes` attaches application-chosen strings to the object being written — `name=value` pairs, comma separated, at most 8, names `[A-Za-z_][A-Za-z0-9_]*`, values at most 128 bytes. They are stored with the object, so the bucket's rules read them as `new.attributes.<name>` on the write and `old.attributes.<name>` afterwards:

```text
create, update:  claims.households[new.attributes.household_id] != null
read, delete:    claims.households[old.attributes.household_id] != null
```

The attribute names the household; the **claim** decides. Anyone may attach any household id, and attaching one they are not a member of matches no claim and grants nothing. A malformed header is refused rather than dropped, because a rule that expected an attribute and did not see one would deny and the cause would be invisible.

### Objects

- `PUT …/objects/{path}` with the object's `Content-Type` stores it; a new path is a `create`, an existing one an `update`. Paths are `/`-separated, 1–512 bytes, and may not contain `.`/`..` segments, empty segments, or control characters — a path can never leave its bucket.
- `GET …/objects/{path}` serves the bytes with their content type and an `ETag` of the plaintext digest; a refusal sends no bytes.
- `DELETE …/objects/{path}` removes it.
- `GET …/objects?prefix=&limit=&cursor=` lists what the caller may read: each candidate is evaluated as a `read`, so a listing never names what a read would refuse.

Requests carry an application session (`Authorization: Bearer`) or, on the loopback-only `/service/storage/…` routes, a service credential with the same privileged-bypass audit as documents. Public buckets serve `GET` to anyone.

#### Conditional uploads

An upload may carry a condition on what is stored at the path, so two clients racing on one object cannot silently overwrite each other:

| Header | Meaning | If it does not hold |
| --- | --- | --- |
| `If-None-Match: *` | store only if the path is free (create-only) | `412 precondition_failed` |
| `If-Match: *` | store only if something is there (replace-only) | `412 precondition_failed` |
| `If-Match: "<etag>"` | store only if the stored object is exactly that version | `412 precondition_failed` |

Only a single strong tag is accepted — a list, a weak `W/"…"` tag, or an `If-None-Match` other than `*` is refused with `400 invalid_request` rather than ignored. Conditions are evaluated **after** the bucket's rules, so a caller the rules refuse learns nothing about what is stored, and **atomically with the commit**, so a concurrent change answers `409 conflict`, never a write past a failed check. A refused upload writes nothing. Nothing about a `DELETE` is conditional today.

### At rest

Object metadata and per-bucket totals live in the environment's keyspace; bytes live in the platform object store under `projects/{p}/environments/{e}/buckets/{bucket}/objects/{digest}.blob`, **encrypted** with XChaCha20-Poly1305 under a key derived for the tenant from the data plane's secret, bound to the bucket and path. The store addresses ciphertext by its own digest; the plaintext digest is kept in the object's record and verified on every read. Re-uploading a path mints a new address and retires the old bytes. Objects are read and written whole (no streaming), which the 16 MiB ceiling keeps affordable.

### Metering and limits

- `object_storage_bytes` — a level: the environment's stored object bytes, sampled like `storage_bytes`.
- `object_egress_bytes_per_month` — a flow: bytes served by downloads, recorded before a byte leaves and cross-checked against the gateway's egress counter.

Every storage request is charged as an egress request; downloads are charged their bytes. Plans include an allowance for each (free: 1 GiB stored, 5 GiB egress, capped; pro: 50 GiB and 250 GiB, overage billed at $0.02/GiB-month and $0.09/GiB). Refusals answer `429 quota_exceeded` with `retry: never`, or `rate_limited` with a delay for the platform rate windows.

### Where this is tested

`crates/mako-file-storage` unit tests (encryption at rest, owner-only access and anonymous refusal, atomic totals, path escapes, size and type limits, the ceiling, forced removal, public buckets, filtered paging, uncompilable rules, per-tenant keys) and `crates/mako-smoke/tests/file_storage.rs` (the whole flow against the real planes: bucket creation, upload/download/list/delete under policy, refusals without bytes, conditional uploads including a stale tag and a refused malformed condition, a public bucket, developer listing and totals, confirmed removal).

---

## Edge functions

Mako runs short-lived TypeScript and JavaScript HTTP functions behind a stable project route using a pinned Supabase Edge Runtime image and a Mako-owned versioned protocol. Functions receive the Fetch `Request`/`Response` contract, supported npm modules and WebAssembly, outbound fetch subject to policy, and streaming responses. A function's sandbox is deny-by-default ([what a function may do](#what-a-function-may-do)).

### Deploy and invoke

1. **Bundle** and validate source into an immutable digest (`uploadFunctionBundle`: a `source` bundle with an `entrypoint`, up to 512 files, and a `dependencies` map, or a `prebuilt` bundle; at most 10 MiB).
2. **Create a version** referencing the exact bundle, runtime pin, entrypoint, selected regions, secret versions, limits, and JWT setting.
3. **Health-check** the version before promotion.
4. **Promote** by atomically switching the active version pointer.
5. **Invoke** through `/{projectId}--{environmentId}/functions/v1/{functionName}[/{path}]` with any method, or `/functions/v1/{functionName}` on a custom domain.

```bash
mako-cloud functions create households --region <region> --secret HOUSEHOLDS_SERVICE_KEY
mako-cloud functions deploy ./functions/households --name households --yes          # bundle, upload, version, health, promote
mako-cloud functions test households --method POST --path /create --body '{"name":"Home"}'
mako-cloud functions logs households
mako-cloud functions deployments list households
mako-cloud functions deployments rollback households 3 --yes
```

A function's logs hold what it prints -- `console.log`, `info`, `warn`, `error` and `debug`, whether it serves with `Deno.serve(...)` or `export default { fetch }` -- plus a line at error level for every response with a 5xx status and lifecycle lines such as `deployment_loaded`. They are scrubbed like every log line and retained under the source `function:<name>` on the environment's Logs page.

A function's **configuration** is `{ verifyJwt, regions (1–16), secretNames (≤64), limits, allowedHosts? (≤8) }`. `limits` are `cpuMilliseconds`, `wallMilliseconds`, `memoryBytes`, `requestBytes`, `responseBytes`, and `concurrency`; the CLI's defaults are 1 000 ms CPU, 10 000 ms wall, 128 MiB, 1 MiB request and response, concurrency 4. JWT verification is on by default; `--no-verify-jwt` makes a function public (routing, payload limits, quotas, and audit context still apply). Rollback selects a previously healthy immutable version and does not modify its bundle; a failed deployment never replaces the active version, and a version that later fails its health check becomes ineligible for promotion.

### Data access and secrets

`@mako-cloud/edge-sdk` forwards the verified caller to auth and document APIs by default, so the same policies used by RxDB apply. Service-level maintenance is a separate explicit client initialized with an attached scoped service secret and produces a privileged-bypass audit record.

The caller a function sees is the one the gateway verified. Your function is handed that credential on the reserved `x-mako-caller-authorization` header, which `createFunctionClientFromRequest` reads; the request's own `Authorization` header is **not** forwarded, so a function that admits anonymous callers cannot mistake an unverified bearer token for an identity. A call with no verified caller reaches the function with none, and the caller client then refuses (`MakoCallerIdentityRequiredError`) rather than acting as somebody.

#### The SDK is supplied by the runtime

A function imports `@mako-cloud/edge-sdk` and nothing else has to happen: the runtime ships the built SDK beside its main worker, materializes it into the worker's own directory, and maps that one specifier onto it. Nothing is vendored into the bundle, no npm install runs, and the version a function gets is the platform's.

It is the **only** bare specifier a bundle may import. Every other one must be declared as a dependency mapping onto an uploaded module (`--dependency <specifier>=<path>` on `mako-cloud functions deploy`); an import the platform cannot resolve is refused at upload with an `unresolved_import` diagnostic rather than failing when the worker boots. A bundle may not carry a module path beginning with `__mako` or remap `@mako-cloud/edge-sdk` (`reserved_module_path`, `reserved_dependency_specifier`), because those are what the runtime injects.

Three environment values the runtime always injects locate the API and the tenant: `MAKO_API_URL`, `MAKO_PROJECT_ID`, and `MAKO_ENVIRONMENT_ID`.

#### The caller client

```ts
import { createFunctionClientFromRequest } from "@mako-cloud/edge-sdk";

Deno.serve(async (request) => {
  const mako = createFunctionClientFromRequest({
    request,
    endpoint: Deno.env.get("MAKO_API_URL")!,
    projectId: Deno.env.get("MAKO_PROJECT_ID")!,
    environmentId: Deno.env.get("MAKO_ENVIRONMENT_ID")!,
  });
  const user = await mako.auth.getUser();                    // the verified caller, or MakoCallerIdentityRequiredError
  const todos = mako.documents<{ ownerId: string; title: string }>("todos");
  const page = await todos.query({
    predicates: [{ field: "ownerId", operator: "eq", value: user.id }],
    sort: [{ field: "ownerId", direction: "asc" }],
    cursor: null,
    limit: 100,
  });
  return Response.json(page.documents);   // { documents, nextCursor }
});
```

`FunctionClient` has `auth` (`getUser()`, `signOut()`) and `documents<T>(collectionId)` with `get(documentId)`, `query(query)`, and `mutate(documentId, mutation)`; every call is made **as the caller**, under the caller's policies. The default client has no privileged mode and accepts no per-operation token override.

#### The service client

Privileged access is a separate, explicit initialization using a service credential from an attached secret. Every operation carries a required reason and request identifier for the server-side bypass audit event:

```ts
import { createServiceClient } from "@mako-cloud/edge-sdk";

const requestId = request.headers.get("x-mako-request-id")!;
const service = createServiceClient({
  endpoint: Deno.env.get("MAKO_API_URL")!,
  projectId: Deno.env.get("MAKO_PROJECT_ID")!,
  environmentId: Deno.env.get("MAKO_ENVIRONMENT_ID")!,
  serviceCredential: Deno.env.get("HOUSEHOLDS_SERVICE_KEY")!,
  reason: "household membership changed",
  requestId,
});

// A patch replaces a key whole, so the next claim is composed out of the current one --
// and the write names the epoch that read returned, so a concurrent change is refused rather than dropped.
const current = await service.users.getAppMetadata(invitee.userId);
const households = { ...(current.appMetadata.households ?? {}), [householdId]: "editor" };
const { authorizationEpoch } = await service.users.setAppMetadata(invitee.userId, { households }, {
  reason: `invitation ${invitationId} accepted`,
  expectedAuthorizationEpoch: current.authorizationEpoch,
});
```

`ServiceFunctionClient` has `documents<T>(collectionId)` (the same three methods, through the `/service/` routes) and `users` (`getAppMetadata`, `setAppMetadata`). The write advances the user's authorization epoch, so the app should refresh its session afterwards; the new claim is on the refreshed token. A `conflict` means somebody else changed this user's claims first: read again and recompose rather than retrying the same patch ([the rules](#trusted-metadata-from-an-applications-function)).

#### One request id per data-plane request

Both clients send `X-Mako-Request-Id`, and the data plane keys a quota reservation by it. Two data-plane requests carrying one request id are refused with `409 conflict` naming the reuse, so a function that makes more than one call derives a distinct id per call from the runtime's:

```ts
const requestId = request.headers.get("x-mako-request-id")!;
const read = createServiceClient({ ...options, requestId: `${requestId}r` });
const write = createServiceClient({ ...options, requestId: `${requestId}w` });
```

#### Giving a function a credential you already hold

`HOUSEHOLDS_SERVICE_KEY` above is a scoped service credential stored as a function secret. Create the credential, then store its value as a secret under the name the function reads:

```bash
mako-cloud keys service create --id key_households \
  --collection memberships --collection users --operation read --operation update \
  --secret-file ./.local/households.key --project "$PROJECT" --env "$ENV"

mako-cloud functions secrets create HOUSEHOLDS_SERVICE_KEY \
  --value-file ./.local/households.key --project "$PROJECT" --env "$ENV"

rm ./.local/households.key
mako-cloud functions create households --secret HOUSEHOLDS_SERVICE_KEY --region <region> ...
```

`--value <v>` takes the value inline; `--value-file <path>` reads it from a file and ignores one trailing newline, so it round-trips a `--secret-file` the credential command wrote and keeps the value out of shell history. Over the API this is `PUT …/function-secrets/{secretName}` with `{"value": "…"}`. A supplied value is handled exactly like a generated one and is **never returned** — not by this call and not by any later read — so the response carries metadata only. Creating a secret that already exists is a conflict.

Function secret values are encrypted, attached by exact version, injected only into the selected deployment, and never returned after creation. Logs redact known active values.

### What a function may do

A worker is started with the capabilities a function needs and nothing else. Everything below is refused at the runtime boundary, not by convention:

- **Network.** A function may open connections to the platform API origin the runtime injects as `MAKO_API_URL`, plus any external HTTPS hosts its deployment declares — and to nothing else. By default nothing is declared and `outboundNetwork: {mode: "deny_all"}` denies the function's own destinations — another host, another port on the same host, a raw socket, a WebSocket, a DNS lookup — while the injected SDK keeps working, because the origin it talks to is the platform's.

  A deployment that needs a third-party API declares it: `mako-cloud functions deploy --allow-host api.example.com` (repeatable, at most 8 hosts; locally, the same flag on `mako-cloud functions serve`). Each declared host is granted on port 443 only. The declaration is validated fail-closed before any version exists: lowercase DNS names only — never an IP literal, a port, a wildcard, or a name that resolves inside the platform (`localhost`, `metadata`, and everything under `.internal`, `.local`, `.localhost`, or `.arpa`) — and the refusal names the entry that failed. One caveat is documented rather than hidden: Deno's permission model authorizes by *name*, so a declared host whose DNS answer later changes is still connectable (DNS rebinding). The worker's own network namespace, the loopback-only internal RPC, and the refused internal-name families bound what such a rebind can reach; a resolver-pinning proxy is the complete fix and a deliberate non-goal for now. Declared hosts are part of the reviewed deployment and visible in `mako-cloud functions get`.
- **Files.** A function may read its own worker directory. It may not read anywhere else and may not write anywhere at all, including `/tmp`. Persist state in a document, an object, or a function secret.
- **Environment.** A function may read the secret names attached to its deployment and the `MAKO_*` values the platform injects. Any other name is refused rather than returned empty.
- **Modules.** Imports resolve inside the bundle, through a dependency mapping to another uploaded module, or to `@mako-cloud/edge-sdk`, which the runtime supplies. A module fetched over the network is never loaded.
- **Processes, native code, host identity.** A function may not spawn a process, open a native library, or read the machine it is running on.

`mako-cloud functions serve` applies the same grants locally, so a function that runs locally is not one the hosted sandbox will refuse. The mechanics are in the Dev Book's [edge runtime protocol](dev-book.md#worker-permissions).

### Isolation and regions

Each project deployment has a distinct worker identity. CPU, wall time, memory, request/response size, concurrency, egress, and post-request work are bounded; violations recycle only the affected worker. The gateway may fail over only to another healthy region selected for that project. If none is healthy, it fails instead of entering an unauthorized region.

Logs and metrics include sanitized status, latency, resource use, immutable version, region, request id, and trace id. They exclude bodies, authorization headers, environment values, and secret values.

### Serving a function locally

`mako-cloud functions serve` runs a function in the same pinned Supabase Edge Runtime release used by the hosted runtime. Docker or Podman must be available; the CLI starts the image by immutable digest and mounts both the function and Mako's main worker read-only.

```bash
mako-cloud functions serve ./functions/hello-world \
  --project-id prj_abcdefgh \
  --environment-id env_abcdefgh \
  --api-url http://host.docker.internal:8787 \
  --jwks-file ./.local/jwks.json \
  --jwt-issuer http://localhost:8787/auth/v1 \
  --jwt-audience mako-app \
  --env-file ./.local/function.env \
  --secret-file ./.local/function.secrets
```

Invoke it at `http://127.0.0.1:9000/prj_abcdefgh/functions/v1/hello-world`.

- `--env-file` and `--secret-file` are `KEY=VALUE` files; the second identifies sensitive values. Only names explicitly present in those files are mapped into the user worker. Secret values are supplied through the child process environment, never placed in container arguments or dry-run output, and runtime stdout/stderr redact exact active secret values before they reach the terminal. Do not commit either file when it contains credentials.
- JWT verification is on by default. Use `--no-verify-jwt` only for a function configured as public. If a public request supplies an `Authorization` header the runtime still validates it, so pass the complete JWKS, issuer, and audience options if callers may send tokens. Local signature and tenant-claim checks match hosted ingress, but local serving cannot check remote session revocation or authorization-epoch freshness unless the local identity service supplying the JWKS is also running.
- The function receives the hosted function-owned path (the public prefix is removed), query string, method, headers, body stream, and request/trace correlation headers. Regional routing, hosted quotas, and production egress infrastructure are differences; local resource limits are still applied.
- `--wall-time-ms` can lower the per-invocation wall limit from its 300 000 ms default and cannot raise it above that hosted-compatible ceiling.
- `--dry-run` inspects the redacted container command. Stop and rerun after changing environment or secret files.
- `import { createFunctionClientFromRequest } from "@mako-cloud/edge-sdk"` works in a served function with no install step, the same way it does hosted.
- A served function reaching the data plane needs `--api-url` to name an address the container can dial. `host.docker.internal` and `host.containers.internal` reach the host but **not** the host's loopback: a data plane bound to `127.0.0.1` answers `connection refused`. Bind the data plane to an address the container can reach, or — with rootless Podman — run the container with `--network pasta:--map-host-loopback,169.254.1.2`.
- A `fetch` that a served function needs and the hosted sandbox denies fails the same way in both places — `Requires net access to ...` — rather than passing locally and failing after deployment.

### Deploying a function locally needs an object store

Serving is self-contained, but **deploying** is not. `mako-cloud functions deploy` and the management API upload the bundle to the control plane, which stores the artifact in the S3 object store that `MAKO_OBJECT_STORE_ENDPOINT`, `MAKO_OBJECT_STORE_ACCESS_KEY_REF`, and `MAKO_OBJECT_STORE_SECRET_KEY_REF` name. Without a reachable, credentialed one, bundle upload and `mako-cloud functions deployments create` answer `503 function administration is unavailable`.

Two things surprise people here:

- `mako-local-bootstrap` does **not** need it. It constructs the function administration service in its own process with an in-memory object store, so the sample functions it deploys never touch S3. A bootstrapped tenant with a working function therefore proves nothing about whether your object store is usable.
- The compose object store in `infra/local/compose.yaml` starts with no S3 identity of its own, so the platform's signed requests are refused (`InvalidAccessKeyId ... Available keys: 0`) until one is configured that matches the access and secret key the services resolve.

Deploying a function also needs a runtime supervisor the control plane can authenticate to; `mako-cloud functions serve` cannot act as one.

### Running a hosted function locally

Local serving and hosted invocation are different paths. Serving runs the pinned runtime for one function and answers directly. Hosted invocation goes through the gateway, which resolves the function against the control plane and forwards to a supervisor holding a **registered deployment**.

`mako-cloud functions serve` cannot act as the hosted supervisor: it generates a random `MAKO_RUNTIME_AUTHORIZATION` for its container, so the control plane cannot authenticate to it and no deployment can be registered. To exercise the hosted path locally, run the runtime on the same contract a deployment uses — `infra/ansible/roles/dependencies/files/quadlet/mako-edge-runtime.container` is the authoritative form:

- `MAKO_RUNTIME_AUTHORIZATION` must equal the internal auth secret the services use.
- `MAKO_RUNTIME_REGION` must equal `MAKO_REGION`, or the control plane reports the function unavailable in that region.
- Mount `packages/cli/runtime/main` at `/home/deno/functions/main` and publish container port 9000.

With that supervisor listening, `mako-local-bootstrap` deploys its sample function through the real administrative path — create, bundle, deploy an immutable version, health-check, promote — and the `deploy` step is what registers the deployment with the supervisor. Without a supervisor the bootstrap skips function deployment and says so. Invocation addresses the project reference `/{projectId}--{environmentId}/functions/v1/{functionName}`; a bare project id never resolves.

The edge gateway resolves its dependencies from compiled-in constants (data plane 8080, control plane 8081, its own 8082, runtime 9000), so the hosted-path suite cannot run beside a development stack on the same ports; see the [Dev Book](dev-book.md#running-the-edge-runtime-locally) for the automated version of this flow, engine selection, and the rootless-Podman notes.

### Where this is tested

`crates/mako-smoke/tests/edge_function.rs` (run through `MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-e2e`) starts the pinned runtime, deploys a function through the administrative path, brings up all three services, and asserts the function's response comes back through the gateway, that an undeployed function is not served, that a bare project reference does not resolve, and — with a second sample function — that the SDK import, a supplied service secret, an encoded document id, and request-id reuse behave as documented. `MAKO_RUN_EDGE_RUNTIME_TESTS=1 npm run test:edge-security` exercises the real pinned image for compatibility, cross-project canaries, secret redaction, egress, limits, crash containment, regional routing, and supply-chain audit ([details](dev-book.md#edge-security-qualification)).

---

## Scheduled functions

Run a deployed function on a cron schedule — cleanup jobs, reports, syncs — without an external cron, with a history developers can read. An authorized member attaches a *schedule* to a function: a five-field cron expression evaluated in UTC and the request each due time sends. A control-plane worker fires what has fallen due **through the edge gateway**, so quotas, metering, function metrics, logs, and audit apply to a scheduled run exactly as to any other invocation, and records every run.

A schedule targets the function's **active deployment** only: attaching one to a function without an active deployment is refused with `409 conflict`, and each run invokes whatever deployment is active when it starts.

### Attaching a schedule

`POST …/functions/{functionName}/schedules` with an idempotency key:

```json
{
  "name": "Nightly report",
  "cron": "0 3 * * *",
  "request": { "method": "POST", "path": "/reports?kind=daily", "headers": { "x-report": "nightly" },
               "contentType": "application/json", "body": "{\"day\":\"today\"}" },
  "enabled": true
}
```

`cron` is required; everything else has a default (`name` empty, `request` a body-less `POST /`, `enabled` true). The response is `201` with the schedule, including `nextRunAt`, `state` (`active` or `paused`), `timezone: "UTC"`, and `lastRun`.

```bash
mako-cloud schedules create --function nightly-report --cron "0 3 * * *" --name "Nightly report" --path "/reports?kind=daily" --header x-report=nightly
mako-cloud schedules list --function nightly-report
mako-cloud schedules update sch_… --function nightly-report --disable      # pause
mako-cloud schedules run-now sch_… --function nightly-report
mako-cloud schedules runs sch_… --function nightly-report --outcome failed
mako-cloud schedules delete sch_… --function nightly-report --yes
```

`GET …/schedules` lists a function's schedules; `GET`, `PATCH`, and `DELETE …/schedules/{scheduleId}` read, change, and remove one. A function carries at most 100 schedules. Any member of the team may read; a role that can change projects may write. Every mutation is audited as `function_schedule_create`, `function_schedule_update`, `function_schedule_delete`, or `function_schedule_run_now`; a refused read or write is audited as denied.

`PATCH` accepts any subset of `name`, `cron`, `request`, and `enabled`; fields omitted keep their values, and an update that changes nothing is refused. A changed `cron` recomputes `nextRunAt` from now. `enabled: false` **pauses** the schedule: `state` becomes `paused`, `nextRunAt` becomes `null`, and nothing fires until it is re-enabled, which requires an active deployment again. `DELETE` removes the schedule and its whole run history.

### Cron syntax

Five whitespace-separated fields, **evaluated in UTC**:

| Field | Values | Names |
| --- | --- | --- |
| minute | `0`–`59` | |
| hour | `0`–`23` | |
| day of month | `1`–`31` | |
| month | `1`–`12` | `jan`–`dec` |
| day of week | `0`–`6` (Sunday is `0`), `7` also Sunday | `sun`–`sat` |

Each field is `*`, a value, a range (`1-5`), a list (`1,15,30`), or a step over `*` or a range (`*/15`, `1-10/2`). Names are case-insensitive three-letter abbreviations. Examples: `0 3 * * *` (every day at 03:00 UTC), `*/15 * * * 1-5` (every fifteen minutes, Monday to Friday), `0 0 29 2 *` (midnight on 29 February, every leap year), `30 6 1,15 * *` (06:30 on the 1st and 15th).

Day of month and day of week follow the classic vixie rule: when **both** are restricted, a day matches if *either* does (`0 9 1 * mon` is the 1st of the month *and* every Monday); when either field starts with `*`, both must match (`0 9 * * 1` is Mondays only).

There is no timezone field and no daylight-saving arithmetic. Anything else — six fields, `?`, `L`, `W`, `#`, `@daily`, seconds, years, wrapped ranges such as `22-2` — is refused at save time with a message that names the field (`cron expression is invalid at minute: value is out of range`). An expression that never fires within the next five years (`0 0 30 2 *`) is refused too.

### The request

- `method`: `GET`, `POST` (default), `PUT`, `PATCH`, or `DELETE`.
- `path`: a path under the function beginning with `/` (default `/`), at most 1024 bytes; a query string is allowed. Segments must be non-empty and neither `.` nor `..`.
- `headers`: at most 16 extra headers, values at most 1024 bytes, names lowercased. `authorization`, `host`, `content-length`, `content-type` (set `contentType` instead), `transfer-encoding`, and any `x-mako-*` header are refused.
- `contentType`: the body's media type, default `application/json`.
- `body`: the body as text, at most 64 KiB. A `GET` carries no body and is refused with one.

The scheduler sends the request through the edge gateway's own invocation path, so it is subject to the function's request-size limit, the tenant's function-invocation quota and rate limit, and the environment's function metrics and logs. It is **not** counted as a public invocation and needs no bearer token even when the function has `verifyJwt` on: the scheduler's hop is authenticated by the platform's internal signature.

### How a scheduled invocation looks to the function

The function receives an ordinary request at its configured path and method with the configured headers and body, plus:

| Header | Value |
| --- | --- |
| `x-mako-schedule-id` | the schedule (`sch_…`) |
| `x-mako-schedule-run-id` | the run (`run_…`) |
| `x-mako-schedule-due-at` | the due time, RFC 3339 UTC |
| `user-agent` | `mako-cloud-scheduler/1` |
| `content-type` | `contentType`, on every method but `GET` |

There is no caller identity: the invocation is anonymous from the function's point of view, and the gateway audits it with the actor `function-scheduler/{scheduleId}/{runId}`. A function that must behave differently when scheduled should check `x-mako-schedule-id`.

**Those three headers are the scheduler's alone.** The gateway sets them on the internal hop and strips them from every public request before the function sees them — so a caller who sends `x-mako-schedule-id` does not become a schedule. What it does not mean is that a scheduled function's route is private: a stranger can still invoke it *without* the header. A function whose work only the schedule should start must therefore refuse an invocation that carries no `x-mako-schedule-id` — or, when the developer also wants to start a run by hand, hold a secret of its own and require it in a header the schedule is created with (`--header`). `examples/rational/functions/nightly` does the second.

### When runs happen

The worker runs every five seconds. Each pass reads every schedule whose `nextRunAt` is at or before now, oldest first; for each one **advances `nextRunAt` to the next due time after now before invoking**, so a run that outlives its interval never re-fires its own due time; invokes the function through the gateway, waiting at most **60 seconds**; records the run; and prunes history. Invocations in one pass run one after another; up to 20 due schedules are started per pass. The five-second cadence is the floor on how promptly a due time is noticed, not a bound on how long a pass takes.

A run is recorded with `dueAt`; `startedAt`, `completedAt`, `durationMilliseconds`; `functionVersion`; `outcome` — `succeeded` (2xx), `failed` (any other status), `error` (the invocation could not complete), `skipped_overlap`, or `null` while queued or executing; `responseStatus`; `error` — `timeout`, `no_active_deployment`, `gateway_unavailable`, `invalid_request`, or `throttled`; and `manual`. The schedule's `lastRun` summarises the most recent run.

**Overlap is skipped, never run concurrently.** A running invocation holds a per-schedule lease until it completes; the lease expires after the invocation timeout plus a grace period (60 s + 30 s), which bounds how long a worker that died mid-run can block its schedule. A due time that arrives while the lease is held is recorded as `skipped_overlap`; `nextRunAt` advances as usual. A `*/1 * * * *` schedule whose function takes ninety seconds therefore runs every other minute.

**Missed due times run once.** If the worker was down across one or more due times, the schedule runs **once**, for the **latest** missed due time, and `nextRunAt` is computed from now; earlier missed slots are not recorded. A function that must not lose work should read `x-mako-schedule-due-at` and reconcile from its own last checkpoint.

### Run now

`POST …/schedules/{scheduleId}/actions/run-now` with an idempotency key queues one invocation with the schedule's request outside its cron times and answers `202` with the queued run (`outcome: null`, `manual: true`, `dueAt` now). It takes the same lease as a cron run and does not move `nextRunAt`. It is refused with `409 conflict` while a run is still executing and when the function has no active deployment. A paused schedule can still be run now.

### Run history

`GET …/schedules/{scheduleId}/runs` lists runs newest first, with `outcome`, `cursor`, and `limit` (1–200, default 50) parameters and a `nextCursor`. History is retained for **seven days** and never more than **1 000 runs** per schedule; a queued or executing run is never pruned. Deleting the schedule deletes its history.

### Where this is tested

`crates/mako-control-plane` schedule tests (cron parsing and every refusal above, the lease, overlap skipping, missed-slot collapse, history retention), `services/mako-control-plane/src/function_schedule_http.rs`, `crates/mako-edge-gateway` (scheduler header stripping and the internal invoke route), and `packages/cli/test/schedules.test.mjs`.

---

## Database webhooks

Signed HTTP deliveries when documents change. An authorized member registers an HTTPS endpoint for an environment, subscribed to the `insert`, `update`, and `delete` events of chosen collections. The control plane consumes the environment's committed change log, writes one *delivery* per subscribed change into a durable outbox, and posts each with an HMAC-SHA256 signature. Deliveries are retried with backoff for a bounded window, kept in order per document, and logged with their response status; an endpoint that keeps failing is paused and the pause is shown, never silently dropped. Nothing here touches the data plane's write path: a slow endpoint slows its own deliveries and nothing else.

### Registering an endpoint

`POST …/webhooks` with an idempotency key:

```json
{
  "url": "https://hooks.example.com/mako",
  "description": "Order sync",
  "subscriptions": [
    { "collectionId": "orders", "events": ["insert", "update", "delete"] },
    { "collectionId": "customers", "events": ["update"] }
  ]
}
```

- `url` is an absolute `https` URL without credentials, at most 2048 bytes. Plain `http` is admitted only to loopback and only on deployments that are not staging or production.
- `subscriptions` names 1 to 32 distinct collections, each with 1 to 3 distinct events. Every collection must exist; one that does not is `404 not_found`.
- `enabled` defaults to `true`.

The response is `201` with the endpoint (`id`, `url`, `description`, `subscriptions`, `state`, `enabled`, `pausedReason`, `pausedAt`, `consecutiveFailures`, `secretVersion`, timestamps) and its **signing secret** (`whsec_` + 64 hex characters), shown exactly once. The secret is generated by the platform, stored sealed, and never listed or logged. It can only be rotated: `POST …/webhooks/{webhookId}/actions/rotate-secret` returns a new secret, advances `secretVersion`, and signs every delivery attempted from then on with the new secret only. Deliveries in flight with the old version are not re-signed, so keep the old secret until the delivery log shows no pending deliveries with the previous `x-mako-secret-version`.

Registration records a **cursor per subscribed collection at the environment's current committed position**, so only changes committed after registration are delivered. Changing `subscriptions` starts cursors for new collections the same way and drops the cursors of removed ones.

```bash
mako-cloud webhooks create --url https://hooks.example.com/mako --subscribe orders --subscribe customers:update --secret-file ./whsec.txt
mako-cloud webhooks list
mako-cloud webhooks deliveries whk_… --state failed
mako-cloud webhooks redeliver whk_… whd_…
mako-cloud webhooks resume whk_…
mako-cloud webhooks rotate-secret whk_… --yes
```

Any member of the team may read; a role that can change projects may write. Mutations are audited as `webhook_endpoint_create`, `_update`, `_delete`, `_rotate_secret`, `_resume`, or `_redeliver`.

### The delivery

Each delivery is one `POST` to the endpoint's URL with these headers:

| Header | Value |
| --- | --- |
| `content-type` | `application/json` |
| `user-agent` | `mako-cloud-webhooks/1` |
| `x-mako-webhook-id` | the endpoint id, `whk_…` |
| `x-mako-delivery-id` | the delivery id, `whd_…`; a redelivery has a new one |
| `x-mako-event` | `insert`, `update`, or `delete` |
| `x-mako-secret-version` | the `secretVersion` whose secret signed this delivery |
| `x-mako-signature` | `t=<unix seconds>,v1=<lowercase hex HMAC-SHA256>` |

and this body, in exactly this field order:

```json
{
  "id": "whd_7h2k9m4x1p6q3w8r",
  "event": "update",
  "collection": "orders",
  "documentId": "ord_10042",
  "revision": "3-9f1c",
  "previousRevision": "2-a77e",
  "commitPosition": 18234,
  "occurredAt": "2026-08-29T10:05:12Z",
  "projectId": "prj_example0001",
  "environmentId": "env_example0001",
  "redeliveryOf": null
}
```

- `previousRevision` is `null` for an insert. A delete carries the tombstone revision. `commitPosition` is the environment-wide commit sequence, strictly increasing per document, so a consumer that stores the last position it applied per document can discard a stale duplicate.
- `occurredAt` is when the platform observed the change; a redelivery keeps its original's `occurredAt` and names it in `redeliveryOf`.
- **Document fields are never included.** A delivery says *what* changed and *which revision* it became. Read the document through the API with a credential entitled to it.

The endpoint has 10 seconds to answer. Any `2xx` counts as delivered; anything else is a failed attempt.

#### Verifying the signature

The signed string is the timestamp from the header, a dot, and the raw request body: `<t>.<body>`. The key is the signing secret exactly as shown, as UTF-8 bytes (including the `whsec_` prefix). Compare in constant time and reject timestamps outside a tolerance you choose (five minutes is customary).

```js
import { createHmac, timingSafeEqual } from "node:crypto";

// `rawBody` must be the request body bytes exactly as received.
export function verifyMakoWebhook(rawBody, headers, secret, toleranceSeconds = 300) {
  const header = headers["x-mako-signature"] ?? "";
  const parts = Object.fromEntries(header.split(",").map((part) => part.split("=", 2)));
  const timestamp = Number.parseInt(parts.t ?? "", 10);
  if (!Number.isFinite(timestamp) || typeof parts.v1 !== "string") return false;
  if (Math.abs(Date.now() / 1000 - timestamp) > toleranceSeconds) return false;
  const expected = createHmac("sha256", secret).update(`${timestamp}.`).update(rawBody).digest("hex");
  const provided = Buffer.from(parts.v1, "hex");
  return provided.length === 32 && timingSafeEqual(Buffer.from(expected, "hex"), provided);
}
```

Worked example: with secret `whsec_test`, timestamp `1700000000`, and body `{"id":"whd_x"}`, the signed string is `1700000000.{"id":"whd_x"}` and the header is `t=1700000000,v1=<hex of HMAC-SHA256("whsec_test", '1700000000.{"id":"whd_x"}')>`. Verify the signature before parsing the body, and always against the raw bytes: a re-serialized body signs differently.

### Retries, backoff, and the retry window

A delivery is `pending` from the moment it is queued until it is `delivered` (a `2xx`) or `failed`. After a failed attempt the delivery records `lastError` — `connect_refused`, `timeout`, `tls`, `invalid_url`, `invalid_response`, or `status_<code>` — and `lastResponseStatus` when the endpoint answered, then waits `min(5 s × 2^attempts, 15 min)` plus up to a quarter of that at random: 10 s, 20 s, 40 s, … capped at 15 minutes from the eighth attempt. Retries continue for **24 hours from the first attempt**; a delivery still failing after that is marked `failed` and left in the log for you to redeliver.

Deliveries are **in order per document**: a later delivery for the same document waits while an earlier one is still pending, so an endpoint that is down for an hour receives that document's changes in commit order once it recovers. Deliveries for different documents do not wait on each other. One endpoint is attempted at most 50 deliveries per two-second pass.

The outbox survives restarts: a delivery is durable before the cursor that produced it advances (one atomic write), and its state only moves forward after the endpoint answered. Delivery is therefore at least once; the delivery id and `commitPosition` let a consumer make it exactly once.

### Pause and resume

After **20 consecutive failed attempts** across an endpoint's deliveries the platform pauses it: `state` becomes `paused`, `pausedReason` is `sustained_failure`, and `pausedAt` says when. A paused endpoint receives no deliveries and queues no new ones, but nothing is dropped: pending deliveries stay pending, and changes committed meanwhile wait in the change log at the endpoint's cursors. Any successful delivery resets `consecutiveFailures` to zero.

`POST …/webhooks/{webhookId}/actions/resume` clears the pause and the counter and makes every pending delivery due immediately; intake continues from the cursors. Nothing already `failed` is revived — redeliver those individually. Resume is refused (`409`) on a disabled endpoint.

Disabling an endpoint (`PATCH` with `"enabled": false`, state `disabled`) also stops intake and delivery. Re-enabling it starts over: every cursor moves to the current committed position, so changes made while it was disabled are not delivered, and deliveries that were still pending are marked `failed` with `lastError` `disabled`.

### The delivery log and redelivery

`GET …/webhooks/{webhookId}/deliveries` lists deliveries newest first, up to `limit` (default 50, at most 200) per page, continued with `cursor`, optionally narrowed with `state=pending|delivered|failed`. Each entry carries the event, collection, document id, revision, commit position, state, attempt count, next attempt time, last response status, last error, what it is a redelivery of, and when it was created and delivered.

`POST …/webhooks/{webhookId}/deliveries/{deliveryId}/actions/redeliver` queues a new delivery of the same event — a new id, `attempts` at zero, `redeliveryOf` naming the original — and answers `202`. Redelivery is refused (`409`) while the endpoint is paused or disabled.

### Retention

Delivered and failed deliveries are kept for **7 days**, and never more than **2000 per endpoint**. Pending deliveries are never removed by retention. Deleting an endpoint removes its subscriptions, cursors, pending deliveries, and its whole log.

### What is never included

Document fields, in any event; the signing secret, after the one response that shows it; and changes committed before the endpoint (or a new subscription) was registered, or while it was disabled.

### Where this is tested

`crates/mako-control-plane/src/webhook.rs` (intake, signing, backoff, ordering, pause and resume, retention), `crates/mako-smoke/tests/webhooks.rs` (a real endpoint receiving and verifying signed deliveries against the running planes), `examples/rational/test-live/alerts.spec.ts` (a receiver verifying the signature as described here), and `packages/cli/test/webhooks.test.mjs`.

---

## Application mail

Mail an environment sends to its *application users*: address verification, password recovery, invitations, and magic sign-in links. Developer mail (the wait-list, verification, and recovery messages for developer accounts) is a separate path; application mail shares its relay, its encryption key, and its outbox discipline, and nothing else.

### How a message flows

1. **The data plane writes an intent.** When an application user asks for a magic link, verifies an address, recovers a password, or is invited, the data plane does not send mail. It writes a *mail intent* in the environment's keyspace — an id (`aml_<32 hex>`), the tenant ids, the kind, the recipient, and a small map of variables — and holds the only copy until the control plane has taken it.
2. **The control plane drains intents** over an internal route on its mail worker thread (lease 60 seconds, at most 32 per pass). Drained intents stay leased on the data plane; a lease that expires without an acknowledgement is handed out again.
3. **Each intent becomes an outbox record.** The worker resolves the project and environment names, picks the environment's template for the kind (or the built-in default), renders it, builds a plain-text envelope, seals it, and stores it in the control plane's application mail outbox under a must-be-absent write, so an intent drained twice produces one record. Intake is at-least-once from the data plane and exactly-once into the outbox.
4. **Acknowledge.** Once its records are durable the worker acknowledges the ids and the data plane forgets them. If the data plane cannot be reached, the intents stay leased and the next pass drains them again — nothing is lost and nothing is sent twice.
5. **Deliver.** A delivery pass leases each record, decrypts the envelope, hands it to the SMTP transport with the intent id as the message id, and marks it delivered. A transient failure retries with exponential backoff (30 s doubling, capped); a permanent failure or the attempt limit dead-letters the record with a stable error code.

Some intents can never become mail: the environment no longer exists, the recipient is not an address, or the template would not render. Those are recorded as dead letters at intake (`environment_not_found`, `invalid_recipient`, `invalid_template`, `invalid_envelope`) and acknowledged, so the record explains why.

### Templates

Every environment has four templates, one per kind: `verification`, `recovery`, `invitation`, and `magic_link`. Each is **plain text**: a one-line subject (1–200 bytes) and a text body (1 byte–32 KiB) with `{{variable}}` placeholders. The renderer never interprets markup, and mail is delivered as `text/plain` exactly as rendered, so a template cannot carry script or remote content.

Templates are validated when they are saved or previewed, never at delivery: the body must include `{{link}}`, since delivering it is what each kind of mail is for; only the kind's allowed variables may appear (an unknown `{{name}}` is refused with a message listing what is allowed); every brace must belong to a well-formed placeholder; placeholder names are lowercase letters and underscores, with whitespace inside the braces tolerated (`{{ link }}`).

Rendering substitutes only allowlisted variables. A variable the data plane did not send renders as empty text. Values are sanitized for where they land: control characters never reach the subject line, and only line breaks and tabs survive in the body, so a recipient-controlled value cannot inject headers. A subject that renders blank falls back to the default; one past 200 bytes is cut at a character boundary.

| Variable | Kinds | Meaning |
| --- | --- | --- |
| `link` | all | The single-use link the user opens |
| `expires_at` | all | When the link stops working, RFC 3339 |
| `email` | all | The recipient address |
| `project_name` | all | The project's display name |
| `environment_name` | all | The environment's display name |
| `inviter` | `invitation` | Who sent the invitation; may be empty |

A kind that has not been customized uses a built-in default and reports `isDefault: true`, `version: 0`, `updatedAt: null`. Default subjects: `Verify your email for {{project_name}}`, `Reset your {{project_name}} password`, `You are invited to {{project_name}}`, `Your sign-in link for {{project_name}}`. Every default body greets the reader, names the project and environment, carries `{{link}}` on its own line, states `{{expires_at}}`, and tells a reader who did not ask for the mail to ignore it.

### Managing templates

Templates are environment-scoped management resources. Any member of the owning team may read and preview them; a role that can change projects may save or reset them. Reads and writes are audited as `email_template_read` and `email_template_update`.

| Operation | Route |
| --- | --- |
| `listEmailTemplates` | `GET …/email-templates` — all four kinds |
| `getEmailTemplate` | `GET …/email-templates/{templateKind}` |
| `updateEmailTemplate` | `PUT …/email-templates/{templateKind}` with `{subject, textBody}` and `Idempotency-Key`; the version advances on every save |
| `resetEmailTemplate` | `DELETE …/email-templates/{templateKind}` — back to the default |
| `previewEmailTemplate` | `POST …/email-templates/{templateKind}/actions/preview` with an optional `{subject?, textBody?}` |

A preview renders with placeholder values (`https://app.example.com/...`, `person@example.com`, `2030-01-01T12:00:00Z`, `A teammate`) and the real project and environment names. Without a body it renders the template in effect; with one it renders the unsaved text. Invalid text is refused with a 400 whose message is the same one a save would give.

```bash
mako-cloud email-templates list
mako-cloud email-templates set magic_link --subject "Sign in to {{project_name}}" --body @magic-link.txt
mako-cloud email-templates preview magic_link
mako-cloud email-templates reset magic_link
```

### Where this is tested

`crates/mako-control-plane/src/email_template.rs` (kinds, defaults, validation, rendering, authorization) and `application_mail.rs` (drain, render, seal, acknowledge, deliver, dead-letter), `services/mako-control-plane/src/email_template_http.rs`, and `packages/cli/test/email-templates.test.mjs`. The deployment-side configuration (relay, encryption key, retention) is in the [Dev Book](dev-book.md#application-and-developer-mail).

---

## Allowed origins (CORS)

A browser refuses to hand a cross-origin response to the page that asked for it unless the server names that page's origin back. An environment carries the list of origins that may be named: the browser applications allowed to call its application API.

The list belongs to the **environment**, not to a hostname, so it applies everywhere that environment's API is served — on the platform's own hostname and on every [custom domain](#custom-domains) the environment is verified for. An application therefore needs no domain of its own before its pages can call its own API.

### Reading and setting the list

```text
GET  …/allowed-origins
PUT  …/allowed-origins        { "allowedOrigins": ["https://app.example.com", "http://127.0.0.1:5173"] }
```

`PUT` replaces the list whole (with an idempotency key). Any member of the project's team reads the list; a member who may mutate projects sets it. Both are audited (`allowed_origins_read`, `allowed_origins_update`). An environment that has never set a list allows no origin; an empty list means the same and is how cross-origin access is withdrawn.

### What an origin may be

An origin is **exact**: `scheme://host` or `scheme://host:port`, with a lowercase host and nothing else — no path, no trailing slash, no wildcard, no userinfo, no query or fragment. `https` is required except for a loopback host (`localhost`, a name under it, or an address in `127.0.0.0/8`), which may use `http` so a development server on your own machine can be listed. Origins must be unique, at most 262 characters each, and an environment lists **at most 16**. Anything else is refused with `400 invalid_request` naming the rule it broke.

The control plane stores the list and installs it into the data plane, which answers browsers from it; the edge gateway receives it with the route it resolves for a function. Both refuse an origin they could not match byte for byte.

### What the platform emits, and when

| Request | Answer |
| --- | --- |
| `OPTIONS` on an application route, listed `Origin` | `204` with `Access-Control-Allow-Origin: <the origin>`, `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers`, `Access-Control-Max-Age: 600`, `Vary: Origin` |
| Any other method on an application route, listed `Origin` | the route's own answer, plus `Access-Control-Allow-Origin: <the origin>`, `Access-Control-Expose-Headers`, `Vary: Origin` |
| Unlisted or absent `Origin` | the route's own answer, unchanged |
| Anything on a management, operator, developer-workspace, or `/service/` route | the route's own answer, unchanged, **whatever** the environment allows |

The emitted values are the platform's: `Access-Control-Allow-Methods: GET, POST, PUT, PATCH, DELETE, OPTIONS`; `Access-Control-Allow-Headers: authorization, content-type, x-mako-key, idempotency-key, if-none-match, if-match`; `Access-Control-Expose-Headers: etag, x-mako-request-id, content-type`; `Access-Control-Max-Age: 600`. `Access-Control-Allow-Credentials` is never sent and `*` is never sent: an application carries its session in the `Authorization` header and its public key in `X-Mako-Key`, so cookies are not part of the exchange. A failure is labelled like a success: an application that cannot read a `401` cannot react to it.

**Application routes** are the ones an application calls with its own session or public key: `…/auth/…`, `…/collections/…` (documents and replication), and `…/storage/…`. Edge functions are covered too, on both shapes: `/{projectRef}/functions/v1/{name}` on the platform hostname and `/functions/v1/{name}` on a custom domain. The gateway answers a preflight from a listed origin itself; an `OPTIONS` that is not such a preflight still reaches the function.

### The topology this is for

The platform serves an API, not static files. A browser application is served from **its own hostname** — a static host, or `http://127.0.0.1:5173` while it is being written — and calls the project's API on the platform hostname or on the project's custom domain. That is two origins, so every call is cross-origin, and it works exactly when the application's origin is in the environment's list.

Two things worth stating plainly: an origin that is not listed is refused by the **browser**, not by the platform — the request may still reach the API and is authorized on its own merits, so the allowlist is a browser-safety mechanism and never an authorization one; and the list is per environment, so an application's development origin can be listed in a development environment without ever being listed in production.

### Where this is tested

`crates/mako-service-runtime` CORS middleware tests, `services/mako-data-plane` and `mako-edge-gateway` route tests for listed and unlisted origins, `packages/cli/test/allowed-origins.test.mjs`, and `apps/console/test-e2e/allowed-origins.spec.ts`.

---

## Custom domains

Serve a project's application API and functions on your own hostname, with a certificate the platform obtains and renews, so applications never expose the platform's hostname. An authorized member adds a hostname to a project for one of its environments, proves control of it with a DNS `TXT` record, and the platform verifies the record, issues a certificate on the first request, and serves the environment's API and functions on the name. Nothing is served on a hostname before it is verified, and serving stops when the proof goes away.

### Adding a domain

`POST /v1/projects/{projectId}/domains` with an idempotency key and `{ "hostname": "api.example.com", "environmentId": "env_…" }`. Domains are **project-level** resources authorized like project settings: any member of the project's team reads them, a member who may mutate projects adds, verifies, and removes them. The environment named must belong to the project.

The response is the domain in state `pending` with the record to publish:

```json
{
  "id": "dom_…", "projectId": "prj_…", "environmentId": "env_…", "hostname": "api.example.com",
  "state": "pending",
  "verification": { "recordName": "_mako-verify.api.example.com", "recordType": "TXT",
                    "recordValue": "mako-domain-verify=<32 random characters>" },
  "verifiedAt": null, "lastCheckedAt": null, "lastError": null, "createdAt": "…", "updatedAt": "…"
}
```

A hostname is a lowercase DNS name of at least two labels (each 1–63 letters, digits, or hyphens, not starting or ending with a hyphen), at most 253 characters, normalized without a trailing dot. IP addresses, `localhost`, and the platform's own public hostname or anything under it are refused with `400 invalid_request`. A hostname belongs to **at most one project** on the deployment: adding one that any project already claims, verified or not, is `409 conflict`. A project may hold at most 20 domains.

```bash
mako-cloud domains add --hostname api.example.com --env env_… -p prj_…     # prints the TXT record
mako-cloud domains verify dom_… -p prj_…
mako-cloud domains list -p prj_…
mako-cloud domains remove dom_… -p prj_… --yes
```

### The verification record

Publish a `TXT` record at `verification.recordName` (`_mako-verify.<hostname>`) whose value is exactly `verification.recordValue`. Other `TXT` records at the same name are ignored; a `CNAME` that resolves to the record works too. Keep it published, because verification is re-checked for as long as the domain exists.

Separately, point the hostname itself at the platform's address (an `A` or `AAAA` record, or a `CNAME` to the platform's public hostname). The verification record alone does not route traffic, and the certificate can only be issued once the name reaches the platform.

### Verification: the check, its cadence, and the two-check rule

A control-plane worker looks every domain's record up **once a minute**; `POST …/domains/{domainId}/actions/verify` runs the same check immediately and returns the domain with the outcome. Each check records `lastCheckedAt` and, when the record was not found, `lastError`:

| Check found | `pending` domain | `verified` domain | `failed` domain |
| --- | --- | --- | --- |
| The value | becomes `verified`; `verifiedAt` set once, `lastError` cleared | stays `verified` | becomes `verified` again |
| No `TXT` record (`record_missing`) | stays `pending` | one strike; **two consecutive** strikes make it `failed` | stays `failed` |
| Other values only (`record_mismatch`) | stays `pending` | one strike, as above | stays `failed` |
| No answer (`dns_unavailable`) | no change | no change, no strike | no change |

A `verified` domain becomes `failed` — and serving stops — only after two checks in a row that answered and did not find the record. One miss is a strike, not a revocation, so a resolver hiccup, a propagating change, or a single bad answer never takes a production hostname down; a check the resolver could not answer at all is neither a strike nor a reset. A `failed` domain keeps its `verifiedAt` for the record and is served again as soon as the record is back.

### What a domain serves, and what it never serves

A verified hostname serves exactly two things for its environment: the **application data-plane routes** — `/v1/projects/{projectId}/environments/{environmentId}/...` for auth, documents, replication, and storage, with the same paths, keys, and tokens as on the platform hostname — and **function invocations** as `/functions/v1/{functionName}`, without the project reference, because the hostname names the environment.

Everything else answers `404` on a custom domain: the console, the management API, the operator API, the developer workspace routes, the service-credential routes, and the platform's function path shape. A request on a custom domain whose path names a different project or environment than the one the hostname is verified for is `404 not_found`. Serving is gated at every hop: the reverse proxy marks the custom-domain listener, the data plane checks the hostname against the verified list the control plane installs for each environment, and the edge gateway checks it against the route's verified list.

A domain carries no origin list of its own; which browser origins may call the API is a setting of the environment ([Allowed origins](#allowed-origins-cors)), applied wherever that API is served.

### Certificates: on-demand TLS and the ask gate

Certificates are obtained by the reverse proxy **on demand**, at the first TLS handshake for a hostname, and renewed automatically. Before it requests one, the proxy asks the control plane whether the hostname is `verified`; no certificate is ever requested for a hostname nobody verified, and a `failed` or removed domain is refused at the next handshake. On the public beta the staging certificate authority is in use, so browsers do not trust the certificates issued there; that is a property of the beta, not of custom domains.

### Removing a domain

`DELETE /v1/projects/{projectId}/domains/{domainId}` answers `204`. Serving on the hostname stops, the certificate is no longer renewed, and the hostname is free to be claimed by any project again. Create, verify, and delete are audited as `custom_domain_create`, `custom_domain_verify`, and `custom_domain_delete`.

### Where this is tested

`crates/mako-smoke/tests/custom_domains.rs` drives the whole lifecycle against the real control plane with a loopback DNS stub (`mako_smoke::DnsStub`): publish the record, verify, clear it, and watch re-verification fail after two checks. `packages/cli/test/domains.test.mjs` and `apps/console/test-e2e/custom-domains.spec.ts` cover the CLI and console.

---

## Managing application users

Authorized project members may search, invite, create, disable, restore, update the metadata of, revoke the sessions of, and delete application users. These actions are audited without password hashes or tokens.

```bash
mako-cloud users search --query alice@example.com        # bounded; no credential material
mako-cloud users invite --email alice@example.com        # the user finishes signing up themselves
mako-cloud users create --email bot@example.com          # created directly, no invitation
mako-cloud users get usr_…                               # status, trusted and profile metadata, sessions
mako-cloud users update-metadata usr_… --input '{"trustedMetadata":{"role":"editor","households":{"hh_1":"owner"}},"profileMetadata":{"displayName":"Alice"}}'
mako-cloud users revoke-session usr_… ses_… --yes
mako-cloud users revoke-sessions usr_… --yes
mako-cloud users disable usr_… --yes                     # sessions stop working until restored
mako-cloud users restore usr_…
mako-cloud users delete usr_… --yes
```

- A user's `status` is `pending_verification`, `active`, `disabled`, or `deleted`. Sign-up produces `pending_verification` when the environment requires email verification.
- An invitation creates a pending user and mails the `invitation` template with a single-use link, valid for seven days, to the environment's **first registered redirect URL** with `#password_reset_token=<token>`. The invitee chooses their password through `password-recovery/redeem`, exactly as a recovery does, which activates them and signs them in; `{{inviter}}` is the inviting developer's email. An environment with no redirect URL refuses the invitation with `409` and creates no user, so register one under **Auth providers** first. A user with no password yet -- invited, or created with `users create` -- is refused at password sign-in like a wrong password.
- `update-metadata` replaces **both** documents whole: `trustedMetadata` (what policies read as `identity.role` and `claims.*`) and `profileMetadata` (user-editable, never a policy input). A trusted-metadata change advances the user's authorization epoch, so the next token carries the new claims and replicating clients run their security reset. From an edge function, prefer the merge-patch [service route](#trusted-metadata-from-an-applications-function), which changes one key at a time and can be guarded with an expected epoch.
- Revoking sessions, disabling, deleting, or a password recovery publishes an ordered invalidation to every gateway; a client holding a revoked session sees `unauthenticated` on its next request and must sign in again.
- A user view lists at most 100 sessions (`sessionsTruncated` says whether there were more); a search returns at most 100 results.

Authentication outcomes for the environment — sign-ins, refusals, provider and magic-link results, revocations — are readable as sanitized events with `mako-cloud observability auth-events`.

---

## The data workspace

The environment workspace groups overview, the data explorer, sync diagnostics, policies, backups, API & Connect, and settings under `/projects/{projectId}/environments/{environmentId}/…`. Navigation is permission-filtered, and switching environment destroys the current explorer capability, snapshot cursor, draft, and selected document before loading the next scope.

### The explorer

Open **Data** to load the first collection automatically, or use the collection rail to search and switch collections. Personal and team projects use the same direct browsing flow. There is no access form, application-user selection, or reason prompt. The **Documents** tab offers browsing and primary-key lookup; **Query editor** holds predicates, sorting, and index planning; **Import / export** holds bulk jobs when enabled. Results show up to six document fields alongside the primary key, revision, and state. Missing fields, JSON null, and false remain distinct. **View JSON** opens the complete document and its conditional editor. **New document** opens a blank editor. Changes still require an explicit submission, schema validation, and the expected revision for updates and deletes.

Switching collections or environments clears results, drafts, and grants. A late response from the previous scope cannot repopulate the screen.

The console obtains a collection-scoped access grant in memory and renews it automatically when needed. Project data-administration permission is required, which currently belongs to personal-project owners and team owners or administrators. Browsing in the console shows project documents independently of application users' document policies. Those policies still apply to application traffic. Every document operation remains audited, with the standard console access reason recorded as a hash.

The API and CLI also support **policy preview**, which evaluates reads, queries, and simulations as a selected active application user and cannot commit writes. This is an explicit testing option through those tools, not a prerequisite for console browsing. CLI administrative access still accepts an explicit reason.

Grants last at most five minutes and stay in memory. Never put an `x-mako-explorer-capability` value in a URL, browser storage, logs, telemetry, error reports, or support tickets. From the CLI, `mako-cloud explorer …` issues a grant for the one call, performs it, and revokes the grant afterwards.

- **Browse** is canonical primary-key order over a stable snapshot. It omits deleted documents unless retained tombstones are explicitly requested with history permission. Policy-hidden documents do not affect returned counts or cursor behavior.
- **Query** planning accepts at most 16 predicates, four sort fields, and 200 rows; only a matching active index may execute, and the server returns the required-index shape rather than falling back to a scan (`mako-cloud explorer plan` shows it).
- **History** lists a document's retained revisions and tombstones.
- **Simulate** parses and validates the proposed JSON, active schema, expected revision, and current policy without writing. **Mutate** (administrative) uses the same conditional mutation path as application traffic; a revision conflict is presented as original/proposed/current and is never merged or retried automatically.

### Import and export

The only bulk format is UTF-8 **JSON Lines**, one JSON object per non-empty line. Imports require an immutable digest-verified upload and a dry run before confirmation; conflict strategies are `create_only`, `update_existing`, and `upsert`. Each row is conditionally idempotent; cancellation stops future work and does not roll back committed rows. Exports read one consistent snapshot and become downloadable only after their manifest and digest are finalized; partial artifacts are never served.

```bash
mako-cloud data export --collection transactions --output ./transactions.jsonl
mako-cloud data import --collection transactions --input ./transactions.jsonl --strategy upsert   # upload, dry run, confirm
mako-cloud data jobs list
mako-cloud data jobs get job_… --wait
mako-cloud data jobs cancel job_… --yes
```

Limits are 1 MiB per document, 512 MiB per upload or output, four active jobs per tenant, one hour of execution, 24 hours of artifact retention, and five minutes per upload/download grant. Job progress reports exact processed, committed, failed, skipped, exported, and byte counts. Object-store outages defer cleanup or job execution rather than silently publishing incomplete output.

### API & Connect and sync diagnostics

The Connect page (`mako-cloud workspace connect`) shows the public endpoint, active public credential id, active collection/schema versions, the supported RxDB range (`>=17.0.0 <18.0.0`), and template version 1. Public credential values are one-time material and cannot be recovered later. The connection check (`mako-cloud workspace check`) accepts public metadata only and returns separate DNS, TLS, route, readiness, key-recognition, schema, client-version, and replication-route steps; it never signs in an application user or invokes pull/push.

Sync diagnostics (`mako-cloud sync summary`) contain bounded aggregates for pull/push, live streams, lag, conflicts, policy denials, throttling, checkpoint expiry, stream gaps, resync, schema mismatch, and coarse client-compatibility classes. They never return raw user, device, session, IP, token, or document ids.

### Backups and isolated recovery

Developer backup inventory (`mako-cloud backups list`) includes only tenant-verified manifests and safe recovery-point, verification, retention, drill, and objective fields; physical paths, hosts, credentials, signing material, and other tenants are excluded.

A **restore request** (`mako-cloud backups restore-requests create`) requires current backup-read and restore permissions plus a password verification from the current developer session within five minutes. It creates a **new isolated environment** from the chosen backup and is quota bounded. Overwrite and promotion are always prohibited: a restore never replaces the live environment. Access to the restored environment remains disabled until tenant isolation, storage verification, service readiness, and recovery validation succeed; `restore-requests list` shows the verification state.

### Where this is tested

`crates/mako-control-plane` explorer, data-job, and workspace tests; `services/mako-data-plane/src/explorer_http.rs`; `packages/cli/test/data.test.mjs`; `apps/console/test-e2e/developer-data-workspace.spec.ts`; `apps/console/test-e2e/database-console.spec.ts`; `services/mako-control-plane/src/workspace_http.rs`; and the workspace-summary assertions in `crates/mako-smoke/tests/file_storage.rs`. The operator-side incident procedure is the Dev Book's [developer data workspace runbook](dev-book.md#runbook-developer-data-workspace-incident).

---

## Observability for your project

Every environment exposes bounded, tenant-scoped signals through the management API (`…/observability/*`), the console's **Observability**, **Logs**, **Usage**, and **Activity** screens, and `mako-cloud observability …`. Each query takes `from`, `until`, `limit`, and a `cursor`, and answers a page of records.

| Signal | Command | What it holds |
| --- | --- | --- |
| Usage | `mako-cloud usage` / `mako-cloud observability usage` | Retained usage samples per resource. **Flows** (requests, bytes per month, invocations) sum their records; **levels** (stored bytes, users) average their samples |
| Quotas | `mako-cloud observability quotas` | Consumption against each enforced limit, with `retryAfter` when work was throttled |
| Health | `mako-cloud observability health` | Regional data-plane service status and sanitized diagnostics |
| Replication errors | `mako-cloud observability replication-errors` | RxDB replication failures with retry guidance and correlation identifiers |
| Auth events | `mako-cloud observability auth-events` | Sanitized application authentication outcomes — sign-in, refusal, provider and magic-link results, revocations — without credentials |
| Function metrics | `mako-cloud observability function-metrics` | Invocation, error, latency, and compute counts per function version and region |
| Logs | `mako-cloud logs` / `mako-cloud observability logs` | Retained, scrubbed log lines from functions, the data plane, and sync, newest first |
| Index state | `mako-cloud observability index-state` | Index build transitions per collection index |
| Audit | `mako-cloud activity` / `mako-cloud observability audit` | Append-only administration history for the environment |

### Function logs

A function's printed output is collected off the request path: the control plane reads each deployed function's runtime buffer on a short cadence and carries new lines into the retained telemetry store, where they are served by the logs endpoint with the same retention and tenant scoping as every other signal. Because log text is written by your code, it is **scrubbed** before storage — at the store itself, so no producer can bypass it. The scrub masks configured secrets, bearer and JWT values, password and cookie assignments, platform credential formats, and email addresses (the local part is masked, the domain kept). It is best-effort by design: it removes token-, password-, and email-shaped text, not every possible secret, so applications should still avoid printing sensitive values.

### What never appears

Actor, request, trace, session, document, raw URL, email, token, and secret values never become metric labels. Request and trace identifiers belong in the correlation fields of logs and errors, where you can quote them to support. Sync diagnostics and health never carry raw user, device, session, IP, token, or document ids.

### Retention windows

Telemetry is retained for ninety days on production deployments; function schedule run history and webhook delivery logs have their own seven-day windows ([schedules](#run-history), [webhooks](#retention)).

---

## Plans, quotas, and billing

### The beta charges nobody

The beta measures what every tenant uses, shows each team the bill that use would imply, and keeps a balance that may go negative. Nothing is collected: every bill response carries `collectable: false` and says in words that no charge will be made, and a CI guard (`npm run validate:no-collection`) proves that no payment-provider integration exists and that nothing which enforces limits can read the balance. Payment collection, stored payment methods, provider webhooks, dunning, and suspension for non-payment do not exist.

### Meters

Usage is observed in the plane that serves the work and shipped to the telemetry service, buffered and bounded so a telemetry outage costs reporting rather than availability.

| Resource | Where it is observed | How it aggregates |
| --- | --- | --- |
| `storage_bytes` | Sampled by the data plane when writes mark a tenant due | Average of samples |
| `application_users` | Sampled on signup and administrative lifecycle | Average of samples |
| `replication_requests_per_minute`, `replication_bytes_per_month` | Emitted where the gateway charges the request | Sum of records |
| `edge_invocations_per_month` | Emitted by the edge gateway's audit sink, admitted invocations only | Sum of records |
| `object_storage_bytes` | Bytes held in an environment's buckets, sampled like `storage_bytes` | Average of samples |
| `object_egress_bytes_per_month` | Bytes served by object downloads, per download request | Sum of records |

Flows sum; levels average. Twelve samples of the same nine stored gigabytes are one overage, not twelve, and a mid-period delete halves the storage charge. The usage ledger is cross-checked against the gateway's quota counters minute by minute; a material difference is reported as a degraded health record for the tenant, so billing and enforcement cannot silently drift apart.

### Plans

Two plans exist. An **included amount** is what the price covers. On the **free** plan exceeding it is refused (`429 quota_exceeded`, `retry: never`); on **pro** the excess is billed and never blocked, because capping there would stop a customer at the moment they started paying more.

| Resource | Free (capped) | Pro (overage billed) |
| --- | ---: | ---: |
| Document storage (`storage_bytes`) | 500 MiB | 8 GiB |
| Replication bytes per month | 5 GiB | 250 GiB |
| Object storage (`object_storage_bytes`) | 1 GiB ($0.02/GiB-month over) | 50 GiB |
| Object egress per month | 5 GiB ($0.09/GiB over) | 250 GiB |
| Application users | 50 000 | 100 000 |
| Edge invocations per month | 500 000 | 2 000 000 |

A team records its plan (`free` by default). The only way onto another plan in the beta is an audited operator action, which also reinstalls the limits every one of the team's environments is held to. Operator plan exceptions ("ignore the plan for this resource") replace as a set, expire, and apply everywhere entitlements are read — enforcement and the bill move together.

### Platform rate limits

Every plan carries these and no plan removes them; they exist so one customer's burst cannot become everyone's outage. A refusal is `429 rate_limited` with `retry: after_delay`.

| Resource | Limit per tenant |
| --- | --- |
| Authentication requests | 600 per minute |
| Document requests | 10 000 per minute |
| Document bytes | 50 MiB per minute |
| Replication requests | 120 per minute |
| Replication bytes | 64 MiB per minute |

Function invocations, public invocations, request bytes, and egress requests and bytes are metered and quota-governed through the same gateway engine.

### The bill

`GET /v1/teams/{teamId}/bill` (`mako-cloud teams bill org_… [--period YYYY-MM]`) rates the current calendar month so far against the team's effective plan and the rate card, whose prices were verified against supabase.com/pricing on 2026-08-25. Money is integer micro-dollars end to end; only the display divides. Credits are operator-granted, exactly-once per credit id, and the balance is credits minus all charges — every closed period plus the live month — unclamped. The console renders shared totals on **Usage and plan** at `/usage-and-plan`, with the non-payable notice ahead of any number.

A period that spanned a plan change is rated stretch by stretch under the plan that held during it: the base fee and a flow's included allowance take the stretch's share of the period, while a level compares its stretch average against the full included level and prorates the charge by time held. Use under a free stretch stays uncharged. Time before the team existed is covered by the free plan's zero-priced terms, which prorates a mid-month signup's base fee by construction.

Each project's **Billing** page at `/projects/{projectId}/billing` reads `GET /v1/projects/{projectId}/bill`, also available as `mako-cloud projects bill <project-id>`. It shows that project's current-month usage across all environments and its allocated usage costs. Each resource's shared usage charge is divided in proportion to project quantities, with integer rounding in project-id order. Flows sum their records; levels average samples per environment, then sum environments. Plan changes weight level quantities by time. The base subscription fee, credits, and balance remain on the overall page. This allocation explains the shared bill; it does not create separate subscriptions or invoices. The response reports retention gaps and refuses incomplete bounded reads. Project billing is a read-only view and does not close invoices.

Ended months are **closed** on any bill read while their evidence is still inside the ninety-day telemetry retention: the period is derived, stored as an invoice exactly once, and never rewritten. `?period=YYYY-MM` serves a closed month's invoice, marked `finalized` with the instant it closed; a month whose usage evidence had partly aged out says so instead of pretending.

### Where this is tested

`crates/mako-billing` (catalog, entitlements, exceptions, the rate card, proration, period close), `crates/mako-gateway/src/quota.rs` (windows, hard limits, retry advice), `crates/mako-smoke/tests/telemetry_pipeline.rs` (metering end to end), and `scripts/validate-no-collection.js`.

---

## The public API and SDKs

### The contract

[`api/openapi/mako-cloud-v1.yaml`](../api/openapi/mako-cloud-v1.yaml) is the authoritative versioned HTTP contract. It covers management, application auth, documents, RxDB replication, functions, observability, the developer workspace, and the isolated operator surface, tagged `Management`, `Auth`, `Documents`, `Replication`, `Functions`, `Operator`, `Explorer`, and `Developer Workspace`. Generated TypeScript types live in `packages/api-types/src/generated/schema.ts`.

Direct RocksDB access, MongoDB drivers, SQL, and arbitrary unindexed document scans are not public interfaces. The console uses the same management contract and authorization outcomes as automation.

### Identity domains on the wire

| Routes | Credential |
| --- | --- |
| Management (`/v1/teams`, `/v1/projects`, `…/environments/{e}/*` administration, observability, workspace) | `Authorization: Bearer` developer session or automation token |
| Application auth, documents, replication, storage (`…/auth/*`, `…/collections/*`, `…/storage/*`) | `X-Mako-Key` public project key, plus an application-user bearer token where the route requires one |
| `…/service/*` | `X-Mako-Service-Key` scoped service credential and `X-Mako-Request-Id`; loopback only |
| Function invocation (`/{projectRef}/functions/v1/*`) | Application-user bearer token unless the function is public |
| `/v1/developer-auth/*` | Same-origin hosted registration, verification, session, recovery, and wait-list status; wait-list tokens use a dedicated audience rejected by management routes |
| `/v1/operator/*`, `/v1/operator-auth/*` | A separate operator identity, never reachable through an application or developer token |

### Errors, retries, and idempotency

Public failures use the versioned `ApiErrorEnvelope` ([shape and codes](#errors-and-retries)). Mutating operations that declare the OpenAPI `Idempotency-Key` parameter should reuse one stable key when retrying an ambiguous timeout; a changed payload with the same key is a conflict. RxDB pushes additionally persist per-row mutation outcomes, so a dropped response cannot create a second revision.

### SDKs

| Package | For | Notes |
| --- | --- | --- |
| [`@mako-cloud/rxdb`](../packages/rxdb-client/README.md) | Application code (browser, mobile, Node) | Install from the [built release archive](#install-and-configure). Auth, replication, storage. Carries only a public key and an application-user session |
| `@mako-cloud/edge-sdk` | Edge functions | Supplied by the runtime; never installed. Caller-aware client plus the explicit service client |
| `@mako-cloud/management-sdk` | Scripts, CI, tools | `createManagementClient({ endpoint, credential: { kind, accessToken } })` → `MakoManagementClient` with one typed method per management operation (`accessToken` may be a string or a provider function); `createDeveloperAuthClient` for registration and sessions; `createOperatorClient` for operator inventory. Errors are `ManagementApiError` carrying the envelope |
| `@mako-cloud/api-types` | Anyone generating a client | The generated OpenAPI types and a minimal fetch client (`createMakoApiClient`) |
| `@mako-cloud/cli` | Terminals and CI | The `mako-cloud` command, built on the management SDK |

```ts
import { createManagementClient } from "@mako-cloud/management-sdk";

const mako = createManagementClient({
  endpoint: "https://cloud-test.makodb.com",
  credential: { kind: "automation_token", accessToken: process.env.MAKO_TOKEN! },   // or "developer_session"
});
const projects = await mako.listProjects();
const collections = await mako.listCollections(projectId, environmentId);
```

`npm run generate:api` regenerates the types from the OpenAPI document and `npm run generate:api:check` proves they match; `packages/management-sdk/test/client.test.mjs` checks the public and operator operation inventory against the document, so an operation cannot exist on the wire without a typed method.

### Where the authoritative state lives

The portal and operator API persist identity, wait-list, team, project, incident, audit, and function-metadata state in a server-side control database; application users, credentials, documents, policies, indexes, and replication state live in the data plane. This is an implementation boundary, not a public SQL API. When the data plane is unavailable, management authentication and control-owned resources remain available while tenant-data operations return their scoped `unavailable` error.

---

## Sample applications

### local-first

[`examples/local-first`](../examples/local-first/README.md) is the reference RxDB integration: a self-contained browser to-do application using the real `@mako-cloud/rxdb` auth, pull, push, SSE, checkpoint, and signal adapters. It ships two backends behind one seam: `FakeMakoBackend`, an in-browser implementation of the public protocol that lets the suite test offline behavior without a server, and `LiveMakoBackend`, which points the same application at running service binaries.

```sh
npm install
npm run build --workspace @mako-cloud/rxdb
npm run dev --workspace @mako-cloud/example-local-first      # Go offline, add a todo, Go online
npx playwright install chromium
npm run test:browser --workspace @mako-cloud/example-local-first
cargo build --workspace --bins && npm run test:browser-live --workspace @mako-cloud/example-local-first
```

The six scenarios verify that a write remains queryable offline and is pushed after reconnect; concurrent local and remote edits invoke the conflict handler; a remote tombstone removes the local document; an expiring access token is refreshed; a broken SSE stream reconnects and emits `resync`; and access revocation clears protected local state and requires authentication. The live variant serves the app from an origin that also proxies `/v1` to the data plane — the same topology a deployment gets from its reverse proxy.

### Rational

[`examples/rational`](../examples/rational/README.md) is a Monarch-style household money manager built on nothing but what Mako Cloud offers a developer: an ordinary project, its ordinary API URL, document policies, RxDB replication, file storage, and three edge functions. It is published from a repository of its own (<https://github.com/shuaimu/rational>), and the site GitHub Pages serves from it talks to a real project on the public beta. How close it comes to Monarch is measured feature by feature in [`MONARCH-PARITY.md`](../examples/rational/MONARCH-PARITY.md), which `npm run validate:rational-parity` refuses under ninety percent.

It exists for two reasons, and the second is the important one: to show the platform is enough to build a product on, and to find out where it is not. Every gap Rational hits is recorded in [`PLATFORM-FINDINGS.md`](../examples/rational/PLATFORM-FINDINGS.md) as symptom → platform change → regression test and fixed **in the platform**, never worked around in the application — forty-nine findings so far, all closed. Several were things no test could have found without a real application asking: a project's ordinary API URL emitted no CORS (#9); a deployed function could not import the SDK (#10), could not learn who called it (#26), and on the beta could not reach the platform API (#28); `allow_net: []` meant *any host* (#18); a collection could not be walked at all (#33); the scheduler's headers identified a run but authenticated nothing (#32).

| Rational | Platform capability |
| --- | --- |
| Sign in by password, provider, or magic link | [Application authentication](#application-authentication), [providers](#sign-in-providers-and-magic-links) |
| Households shared by several people, with roles | Trusted claims set by the app's own function ([service route](#trusted-metadata-from-an-applications-function)) |
| Accounts, transactions, categories, budgets | Collections, [policies](#document-policies), [replication](#building-a-local-first-app-with-rxdb) |
| One database per household on the device | The replication [filter](#replicating-one-slice-of-a-collection) |
| Every collection live at once | The [environment-scoped stream](#one-stream-for-many-collections) |
| Receipts attached to a transaction | [File storage](#application-file-storage) with object attributes a bucket rule reads |
| Invitations only the invitee may read | `identity.email` and `identity.email_verified` in [policies](#scoping-a-document-to-an-address) |
| Membership changes | The `households` [edge function](#edge-functions) under a service credential |
| A bank connection that syncs itself | The `institution-sync` function on a [schedule](#scheduled-functions), talking to Plaid through the [declared-egress allowlist](#what-a-function-may-do) |
| Filing, duplicate-checking, and net worth overnight | The `nightly` function on a 02:00 UTC schedule |
| Being told about a large charge or an overrun | Alerts decided server-side, delivered in-app as documents and outward by a [signed webhook](#database-webhooks) |
| Working offline | Dexie storage, durable checkpoints, and the [security reset](#authorization-epoch-security-reset) |
| Calling the API from a static host | [Allowed origins](#allowed-origins-cors) |

```bash
npm run test:browser -w @mako-cloud/example-rational   # every screen, against the in-browser fake
npm run test:unit -w @mako-cloud/example-rational      # the pure functions
npm run test:rational-smoke                            # the model itself, over HTTP, no browser
npm run validate:rational-parity                       # the Monarch parity matrix
```

Against a real stack, `examples/rational/scripts/bootstrap.mjs` creates the project, environment, collections, indexes, policies, bucket, and public key through the CLI, writes `mako.env.json`, and with `--functions` issues the service credential, installs it as a function secret, and deploys the `households`, `institution-sync`, and `nightly` functions — the last two on schedules, each holding a run key its schedule carries so a public request cannot start one. Its README documents the model (twelve collections at schema version 3, one under the thirteen the open-source RxDB build opens per page), the policies, the households function's five routes, and every test suite. It is dressed by the platform's design system, `@mako-cloud/ui`, the same kit the console uses ([Dev Book](dev-book.md#the-design-system)).

---

## Troubleshooting

| Symptom | Likely cause | What to do |
| --- | --- | --- |
| Every request answers `403 permission_denied` on a new collection | No active policy — collections are default deny | Draft, validate, and activate a policy ([Document policies](#document-policies)) |
| `409 conflict` on a document write with "idempotency key" in the message | `Idempotency-Key` differs from the body's `mutationId` | Send one value in both places |
| `409 conflict` naming a reused request id | Two data-plane requests carried the same `X-Mako-Request-Id` | Derive a distinct id per call (`${requestId}r`, `${requestId}w`); do not retry |
| `409 checkpoint_expired` on pull | The checkpoint's history has been compacted away | Treat as a full resync: clear the collection, new replication identifier, pull from the beginning |
| `409 schema_mismatch` | The client's `schemaVersion` is not the collection's active one | Run the application's migration and create replication bound to the required version |
| The browser blocks the request with a CORS error | The page's origin is not in the environment's allowed origins, or the route is a management route | List the exact origin (`mako-cloud allowed-origins set --origin …`); management and operator routes are never answered cross-origin |
| RxDB looks connected but never syncs | A refused credential is being retried, or a dozen live streams are queued behind the browser's connection limit | Stop replication on `authentication_required` and sign in again; use `MakoLiveStreamGroup` for many collections |
| The client signs the user out during a network outage | A custom persistence or client treats a transient failure as a refusal | Use `MakoAuthClient`: only a definitive refusal clears the session; transient failures set `refreshUnavailable` |
| A user's new role does not take effect | Claims live on the token | The write advances the authorization epoch; refresh the session and expect a security reset |
| A query is refused with a required-index shape | No active index matches the equality predicates and sort | Create the index the response describes and wait for `active` |
| `503 function administration is unavailable` on deploy | No reachable, credentialed S3 object store for bundles | Configure the object store the control plane names; `mako-local-bootstrap` uses an in-memory one and proves nothing about yours |
| A function's `fetch` fails with `Requires net access to …` | Egress is deny-by-default | Declare the host with `--allow-host api.example.com` (HTTPS, port 443 only) |
| A served function gets `connection refused` reaching the data plane | Containers cannot reach the host's loopback through `host.docker.internal` | Bind the data plane to a reachable address, or use `--network pasta:--map-host-loopback,169.254.1.2` with rootless Podman |
| `unresolved_import` at upload | A bare specifier other than `@mako-cloud/edge-sdk` | Map it with `--dependency <specifier>=<path>` onto an uploaded module |
| A schedule cannot be created (`409 conflict`) | The function has no active deployment | Deploy and promote first |
| A schedule shows `skipped_overlap` runs | The function outlives its interval | Lengthen the interval or shorten the function; overlap is never run concurrently |
| Webhook endpoint is `paused` | 20 consecutive failed attempts | Fix the receiver, then `mako-cloud webhooks resume`; redeliver `failed` deliveries individually |
| Webhook signatures fail to verify | The body was re-serialized before signing, or the wrong `secretVersion` | Sign the raw bytes; keep the old secret until no pending deliveries carry the previous version |
| A custom domain flips to `failed` | Two consecutive checks found no TXT record | Restore `_mako-verify.<hostname>`; it is served again on the next successful check |
| `429 quota_exceeded` with `retry: never` | A free-plan cap | Reduce use or move to a plan that bills overage |
| `429 rate_limited` | A platform rate window | Wait `retry.afterMs` |
| `mako-cloud` exits with code 7 | The credential file is readable by others | Fix permissions (`0600`) or set `MAKO_CONFIG_DIR` to a private directory |
| `mako-cloud` exits with code 3 in CI | No credential, or a step-up action without a terminal | Set `MAKO_TOKEN`/`MAKO_ENDPOINT`; for step-up, `MAKO_STEP_UP_PASSWORD_FILE` |

When you contact support, quote the `requestId` from the error envelope (or `request <id>` in the CLI's output) — never a token, a document, or a secret.

---

## Limits at a glance

| Area | Limit |
| --- | --- |
| Request body (JSON API) | 1 MiB default per request |
| Document | 1 MiB |
| Trusted or profile metadata | 64 KiB, nesting depth 16 |
| Query predicates / sort fields / page | 16 / 16 / 1000 (explorer: 16 / 4 / 200) |
| Pull or push batch | 1000 rows |
| Live-stream buffer | 1000 events, then `resync` |
| Refresh family lifetime / concurrency grace | 30 days / 5 s |
| Credential rotation overlap | ≤ 30 days |
| Policy rule expression | 16 KiB |
| Bucket object | 16 MiB ceiling; object attributes ≤ 8, values ≤ 128 bytes; path ≤ 512 bytes |
| Function bundle | 10 MiB; ≤ 512 files; ≤ 256 dependency mappings |
| Function configuration | 1–16 regions, ≤ 64 secret names, ≤ 8 allowed hosts |
| Local serve wall-time ceiling | 300 000 ms |
| Schedules per function / history | 100 / 7 days and 1000 runs; request body 64 KiB, ≤ 16 headers |
| Webhook subscriptions / URL / log | 32 collections / 2048 bytes / 7 days and 2000 deliveries; retry window 24 h; pause after 20 failures |
| Sign-in providers / redirect URLs / magic-link TTL | 16 / 32 / 60–3600 s |
| Allowed origins | 16 per environment, ≤ 262 characters each |
| Custom domains | 20 per project; one project per hostname |
| Email template | subject 200 bytes, body 32 KiB |
| Data jobs | 512 MiB per artifact, 4 active per tenant, 1 h execution, 24 h retention, 5 min grants |
| Explorer grant | ≤ 300 s |
| User search / sessions listed | 100 / 100 |
| Team name / project name / environment name | 120 / 120 / 100 characters |

---

## Glossary

- **Application user** — a person who uses your application; authenticated per project and environment.
- **Authorization epoch** — a counter per environment and per user that advances when policies or trusted claims change; tokens and checkpoints are bound to it, and a mismatch triggers a client security reset.
- **Automation token** — a team-scoped, permission-bounded credential for scripts and CI.
- **Checkpoint** — a signed opaque token (`mcp1.…`) that says where a client's replication stands.
- **Collection** — a set of JSON documents sharing a versioned schema and primary key.
- **Deployment / version** — an immutable, health-checked build of a function; exactly one is active.
- **Developer** — a person building on Mako Cloud; owns teams and projects.
- **Environment** — an isolated unit inside a project holding collections, users, keys, functions, and settings.
- **Explorer grant** — a short-lived capability for reading or writing documents from the console or CLI.
- **Operator** — Mako Cloud platform staff with a separate identity.
- **Personal projects**: projects owned by your account.
- **Policy** — allow/deny rules deciding every document operation; default deny.
- **Project reference** — `{projectId}--{environmentId}`, the prefix functions are invoked under.
- **Public project key** — `mako_pk.…`, a client identifier safe to ship.
- **Service credential** — `mako_sk.…`, a scoped secret for trusted server-side code; bypasses policy within its scope with an audit record.
- **Tombstone** — a document delivered as `_deleted: true`; also how a document that left the caller's view is removed locally.
- **Trusted claims / trusted metadata** — administrator-controlled per-user data that becomes the token's claims and the policy's `claims.*`.
