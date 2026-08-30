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
platform**, never worked around in the application. Thirty findings so far,
all closed. Several were things no test could have found without a real
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
| Working offline | Dexie storage, durable checkpoints, and the authorization-epoch [security reset](rxdb-client.md#authorization-epoch-security-reset) |
| Calling the API from a static host | [Allowed origins](allowed-origins.md) |

## Running it

```bash
npm install
npm run test:browser -w @mako-cloud/example-rational   # every screen, against the in-browser fake
npm run test:unit -w @mako-cloud/example-rational      # the pure functions
```

Against a real stack, `examples/rational/scripts/bootstrap.mjs` creates the
project, environment, collections, indexes, policies, bucket, and public key
through the CLI, writes `mako.env.json`, and with `--functions` issues the
service credential, installs it as a function secret, and deploys the
`households` function. `npm run dev -w @mako-cloud/example-rational` then
serves the app against it.

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
