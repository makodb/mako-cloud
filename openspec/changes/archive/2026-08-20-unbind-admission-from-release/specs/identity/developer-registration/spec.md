## MODIFIED Requirements

### Requirement: Registration exposure is observable and deny-by-default
The reverse proxy SHALL expose only the documented public developer-auth and
wait-list self-service routes plus the already protected operator routes. It
MUST NOT expose private identity operations, raw storage, mail, metrics, or
session-issuance utilities. Registration, verification, authentication,
recovery, review, decision, mail-outbox, and denial signals SHALL be observable
with bounded cardinality and no raw email, password, token, reviewer note, IP
address, or session value in metric labels or ordinary logs. An operator MAY
explicitly accept the documented residual risks and select unrestricted
public-preview admission for one exact plan and blocker set without a calendar
expiry. That acceptance MUST remain revocable through an explicit manual pause
and MUST fail closed to source-restricted admission when its approval binding
or any non-waivable safeguard becomes invalid. Deploying a different release
MUST NOT by itself close admission; the approval records the release its
safeguards were measured on as provenance rather than as a binding.

#### Scenario: Public caller probes a private identity route
- **WHEN** an internet client requests an identity route outside the published allowlist
- **THEN** the proxy and service fail closed without invoking a private operation or revealing whether an identity exists

#### Scenario: Operator monitors wait-list health
- **WHEN** an authorized operator inspects registration health
- **THEN** the system reports aggregate rate, queue, decision, delivery, throttle, and failure signals without applicant identifiers or secrets in metrics

#### Scenario: Accepted preview remains open over time
- **WHEN** an operator has accepted the residual risks for the current plan and blocker set and every non-waivable safeguard remains valid
- **THEN** public-preview admission remains active without requiring periodic calendar-based renewal, and remains active across a release deployment

#### Scenario: Operator manually pauses public preview
- **WHEN** an authorized operator invokes the documented manual pause operation
- **THEN** public-preview admission atomically returns to source-restricted `pre_gate` without stopping private services or deleting wait-list state

#### Scenario: Persistent preview safety binding drifts
- **WHEN** the approval binding, TLS, readiness, backup, recovery, route-isolation, integrity, or emergency-stop safeguard becomes invalid
- **THEN** the guard returns admission to `pre_gate` even though the operator has not manually paused it
