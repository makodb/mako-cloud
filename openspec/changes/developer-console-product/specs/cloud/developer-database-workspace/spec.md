## MODIFIED Requirements

### Requirement: Persistent project workspace
The developer console SHALL provide persistent, responsive navigation for project overview, Data Explorer, collections and indexes, synchronization, application users, policies, functions, logs and metrics, backups, API & Connect, and settings. The active team, project, environment, and developer identity MUST remain visible, and navigation MUST show only resources allowed by the current membership.

The same navigation SHALL exist at the home and project levels, so that a developer reaches any product area from anywhere in at most two selections. Destinations the deployment does not provide MUST be shown as unavailable rather than omitted, so the product's shape is visible before every area is enabled.

#### Scenario: Developer changes environments
- **WHEN** an authorized developer selects another environment in the project switcher
- **THEN** the workspace updates every destination to that environment, clears incompatible cursors or drafts, and does not retain data from the previous environment on screen

#### Scenario: Developer reaches a destination from home
- **WHEN** an authorized developer is on the home dashboard
- **THEN** any product area of any reachable environment is at most two selections away

### Requirement: Database project overview
The project overview SHALL summarize lifecycle and provisioning state, endpoint readiness, collection and user counts, current usage and quota position, RxDB sync health, recent safe errors, active function deployment, backup freshness, and recent audit activity. Each summary MUST identify its observation time and distinguish unavailable or stale data from healthy data.

The overview SHALL also present the selected environment's API URL and public key with a quickstart, and at the project level SHALL aggregate across environments so a project with several environments is understood before one is chosen.

#### Scenario: Developer opens a healthy project
- **WHEN** all required environment services are ready and current summaries are available
- **THEN** the overview shows connection readiness, current usage, sync and backup status, recent activity, and links to relevant workspace destinations

#### Scenario: One summary source is unavailable
- **WHEN** a summary provider fails or becomes stale
- **THEN** the overview marks only that summary unavailable or stale and does not represent missing data as healthy

#### Scenario: Developer opens a project with several environments
- **WHEN** an authorized developer opens a project that has more than one environment
- **THEN** the project overview shows each environment's state and readiness and the selected environment's keys and quickstart

### Requirement: Workspace context and accessibility
Workspace URLs SHALL preserve safe team, project, environment, destination, filter, and time context without secrets, tokens, raw email addresses, or document content. Navigation, summaries, setup instructions, diagnostic tables, and recovery progress SHALL be keyboard accessible and usable with assistive technology.

Home and project-level URLs SHALL follow the same rules, and the navigation shell SHALL be operable by keyboard with the current destination announced to assistive technology.

#### Scenario: Developer opens a shared diagnostic URL
- **WHEN** an authorized colleague opens a workspace URL containing safe project, environment, sync filter, and time context
- **THEN** the system reauthorizes that colleague, reconstructs the permitted view, and omits any context they are not allowed to access

#### Scenario: Developer navigates the shell by keyboard
- **WHEN** a developer moves through the navigation shell using only the keyboard
- **THEN** every destination is reachable and the current destination is announced
