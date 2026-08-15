## 1. Contracts and Threat Model

- [x] 1.1 Update the threat model and abuse-case registry for explorer grants, policy preview, administrative bypass, primary-key browse, imports, exports, artifact downloads, connection checks, and developer restore requests.
- [x] 1.2 Define project data-read, data-admin, document-history, import, export, backup-read, and restore-request permissions and map existing organization roles to least-privilege defaults.
- [x] 1.3 Define OpenAPI schemas for explorer grant issuance/revocation, browse/query/plan/get/history/simulate/mutate, data jobs, workspace summaries, connection checks, sync summaries, backup inventory, and restore requests.
- [x] 1.4 Define shared limits for grants, documents, predicates, sorting, pages, uploads, artifacts, jobs, durations, downloads, and restore requests with safe error codes and retry guidance.
- [x] 1.5 Add adversarial contract tests for cross-organization identifiers, mode confusion, cursor substitution, expired grants, revoked membership, missing permissions, secret leakage, and generic not-found behavior.

## 2. Explorer Capability Authorization

- [x] 2.1 Implement signed explorer capability claims with developer identity, tenant/collection scope, access mode, allowed operations, reason hash, nonce, authorization epoch, and bounded expiry.
- [x] 2.2 Implement control-plane grant issuance that revalidates developer status, membership, role permission, project/environment state, collection existence, and selected application user.
- [x] 2.3 Implement grant revocation and authorization-epoch advancement for membership, role, project, environment, collection, and application-user lifecycle changes.
- [x] 2.4 Implement data-plane grant validation with strict issuer/audience, signature, expiry, nonce, epoch, scope, mode, and operation checks.
- [x] 2.5 Ensure explorer grants remain in memory only and add console tests proving they never enter URLs, local/session storage, error reports, analytics, or logs.
- [x] 2.6 Add key rotation, clock-boundary, replay, stale epoch, wrong audience, wrong collection, and revoked-user tests for explorer capabilities.

## 3. Primary-Key Browse and Query Planning

- [x] 3.1 Add tenant- and collection-scoped canonical primary-key browse primitives to the document engine with consistent snapshots, bounded pages, byte limits, and opaque cursors.
- [x] 3.2 Filter deleted documents by default and add explicitly authorized retained-tombstone browsing without bypassing retention checks.
- [x] 3.3 Apply read authorization while filling browse pages through the captured high water so policy-hidden documents do not leak through counts, cursors, or short pages.
- [x] 3.4 Bind browse and query cursors to project, environment, collection, mode, query fingerprint, schema version, snapshot, and authorization epoch.
- [x] 3.5 Implement deterministic query planning that validates schema field types, matches active index prefixes, selects the eligible index, and returns effective ordering and limits.
- [x] 3.6 Return a safe minimal required-index shape for unsupported queries and prohibit fallback collection scans.
- [x] 3.7 Add document-engine tests for concurrent writes, snapshot consistency, tombstones, policy filtering, cursor tampering, schema changes, index lifecycle, and realistic browse cardinality.

## 4. Explorer Data-Plane APIs

- [x] 4.1 Add dedicated capability-authenticated explorer routes for primary-key get, browse, indexed query, and query plan.
- [x] 4.2 Add bounded document revision metadata and retained-tombstone routes protected by document-history permission and current retention state.
- [x] 4.3 Add non-committing create/update/delete simulation routes that reuse current schema and policy evaluation without issuing a sequencer position or storage write.
- [x] 4.4 Add conditional create/update/delete routes that reuse existing validation, durability, index, change-log, idempotency, quota, and conflict semantics.
- [x] 4.5 Ensure policy-preview routes map to the selected verified application-user context and administrative routes use an explicit scoped privileged authorizer.
- [x] 4.6 Add safe conflict responses that include current content only when the active explorer mode can read it and never auto-merge or retry.
- [x] 4.7 Add route integration tests for schema mismatch, invalid primary key, policy denial, bypass audit, idempotent replay, revision conflict, deletion, quota failure, and storage failure.

## 5. Explorer Audit and Telemetry

- [x] 5.1 Add explorer audit actions for grant issue/revoke/use, preview read/query/simulation, administrative read/query/history/simulation/mutation, import/export, artifact access, and denied attempts.
- [x] 5.2 Record developer actor, tenant and collection scope, grant, reason, operation, target or query fingerprint, index, outcome, request, and timestamp without document bodies or secrets.
- [x] 5.3 Add bounded metrics for explorer requests, modes, outcomes, latency, grant failures, query-plan failures, conflicts, and job states without user, document, token, or raw email labels.
- [x] 5.4 Add redaction and audit-continuity tests covering successful, denied, failed, expired, and retried explorer operations.

