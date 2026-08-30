## MODIFIED Requirements

### Requirement: Indexed document access
Trusted server components SHALL be able to get a document by primary key and query documents through declared single-field or compound indexes using equality, bounded range, deterministic sort, cursor pagination, and a result limit. Queries that cannot be satisfied safely by an active index MUST be rejected rather than silently performing an unbounded scan. A one-sided range over an index's leading field, with no equality predicate before it, SHALL be served: it is how a caller walks a collection when there is no query for "everything", and refusing it would leave no way to do so at all. A rejection SHALL name which rule the query broke, not merely that it was invalid.

#### Scenario: Indexed query is executed
- **WHEN** trusted server code submits a supported predicate and sort matching an active index
- **THEN** the engine returns a deterministic page and an opaque cursor for the next page

#### Scenario: Query lacks an eligible index
- **WHEN** a query would require an unbounded collection scan
- **THEN** the engine rejects it with an error identifying the required index shape

#### Scenario: A range with no equality walks the collection
- **WHEN** trusted server code ranges over the leading field of a single-field index with no equality predicate
- **THEN** every document of the collection is returned across pages, rather than the query being refused for having no leading component
