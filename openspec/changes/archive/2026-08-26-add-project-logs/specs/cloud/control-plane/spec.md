## MODIFIED Requirements

### Requirement: Usage, quotas, logs, and health
The control plane SHALL display project usage, quota consumption, data-plane health, replication errors, auth events, function metrics and logs, and index status within defined freshness and retention windows. Quota enforcement MUST identify the exhausted resource and whether retry is possible.

Reported usage SHALL come from the retained billing ledger and MUST identify the period it covers. Reported quota consumption SHALL be measured against the limits that follow from the organization's effective plan and from any operator override, rather than against limits shared by every tenant.

Function log lines SHALL be collected into the retained telemetry store and served through the project logs surface. Because log text is written by customer code, it MUST be scrubbed before storage: configured secrets, bearer and JWT values, password and cookie assignments, platform credential formats, and email addresses are masked, and the store applies the scrub at ingest so no producer can bypass it. The scrub is best-effort and MUST be documented as such.

#### Scenario: Project reaches a hard quota
- **WHEN** a new operation would exceed an enforced quota
- **THEN** the operation is rejected with a stable quota code while unrelated permitted operations remain available

#### Scenario: A member inspects usage for a period
- **WHEN** an authorized member inspects project usage
- **THEN** the reported totals come from the billing ledger and name the period they cover

#### Scenario: A function's printed line is retained
- **WHEN** a deployed function prints a line and the collector's next pass completes
- **THEN** the line is served by the project logs surface with its level, timestamp, and correlation id, and survives a runtime restart

#### Scenario: A sensitive-looking value is masked before storage
- **WHEN** a log line carrying a token, password assignment, or email address reaches the telemetry store
- **THEN** the stored line has those values masked, whichever producer sent it
