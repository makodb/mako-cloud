## Purpose

What every deployable service must expose so that operations can see it, and what
the alert inventory may claim to watch. A metric that nothing publishes is worse
than no metric at all: the rule that reads it looks like coverage, reports nothing,
and hides the failure it was written to catch.

## ADDED Requirements

### Requirement: Every deployable service exposes a bounded metrics endpoint

Each deployable service SHALL serve a metrics endpoint describing its own health
and the work it performs. The endpoint SHALL be reachable only from the
deployment's own collection path and never from the public route. It SHALL emit a
bounded set of series whose label values come from a closed domain, and SHALL
never carry an actor, request, trace, session, document, raw URL, email, token,
tenant key, or secret as a label or value. A service that cannot determine a value
SHALL omit that series rather than publish a placeholder that reads as healthy.

#### Scenario: Operations reads a service it deploys

- **WHEN** the deployment's collector scrapes a running service's metrics endpoint
- **THEN** the response describes that service's readiness, its stored data's health, and the outcomes of the work it performs, using only bounded label values

#### Scenario: The metrics endpoint is requested from the public route

- **WHEN** the metrics endpoint is requested over the public route rather than the deployment's own collection path
- **THEN** the request is refused, and no series is disclosed

#### Scenario: A value is unavailable

- **WHEN** a service cannot determine the value of a series it normally publishes
- **THEN** it omits that series, rather than publishing a zero or a default that an alert would read as healthy

### Requirement: An alert may only watch a series something publishes

A published alert rule SHALL depend only on series that a service or a deployment
collector actually produces. An expression over an absent series is not an error in
the alerting system: a threshold comparison never becomes true and the rule is
permanently silent, while an absence check removes nothing and the rule fires
forever. Both read as coverage and neither is. The rule inventory SHALL therefore be
checked against the set of series the workspace and the deployment publish, and a
rule watching a series with no producer SHALL fail that check unless it is recorded,
with a reason, as a coverage gap being tracked.

An alert SHALL NOT be deleted, retargeted at a weaker series, or have its threshold
relaxed in order to satisfy this check. The rules describe the failures worth
catching; a missing producer is the defect.

#### Scenario: A rule is written over a series nothing publishes

- **WHEN** the alert inventory contains a rule whose expression names a series no service or collector produces, and the rule is not recorded as a tracked gap
- **THEN** the check fails, naming the rule, its severity, and the absent series

#### Scenario: A tracked gap is closed

- **WHEN** every series a recorded gap's rule depends on becomes published
- **THEN** the check fails until that rule is removed from the recorded gaps, so the list of known-blind rules can only shrink

#### Scenario: A recorded gap names a rule that no longer exists

- **WHEN** the recorded gaps name a rule absent from the alert inventory
- **THEN** the check fails, so a renamed or deleted rule cannot leave a stale exemption behind

### Requirement: Absent telemetry is itself observable

The deployment SHALL be able to distinguish a service reporting health from a
service reporting nothing. A collection target that stops answering, or that answers
without the series operations depends on, SHALL be detectable rather than
indistinguishable from a healthy quiet system.

#### Scenario: A service stops answering the collector

- **WHEN** a deployed service's metrics endpoint stops responding to the collector
- **THEN** operators are alerted that the service is unobserved, separately from any alert about the service's own health

