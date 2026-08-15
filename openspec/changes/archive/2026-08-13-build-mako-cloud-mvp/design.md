## Context

See `proposal.md` for motivation and the six capability specs for behavior. This repository is greenfield, so the design establishes both the product boundaries and the first implementation architecture.

The application-facing data path is RxDB replication, not MongoDB, SQL, or a general browser database API. Production uses local RocksDB optimistic transactions directly, but the document layer still depends on an internal semantic key-value contract: conflict-safe replication needs conditional writes, stable snapshots, and atomic updates across document, index, idempotency, and change-log keys. Each stateful production database has one process owner and one persistent volume.

There are two identity domains. Developer and operator identities control organizations and projects; application-user identities are isolated inside each project environment and drive document policies. They must never be interchangeable.

The major request flows are:

```text
RxDB client -> Data gateway -> Session verification -> Sync API
                                                   -> Policy evaluator
                                                   -> Document engine -> KV adapter

HTTP caller -> Edge gateway -> JWT/routing -> Deno worker -> Caller-aware SDK
                                                        -> Auth/document APIs

Web console -> Management API -> Control metadata + Provisioning workflows
Platform admin ----------------> Operator API -----------^
```

## Goals / Non-Goals

**Goals:**

- Deliver one secure vertical slice that uses local RocksDB as its sole production key-value backend while retaining a testable semantic adapter boundary above it.
- Keep checkpointing, conflicts, tombstones, policy visibility, and RxDB client state coherent under retry, concurrency, disconnection, and revocation.
- Make the management API the source of truth and the web console a client of it.
- Use versioned, reversible changes for schemas, policies, credentials, and functions.
- Make tenant boundaries and privileged bypass explicit at every service boundary.
- Support short-lived, regionally routed Deno-compatible edge functions with local development parity.

**Non-Goals:**

- Building consensus, replication, automatic failover, active-active writes, shared-directory access, or horizontal sharding for the production RocksDB database.
- MongoDB wire compatibility, MongoDB drivers, SQL, joins, aggregation pipelines, or arbitrary unindexed scans.
- Exact Supabase API, CLI, or source compatibility; Supabase is a behavior and runtime reference.
- Social login, phone login, MFA, enterprise SSO, billing, or a provider marketplace in the MVP.
- Exactly-once network delivery; APIs provide idempotent effects over at-least-once requests.
- Long-running edge jobs or general container hosting.
- End-to-end encryption that hides document bodies from server-side policies and edge functions.

## Decisions

### 1. Split control plane, data plane, and function plane

Use separately deployable services with narrow contracts:

- A management API and asynchronous provisioner own organizations, projects, environments, configuration versions, quotas, and lifecycle.
- An identity service owns project application users, credentials, sessions, signing keys, and revocation state.
- A data gateway owns public project routing, token verification, rate limits, request identity, and stable API errors.
- A sync service implements the RxDB-shaped pull, push, and live-stream endpoints.
- A document engine owns storage keys, schemas, revisions, indexes, tombstones, and change iteration.
- A policy service compiles policy versions and evaluates them inside read and write paths.
- An edge gateway and regional worker pools own function routing and isolated execution.

The initial implementation should use a Rust workspace for gateways, identity, policy, sync, document, and control services; a TypeScript package for the RxDB adapter and management SDK; and a TypeScript web application for the console. Services use versioned HTTP/JSON externally and may use gRPC or an equivalent typed RPC protocol internally.

**Why:** The security and scaling profile differs across planes, while Rust provides one memory-safe core around storage and policy-sensitive paths. TypeScript matches RxDB, browser tooling, and Deno functions.

**Alternatives considered:** A single service is simpler to start but couples function isolation and sync latency to administration workloads. An all-TypeScript backend shortens initial coding but makes the embedded RocksDB and high-throughput storage boundary less direct. Separate repositories are deferred until ownership or release cadence requires them.

### 2. Define a semantic KV adapter, not a RocksDB method wrapper

Create an internal adapter contract with:

- `get`, durable `put`/`delete`, and lexicographic range iteration;
- stable read snapshots;
- atomic batches across keys in one project-environment partition;
- compare-and-set or serializable transaction semantics;
- health, durability-mode, and capability reporting.

Use RocksDB `OptimisticTransactionDB` for local development and production. Production opens one explicitly provisioned persistent path with synchronous durability, exclusive locking, ownership and format markers, and no memory or empty-database fallback. The adapter capability and recovery checks are part of readiness. Optimistic snapshot conflict detection must continue passing the shared atomicity and conditional-race suite, and this binding supplies native checkpoint creation over the live acknowledged database.

