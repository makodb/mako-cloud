# Metering Specification

## Purpose

Measure the resources a tenant is billed for, carry those measurements from the planes that observe them into the retained billing ledger every charge is derived from, and keep that ledger honest against what enforcement counted.

## Requirements

### Requirement: Billable use is counted once
The platform SHALL record billable use in the plane that serves the work and transfer it to the billing ledger over an authenticated, ordered stream. Each transferred batch MUST carry its source and offset, and the ledger MUST apply a batch at most once. Transfer MUST NOT fail or delay the customer request that produced the use: buffering is bounded and non-blocking, and use shed under pressure MUST be counted and detectable rather than silently lost.

#### Scenario: A batch is delivered twice
- **WHEN** a usage batch is delivered again after an ambiguous failure
- **THEN** the ledger totals are unchanged and the batch is reported as already applied

#### Scenario: The ledger store is unavailable
- **WHEN** usage cannot be transferred because the ledger store is unreachable
- **THEN** the requests that produced the use still succeed and buffered use transfers once the store returns

#### Scenario: Use is lost under pressure
- **WHEN** the bounded buffer sheds records or a plane crashes with use still buffered
- **THEN** the loss is counted, and a material divergence between the ledger and enforcement's counters raises an alert instead of passing unnoticed

### Requirement: Storage at rest is measured and its approximation is stated
The platform SHALL sample stored bytes per environment, marked due by the writes that can change them and measured off the request path. A storage charge MUST be derived by averaging the period's retained samples, and customer-facing documentation of that charge MUST state that it is sampled rather than continuously integrated.

#### Scenario: Storage is billed for a period
- **WHEN** a period is rated
- **THEN** the storage quantity is the average of that period's retained samples, so repeated samples of the same stored bytes do not multiply the charge

#### Scenario: An idle tenant is not resampled
- **WHEN** a tenant has not written since it was last measured
- **THEN** its last sample stands and no walk re-measures it

### Requirement: The ledger is the reported source of usage
Usage reported through management surfaces SHALL come from the retained billing ledger. The platform MUST NOT report usage from a store that is not the authority for billing, and a usage report MUST identify the period it covers.

#### Scenario: A developer reads their usage
- **WHEN** an authorized member requests project usage
- **THEN** the response reports the ledger's totals for the identified period

### Requirement: Metering and enforcement are cross-checked
The platform SHALL compare ledger totals against the quota engine's counters for the same resources and windows, and MUST raise an operator-visible alert when they diverge materially. Neither source may be silently corrected from the other.

#### Scenario: The two disagree
- **WHEN** ledger totals and quota counters diverge materially for a window
- **THEN** an alert is raised identifying the tenant, resource, and window, and neither source is overwritten
