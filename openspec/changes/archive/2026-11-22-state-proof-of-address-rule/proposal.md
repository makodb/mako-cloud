## Why

An application user is `pending verification` until the address is proven.
Four things prove it: the mailed verification link, a magic link, a password
link (recovery or invitation), and a provider sign-in whose verified email
matches the user. `identity/project-auth` and `identity/auth-providers` say
that each of them works, but nowhere what a proof may do to a user's status.
Two defects sat in that gap and were fixed on 2026-11-22: a provider sign-in
left a pending user pending and unable to get a session (9cf7b7c), and a
verification link made a disabled or deleted user active again (9cd9d79).

The User Book gives the rule piecemeal ("redemption activates it", "A magic
link to the same address also proves it and activates the account", "a user
still pending verification becomes active") and says a disabled user's
"sessions stop working until restored", which only an administrator does.

## What Changes

A new requirement in `identity/project-auth`, "Proof of an address activates
only a pending user", with three scenarios. No code changes: every path
already behaves this way and the scenarios cite existing tests.

The report asks the whitepaper's author whether such an activation should
discard a password set before the address was proven; this requirement does
not decide that.

## Capabilities

### New Capabilities

### Modified Capabilities

- `identity/project-auth`: what a proof of address may do to a user's status.

## Impact

- `openspec/specs/identity/project-auth/spec.md`
- `docs/requirements-traceability.md`: new rows PA-21 to PA-23.
