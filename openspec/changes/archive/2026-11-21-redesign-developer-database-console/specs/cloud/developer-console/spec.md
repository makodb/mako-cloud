## MODIFIED Requirements

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

## ADDED Requirements

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
