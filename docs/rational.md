# Rational: the sample application, and what it found

Rational is a household money manager built on nothing but what Mako Cloud
offers a developer: an ordinary project, its ordinary API URL, document
policies, RxDB replication, file storage, and one edge function. It lives in
[`examples/rational`](../examples/rational/README.md), it is published from a
repository of its own at <https://github.com/shuaimu/rational>, and the site
GitHub Pages serves from it talks to a real project on the public beta.

It exists for two reasons, and the second is the important one.

The first is to show that the platform is enough to build a product on. The
second is to find out where it is not. Every gap Rational hits is recorded in
[`examples/rational/PLATFORM-FINDINGS.md`](../examples/rational/PLATFORM-FINDINGS.md)
as symptom → platform change → regression test, and is fixed **in the
platform**, never worked around in the application. Thirty-eight findings so
far, all closed. Several were things no test could have found without a real
application asking for them:

- A project's ordinary API URL emitted no CORS at all, so a browser
  application could not call its own project without owning a DNS name
  (#9).
- A deployed function could not import the SDK the documentation tells it to
  import (#10), could not be told who called it (#26), and on the beta could
  not reach the platform API at all (#28) — the documented way to write a
  function did not work in production, in three separate ways.
- Customer functions were not sandboxed as claimed: an empty Deno permission
  list grants without restriction, so `allow_net: []` meant *any host* (#18).
- A collection with more than one index could not be listed (#20), a function
  secret's value could never be corrected (#22), and a function could only
  ever be called at its bare name (#24).
- A collection could not be walked at all (#33): a query with no predicate is
  refused by design, and the one shape that can enumerate — a range over an
  index's leading field — was refused too, so a scheduled job could read only
  documents whose ids it already knew.
- The scheduler's headers identified a run but authenticated nothing (#32),
  while the documentation told a function to act on them.

## What Rational maps onto

| Rational | Platform capability |
| --- | --- |
| Sign in by password, provider, or magic link | [Project auth](project-auth.md), [sign-in providers](auth-providers.md) |
| Households shared by several people, with roles | Trusted claims, and the [service route that sets them](project-auth.md) from the app's own function |
| Accounts, transactions, categories, budgets | Collections, [document policies](document-policies.md), [RxDB replication](rxdb-client.md) |
| One database per household on the device | The replication [filter](rxdb-client.md#replicating-one-slice-of-a-collection) |
| Every collection live at once | The [environment-scoped stream](rxdb-client.md#one-stream-for-many-collections) |
| Receipts attached to a transaction | [File storage](file-storage.md) with object attributes a bucket rule reads |
| Invitations only the invitee may read | `identity.email` and `identity.email_verified` in [policies](document-policies.md#scoping-a-document-to-an-address) |
| Membership changes | The `households` [edge function](edge-functions.md) under a service credential |
| A bank connection that syncs itself | The `institution-sync` function on a [schedule](edge-functions.md), writing under a service credential without doubling a transaction |
| A real institution through Plaid Sandbox | The [declared-egress allowlist](edge-functions.md) (#36): the deployment names `sandbox.plaid.com`, the access token lives in a collection whose policy allows no application user anything, and the same fifteen-minute schedule pulls `/transactions/sync` under a cursor |
| Filing, duplicate-checking, and net worth overnight | The `nightly` function on a 02:00 UTC schedule |
| Being told about a large charge or an overrun | Alerts decided server-side, delivered in-app as documents and outward by a [signed webhook](webhooks.md) on the `alerts` collection |
| Working offline | Dexie storage, durable checkpoints, and the authorization-epoch [security reset](rxdb-client.md#authorization-epoch-security-reset) |
| Calling the API from a static host | [Allowed origins](allowed-origins.md) |

## Running it

```bash
npm install
npm run test:browser -w @mako-cloud/example-rational   # every screen, against the in-browser fake
npm run test:unit -w @mako-cloud/example-rational      # the pure functions
npm run test:rational-smoke                            # the model itself, over HTTP, no browser
```

The third is the one the beta runs. `crates/mako-smoke/tests/rational.rs` publishes the
project from `examples/rational/mako/` — the same files the bootstrap publishes — and then
walks a household's life over HTTP: three ways in, sharing by claim, an import, a rule, a
receipt, an alert that leaves by signed webhook, and writes queued while a device was away.
It needs no browser, so the hosted qualification runs it against the deployed source on every
release.

Against a real stack, `examples/rational/scripts/bootstrap.mjs` creates the
project, environment, collections, indexes, policies, bucket, and public key
through the CLI, writes `mako.env.json`, and with `--functions` issues the
service credential, installs it as a function secret, and deploys the
`households`, `institution-sync`, and `nightly` functions -- the last two on
schedules, each holding a run key its schedule carries so a public request
cannot start one. With `--alerts-webhook <url>` it also registers the
household's endpoint for the `alerts` collection and writes the signing secret
to a file of its own. `npm run dev -w @mako-cloud/example-rational` then serves the app
against it.

The two scheduled functions share their engines with the application: rules,
recurrence detection, the budget math, and the alert rules live in
`examples/rational/functions/shared/`, and the same code files a transaction
whether the person clicked or the job ran at two in the morning. A function
bundle is a directory and nothing outside it is uploaded, so the bundle
carries its own copy: a function imports `../shared/rules.ts`, where the
module really is, and the bootstrap copies `shared/` into the staged bundle
and rewrites that one specifier. That seam is why the shared modules import
nothing themselves -- the browser build resolves `.js` specifiers and Deno
resolves `.ts`, and no single import satisfies both.

A build with no `mako.env.json` — a fresh clone — runs against an in-browser
fake of the same protocol and says so in a banner. That is what a fork gets
before it has a project of its own; it is not what the published site is.

## Reading the findings log

Each row is one defect or gap, in the order it was found:

- **Found by** — the task or the moment that exposed it, because how a defect
  surfaced is usually the shortest description of what was missing.
- **Symptom** — what actually happened, in the terms the developer saw.
- **Platform change** — what was changed, and why that rather than something
  else. Where an alternative was rejected, the row says which and why.
- **Regression test** — the test that fails against the old behaviour. A row
  without one is not closed.
- **Status** — `fixed`, `open`, or a design change with its reasoning.

A few rows are not platform defects: #3 is a design adjustment (RxDB's
open-source build opens at most 13 collections per page), #15 is a
documentation gap, and #21, #27, and #29 are defects in the platform's own
tests and guards that Rational's use of them exposed — a staleness guard that
cried wolf, a suite that could not run where `docker` is podman, and two
qualification fixtures still asserting the contract from before the sandbox
was fixed.
