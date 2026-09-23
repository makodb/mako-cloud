## ADDED Requirements

### Requirement: Task-oriented database overview
The environment overview SHALL display named database metrics, a collection inventory, database setup actions, and bounded recent audit metadata. Every metric MUST retain provider availability, freshness, observation window, and pagination limits where relevant. Missing data MUST NOT appear as zero or healthy. The management summary MUST return only metadata within the caller's authorized environment.

#### Scenario: Populated environment
- **WHEN** an authorized developer opens an environment with collections and activity
- **THEN** the overview shows collection names and schema versions, explicit metric labels, and recent safe activity with links to the relevant tools

#### Scenario: Provider is stale or fails
- **WHEN** a provider fails or its freshness deadline passes
- **THEN** its summary displays unavailable or stale while other loaded sections remain usable

### Requirement: Grouped navigation and isolated environment state
The console SHALL group database, application, operations, and configuration destinations, retaining the current destination and environment context. Switching project or environment MUST clear previous document data, grants, query cursors, and editor drafts before displaying the destination.

#### Scenario: Environment changes during a request
- **WHEN** a developer switches environment while an old read is pending
- **THEN** the old response cannot populate the new environment's screen
