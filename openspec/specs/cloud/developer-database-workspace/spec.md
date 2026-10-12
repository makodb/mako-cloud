# Developer Database Workspace Specification

## Purpose

Provide developers with a coherent project and environment workspace for understanding database readiness, connecting RxDB applications, diagnosing synchronization, and initiating tenant-safe recovery.

## Requirements

### Requirement: Persistent project workspace
The developer console SHALL provide persistent, responsive navigation for project overview, Data Explorer, collections and indexes, synchronization, application users, policies, functions, logs and metrics, backups, API & Connect, and settings. The active personal or team ownership, project, environment, and developer identity MUST remain visible, and navigation MUST show only resources allowed by the current membership.

The same navigation SHALL exist at the home and project levels, so that a developer reaches any product area from anywhere in at most two selections. Destinations the deployment does not provide MUST be shown as unavailable rather than omitted, so the product's shape is visible before every area is enabled.

#### Scenario: Developer changes environments
- **WHEN** an authorized developer selects another environment in the project switcher
- **THEN** the workspace updates every destination to that environment, clears incompatible cursors or drafts, and does not retain data from the previous environment on screen

#### Scenario: Developer reaches a destination from home
- **WHEN** an authorized developer is on the home dashboard
- **THEN** any product area of any reachable environment is at most two selections away

### Requirement: Database project overview
The project overview SHALL summarize lifecycle and provisioning state, endpoint readiness, collection and user counts, current usage and quota position, RxDB sync health, recent safe errors, active function deployment, backup freshness, and recent audit activity. Each summary MUST show its observation age as plain relative text, such as "Updated 1 minute ago", and distinguish unavailable data from healthy data. Developer dashboards MUST NOT label successful observations "Current" or "Stale".

The overview SHALL also present the selected environment's API URL and public key with a quickstart, and at the project level SHALL aggregate across environments so a project with several environments is understood before one is chosen.

#### Scenario: Developer opens a healthy project
- **WHEN** all required environment services are ready and current summaries are available
- **THEN** the overview shows connection readiness, current usage, sync and backup status, recent activity, and links to relevant workspace destinations

#### Scenario: One summary source is unavailable
- **WHEN** a summary provider fails
- **THEN** the overview marks only that summary unavailable, or keeps its last successful value with an explicit refresh error and its original observation time, and does not represent missing data as healthy

#### Scenario: Developer opens a project with several environments
- **WHEN** an authorized developer opens a project that has more than one environment
- **THEN** the project overview shows each environment's state and readiness and the selected environment's keys and quickstart

### Requirement: API and RxDB connection guidance
The workspace SHALL show the selected environment's public API endpoint, public project key, collection identifier, active schema version, supported RxDB client package and version range, and generated setup examples matching the published client API. The page MUST NOT reveal service keys, signing material, function secrets, or other privileged credentials.

#### Scenario: Developer copies an RxDB setup example
- **WHEN** a developer selects a collection and supported client version
- **THEN** the workspace generates a copyable example containing the correct endpoint, public key, collection, schema version, authentication placeholder, and required replication configuration

#### Scenario: No public key is active
- **WHEN** the selected environment has no usable public client key
- **THEN** the page reports setup as incomplete and links an authorized member to create or activate a public key without substituting a service credential

### Requirement: Safe connection check
An authorized developer SHALL be able to run a bounded connection check that verifies DNS/TLS reachability, public endpoint routing, environment readiness, public-key acceptance, schema compatibility, and replication route availability without authenticating as an application user or reading documents.

#### Scenario: Connection check finds a schema mismatch
- **WHEN** the selected client schema version is incompatible with the active collection schema
- **THEN** the check reports the required version and remediation guidance without starting replication or returning collection data

### Requirement: Developer RxDB synchronization dashboard
The workspace SHALL provide environment and collection-scoped synchronization summaries for pull and push activity, live connections, lag, conflicts, policy denials, throttling, checkpoint expiry, stream gaps, forced resynchronization, and schema incompatibility. The dashboard MUST use bounded metadata and MUST NOT expose other tenants, document bodies, secrets, or raw application-user tokens.

#### Scenario: Developer investigates forced resynchronization
- **WHEN** a collection has recent checkpoint-expired or authorization-epoch resets
- **THEN** the dashboard shows bounded counts, time windows, safe reason classes, affected client-version classes, and remediation guidance

