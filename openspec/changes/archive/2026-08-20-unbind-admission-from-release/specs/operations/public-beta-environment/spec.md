## MODIFIED Requirements

### Requirement: Public admission follows the single-region beta gate
Qualified public beta traffic SHALL be admitted only after the exact VM,
release digest, configuration, persistent storage, runtime, and public HTTPS
route satisfy the single-region beta thresholds for durability, recovery,
security, latency, capacity, cost, and operator drills. Pre-gate qualification
access MUST be limited to authorized testers unless an authorized operator
activates the distinct risk-accepted public-preview mode. Neither preview mode,
a reachable VM, nor a valid certificate changes the release decision or permits
the deployment to be represented as a passing or qualified beta release.

Risk-accepted public preview MUST retain publicly trusted HTTPS and verified
HSTS, the exact public route allowlist, application authentication,
document-policy enforcement, quotas, rate limits, audit, verified backup and
recovery, zero acknowledged-write loss and integrity failures for the exact
release, and a tested emergency admission stop. Those safeguards are
non-waivable. An operator MAY accept named latency, alert-delivery, cost, and
incomplete observation-window blockers only through a retained approval record
that identifies the operator, exact plan hash, active blockers, acceptance
time, and the release digest those safeguards were measured on. That record
persists until an operator pauses it rather than expiring on a calendar.

The preview approval MUST fail closed to source-restricted pre-gate admission
when its plan binding no longer matches, the recorded blocker set changes, or
any non-waivable safeguard fails. Deploying a different release MUST NOT by
itself close admission, and the deployed release MUST NOT be compared against
the approval to decide admission. Re-enabling preview
requires a new approval record. Preview changes only the ingress source
allowlist; it MUST NOT bypass authentication, authorization, policy, quota,
rate-limit, audit, route, or private-listener controls.

#### Scenario: Qualification evidence is incomplete
- **WHEN** any required beta observation is missing, exceeds its threshold, or retains a blocker and no valid risk-accepted preview approval exists
- **THEN** unrestricted public admission remains disabled while authorized qualification traffic may continue

#### Scenario: Operator accepts bounded public-preview risk
- **WHEN** every non-waivable safeguard passes and an authorized operator records an approval bound to the exact plan and current blocker set
- **THEN** the deployment admits rate-limited public-preview traffic without changing the blocked beta-gate result or describing the release as qualified beta

#### Scenario: Public-preview approval becomes stale or unsafe
- **WHEN** the preview approval's plan binding changes, the active blocker set differs, or a non-waivable safeguard fails
- **THEN** unrestricted ingress fails closed to source-restricted pre-gate admission and requires a new valid approval before preview can resume

#### Scenario: The beta gate passes
- **WHEN** all measurements and operator drills pass for the exact deployed candidate with no exception
- **THEN** an authorized operator may enable rate-limited public beta admission and records the approval and release digest
