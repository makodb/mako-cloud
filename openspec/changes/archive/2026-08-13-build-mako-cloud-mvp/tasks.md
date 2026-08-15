## 1. Repository and Platform Foundations

- [x] 1.1 Create the Rust workspace and service/library crates for common APIs, storage, documents, policy, identity, sync, gateways, control plane, provisioning, and audit.
- [x] 1.2 Create the TypeScript workspace for the RxDB client, management SDK, edge SDK, console, examples, and shared generated API types.
- [x] 1.3 Define the versioned public error envelope with stable codes, request IDs, retry classification, and safe structured details in Rust and TypeScript.
- [x] 1.4 Define OpenAPI contracts and generated clients for management, auth, replication, live-stream, function, and operator endpoints.
- [x] 1.5 Add project/environment identity types and reject ambiguous or unscoped tenant context at internal service boundaries.
- [x] 1.6 Build a local development stack with certificates, service discovery, RocksDB data directories, an email sink, object storage, and observability dependencies.
- [x] 1.7 Add formatting, linting, dependency auditing, unit-test, integration-test, and artifact-build jobs to CI.
- [x] 1.8 Add configuration loading with typed validation, environment overrides, secret references, and fail-fast startup diagnostics.
- [x] 1.9 Document the initial trust boundaries, privileged identities, sensitive data classes, and abuse cases in a threat model checked by security tests.

## 2. Semantic KV Adapter and RocksDB Reference

- [x] 2.1 Define the KV adapter traits for point access, ordered range scans, stable snapshots, atomic batches, serializable or compare-and-set transactions, health, and capability reporting.
- [x] 2.2 Implement collision-safe ordered key encoding for system, project, environment, collection, document, index, change, idempotency, and sequencer ranges.
- [x] 2.3 Add key-codec property tests covering arbitrary identifiers, prefix boundaries, ordering, and cross-tenant escape attempts.
- [x] 2.4 Implement an in-memory deterministic adapter for model and failure-injection tests.
- [x] 2.5 Implement the local RocksDB adapter with configured durability, snapshots, bounded iterators, atomic batches, and conditional transactions.
- [x] 2.6 Build the shared adapter conformance suite for isolation, ordering, atomicity, snapshot consistency, conditional-write races, and restart durability.
- [x] 2.7 Add crash points and injected I/O failures around each transactional phase and verify no acknowledged partial state is observable.
- [x] 2.8 Expose adapter capability and durability results through readiness checks and prevent data-plane readiness on a failed contract.
- [x] 2.9 Add strict production RocksDB configuration, explicit volume provisioning and ownership markers, direct service wiring, and fail-closed readiness with no memory or empty-database fallback.
- [x] 2.10 Run the shared conformance and durability suites against the exact synchronous production RocksDB configuration and retain the result as release evidence.

## 3. Document Model, Revisions, and Change Ordering

- [x] 3.1 Implement canonical JSON document encoding with primary key, schema version, opaque revision, commit position, deletion state, and normalized body.
- [x] 3.2 Implement versioned collection metadata with JSON Schema, primary-key definition, compatibility state, and lifecycle status.
- [x] 3.3 Add document validation and immutable-primary-key checks that leave no state changes on failure.
- [x] 3.4 Enforce project/environment/collection keyspace scoping in every document-engine entry point and test cross-tenant identifiers.
- [x] 3.5 Implement the per-environment logical sequence allocator with range leases and durable committed/aborted gap tracking.
- [x] 3.6 Implement high-water advancement and restart recovery that never advances pull visibility across unresolved sequence gaps.
- [x] 3.7 Implement the atomic mutation transaction for revision checks, document state, index deltas, change record, idempotency result, and commit publication.
- [x] 3.8 Add conditional create, update, and delete operations and return typed current-revision conflicts.
- [x] 3.9 Add stable mutation IDs and persisted idempotency outcomes so repeated commits cannot create duplicate revisions or events.
- [x] 3.10 Implement primary-key reads and consistent snapshot reads for multi-key operations.
- [x] 3.11 Implement revisioned `_deleted` tombstones and prohibit immediate physical removal from replication history.
- [x] 3.12 Implement retention watermarks, expired-checkpoint detection, and safe compaction for old revisions, changes, idempotency records, and tombstones.
- [x] 3.13 Add concurrency and restart tests proving document, revision, change position, and acknowledgement invariants.