## 6. Data Import and Export Jobs

- [x] 6.1 Define versioned tenant-scoped job, manifest, progress, error-summary, cancellation, retention, and artifact-address records for JSON Lines import/export.
- [x] 6.2 Add additive RocksDB keyspaces and indexes for listing data jobs by project, environment, collection, creator, type, state, and creation time.
- [x] 6.3 Extend the object-store address contract for tenant-bound immutable data-job uploads and outputs with digest verification and retention metadata.
- [x] 6.4 Implement short-lived upload and download grants that reauthorize tenant scope, job state, membership, permission, expiry, and artifact digest at use time.
- [x] 6.5 Implement import upload parsing and a non-committing dry run for format, manifest, schema, primary key, metadata, size, quota, and expected conflict classes.
- [x] 6.6 Implement create-only, update-existing, and upsert import execution through per-row conditional idempotent mutations with bounded concurrency and restart-safe progress.
- [x] 6.7 Implement consistent-snapshot query/collection export streaming with row/byte/duration quotas, manifest finalization, digest verification, and no partial downloadable artifact.
- [x] 6.8 Implement cancellation, expiry cleanup, orphan recovery, and exact committed/failed/skipped/exported progress reporting.
- [x] 6.9 Add failure-injection tests for malformed lines, oversized values, concurrent changes, duplicate retries, worker restart, object-store outage, cancellation, quota exhaustion, download expiry, and tenant substitution.

## 7. Project Workspace and Overview APIs

- [x] 7.1 Implement lightweight independent workspace summaries for lifecycle/readiness, counts, usage/quotas, sync, recent safe errors, function deployment, backups, and audit activity.
- [x] 7.2 Include observation time, freshness, retention, and provider status in every summary and preserve successful sections when another provider fails.
- [x] 7.3 Implement workspace navigation metadata for organizations, projects, environments, lifecycle states, and permission-filtered destinations.
- [x] 7.4 Add tests for environment switching, stale summaries, partial provider outage, permission filtering, cross-tenant isolation, and bounded response size.

## 8. API & Connect Experience APIs

- [x] 8.1 Implement management APIs for the public environment endpoint, active public-key metadata, collections, active schema versions, and supported RxDB-client compatibility ranges.
- [x] 8.2 Add versioned RxDB setup templates and tests that compile or type-check generated examples against the published client package API.
- [x] 8.3 Implement a bounded connection check for DNS/TLS, public routing, environment readiness, public-key recognition, schema compatibility, and replication-route availability without reading documents.
- [x] 8.4 Add safe remediation codes for missing keys, route failure, unready environment, schema mismatch, unsupported client, and transient dependency failure.
- [x] 8.5 Add tests proving connection checks cannot mint application-user sessions, invoke pull/push, expose internal routes, or return service credentials and secrets.

## 9. Developer Sync Diagnostics

- [x] 9.1 Add tenant-scoped aggregate shapes for pull/push activity, live streams, lag, conflicts, policy denials, throttling, checkpoint expiry, stream gaps, resync, and schema mismatch.
- [x] 9.2 Add bounded client-version classes and compatibility evaluation without raw device, user, session, IP, token, or document identifiers.
- [x] 9.3 Implement sync summary queries with enforced time windows, cursor pagination where needed, observation/retention metadata, and quota-safe cardinality.
- [x] 9.4 Add remediation mapping for retryable service conditions versus non-retryable client, schema, policy, and credential configuration errors.
- [x] 9.5 Extend observability validation and tests to prohibit sensitive/high-cardinality labels and cross-project results.

## 10. Developer Backup and Restore Requests

- [x] 10.1 Implement tenant-filtered backup inventory from verified manifests with recovery point, verification, retention, restore-drill, and safe objective status.
- [x] 10.2 Exclude physical paths, infrastructure inventory, credentials, signing material, unrelated tenants, and unverified scope from developer responses.
- [x] 10.3 Implement stepped-up restore requests that create only a new isolated recovery environment within the authorized project and bind a verified backup and quota impact.
- [x] 10.4 Connect developer restore requests to the guarded recovery orchestrator and expose safe progress without allowing overwrite or promotion.
- [x] 10.5 Block restored-environment access until tenant isolation, storage verification, service readiness, and recovery validation succeed.
- [x] 10.6 Add tests for wrong-tenant manifests, unverified backup, stale step-up, duplicate requests, existing-target injection, quota failure, verification failure, and prohibited promotion.

