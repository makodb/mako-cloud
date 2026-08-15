## Context

Mako Cloud currently has local development assets, native Rust service
binaries, a pinned containerized edge runtime, a single-owner production
RocksDB contract, and local qualification evidence. The single-region beta gate
is still blocked on an actual retained deployment, hosted HTTPS measurements,
30-day capacity and cost observations, and an operator-observed recovery and
rollback drill. See `proposal.md` for the motivation and
`specs/operations/public-beta-environment/spec.md` for the behavior contract.

The target is one VM in the Proxmox environment accessible from this workspace.
The address and name are fixed: `130.245.173.11` and
`cloud-test.makodb.com`. The network prefix, gateway, bridge, Proxmox node,
storage pools, DNS servers, management source network, and VM identifier are
not configuration guesses; they are inputs discovered and validated during the
apply workflow. The user has already created the DNS A record.

One VM is intentionally a single availability and durability domain. Local
RocksDB remains the production backend, so the design cannot offer automatic
failover or multi-region claims.

## Goals / Non-Goals

**Goals:**

- Produce a reviewable, idempotent Proxmox plan before external mutation and a
  recorded desired state after provisioning.
- Run the exact Mako candidate as non-root supervised services with exclusive,
  persistent RocksDB paths and off-live-path authenticated backups.
- Compose the existing Mako API and security contracts into production HTTP
  services before any reverse proxy or public admission can treat them as
  routable.
- Offer a publicly trusted HTTPS origin while exposing no internal service,
  storage, metrics, or administrative listener to the internet.
- Support controlled qualification traffic, repeatable upgrades and rollback,
  measurable beta gates, and recoverable teardown.
- Support a time-bounded, explicitly risk-accepted public preview without
  weakening the qualified-beta gate or any non-waivable application and
  infrastructure safeguard.
- Preserve enough environment evidence to reproduce or audit every release
  decision.

**Non-Goals:**

- High availability, automatic VM failover, live migration as an availability
  guarantee, multi-region storage, or distributed RocksDB access.
- A Kubernetes deployment or a reusable general-purpose Proxmox tenant
  platform.
- Changing Mako's public APIs, RxDB protocol, authorization model, or local
  RocksDB production decision.
- Treating certificate issuance or successful VM boot as approval for
  unrestricted beta traffic.
- Treating risk-accepted preview admission as evidence that the release meets
  qualified-beta availability, latency, alert-delivery, cost, or observation
  requirements.

## Decisions

### 1. Use a declarative Proxmox plan followed by an idempotent apply

Repository tooling will query Proxmox inventory and the node's existing network
configuration, resolve an unused VM identifier, and emit a sanitized plan file.
The plan contains the target node, bridge, prefix, gateway, DNS, storage pools,
source-image checksum, resource sizing, MAC address, static address, and
destructive-change classification. Apply consumes that exact plan and rejects
drift or a changed plan hash.

Address preflight combines Proxmox inventory, bridge-neighbor discovery, active
probing appropriate to the discovered subnet, and DNS checks from local and
public resolvers. Lack of a ping response is not considered proof that an
address is unused. Unexpected A or AAAA records, an inventory collision, or an
active neighbor aborts before `qm create`, clone, disk import, or network
assignment.

The preferred guest is a checksum-pinned Ubuntu 24.04 LTS cloud image using
UEFI, q35, virtio SCSI, QEMU guest agent, discard, and cloud-init. The proposed
starting size is 8 vCPU, 16 GiB RAM, a 64 GiB OS disk, and at least 256 GiB of
application data capacity; preflight may increase it to satisfy 30% headroom
but must stop if the host cannot safely supply it. Application checkpoint
backups require a separate Proxmox-backed or network destination rather than a
directory nested under a live RocksDB path.

Alternatives considered:

- LXC was rejected at the user's direction and would also complicate runtime
  isolation and nested container behavior.
- Terraform adds provider state and credentials for one environment without
  improving access to node-local facts. Small plan/apply tooling around the
  Proxmox API/CLI keeps state explicit and auditable.
- Manual `qm` commands are easy to start but are hard to review, reproduce,
  validate for collisions, and safely rerun.

### 2. Bootstrap minimally with cloud-init and converge the guest declaratively

Cloud-init sets the hostname, static network values, time synchronization, QEMU
guest agent, one key-only administrative identity, and a first-boot marker. It
does not contain application secrets. A pinned Ansible playbook then hardens and
converges the guest: package versions, automatic security updates, nftables,
filesystem ownership, Podman, Caddy, monitoring dependencies, service units,
backup timers, and log retention. The inventory is generated from the sanitized
Proxmox plan; credentials and private keys remain outside it.

