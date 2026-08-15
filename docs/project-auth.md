# Project application authentication

Application users belong to exactly one project environment. Their identities,
sessions, roles, and trusted claims are distinct from Mako developer and
operator identities. The same normalized email may identify unrelated users in
different projects.

## Session lifecycle

1. Sign-up validates the project password policy and follows its email
   verification setting. Public responses do not reveal whether an account
   exists.
2. Sign-in issues a short-lived Ed25519-signed access JWT and an opaque refresh
   credential. The JWT binds issuer, audience, subject, project, environment,
   role, session, expiry, and authorization epochs.
3. Refresh rotates the stored credential hash. Reuse outside the bounded
   concurrency grace window revokes the whole refresh family.
4. Sign-out, administrator disable/delete, password recovery, or explicit
   session revocation publishes an ordered invalidation. Gateways fail closed
   if revocation freshness cannot be proven.

Use `GET /v1/projects/{projectId}/environments/{environmentId}/auth/jwks` with
the public project key to retrieve verification keys. A key rotation publishes
the new active key while retaining the prior public key through the configured
overlap so existing short-lived tokens remain verifiable. Do not retire the old
key until the maximum token lifetime and clock-skew window have elapsed.

## Credentials and metadata

Public project credentials may be embedded in browser applications but never
authorize protected data. Secret service credentials are one-time-display,
hashed, scoped, rotatable, and restricted to explicit privileged routes.

Only administrator-controlled app metadata and verified JWT claims are trusted
policy inputs. User-editable profile metadata must never assign a role or grant
document access. Passwords, refresh credentials, private signing keys, and raw
session tokens are excluded from APIs, logs, and audit records.

## Client behavior

Use `MakoAuthClient` from `@mako-cloud/rxdb` and persist refresh state only in
the platform's protected credential storage. On `authentication_required`, stop
replication, close the live stream, clear protected local state as required by
the application, and request a new sign-in. See [RxDB client](rxdb-client.md)
for refresh and authorization-reset integration.

## Administration and incidents

Authorized project members may search, invite, disable, restore, update trusted
metadata for, revoke sessions for, and delete application users. These actions
are audited without password hashes or tokens. Refresh replay response is
documented in the [auth replay runbook](runbooks/auth-refresh-replay.md).

## Tested evidence

Run `npm run test:auth-security`. It covers Argon2id upgrades, enumeration-safe
flows, JWT boundaries, encrypted signing-key rotation, refresh replay,
revocation freshness, project credentials, cross-project rejection, and RxDB
client refresh behavior. The latest scope is recorded in
[authentication qualification](auth-security-qualification.md).
