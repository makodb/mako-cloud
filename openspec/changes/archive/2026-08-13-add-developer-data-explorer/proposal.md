## Why

The developer console can configure collections, policies, credentials, users, functions, and observability, but developers cannot inspect or safely operate the documents their applications synchronize through RxDB. A recognizable database-cloud experience needs a first-class data explorer plus a coherent project workspace for connecting clients, diagnosing synchronization, and managing recovery.

## What Changes

- Add a project/environment navigation shell and overview that summarizes readiness, usage, RxDB sync health, recent errors, deployments, and backup status.
- Add a collection Data Explorer for primary-key lookup and bounded indexed browse, filter, sort, and cursor pagination, with query planning that identifies the active index or the index needed to run a rejected query safely.
- Add schema-aware JSON document viewing, creation, editing, conditional update, deletion, tombstone/revision visibility, and explicit conflict handling.
- Add two conspicuously different access modes: policy-enforced application-user preview and authorized developer administrative access. Administrative access uses a server-side audited bypass and never exposes a service credential to the browser.
- Add policy-result and validation previews before mutation, together with guarded confirmation and audit for administrative writes.
- Add asynchronous import and export jobs with schema validation, dry-run results, conflict strategy, bounded resource use, progress, downloadable artifacts, expiry, and cancellation.
- Add an API & Connect experience that shows the environment endpoint, public client key, collection/schema information, RxDB setup snippets, and a live connection check without revealing service credentials or secrets.
- Add a developer-facing RxDB sync dashboard for replication activity, lag, conflicts, policy denials, checkpoint expiry, resynchronization, and client compatibility guidance.
- Add tenant-scoped backup inventory and a guarded developer restore-request workflow that can restore only authorized project environments through the platform recovery system.
- Exclude arbitrary collection scans, MongoDB/SQL compatibility, production query consoles, operator fleet administration, billing, subscriptions, and routine platform-operator access to documents.

## Capabilities

### New Capabilities

- `cloud/developer-data-explorer`: Let authorized project members browse, query, inspect, import, export, and conditionally mutate documents with explicit policy-enforced or audited administrative access semantics.
- `cloud/developer-database-workspace`: Provide project overview, navigation, RxDB connection guidance, synchronization diagnostics, and tenant-scoped backup and restore-request experiences.

### Modified Capabilities

None.

## Impact

- Expands the developer console routes, project navigation, collection experience, forms, tables, JSON editor, query builder, job progress, and accessibility coverage.
- Adds management-authorized document gateway APIs, query-plan diagnostics, import/export jobs, connection metadata, sync summaries, backup inventory, and restore requests.
- Reuses the document engine's revision, schema, index, query, tombstone, and idempotency contracts while adding a bounded primary-key browse path needed by the explorer.
- Extends project RBAC, delegated application-user preview, privileged-bypass audit, mutation audit, object/artifact retention, quota charging, and redaction behavior.
- Integrates with existing RxDB client metadata, observability records, backup evidence, and the guarded platform recovery workflow.
- Requires threat-model review because the console gains customer-document access and data import/export paths.
