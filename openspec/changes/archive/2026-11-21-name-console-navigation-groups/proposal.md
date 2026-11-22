## Why

`cloud/developer-console`, "Navigation shell", fixes the environment shell's
destinations as eleven items: "Database (collections, schemas, indexes),
Explorer, Auth (application users, providers), Storage, Functions
(deployments, secrets, schedules), Sync (replication diagnostics, webhooks),
Logs, Observability, API & Keys, Backups, and Settings".

The User Book describes a different shell: the sidebar "groups destinations
under **Database**, **Application**, **Observe**, and **Configure**", and its
table lists twenty destinations. Overview, Policies, Email templates, Usage,
Activity and API docs are missing from the spec; the spec's "Explorer" is
"Data" in the book; "Auth" is two destinations, Users and Auth providers;
Webhooks is its own destination; and "API & Keys" is "API & Connect" plus
"API keys". The console shows the book's four groups and twenty destinations
(checked on the running console on 2026-11-21).

`cloud/developer-database-workspace`, "Grouped navigation and isolated
environment state", names the groups "database, application, operations, and
configuration", where the book and the console say Observe.

## What Changes

- "Navigation shell" names the four groups and the twenty destinations, says
  navigation follows the permissions the platform returns, and says a narrow
  screen selects a destination through a "Go to" control.
- "Grouped navigation and isolated environment state" uses the same four
  group names.

No scenario is added or renamed, so the traceability matrix does not change.
No code changes.

## Capabilities

### New Capabilities

### Modified Capabilities

- `cloud/developer-console`: the navigation shell's groups and destinations
  are the ones the User Book lists.
- `cloud/developer-database-workspace`: the same four group names.

## Impact

- `openspec/specs/cloud/developer-console/spec.md`
- `openspec/specs/cloud/developer-database-workspace/spec.md`
