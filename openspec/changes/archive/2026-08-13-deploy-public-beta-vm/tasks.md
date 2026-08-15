## 1. Proxmox Discovery and Safe Planning

- [x] 1.1 Define a typed, non-secret desired-state schema for the `cloud-test.makodb.com` VM, fixed address, resource sizing, image checksum, network, storage, management sources, and plan hash.
- [x] 1.2 Implement read-only Proxmox discovery for nodes, bridges, prefixes, gateways, DNS servers, storage capabilities and free capacity, existing guests, the next unused VM identifier, and approved guest images.
- [x] 1.3 Implement DNS A/AAAA, Proxmox inventory, neighbor, routing, and active address-conflict checks for `130.245.173.11`, treating uncertainty or an unexpected record as a hard pre-mutation failure.
- [x] 1.4 Generate a sanitized reviewable plan from discovered values, classify destructive differences, bind apply to its hash, and reject ambiguous or guessed network and storage values.
- [x] 1.5 Add fixture-driven tests for successful discovery, multiple bridges or gateways, insufficient host capacity, used VM identifiers, DNS mismatch, active-address collision, secret redaction, and plan drift.
- [x] 1.6 Run the read-only planner in the current Proxmox environment and retain the resolved node, VM identifier, bridge, prefix, gateway, DNS, storage, image, capacity, management source, and conflict-free evidence without creating a resource.

## 2. VM Provisioning

- [x] 2.1 Pin and verify the Ubuntu 24.04 LTS cloud image and define explicit UEFI/q35, virtio, QEMU-agent, CPU, memory, OS-disk, data-disk, backup-target, MAC, boot, and static-network settings.
- [x] 2.2 Create minimal cloud-init templates for hostname, discovered static networking, time sync, QEMU guest agent, first-boot state, and a key-only administrative user without embedding application secrets.
- [x] 2.3 Implement idempotent VM apply and inspect operations that consume the recorded plan, preserve matching resources, reject drift, and require explicit typed confirmation for disk replacement or VM recreation.
- [x] 2.4 Add dry-run and fixture tests proving repeated apply is non-destructive, partial provisioning can resume safely, and destructive changes or stale plan hashes stop before mutation.
- [x] 2.5 Apply the conflict-free plan to create the VM and persistent disks, assign `130.245.173.11`, boot it, and retain sanitized Proxmox task and resulting desired-state evidence.
- [x] 2.6 Verify guest-agent health, console recovery, static routing, DNS resolution, time synchronization, key-based SSH, disk identity, and reboot persistence before guest hardening proceeds.

## 3. Guest Convergence and Network Hardening

- [x] 3.1 Add pinned Ansible inventory generation, dependency metadata, and idempotent roles for the base OS, security updates, time sync, QEMU agent, nftables, Caddy, Podman, service accounts, filesystems, logs, monitoring, and backup timers.
- [x] 3.2 Configure key-only SSH and disable password and direct root login only after verified key access and Proxmox console recovery; test a second convergence run for no destructive changes.
- [x] 3.3 Implement Proxmox and guest default-deny firewalls exposing only public TCP 80/443 and management TCP 22 from the discovered operator network, with explicit IPv6 handling and rate-limited firewall logs.
- [x] 3.4 Restrict outbound traffic to documented system, DNS/NTP, ACME, artifact, backup, mail/object-storage, telemetry, and policy-approved edge-function dependencies without exposing secrets in rules or logs.
- [x] 3.5 Verify from authorized and unauthorized external vantage points that 80/443 follow the current admission mode, management access is restricted, and every internal Mako, RocksDB, Podman, object-store, SMTP, OTLP, Prometheus, and Grafana port is unreachable publicly.

## 4. Immutable Mako Deployment

