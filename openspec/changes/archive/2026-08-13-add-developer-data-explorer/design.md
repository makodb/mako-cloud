## Context

The developer console already manages organizations, projects, environments, collections, schemas, indexes, policies, application users, credentials, functions, and retained observability records. The data plane already implements policy-authorized and service-credential document get, query, and conditional mutation routes over revisioned documents and active indexes. It does not currently accept developer management sessions, and the control plane is intentionally prohibited from directly reading customer document bodies.

The change crosses browser routing and state, developer RBAC, control-plane authorization, data-plane authorization, the document engine's query surface, audit, object storage, background jobs, observability, and backup/recovery. See `proposal.md` for motivation and the two capability specs for observable behavior.

## Goals / Non-Goals

**Goals:**

- Deliver a database-oriented developer workspace whose document access remains explicitly scoped and auditable.
- Reuse existing document, index, policy, replication, observability, object-store, and backup primitives instead of creating a parallel database interface.
- Keep service credentials and customer document payloads out of the control plane and browser persistence.
- Make policy preview, administrative access, mutation conflicts, imports/exports, and recovery state unambiguous.

**Non-Goals:**

- A SQL, MongoDB, aggregation-pipeline, arbitrary scripting, or unbounded scan console.
- General application access through the explorer APIs; RxDB remains the supported application data interface.
- Automatic conflict merging, in-place developer restore, backup promotion, billing, or operator fleet administration.
- Giving platform operators ordinary access to tenant documents.

## Decisions

### 1. Use short-lived explorer capabilities between control and data planes

The console will ask the control plane to create an explorer access grant after authenticating the developer session and checking organization membership, project/environment lifecycle, collection scope, requested mode, and action class. The control plane returns a signed, narrow, short-lived capability containing the developer identity, tenant scope, collection scope or allowlist, mode, allowed operations, reason hash, issued/expiry times, nonce, and authorization epoch.

The console presents that capability to dedicated data-plane explorer routes. The data plane validates the capability, current authorization epoch, scope, operation, and revocation state before reading or mutating documents. It maps policy-preview grants to a verified application-user policy context and administrative grants to an explicit privileged authorizer that emits bypass audit. The capability is kept only in memory, is never accepted in URLs, and cannot be exchanged for a service credential.

**Alternative considered:** Proxy document bodies through management APIs in the control plane. Rejected because it violates the current privileged-identity boundary and unnecessarily expands the services handling customer content.

**Alternative considered:** Put a service key in the browser. Rejected because a reusable bypass credential cannot be safely constrained to the UI session and would turn browser compromise into durable tenant-data compromise.

### 2. Make policy preview read-only and administrative mode explicit

Policy preview creates a delegated context for a selected active application user. It supports get/query and non-committing create/update/delete evaluation against the current policy and schema but cannot commit. The UI always names the selected preview identity and policy version while avoiding any claim that this is the user's authenticated session.

Administrative mode requires a distinct data-administration permission, an access reason, confirmation, and a grant with a short maximum lifetime. Every request references the grant and receives enhanced audit. Reads and writes display a persistent warning. Project role defaults will grant administrative document access only to owners and administrators; organizations can later gain finer custom roles without changing the capability contract.

**Alternative considered:** Let all developers browse with an implicit bypass. Rejected because it hides a significant authorization boundary and makes access review and least privilege impossible.

### 3. Add an implicit canonical primary-key browse index

Every collection will expose a bounded, snapshot-consistent ordering over canonical document IDs for the explorer and internal administration. The ordering is tenant-, environment-, and collection-prefixed, excludes tombstones unless explicitly requested, and uses an opaque cursor bound to snapshot, schema version, mode, query fingerprint, and scope. Existing active indexes remain mandatory for filters and non-primary sorts.

The primary-key browse contract is implemented in the document engine, not by decoding a broad RocksDB scan in the HTTP layer. It shares page and byte limits with indexed queries and policy filtering continues until the page is filled or the captured range is exhausted.

