# Developer Console Specification

## Purpose

What a developer sees from sign-in onward: the home dashboard, project home, the navigation shell every product area lives in, first-run onboarding, and the settings, usage, billing, and activity that belong where the developer is working.

## Requirements

### Requirement: Home dashboard after sign-in
After sign-in the console SHALL show reachable projects in lists, with personal projects in a root-level section and team projects grouped separately under Teams. Each project row SHALL carry its name, owner, lifecycle state, region, shared plan, and headline usage for the current period. The dashboard MUST show only projects and activity the current memberships allow, distinguish unavailable summaries from healthy ones, and show personal projects without requiring a team selection. The internal personal space MUST NOT appear as a team in navigation or team-management lists.

#### Scenario: A developer with projects signs in
- **WHEN** a developer who belongs to a team with projects and has individual projects signs in
- **THEN** the home dashboard lists personal projects separately from team projects, with each row showing its owner, state, region, shared plan, and usage

#### Scenario: A summary source is unavailable
- **WHEN** usage or plan information cannot be loaded for one project
- **THEN** that project's row marks the missing summary unavailable and every other row is unaffected

### Requirement: First-run onboarding
A developer with no reachable projects SHALL be offered a guided first run that creates a project in their personal space or a team they may create in, then opens its workspace. The project workspace SHALL provide the provisioning state and connection setup, including public keys, API URL, client snippet, and connection check. Onboarding MUST be dismissible and reopenable, remember dismissal across reloads in the developer's browser session, and MUST NOT create projects or credentials without an explicit action.

#### Scenario: A new developer completes onboarding
- **WHEN** a developer with no projects signs in and follows the guided first run
- **THEN** a project is created with their chosen owner and region, the console opens its workspace to continue setup, and the home guide does not issue credentials automatically

#### Scenario: Onboarding is dismissed
- **WHEN** a developer dismisses the guided first run
- **THEN** the home dashboard shows its ordinary empty state with a create-project action, and the guide can be reopened

### Requirement: Project home
A project SHALL have a home page summarizing its environments and their lifecycle, endpoint readiness, keys and API URL for the selected environment, a quickstart, usage and quota position, data-plane health, and recent activity, with the same navigation destinations the environment level offers. Every summary MUST show a relative observation age such as "Updated 1 minute ago" instead of "Current" or "Stale" freshness badges. Project summaries SHALL refresh every 60 seconds without overlapping requests and retain their original observation times when a transient refresh failure leaves previous values visible.

#### Scenario: A developer opens a project
- **WHEN** an authorized developer opens a project with an active environment
- **THEN** the project home shows that environment's readiness, keys, usage, health, and activity, and each product area is reachable from it

### Requirement: Navigation shell
Every environment-level screen SHALL be mounted in a persistent navigation shell whose destinations are Database (collections, schemas, indexes), Explorer, Auth (application users, providers), Storage, Functions (deployments, secrets, schedules), Sync (replication diagnostics, webhooks), Logs, Observability, API & Keys, Backups, and Settings. The active personal or team ownership, project, environment, and developer identity MUST remain visible; destinations the deployment does not provide MUST be shown as unavailable rather than hidden; and an existing deep link MUST open inside the shell with its context intact. The shell and every destination SHALL render through the shared design system, and the shell SHALL honour the developer's light or dark theme preference, kept per device, defaulting to the device's own preference. Signed-in console sidebars MUST NOT include a Documentation destination.

#### Scenario: Every destination is one click away
- **WHEN** an authorized developer is anywhere inside an environment
- **THEN** each destination is reachable from the shell without returning to a parent page, and the current destination is indicated

#### Scenario: A deep link opens inside the shell
- **WHEN** a developer opens a bookmarked environment URL
- **THEN** the screen renders inside the shell with personal or team ownership, project, and environment context shown

#### Scenario: The console follows the developer's theme
- **WHEN** a developer chooses the dark theme and opens any destination or reloads
- **THEN** the shell and the destination render with the dark palette, and the choice is remembered on that device

### Requirement: Served capabilities are visible in the console
Capabilities the management API already serves SHALL be reachable in the console: retained project logs with time and level filters, index build state, data-job detail, JWT signing-key initialization, team renaming, and the team's current plan and credit balance.

#### Scenario: A developer reads retained logs
- **WHEN** an authorized developer opens Logs for an environment
- **THEN** the retained, scrubbed log lines are listed newest first with level and time filters and paginate through the retention window

#### Scenario: A developer renames a team
- **WHEN** a team administrator renames the team from its page
- **THEN** the new name is saved, audited, and shown everywhere the team appears

### Requirement: Usage and billing where developers work
The console SHALL show usage against quota per project and environment and a project Billing page containing that project's current-period usage and allocated resource costs across all its environments. The page SHALL identify the owner's shared plan, period, retained-evidence coverage, observation time, and proportional resource-usage allocation. It MUST NOT present the shared plan's base fee, credits, or balance as project-specific charges or imply that every project has its own plan or included allowances.

A root-level Usage and plan page SHALL show combined usage, shared allowances, plan fees, credits, and balance separately for the personal account and each authorized team, with links to project billing. Project billing SHALL link back to that overall page. Every money display MUST carry the non-payable notice the API carries. Unavailable billing MUST remain an explicit failure rather than appearing as zero cost or falling back to the owner's bill.

#### Scenario: A developer checks a project's usage
- **WHEN** an authorized developer opens usage for a project
- **THEN** each metered resource shows the period's quantity against the plan's allowance with the period named

#### Scenario: Developer opens project billing
- **WHEN** an authorized developer opens Billing for a personal or team project
- **THEN** the page shows only that project's resource quantities and allocated costs across its environments, names the shared plan and period, and links to overall Usage and plan

#### Scenario: Developer checks the shared plan
- **WHEN** an authorized developer opens Usage and plan
- **THEN** personal-account and team totals appear separately with shared allowances, base fees, credits, balances, and links to each permitted project's cost breakdown

#### Scenario: Project billing is unavailable
- **WHEN** the project bill cannot be loaded
- **THEN** the page shows the request failure without presenting zero costs or substituting the owner's bill

### Requirement: Direct database entry from project lists
Project lists SHALL offer direct database, schema, and connection destinations for an active environment, naming that environment. The personal-project screen MUST NOT include project search. It SHALL provide one project-creation entry point appropriate to its populated, empty, or onboarding state, without duplicating the create action in the sidebar. Failed project listings MUST remain distinct from an empty account and MUST NOT trigger first-run onboarding.

#### Scenario: Developer opens data from the project list
- **WHEN** an authorized developer views a project with an active environment
- **THEN** its row offers a direct data-browser link with the environment named

#### Scenario: Personal projects are populated
- **WHEN** the personal-project screen has loaded projects and onboarding is inactive
- **THEN** it shows project rows, one create-project entry point, and no project-search field

#### Scenario: No projects exist
- **WHEN** every authorized owner listing succeeds and there are no reachable projects
- **THEN** the screen offers its onboarding or ordinary empty-state creation flow without a second create-project entry point

#### Scenario: Project listing fails
- **WHEN** an owner or project listing fails
- **THEN** the screen reports that failure and does not infer that the account has no projects

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
