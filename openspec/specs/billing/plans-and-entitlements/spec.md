# Plans and Entitlements Specification

## Purpose

Define the plans a team can be on and make a plan's limits the limits the gateway actually enforces.

## Requirements

### Requirement: A plan catalog exists as data
The platform SHALL define plans, their entitlements, and their overage terms as versioned data, so a past period resolves against the plan version that applied to it rather than the version of the day it is read.

#### Scenario: A plan's terms change
- **WHEN** a plan's terms are revised in the catalog
- **THEN** new derivations resolve the revised plan while a closed period keeps the version it was rated under

### Requirement: A team subscribes to a plan
Every team SHALL have exactly one effective plan at any time, defaulting to the free plan. A change of plan MUST record when it takes effect and MUST NOT retroactively alter a closed period.

#### Scenario: A team changes plan mid-period
- **WHEN** a team moves to a different plan partway through a period
- **THEN** the change records its effective time and both plans' terms apply to their own portions of the period

### Requirement: Enforced limits follow from the plan
The gateway SHALL resolve the limits it enforces from the tenant's effective plan and from any operator quota override that applies, rather than from a single policy shared by every tenant. Resolution MUST be cached with a bounded lifetime, and a resolution failure MUST fall back to the deployment's default limits and never to unlimited.

#### Scenario: Two tenants on different plans
- **WHEN** two tenants on different plans issue the same request rate
- **THEN** each is limited according to its own plan

#### Scenario: An operator raises a tenant's limit
- **WHEN** an operator records a quota override for a tenant
- **THEN** the gateway enforces the overridden limit within the cache lifetime

#### Scenario: Plan resolution fails
- **WHEN** the tenant's installed policy cannot be resolved
- **THEN** the tenant is limited to the deployment's default limits, never left unlimited
