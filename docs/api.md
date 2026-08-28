# Public API

[`api/openapi/mako-cloud-v1.yaml`](../api/openapi/mako-cloud-v1.yaml) is the
authoritative versioned HTTP contract. It covers management, application auth,
documents, RxDB replication, functions, observability, and the isolated
operator surface. Generated TypeScript types live in
`packages/api-types/src/generated/schema.ts`; regenerate and verify them with
`npm run generate:api` and `npm run generate:api:check`.

The developer workspace adds independently bounded overview/navigation,
capability-authenticated explorer, data-job, Connect/check, aggregate sync,
verified backup, and isolated restore-request resources. Their operational
semantics and warnings are documented in [Developer data workspace](developer-data-workspace.md).

## Identity domains

- Management endpoints use a developer session or scoped team
  automation token.
- Projects belong to teams. Every developer also has a personal space -- an
  implicit one-member team, created the first time they create a project
  without naming a `teamId` and reused thereafter -- that holds their
  individual projects. It is listed among their teams with `kind: personal`,
  is billed and limited like any team, and refuses invitations, membership
  changes, and deletion.
- Project auth, document, replication, and protected function endpoints use an
  application-user session scoped to one project and environment.
- Public project keys identify and meter a client but grant no policy bypass.
- `/service/` document routes require an explicit scoped secret service
  credential and privileged audit context.
- `/v1/operator/` routes use a separate operator identity and are never
  reachable through an application or developer token.
- `/v1/developer-auth/` is the same-origin hosted registration, verification,
  session, recovery, and coarse wait-list-status family. Wait-list tokens use
  a dedicated audience rejected by management routes. Review under
  `/v1/operator/developer-waitlist` requires `waitlist_review`, a reason,
  request correlation, and an idempotency key.

Tenant identity is derived from verified credentials and must match every
`projectId` and `environmentId` path parameter. Do not infer authorization from
user-supplied document fields or a public key.

The operator control-center subresources are bounded, cursor-paginated,
metadata-only read models with explicit freshness and partial-provider status.
Mutation resources use reviewed versions, idempotency keys, action-bound recent
password verification, and durable audit/activity references. See the
[operator control-center guide](operator-control-center.md) for the role,
permission, feature-gate, qualification, and rollback inventory.

## Errors, retries, and idempotency

Public failures use the versioned `ApiErrorEnvelope`: stable machine code, safe
message, request identifier, retry classification, and bounded safe details.
Clients must not parse arbitrary server text. Log the request identifier, not
credentials or document bodies.

Mutating operations that declare the OpenAPI `Idempotency-Key` parameter should
reuse one stable key when retrying an ambiguous timeout. A changed payload with
the same key is a conflict. RxDB pushes additionally persist per-row mutation
outcomes, so a dropped response cannot create a second revision.

Honor retry advice:

- retry transient throttling only after the returned delay;
- refresh an expiring application session through the auth token endpoint;
- stop on permission, schema, or authentication-required errors;
- treat expired checkpoints and stream gaps as secure full-resync states.

## Supported clients

- Use `@mako-cloud/management-sdk` for management and operator inventory.
- Use `@mako-cloud/rxdb` for application data and auth; see the
  [RxDB guide](rxdb-client.md).
- Use `@mako-cloud/edge-sdk` inside functions; its default data client forwards
  verified caller context rather than service privilege.

The console uses the same management contract and authorization outcomes as
automation. Direct RocksDB access, MongoDB drivers, SQL, and arbitrary unindexed
document scans are not public interfaces.

The portal and operator API persist their authoritative identity, wait-list,
team, project, incident, audit, and function-metadata state in the
server-side control SQLite database. This is an implementation boundary, not a
new public SQL API. Application users, credentials, documents, policies,
indexes, and RxDB replication remain data-plane RocksDB authority. When that
data plane is unavailable, control authentication and control-owned resources
remain available while tenant-data operations return their existing scoped
unavailable error.

## Tested evidence

- `npm run generate:api:check` proves generated types match the OpenAPI source.
- `packages/management-sdk/test/client.test.mjs` checks public and operator
  operation inventory plus safe error handling.
- `crates/mako-control-plane/src/management_access.rs` checks console/API RBAC
  parity.
- `docs/requirements-traceability.md` maps every capability scenario to its
  automated test.
