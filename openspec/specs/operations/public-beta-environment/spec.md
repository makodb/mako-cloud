# Public Beta Environment Specification

## Purpose

Define the safety, security, persistence, HTTPS, operational, and release
contract for Mako Cloud's internet-reachable single-VM beta environment.

## Requirements

### Requirement: Provisioning uses discovered Proxmox values
The provisioning workflow SHALL discover the authoritative Proxmox node,
network bridge, address prefix, gateway, DNS servers, VM storage, image source,
and an unused VM identifier before creating resources. It MUST present the
resolved plan and refuse mutation when required values are missing or
inconsistent; it MUST NOT guess a gateway, prefix, bridge, storage pool, or VM
identifier.

#### Scenario: Common environment values are available
- **WHEN** the Proxmox environment exposes one valid set of common VM network and storage values
- **THEN** provisioning records those values in a reviewable plan before creating the VM

#### Scenario: Network values are ambiguous
- **WHEN** multiple plausible bridges or gateways exist and no authoritative choice can be derived
- **THEN** provisioning stops before mutation and identifies the values requiring operator selection

### Requirement: The requested identity and address are conflict-free
The environment SHALL use a dedicated virtual machine, the static guest address
`130.245.173.11`, and the public origin `cloud-test.makodb.com`. Before assigning
the address, provisioning MUST verify that the DNS A record resolves exactly to
that address and perform address-conflict checks from both the Proxmox network
and the intended guest network. An active address, duplicate VM identity, or
unexpected DNS result MUST stop provisioning without replacing an existing
resource.

#### Scenario: Address and identity are unused
- **WHEN** DNS resolves to `130.245.173.11`, conflict checks find no active owner, and the selected VM identifier is unused
- **THEN** the workflow may create a VM bearing the recorded beta-environment identity

#### Scenario: Address responds before assignment
- **WHEN** neighbor discovery, address probing, or Proxmox inventory indicates that `130.245.173.11` is already in use
- **THEN** provisioning fails before assigning the address and reports the conflict without altering the existing owner

### Requirement: VM provisioning is reproducible and least privilege
The beta environment SHALL run in a VM rather than an operating-system
container. Guest creation MUST use a pinned supported operating-system image,
explicit CPU, memory, disk, firmware, network, and boot configuration, and
key-only administrative access. Password login, direct root login, and
unnecessary guest devices or privileges MUST be disabled. Reapplying the same
desired configuration SHALL be idempotent, while replacement or destructive
reinitialization MUST require an explicit operator confirmation.

#### Scenario: Provisioning is repeated
- **WHEN** provisioning is rerun against a VM already matching the recorded desired state
- **THEN** it reports no destructive change and preserves guest disks and application data

#### Scenario: Existing VM differs destructively
- **WHEN** convergence would replace a disk, recreate the VM, or discard persistent state
- **THEN** the workflow stops and requires a specific destructive confirmation rather than proceeding automatically

### Requirement: Public traffic uses trusted HTTPS
The only supported public beta origin SHALL be
`https://cloud-test.makodb.com`. The deployment MUST obtain a publicly trusted
certificate only after DNS and reachability preflight succeeds, MUST redirect
plain HTTP to HTTPS except for required certificate validation traffic, and
MUST renew the certificate automatically before expiry. Certificate issuance or
renewal failure MUST be observable and MUST NOT expose an unencrypted Mako API
fallback.

#### Scenario: Initial certificate issuance succeeds
- **WHEN** the VM owns `130.245.173.11`, DNS resolves correctly, and the certificate authority can validate the hostname
- **THEN** the public origin serves a valid certificate for `cloud-test.makodb.com` and HTTP requests redirect to HTTPS

#### Scenario: Certificate cannot be renewed
- **WHEN** automated renewal fails or the remaining certificate lifetime crosses the alert threshold
- **THEN** operators receive an alert and the deployment never falls back to serving application traffic over plain HTTP

### Requirement: Network exposure is deny by default
Proxmox and guest firewalls SHALL deny unsolicited ingress by default. Public
ingress SHALL be limited to TCP ports required for certificate validation and
HTTPS application traffic. Administrative access MUST be restricted to the
derived operator network or Proxmox console, and internal Mako service,
database, observability, and management ports MUST NOT be directly reachable
from the public network. Egress SHALL be limited to documented operating-system,
certificate, backup, email, object-storage, and function-runtime dependencies.