Password authentication and direct SSH root login are disabled only after a
key-authenticated connection and Proxmox console recovery path are verified.
Guest reapplication is idempotent. VM replacement, disk recreation, and
teardown use separate explicit commands so ordinary convergence cannot delete
state.

Alternatives considered:

- A large cloud-init script has weak rerun and failure-recovery semantics.
- Hand-configuring the guest would make the qualification result
  non-reproducible.

### 3. Install immutable native service releases under systemd

CI-built, checksummed Rust binaries and static assets are staged beneath
`/opt/mako/releases/<release-digest>/`; `/opt/mako/current` selects one complete
release. Each Mako process receives its own unprivileged service identity,
configuration file, protected credential files, runtime directory, and systemd
unit. Units bind to loopback and use readiness, restart limits, file-system
protection, capability restrictions, and bounded graceful shutdown. Release
activation changes the selector atomically only after offline validation.

Pinned third-party dependencies and the edge runtime run rootless under Podman
with named persistent state where needed. The beta configuration must not reuse
the local stack's anonymous Grafana access. Metrics and dashboards bind only to
loopback and are accessed over an authenticated management path.

The data-plane and control-plane services use distinct paths on the persistent
data disk, for example `/var/lib/mako/data-plane/rocksdb` and
`/var/lib/mako/control-plane/rocksdb`, with ownership markers and one process
per path. Backups use distinct paths and an off-VM destination. A VM snapshot is
not a substitute for Mako's authenticated checkpoint backup.

Alternatives considered:

- Kubernetes adds orchestration complexity but cannot remove the one-VM,
  one-writer storage limitation.
- Packaging all stateful services into one container obscures process identity,
  ownership, and graceful RocksDB shutdown. Native supervised services make
  those boundaries explicit.

### 4. Compose storage-backed production HTTP services before proxying

The three native binaries own disjoint documented route families. The data
plane on loopback port 8080 owns application-user authentication, documents,
queries, service access, and RxDB pull, push, and SSE. The control plane on
loopback port 8081 owns developer and operator authentication plus organization,
project, environment, collection, schema, index, policy, application-user,
credential, function-administration, and observability routes. The edge gateway
on loopback port 8082 owns stable function invocation and streams qualified
requests to the pinned rootless runtime on loopback port 9000.

A shared native HTTP transport provides exact route and method matching,
bounded JSON bodies, correlation/request identifiers, stable public error
envelopes, response security defaults, graceful shutdown, and streaming support.
It does not implement domain decisions. Each service entrypoint constructs the
existing identity, document, sync, policy, gateway, audit, quota, and management
components over its exclusively owned production RocksDB adapter and exposes
route readiness only after their required dependencies are ready.

The generated OpenAPI document is the route contract and allowlist. Transport
adapters translate the published wire models to existing typed domain inputs and
translate safe domain outcomes back to the published responses. Authentication,
tenant scope, per-document policy, quotas, audit, idempotency, checkpoints, and
function-secret handling remain in their existing domain boundaries. A
recognized business route returning a placeholder or falling through to the
private readiness server is a qualification failure.

The data plane is the single persistent authority for project application
users, sessions, refresh families, public and service credentials, signing-key
rings, identity revocations, and authorization epochs. Authentication and JWKS
routes therefore operate directly against the data-plane RocksDB path. The
control plane remains authoritative for developer/operator identity,
organizations, projects, environment lifecycle, function deployments, and
versioned function secrets; it does not create a second application-identity
copy in its RocksDB path.

Control-plane application-user, project-credential, and signing-key
administration calls a private data-plane identity-administration API. The edge
gateway calls a private data-plane identity-verification API for protected
invocations and a private control-plane resolution API for immutable function
routes and exact secret versions. These calls use a reserved
`/_internal/v1/` namespace on loopback listeners. Caddy never proxies that
namespace. Each private route has an exact caller and method allowlist,
protocol version, tenant scope, bounded body, request and idempotency
identifiers, timestamp and nonce replay window, canonical payload digest, and
a deployment-keyed request signature. Authoritative RocksDB state records
mutation idempotency and replay guards with bounded retention. For mutations
whose success response contains one-time credential material, the data plane
also stores the exact successful response in a tenant-bound, deployment-key
encrypted, expiry-bounded response journal. An exact retry returns that response
without repeating the mutation; a changed digest conflicts; expiry removes both
the replayable response and its one-time material. The journal is never logged,
returned by inspection APIs, or readable by the control-plane RocksDB owner.
Safe response errors and audit events share the public request correlation
identifier but never contain credentials, tokens, signing material, secrets,
or document bodies.

