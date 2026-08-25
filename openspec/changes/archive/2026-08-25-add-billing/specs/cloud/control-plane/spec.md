## MODIFIED Requirements

### Requirement: Usage, quotas, logs, and health
The control plane SHALL display project usage, quota consumption, data-plane health, replication errors, auth events, function metrics and logs, and index status within defined freshness and retention windows. Quota enforcement MUST identify the exhausted resource and whether retry is possible.

Reported usage SHALL come from the retained billing ledger and MUST identify the period it covers. Reported quota consumption SHALL be measured against the limits that follow from the organization's effective plan and from any operator override, rather than against limits shared by every tenant.

#### Scenario: Project reaches a hard quota
- **WHEN** a new operation would exceed an enforced quota
- **THEN** the operation is rejected with a stable quota code while unrelated permitted operations remain available

#### Scenario: A member inspects usage for a period
- **WHEN** an authorized member inspects project usage
- **THEN** the reported totals come from the billing ledger and name the period they cover