- [x] 4.1 Build the exact Mako candidate and console assets, verify all existing CI and qualification checks, and create a checksummed release manifest containing the source, runtime, dependency, and artifact digests.
- [x] 4.2 Add a release installer that stages complete immutable releases below `/opt/mako/releases/<digest>`, validates them offline, retains the last-known-good release, and atomically selects `/opt/mako/current` without overwriting an active release.
- [x] 4.3 Provision distinct service-owned production RocksDB paths, format and ownership markers, disk reserves, synchronous durability configuration, backup staging, and an off-VM authenticated backup destination with no memory or empty-path fallback.
- [x] 4.4 Add hardened systemd service and timer units for Mako services, checkpoint backups, and health collection with non-root identities, protected filesystems, credential files, restart limits, dependency ordering, and bounded graceful shutdown.
- [x] 4.5 Add a beta-only rootless Podman deployment for pinned third-party dependencies and the edge runtime, disabling anonymous observability access and binding all dependency and metrics listeners to loopback or private namespaces.
- [x] 4.6 Generate production configuration for `https://cloud-test.makodb.com`, loopback listeners, separate RocksDB paths, off-live-path backups, internal authentication, object storage, mail, OTLP, quotas, limits, and protected secret references; prove plans and logs redact every secret.
- [x] 4.7 Implement a shared production HTTP transport with exact route/method matching, bounded bodies, request identifiers, stable API errors, graceful shutdown, response security defaults, and streaming support while preserving private health/readiness routes.
- [x] 4.8 Compose the production data-plane service graph over its owned RocksDB path as the sole authority for project application users, sessions, credentials, signing-key rings, revocations, and authorization epochs, together with document, policy, gateway, quota, audit, and readiness dependencies and no memory, duplicate-state, or empty-path fallback.
- [x] 4.9 Implement the versioned `/_internal/v1/` service contract and clients with exact caller/method allowlists, deployment-keyed canonical request signatures, tenant binding, bounded bodies, timestamps, nonces, replay and idempotency guards, safe correlated errors, dependency readiness, and reverse-proxy exclusion; cover control-to-data identity administration, edge-to-data identity verification, and edge-to-control function/secret resolution.
- [x] 4.10 Implement the documented data-plane authentication, JWKS, document, query, and service-access HTTP routes with strict wire-model validation, tenant binding, policy enforcement, quotas, audit, and stable error mapping.
- [x] 4.11 Implement the documented RxDB pull, push, and SSE routes with bounded batches, signed tenant-bound checkpoints/cursors, idempotent mutations, conflict visibility, heartbeats, overflow resync, and disconnect-safe streaming.
- [x] 4.12 Compose the production control-plane service graph over its owned RocksDB path with developer/operator authentication, RBAC, audit, provisioning, management, data-plane identity-administration client, function-resolution service, and readiness dependencies.
- [x] 4.13 Implement organization, project, environment, and lifecycle HTTP routes through the existing control-plane services with developer/operator authorization and audit.
- [x] 4.14 Implement collection, schema, migration, index, and policy HTTP routes through the existing compatibility, policy, and control-plane services with stable asynchronous lifecycle responses.
- [x] 4.15a Implement and register the authenticated data-plane identity-administration server operations, including tenant/RBAC audit context and an expiry-bounded deployment-key-encrypted success-response journal that replays one-time credential results for exact ambiguous retries and conflicts on changed digests.
- [x] 4.15b Implement a production S3-compatible immutable object-store adapter for the configured loopback service with deployment-managed authentication, tenant-derived addresses, digest verification, bounded I/O, immutable retry semantics, and no memory fallback.
- [x] 4.15c Implement the production `FunctionDeploymentBackend` client for the pinned versioned loopback runtime supervisor, including deployment/health/test/log/delete operations, exact secret delivery, correlation, response bounds, protocol validation, and fail-closed readiness.
- [x] 4.15d Implement the production `ObservabilityBackend` over the persistent audit store and configured loopback telemetry sources with tenant and retention binding, redaction, bounded cursors/pages/responses, and fail-closed dependency readiness.
- [x] 4.15e Implement and deploy the authenticated Mako runtime-supervisor HTTP server on loopback port 9001 with the complete versioned health/load/probe/test/log/retire protocol, encrypted durable deployment state, bounded logs and payloads, pinned-runtime worker lifecycle, and service/guest-restart recovery.
- [x] 4.15f Implement and deploy the authenticated Mako telemetry-query HTTP server on loopback port 9465 over persistent tenant-labelled metric and log sources, with versioned health/query operations, retention/cursor/page/response bounds, redaction, durable source state, and fail-closed readiness.
- [x] 4.15 Implement application-user, project-credential, and signing-key administration through the private data-plane identity authority; implement control-owned function secrets, function deployment, observability, and operator HTTP routes with redaction, RBAC, audit, and bounded pagination.
- [x] 4.16 Compose the edge gateway invocation route with private data-plane identity verification, tenant binding, quotas, limits, private control-plane function and exact secret-version resolution, audit context, and streaming proxying to the pinned rootless edge runtime.
- [x] 4.17 Add route-contract, internal-auth/replay/idempotency, method, request-limit, stable-error, request-ID, tenant-boundary, policy, RxDB streaming, edge-runtime, unknown-route, dependency-failure, graceful-shutdown, and production-RocksDB integration tests for all three service binaries.
- [x] 4.18 Deploy the candidate with public application and private RPC routes externally disabled and verify startup, documented API and private dependency readiness, edge runtime compatibility, exclusive storage ownership, semantic recovery, acknowledged high water, tenant isolation, policy enforcement, internal replay rejection, and guest-reboot recovery.