The control plane uses the configured loopback S3-compatible object store for
immutable function bundles. The client validates tenant-derived object names,
content digests, response bounds, and immutable retry semantics, and uses
deployment-managed credentials; production composition cannot select the
in-memory reference store. A concrete runtime deployment client translates
`FunctionDeploymentBackend` operations to the versioned Mako runtime protocol
on the pinned loopback supervisor, binds deployment identity and correlation,
supplies secret values only on the sensitive trusted hop, and treats protocol,
health, timeout, or response-bound failures as unavailable. It never treats the
static canary files as a production deployment backend.

The beta deployment therefore runs a Mako-owned runtime supervisor on loopback
port 9001 beside the pinned Supabase edge runtime on port 9000. The supervisor
implements the exact versioned health, load, probe, test, log, and retire
operations consumed by the control-plane client. It authenticates every
management request with deployment-managed credentials, binds request and
deployment identity, rejects unbounded or incompatible payloads, and persists
the immutable bundle, exact secret-version inputs, lifecycle status, and bounded
logs under a service-owned durable runtime volume. Bundle and secret material is
encrypted at rest with separate deployment key material. On container or guest
restart it reconstructs and health-checks deployed workers before reporting
ready; the static canary remains diagnostic-only and can never satisfy
deployment readiness.

The production observability backend reads the control-plane audit store and
the configured loopback telemetry dependencies through bounded, typed queries.
It enforces tenant scope, requested retention windows, cursor and page limits,
response-size bounds, redaction, and dependency readiness. It does not return a
placeholder page or instantiate a test backend when Prometheus, logs, or audit
storage is unavailable.

The beta also runs a Mako-owned telemetry query service on loopback port 9465.
It implements the exact authenticated versioned health and query protocol
consumed by the observability backend, reads only configured persistent metric
and log sources, and requires a canonical project/environment tenant label on
every returned record. The service enforces retention, cursor, page, request,
and response bounds plus centralized sensitive-field redaction. Its durable
source offsets and query state survive ordinary restarts, and readiness fails
closed when a required source is missing, stale, malformed, or cannot prove
tenant isolation. Prometheus and raw collector output are dependencies of this
service, not directly exposed management APIs.

Private clients fail closed on authentication failure, clock/replay failure,
protocol mismatch, stale verification, timeout, or dependency unavailability.
They do not fall back to a memory implementation, a local duplicate, an empty
database, or token claims verified without current session and authorization
epochs. Readiness is dependency-specific: the data plane requires its local
identity and storage graph; control-plane identity-administration routes
require the private data-plane contract; and protected edge invocation requires
both fresh data-plane identity verification and control-plane function
resolution.

Alternatives considered:

- Returning route-shaped placeholder responses would make proxy and browser
  checks pass while leaving authentication, storage, and RxDB unusable.
- Implementing business logic in Caddy or a separate JavaScript facade would
  duplicate the typed Rust contracts and create a security boundary that the
  existing qualification suites do not cover.
- One combined native process would weaken the separate RocksDB ownership and
  least-privilege service identities already established for production.
- Letting the control plane persist application credentials or signing keys in
  its own database would create split authority and make data-plane
  authentication depend on unsafe RocksDB sharing or asynchronous secret
  duplication. A narrow authenticated internal API preserves one authority and
  explicit process boundaries.
- Replaying one-time credentials by regenerating them or storing plaintext
  responses would either change the mutation or violate secret-at-rest rules;
  the encrypted bounded response journal preserves both idempotency and
  one-time display semantics.
- Using the in-memory object store or test deployment/observability backends in
  production would make readiness and restart behavior dishonest, so production
  graph construction rejects those fallbacks.

### 5. Terminate public TLS with Caddy and an explicit route allowlist

Caddy binds public ports 80 and 443. Its automatic HTTPS support obtains and
renews an ACME certificate for `cloud-test.makodb.com`; initial validation uses
the ACME staging endpoint before the production issuer to avoid accidental rate
limits. Port 80 serves only the ACME path and HTTPS redirect. Mako services bind
to loopback, and Caddy proxies only the documented auth, management,
replication/SSE, and function routes, preserving streaming and request IDs.
Unknown routes are rejected.

The public URL in Mako configuration is HTTPS even though the trusted local hop
is loopback. Caddy adds strict transport security only after successful HTTPS
verification, applies request limits and security headers, and exports
certificate-expiry and proxy-health signals. Renewal is tested with a dry run
and alerts before the remaining lifetime becomes operationally unsafe.

