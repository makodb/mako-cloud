## ADDED Requirements

### Requirement: Public admission has four explicit modes
The deployment SHALL select public admission from exactly four modes, and
convergence MUST refuse any other value:

- `disabled`, the default: no public application traffic is served. The
  disabled configuration MUST stay closed even if the reverse proxy is started
  by hand, and the reverse proxy MUST NOT be enabled in this mode.
- `pre_gate`: admission is restricted by source address to the configured
  management and tester networks. Convergence MUST refuse this mode when no
  such network is configured.
- `risk_accepted_preview`: unrestricted admission under a valid preview
  approval. Convergence SHALL start in `pre_gate`, and a recurring guard SHALL
  select the preview configuration only while the approval and every
  non-waivable safeguard hold, falling back to `pre_gate` otherwise.
- `approved_beta`: unrestricted admission for a release whose single-region
  beta gate passed. Convergence MUST refuse this mode unless the recorded
  approval names the selected release digest. This mode has no preview guard:
  a failed safeguard SHALL raise an operator alert and MUST NOT by itself
  close admission.

The emergency admission stop SHALL remain authoritative in every mode.
Convergence SHALL render the reverse-proxy configuration of all four modes and
activate the selected one atomically. A rendered configuration or its selector
MUST NOT be edited by hand.

#### Scenario: Admission is disabled
- **WHEN** the admission mode is `disabled`
- **THEN** no public application traffic is served, including when the reverse proxy is started by hand

#### Scenario: Pre-gate admission names no authorized source
- **WHEN** `pre_gate` is selected and no management or tester network is configured
- **THEN** convergence refuses the configuration and admission is unchanged

#### Scenario: Approved beta is selected without a matching approval
- **WHEN** `approved_beta` is selected and the recorded approval does not name the selected release digest
- **THEN** convergence refuses the configuration and admission is unchanged

#### Scenario: A safeguard fails in approved beta
- **WHEN** a readiness, certificate or backup problem occurs while the admission mode is `approved_beta`
- **THEN** operators are alerted, admission is not closed automatically, and the emergency admission stop remains available to the operator

## MODIFIED Requirements

### Requirement: Operations and teardown are observable and recoverable
The deployment SHALL expose sanitized health, certificate, resource,
filesystem, RocksDB, backup, latency, error, audit, and service-restart signals
with actionable alerts. Runbooks MUST cover certificate failure, resource
exhaustion, service failure, storage recovery, compromise, upgrade, rollback,
and public-traffic disablement. Teardown MUST first disable traffic, preserve or
explicitly dispose of retained data and backups, revoke credentials and
certificates where appropriate, and require confirmation before deleting the
VM or persistent data.

#### Scenario: A critical readiness dependency fails
- **WHEN** storage, certificate, reverse-proxy, identity, policy, or gateway readiness becomes unhealthy
- **THEN** operators receive a correlation-aware alert, a risk-accepted preview fails closed to source-restricted pre-gate admission, and approved beta admission stays as the operator selected it

#### Scenario: Teardown is requested
- **WHEN** an operator requests removal of the beta environment
- **THEN** the workflow presents retained data, backup, credential, DNS, and certificate consequences and waits for explicit confirmation before destructive action
