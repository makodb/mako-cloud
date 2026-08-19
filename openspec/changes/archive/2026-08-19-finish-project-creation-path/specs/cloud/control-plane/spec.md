## MODIFIED Requirements

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