## 4. Indexes and Trusted Queries

- [x] 4.1 Implement versioned non-unique, unique, and compound index definitions with building, active, failed, and deleting states.
- [x] 4.2 Implement index key encoding for missing values, scalar types, compound ordering, primary-key tie-breaking, and uniqueness ownership.
- [x] 4.3 Apply index additions and removals inside each atomic document mutation.
- [x] 4.4 Implement snapshot index backfill with a captured change position and resumable progress metadata.
- [x] 4.5 Catch an index build up through the change log and switch it active only after backfill and concurrent writes are complete.
- [x] 4.6 Detect unique-index duplicates during backfill and live writes without exposing protected document bodies in diagnostics.
- [x] 4.7 Implement the trusted query planner for primary-key, equality, bounded range, compound sort, cursor, and limit operations.
- [x] 4.8 Reject unsupported predicates and unbounded scans with a stable required-index error.
- [x] 4.9 Add online build, concurrent-write, uniqueness-race, cursor-stability, and index-removal tests.

## 5. Document Policy Engine

- [x] 5.1 Define the typed policy model for operation-specific allow and deny rules, versions, activation state, and source diagnostics.
- [x] 5.2 Define the CEL-compatible evaluation environment for verified user ID, role, trusted claims, old/new document states, operation, and safe request metadata.
- [x] 5.3 Implement parsing, deterministic compilation, cost limits, and schema-aware type checking without network or arbitrary-code access.
- [x] 5.4 Implement default-deny and deny-overrides-allow evaluation with stable, non-sensitive result codes.
- [x] 5.5 Add create, update, and delete evaluation hooks that use the correct old/new states inside the document conditional transaction.
- [x] 5.6 Add read filtering for point reads, indexed queries, pull batches, live events, and readable conflict responses.
- [x] 5.7 Implement old/new visibility classification that produces normal states, synthetic tombstones, or hidden checkpoint advancement.
- [x] 5.8 Implement draft, validation, example-test, atomic activation, rollback, and immutable history for policy versions.
- [x] 5.9 Implement environment and per-user authorization epochs and publish invalidations when policy or trusted claims change.
- [x] 5.10 Implement the explicit service/operator bypass path with credential scoping, mandatory audit context, and no public-client access.
- [x] 5.11 Add fail-closed behavior for evaluator timeout, unavailable policy state, invalid context, and internal errors.
- [x] 5.12 Build differential security tests proving identical policy results across replication, trusted queries, edge SDK calls, and operator impersonation.

## 6. Project Application Authentication

- [x] 6.1 Implement project-environment-scoped user, identity, credential, trusted metadata, profile metadata, session, and token-family records.
- [x] 6.2 Add normalized email uniqueness inside a project while allowing the same email to identify distinct users in other projects.
- [x] 6.3 Implement configurable password policy and Argon2id password hashing with automatic parameter upgrades.
- [x] 6.4 Implement sign-up and verification-token flows with enumeration-safe responses and a pluggable transactional email provider.
- [x] 6.5 Implement email/password sign-in with uniform failure responses, throttling hooks, and audit events.
- [x] 6.6 Implement single-use password-recovery tokens, password change, credential invalidation, and session-revocation behavior.
- [x] 6.7 Implement asymmetric project signing-key creation, encrypted private-key handling, JWKS publication, overlap rotation, and retirement.
- [x] 6.8 Issue access JWTs with all required project, environment, subject, role, session, expiry, and authorization-epoch claims.
- [x] 6.9 Implement shared gateway token verification for signature, issuer, audience, project, environment, expiry, session, and epoch.
- [x] 6.10 Implement hashed rotating refresh credentials, bounded concurrency grace, family replay detection, and fresh access-token issuance.
- [x] 6.11 Implement user/session disable, deletion, single-session sign-out, all-session sign-out, and ordered revocation notifications.
- [x] 6.12 Build a bounded-freshness revocation cache for gateways and fail closed when freshness cannot be proven.
- [x] 6.13 Implement rotatable public project keys and scoped service credentials with one-time secret display and overlap windows.
- [x] 6.14 Implement administrator APIs for application-user search, create/invite, inspect, metadata update, disable/restore, session revoke, and delete.
- [x] 6.15 Add auth endpoint rate limits, account-enumeration tests, refresh-replay tests, signing-key rotation tests, and token cross-project tests.

