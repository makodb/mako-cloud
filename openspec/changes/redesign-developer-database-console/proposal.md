## Why

The signed-in console hides database work behind project and environment landing pages. Its overview renders provider payloads as generic fields and its document table omits document content, leaving useful backend capabilities difficult to find and use.

## What Changes

- Give the home dashboard a persistent workspace sidebar, project search, an obvious create action, and direct links to each project's database tools.
- Group environment navigation around database, application services, operations, and project configuration. Preserve context and existing deep links.
- Replace the generic environment overview with named metrics, a collection inventory, setup actions, and bounded recent activity backed by management APIs.
- Extend server summary payloads with explicit observation windows, bounded activity, and usage samples without disclosing document bodies or credentials.
- Make the data explorer a database workbench with collection navigation, schema-derived document columns, and the existing query, conditional editing, history, and import/export workflows.
- Clear scoped UI state when navigating between projects and environments; preserve access grants, policy preview, audit, and conditional writes.
- Verify desktop, mobile, dark theme, empty accounts, partial outages, context switches, and data workflows.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `cloud/developer-console`: Searchable project landing page with direct database entry and visible workspace actions.
- `cloud/developer-database-workspace`: Task-oriented summaries and grouped navigation with safe context switching.
- `cloud/developer-data-explorer`: Collection navigation and document fields displayed in the results table.

## Impact

React console, management summary and navigation handlers, browser tests, Rust tests, and user documentation. Existing REST routes, SDK types, tenant authorization, storage engines, and public homepage remain compatible. This changes the hosted management experience and supporting APIs; it does not replace the database engine or claim unmeasured health or performance.