**Why:** Keeping the semantic contract makes the document layer's atomicity and snapshot assumptions explicit, preserves deterministic test and fault-injection adapters, and prevents call sites from weakening production durability.

**Alternatives considered:** Coupling every higher service directly to RocksDB would discard the shared conformance and failure-injection boundary. A distributed production backend is deferred until a separate evidence-backed change justifies its operational and semantic surface.

### 3. Use partitioned keyspaces and one atomic mutation record

Prefix every key with an encoded project and environment identifier. Within that boundary, use distinct key ranges for configuration, current documents, immutable recent revisions, indexes, idempotency outcomes, change records, and sequencer metadata. Encode user-provided identifiers so they cannot escape their key range.

One accepted mutation transaction will:

1. read and conditionally verify the current revision;
2. evaluate write policy against the verified old and proposed new states;
3. update the canonical document and affected index keys;
4. write an immutable change record containing enough old/new visibility metadata;
5. record the idempotency result; and
6. publish the commit position for pull and stream readers.

Canonical documents store the primary key, schema version, opaque revision, commit position, deletion flag, and normalized JSON body. Revision tokens are derived from immutable mutation identity, not wall-clock time.

**Why:** Co-locating derived state in one transaction prevents accepted documents, indexes, and replication history from disagreeing.

**Alternatives considered:** An asynchronous index or change-log projector improves write latency but introduces windows where an acknowledged write cannot replicate or query correctly. It can be revisited only with explicit consistency states.

### 4. Use a gap-aware per-environment commit sequencer

Assign mutations a monotonically ordered logical position per project environment. Sequence allocation may lease ranges for throughput, but pull high water advances only through positions proven committed or explicitly aborted. Change checkpoints encode the environment, collection scope, committed position, document tie-breaker, schema version, and authorization epoch and are authenticated to prevent client tampering.

**Why:** Wall-clock timestamps and ordinary distributed IDs can be observed out of commit order, causing a checkpoint to skip a late commit. A committed high-water fence makes catch-up complete and deterministic.

**Alternatives considered:** Hybrid logical clocks reduce coordination but still need a visibility rule for late commits. Per-document clocks cannot support a collection-wide pull checkpoint. A single unleased counter is correct but creates a hot key; range leases plus a gap table preserve correctness while allowing limited concurrency.

### 5. Keep queries deliberately small and index-backed

Expose primary-key reads and an internal trusted query interface with equality, bounded ranges, compound sort, cursor pagination, and limits. Index definitions are versioned resources. Builds scan a snapshot, consume subsequent change records, validate uniqueness, and switch to active atomically. Policy evaluation runs after candidate retrieval and before results leave the service.

**Why:** RxDB replication does not require MongoDB query compatibility. A constrained interface is enough for edge functions, administration, partial sync evolution, and policy-aware server operations while protecting the KV store from accidental full scans.

**Alternatives considered:** A Mongo-style query language would recreate a large compatibility surface. Allowing scans with warnings makes tenant workload isolation unpredictable.

### 6. Wrap RxDB's replication protocol in a Mako client package

Publish a TypeScript package that configures `replicateRxCollection()` and hides Mako-specific checkpoint, auth refresh, throttling, and authorization-reset extensions.

- Pull captures a committed high water, scans change records after the checkpoint, evaluates old/new visibility, and returns up to the requested count of authorized document states. It scans past hidden events until the batch is full or high water is exhausted.
- Push sends rows with stable mutation identifiers, assumed master state, and new fork state. Each row is conditionally authorized and committed. Readable conflicts return the current master; policy denials become typed non-retryable adapter errors without master content.
- Live delivery uses SSE initially. Each event includes a checkpoint; reconnect always triggers `RESYNC` and checkpoint catch-up. The gateway bounds buffers and disconnects slow consumers.
- Tombstones are ordinary revisioned states. Retention is configured in relation to the maximum supported offline interval; checkpoints older than retention receive an explicit full-resync requirement.

**Why:** RxDB already defines pull, push, and stream primitives and client-side conflict handling. A small adapter preserves that model while handling platform-specific auth and security correctly.

**Alternatives considered:** GraphQL adds no value for an RxDB-only frontend. WebSockets allow two-way messages but SSE is sufficient for server-to-client events and simpler to operate; WebSockets remain an optimization option.

### 7. Treat authorization changes as replication state changes

Use a CEL-compatible, typed, non-Turing-complete policy language. Compile policy versions against collection schemas into a deterministic evaluator with inputs for operation, verified identity/role/trusted claims, old document, new document, and safe request metadata. Matching denies override allows; absence of an allow denies.

For each change record, evaluate prior and new read visibility for the current caller:

