## Why

> Supersession note (2026-08-12): `separate-operator-developer-lifecycle` preserves this
> change's shipped password sign-in evidence while replacing its active-developer eligibility and
> combined-bootstrap behavior with a shared authentication identity and independent roles.

The public-beta operator console currently requires an operator to mint, copy, and paste a short-lived file token, which is too complex for routine administration. Operators should be able to use a familiar email-and-password flow while the system preserves an explicit, separately auditable operator authorization boundary.

## What Changes

- Add same-origin operator sign-in using the email and password of a verified, security-active authentication identity that has been explicitly granted operator permissions, independent of developer admission state.
- Issue a separate, short-lived, revocable operator session after password verification; developer and wait-list sessions remain invalid on operator routes.
- Add protected operator-session inspection, renewal or bounded continuation, sign-out, revocation, rate limiting, generic authentication failures, and audit records without exposing credentials or raw sessions.
- Require recent password verification for high-risk operator actions while retaining their existing reason, permission, idempotency, and audit requirements.
- Add a protected bootstrap and lifecycle procedure for granting, changing, and revoking operator permissions without changing developer admission. The historical public-beta bootstrap of `msmummy@gmail.com` combined those effects; the successor change repairs that one side effect and prohibits it for later bootstrap operations.
- Replace the hosted console's copy-and-paste token form with an email/password operator sign-in form and a normal sign-out flow. Keep any file-token issuer as an explicitly configured, disabled-by-default break-glass mechanism rather than the routine browser login.
- **BREAKING**: The hosted beta's normal operator login workflow changes from pasted bearer tokens to password-authenticated, cookie-backed operator sessions.

## Capabilities

### New Capabilities

- `identity/operator-authentication`: Password verification, explicit operator entitlements, isolated operator sessions, step-up authentication, lifecycle controls, console behavior, and public-beta bootstrap requirements.

### Modified Capabilities

None.

## Impact

- Control-plane identity storage and RocksDB migrations for operator entitlements, password-attempt state, revocable sessions, and audit-safe lifecycle records.
- New same-origin `/v1/operator-auth/` OpenAPI routes plus authentication changes for existing `/v1/operator/` routes.
- Shared password recovery, password change, and account-wide security handling so they revoke operator sessions; developer-only lifecycle changes remain isolated.
- Console operator authentication adapter, routes, forms, session handling, error states, and end-to-end tests.
- Caddy's exact public route allowlist, security headers, request limits, and credential forwarding rules.
- Public-beta configuration, bootstrap tooling, deployment qualification, recovery and rollback runbooks, and evidence for the initial operator grant.