#### Scenario: Public client probes an internal service port
- **WHEN** an unauthenticated internet client connects to a Mako internal, RocksDB, metrics, or administration port
- **THEN** the firewall rejects the connection without reaching that service

#### Scenario: Operator connects from an unauthorized source
- **WHEN** an SSH connection originates outside the approved management path
- **THEN** the connection is denied even when it presents a valid username

### Requirement: Deployment preserves the local RocksDB production contract
Each RocksDB-owned Mako service SHALL have one exclusively owned persistent local RocksDB path with synchronous durability, fail-closed readiness, capacity reserves, and no memory or empty-database fallback. The control plane SHALL instead have one exclusively owned SQLite database path with its required durability, migration identity, readiness, capacity reserves, and no browser, memory, empty-database, or RocksDB fallback. Every stateful path MUST be separate and survive ordinary service and VM restarts. The deployment MUST NOT claim multi-region durability, automatic storage failover, or uninterrupted availability during VM or disk loss.

#### Scenario: VM restarts normally
- **WHEN** the beta VM reboots with its persistent disks intact
- **THEN** RocksDB-owned services reopen their original databases, the control plane reopens its original SQLite authority, and each passes its own recovery checks before accepting affected traffic

#### Scenario: Persistent path is unavailable
- **WHEN** a RocksDB-owned service cannot open its configured path or prove acknowledged-high-water recovery
- **THEN** that service remains unready instead of creating an empty database or selecting another backend, while an independently healthy control plane can report and coordinate the incident

#### Scenario: SQLite control path is unavailable
- **WHEN** the control plane cannot open its configured SQLite path or prove schema, integrity, migration, and durable high-water readiness
- **THEN** control traffic remains unavailable instead of creating an empty database or falling back to RocksDB

### Requirement: Public-beta control storage is migrated and operated separately
The public-beta deployment SHALL provision a protected control SQLite path, migration workspace, backup staging and publish paths, service identity, configuration, monitoring, and recovery workflow independently of all RocksDB paths. Cutover MUST use a stopped and fenced control-plane RocksDB checkpoint, verified SQLite target, immutable release binding, and explicit operator promotion. Backup schedules, age alerts, restore drills, capacity evidence, and release qualification MUST identify SQLite and RocksDB results separately.

#### Scenario: Existing beta control state is cut over
- **WHEN** the exact migration-capable release has stopped control writes and verified the fenced source and target inventories
- **THEN** the deployment atomically selects SQLite, restarts the control plane, verifies developer and operator authentication plus control APIs, and retains the source checkpoint without reopening it

#### Scenario: Control migration qualification fails
- **WHEN** migration, readiness, authentication, authorization, audit, backup, restore, or failure-isolation evidence is incomplete or failing
- **THEN** the deployment does not promote the SQLite target or claim a qualified control-storage cutover

#### Scenario: Post-cutover code rollback is requested
- **WHEN** an operator requests release rollback after SQLite has accepted new writes
- **THEN** release tooling permits only a format-compatible SQLite-capable target and refuses a pre-SQLite binary without an explicit verified reverse migration

### Requirement: Deployment artifacts and secrets are controlled
The environment SHALL deploy a recorded immutable Mako release and pinned edge
runtime using reproducible configuration under service supervision. Secrets
MUST be generated or supplied through protected files or a deployment secret
mechanism, MUST be excluded from source control and command output, and MUST be
readable only by the intended service identity. Services MUST run without root
privileges and restart safely after guest reboot.

#### Scenario: Guest reboot completes
- **WHEN** the VM restarts after a healthy deployment
- **THEN** the reverse proxy and Mako services start in dependency order and public readiness returns only after required health checks pass

#### Scenario: Secret is inspected through deployment output
- **WHEN** an operator reviews plans, logs, process arguments, or generated evidence
- **THEN** no private key, password, raw token, service credential, or function secret is disclosed

### Requirement: Production services compose the existing Mako API contracts
The native production services SHALL handle the documented management,
operator, authentication, document, service-access, RxDB replication/SSE, and
edge-function routes through their existing storage-backed domain behavior.
Recognized routes MUST NOT be represented by placeholder success responses or
private readiness handlers. Every route SHALL preserve the published method,
request, response, stable-error, authentication, authorization, tenant-scope,
quota, policy, audit, and streaming contracts. Unknown routes and unsupported
methods MUST fail closed.

#### Scenario: Authorized management request reaches the control plane
- **WHEN** an authorized developer or operator calls a documented management route
- **THEN** the control-plane service dispatches the request to the corresponding persistent management behavior and returns the published response contract