## 7. RxDB Replication Service

- [x] 7.1 Define authenticated pull, push, and SSE endpoint schemas plus signed opaque checkpoint and cursor codecs.
- [x] 7.2 Implement null-checkpoint initial pull with a captured committed high water and deterministic change ordering.
- [x] 7.3 Implement authorized pull scanning that fills the requested visible batch or exhausts high water while advancing past hidden changes.
- [x] 7.4 Implement checkpoint validation for project, environment, collection, schema version, authorization epoch, tampering, and retention expiry.
- [x] 7.5 Implement bounded push batches containing stable mutation IDs, assumed master states, and new fork states.
- [x] 7.6 Map stale readable revisions to RxDB conflict states and map denied or unreadable rows to non-sensitive typed authorization failures.
- [x] 7.7 Persist and replay per-row push outcomes so a retried batch cannot duplicate successful writes.
- [x] 7.8 Implement live SSE delivery from committed high water with policy filtering, checkpoint events, heartbeats, and bounded per-connection buffers.
- [x] 7.9 Emit resynchronization signals for reconnects, stream gaps, retention expiry, service failover, and authorization-epoch changes.
- [x] 7.10 Deliver stored and synthetic tombstones with the primary key and RxDB-required deletion metadata.
- [x] 7.11 Reject incompatible collection schema versions with a stable non-retryable migration-required error.
- [x] 7.12 Integrate gateway authentication, public-key validation, revocation checks, policy context, request IDs, quotas, and retry guidance.
- [x] 7.13 Add multi-client tests for concurrent writes, offline conflicts, hidden changes, visibility transitions, deletion, reconnect, retry, and expired checkpoints.
- [x] 7.14 Add slow-consumer, oversized-batch, rate-limit, and service-restart tests for backpressure and recovery.

## 8. RxDB Client Package

- [x] 8.1 Scaffold the versioned TypeScript package with supported RxDB peer-version checks, typed configuration, and browser/Node build outputs.
- [x] 8.2 Implement project-auth sign-up, sign-in, refresh, sign-out, and session persistence helpers without exposing raw service credentials.
- [x] 8.3 Implement the RxDB pull handler adapter with checkpoint persistence, batch sizing, schema-version binding, and server error mapping.
- [x] 8.4 Implement the push handler adapter with stable mutation IDs, conflict return mapping, and non-retryable policy-error reporting.
- [x] 8.5 Implement the SSE pull stream with checkpoints, reconnect backoff, token replacement, `RESYNC`, and bounded buffering.
- [x] 8.6 Implement automatic access-token refresh and an authentication-required state when refresh is revoked or unavailable.
- [x] 8.7 Implement authorization-epoch mismatch handling that pauses sync, invokes the application reset callback, securely clears affected state, and starts a new replication identifier.
- [x] 8.8 Implement explicit schema-migration-required and expired-checkpoint/full-resync states with application hooks.
- [x] 8.9 Expose RxDB activity, sent/received documents, conflicts, throttling, security resets, and sanitized errors through typed observables or callbacks.
- [x] 8.10 Build a reference local-first application and automated browser tests covering offline writes, conflicts, tombstones, token refresh, reconnect, and access revocation.
- [x] 8.11 Publish client setup, policy interaction, conflict-handler, security-reset, and migration documentation generated from tested examples.

## 9. Control Plane and Provisioning APIs

