## MODIFIED Requirements

### Requirement: Project and environment lifecycle
Authorized members SHALL be able to create, inspect, suspend, restore, and delete projects and isolated environments. Provisioning SHALL report explicit asynchronous states and MUST expose data-plane endpoints only after required storage, identity, policy, and function resources are ready.

A project or environment reported as provisioning SHALL converge to an active or failed state without further caller action. The control plane MUST advance enqueued provisioning work on its own, and MUST reconcile a resource whose provisioning work has already completed but whose lifecycle state has not been updated, so that no resource remains indefinitely in a provisioning state.

Authorized members SHALL also be able to rename a project and to transfer it between owners: from a personal space to a team the developer administers, from a team to the developer's personal space, or between two teams the developer administers. A transfer MUST change only the owner: identifiers, environments, data, policies, users, credentials, and functions are unchanged, quota policy is reinstalled from the new owner's plan, and the action is audited for both the previous and the new owner.

#### Scenario: Project provisioning succeeds
- **WHEN** an authorized member creates a project in a supported region
- **THEN** the console reports progress and eventually supplies active auth, replication, function, and management endpoints

#### Scenario: Provisioning fails
- **WHEN** any required resource cannot be created
- **THEN** the project enters a failed state with retryable diagnostics and no partially exposed data plane

#### Scenario: Created project converges without caller action
- **WHEN** an authorized member creates a project and its environment and then only reads their state
- **THEN** both reach an active state, and subsequent environment-scoped management operations are accepted rather than rejected as not yet provisioned

#### Scenario: A project is renamed
- **WHEN** an authorized member renames a project
- **THEN** the new name is stored, audited, and returned by every listing, and nothing else about the project changes

#### Scenario: A project is transferred between owners
- **WHEN** a developer who administers both the current owner and the target owner transfers a project and confirms
- **THEN** the project is listed under the target owner only, keeps its identifier and every environment and resource, is limited by the target owner's plan, and the transfer is audited under both owners
