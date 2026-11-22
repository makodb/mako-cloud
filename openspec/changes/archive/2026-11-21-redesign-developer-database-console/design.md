## Context

The console has project management, collection/schema/index editors, scoped explorer grants, usage, sync, backups, and application services. See proposal.md for the user-facing problem. This design includes the subsequent shipped decisions for project lists, personal/team separation, shared billing, and direct browsing. The original redesign's test record remains in verification.md; it does not describe every later adjustment.

## Goals / Non-Goals

Goals: make existing database workflows obvious and useful on desktop and mobile; return actionable tenant-scoped summaries; prevent stale results and drafts crossing environments.

Non-goals: storage engine replacement, SQL support, independent project plans or payment collection, removing data-administration permission checks, or inventing utilization/latency charts.

## Decisions

1. Retain the React/shared-UI stack and existing routes. Add a reusable grouped destination presentation and direct database links on home and project pages. Keep old bookmarks and normal modified-click behavior. A new router or component library would increase migration cost without improving these workflows.
2. Home keeps per-owner authorization and bounded summary requests. Render project rows under separate Personal projects and Teams sections, with a root Personal projects destination and only collaborative teams in team navigation and selectors. Keep the internal personal owner for authorization, billing, and transfers. Omit personal-project search and the sidebar Documentation link. Choose one creation entry point for the populated, empty, or onboarding state and link directly to environment tools using already-loaded metadata.
3. Environment overview reads the summary and collection list independently. Render lifecycle readiness as lifecycle readiness, counts with bounds, and usage as reported samples. Activity contains only scrubbed audit metadata. Server payload extensions remain optional so old releases remain readable.
4. Explorer retains its grant and conditional-write protocol. For personal and team projects, the console obtains and renews a short-lived collection grant in memory and loads documents without an access form or application-user selection. The service still enforces data-administration permission and audits access; explicit policy preview remains in the API and CLI. Add a collection rail and schema/content columns for only documents returned by authorized reads. Object values are bounded previews and React text, never HTML. Preserve query pagination, history, conflict handling, and jobs.
5. Key the authenticated content by project/environment identity so navigation discards old data and revokes grants. Keep environment metadata loading separate from navigation failures. Dashboard providers refresh every 60 seconds without overlapping requests. Show relative observation ages instead of Current/Stale badges. Keep last successful values and their timestamps during transient refresh failures, show the error, and clear retained values on access loss. Cancel polling and ignore pending responses when the view or environment changes.
6. Read project costs from the project billing API across all its environments. The API allocates each owner's rated resource charges by project usage, preserving totals through deterministic rounding. Plans and allowances remain shared by the personal account or team; base fees, credits, and balances appear on overall Usage and plan. Name the period and retention coverage, and keep failed billing visible as unavailable.
7. Reuse Playwright fixtures for project lists, personal/team separation, one create action, absence of personal search, project billing, automatic collection access, populated/empty/error/race cases, and desktop, mobile, and dark screenshots. Rust and service tests cover summary filtering, bounded output, cost allocation, authorization, and management routes.

## Risks / Trade-offs

- Bounded telemetry does not give lifetime totals. Label samples and time windows, propagate truncation, and link to detailed usage.
- A schema can contain many or nested fields. Limit initial columns and provide the full JSON inspector.
- Changing navigation can break browser tests that assume a flat list. Keep stable route semantics and update assertions only for intentional behavior changes.
- Existing shared worktree changes can overlap. Edit only the relevant sections and review the resulting diff.

## Migration Plan

Build and test locally, document the workflows, then ship through the immutable release process when deploying this change. Include both the registration and public-preview Ansible overrides for this environment. API changes are additive; rollback can select the prior console and service release without a data migration.
