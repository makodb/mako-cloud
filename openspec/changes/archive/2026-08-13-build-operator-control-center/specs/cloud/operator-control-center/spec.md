## Purpose

Provide platform operators with a fleet-wide, metadata-first control center for diagnosing Mako Cloud, understanding tenant impact, and executing narrowly authorized operational workflows with complete audit context.

## ADDED Requirements

### Requirement: Permission-aware operator workspace
The system SHALL provide an authenticated operator workspace with persistent navigation for overview, tenants, operations, alerts and incidents, backups, fleet, security, and activity. The workspace MUST expose only pages, fields, and actions allowed by the operator's current entitlements and MUST keep operator identity and session expiry visible.

#### Scenario: Operator opens the workspace
- **WHEN** an authenticated operator opens the operator console
- **THEN** the system shows the navigation destinations allowed by that operator's entitlements and identifies the active operator session

#### Scenario: Operator lacks a page entitlement
- **WHEN** an operator requests a page or API resource outside their entitlements
- **THEN** the system denies the request, does not disclose the protected resource, and records the denied attempt

### Requirement: Fleet overview with freshness
The system SHALL provide a global overview containing bounded summaries of tenant and environment lifecycle, service readiness, request health, active alerts, RxDB synchronization, RocksDB capacity and recovery signals, backup freshness, registration and mail delivery, and deployed release state. Every summary MUST identify its observation time and MUST distinguish healthy, degraded, unavailable, stale, and unknown data.

#### Scenario: Operator reviews current platform state
- **WHEN** an entitled operator opens the overview and all providers are available
- **THEN** the system presents current global summaries, active exceptions, observation times, and drill-down links without customer document content

#### Scenario: One overview provider is unavailable
- **WHEN** a telemetry or operational provider cannot supply one overview section
- **THEN** the system marks only that section unavailable or stale, preserves the remaining overview, and does not represent missing data as healthy

### Requirement: Searchable tenant directory
The system SHALL provide a cursor-paginated tenant directory that can be searched by bounded organization, project, environment, developer-email, and identifier criteria and filtered by lifecycle, health, region, and plan or quota class. Results MUST contain only operator-safe summary fields and MUST use stable ordering for pagination.

#### Scenario: Operator searches for a tenant
- **WHEN** an entitled operator submits a valid tenant search
- **THEN** the system returns a bounded page of matching tenant summaries, an opaque continuation cursor when more results exist, and the effective filters

#### Scenario: Search criteria are too broad or invalid
- **WHEN** an operator submits unsupported, unbounded, or malformed search criteria
- **THEN** the system rejects the search with a safe validation error and does not start an unrestricted tenant scan

### Requirement: Operator-safe Tenant 360 view
The system SHALL provide a Tenant 360 view containing project and environment topology, lifecycle and provisioning state, usage and quotas, RxDB sync health, application-auth health, edge-function status, backup status, recent safe error summaries, operator activity, and support-access history. Each section MUST retain project and environment scope and MUST report partial failure independently.

#### Scenario: Operator opens a tenant
- **WHEN** an entitled operator selects a tenant-directory result
- **THEN** the system displays the tenant's scoped operational sections, freshness, current exceptions, and relevant contextual workflows

#### Scenario: A tenant section cannot be loaded
- **WHEN** one Tenant 360 data source is unavailable
- **THEN** the system labels that section unavailable without hiding already loaded sections or substituting cross-tenant data

### Requirement: Safe observability drill-down
The system SHALL expose bounded operational summaries and allow entitled operators to drill into approved dashboards, logs, traces, and runbooks with the selected tenant, environment, service, region, and time window carried as safe filters. Links MUST come from an allowlisted destination set and MUST NOT place tokens, secrets, raw email addresses, document identifiers, or document content in URLs.

#### Scenario: Operator opens a detailed dashboard
- **WHEN** an operator follows an observability drill-down from a tenant or alert
- **THEN** the approved destination opens with bounded scope and time filters that preserve the originating operational context

### Requirement: Alert and incident operations
The system SHALL provide an active alert inventory with severity, state, first and last observation time, affected services and tenants, and runbook references. Entitled operators SHALL be able to acknowledge, assign, annotate, and resolve incident records while preserving an ordered incident timeline.

#### Scenario: Operator acknowledges an alert
- **WHEN** an entitled operator acknowledges an active alert with a case-quality annotation
- **THEN** the system records the actor, alert fingerprint, affected scope, annotation, timestamp, and resulting incident state

#### Scenario: Alert input becomes stale
- **WHEN** the alert provider has not refreshed within its configured freshness window
- **THEN** the console marks the alert inventory stale and does not silently resolve previously active alerts

### Requirement: Provisioning work queue
The system SHALL list failed, stalled, and pending provisioning workflows with tenant scope, current step, safe failure class, attempt history, and available authorized recovery actions. A repair action MUST revalidate the current workflow state and MUST be idempotent for a supplied operation key.

#### Scenario: Operator repairs a failed workflow
- **WHEN** an entitled operator chooses an allowed repair action for the current provisioning state and confirms its impact
- **THEN** the system records the request, performs or schedules the action once, and displays the resulting workflow state and audit reference

#### Scenario: Workflow changed before repair
- **WHEN** the selected workflow state no longer matches the state reviewed by the operator
- **THEN** the system rejects the stale mutation and requires the operator to review the current state before trying again

### Requirement: Contextual quota, abuse, and support workflows
The system SHALL list active and historical quota overrides, abuse responses, and support sessions within the selected tenant context. Authorized operators SHALL be able to create, replace, expire, or revoke the applicable records without manually re-entering tenant identifiers, and the console MUST show the current state and history before mutation.