#### Scenario: Application user authenticates and accesses documents
- **WHEN** an application user calls a documented authentication, document, query, or service-access route
- **THEN** the data-plane service validates the request, uses the configured identity and document services, and enforces tenant scope, credentials, policies, quotas, and audit before returning data

#### Scenario: RxDB client synchronizes through the production service
- **WHEN** an authenticated RxDB client calls pull, push, or replication-stream for a collection
- **THEN** the data-plane service preserves the published checkpoint, idempotency, conflict, visibility, bounded-batch, and SSE behavior against the production RocksDB path

#### Scenario: Function invocation reaches the pinned edge runtime
- **WHEN** an authenticated caller invokes a documented function route
- **THEN** the edge gateway validates tenant scope, credentials, quotas, limits, secrets, and audit context before streaming the request and response through the pinned edge runtime

#### Scenario: A route dependency is unavailable
- **WHEN** identity, policy, storage, audit, quota, or edge-runtime readiness required by a documented route cannot be proven
- **THEN** the affected route returns a stable fail-closed error and the service does not advertise route readiness

#### Scenario: Unknown route or unsupported method is requested
- **WHEN** a caller uses a path outside the documented allowlist or a method not declared for that path
- **THEN** the service returns a stable not-found or method-not-allowed response without invoking a domain service

### Requirement: Production management dependencies are persistent and real
The production control-plane graph SHALL use the configured authenticated
S3-compatible object store for immutable function bundles, a versioned client
for the pinned loopback edge-runtime supervisor for deployment lifecycle, and
retention-bounded tenant-scoped observability readers for audit and configured
telemetry sources. Production startup and affected route readiness MUST reject
in-memory object storage, test backends, placeholder pages, an incompatible
runtime protocol, or an unavailable required dependency. Bundle, runtime, and
observability clients MUST enforce bounded requests and responses, correlation,
redaction, tenant scope, and stable safe errors.

The deployment SHALL run an authenticated Mako runtime-supervisor server on a
loopback-only listener implementing the client's exact versioned health, load,
probe, test, log, and retire operations. It MUST persist encrypted immutable
deployment inputs, exact secret-version bindings, lifecycle state, and bounded
logs on a service-owned durable path, reconstruct healthy deployed workers
after ordinary service or guest restart, and MUST NOT substitute a static
canary for a loaded worker.

The deployment SHALL run an authenticated Mako telemetry-query server on a
loopback-only listener implementing the observability client's exact versioned
health and bounded-query operations. It MUST query only configured persistent
metric and log sources, require canonical project/environment tenant labels,
enforce retention, cursor, page, request, response, and redaction bounds, retain
restart-safe source state, and fail readiness when source compatibility,
freshness, or tenant isolation cannot be proven.

#### Scenario: Function bundle and deployment survive service restart
- **WHEN** an authorized developer uploads and deploys an immutable function bundle and the control-plane service restarts
- **THEN** the configured object store still returns the digest-matching bundle and the runtime deployment backend reports the same tenant-bound version without using process memory

#### Scenario: Observability dependency is unavailable
- **WHEN** an authorized observability query requires an audit or telemetry source whose compatible bounded response cannot be proven
- **THEN** the route returns a stable unavailable error and does not synthesize an empty or successful placeholder page

#### Scenario: Runtime supervisor restarts
- **WHEN** the runtime-supervisor service or beta guest restarts after a healthy function deployment
- **THEN** the supervisor decrypts and reconstructs the same immutable tenant-bound worker, verifies its readiness, and reports the deployment without requiring process-memory state or exposing its secrets

#### Scenario: Runtime management request is incompatible or unauthorized
- **WHEN** a caller supplies invalid deployment authentication, an unsupported protocol, a changed deployment digest, or an over-limit payload
- **THEN** the supervisor rejects the operation before loading or mutating worker state and returns only a bounded correlated safe error

#### Scenario: Telemetry query spans another tenant
- **WHEN** telemetry source data is unlabeled, ambiguously labeled, or labeled for a different project or environment
- **THEN** the telemetry-query service excludes it and fails closed if the requested tenant scope cannot be proven

#### Scenario: Telemetry service restarts
- **WHEN** the telemetry-query service restarts with its persistent sources intact
- **THEN** it resumes from durable source state and returns the same bounded tenant-scoped query results without synthesizing an empty success