- [x] 9.1 Implement reserved system-keyspace models for developer identities, organizations, memberships, invitations, projects, environments, roles, quotas, and lifecycle state.
- [x] 9.2 Integrate a control-plane developer identity provider and keep its sessions and claims separate from project application auth.
- [x] 9.3 Implement organization CRUD, invitation acceptance, membership management, and owner/admin/developer/viewer authorization.
- [x] 9.4 Implement project and environment create, inspect, suspend, restore, and deletion-intent management endpoints.
- [x] 9.5 Implement the durable idempotent provisioning workflow engine with step state, retries, compensation, diagnostics, and operator repair.
- [x] 9.6 Provision storage namespaces, auth keys, default-deny policy state, replication routes, function metadata, quotas, and observability resources before activation.
- [x] 9.7 Implement management endpoints for collection schemas, compatibility checks, index lifecycle, and migration state.
- [x] 9.8 Implement management endpoints for policy draft, validation, example testing, activation, rollback, and authorization-epoch visibility.
- [x] 9.9 Expose application-user and session administration through permission-checked control-plane routes.
- [x] 9.10 Implement public key, service credential, automation token, JWT signing-key, and function-secret management with rotation and one-time display.
- [x] 9.11 Implement function metadata, deployment, promotion, rollback, configuration, test invocation, log lookup, and deletion endpoints.
- [x] 9.12 Implement usage, quota, health, replication error, auth event, index state, log, and audit query endpoints with retention-aware pagination.
- [x] 9.13 Implement the isolated operator API for tenant lookup, provisioning repair, quota override, abuse response, and time-bounded support access.
- [x] 9.14 Implement grace-period deletion, prompt data-plane revocation, restoration, final secret/data destruction, and completion auditing.
- [x] 9.15 Add API parity and RBAC tests ensuring automation and console requests receive identical authorization and validation outcomes.

## 10. Developer and Operator Console

- [x] 10.1 Scaffold the TypeScript web console with generated management client, developer authentication, route guards, error boundaries, and request-ID display.
- [x] 10.2 Build organization switching, invitations, membership, and role-management screens.
- [x] 10.3 Build project/environment creation, provisioning progress, health, suspend, restore, and deletion-grace screens.
- [x] 10.4 Build collection, JSON-schema version, compatibility, migration, index-build, and index-failure management screens.
- [x] 10.5 Build the policy editor with schema-aware diagnostics, example contexts, evaluation traces, versions, activation, rollback, and default-deny warnings.
- [x] 10.6 Build application-user search, detail, trusted metadata, disable/restore, session revoke, invite, and delete screens.
- [x] 10.7 Build public/service credential, automation token, signing-key, and function-secret creation, rotation, one-time display, and revocation screens.
- [x] 10.8 Build edge-function source/bundle upload, configuration, deployment, version promotion, test invocation, rollback, log, and metric screens.
- [x] 10.9 Build usage, quota, service health, replication error, auth event, audit search, and export views.
- [x] 10.10 Build a separately routed operator console for support sessions, provisioning repair, quota overrides, and abuse response.
- [x] 10.11 Add responsive layout, keyboard navigation, accessible labels/status, secret-display safeguards, and destructive-action confirmations.
- [x] 10.12 Add end-to-end role, provisioning, policy, user, credential, function, audit, and deletion-lifecycle tests against the management API.

## 11. Edge Function Plane

- [x] 11.1 Evaluate and pin a Supabase Edge Runtime release or compatibility-tested Deno worker version behind a documented internal protocol.
- [x] 11.2 Implement deterministic TypeScript/JavaScript bundling with dependency resolution, size limits, digesting, validation, and sanitized diagnostics.
- [x] 11.3 Implement immutable bundle storage and version metadata for runtime version, entrypoint, regions, secrets, limits, auth setting, and health.
- [x] 11.4 Implement create, deploy, health-check, atomic promote, list, rollback, and delete operations for function versions.
- [x] 11.5 Build the edge gateway for stable function URLs, project routing, standard methods, streaming responses, request limits, and JWT-required-by-default behavior.
- [x] 11.6 Implement the explicit public-invocation setting for webhook functions while retaining project routing, rate limits, quotas, and audit context.
- [x] 11.7 Build the regional worker supervisor with per-project isolation, concurrency admission, crash recovery, and clean runtime recycling.
- [x] 11.8 Enforce CPU, wall-time, memory, request, response, and outbound-network limits and return stable correlation-aware limit errors.
- [x] 11.9 Encrypt function secrets, attach explicit versions to deployments, inject only selected values, and redact active values from logs and APIs.
- [x] 11.10 Implement the edge SDK document/auth clients that propagate the verified caller identity by default.
- [x] 11.11 Implement explicit service-client initialization from an attached credential and emit privileged-bypass audit events.
- [x] 11.12 Implement regional deployment health and nearest-healthy-selected-region routing without unauthorized-region fallback.
- [x] 11.13 Capture sanitized structured logs, metrics, resource usage, status, version, region, and request/trace identifiers for each invocation.
- [x] 11.14 Build the local function serve command with hosted-compatible request handling, environment mapping, JWT verification toggle, secrets, and runtime APIs.
- [x] 11.15 Add compatibility tests for Fetch APIs, TypeScript, JavaScript, supported npm modules, WebAssembly, outbound fetch, and streaming responses.
- [x] 11.16 Add adversarial tests for cross-project memory/environment access, secret leakage, egress policy, resource exhaustion, worker crashes, and work continuing past invocation lifetime.