## 5. Trusted HTTPS and Controlled Admission

- [x] 5.1 Create a Caddy configuration that proxies only documented auth, management, RxDB replication/SSE, and function routes, preserves streaming and request identifiers, rejects unknown routes, applies request limits and security headers, and never exposes a plaintext application fallback.
- [x] 5.2 Add separate pre-gate and approved-beta admission modes: ACME validation remains reachable, while application HTTPS is initially limited to configured operator and tester networks without bypassing Mako authentication, policies, quotas, rate limits, or audit.
- [x] 5.3 Validate the public DNS A record, absence or correct handling of AAAA, public routing, and port 80 reachability; issue a staging certificate before obtaining the production trusted certificate for `cloud-test.makodb.com`.
- [x] 5.4 Verify externally that HTTP redirects to HTTPS, the certificate chain and hostname validate, HSTS is enabled only after verification, unknown/internal routes fail closed, and RxDB SSE plus function streaming work through the proxy.
- [x] 5.5 Configure and test automatic certificate renewal, expiry metrics, alert thresholds, and failure behavior using the operator-provided ACME contact and alert destination, proving renewal failure cannot enable plaintext service.

## 6. Backup, Recovery, Rollback, and Operations

- [x] 6.1 Schedule signed checkpoint backups frequently enough for the 15-minute RPO, copy and verify them off the live paths and VM failure domain, enforce retention, and alert on failure, age, digest, or manifest violations.
- [x] 6.2 Exercise an authenticated restore into an empty offline target and explicit promotion, verifying database format, tenant inventory, acknowledged high water, readiness, recovery point, and recovery time while preserving the original live paths.
- [x] 6.3 Implement and test an emergency public-admission stop that removes application routes without deleting the VM, disks, evidence, or backups.
- [x] 6.4 Implement release inspection, upgrade, and format-compatible rollback commands that drain traffic, checkpoint, fence stateful paths, select an immutable release, and refuse empty-path or incompatible rollback.
- [x] 6.5 Exercise service, configuration, policy, function, schema, signing-key, and storage rollback on the deployed candidate and retain operator, timing, readiness, tenant-boundary, and audit evidence.
- [x] 6.6 Add sanitized dashboards and actionable alerts for service/readiness, certificate, filesystem, RocksDB, backup, recovery, latency, errors, audit, process restart, Podman, and host saturation signals without public observability listeners.
- [x] 6.7 Publish environment runbooks for access, deploy, certificate failure, capacity, backup/restore, compromise, service failure, rollback, admission stop, and confirmed teardown, including the single-VM outage and non-HA contract.
- [x] 6.8 Implement a teardown planner that first disables traffic and enumerates VM disks, backups, credentials, certificates, DNS consequences, and retained evidence, requiring explicit typed confirmations and never running as part of ordinary convergence.