#### Scenario: Operator revokes a temporary grant
- **WHEN** an entitled operator revokes an active quota override or support session
- **THEN** the system applies the revocation to the selected tenant, reports the effective time, and retains the previous record in history

#### Scenario: Operator attempts a cross-context mutation
- **WHEN** a mutation payload names a tenant or environment different from the current contextual resource
- **THEN** the system rejects the request and records the denied cross-scope attempt

### Requirement: Guarded privileged mutations
Every state-changing operator workflow SHALL require explicit confirmation, a reason and case reference appropriate to the operation, current-state validation, least-privilege authorization, an idempotency key, and an immutable allowed or denied audit event. Temporary access or overrides MUST have a bounded expiry. Operations classified as destructive, customer-data access, recovery, entitlement administration, or public-admission changes MUST require fresh step-up authorization.

#### Scenario: Step-up is required
- **WHEN** an operator confirms a high-impact action without sufficiently recent step-up authorization
- **THEN** the system does not execute the action and directs the operator to complete step-up authentication without losing the reviewed context

#### Scenario: Repeated operation key
- **WHEN** a client retries an operator mutation with the same operation key and identical input
- **THEN** the system returns the original outcome without performing the mutation twice

### Requirement: Backup and recovery control
The system SHALL list backup inventory, age, integrity or remote-verification evidence, restore-drill history, recovery-objective status, and in-progress recovery jobs by protected service and volume. Recovery actions MUST require a verified backup, a selected target, an impact preview, step-up authorization, progress reporting, and post-restore verification before promotion.

#### Scenario: Operator starts a restore workflow
- **WHEN** an entitled and stepped-up operator selects a verified backup and confirms the target and impact
- **THEN** the system creates an auditable recovery job, exposes its progress, and prevents promotion until verification succeeds

#### Scenario: Selected backup is not verified
- **WHEN** an operator attempts to restore from a backup without valid integrity evidence
- **THEN** the system refuses to start the restore and identifies the missing verification requirement

### Requirement: RxDB synchronization operations
The system SHALL report global and tenant-scoped RxDB synchronization health including connected live streams, pull and push outcomes, replication lag, conflicts, policy denials, checkpoint expiration, and forced resynchronization. The view MUST use bounded metadata and MUST NOT expose document bodies, selectors, or unbounded client identifiers.

#### Scenario: Operator investigates sync degradation
- **WHEN** an operator drills from a replication alert into an affected tenant
- **THEN** the system shows the relevant environment and collection-level safe aggregates, time window, and correlated error classes without document content

### Requirement: Fleet and RocksDB operations
The system SHALL provide fleet inventory for services, instances, regions, deployed versions, readiness, restarts, dependency status, certificate expiry, and configuration drift, together with RocksDB volume readiness, capacity, write-stall, compaction, corruption, backup, and recovery signals. The system MUST identify unsupported or unknown state rather than inferring health.

#### Scenario: Operator investigates storage pressure
- **WHEN** a RocksDB volume approaches its configured capacity or write-stop threshold
- **THEN** the console identifies the affected service and volume, current bounded measurements, related alerts, and the approved runbook

### Requirement: Operator security administration
The system SHALL allow specifically entitled operators to inspect operator identities, role-derived entitlements, active and recent sessions, authentication failures, throttling, and support grants. Entitlement changes and session revocation MUST require step-up authorization, MUST prevent removal of the last recoverable administrator, and MUST take effect within a documented bounded interval.

#### Scenario: Security operator revokes a session
- **WHEN** an entitled and stepped-up security operator revokes an active operator session
- **THEN** subsequent use of that session is denied within the revocation bound and the revocation is added to the activity history

### Requirement: Searchable immutable operator activity
The system SHALL provide cursor-paginated operator activity searchable by time, actor, action, target, tenant scope, case reference, request identifier, and outcome. Authorized export MUST preserve the selected filters and integrity metadata while excluding secrets and document bodies.

#### Scenario: Operator investigates a tenant change
- **WHEN** an entitled operator filters activity for a tenant and time window
- **THEN** the system returns an ordered, bounded timeline linking the actor, action, target, reason or case, outcome, and request correlation data

### Requirement: Customer-data access boundary
Routine overview, tenant, observability, incident, fleet, backup, security, and activity views MUST NOT reveal customer document bodies or secrets. Document access SHALL require a separately created support session with the exact project and environment scope, explicit document-read permission, case and reason, short expiry, visible support-mode indication, and enhanced access audit.

#### Scenario: Ordinary operator attempts to view a document
- **WHEN** an operator without a valid matching document-read support session requests customer document content
- **THEN** the system denies the request without revealing whether the document exists and records the attempt

#### Scenario: Authorized support access is active
- **WHEN** an operator uses a valid matching support session to access permitted customer data
- **THEN** the console displays a persistent support-mode banner and the system records the session, operator, scope, target, reason, and outcome

### Requirement: Accessible and resilient presentation
The operator workspace SHALL be usable with keyboard navigation and assistive technology, SHALL adapt to supported viewport widths, and SHALL preserve selected tenant, filters, and time context in navigable URLs without placing sensitive values in them. Loading, empty, degraded, stale, unauthorized, and error states MUST be visually and programmatically distinct.

#### Scenario: Operator navigates with a keyboard
- **WHEN** an operator uses only keyboard controls to move between navigation, filters, tables, detail sections, and confirmations
- **THEN** focus order, visible focus, labels, status announcements, and escape paths allow the workflow to be completed without pointer input
