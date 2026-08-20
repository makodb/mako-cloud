## Purpose

Define the plans an organization can be on and make a plan's limits the limits the gateway actually enforces.

## ADDED Requirements

### Requirement: A plan catalog exists as data
The platform SHALL define plans, their limits, and their entitlements as versioned data rather than compiled constants, so a plan can change without a code change and a past period resolves against the plan version that applied to it.

#### Scenario: A plan's limits change
- **WHEN** a plan's limits are revised
- **THEN** organizations on that plan resolve the new limits, and a closed period still resolves the version that applied when it was open

### Requirement: An organization subscribes to a plan
Every organization SHALL have exactly one effective plan at any time, defaulting to the free plan. A change of plan MUST record when it takes effect and MUST NOT retroactively alter a closed period.

#### Scenario: An organization changes plan mid-period
- **WHEN** an organization moves to a different plan partway through a period
- **THEN** the change records its effective time and both plans' terms apply to their own portions of the period

### Requirement: Enforced limits follow from the plan
The gateway SHALL resolve the limits it enforces from the tenant's effective plan and from any operator quota override that applies, rather than from a single policy shared by every tenant. Resolution MUST be cached with a bounded lifetime, and a resolution failure MUST fall back to the most restrictive plan's limits and never to unlimited.

#### Scenario: Two tenants on different plans
- **WHEN** two tenants on different plans issue the same request rate
- **THEN** each is limited according to its own plan

#### Scenario: An operator raises a tenant's limit
- **WHEN** an operator records a quota override for a tenant
- **THEN** the gateway enforces the overridden limit within the cache lifetime

#### Scenario: Plan resolution fails
- **WHEN** the effective plan cannot be resolved
- **THEN** the tenant is limited to the most restrictive plan's limits and the failure is reported