**Alternative considered:** Require customers to create a custom index before seeing any documents. Rejected because it makes an empty explorer the default and does not meet the basic database-console experience.

### 4. Provide a deterministic query-plan endpoint

The query builder emits the same predicate and sort model used by the document engine. A planning endpoint validates field types against the active schema, matches active index prefixes, chooses one deterministic eligible index, reports the effective order and limit, or returns the minimal required index shape. It performs no document read and is safe to call before execution.

The explorer will not support arbitrary JSON selectors beyond the engine's equality and bounded-range operators. Missing-index failures link to the existing index-management page with a prefilled, reviewable definition; index creation remains a separate authorized action.

**Alternative considered:** Fall back to a collection scan for convenience. Rejected because scan cost is unpredictable and could degrade the single-node RocksDB service.

### 5. Preserve revision and idempotency semantics in the UI

Document drafts retain the revision and schema version they were opened against. The browser generates a unique mutation/idempotency identifier and submits explicit create, update, or delete input. The data plane revalidates grant, schema, policy/bypass, current revision, size, quota, and idempotency inside the existing mutation path.

Conflicts return only current content the active mode is allowed to read. The UI displays original, proposed, and current JSON in an explicit comparison and never auto-merges or auto-retries. Delete creates the existing tombstone form. A schema-aware formatted text editor with deterministic parse/format/validation behavior is sufficient initially; richer editor widgets can be added without changing the API.

### 6. Treat import and export as durable data jobs

Large data transfer will use durable, tenant-scoped job records and a bounded worker rather than browser loops. The portable format is versioned UTF-8 JSON Lines plus a manifest containing tenant scope, collection, schema version, snapshot or dry-run basis, row count, byte count, digest, and timestamps. Import upload and export download use short-lived, tenant-bound artifact grants; immutable artifacts are encrypted by the configured object-storage boundary and expire through a recorded retention policy.

Import has separate upload, dry-run, confirmation, execution, and terminal states. Dry run validates format, schema, primary keys, metadata, limits, and expected conflict classes without writes. Execution converts each row into an existing conditional, idempotent document mutation. Cancellation stops future rows but does not roll back already committed documents; the UI states this before confirmation and reports exact committed/failed/skipped counts.

Export captures a consistent snapshot and streams bounded pages to a temporary artifact. Failed or cancelled exports never expose partial output. Download authorization is rechecked when the short-lived artifact grant is created.

**Alternative considered:** Perform imports and exports entirely in the browser. Rejected because navigation, token expiry, network interruption, and large collections make progress and retry semantics unreliable.

### 7. Derive connection guidance from live configuration and published SDK metadata

The API & Connect page reads public endpoint, active public key metadata, collection/schema metadata, and the published RxDB-client compatibility matrix from management APIs. Code examples are generated from versioned templates tested against the exported client API; secret values use placeholders and only the explicitly public key is copyable.

Connection check is a server-coordinated sequence of bounded, non-document probes for DNS/TLS, public routing, readiness, key recognition, schema compatibility, and replication-route availability. It does not mint an application-user session or perform a pull.

**Alternative considered:** Hard-code snippets and endpoints in the frontend. Rejected because hosted routes, active keys, schema versions, and supported client APIs can drift independently.

### 8. Reuse retained observability for the sync dashboard

The developer sync dashboard will query the existing tenant-scoped observability service through dedicated aggregate shapes for pull/push results, streams, lag, conflicts, policy denials, checkpoint expiry, resync, schema mismatch, and bounded client-version classes. Raw user IDs, device IDs, tokens, document IDs, selectors, and payloads are excluded. Each response carries retention and observation timestamps.

The overview consumes small independent summaries rather than loading full observability pages. One failed summary produces a local degraded state rather than failing the workspace.

### 9. Limit developer recovery to a new isolated environment

