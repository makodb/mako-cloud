## MODIFIED Requirements

### Requirement: Local tenant bootstrap
A bootstrap capability SHALL create a complete, usable local tenant — a developer identity, team, project, environment, public project key, signing key, collection, and a permissive development document policy — without requiring outbound mail, authenticated TLS SMTP, or an operator wait-list decision. The bootstrap SHALL be deterministic, so that repeated runs against clean state produce the same developer, team, project, environment and collection identifiers, and idempotent, so that a repeated run against existing state neither duplicates nor corrupts records. A public project key's secret is shown only when the key is created, so a repeated run SHALL rotate the existing public project key and report the new one instead of the original. It MUST write only to stores it exclusively owns at the time it runs. When a local runtime supervisor is reachable it SHALL also deploy its sample functions through the administrative path; without one it SHALL skip that step and say so.

#### Scenario: Bootstrap produces a usable tenant
- **WHEN** the bootstrap runs against prepared local state
- **THEN** it reports the created project, environment, collection, and public project key, and those values authenticate successfully against the running data plane

#### Scenario: Bootstrap runs a second time
- **WHEN** the bootstrap runs again against state it already created
- **THEN** it completes without duplicating records, reports the same developer, team, project, environment and collection identifiers, and reports a newly rotated public project key that authenticates