## 11. Developer Console Workspace Shell

- [x] 11.1 Add nested project/environment routes and persistent responsive navigation for overview, data, collections, sync, users, policies, functions, observability, backups, connect, and settings.
- [x] 11.2 Add an organization/project/environment switcher that clears capabilities, cursors, drafts, selected preview user, and incompatible state before loading the new environment.
- [x] 11.3 Add shared breadcrumbs, status/freshness, partial-error, empty-state, table, cursor-pagination, confirmation, job-progress, and permission-aware navigation components.
- [x] 11.4 Implement safe legacy URL redirects into the new hierarchy without placing developer emails, tokens, reasons, document content, or secrets in URLs.
- [x] 11.5 Add keyboard, focus, screen-reader, responsive-layout, direct-route authorization, and environment-switch data-remanence tests.

## 12. Data Explorer Console

- [x] 12.1 Build collection selection, exact primary-key lookup, canonical browse, query-builder, query-plan, active-index, result-limit, and cursor-pagination experiences.
- [x] 12.2 Build policy-preview grant creation with application-user selection, persistent preview identity/policy labeling, read/query, and non-committing mutation simulation.
- [x] 12.3 Build administrative grant entry with permission check, reason, confirmation, expiry, revocation, and persistent warning state.
- [x] 12.4 Build formatted schema-aware JSON view/create/edit/delete experiences with client-side parse validation and authoritative server diagnostics.
- [x] 12.5 Build explicit original/proposed/current conflict comparison with reload, cancel, and prepare-new-update paths and no automatic merge.
- [x] 12.6 Build revision metadata and retained-tombstone views with current/historical/deleted distinctions and retention status.
- [x] 12.7 Build missing-index guidance that links to a prefilled but separately reviewable index-creation workflow.
- [x] 12.8 Build import upload, dry-run results, conflict-strategy confirmation, progress, cancellation, terminal summary, and bounded error views.
- [x] 12.9 Build export scope confirmation, progress, cancellation, manifest summary, expiring download, and artifact-integrity views.
- [x] 12.10 Add end-to-end tests for mode separation, capability expiry, hidden rows, browse/query, mutation simulation, CRUD, conflicts, tombstones, import/export, audit references, and accessibility.

## 13. Overview, Connect, Sync, and Recovery Console

- [x] 13.1 Build the database project overview with independently loading readiness, usage, sync, error, deployment, backup, and activity summaries.
- [x] 13.2 Build the API & Connect page with public-only credential display, collection/schema selection, supported-version guidance, copyable RxDB examples, and setup-completeness checks.
- [x] 13.3 Build the connection-check workflow with step-by-step safe results and remediation links.
- [x] 13.4 Build the RxDB sync dashboard with environment/collection/time filters, activity, lag, conflicts, policy denials, reset causes, compatibility, and remediation guidance.
- [x] 13.5 Build tenant-scoped backup inventory and verified recovery-point detail without infrastructure-sensitive fields.
- [x] 13.6 Build the stepped-up isolated recovery-environment request and progress experience with explicit prohibition of overwrite and promotion.
- [x] 13.7 Add end-to-end tests for partial summary outages, snippet correctness, missing public key, schema mismatch, unsupported client, sync filtering, backup isolation, and restore-request safeguards.

## 14. Qualification and Rollout

- [x] 14.1 Update developer documentation, API references, RxDB quickstart, policy-preview guidance, data-administration warnings, import/export format, limits, backup recovery, and support runbooks.
- [x] 14.2 Run Rust and console formatting, linting, unit, integration, type, accessibility, OpenAPI, SDK-parity, observability-contract, redaction, and cross-tenant test suites.
- [x] 14.3 Qualify capability rotation/revocation, browser-storage inspection, CSP behavior, audit continuity, primary-key browse load, and provider-failure handling in a production-like environment.
- [x] 14.4 Deploy the workspace shell, overview, Connect page, and read-only explorer behind feature gates while preserving legacy routes as rollback.
- [x] 14.5 Qualify and enable administrative mutations, import, export, sync detail, and restore requests as separate gates with documented quotas and audit evidence.
- [x] 14.6 Verify public HTTPS routing and route allowlists expose only intended developer endpoints and continue blocking internal, service-credential, operator, health, and artifact-storage routes.
- [x] 14.7 Roll out legacy URL redirects only after saved-link, permission, session-expiry, environment-switch, and rollback tests pass.