Admission has four explicit states. `disabled` removes application admission,
`pre_gate` source-allowlists HTTPS to the derived operator and tester networks,
`risk_accepted_preview` admits public HTTPS under a current bounded risk
acceptance, and `approved_beta` admits public HTTPS after the single-region beta
gate passes. ACME validation remains separately reachable where required. Every
admitted state preserves normal project credentials, authentication, quotas,
policies, audits, the exact route allowlist, and private-listener isolation;
changing admission state never disables application security.

Risk-accepted preview uses a retained, non-secret JSON approval record. It names
the operator, exact plan and release digests, complete active blocker list and
its deterministic digest, acceptance timestamp, expiry timestamp no more than
14 days later, and a typed confirmation bound to those values. A generic
confirmation, approval for another release or plan, a changed blocker set, or
an expired record is invalid. Preview may accept only the latency,
alert-delivery, cost, and incomplete observation-window blockers identified by
the specification. Trusted HTTPS and HSTS, exact public routing, application
security, verified backup and recovery, zero acknowledged-write loss and
integrity failures, service readiness, and the tested emergency stop remain
non-waivable.

Convergence renders and validates separate fail-closed Caddy configurations for
each admission state and selects the active configuration atomically; operators
do not edit the generated live file. A systemd admission guard evaluates the
approval at least once per minute against the selected release, plan, blocker
digest, certificate state, application readiness, backup/recovery and integrity
signals, and emergency-stop readiness. Expiry or any mismatch atomically falls
back to `pre_gate` and records sanitized evidence. The existing emergency stop
remains the stronger operation and can remove admission completely.

Preview evidence is labelled `risk-accepted-public-preview` and retains the
accepted blocker set. Release-gate thresholds and results remain unchanged and
blocked until they independently pass. Reapproval is required after every
release, plan, or blocker change and at least every 14 days.

Alternatives considered:

- Nginx plus Certbot separates proxying and certificate lifecycle but introduces
  more renewal coordination for this single hostname.
- Service-native public TLS duplicates certificate distribution across
  processes and exposes more listeners.
- Reusing `approved_beta` for preview was rejected because it would make a
  deliberate risk acceptance indistinguishable from a passing release gate.
- A permanent or unbound preview switch was rejected because it could outlive
  the reviewed release, blocker set, or operator decision.

### 6. Enforce network policy at Proxmox and guest layers

Both layers default-deny ingress. The public network admits TCP 80 and 443;
administration admits TCP 22 only from the discovered operator network, with
Proxmox console as recovery. Internal service, Podman, RocksDB, object-store,
SMTP sink, OTLP, Prometheus, and Grafana listeners remain on loopback or a
private namespace. IPv6 is either configured and filtered equivalently or
disabled, and an unexpected public AAAA record blocks TLS/public readiness.

Outbound policy explicitly supports DNS, NTP, approved operating-system and
artifact repositories, ACME, backup transport, configured mail/object storage,
telemetry delivery, and edge-function HTTPS egress allowed by project policy.
Logging is rate-limited and excludes secrets and document bodies.

### 7. Make backup, release rollback, and disablement first-class operations

Systemd timers create signed Mako checkpoints at a cadence below the 15-minute
RPO ceiling and copy verified artifacts to the selected off-VM destination.
Monitoring treats a stale or unverifiable backup as a release blocker. Restore
always targets a new empty offline path and requires explicit promotion after
manifest, tenant-boundary, format, and acknowledged-high-water checks.

Release rollback disables Caddy application admission, drains services, takes
a verified checkpoint, checks database-format compatibility, selects the prior
immutable release, and restarts stateful services against the same fenced paths.
The operator then executes the existing policy, function, schema, signing-key,
and storage rollback matrix and retains timing and audit evidence. If rollback
is unsafe, traffic remains disabled while restore proceeds to an offline target.

An emergency stop operation removes application routes at Caddy and Proxmox
without destroying the guest. It is separate from teardown.

### 8. Treat the VM as a beta candidate until measured gates pass

Qualification records the release digest, Proxmox/guest plan hash, image and
runtime digests, firewall state, certificate chain and expiry, storage layout,
backup destination, start/end timestamps, load profile, and approver. Existing
security suites and production RocksDB qualification are rerun on the VM.
Hosted end-to-end measurements cover auth, RxDB pull/push/live, warm/cold
functions, recovery, capacity headroom, and certificate renewal.

The 30-day window records peak and percentile resource use plus allocated host,
power, network, backup, and operational cost; provider invoices are attached
where they exist. If owned infrastructure lacks enough cost evidence, that
observation stays null and unrestricted beta remains blocked rather than being
reported as zero cost.

