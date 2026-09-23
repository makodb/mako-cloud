## Context

The console already has project management, collection/schema/index editors, scoped explorer grants, usage, sync, backups, and application services. Its home, project, and environment pages use different navigation and bury the database entry points. See proposal.md for the user-facing problem. Existing uncommitted homepage and CLI changes must be preserved.

## Goals / Non-Goals

Goals: make existing database workflows obvious and useful on desktop and mobile; return actionable tenant-scoped summaries; prevent stale results and drafts crossing environments.

Non-goals: storage engine replacement, SQL support, new billing semantics, automatically granting privileged document access, or inventing utilization/latency charts. The current request authorizes implementation and tests; deployment can follow as a separate release.

## Decisions

1. Retain the React/shared-UI stack and existing routes. Add a reusable grouped destination presentation and direct database links on home and project pages. Keep old bookmarks and normal modified-click behavior. A new router or component library would increase migration cost without improving these workflows.
2. Home keeps existing per-owner authorization and bounded summary requests, adding search, a workspace sidebar, an explicit create form, and direct environment tools using already-loaded environment metadata.
3. Environment overview reads the summary and collection list independently. Render lifecycle readiness as lifecycle readiness, counts with bounds, and usage as reported samples. Activity contains only scrubbed audit metadata. Server payload extensions remain optional so old releases remain readable.
4. Explorer retains its grant and conditional-write protocol. Add a collection rail and schema/content columns for only documents returned by authorized reads. Object values are bounded previews and React text, never HTML. Preserve query pagination, history, simulation, conflict handling, and jobs.
5. Key the authenticated content by project/environment identity so navigation discards old data and revokes grants. Keep environment metadata loading separate from navigation failures. Failed providers stay visible as unavailable; expired observations display stale.
6. Reuse the current Playwright fixtures for regression tests, add realistic populated/empty/error/race cases, and capture desktop, mobile, and dark screenshots. Rust unit tests verify summary filtering and bounded output; service tests cover authorization and summary routes where available.

## Risks / Trade-offs

- Bounded telemetry does not give lifetime totals. Label samples and time windows, propagate truncation, and link to detailed usage.
- A schema can contain many or nested fields. Limit initial columns and provide the full JSON inspector.
- Changing navigation can break browser tests that assume a flat list. Keep stable route semantics and update assertions only for intentional behavior changes.
- Existing shared worktree changes can overlap. Edit only the relevant sections and review the resulting diff.

## Migration Plan

Build and test locally, document the workflows, then ship through the immutable release process when deploying this change. Include both the registration and public-preview Ansible overrides for this environment. API changes are additive; rollback can select the prior console and service release without a data migration.
