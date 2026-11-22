## ADDED Requirements

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