Backup inventory is filtered server-side using verified backup manifests and returns only project/environment recovery points and safe evidence. A developer restore request creates a new recovery environment in the same project and references the verified backup and tenant inventory. The recovery orchestrator restores and verifies the target before it becomes accessible.

The developer cannot select an existing storage path, overwrite an environment, or promote restored storage. Those operations remain behind the separately authorized operator recovery workflow. Restore request and progress contain no physical paths or signing material.

**Alternative considered:** Allow project owners to overwrite production directly. Rejected because single-node local RocksDB recovery has large availability and data-loss consequences and requires platform-level verification.

### 10. Organize the console around project and environment context

The authenticated shell will gain a persistent organization/project/environment switcher and nested project routes. Environment changes clear explorer capabilities, document drafts, cursors, preview identities, and incompatible query state before new data loads. Safe filters and time ranges may remain in URLs; capabilities, reasons, email addresses, document bodies, and secrets may not.

The initial route hierarchy is:

```text
/projects/:projectId/environments/:environmentId
  /overview
  /data/:collectionId/:documentId?
  /collections/:collectionId?
  /sync
  /users
  /policies
  /functions
  /observability
  /backups/:jobId?
  /connect
  /settings
```

Existing URLs will redirect into the corresponding nested destination while preserving authorized context.

## Risks / Trade-offs

- **[Developer document access increases the customer-data attack surface]** → Use narrowly scoped expiring grants, server-side RBAC, no service credentials in browsers, enhanced audit, redaction tests, and rapid authorization-epoch revocation.
- **[Policy preview could be mistaken for true user impersonation]** → Keep it read-only, label the selected user and policy version persistently, prohibit session-token minting, and audit preview creation/use.
- **[Filtering can be surprising when policies hide rows]** → Identify the active mode and policy version, avoid protected counts, and explain that pages contain authorized results through a captured high water rather than raw index cardinality.
- **[Import can partially succeed across documents]** → Require dry run and explicit conflict mode, make each mutation atomic/idempotent, report committed/failed/skipped counts, and state that cancellation does not undo prior rows.
- **[Exports create sensitive secondary artifacts]** → Encrypt tenant-bound immutable artifacts, use short retention and download grants, audit downloads, validate digest, and remove incomplete output.
- **[Primary-key browsing adds another access path]** → Implement it inside the same scoped engine and authorization pipeline, with bounded snapshots, cursors, quota charging, and cross-tenant tests.
- **[Restore requests consume scarce single-node capacity]** → Restore only into quota-approved isolated targets, require verified evidence and step-up, and leave overwrite/promotion to operators.
- **[The combined workspace is large]** → Deliver the shell, connection page, overview, and read-only explorer first; then mutations, sync detail, jobs, and recovery behind independent gates.

## Migration Plan

1. Add the explorer capability format, permission checks, revocation epoch, primary-key browse contract, query planner, and audit events behind disabled feature gates.
2. Add data-plane explorer routes, management grant issuance, SDK/OpenAPI types, and adversarial cross-tenant tests without exposing console navigation.
3. Add the project shell, overview, API & Connect page, connection check, and read-only policy-preview/administrative explorer in a qualification environment.
4. Enable conditional administrative mutations after schema, revision-conflict, idempotency, bypass-audit, and support-mode tests pass.
5. Add export and import job keyspaces, worker, artifact retention, progress UI, and separate feature gates; qualify quotas, interruption, cancellation, and redaction.
6. Add sync summaries, backup inventory, and isolated developer restore requests, reusing the qualified recovery orchestrator.
7. Redirect legacy project URLs only after navigation, permissions, and saved-link compatibility tests pass.

Rollback disables grant issuance and the new routes, returns users to existing project pages, and stops accepting new data jobs or restore requests. In-progress jobs remain visible to operators for safe completion or cancellation; no rollback deletes committed documents, audit records, or artifacts before their declared retention expires.
