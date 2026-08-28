## MODIFIED Requirements

### Requirement: Developer account and team management
The control plane SHALL support developer accounts, teams, invitations, and team memberships with owner, administrator, developer, and viewer roles. Role checks MUST apply consistently to console and management API actions.

Every developer SHALL also have a personal space: an implicit team of exactly one member, created on first use with a deterministic identity, that holds the developer's individual projects. A personal space MUST refuse invitations, role changes, member removal, and deletion, and MUST otherwise be a team for billing, limits, audit, and operator purposes.

#### Scenario: Owner invites a developer
- **WHEN** a team owner invites an email with the developer role and the recipient accepts
- **THEN** the recipient gains only the permissions assigned to that role in the team

#### Scenario: Viewer attempts mutation
- **WHEN** a viewer attempts to change project configuration
- **THEN** the control plane denies the operation and records an audit event

#### Scenario: A developer creates an individual project
- **WHEN** a developer creates a project without naming a team
- **THEN** the project is created in the developer's personal space, which is created on first use and reused thereafter, and the space appears in the developer's team list marked personal

#### Scenario: A personal space refuses members and deletion
- **WHEN** anyone invites a developer to a personal space, changes its membership, or requests its deletion
- **THEN** the request is refused with a conflict that names the personal space as the reason
