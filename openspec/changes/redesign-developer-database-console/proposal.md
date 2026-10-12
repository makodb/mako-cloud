## Why

The signed-in console hides database work behind project and environment landing pages. Its overview renders provider payloads as generic fields and its document table omits document content, leaving useful backend capabilities difficult to find and use.

## What Changes

- Give the home dashboard project lists, separate root-level Personal projects and Teams navigation, one appropriate create action, and direct links to each project's database tools. Omit personal-project search and the Documentation sidebar link.
- Show each project's usage costs under its owner's shared plan, with account and team totals on the overall Usage and plan page.
- Group environment navigation around database, application services, operations, and project configuration. Preserve context and existing deep links.
- Replace the generic environment overview with named metrics, a collection inventory, setup actions, and bounded recent activity backed by management APIs.
- Extend server summary payloads with explicit observation windows, bounded activity, and usage samples without disclosing document bodies or credentials.
- Make the data explorer a database workbench with collection navigation, schema-derived document columns, and the existing query, conditional editing, history, and import/export workflows.
- Load documents directly for authorized personal and team project members, obtaining scoped grants automatically. Clear scoped UI state when navigating between projects and environments; retain permission checks, API/CLI policy preview, audit, and conditional writes.
- Verify desktop, mobile, dark theme, empty accounts, partial outages, context switches, and data workflows.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `cloud/developer-console`: Separate personal and team project lists with direct database entry, project billing, shared Usage and plan, and one creation entry point.
- `cloud/developer-database-workspace`: Task-oriented summaries and grouped navigation with safe context switching.
- `cloud/developer-data-explorer`: Collection navigation and document fields displayed through automatic authorized console access.

## Impact

React console, management summary and navigation handlers, browser tests, Rust tests, and user documentation. Existing REST routes, SDK types, tenant authorization, storage engines, and public homepage remain compatible. This changes the hosted management experience and supporting APIs; it does not replace the database engine or claim unmeasured health or performance.
