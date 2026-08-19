# Control Plane Specification

## Purpose

Provide the multi-tenant management APIs and web console required for developers and operators to provision, configure, secure, observe, and administer Mako Cloud.

## Requirements

### Requirement: Developer account and organization management
The control plane SHALL support developer accounts, organizations, invitations, and organization memberships with owner, administrator, developer, and viewer roles. Role checks MUST apply consistently to console and management API actions.

#### Scenario: Owner invites a developer
- **WHEN** an organization owner invites an email with the developer role and the recipient accepts
- **THEN** the recipient gains only the permissions assigned to that role in the organization

#### Scenario: Viewer attempts mutation
- **WHEN** a viewer attempts to change project configuration
- **THEN** the control plane denies the operation and records an audit event

### Requirement: Project and environment lifecycle
Authorized members SHALL be able to create, inspect, suspend, restore, and delete projects and isolated environments. Provisioning SHALL report explicit asynchronous states and MUST expose data-plane endpoints only after required storage, identity, policy, and function resources are ready.

A project or environment reported as provisioning SHALL converge to an active or failed state without further caller action. The control plane MUST advance enqueued provisioning work on its own, and MUST reconcile a resource whose provisioning work has already completed but whose lifecycle state has not been updated, so that no resource remains indefinitely in a provisioning state.

#### Scenario: Project provisioning succeeds
- **WHEN** an authorized member creates a project in a supported region
- **THEN** the console reports progress and eventually supplies active auth, replication, function, and management endpoints

#### Scenario: Provisioning fails
- **WHEN** any required resource cannot be created
- **THEN** the project enters a failed state with retryable diagnostics and no partially exposed data plane

#### Scenario: Created project converges without caller action
- **WHEN** an authorized member creates a project and its environment and then only reads their state
- **THEN** both reach an active state, and subsequent environment-scoped management operations are accepted rather than rejected as not yet provisioned

### Requirement: Console and management API parity
Every MVP resource operation available in the web console SHALL have a documented management API, and the console SHALL use the same authorization and validation rules as that API.

#### Scenario: Resource is changed through API
- **WHEN** an authorized automation token updates supported project configuration
- **THEN** the console reflects the change and the audit trail identifies the API actor

### Requirement: Collection and schema administration
Authorized project members SHALL be able to create and inspect collections, publish compatible schema versions, inspect migration state, and manage index lifecycle. Destructive or incompatible schema changes MUST require an explicit migration workflow rather than silently reinterpreting stored documents.

Creating a collection SHALL make that collection resolvable for document reads and writes in the environment's owning data plane. The control plane MUST NOT report a collection as active until its metadata is durably recorded in the data plane that serves the environment. When that propagation cannot be completed, the operation MUST fail with a retryable diagnostic and MUST NOT leave a collection that management surfaces report as usable while document operations reject it as missing.

#### Scenario: Compatible schema version is published
- **WHEN** validation proves existing documents and clients remain compatible
- **THEN** the version becomes available for new replication sessions

#### Scenario: Incompatible schema is submitted directly
- **WHEN** a change would invalidate stored documents or active clients
- **THEN** publication is blocked and the console directs the member to a migration workflow

#### Scenario: Created collection accepts document traffic
- **WHEN** an authorized member creates a collection and the control plane reports it active
- **THEN** replication push and pull for that collection in the same environment resolve the collection instead of rejecting it as not found

#### Scenario: Data plane cannot record the collection
- **WHEN** the owning data plane cannot durably record the new collection's metadata
- **THEN** creation fails with a retryable diagnostic and the collection is not reported as active by any management surface

### Requirement: Policy management experience
Authorized project members SHALL be able to author, validate, test, version, activate, and roll back document policy sets. The console MUST display the default-deny state and the authorization epoch affected by activation.

Activating a policy version SHALL make that version the version enforced by the environment's owning data plane. The control plane MUST NOT report a policy version as active until it is durably recorded and active in that data plane. When that propagation cannot be completed, activation MUST fail with a retryable diagnostic and the previously enforced version MUST remain in effect.

