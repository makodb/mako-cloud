## Why

Two requirements under `operations` say something the books contradict, and
the code follows the books.

- `operations/local-bootstrap-and-smoke`, "Local tenant bootstrap": the
  scenario "Bootstrap runs a second time" says a repeated run "reports the
  same identifiers". The Dev Book says a repeated run "rotates the credential
  and reports the new one", because a public key's secret is shown only at
  creation. The bootstrap rotates the existing public key under a new
  credential id. The requirement also omits two things the Dev Book says the
  bootstrap seeds (a signing key and a permissive development document
  policy) and the optional deployment of the sample functions to a reachable
  local runtime supervisor.
- `operations/public-beta-environment`, "Public traffic uses trusted HTTPS":
  "The only supported public beta origin SHALL be
  `https://cloud-test.makodb.com`". The User Book says the beta also serves
  developers' verified custom domains, with certificates obtained on demand,
  and the reverse-proxy template has a site for them.

## What Changes

- "Local tenant bootstrap" lists what is seeded, keeps the tenant identifiers
  stable across runs, and says a repeated run rotates the public project key
  and reports the new one. It also states the optional function deployment.
- "Public traffic uses trusted HTTPS" makes `https://cloud-test.makodb.com`
  the platform's own origin, the only one that serves the console and the
  management and operator APIs, and names verified custom domains as
  additional hostnames with on-demand certificates.

No scenario is added or renamed, so the traceability matrix does not change.
No code changes.

## Capabilities

### New Capabilities

### Modified Capabilities

- `operations/local-bootstrap-and-smoke`: a repeated bootstrap rotates the
  public key; the seeded set and the optional function deployment are stated.
- `operations/public-beta-environment`: the platform origin is distinguished
  from verified custom domains.

## Impact

- The two spec files named above.
