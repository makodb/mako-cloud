## Why

The public Mako Cloud console currently accepts only manually issued developer
session tokens, so prospective customers cannot create an account. Public
registration must not grant product access automatically while the service is
in preview; new developer identities need a durable, reviewable wait-list state
and an explicit platform-operator activation step.

## What Changes

- Add enumeration-safe public developer registration, email verification, sign
  in, sign out, recovery, and wait-list status endpoints backed by persistent
  control-plane identity state.
- Place every self-registered developer in a `waitlisted` state by default and
  issue only a narrowly scoped wait-list session until a platform operator
  activates the account.
- Deny wait-listed, rejected, disabled, or unverified identities all
  organization, project, environment, credential, data, replication, and
  function access, even if they retain or replay an older session.
- Add a public console registration and verification experience plus a
  signed-in wait-list status page that does not expose queue position or other
  applicants.
- Add a separate operator-console queue with bounded search and pagination,
  applicant details, and idempotent approve or reject actions that require a
  reason and produce immutable audit events.
- Notify applicants when verification, approval, or rejection requires an
  action, while failing closed when the configured mail dependency is absent
  or unhealthy.
- Allow an explicitly risk-accepted public preview to remain open without a
  calendar expiry for its exact release until an operator pauses it or a
  non-waivable safeguard fails.
- Preserve manually issued short-lived developer sessions only as a protected
  bootstrap and recovery mechanism; they do not provide a public activation
  bypass.

## Capabilities

### New Capabilities

- `identity/developer-registration`: Defines public Mako Cloud developer
  registration, email verification, wait-list isolation, operator review,
  activation, rejection, session behavior, notifications, audit, and console
  workflows.

### Modified Capabilities

None.

## Impact

- Adds persistent developer credential, verification, session, recovery, and
  lifecycle records to the control-plane RocksDB schema and requires a
  compatibility-preserving migration for existing active developer identities.
- Adds public developer-auth and wait-list routes plus protected operator review
  routes to the control-plane HTTP service, OpenAPI contract, management SDK,
  Caddy allowlist, quotas, metrics, alerts, and audit inventory.
- Updates the web console with registration, verification, recovery, pending
  status, and operator wait-list screens while retaining strict separation from
  project application-user identity.
- Requires a production mail-delivery adapter and protected credentials before
  hosted registration can be enabled; raw passwords, tokens, verification
  secrets, recovery secrets, and applicant email addresses remain excluded from
  logs and metric labels.
- Produces a new immutable public-preview release, so public-preview acceptance
  remains exact-release-bound and must fall back to `pre_gate` on release drift,
  a failed safeguard, or an explicit operator pause. It does not expire merely
  because calendar time passes.
