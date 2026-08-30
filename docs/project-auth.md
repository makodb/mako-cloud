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

## Trusted metadata from an application's function

An application's own trusted code -- an edge function holding an attached
service secret -- can set a user's administrator-controlled app metadata, so
the memberships and roles the application manages become the claims that user's
next token carries and policies trust:

```
POST /v1/projects/{projectId}/environments/{environmentId}/service/users/{userId}/app-metadata
X-Mako-Service-Key: mako_sk....
X-Mako-Request-Id: req_...
Content-Type: application/json

{ "reason": "invitation accepted", "appMetadata": { "households": { "hh_1": "editor" } } }
```

- **Credential.** Only a secret service credential is accepted, and it must be
  scoped to the reserved `users` target with the `update` operation
  (`mako keys create --service --collections memberships,users --operations
  read,create,update`). Document scopes name collections; `users` names the
  identity surface and is checked by the same gateway. A credential without it
  is refused with `permission_denied`. A request that carries an application
  bearer token or a public project key beside or instead of the service key is
  refused with `unauthenticated` -- there is no fallback, and no route through
  which an application user can reach app metadata.
- **Body.** `reason` (1-512 printable characters) is the audited bypass reason
  and is required. `appMetadata` is a one-level JSON merge patch over the
  user's trusted metadata: a key set to `null` is removed, any other key
  replaces the stored value whole. The patch and the merged result are bounded
  like trusted metadata (64 KiB, nesting depth 16); a patch that would exceed
  them is refused before anything is written. User-editable profile metadata
  is never read or changed by this route. `expectedAuthorizationEpoch` is
  optional and names the epoch the patch was composed against; the write is
  refused with `conflict` if the user's epoch has moved since.
- **Reading it back.** `GET` on the same path, with the reason in
  `X-Mako-Bypass-Reason` and the credential scoped to `users` with `read`,
  returns the user's app metadata and current authorization epoch. Because a
  patch replaces a key whole, a function that manages one member of a claim
  map composes the next value out of this one; naming the epoch it read on the
  write that follows is what keeps two concurrent changes from silently
  keeping whichever wrote last. The read is a privileged bypass like the
  write and is audited as `service_user_app_metadata_read`.
- **Audit.** Before the metadata is written, a `service_bypass` audit record
  is appended with the credential as the actor
  (`service_user_app_metadata_update` on resource `application_user/{userId}`,
  reason code `service_bypass_verified`, `bypass_reason` in the details). If
  that record cannot be written, the request fails closed and nothing changes.
  The record is written once the credential is verified, so an unknown user
  (`not_found`) is audited too.
- **Reaching the next token.** A change advances the user's authorization
  epoch, exactly as an administrator's metadata edit does, and invalidates
  developer explorer grants for the environment. The access token the user
  holds no longer verifies, the client refreshes, and the refresh re-reads the
  user and issues a token whose `trusted_claims` (and `role`, when trusted
  metadata carries one) are the new values. The response carries the merged
  `appMetadata` and the user's `authorizationEpoch` after the write. A patch
  that changes nothing is audited but advances no epoch.
- **Never public.** The reverse proxy answers `/service/` with 404 on the
  platform hostname and on every custom domain, and the data plane refuses on
  its own any request that arrived on a custom domain. The route is reachable
  only from the loopback origin edge functions are given.

`@mako-cloud/edge-sdk` exposes the route as
`createServiceClient(...).users.setAppMetadata(userId, patch, reason?)`; see
[edge functions](edge-functions.md).

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
