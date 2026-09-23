# Rational

Rational is a Monarch-style money-management application for households — sign-in by every
method the environment offers, households shared under roles, accounts of every class,
transactions with merchants, splits, transfers, and receipts, category groups, rules, budgets
in two modes, recurring bills on a calendar, goals that save up or pay down, investments, cash
flow with a Sankey and a treemap, notifications, and a dashboard that borrows every number from
the screen that owns it — built only on what Mako Cloud offers applications. It is the
platform's standing proof that those capabilities hold up together: every feature maps to a
named platform capability, every platform defect it exposes is recorded in
[`PLATFORM-FINDINGS.md`](PLATFORM-FINDINGS.md) and fixed in the platform, never worked
around here, and how close it comes to Monarch is written down feature by feature in
[`MONARCH-PARITY.md`](MONARCH-PARITY.md), where a validator refuses anything under ninety
percent.

The app is React 19 + Vite on RxDB 17 with Dexie (IndexedDB) storage, replicating through
`@mako-cloud/rxdb`. It is local-first: reads come from the device, writes queue while
offline and push on reconnect, and a reload or an offline restart finds the household's
data where it left it. Its look comes from the platform's design system, `@mako-cloud/ui`
(`packages/ui`, described in the [Dev Book](../../docs/dev-book.md#the-design-system)): tokens
with a light and a dark palette that follow the device or the theme toggle, Radix-based
components, Lucide icons, Inter, and Recharts charts; the Sankey, the treemap, and the month
calendar are its own SVG. The published repository carries the kit's sources under `src/kit`,
so it depends on nothing that is not on npm.

## Run it

Without a server the app runs against an in-browser fake of the public Mako protocol,
seeded with a demo household:

```sh
npm install
npm run build --workspace @mako-cloud/rxdb
npm run dev --workspace @mako-cloud/example-rational
```

Create an account on the sign-in screen (any email, 8+ character password) and the demo
household appears.

Against a real environment, bootstrap the tenant once, then run the dev server, which
proxies `/v1` to the data plane so the browser talks same-origin (the data plane sends no
CORS headers, and a deployment fronts both behind one reverse proxy anyway):

```sh
# a local stack: see docs/dev-book.md#local-development, then
cargo run --bin mako-local-bootstrap            # once, with the services stopped
# in examples/rational, signed in with `mako-cloud auth login` or with MAKO_TOKEN set:
node scripts/bootstrap.mjs --endpoint http://127.0.0.1:8081 --data-endpoint http://127.0.0.1:8080
node scripts/seed.mjs                            # the demo household as owner@rational.test
npm run dev
```

`scripts/bootstrap.mjs` drives the `mako-cloud` CLI and is idempotent — rerunning reuses what it
finds, and publishes a higher schema version of a collection when the model moved. Its flags:

| flag | default | does |
| ---- | ------- | ---- |
| `--endpoint` | `MAKO_ENDPOINT` | management API |
| `--token`, `--token-kind` | `MAKO_TOKEN`, `developer_session` | developer credential; without it the CLI's stored profile (`--profile`, `--config-dir`) is used |
| `--data-endpoint` | `http://127.0.0.1:8080` | the data plane the app and the seed talk to; written to `mako.env.json` |
| `--project-name`, `--env-name`, `--region`, `--team` | `Rational`, `development`, `local`, personal space | which project and environment to create or reuse |
| `--output` | `examples/rational/mako.env.json` | where the app's tenant, function endpoint, and sign-in settings are written |
| `--household-id`, `--household-name`, `--currency` | `hh_demo`, `Demo household`, `USD` | the seeded household |
| `--owner-email/-password`, `--editor-email/-password` | `owner@rational.test`, `editor@rational.test`, `RationalDemo1!` | demo application users with owner and editor claims |
| `--skip-users` | off | stop after the tenant is configured |
| `--functions` | off | deploy the three functions with their own service credentials and secrets |
| `--functions-endpoint` | `http://127.0.0.1:8082` | the edge gateway the app calls functions through; written to `mako.env.json` |
| `--function-name` | `households` | the function name to deploy to |
| `--cli` | `node_modules/.bin/mako-cloud` | the CLI to run |

It publishes every collection of `mako/collections.json` with its indexes, activates the
policies in `mako/policies/`, creates the `receipts` bucket from `mako/buckets/receipts.json`,
initializes the signing key, issues a public key, reads the environment's sign-in settings,
optionally deploys the functions, registers the two demo users, writes their household claims
as trusted metadata, and seeds the `households` and `memberships` documents under a service
credential it retires afterwards — the same writes the function makes under its own credential,
so an environment without a function runtime still has a working demo household.

`mako.env.json` is what the app is compiled against:

```json
{
  "endpoint": "http://127.0.0.1:8080",
  "projectId": "prj_…",
  "environmentId": "env_…",
  "publicProjectKey": "mako_pk.…",
  "functionsEndpoint": "http://127.0.0.1:8082",
  "signIn": { "providers": [{ "name": "acme", "enabled": true }], "magicLinks": true }
}
```

`signIn` comes from `mako-cloud auth-settings get`: every provider the environment knows, with
whether it is enabled, and whether magic links are on. The sign-in screen renders a button per
provider and shows a provider that is switched off as "not enabled for this environment"
rather than failing when it is pressed; without the file (or against an environment that
answers nothing) the app offers password sign-in only. `functionsEndpoint` is `null` when no
function was deployed, and the household screens then say membership cannot be changed here.

`scripts/seed.mjs` signs in as an application user and pushes a deterministic demo household
(seven accounts including a house and a brokerage with holdings, fourteen category groups,
twelve categories, four tags, sixteen merchants, about two hundred transactions across three
months with paired transfers and three synced ones awaiting review, budgets for three months,
four confirmed recurring bills, three goals, one rule, and ninety nights of net-worth
snapshots) through the replication API. It is a no-op once the household holds accounts
(`--force` to push again).

## Tests

```sh
npm run test:unit -w @mako-cloud/example-rational            # node --test over the built selectors and engines
npm run test:browser -w @mako-cloud/example-rational         # Playwright against the in-browser fake
npm run test:browser-live -w @mako-cloud/example-rational    # Playwright against real binaries
npm run validate:rational-parity                             # the parity matrix, from the repository root
```

The unit tests cover the pure selectors and the shared engines: balances with holdings, net
worth, cash flow over a range with its exclusions, the Sankey's conservation, the treemap's
areas, the budget month in both modes, bills paid and late, goal projections, rules with every
condition and action, transfer suggestions, merchant resolution, the query codec, CSV export,
the chart layouts, the nightly job's decisions, and the alert rules — and they time the heavy
ones over fifty thousand transactions (finding #45). The wire-mocked suite covers every screen:
sign-in by password, provider, and magic link; onboarding into a space of one's own; households
created, invited to, and left; the dashboard against the screens it borrows from; accounts,
tracked values, hidden accounts, holdings; transactions searched, filtered, bulk-edited, paired
as transfers, reviewed, and exported; budgets rolling over, budgeted by group, in flex mode, and
copied; cash flow over a quarter with its exclusions and its charts; recurring charges confirmed,
paused, marked by hand, and on the calendar; goals of both kinds; categories seeded, reordered,
and deleted with their transactions moved; merchants renamed and merged; rules in order; the
settings pages; imports; receipts; offline writes; live remote edits with a conflict; and a
security reset.

The live suite builds nothing itself: it needs `cargo build --workspace --bins`
(or `MAKO_SMOKE_BINARY_DIR`) and a built CLI, then seeds a throwaway stack with
`mako-local-bootstrap`, starts the data plane and the control plane, mints a developer
session, runs `scripts/bootstrap.mjs` and `scripts/seed.mjs` against them, serves the app
through the Vite proxy, and drives persistent browser contexts: a reload keeps the data and
pulls only new changes, an offline edit pushes after reconnect and reaches another member,
conflicting edits on two devices resolve identically, and an offline restart reads from
IndexedDB.

The households function needs more than that — the pinned edge runtime in a container, the
edge gateway, and the fixed ports the gateway compiles in — so it is opt-in and the
`test-live/households.spec.ts` story skips without it:

```sh
MAKO_RUN_EDGE_RUNTIME_TESTS=1 MAKO_EDGE_TEST_ENGINE=podman \
  XDG_DATA_HOME=/var/tmp/mako-containers XDG_RUNTIME_DIR=/run/user/$(id -u) \
  npm run test:browser-live -w @mako-cloud/example-rational
```

With it, the setup starts an object store on 8333 (a function bundle is uploaded to one, and
the in-memory store `mako-local-bootstrap` uses is not on that path), the runtime supervisor on
9000, the data plane on 8080, the control plane on 8081, and the gateway on 8082, deploys the
function with `--functions`, and the spec proves the whole membership path against the real
stack: an invitation is accepted, the household replicates to the new member, a viewer's write
is refused and an editor's accepted, and removing the member clears their copy of the household
while the owner keeps theirs. Those ports must be free — nothing else may be running on the
stack's ports at the time — and the two images (the pinned edge runtime and the object store)
must be pullable. Anything missing leaves the spec skipped with the reason on stderr.

## The screens

The sidebar is Monarch's: Dashboard, Accounts, Transactions, Cash Flow, Budget, Recurring,
Goals, Investments, Settings. Every screen's state a person would want back after a reload —
the account, the month, the range, every transaction filter — lives in the address.

| screen | route | shows |
| ------ | ----- | ----- |
| Dashboard | `#/dashboard` | net worth with its trend, this month against last, the budget's headroom, the next bills, goals, the review queue, unread notifications, recent transactions, investments — each figure borrowed from the screen that owns it, each card linking there; the layout is rearranged per device |
| Accounts | `#/accounts`, `#/accounts/<id>` | classes with subtotals, a net-worth chart over a range, hidden and closed accounts; a detail page with the balance history from the nightly snapshots, the account's transactions, a value update for a house or a car, holdings for a brokerage |
| Transactions | `#/transactions?…` | search and filters carried in the address, rows grouped by day with merchant, category, tags, and review, hidden, and transfer markers, bulk edit, CSV export, and a panel that edits, splits, attaches receipts, pairs a transfer, marks a bill recurring, or starts a rule |
| Cash Flow | `#/cash-flow?range=…` | income, spending, savings, and savings rate over a month, quarter, year, or custom range; monthly bars; a Sankey of income into spending; spending by group, category (as a treemap), merchant, tag, and account; a category's trend; CSV export — transfers, hidden transactions, and balance updates excluded throughout |
| Budget | `#/budget?month=…` | expected income, groups with subtotals and optional group budgets, categories with inline amounts, rollover, non-monthly shares, unbudgeted spending, what is left to budget, copy last month; in flex mode the fixed, non-monthly, and flexible buckets, with the flexible number as what income leaves |
| Recurring | `#/recurring?view=…` | suggested charges from this device and from the nightly job, upcoming bills paid, due, or late, every recurrence with its monthly total, and a calendar |
| Goals | `#/goals` | save-up goals whose progress follows linked accounts or recorded contributions, pay-down goals on a liability with a projected payoff, priorities, on-track status, contributions |
| Investments | `#/investments` | holdings across accounts with value and gain, allocation by asset class, prices updated by hand |
| Settings | `#/settings/<page>` | household (name, currency, budget mode), members, categories in groups with a default set, tags, merchants, rules, connections, import, notifications, data export |

## The model

`mako/collections.json` is the single source of the document model, at schema version 3. The
bootstrap publishes its `jsonSchema`, `primaryKey`, and `indexes` to the platform;
`src/model/collections.ts` derives the RxDB schema of every collection from the same object,
so the two cannot drift. Every document has `id` (the primary key), `household_id`,
`created_at`, and `updated_at` (unix milliseconds); deletion is RxDB's `_deleted`; amounts are
integer minor units with an ISO 4217 `currency`.

| collection | scope | holds |
| ---------- | ----- | ----- |
| `households` | directory | name, base currency, owner, budget mode |
| `memberships` | directory | `<household>.<user>` projection of a member's claim (role, status), plus `<household>.invite.<digest>` for a pending invitation; written only by the households function |
| `accounts` | household | every class from checking to real estate and crypto; opening balance and date; hidden from net worth; owner; embedded `holdings` for an investment or crypto account; closed_at |
| `transactions` | household | account, date, amount, description, merchant, category, tags, notes, embedded `splits`, receipts, review state, hidden flag, balance-update mark, the transfer that pairs two legs, rule and recurrence links |
| `taxonomy` | household | categories, tags, category groups, and merchants (`kind`); a category's group, icon, position, and budget bucket with a non-monthly target; a merchant's statement patterns |
| `rules` | household | match by statement text (contains or exactly), merchant, category, direction, amount range, account; set category and merchant, add tags, hide, mark reviewed; in priority order |
| `budgets` | household | `bud_<category or group>.<yyyy-mm>`, `bud_income.<yyyy-mm>`, `bud_flex.<yyyy-mm>`; amount, rollover, kind |
| `recurrences` | household | detected, manual, confirmed, paused, or dismissed recurring charges with what last paid them |
| `goals` | household | save-up or pay-down; target, date, linked accounts, planned monthly, priority, progress source, embedded contributions |
| `net_worth_snapshots` | household | one per day, with every account's balance that night |
| `alerts` | household | alert settings and the alerts they fire (`kind`), with `alert_kind` naming one of six rules |
| `connections` | household | institution connections, Plaid links, and CSV import batches (`kind`) |

Ten household collections plus the two directory ones is twelve open at once,
one under the thirteen the open-source RxDB build allows (`COL23`), so every
collection of the model is open from the start. Small document types share a
collection behind a `kind` discriminator — that is finding #3, a design
adjustment rather than a platform defect, and the policies of the merged kinds
are identical, so nothing about sharing changed. A collection's ids are one
namespace for the whole environment, which is why a household's default groups
and categories are `grp_<household>.<slug>` and `cat_<household>.<slug>`:
deterministic, so two devices seeding at once write the same documents, and
distinct, so two households never collide.

Every collection is indexed on `(household_id, updated_at)`; `transactions` also on
`(household_id, account_id, date)` and `(household_id, date)`, and `memberships` on
`(user_id, updated_at)` so the households function can rebuild a person's whole claim
before it writes one.

Policies (`mako/policies/*.json`) are the same shape for every household collection and read
the household role from the token's trusted claims, indexed by the document's own
`household_id`: members read (`claims.households[old.household_id] != null`), owners and
editors create, update (both states, same household), and delete. `households` may be created
by anyone signed in and changed only by the owner; `memberships` are readable by members and
have no client write rule, plus one rule that lets any signed-in person read a membership whose
`status` is `invited` — a policy cannot compare a document field with the caller's verified
email, so an invitation cannot be scoped to its invitee and the app filters by address on the
device (see the findings log). The `receipts` bucket (policy access, 8 MiB, images and PDFs) is
owner-only for now — see the note in `mako/buckets/receipts.json`.

What counts as money moving is one definition, in `functions/shared/exclusions.ts`: a
transaction that is hidden, one that is a leg of a transfer, and one that is a balance update
of a tracked account are left out of income, spending, budgets, cash flow, recurrence
detection, and alerts, everywhere at once — while an account's balance counts every one of
them, because a balance is what the bank says.

## The households function

`functions/households/index.ts` is the only writer of membership. A member's role lives in the
claims their token carries (`households: {"<id>": "owner"|"editor"|"viewer"}`), because a
document policy can read a trusted claim but cannot join a membership table — and only trusted
code may write a claim. Five routes, all `POST` under
`{functionsEndpoint}/{projectId}--{environmentId}/functions/v1/households`:

| route | body | who |
| ----- | ---- | --- |
| `/create` | `{name, currency}` | anyone signed in; becomes the household's owner |
| `/invite` | `{householdId, email, role}` | the owner |
| `/accept` | `{householdId}` | the person the invitation names |
| `/role` | `{householdId, userId, role}` | the owner |
| `/remove` | `{householdId, userId}` | the owner; never the owner themselves |

The caller comes from `createFunctionClientFromRequest(...).auth.getUser()` and the caller's
role from the same verified token's `households` claim. Every write goes through
`createServiceClient(...)` with a bypass reason naming the action: `users.setAppMetadata` moves
the claim, and the `memberships` projection is written alongside it — `<household>.<user>` for
a member, `<household>.invite.<digest of the address>` for an invitation, which is how someone
without an account yet can be invited. Because `setAppMetadata` replaces the `households` claim
whole, a change for another person is rebuilt from that projection through the
`(user_id, updated_at)` index rather than guessed.

The credential the bootstrap issues is scoped to exactly that: collections `memberships`,
`households`, and the reserved `users` target, operations `read`, `create`, and `update`. There
is deliberately no `delete`: an accepted invitation and a removed member keep their row with a
new `status`, which is both a smaller credential and a better record.

Three things about the deployment are worth knowing, and all three are in the findings log:

- The platform generates the value of a function secret and the secret of a service credential,
  and neither can be created with a value the developer chooses, so a service credential cannot
  be installed as a function secret. `--functions` therefore uploads a generated
  `credential.ts` in the copy of the directory it deploys; `functions/households/credential.ts`
  in the repository holds no secret and fails closed, and the function prefers the
  `HOUSEHOLDS_SERVICE_KEY` secret whenever that can carry a real credential.
- Membership ids use `.` rather than `:` because the service document route compares the raw
  path segment with the body's id while the edge SDK percent-encodes the path, so an id holding
  any character `encodeURIComponent` escapes cannot be written from a function at all.
- A deployed function could not import `@mako-cloud/edge-sdk` at all (#10): the validator and
  the supervisor passed the bare specifier through and Deno refused it, so the pattern the
  documentation describes was unimplementable. The SDK now travels in the main worker's module
  graph and is mapped per user worker by an inline import map. Nor did a release carry the main
  worker (#23), so a host kept whichever supervisor Ansible had copied there; the release now
  carries `runtime-main/` and the upgrade installs it.

After every membership write the app refreshes its session — the new claim only arrives on a
new token — and tells each replicated scope the epoch it now holds, so the person who just
joined opens the household and the person who was removed has their local copy of it erased.
A household that finishes its first pull with no taxonomy at all is a new one, and the app seeds
it with the default groups and categories once.

## The scheduled functions

Two functions run on their own, under their own service credentials, on schedules the
bootstrap creates with `mako-cloud schedules`:

| function | schedule | what it does |
| -------- | -------- | ------------ |
| `institution-sync` | every 15 minutes | asks a deterministic simulated institution (or Plaid) for each connected account's statement and writes what is new, keyed by `(account, external_id)` so an overlapping window never doubles a transaction; what it imports waits in the review queue; a connection whose pass fails is marked and, if the household asked, becomes a `sync_error` alert |
| `nightly` | 02:00 UTC | files uncategorized transactions with the household's own rules — every action a rule carries — marks a synced transaction that repeats a manual one, proposes repeating charges it has not proposed before, advances a confirmed bill its payment has arrived for, records the day's net worth with every account's balance, and decides the alerts the household asked for |

Everything the nightly job does is idempotent, and everything it does is reversible by the
person: a category it chose records the `rule_id` that chose it and the transactions screen
says *by &lt;rule&gt;*; a duplicate is annotated, never deleted; a detected recurrence is a
`detected` document the household still confirms or dismisses; a snapshot is one document per
household per day. A filing the person made is never overruled.

Both hold a **run key**. A scheduled invocation is anonymous — the scheduler
sends `x-mako-schedule-id`, which the platform now strips from public requests
(findings log #32), but the route itself stays public — so each function
requires `x-rational-run-key` and the bootstrap creates the schedule carrying
it. That is also how the live suite starts a night deliberately instead of
waiting until two in the morning; without the key the same request is `401`.

## Plaid

The simulated institution proves the sync machinery; Plaid Sandbox proves it against a real
aggregator, through the platform's **declared-egress allowlist** (findings log #36) rather than
any special treatment. Give the bootstrap sandbox credentials — `--plaid-client-id` and
`--plaid-secret`, or the `PLAID_CLIENT_ID` / `PLAID_SECRET` environment — and it installs them
as function secrets and deploys `institution-sync` with `--allow-host sandbox.plaid.com`, the
one external host that deployment's worker may reach. Without them, nothing changes: the
function reports itself unconfigured, the Connections page never offers the option, and every
CI-gating suite passes with no external network.

The browser's part is deliberately small: `POST /plaid/link-token` mints a Link token for the
signed-in member, Plaid's widget runs, and the short-lived public token goes straight back to
`POST /plaid/exchange`. The access token that comes out of the exchange lands only in
`plaid_items` — a collection whose policy has **no rules at all**, which under default-deny
means no application user can read or write it; the app does not even open it. What the
household sees is an ordinary `connections` document (`kind: "plaid"`), synced by the same
fifteen-minute schedule with `/transactions/sync` under a stored cursor: added entries create,
modified entries update the same `(account, external_id)` document, and an entry the
institution withdrew — a pending charge that posted — is deleted while its replacement arrives
in the same page. The cursor advances only after a page's writes commit, so a crashed pass
replays into upserts that change nothing.

The opt-in live spec (`test-live/plaid.spec.ts`) runs only when the credentials are present:
it mints a sandbox public token the way Plaid's docs suggest for tests, links a real sandbox
institution, runs the schedule twice, and proves the second run imports nothing.

## Notifications

The household says what it wants to be told about — a large transaction, a budget gone over,
an account running low, a bill due within so many days, a goal reached, a connection that
stopped syncing — and the server decides. `nightly` evaluates the household-wide ones over the
whole household; `institution-sync` evaluates the large-transaction one over what each pass
just wrote, because a big charge should not have to wait until 02:00, and raises `sync_error`
when its own pass fails. A device that is closed would never fire an alert, which is the whole
reason none of this happens in the browser.

Each alert has a derived id (`alr_<household>.<kind>.<subject>`), so a
condition still true tomorrow is the same alert and not a second one, and
whichever job sees it first is the one that writes it.

Alerts reach a person two ways. In the app they are documents like any other, counted unread
on the bell in the top bar and listed under Settings › Notifications. Outside it, the bootstrap
can register the household's webhook endpoint for the `alerts` collection with
`--alerts-webhook <url>`; the platform then signs each delivery and the signing secret is
written to a file of its own — never to `mako.env.json`, which is served to the browser. A
delivery names what changed and never what it contains, so nothing a member could not read
leaves the environment on that channel. Settings live in the same collection as fired alerts
(the thirteen-collection limit again), so an endpoint subscribed to `alerts:insert` sees setting
documents too and tells them apart by reading the document.

They share their engines with the application. `functions/shared/` holds the rules engine,
recurrence detection and bill resolution, the budget math, balances, goal projections, transfer
suggestions, merchant resolution, the exclusions, and the alert rules, and it is the same code
the browser runs — so a charge is filed the same way whether somebody clicked or the job woke
up.

A bundle is a directory and nothing outside it is uploaded, so the copy has to travel with the
function. In the repository a function imports `../shared/rules.ts`, which is where the module
really is; the bootstrap copies `shared/` into the staged bundle and rewrites that one specifier
to `./shared/rules.ts`. That seam is why the modules the app shares import nothing themselves:
the browser build wants `.js` specifiers, Deno wants `.ts`, and no single import satisfies both.

`functions/shared/nightly.ts` is the exception that proves it: the nightly job's decisions —
which transactions its rules would file, which synced ones repeat a manual one, which bills
were paid, what the day's net worth is — are pure functions of the documents the job read, and
the app never imports them. So they live beside the other engines and speak Deno's `.ts`
specifiers, and `test-unit` can exercise them anywhere, including in the published copy of this
application where the edge SDK the function itself imports does not exist.

## What maps to what

| Rational feature | platform capability |
| ---------------- | ------------------- |
| sign up, sign in, session across reloads | project auth password sign-up/sign-in, refresh, `MakoAuthClient` behind `src/auth.ts`, session kept by `BrowserAuthSessionPersistence` |
| provider sign-in, magic links | `startProviderSignIn` / `completeProviderSignIn` and `requestMagicLink` / `redeemMagicLink`, with `MakoAuthClient.signInFragment` classifying the fragment the browser lands with |
| which methods to offer | `mako-cloud auth-settings get` at bootstrap time, written into `mako.env.json` |
| households and roles | trusted app metadata `households: {<id>: role}` surfaced as token claims; document policies with `claims.households[…]`; the `households` edge function writing them through the service app-metadata route |
| membership changes taking effect | the authorization epoch the write advances, a session refresh, and `MakoAuthorizationEpochCoordinator` per scope |
| household switcher, household settings | `memberships` and `households` replicated into a per-user directory database; the owner-only update rule on `households` |
| accounts, transactions, categories, groups, merchants, tags, budgets, recurrences, goals | one platform collection each (small kinds sharing one), replicated with `replicateRxCollection` and the package's pull, push, and SSE adapters into one RxDB database per household |
| balances, net worth, cash flow, budgets, bills, goals, allocation, the dashboard | memoized selectors over local RxDB queries (`src/selectors`) and the engines in `functions/shared` |
| offline writes and reconnect | RxDB's local queue; the transport reports connectivity; `reSync()` on reconnect |
| conflicting edits | RxDB assumed-master conflicts resolved by `updated_at`, then canonical JSON (`src/data/conflict.ts`) |
| security reset | `MakoAuthorizationEpochCoordinator`: the household database is erased and re-created with a new replication identifier |
| resuming where it stopped | `DexieReplicationStatePersistence` per collection: the checkpoint the pull and the stream reached, the security state, and the recovery state, all cleared together by a reset — and by a removal, so a device that signs back in pulls the whole household again |
| schema mismatch, expired checkpoint, stream gap | `MakoReplicationRecoveryCoordinator` |
| sync status and diagnostics | `MakoReplicationSignals` plus transport counters, exposed as `window.rational.diagnostics()` |
| receipts | `receipts` storage bucket, objects attributed to the household |
| a house valued by hand, a brokerage by its holdings | fields of the account document; a balance update is a transaction marked as such |
| what the household is told overnight | the two scheduled functions under service credentials, and the webhook on `alerts` |

## Layout

- `mako/` — the model, policies, and bucket the bootstrap publishes
- `functions/households` — the edge function that owns membership
- `functions/institution-sync`, `functions/nightly` — the scheduled functions
- `functions/shared` — rules, recurrences and bills, budgets, balances, goals, transfers,
  merchants, exclusions, alerts, the nightly job's own decisions, and the run key. The modules the
  app imports import nothing at all, so the browser build and Deno can both read them;
  `nightly.ts` and `run-key.ts` are the functions' alone and speak Deno's `.ts` specifiers
- `scripts/` — `bootstrap.mjs`, `seed.mjs`, the deterministic `demo-data.mjs`, and
  `default-taxonomy.mjs`, the groups and categories a new household starts with
- `src/model` — document types and the RxDB schemas derived from `mako/collections.json`
- `src/data` — transport, conflict handler, database, replication wiring, replicated scopes with
  the coordinators, write helpers, and the application object
- `src/selectors` — pure, memoized derivations
- `src/ui` — the screens (hash router, no router dependency), built from the kit's components
  and Tailwind utilities; `src/ui/charts` — the hand-drawn Sankey, treemap, and month calendar
- `src/app.css` — the one stylesheet: it imports the design system and nothing else
- `src/testing/fake-backend.ts` — the in-browser protocol fake
- `test/`, `test-live/`, `test-unit/` — the three suites
- `MONARCH-PARITY.md` — how close this is to Monarch, feature by feature
