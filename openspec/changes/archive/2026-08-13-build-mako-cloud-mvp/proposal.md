## Why

RxDB applications need a secure, hosted backend that preserves their local-first workflow without requiring PostgreSQL or MongoDB compatibility. Mako Cloud will provide that backend on top of an ordered RocksDB-style key-value interface, combining synchronization, application-user identity, document-level authorization, edge functions, and a manageable cloud control plane.

## What Changes

- Add a document storage engine over a RocksDB-style key-value contract, including collections, schemas, indexes, atomic document revisions, tombstones, and an ordered change log.
- Add an RxDB-native client and replication API for checkpointed pull, conflict-aware push, and live server-to-client change delivery.
- Add project-scoped authentication for application users, beginning with email/password identities, refreshable JWT sessions, session revocation, and administrative user management.
- Add default-deny document policies that authorize each create, read, update, and delete against the authenticated user's trusted claims and the relevant document states.
- Add a Supabase-style edge-function service for versioned TypeScript/JavaScript functions in a Deno-compatible isolated runtime, with authenticated invocation, project secrets, logs, metrics, and resource limits.
- Add a multi-tenant cloud control plane and web console for developer accounts, organizations, projects, environments, collections, indexes, policies, API keys, functions, usage, audit events, and operator administration.
- Use local RocksDB optimistic transactions as the sole production and development storage backend, with one exclusively owned persistent volume per stateful service database and no in-memory fallback.
- Document the initial single-node storage topology: storage writes pause during node or volume recovery, and automatic failover, active-active writes, shared RocksDB directories, and horizontal scaling of one database are unsupported.
- Explicitly exclude MongoDB wire/driver compatibility, a general-purpose database API for arbitrary frontends, billing, enterprise SSO, analytical queries, joins, aggregation pipelines, and multi-document application transactions from the MVP.

## Capabilities

### New Capabilities

- `storage/document-engine`: Persist versioned JSON documents, indexes, tombstones, and ordered change records over the backing key-value interface.
- `sync/rxdb-replication`: Connect RxDB collections to authenticated checkpoint, push, and live-stream replication endpoints.
- `identity/project-auth`: Manage project application users and issue verifiable, refreshable, revocable sessions for data and function access.
- `security/document-policies`: Define and enforce document-level access policies consistently across replication, administrative access, and edge-function data calls.
- `functions/edge-runtime`: Develop, deploy, invoke, observe, and govern isolated Deno-compatible edge functions.
- `cloud/control-plane`: Manage the multi-tenant platform through developer/operator APIs and a web console.

### Modified Capabilities

None. This is a greenfield project with no existing capability specifications.

## Impact

- Introduces the complete Mako Cloud control plane and data plane, including public auth, sync, streaming, function-invocation, management, and operator APIs.
- Requires a transactional ordered key-value adapter, an RxDB client integration package, an identity service, a policy evaluator, an edge gateway/runtime, a web console, and operational services for metering, logs, and audit history.
- Establishes security-sensitive contracts for tenant isolation, token signing and rotation, policy evaluation, secret handling, function sandboxing, and revocation propagation.
- Establishes RocksDB as the sole production conformance backend. Operators must provision persistent volumes, monitor capacity and backups, and use explicit same-volume restart or verified restore-and-promote procedures after failure.