## 7. Hosted Beta Qualification

- [x] 7.1 Rerun auth, policy, RxDB chaos, tenant-boundary, edge-runtime, dependency, and production RocksDB security/durability suites against the exact deployed release and retain machine-readable hosted evidence.
- [x] 7.2 Run hosted end-to-end benchmarks for auth, RxDB pull/push/live, warm and cold functions, control operations, recovery, concurrent load, and saturation; verify zero acknowledged loss/integrity failures and at least 30% capacity headroom at the target load.
- [ ] 7.3 Start and complete the 30-day resource, capacity, backup, availability, workload, egress, allocated-host, power, network, storage, and operational cost window, attaching provider or facility evidence where available and leaving unproven values null.
- [x] 7.4 Update the release-gate evidence and validators for the exact VM, plan hash, release/runtime digests, storage, HTTPS route, measurement window, recovery drill, and costs without altering thresholds or inferring a pass from deployment existence.
- [x] 7.5 Keep qualified public-beta admission disabled while any beta observation is missing, failing, or blocked; after every threshold passes, record operator approval and the candidate digest before enabling qualified rate-limited public beta ingress.

## 8. Validation and Documentation

- [x] 8.1 Add CI checks for provisioning-plan schema, collision fixtures, cloud-init and Ansible lint, firewall policy, Caddy route exposure, systemd hardening, secret scanning, release manifests, and retained beta evidence without contacting or mutating Proxmox.
- [x] 8.2 Add a safe local/dry-run validation command that verifies every infrastructure and guest asset and clearly separates read-only plan, external apply, admission enablement, and destructive teardown operations.
- [x] 8.3 Update deployment, configuration, release-gate, rollback, security, and operations documentation with the tested public-beta flow, exact public origin, evidence locations, single-VM limitations, and certificate-renewal procedure.
- [x] 8.4 Run strict OpenSpec validation and the full repository format, lint, type, unit, integration, API-generation, documentation, security, rollback, production-release, and infrastructure validation suites; record any environment-dependent follow-up as a remaining blocker rather than marking it complete.

## 9. Risk-Accepted Public Preview

- [x] 9.1 Implement a non-secret public-preview approval schema and command that validate operator identity, exact plan and release digests, the complete active blocker set and deterministic digest, acceptance and maximum-14-day expiry times, non-waivable safeguard evidence, and a typed confirmation bound to those values.
- [x] 9.2 Add the `risk_accepted_preview` admission state to deployment validation and render separate validated Caddy configurations for disabled, pre-gate, preview, and qualified-beta modes with atomic selection and unchanged HTTPS, route, authentication, policy, quota, rate-limit, audit, and private-listener controls.
- [x] 9.3 Add a systemd admission guard and at-least-once-per-minute timer that compare the active preview approval with the selected release, plan, blocker digest, certificate/HSTS state, service readiness, backup/recovery and zero-loss/integrity evidence, and emergency-stop readiness, atomically falling back to `pre_gate` with sanitized evidence on any expiry, mismatch, or safety failure.
- [x] 9.4 Update release-gate evidence, validators, deployment documentation, and runbooks to label preview as `risk-accepted-public-preview`, retain its accepted blockers, preserve the blocked qualified-beta result and unchanged thresholds, document reapproval and rollback, and distinguish preview approval from beta approval.
- [x] 9.5 Add fixture and integration coverage for valid approval, overlong or expired approval, typed-confirmation mismatch, release/plan/blocker drift, every non-waivable safeguard failure, atomic pre-gate fallback, public route security, internal-port isolation, and the independent emergency admission stop.
- [x] 9.6 Build and deploy the exact preview-capable release in pre-gate mode, record the operator's digest-bound and time-bounded approval, enable risk-accepted public preview, verify HTTPS access from a previously unauthorized external source while application security and internal isolation remain enforced, exercise fail-closed fallback and reapproval, and retain sanitized deployment evidence.