- visible -> visible: deliver the new state;
- invisible -> visible: deliver the new state;
- visible -> invisible: deliver a synthetic tombstone;
- invisible -> invisible: advance the checkpoint without payload.

Policy activation increments an environment authorization epoch. Trusted-claim or membership changes increment the affected user's epoch. The client package persists the epoch with its replication metadata; on mismatch it pauses replication, notifies the application, securely clears the affected RxDB collection, and performs an initial pull under a new replication identifier. Security takes precedence over retaining unsynced local state, so the reset event is explicit and observable.

**Why:** Merely filtering future pulls leaves documents already stored in an offline client after access is revoked. Old/new visibility plus epoch resets covers document-driven and identity/policy-driven revocation.

**Alternatives considered:** Precomputing an event stream per user is prohibitively expensive. Re-running pull from zero without clearing RxDB does not delete absent documents. Arbitrary JavaScript policies are difficult to bound, type-check, and reproduce safely.

### 8. Separate project auth from platform auth

Use the control-plane identity provider only for developer/operator accounts. Build project auth as a separate namespace per environment:

- email/password credentials hashed with Argon2id and project-configurable verification;
- short-lived asymmetrically signed access JWTs with project, environment, session, role, and authorization epoch;
- project JWKS with signing-key overlap for rotation;
- opaque rotating refresh credentials stored as hashes, with family replay detection;
- a gateway revocation cache fed by an ordered revocation stream, with fail-closed lookup when freshness exceeds its bound;
- pluggable transactional email delivery for verification and recovery.

Public project keys identify and meter a client but do not authorize data. Service keys are hashed at rest, scoped, rotatable, and bypass policy only through an explicit privileged request path.

**Why:** Project application users belong to the customer's application, not to Mako Cloud administration. Short access-token lifetimes plus session and epoch checks bound revocation delay.

**Alternatives considered:** Reusing developer identities prevents customer-specific auth domains. Stateless JWT validation alone cannot provide prompt disable/revocation. Shared-secret JWTs make verifier compromise more damaging than asymmetric keys.

### 9. Run edge functions in a supervised Deno-compatible plane

Adopt the open-source Supabase Edge Runtime or a compatibility-tested Deno worker service behind a Mako-owned gateway. Pin the runtime version and isolate it behind an internal protocol so it can be replaced if upstream behavior changes.

Store immutable bundles in object storage and metadata in the control keyspace. A deployment references a bundle digest, runtime version, selected regions, attached secret versions, resource profile, JWT-verification setting, and health state. The gateway authenticates and routes; the worker receives only explicitly attached environment variables and a caller token. Regional workers enforce CPU, wall, memory, concurrency, payload, and outbound-network policy.

The default Mako SDK forwards the caller token to auth and document services. A function receives service-level access only when the project explicitly attaches a service credential as a secret and the function initializes a privileged client.

**Why:** Supabase's Deno-compatible runtime model supplies the requested TypeScript-first, portable edge-function behavior while keeping gateway authentication and resource enforcement outside user code.

**Alternatives considered:** Per-invocation containers have stronger familiar isolation but slower starts and higher cost. Node workers improve package compatibility but do not match the requested Supabase/Deno model. Embedding user code in data services is unacceptable for isolation.

### 10. Make the management API authoritative and provisioning asynchronous

Store control-plane records in a reserved system keyspace through the same semantic adapter, separate from tenant data keys. Mutations write an intent and enqueue an idempotent provisioning workflow. Each step records state and compensation information so retries and operator repair are safe.

The React/TypeScript console calls the management API for organization, project, collection, policy, user, credential, function, usage, log, and audit operations. Fine-grained permissions map organization roles to explicit actions; support/operator access uses a separate API, mandatory reason, expiry, and audit trail.

**Why:** Provisioning spans services and regions and cannot be one storage transaction. Durable workflows make partial failure visible and repairable. API parity avoids a second privileged path hidden in the console.

**Alternatives considered:** Synchronous provisioning makes timeouts ambiguous. Storing control metadata in a separate relational database adds an operational dependency and a second durability model before it is justified.

### 11. Standardize versioning, idempotency, and observability

Every public API returns a stable machine code, human message, request identifier, retry classification, and safe details. Mutating management and push requests accept idempotency keys. API versions are explicit in paths or content types; schemas, policies, credentials, and functions have independent resource versions.

Emit structured logs, metrics, traces, usage records, and append-only audit events with project/environment labels and correlation identifiers. Keep document bodies, passwords, raw tokens, and secret values out of telemetry. Quota decisions occur at gateways and use metering records that can be reconciled asynchronously.

**Why:** Offline clients and cloud workflows retry routinely. Stable outcomes and end-to-end correlation are required to distinguish retryable infrastructure failure from schema, policy, auth, or conflict outcomes.