### Requirement: Application identity has one data-plane authority
The data plane SHALL be the sole persistent authority for project application
users, sessions, public and service project credentials, signing-key rings, and
authorization epochs. Control-plane application-user, credential, and
signing-key administration MUST invoke versioned private data-plane operations
instead of persisting a second identity copy or opening the data-plane RocksDB
path. Stateless edge verification MUST use a private identity-verification
operation or a freshness-bounded verified result from that authority. Function
deployment and versioned function-secret state SHALL remain control-plane
owned and SHALL be resolved by the edge gateway through a private
control-plane operation. No private operation may be admitted through the
public reverse-proxy allowlist.

Private service calls MUST use deployment-managed authentication, explicit
caller and tenant scope, bounded inputs, request and idempotency identifiers,
replay protection, safe stable errors, and audit correlation. Identity
mutations MUST be idempotent across ambiguous transport retries, and a changed
mutation under the same idempotency identifier MUST conflict. A missing,
invalid, stale, replayed, or protocol-incompatible private dependency MUST fail
closed without falling back to local memory, duplicated records, an empty
database, or unverified token claims.

The data-plane identity-administration server SHALL implement every private
operation required by the published application-user, project-credential, and
signing-key administration routes. For a successful identity mutation whose
response includes one-time credential material, it MUST durably journal the
exact response encrypted under deployment-managed key material and bound to the
tenant, operation digest, and expiry. An exact ambiguous retry MUST return the
same response without repeating the mutation; a different digest under the same
idempotency identifier MUST conflict. Expired journals MUST be unusable and
eligible for bounded cleanup, and no inspection, error, audit, or log response
may expose their plaintext.

#### Scenario: Control plane administers application identity
- **WHEN** an authorized developer administers an application user, project credential, or signing key through the control-plane API
- **THEN** the control plane preserves its management authorization and audit context while an authenticated, tenant-bound, idempotent private call applies the identity mutation to the data-plane-owned RocksDB state

#### Scenario: One-time credential response is lost in transit
- **WHEN** an identity mutation succeeds but its response is ambiguous and the control plane retries the exact tenant-bound request with the same idempotency identifier
- **THEN** the data plane returns the encrypted-journaled original response without creating or rotating another credential, while changed input conflicts

#### Scenario: Edge gateway verifies an application caller
- **WHEN** the edge gateway receives a protected function invocation
- **THEN** it obtains a freshness-proven identity result from the data-plane authority and rejects the invocation if token, session, tenant, authorization epoch, or private dependency verification fails

#### Scenario: Edge gateway resolves function configuration
- **WHEN** the edge gateway selects a deployed function version and its declared secrets
- **THEN** it obtains the immutable route and exact active secret versions through an authenticated private control-plane call without persisting plaintext secret copies

#### Scenario: Private call is forged or replayed
- **WHEN** a caller omits valid deployment authentication, changes the bound tenant or payload, reuses a consumed replay value, or targets a private operation through the public origin
- **THEN** the operation is rejected before reading or mutating identity, credential, signing-key, function, or secret state and produces only a safe correlated audit outcome

#### Scenario: Private identity dependency is unavailable
- **WHEN** the control plane or edge gateway cannot prove a compatible and fresh connection to the required identity authority
- **THEN** affected administration and invocation routes return a stable unavailable response and do not use duplicated or stale authority state

### Requirement: Backups and rollback are exercised off the live paths
The beta environment SHALL create authenticated checkpoint backups outside each
live RocksDB path, enforce the beta backup-age objective, and verify restore to
an empty offline target. Operators SHALL exercise service, configuration,
policy, function, schema, signing-key, and format-compatible storage rollback
for the exact candidate release. Restore promotion and rollback MUST be
operator-controlled and MUST NOT overwrite the sole live copy automatically.

#### Scenario: Scheduled backup completes
- **WHEN** a healthy stateful database reaches its backup schedule
- **THEN** an authenticated checkpoint and manifest are stored outside the live path and verified before the backup is considered successful

#### Scenario: Operator performs the beta recovery drill
- **WHEN** an operator restores the selected backup to an empty target and executes the documented rollback matrix
- **THEN** recovery, readiness, tenant-boundary, audit, and rollback evidence are recorded without destroying the original live volume

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
- **THEN** public application admission fails closed and operators receive a correlation-aware alert

#### Scenario: Teardown is requested
- **WHEN** an operator requests removal of the beta environment
- **THEN** the workflow presents retained data, backup, credential, DNS, and certificate consequences and waits for explicit confirmation before destructive action
