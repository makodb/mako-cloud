## MODIFIED Requirements

### Requirement: Grouped navigation and isolated environment state
The console SHALL group environment destinations under Database, Application, Observe, and Configure, retaining the current destination and environment context. Switching project or environment MUST clear previous document data, grants, query cursors, and editor drafts before displaying the destination.

#### Scenario: Environment changes during a request
- **WHEN** a developer switches environment while an old read is pending
- **THEN** the old response cannot populate the new environment's screen
