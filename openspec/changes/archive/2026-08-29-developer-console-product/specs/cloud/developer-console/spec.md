## Purpose

What a developer sees from sign-in onward: the home dashboard, project home, the navigation shell every product area lives in, first-run onboarding, and the settings, usage, billing, and activity that belong where the developer is working.

## ADDED Requirements

### Requirement: Home dashboard after sign-in
After sign-in the console SHALL show a home dashboard listing every project the developer can reach — across the personal space and every team they belong to — as cards carrying the project's name, owner, lifecycle state, region, plan, and headline usage for the current period, alongside the developer's recent activity. The dashboard MUST show only projects and activity the current memberships allow, MUST distinguish unavailable summaries from healthy ones, and MUST NOT require choosing a team before showing projects.

#### Scenario: A developer with projects signs in
- **WHEN** a developer who belongs to a team with projects and has individual projects signs in
- **THEN** the home dashboard lists all of those projects as cards grouped by owner, each showing state, region, plan, and usage, and shows their recent activity

#### Scenario: A summary source is unavailable
- **WHEN** usage or activity cannot be loaded for one project
- **THEN** that card marks the missing summary unavailable and every other card is unaffected

### Requirement: First-run onboarding
A developer with no reachable projects SHALL be offered a guided first run that creates a project — in the personal space or a team they may create in — and leads to a connected client: a copyable public key and API URL, a client snippet, and a connection check, in one flow. Onboarding MUST be dismissible, MUST resume where the developer left it, and MUST NOT create anything without the developer's explicit action.

#### Scenario: A new developer completes onboarding
- **WHEN** a developer with no projects signs in and follows the guided first run
- **THEN** a project exists where they chose, its environment becomes active, and the flow ends on a successful connection check with the credentials they need shown once

#### Scenario: Onboarding is dismissed
- **WHEN** a developer dismisses the guided first run
- **THEN** the home dashboard shows its ordinary empty state with a create-project action, and the guide can be reopened

### Requirement: Project home
A project SHALL have a home page summarizing its environments and their lifecycle, endpoint readiness, keys and API URL for the selected environment, a quickstart, usage and quota position, data-plane health, and recent activity, with the same navigation destinations the environment level offers. Every summary MUST identify its observation time.

#### Scenario: A developer opens a project
- **WHEN** an authorized developer opens a project with an active environment
- **THEN** the project home shows that environment's readiness, keys, usage, health, and activity, and each product area is reachable from it

### Requirement: Navigation shell
Every environment-level screen SHALL be mounted in a persistent navigation shell whose destinations are Database (collections, schemas, indexes), Explorer, Auth (application users, providers), Storage, Functions (deployments, secrets, schedules), Sync (replication diagnostics, webhooks), Logs, Observability, API & Keys, Backups, and Settings. The active team, project, environment, and developer identity MUST remain visible; destinations the deployment does not provide MUST be shown as unavailable rather than hidden; and an existing deep link MUST open inside the shell with its context intact.

#### Scenario: Every destination is one click away
- **WHEN** an authorized developer is anywhere inside an environment
- **THEN** each destination is reachable from the shell without returning to a parent page, and the current destination is indicated

#### Scenario: A deep link opens inside the shell
- **WHEN** a developer opens a bookmarked environment URL
- **THEN** the screen renders inside the shell with the team, project, and environment context shown

### Requirement: Served capabilities are visible in the console
Capabilities the management API already serves SHALL be reachable in the console: retained project logs with time and level filters, index build state, data-job detail, JWT signing-key initialization, team renaming, and the team's current plan and credit balance.

#### Scenario: A developer reads retained logs
- **WHEN** an authorized developer opens Logs for an environment
- **THEN** the retained, scrubbed log lines are listed newest first with level and time filters and paginate through the retention window

#### Scenario: A developer renames a team
- **WHEN** a team administrator renames the team from its page
- **THEN** the new name is saved, audited, and shown everywhere the team appears

### Requirement: Usage and billing where developers work
The console SHALL show usage against quota per project and environment, and the bill and balance per team, including the personal space, using the same figures the management API serves. Every money figure MUST carry the non-payable notice the API carries.

#### Scenario: A developer checks a project's usage
- **WHEN** an authorized developer opens usage for a project
- **THEN** each metered resource shows the period's quantity against the plan's allowance with the period named

### Requirement: Developer activity feed
The console SHALL show an activity feed for a project and for a team derived from the audit trail, naming actor, action, target, and time, restricted to what the developer's memberships allow to be read.

#### Scenario: A developer reviews recent activity
- **WHEN** an authorized developer opens activity for a project
- **THEN** recent audited actions are listed newest first and each names actor, action, target, and time

### Requirement: Project settings
A project's settings SHALL offer renaming, transfer between the personal space and teams the developer administers, and deletion with grace, each requiring explicit confirmation and each audited. Settings MUST show the owning team or personal space, region, and identifiers.

#### Scenario: A developer transfers a project to a team
- **WHEN** a developer who owns a project in their personal space transfers it to a team they administer and confirms
- **THEN** the project appears under that team, its policies, users, and data are unchanged, and the transfer is audited on both sides

#### Scenario: A developer deletes a project
- **WHEN** a project owner confirms deletion in settings
- **THEN** the project enters its grace period, the deadline is shown, and restoration remains offered until then