**Alternatives considered:** Ad hoc service errors make the client retry unsafe. Synchronous global billing counters are unnecessary for the MVP and would add a hot dependency to every request.

## Risks / Trade-offs

- **[A host or volume failure causes a write outage]** -> State the single-node availability contract, monitor backup age, and require an operator-controlled same-volume restart or verified restore and promotion.
- **[A production path is missing, shared, or unexpectedly empty]** -> Require an ownership marker, exclusive open, synchronous durability, and semantic readiness before serving; never create a fallback database during startup.
- **[The commit sequencer becomes a throughput hotspot]** -> Partition by project environment, lease ranges, measure gap pressure, and preserve a migration path to sharded collection streams.
- **[Authorization filtering makes pull work proportional to hidden changes]** -> Bound internal scan work, require indexed future partial-sync predicates, meter scanned positions, and continue via opaque server cursors without exposing hidden counts.
- **[Access reset deletes unsynced local work]** -> Surface a dedicated security-reset event, document the behavior, allow an application callback before purge where policy permits, and always prefer confidentiality when access has been revoked.
- **[Policy bugs expose or strand data]** -> Default deny, typed compilation, example tests, atomic versioning, fast rollback, old/new visibility tests, and a security-focused differential test suite across all access paths.
- **[Refresh revocation state is stale at a gateway]** -> Bound cache age, stream invalidations, use short access-token lifetimes, and fail closed when revocation freshness cannot be proven.
- **[Tombstones and old states grow indefinitely]** -> Tie retention to a documented maximum offline interval, compact by safe checkpoint watermarks, and force full resync for expired checkpoints.
- **[Deno runtime escapes or dependency drift compromise isolation]** -> Run workers outside data services, pin and scan runtime images, enforce OS/runtime sandboxing and egress policy, and maintain adversarial isolation tests.
- **[Global edge execution adds latency to region-bound data]** -> Prefer selected regions near the project's data home, expose region metrics, and route data-heavy functions to the data region when configured.
- **[The greenfield scope is too broad for one release]** -> Implement and gate vertical phases; do not expose a capability until its security, recovery, and observability dependencies are complete.

## Migration Plan

This is a greenfield rollout, so migration means staged enablement rather than moving existing customer data.

1. Build the Rust/TypeScript workspace, API/error conventions, local certificates, test harnesses, and the semantic KV adapter with RocksDB conformance and crash tests.
2. Implement the document engine, schema/index lifecycle, commit sequencer, and change iterator; validate atomicity and recovery before adding public routes.
3. Implement project auth, signing-key rotation, revocation propagation, policy compilation/evaluation, audit events, and the privileged service path.
4. Implement pull, push, SSE, the RxDB package, schema mismatch handling, tombstones, conflicts, idempotency, and authorization-epoch reset; run multi-client offline and revocation suites.
5. Implement the management API, provisioner, developer console, application-user administration, quotas, logs, and operator workflows.
6. Integrate the pinned Deno-compatible edge runtime, bundle registry, secret delivery, regional routing, limits, local serve flow, and caller-aware SDK.
7. Qualify the production RocksDB configuration with crash, lock, disk/I/O, compaction, soak, checkpoint backup, empty-target restore, high-water, policy, and tenant-isolation tests on representative persistent storage.
8. Release to an internal environment, then a limited single-region beta. Each gate requires measured latency, durability, capacity, recovery-point, recovery-time, security, and cost thresholds; multi-node storage remains out of scope.

Rollback uses versioned service deployments and feature gates. Policy and function versions have first-class rollback. Schema changes remain additive until a migration is complete. Data format changes use dual-read/dual-write only when necessary and retain the previous reader through the rollback window. If production RocksDB fails qualification or health checks, stop new writes, fence the owner, and resume only from the same verified volume or an explicitly promoted restore that proves its recorded acknowledged high water; never accept traffic from an empty replacement database.

## Open Questions

- Production storage is intentionally single-node for the MVP. A future replicated or distributed backend requires a separate design and qualification change and does not alter the current public APIs.
- Hosted default quotas, retention durations, token lifetimes, and edge resource profiles will be set from load and abuse testing and remain project-plan configuration rather than protocol changes.

## Reference Contracts

- RxDB replication protocol: https://rxdb.info/replication.html
- RxDB HTTP replication example: https://rxdb.info/replication-http.html
- Supabase Edge Functions model: https://supabase.com/docs/guides/functions
- Supabase Edge Runtime: https://github.com/supabase/edge-runtime
- Supabase authentication and row-level-security model: https://supabase.com/docs/guides/auth and https://supabase.com/docs/guides/database/postgres/row-level-security
