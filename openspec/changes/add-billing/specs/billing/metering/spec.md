## Purpose

Measure the resources a tenant is billed for, carry those measurements from the planes that observe them into the control database, and retain the per-period ledger every charge is derived from.

## ADDED Requirements

### Requirement: Billable use is counted once
The platform SHALL record billable use in the plane that serves the work and transfer it to the control-plane billing ledger. Each transferred batch MUST carry an idempotency key derived from its content and range, and the ledger MUST apply a batch at most once. Recorded use MUST survive restart of either plane, and a failure to transfer MUST NOT fail the customer request that produced the use.

#### Scenario: A batch is delivered twice
- **WHEN** a usage batch is delivered again after an ambiguous failure
- **THEN** the ledger totals are unchanged and the batch is reported as already applied

#### Scenario: The control plane is unavailable
- **WHEN** usage cannot be transferred because the control plane is unreachable
- **THEN** the requests that produced the use still succeed, the use is retained locally, and it transfers once the control plane returns

#### Scenario: A serving plane restarts mid-period
- **WHEN** a plane restarts with untransferred use recorded
- **THEN** that use is transferred after restart and is neither lost nor counted twice

### Requirement: Storage at rest is measured and its approximation is stated
The platform SHALL sample stored bytes per environment on a fixed schedule and retain the sample series. A storage charge MUST be derived from the retained samples, and any customer-facing description of that charge MUST state that it is sampled rather than continuously integrated.

#### Scenario: Storage is billed for a period
- **WHEN** a period closes
- **THEN** the storage quantity is derived from that period's retained samples and the samples remain available as the evidence for the line item

#### Scenario: Sampling is interrupted
- **WHEN** samples are missing for part of a period
- **THEN** the gap is recorded with the quantity rather than silently interpolated, and the period is flagged for operator review

### Requirement: The ledger is the reported source of usage
Usage reported through management surfaces SHALL come from the retained billing ledger. The platform MUST NOT report usage from a telemetry store that is not the authority for billing, and a usage report MUST identify the period it covers.

#### Scenario: A developer reads their usage
- **WHEN** an authorized member requests project usage
- **THEN** the response reports the ledger's totals for the identified period

### Requirement: Metering and enforcement are cross-checked
The platform SHALL compare ledger totals against the quota engine's counters for the same resources and period, and MUST raise an operator alert when they diverge materially. Neither source may be silently corrected from the other.

#### Scenario: The two disagree
- **WHEN** ledger totals and quota counters diverge beyond the configured tolerance
- **THEN** an operator alert is raised identifying the tenant, resource, and period, and neither source is overwritten