Release-gate JSON and its validator consume the retained evidence but do not
infer a pass from deployment existence. Enabling unrestricted ingress requires
either all beta observations to pass with a recorded beta approval or a valid
time-bounded preview approval that preserves the blocked gate result and every
non-waivable safeguard.

## Risks / Trade-offs

- [Public IP is already allocated outside Proxmox inventory] → Combine inventory,
  neighbor, active, DNS, and routing checks; abort on any uncertainty before
  mutation.
- [A single VM or host failure stops all service] → State the outage contract,
  monitor aggressively, retain authenticated off-VM backups, and exercise
  restore; do not market high availability.
- [ACME validation or renewal fails] → Verify public DNS and port 80 before
  production issuance, test staging first, alert on expiry, and never fall back
  to plaintext.
- [Internet exposure invites abuse before qualification] → Default-deny both
  firewalls, allowlist qualification sources, enforce application auth/rate
  limits, and provide a one-command admission stop.
- [Risk-accepted preview exposes an unqualified release] → Bind approval to the
  exact plan, release, blockers, operator, and 14-day maximum; preserve all
  non-waivable safeguards, label the environment as preview, and automatically
  fall back to `pre_gate` on expiry, mismatch, or safety failure.
- [Edge runtime or third-party dependencies exhaust one guest] → Apply cgroup
  quotas, per-service limits, disk reserves, and load qualification with 30%
  headroom.
- [Backups share the VM's failure domain] → Require an off-VM destination;
  without one, keep the beta gate blocked.
- [Owned hardware makes invoice-based cost ambiguous] → Record allocation and
  actual provider/facility evidence; leave the gate blocked rather than treating
  sunk hardware as free.
- [Provisioning partially succeeds] → Persist step state and plan hash, make
  convergence idempotent, and separate non-destructive repair from confirmed
  teardown.
- [Repository services are not yet production-routable as assembled] → Compose
  every documented route into the native services, validate real domain-backed
  outcomes locally, and keep deployment plus public admission blocked while any
  route still falls through to readiness-only handling.
- [A private service call is forged, replayed, stale, or unavailable] → Bind the
  canonical signed request to caller, method, path, tenant, timestamp, nonce,
  idempotency identifier, and body digest; persist bounded replay/idempotency
  guards at the authority; fail affected readiness and routes closed.
- [A configured runtime or telemetry client has no compatible server] → Ship
  the matching loopback services in the immutable release, persist and recover
  their state, validate the exact wire protocols before service composition,
  and keep control-plane readiness closed on incompatibility or source loss.

## Migration Plan

1. Run read-only Proxmox, DNS, address, routing, capacity, and image discovery;
   emit and validate the sanitized desired-state plan.
2. Build and verify the immutable Mako candidate and pinned third-party
   artifacts without changing the Proxmox environment.
3. Create the VM, attach persistent disks, apply cloud-init networking, boot,
   and verify console plus key-only guest access before hardening SSH.
4. Converge the guest, provision service-owned data paths and the off-VM backup
   target, install the candidate, and keep public application routes disabled.
5. Compose and validate the documented production HTTP routes plus private
   identity and function-resolution contracts, start internal dependencies and
   Mako services, then pass API, internal-auth/replay, storage recovery,
   readiness, secret-redaction, tenant-boundary, and reboot tests.
6. Open ACME validation, issue the staging and production certificates, verify
   HTTPS/SSE/streaming behavior externally, and restrict qualification access to
   approved sources.
7. Run hosted security, durability, backup/restore, rollback, latency, load, and
   certificate-renewal drills; begin the 30-day capacity and cost window.
8. Update the release evidence. An authorized operator may either enable a
   time-bounded risk-accepted public preview for the exact blocker set and
   release or, only after every single-region beta threshold passes, enable
   qualified rate-limited public beta admission. Preview remains a blocked
   release-gate state and automatically returns to `pre_gate` when invalid.

Rollback during steps 3-6 disables public routes, stops the candidate, and
converges or removes only resources created by the recorded plan. Once data
exists, rollback preserves the VM and disks, follows the release procedure in
Decision 6, and requires explicit confirmation for destructive teardown.

Teardown disables ingress, captures final evidence and backups, revokes guest
and service credentials, handles the certificate and DNS consequences, and
presents every disk and backup scheduled for retention or deletion before
requesting destructive confirmation.

## Open Questions

- Which operational email address should Caddy register with the ACME provider,
  and which destination should receive expiry, backup, storage, and service
  alerts? These values are deployment secrets/contact configuration and do not
  change the architecture or acceptance criteria.