### Requirement: Client compatibility guidance
The workspace SHALL compare observed supported client-version classes and selected collection schema versions with the currently published compatibility policy and SHALL identify upgrade or migration actions. It MUST distinguish retryable service conditions from non-retryable client, schema, policy, or credential configuration errors.

#### Scenario: Unsupported client version is observed
- **WHEN** synchronization telemetry reports a client outside the supported version range
- **THEN** the dashboard identifies the supported range and links to the matching upgrade guidance without exposing a raw client identifier

### Requirement: Tenant-scoped backup inventory
Authorized project members SHALL be able to view backups and restore drills that protect their selected project environment, including creation time, recovery point, verification state, retention expiry, and safe recovery-objective status. They MUST NOT see physical storage paths, backup credentials, signing material, unrelated tenant inventory, or infrastructure-wide capacity.

#### Scenario: Developer views verified backups
- **WHEN** an authorized developer opens backups for an environment
- **THEN** the workspace lists only backups whose protected inventory includes that project environment and shows their verification and retention state

### Requirement: Guarded developer restore request
An authorized project owner or administrator SHALL be able to request restoration of a verified recovery point into a new isolated recovery environment within the same project. The request MUST show impact and quota requirements, require recent step-up authentication and confirmation, create an auditable asynchronous job, and prevent use of the restored environment until platform verification succeeds. In-place overwrite or production promotion MUST remain an operator-controlled workflow.

#### Scenario: Developer requests a recovery environment
- **WHEN** a stepped-up authorized member selects a verified backup and confirms a new recovery-environment name and impact
- **THEN** the system creates a tenant-scoped restore request, reports progress, and exposes the environment only after isolation and verification succeed

#### Scenario: Developer requests in-place restore
- **WHEN** a developer attempts to target an existing environment or promote restored storage directly
- **THEN** the system rejects the request and directs the developer to the separately authorized operator recovery process

### Requirement: Workspace context and accessibility
Workspace URLs SHALL preserve safe team, project, environment, destination, filter, and time context without secrets, tokens, raw email addresses, or document content. Navigation, summaries, setup instructions, diagnostic tables, and recovery progress SHALL be keyboard accessible and usable with assistive technology.

Home and project-level URLs SHALL follow the same rules, and the navigation shell SHALL be operable by keyboard with the current destination announced to assistive technology.

#### Scenario: Developer opens a shared diagnostic URL
- **WHEN** an authorized colleague opens a workspace URL containing safe project, environment, sync filter, and time context
- **THEN** the system reauthorizes that colleague, reconstructs the permitted view, and omits any context they are not allowed to access

#### Scenario: Developer navigates the shell by keyboard
- **WHEN** a developer moves through the navigation shell using only the keyboard
- **THEN** every destination is reachable and the current destination is announced

### Requirement: Task-oriented database overview
The environment overview SHALL display named database metrics, a collection inventory, database setup actions, and bounded recent audit metadata. Every metric MUST retain provider availability, freshness, observation window, and pagination limits where relevant. Missing data MUST NOT appear as zero or healthy. The management summary MUST return only metadata within the caller's authorized environment.

#### Scenario: Populated environment
- **WHEN** an authorized developer opens an environment with collections and activity
- **THEN** the overview shows collection names and schema versions, explicit metric labels, and recent safe activity with links to the relevant tools

#### Scenario: An observation ages or its provider fails
- **WHEN** an observation ages or a refresh fails
- **THEN** the summary shows its actual age without a "Current" or "Stale" badge, retains the last successful observation time on transient failure, and shows an explicit refresh error while other loaded sections remain usable
- **AND** a summary without usable data remains unavailable, and an authorization or missing-resource response clears the retained value

#### Scenario: Dashboard refreshes once per minute
- **WHEN** a developer leaves the environment overview, project summaries, or usage metrics open
- **THEN** each source is fetched on entry and every 60 seconds, with no overlapping request for the same source
- **AND** a background refresh preserves loaded values until a new response arrives, manual refresh remains available on the overview, and leaving the view stops its polling

#### Scenario: Environment changes during a background refresh
- **WHEN** a refresh for the previous environment completes after navigation
- **THEN** it cannot replace the new environment's values or observation times

### Requirement: Grouped navigation and isolated environment state
The console SHALL group database, application, operations, and configuration destinations, retaining the current destination and environment context. Switching project or environment MUST clear previous document data, grants, query cursors, and editor drafts before displaying the destination.

#### Scenario: Environment changes during a request
- **WHEN** a developer switches environment while an old read is pending
- **THEN** the old response cannot populate the new environment's screen
