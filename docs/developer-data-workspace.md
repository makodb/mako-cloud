# Developer data workspace

The environment workspace groups overview, Data Explorer, RxDB sync diagnostics, policies,
backups, API & Connect, and settings under
`/projects/{projectId}/environments/{environmentId}/…`. Navigation is permission-filtered and
switching environment destroys the current explorer capability, snapshot cursor, preview user,
draft, and selected document before loading the next scope. Existing collection, user, function,
credential, and observability URLs remain available as rollback routes.

## Explorer access modes

Policy preview evaluates reads, indexed queries, and mutation simulations as one selected active
application user. It does not create an application session and cannot commit. Administrative
access requires the project's data-admin permission, an explicit reason and confirmation, and is a
document-policy bypass. Its grant lasts at most five minutes and every use is audited. Grants are
kept only in React memory: never put `x-mako-explorer-capability` values in a URL, browser storage,
logs, telemetry, error reports, or support tickets.

Browse is canonical primary-key order over a stable snapshot. It omits deleted documents unless
retained tombstones are explicitly requested with history permission. Policy-hidden documents do
not affect returned counts or cursor behavior. Query planning accepts at most 16 predicates, four
sort fields, and 200 rows; only a matching active index may execute. The server returns a minimal
required-index shape rather than falling back to a collection scan.

Simulation parses and validates the proposed JSON, active schema, expected revision, and current
policy without writing or allocating a committed position. Administrative create/update/delete
uses the same conditional mutation path as application traffic. A revision conflict is presented
as original/proposed/current; the console never merges or retries it automatically.

## Import and export

The only bulk format is UTF-8 JSON Lines, one JSON object per non-empty line. Imports require an
immutable digest-verified upload and dry run before confirmation. Conflict strategies are
`create_only`, `update_existing`, and `upsert`. Each row is conditionally idempotent; cancellation
stops future work and does not roll back committed rows. Exports read one consistent snapshot and
become downloadable only after their manifest and digest are finalized. Partial artifacts are
never served.

Limits are 1 MiB per document, 512 MiB per upload or output, four active jobs per tenant, one hour
of execution, 24 hours of artifact retention, and five minutes per upload/download grant. Job
progress reports exact processed, committed, failed, skipped, exported, and byte counts. Artifact
cleanup is retryable and idempotent; object-store outages defer cleanup or job execution rather
than silently publishing incomplete output.

## API & Connect and sync diagnostics

The Connect page shows the public endpoint, active public credential ID, active collection/schema
versions, supported `>=17.0.0 <18.0.0` range, and template version 1. Public credential values are
one-time material and cannot be recovered later. The connection check accepts public metadata
only and returns separate DNS, TLS, route, readiness, key-recognition, schema, client-version, and
replication-route steps. It never signs in an application user or invokes pull/push.

Sync diagnostics contain bounded aggregates for pull/push, live streams, lag, conflicts, policy
denials, throttling, checkpoint expiry, stream gaps, resync, schema mismatch, and coarse client
compatibility classes. They never return raw user, device, session, IP, token, or document IDs.

## Backup and isolated recovery

Developer backup inventory includes only tenant-verified manifests and safe recovery-point,
verification, retention, drill, and objective fields. Physical paths, hosts, credentials, signing
material, and other tenants are excluded. A restore request requires current backup-read and
restore permissions plus password verification from the current developer session within five
minutes. It creates a new isolated environment and is quota bounded. Overwrite and promotion are
always prohibited, including inside the guarded recovery orchestrator. Access remains disabled
until tenant isolation, storage verification, service readiness, and recovery validation succeed.

## Rollout and incident response

The workspace/read-only explorer, administrative mutations, data jobs, sync detail, and restore
requests have independent console gates. Disable the affected gate first on an explorer audit gap,
unexpected document disclosure, object-store integrity failure, high-cardinality telemetry,
cross-tenant result, or restore isolation failure. Revoke outstanding grants by advancing the
developer or tenant authorization epoch, preserve audit/request IDs, and follow the tenant
isolation, storage, policy, or recovery runbook appropriate to the signal.