## 12. Audit, Metering, and Operations

- [x] 12.1 Define shared structured log, metric, trace, usage, and audit schemas with project/environment, actor, resource, request, and correlation identifiers.
- [x] 12.2 Implement centralized telemetry redaction that excludes document bodies, passwords, raw tokens, credential values, and function secrets.
- [x] 12.3 Implement append-only audit storage and retention-aware filter/export APIs for control, auth, policy, service-bypass, function, and operator events.
- [x] 12.4 Implement idempotent usage records and aggregation for storage, replication requests/bytes, auth activity, function invocation/resources, logs, and egress.
- [x] 12.5 Implement gateway quota and rate-limit decisions with stable hard-limit and retryable-throttle responses.
- [x] 12.6 Add health, saturation, lag, error, sequencer-gap, revocation-freshness, stream, index-build, and function-worker dashboards.
- [x] 12.7 Add alerts and operator runbooks for storage contract failure, unresolved commit gaps, policy failures, auth replay, tenant-isolation signals, and edge sandbox incidents.
- [x] 12.8 Add retention and compaction jobs for logs, audit data, usage detail, revisions, changes, idempotency records, and tombstones with dry-run reporting.

## 13. Security, Reliability, and Release Qualification

- [x] 13.1 Build a requirements-to-test traceability matrix covering every scenario in the six capability specs.
- [x] 13.2 Run tenant-boundary property and fuzz tests across key encoding, gateways, storage, management APIs, logs, object storage, and edge workers.
- [x] 13.3 Run policy differential, visibility-transition, authorization-epoch, conflict-non-disclosure, and privileged-bypass penetration tests.
- [x] 13.4 Run password, token, JWKS rotation, refresh replay, session revocation, enumeration, credential rotation, and cross-project auth security tests.
- [x] 13.5 Run RxDB multi-client chaos tests with offline periods, duplicate/reordered requests, dropped responses, SSE gaps, schema upgrades, retention expiry, and policy changes.
- [x] 13.6 Run storage crash, sequencer-gap recovery, index catch-up, tombstone compaction, and acknowledged-write durability soak tests.
- [x] 13.7 Run edge sandbox escape, dependency supply-chain, secret exfiltration, egress, resource exhaustion, and regional failover tests.
- [x] 13.8 Benchmark write, pull, hidden-change scan, push-conflict, live fan-out, auth, policy, index-build, control-plane, and edge cold/warm paths.
- [x] 13.9 Qualify production RocksDB under crashes, lock contention, disk and I/O failures, compaction, restarts, load, and acknowledged-high-water recovery on representative persistent storage.
- [x] 13.10 Verify checkpoint backup, authenticated manifests, empty-target restore, explicit promotion, acknowledged-high-water recovery, and tenant isolation.
- [x] 13.11 Publish local development, deployment, operations, security, API, RxDB integration, policy, auth, and edge-function documentation from tested flows.
- [x] 13.12 Define internal, single-region beta, and multi-region release gates with measured durability, recovery, security, latency, capacity, and cost thresholds.
- [x] 13.13 Exercise service, policy, function, schema, signing-key, and storage-adapter rollback procedures before enabling external beta traffic.