#### Scenario: Administrator tests a policy
- **WHEN** the administrator supplies representative identity claims and document states
- **THEN** the console shows the allow or deny result and evaluation trace without changing the active policy

#### Scenario: Activated policy governs document traffic
- **WHEN** an authorized member activates a policy version for a collection and the control plane reports it active
- **THEN** document operations in that environment are evaluated against that version instead of being denied by default deny

#### Scenario: Data plane cannot record the policy
- **WHEN** the owning data plane cannot durably record and activate the policy version
- **THEN** activation fails with a retryable diagnostic and no management surface reports the version as active

### Requirement: Credential and secret management
The control plane SHALL manage public project credentials, service credentials, automation tokens, JWT signing keys, and edge-function secrets with least-privilege access, one-time secret display, rotation, revocation, and audit history.

An environment SHALL be able to obtain its first JWT signing key through the management API. Key creation MUST be distinct from rotation, which replaces an existing active key, and MUST NOT return private key material.

#### Scenario: Service credential is created
- **WHEN** an authorized administrator creates a service credential
- **THEN** its secret is shown once and subsequent views expose only identifier, scope, status, and timestamps

#### Scenario: New environment obtains its first signing key
- **WHEN** an authorized administrator initializes signing keys for an environment that has none
- **THEN** an active signing key exists, only its public lifecycle metadata is returned, and the environment can issue application-user sessions

### Requirement: Application-user management
Authorized project members SHALL be able to search and administer their project's application users, sessions, trusted metadata, and authentication settings without gaining access to passwords or raw session credentials.

#### Scenario: Support operator inspects a user
- **WHEN** a member with user-support permission opens an application-user record
- **THEN** the console shows permitted profile, status, and audit metadata but no password hash or raw token

### Requirement: Edge-function management
Authorized project members SHALL be able to create, deploy, configure, invoke for testing, inspect, roll back, and delete edge functions and their versions through the console and management API.

#### Scenario: Developer promotes a function version
- **WHEN** the developer has function-deploy permission and the version is healthy
- **THEN** the active-version change is applied atomically and audited

### Requirement: Usage, quotas, logs, and health
The control plane SHALL display project usage, quota consumption, data-plane health, replication errors, auth events, function metrics and logs, and index status within defined freshness and retention windows. Quota enforcement MUST identify the exhausted resource and whether retry is possible.

#### Scenario: Project reaches a hard quota
- **WHEN** a new operation would exceed an enforced quota
- **THEN** the operation is rejected with a stable quota code while unrelated permitted operations remain available

### Requirement: Immutable audit history
Security-sensitive control-plane and data-plane administration actions SHALL create append-only audit events containing organization, project, environment, actor, action, target, outcome, request identifier, and timestamp. Authorized users SHALL be able to filter and export events without secret values or document bodies.

#### Scenario: Policy version is activated
- **WHEN** an administrator activates a policy version
- **THEN** an audit event links the actor, old and new versions, authorization epoch, and outcome

### Requirement: Platform operator administration
Authorized platform operators SHALL have a separate admin surface for tenant lookup, service health, provisioning repair, quota overrides, abuse response, and audited support access. Operator actions MUST be least-privilege, time-bounded where impersonation is involved, and visibly distinct from tenant actions.

#### Scenario: Operator opens support access
- **WHEN** an authorized operator starts a support session for a project
- **THEN** the session has an expiry, stated reason, bounded permissions, and audit events visible to platform security staff

### Requirement: Tenant deletion lifecycle
Deleting an organization, project, or environment SHALL require explicit confirmation, enter a recoverable grace period, revoke data-plane access promptly, and delete or cryptographically render inaccessible all scoped data after the grace period according to retention policy.

#### Scenario: Project deletion is requested
- **WHEN** an authorized owner confirms project deletion
- **THEN** new application access is suspended, the grace deadline is shown, and restoration remains possible until that deadline

#### Scenario: Grace period expires
- **WHEN** no restoration occurs before the deadline
- **THEN** the platform completes scoped data and secret destruction and records completion in the operator audit trail

