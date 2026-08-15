## Why

Mako Cloud needs a representative, internet-reachable environment to replace
local-only qualification with measured single-region beta evidence. A dedicated
Proxmox VM at `130.245.173.11`, served as `cloud-test.makodb.com` over trusted
HTTPS, provides an isolated target for deployment, operations, rollback, and
limited beta testing without changing the single-owner local RocksDB design.

## What Changes

- Provision a dedicated VM on the current Proxmox environment after discovering
  and validating the cluster's normal bridge, prefix, gateway, storage, DNS, and
  an unused VM identifier.
- Assign `130.245.173.11` only after conflict and reachability preflight checks,
  and configure `cloud-test.makodb.com` as the sole public beta origin.
- Install a reproducible, least-privilege Mako Cloud deployment with separate
  persistent RocksDB directories, backup storage, pinned artifacts, system
  service supervision, and restart-safe configuration.
- Compose the existing Mako management, authentication, document, RxDB
  replication/SSE, and edge-function contracts into the production service
  binaries so every documented route is handled by its real storage-backed
  domain service before reverse-proxy admission is configured.
- Make the data plane authoritative for project application users, sessions,
  public and service credentials, signing-key rings, and authorization epochs.
  Control-plane identity administration and stateless edge verification use a
  versioned, authenticated internal RPC contract instead of opening or
  duplicating the data-plane RocksDB state.
- Complete the production adapters required by the management surface: a
  data-plane identity-administration server with encrypted retry results,
  persistent S3-compatible function-bundle storage, a pinned edge-runtime
  deployment client, and a retention-bounded observability reader. Deploy the
  corresponding Mako-owned loopback runtime-supervisor and telemetry-query
  servers so those clients have real, restart-safe production dependencies
  rather than configured but absent endpoints.
- Terminate HTTPS with an automatically issued and renewed public certificate,
  redirect HTTP to HTTPS, and expose only the required public API and function
  routes through a reverse proxy.
- Apply host and Proxmox firewall rules, secret handling, backup, monitoring,
  audit, recovery, upgrade, and rollback procedures appropriate for an
  internet-facing beta.
- Collect environment-specific durability, recovery, security, latency,
  capacity, cost, certificate-renewal, and operator-drill evidence. Keep
  qualified public beta traffic blocked until the single-region beta gate
  passes, while permitting a separate risk-accepted public preview only after
  an authorized operator records a time-limited, release- and plan-bound
  acceptance that names every missing or failing observation. Preview mode
  retains trusted HTTPS, the public route allowlist, authentication, document
  policies, quotas, rate limits, audit, backups, and emergency disablement, and
  MUST NOT be represented as a passing or qualified beta release.

## Capabilities

### New Capabilities

- `operations/public-beta-environment`: Defines safe provisioning, HTTPS
  exposure, deployment, persistence, operation, qualification, and teardown of
  the single-VM Proxmox public beta environment.

### Modified Capabilities

None.

## Impact

- Adds Proxmox and guest provisioning assets, deployment configuration,
  firewall/reverse-proxy configuration, operational scripts, and beta evidence.
- Adds production HTTP composition and transport code to the native data-plane,
  control-plane, and edge-gateway service entrypoints without changing their
  existing public API contracts.
- Adds private service-to-service identity administration and verification
  calls, protected by deployment credentials, tenant binding, replay controls,
  bounded inputs, stable safe errors, and fail-closed dependency readiness.
- Adds production implementations for object storage, runtime deployment, and
  observability boundaries that currently have only reference or test
  implementations; their listeners remain loopback-only and their credentials
  stay in protected deployment files.
- Adds authenticated loopback runtime-supervisor and telemetry-query services
  with durable state, bounded versioned protocols, tenant isolation, redaction,
  and fail-closed restart/readiness behavior.
- Operates the existing Mako Cloud services and local RocksDB production
  topology without changing public API contracts or adding a distributed
  storage backend.
- Creates external infrastructure and DNS/TLS dependencies during the later
  apply workflow: a Proxmox VM, the static address `130.245.173.11`, ACME
  certificate issuance for `cloud-test.makodb.com`, and restricted public
  network exposure.
- Requires deployment-specific secrets and operator confirmation before
  provisioning, traffic enablement, destructive rebuild, or teardown. A
  public-preview confirmation is distinct from beta approval, expires
  automatically, is bound to the exact plan and release, and records the risks
  accepted without altering release-gate evidence or thresholds.
