# Sign-in providers and magic links

Applications can let their users sign in the ways users expect: with Google,
GitHub, or any OpenID Connect provider, and with a single-use link sent by
email. Both paths end in the same application-user session that password
sign-in issues ([project-auth.md](project-auth.md)), and both are configured
per environment through the management API, the console, or `mako
auth-settings`. The settings live in the data plane that mints application
sessions; the control plane keeps no copy and forwards each authorized read
or replacement to it.

## Configuring an environment

The environment's settings are one document, replaced whole:

```json
{
  "providers": [
    {
      "name": "google",
      "kind": { "type": "oidc", "issuer": "https://accounts.google.com" },
      "clientId": "1234.apps.googleusercontent.com",
      "clientSecret": "GOCSPX-…",
      "scopes": [],
      "enabled": true
    },
    {
      "name": "github",
      "kind": { "type": "git_hub" },
      "clientId": "Iv1.abcdef",
      "clientSecret": "…",
      "enabled": true
    },
    {
      "name": "okta-acme",
      "kind": { "type": "oidc", "issuer": "https://acme.okta.com" },
      "clientId": "0oa…",
      "clientSecret": "…",
      "scopes": ["groups"],
      "enabled": false
    }
  ],
  "redirectUrls": ["https://app.example.com/auth/callback", "http://localhost:5173/auth/callback"],
  "magicLinks": { "enabled": true, "linkTtlSeconds": 900 }
}
```

- `providers` (at most 16): each has a `name` (lowercase letters, digits,
  hyphens; 2–64 characters) that appears in the application's URLs, a `kind`
  — `oidc` with the issuer whose `/.well-known/openid-configuration` the data
  plane discovers, or `git_hub` for GitHub's OAuth 2.0 — the `clientId` the
  provider issued, the `clientSecret`, optional extra `scopes` beyond `openid
  email profile` (ignored for GitHub), and `enabled`. A disabled provider is
  kept but refuses every flow.
- `redirectUrls` (at most 32): where a provider callback or a magic link may
  send the browser. Absolute `https` URLs, or `http` to `localhost` or a
  loopback address for local development; no fragment, no credentials.
  Matched **exactly** — never by prefix — so register every page that
  finishes a sign-in.
- `magicLinks`: whether passwordless sign-in by email is on, and how long a
  link lives (60–3600 seconds).

Register the provider's side with the callback URL
`https://<your api origin>/v1/projects/{projectId}/environments/{environmentId}/auth/providers/{name}/callback`.

`PUT /v1/projects/{p}/environments/{e}/auth-settings` (`updateAuthSettings`,
with an `Idempotency-Key`) installs the document and answers with the
installed view; `GET …/auth-settings` (`getAuthSettings`) reads it. Owners
and administrators may replace the settings; the read needs the same
credential-reading permission as the environment's keys. Every replacement
advances `version` by one; the management API numbers the new version after
the one it read, so a client never has to track it. From a terminal:

```bash
mako auth-settings get -p prj_… -e env_…
mako auth-settings set -p prj_… -e env_… --input @auth-settings.json
```

### Secrets are sealed and never returned

A `clientSecret` is a value, not a reference: the control plane seals it with
XChaCha20-Poly1305 under a key both planes derive from the deployment's
shared internal secret, bound to the project, environment, and provider name
so a sealed secret cannot be moved to another environment or provider, and
only then sends it to the data plane. Neither plane ever returns it. The
installed view shows `hasSecret` per provider instead.

Because the document is replaced whole, a provider given **without**
`clientSecret` keeps the secret already installed under its name — so
reading the settings, editing a client id or flipping `enabled`, and writing
them back is safe. A provider that has no installed secret and is given none
is refused with `invalid_request`, before anything is sent to the data plane.
Removing a provider from the document removes its secret with it.

## The browser flow

The application never sees a provider token, and no token ever travels in a
URL:

1. **Start.** With its public project key, the application calls
   `POST …/auth/providers/{name}/start` (`startProviderSignIn`) with the
   `redirectUrl` it wants the browser back at — one of the registered ones —
   and receives the provider's `authorizationUrl`. It navigates the browser
   there. The request is refused when the provider is not enabled or the
   redirect is not registered.
2. **Callback.** The provider sends the browser to
   `GET …/auth/providers/{name}/callback?state=…&code=…`
   (`completeProviderSignIn`). Nothing on this request is trusted until the
   signed `state` verifies as issued by this environment for this provider
   and not yet expired. The data plane then redeems the code with the
   provider, fetches the identity, and requires a **verified email**: the
   application user is matched by provider and subject first, then by
   verified email, and created when neither matches. The identity is linked
   to the user by provider subject — never by email alone — so a later
   sign-in with the same account finds the same user even if the email
   changes at the provider.
3. **Redirect with a one-time code.** The browser is sent (`302`) to the
   registered redirect with a short-lived, single-use code in the URL
   fragment: `https://app.example.com/auth/callback#code=…`. A refusal —
   the provider declined, the exchange failed, the email is not verified,
   the user is disabled — arrives the same way as `#error=…`, so the
   application can show it. Fragments are not sent to servers and do not
   land in logs.
4. **Exchange.** The application calls `POST …/auth/providers/exchange`
   (`exchangeProviderSignIn`) with `{ "code": … }` and its public project
   key and receives the same `AuthSession` (`accessToken`, `refreshToken`,
   `expiresIn`, `user`) password sign-in returns. The code is spent on first
   use and expires after two minutes.

Every outcome — verified, refused by the provider, exchange failed, user
disabled, code redeemed — is recorded as an authentication event visible in
the environment's observability screens and `mako auth-events`.

## Magic links

With `magicLinks.enabled`:

1. `POST …/auth/magic-link` (`requestMagicLink`) with `{ "email", "redirectUrl" }`
   answers `202 { "accepted": true }` for any well-formed address, so the
   endpoint reveals nothing about who is registered. A magic link is also
   how a new user signs up: an address without a user gets one, pending
   until the link proves the address, and redemption activates it. The data
   plane writes a mail intent that the control plane's mail worker renders
   with the environment's `magic_link` template (or the built-in default)
   and sends; the link points at the registered redirect with a single-use
   token. A disabled or deleted user's address is accepted identically and
   mails nothing.
2. The application reads the token from the link and calls
   `POST …/auth/magic-link/redeem` (`redeemMagicLink`) with `{ "token" }` to
   receive an `AuthSession`.

A link is bound to the environment and the email it was sent to, expires
after `linkTtlSeconds`, and is spent on first use: a second redemption, or
one after expiry, is refused with `unauthenticated` and no session is
issued.

## Local development

Outside production the data plane also speaks plain HTTP to loopback
providers, so a stub standing in for Google or GitHub can be exercised by the
smoke suite; `http://localhost:…` and `http://127.0.0.1:…` redirects are
admitted for the same reason. In production only `https` redirects and
providers are reachable.

## Tested evidence

- `services/mako-control-plane/src/auth_settings_http.rs`: the management
  route seals a supplied secret for the tenant and provider, sends an empty
  seal for an omitted one, refuses a provider with nothing to keep, numbers
  the version after the installed one, and validates the document before
  forwarding.
- `services/mako-data-plane/src/auth_provider_http.rs`: installation,
  keep-secret substitution, the start/callback/exchange flow, and magic-link
  request and redemption.
- `packages/management-sdk/test/client.test.mjs` and
  `packages/cli/test/auth-settings.test.mjs`: `getAuthSettings`,
  `updateAuthSettings`, `mako auth-settings get|set`, and that a secret is
  sent once and printed never.
