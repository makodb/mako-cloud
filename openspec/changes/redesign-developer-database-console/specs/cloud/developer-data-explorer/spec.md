## ADDED Requirements

### Requirement: Document field workbench
The explorer SHALL provide searchable collection navigation and a table containing document fields alongside identifiers and revision metadata. Columns SHALL derive from collection schema and authorized returned documents, with bounded previews for large or nested values. The full JSON inspector, query planner, conditional editor, revision history, and data jobs SHALL remain available under the existing access controls.

#### Scenario: Browse documents with fields
- **WHEN** a developer with a valid grant browses documents
- **THEN** returned field values appear as escaped text in the table, absent values remain distinct from null, and the developer can open the full JSON

#### Scenario: Change collection
- **WHEN** a developer selects another collection
- **THEN** the previous grant is revoked and its documents, query results, and editor selection are cleared
