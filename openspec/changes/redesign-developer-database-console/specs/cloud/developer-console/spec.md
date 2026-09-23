## ADDED Requirements

### Requirement: Direct database entry from searchable projects
The home dashboard SHALL support project search by name, identifier, region, and owner, and SHALL offer direct database, schema, and connection destinations for an active environment. The create-project action and documentation MUST be visible without expanding an unrelated settings screen. Empty search results MUST be distinct from an empty account and from failed listings.

#### Scenario: Developer opens data from the project list
- **WHEN** an authorized developer searches for a project with an active environment
- **THEN** the matching project offers a direct data-browser link with its environment named

#### Scenario: Search has no matches
- **WHEN** the developer's search matches no loaded project
- **THEN** the dashboard offers clearing the search and does not show first-run onboarding
