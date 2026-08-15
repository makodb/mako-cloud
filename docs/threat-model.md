# Initial threat model

Version 1, reviewed 2026-08-05. This document covers the Mako Cloud MVP described by the active OpenSpec change. Security, data-platform, identity, and edge-runtime owners review it before each production release and after a material architecture or trust-boundary change. The machine-checked registry is `security/threat-model.json`.

## Scope and assumptions

In scope are the browser/Node RxDB client, public and internal APIs, project auth, document policy evaluation, the document and replication engines, the control plane, operators, the ordered transactional KV boundary, infrastructure dependencies, edge-function deployment/execution, CI, and release artifacts.

Hosted developer registration adds an untrusted signup/mail boundary and a
distinct wait-list authority. Verification and recovery tokens are
single-purpose digests, pending access uses a separate audience, protected
requests check current persistent lifecycle and epoch, operator review uses a
separate permission and atomic audited decisions, and verified-TLS mail leaves
an encrypted bounded outbox. Applicant identifiers, source addresses, tokens,
passwords, and review reasons are excluded from metric labels and ordinary
logs.

Cloud and persistent-volume providers are assumed to enforce their documented physical and account isolation. Neither storage engine is trusted merely because it opens. Control-plane SQLite must prove private path ownership, exclusive locking, database identity, schema and integrity, capacity, synchronous durability, and the required adapter semantics. Tenant RocksDB must continue to prove its existing ownership and semantic readiness independently. End-user devices, networks, tenant-authored documents, queries, policies, schemas, and function code are untrusted. A valid identity does not imply authorization to another project, environment, document, management action, or function secret.

The control plane is the sole owner of the local SQLite file and its WAL, lock, migration, restore, and backup staging paths. SQLite stores developer and operator identity lifecycle, organizations and projects, provisioning, functions metadata, audit, idempotency, incidents, and mail outbox state. Application users, project credentials and signing keys, documents, policies, indexes, change history, and RxDB replication state remain in tenant RocksDB. The browser is presentation only: it creates no SQLite, IndexedDB, Dexie, RxDB, or local-storage authority.

Availability against provider-wide catastrophe and malicious cloud-provider administrators is outside the MVP guarantee. These remain deployment risks to address with regional recovery, provider controls, and contractual assurance; they do not weaken tenant isolation or fail-closed requirements.

## Security invariants

1. Every storage key and internal call carries a validated project and environment scope; caller-controlled bytes cannot escape their encoded key range.
2. The same active document policy governs direct reads/writes, queries, indexes, pull, live streams, push conflict responses, trusted user-context calls, and support impersonation. Denial is the default.
3. A storage adapter cannot serve traffic until it proves every semantic capability required by the document engine.
4. Secret values, password material, protected document bodies, and unauthorized existence/count information never enter public errors, diagnostics, logs, traces, metrics labels, or audit metadata.
5. Tenant function code runs in a project-bound isolate with bounded resources and explicit secrets, data, and egress capabilities. Cross-invocation state is absent or proven clean.
6. Privileged and security-sensitive actions create append-only actor, target, reason, result, request, and correlation evidence.
7. Missing scope, ambiguous identity, unavailable policy state, configuration drift, unhealthy regional placement, and unsupported runtime/storage capabilities fail closed.

## Trust boundaries

| ID | Boundary | Required controls at entry |
|---|---|---|
| `TB-01` | Untrusted clients and the internet → public gateways | TLS, request/schema limits, authentication where required, generic auth errors, rate limits |
| `TB-02` | Gateway → control and data planes | Authenticated workload identity, explicit project/environment scope, route-level least privilege |
| `TB-03` | Data services → ordered transactional KV | Capability handshake, encoded tenant prefixes, bounded scans, transaction and durability conformance |
| `TB-04` | Services → SMTP, object storage, telemetry, and other dependencies | Scoped credentials, configured endpoints, deadlines, output redaction, no implicit egress |
| `TB-05` | Edge supervisor → untrusted tenant function isolate | Fresh/clean isolate, capability injection, resource ceilings, default-deny network policy |
| `TB-06` | Organization members/support operators → management and operator paths | Organization RBAC, step-up/JIT access, case and reason binding, immutable audit, visible impersonation |
| `TB-07` | Source/dependencies → release artifact | Protected review, locked/audited dependencies, isolated release identity, provenance/signature verification |
| `TB-08` | Control-plane desired state → regional runtime state | Versioned configuration, idempotent reconciliation, authorized promotion/rollback, drift detection |
| `TB-09` | Developer console → capability-authenticated explorer data routes | Short-lived signed scope/mode/operation grant, current epoch/nonce, policy preview or audited administrative authorization |
| `TB-10` | Control plane and offline recovery tools → local SQLite authority | Exclusive lock and identity, private service-owned paths, authenticated manifests, schema/integrity/capacity gates |

No internal network position is itself trusted. Crossing a boundary requires both identity and authorization for the exact operation and scope.

## Privileged identities

| ID | Identity | Permitted privilege and constraint |
|---|---|---|
| `PI-01` | Provisioning controller | Reconcile only the project/environment named by its durable step; no arbitrary document reads |
| `PI-02` | Public gateway service | Authenticate and route; cannot bypass policy evaluation or mint management grants |
| `PI-03` | Data-plane service | Access assigned environment key ranges; cannot mint operator or release credentials |
| `PI-04` | Control-plane service | Mutate management state; cannot directly read customer document bodies |
| `PI-05` | Edge runtime supervisor | Create constrained isolates and inject only the selected project/function capabilities |
| `PI-06` | Break-glass support operator | Approved, time/case-bound, phishing-resistant MFA, enhanced audit, no silent impersonation |
| `PI-07` | Release automation | Publish signed artifacts from protected commits; no production data access |
| `PI-08` | Explorer administrative capability | One tenant collection, explicit operations and reason hash, current developer authority, maximum five-minute lifetime |
| `PI-09` | Control storage migration/recovery operator | Inspect fenced artifacts and publish only verified empty SQLite migration/restore targets using root-readable authentication material and explicit offline confirmation |

Service identities use short-lived workload credentials. Storage, object, mail, telemetry, and secret-store permissions are split by service role. Human shared accounts and permanent support grants are prohibited. Emergency access expires automatically and triggers review.

## Sensitive data classes

| ID | Data | Handling rule |
|---|---|---|
| `SD-01` | Credentials, signing keys, API keys, function secrets | Reference instead of embedding; envelope encryption; redact; rotate/revoke |
| `SD-02` | Passwords, sessions, tokens, identity links | Argon2id; hashed refresh state; audience/project binding; minimal retention |
| `SD-03` | Documents, revisions, indexes, change records, tombstones | Tenant key isolation; uniform policy checks; retention compatible with supported offline clients |
| `SD-04` | Schemas, policies, quotas, and project configuration | Validate, version, authorize, atomically activate, retain rollback/audit history |
| `SD-05` | Audit and security events | Append-only and integrity protected; restrict reads; never include document bodies/secrets |
| `SD-06` | Function source, bundles, deployment state, invocation logs | Digest-bound artifacts; isolated execution; redacted and bounded logs |
| `SD-07` | Membership, billing, support, and case records | Management RBAC; purpose limitation; operator-access audit |
| `SD-08` | Metrics, traces, IPs, request and operational metadata | Bounded labels; pseudonymous IDs; short retention; no payload or secret values |
| `SD-09` | Operator incidents, recovery jobs, projections, and activity exports | Metadata-only; reason/case bound; integrity protected; expiring exports; no document bodies or secrets |
| `SD-10` | Explorer grants, data jobs, import uploads, export artifacts | Tenant/digest bound, encrypted, expiring, bounded, and absent from browser persistence |
| `SD-11` | SQLite control database, migration receipts, backup manifests | Private files, authenticated manifests, byte-exact inventories, integrity checks, no values or customer identifiers in evidence |

Encryption at rest and in transit is required in hosted environments. Backup retention must not silently extend revoked-secret availability or tombstone guarantees. Destructive project lifecycle operations are explicit workflows with retention and recovery state.

## Abuse cases and control ownership

The detailed prevention, detection, response, boundary, identity, asset, and verification mappings are normative in the JSON registry. This table is the human review index.

| ID | Abuse case | Primary owner | Expected secure outcome |
|---|---|---|---|
| `AC-01` | Cross-tenant point/range access | data-platform | Reject before storage access; record scope mismatch without target data |
| `AC-02` | Policy bypass through sync, conflicts, indexes, or diagnostics | security | Uniform decision path; never return protected body/existence/count |
| `AC-03` | Credential stuffing and account enumeration | identity | Generic response plus bounded, observable throttling |
| `AC-04` | Session replay or token theft | identity | Detect refresh reuse, revoke family, preserve project/audience isolation |
| `AC-05` | Management/support privilege escalation | security | Deny outside organization role or JIT case grant; audit every attempt |
| `AC-06` | Edge isolate escape or cross-project reuse | edge-runtime | Terminate isolate/node, rotate exposed capabilities, preserve other tenants |
| `AC-07` | SSRF or unauthorized edge egress | edge-runtime | Default-deny destination, revalidate DNS/redirects, block provider metadata |
| `AC-08` | Resource exhaustion through API, sync, query, or functions | platform | Bound offender and shed its work without noisy-neighbor propagation |
| `AC-09` | Storage adapter misrepresents semantics | data-platform | Fail readiness and block release/writes until conformance passes |
| `AC-10` | Replay, rollback, or partial activation of security configuration | control-plane | Preserve monotonic, atomic, authorized version state and repair workflow |
| `AC-11` | Audit deletion, forgery, or payload injection | security | Detect continuity/sink failure, preserve evidence, prevent sensitive fields |
| `AC-12` | Compromised dependency or tampered release | release-engineering | Block unverified digest, revoke signer, rebuild trusted provenance |
| `AC-13` | Secret/document leakage through errors or telemetry | security | Safe error schema and redaction; rotate and contain on canary detection |
| `AC-14` | Stale offline client resurrects deleted/unauthorized state | sync | Reject stale write or demand full resync without leaking conflict state |
| `AC-15` | Duplicate delivery repeats writes or side effects | data-platform | Atomically return stored scoped idempotency outcome |
| `AC-16` | Auth email abuse and redirect phishing | identity | Allowlisted redirect, single-use expiry, quota, generic response |
| `AC-17` | Global operator inventory leaks data or triggers unbounded work | control-plane | Metadata-only bounded scans, signed scoped cursors, redaction, durable denied audit |
| `AC-18` | Stale or forged operator action targets changed state | security | Reject stale version/action binding and require fresh password verification/review |
| `AC-19` | Unsafe observability deep link exfiltrates credentials | platform | Retain only allowlisted HTTPS origins and safe bounded identifiers |
| `AC-20` | Recovery restores or promotes unverified state | data-platform | Independent gates, verified evidence, durable state, successful verification before promotion |
| `AC-21` | Entitlement change removes last recoverable administrator | identity | Reject unsafe changes and preserve controlled bootstrap recovery |
| `AC-22` | Activity export/projection leaks data or hides integrity gaps | security | Distinct permission, bounded redacted expiry, count/checksum and gap evidence |
| `AC-23` | Forged, replayed, stale, or mode-confused explorer grant | security | Reject on signature, authority, nonce, epoch, scope, operation, mode, or lifetime mismatch |
| `AC-24` | Browse/query leaks hidden rows, tombstones, counts, or cursor state | data-platform | Snapshot and signed cursor binding, policy-fill paging, history permission, no scan fallback |
| `AC-25` | Explorer mutation bypasses document invariants | data-platform | Preview never commits; admin reuses conditional sequenced mutation path and enhanced audit |
| `AC-26` | Import/export artifact crosses scope or exposes partial output | data-platform | Tenant-bound digest grants, dry run, quotas, durable progress, finalized artifacts only |
| `AC-27` | Connect/sync/recovery diagnostics expose secrets or expand authority | platform | Non-document probes, bounded aggregates, safe manifests, isolated stepped-up restore only |
| `AC-28` | Control SQLite file or backup theft | security | Private paths and authenticated off-VM artifacts; contain host and rotate credentials on exposure |
| `AC-29` | Malicious/corrupt SQLite schema becomes authoritative | data-platform | Reject identity/schema/integrity mismatch and restore only into an empty verified target |
| `AC-30` | Migration substitution or partial copy | release-engineering | Fence the source and require release-bound, byte-exact inventory/checksum proof before atomic publication |
| `AC-31` | Rollback resurrects obsolete RocksDB control state | release-engineering | Refuse pre-SQLite binaries after SQLite writes without a separately verified reverse migration |
| `AC-32` | Browser persistence is treated as portal authority | security | No browser database authority; revalidate roles and sessions at the SQLite-backed server |
| `AC-33` | Tenant RocksDB outage is misdiagnosed or bypassed | platform | Keep SQLite-backed control routes available and fail only tenant-data operations with scoped errors |

### Developer data explorer boundary

Explorer capabilities cross `TB-09` and live only in browser memory. The control plane revalidates
active developer status, organization membership and data permission, active project/environment,
active collection, requested mode, and any selected active application user. The data plane accepts
only the signed `mako-control-plane` to `mako-data-plane-explorer` audience, then rechecks the
persistent nonce and current authorization epoch for the exact tenant, collection, mode, and
operation. Grants expire within five minutes and cannot be exchanged for project credentials.

Policy preview uses the selected application's current trusted claims and active policy and cannot
commit. Administrative access requires data-admin permission and a reason hash and establishes a
separate audited privileged authorizer for every operation. Primary-key browse runs inside the
document engine over a stable snapshot, fills through policy-hidden rows, and binds cursors to
tenant, collection, mode, schema, query, snapshot, epoch, and expiry. Indexed queries never fall
back to collection scans. History, import, export, artifact access, backup inventory, and isolated
restore each require separate permissions and bounded contracts. Audit and telemetry contain
identifiers, fingerprints, index names, outcomes, and reason hashes, never capabilities, raw
reasons, email addresses, document bodies, selectors, or artifact contents.

### Operator control-center boundary

The operator control center is a metadata-only administrative surface. Global reads use signed,
scope-bound cursors and bounded storage scans. `tenant_read` remains a compatibility grant for new
read-only views, while incident management, recovery, security administration, and activity export
use separate permissions. Missing or stale provider data is visibly `unknown`, `stale`, or
`unavailable`; absence is never interpreted as healthy.

High-impact workflows require a password verification no older than five minutes plus an action
binding over the action, target, and reviewed resource version. Requests also carry an idempotency
key, explicit confirmation, reason, and optional case reference. Recovery creation and promotion
have independent feature gates. Promotion requires verified backup evidence and successful
post-restore verification; provider and executor interfaces do not accept arbitrary commands or
paths.

Routine operator routes cannot return customer document bodies. Diagnostic links must use a
configured HTTPS origin allowlist and safe identifiers. Activity exports require a distinct
permission, preserve their filter digest and integrity checksum, are bounded by the retained
projection, and expire after one hour.

### Control SQLite boundary and correlated failures

The control-plane process crosses `TB-10` through one vendor-neutral adapter. A fixed application
ID, configured database identity, supported schema, exclusive process lock, `trusted_schema=OFF`,
integrity probe, capacity reserve, WAL limit, and synchronous writes gate authority. Database and
lock paths are normalized and may not be symlinks or overlap any RocksDB, migration, backup,
restore, or reserve path. SQLite has no listener and is never proxied by Caddy. Migration and
recovery tools operate offline, create only new targets, authenticate their manifests, and publish
through an atomic rename after inventory and integrity verification.

File permissions reduce but do not eliminate `AC-28`: a host-root or disk-snapshot compromise can
copy the database and password hashes. Detection therefore includes permission drift, artifact
verification, and canary scanning; response contains the VM, rotates affected credentials and
sessions, and restores only from a trusted artifact. `AC-29` fails startup on a wrong application
ID, database identity, unsupported schema, failed integrity check, or raw SQLite error; startup
never creates a blank production authority.

For `AC-30`, a stopped RocksDB checkpoint is fenced by digest and bound to an exact migration plan,
release, configuration, paths, identity, and format. Complete-keyspace framed checksums and prefix
inventories must match the temporary SQLite target before fsync and atomic publication. A protected
receipt makes retries idempotent and rejects source drift or an existing ambiguous target. For
`AC-31`, once SQLite accepts a post-cutover write, release selection requires declared support for
its format. The old checkpoint remains evidence, not an active fallback.

`AC-32` is enforced by static console tests and session tests: operator authentication remains in an
HttpOnly cookie, the developer token remains short-lived in session storage, and no browser
database or durable local authority is introduced. During `AC-33`, control readiness and
SQLite-backed authentication, wait-list, audit, incident, organization, and project metadata remain
available. Tenant reads, mutations, credentials, application-user administration, replication, and
recovery actions continue to require the data plane and fail closed with an explicit unavailable
provider rather than treating missing data as empty or healthy.

## Verification and release gates

`crates/mako-audit/tests/threat_model.rs` is the first security control test. It fails if the registry loses required categories, has duplicate or malformed IDs, omits control ownership/prevention/detection/response/verification, references an unknown boundary/identity/data class, or drifts out of this review document. The verification IDs in each abuse case become executable tests as their owning subsystem is implemented; a mapped test may not be deleted without replacing the mapping and reviewing the residual risk.

Production release gates require:

- formatting/lint and Rust/Node lockfile audits;
- unit, integration, cross-tenant, redaction, storage-conformance, policy-path, auth-abuse, edge-isolation/egress, and recovery tests relevant to the release;
- no unsupported storage/runtime capability;
- reviewed migration, rollback, key rotation, tombstone retention, and incident-response impact;
- signed artifacts and provenance tied to the reviewed commit.

Security regressions are release blocking. A flaky security test is treated as an unresolved control failure, not skipped indefinitely.

## Detection and response

Security events use bounded schemas with project/environment, actor class, operation, decision, policy/config version, request/correlation ID, and reason code—never secret or document payloads. Alerts cover cross-scope attempts, authorization anomalies, auth spraying/reuse, support grants/impersonation, sandbox/egress violations, audit gaps, configuration divergence, storage conformance failures, unusual tenant resource saturation, secret canaries, and release verification failures.

Response favors containment that does not damage unrelated tenants: deny the request, revoke the narrow identity/token family/capability, disable the affected route/function/version, quarantine a runtime node, or stop writes when consistency is uncertain. Evidence is preserved in the protected audit sink. Customer notification, credential rotation, repair, and retrospective scope follow the incident classification.

### Public-beta ingress boundary

For `cloud-test.makodb.com`, the default and current security posture is no
public application listener. Caddy is disabled on VM `124`, and an active,
boot-persistent Proxmox bridge rule drops destination TCP 80/443 independently
of the guest. Management SSH is key-only and limited to discovered operator
addresses; all Mako, RocksDB, rootless dependency, Prometheus, and Grafana
listeners are private or loopback-only. The admission stop retains the VM,
disks, backups, releases, logs, and evidence so containment does not become an
unreviewed destructive action.

Restricted HTTPS admission, when approved, still crosses `TB-01` and cannot
bypass Mako authentication, document policy, tenant binding, quotas, request
limits, or audit. Only the checked OpenAPI and function route allowlists may be
proxied; health and `/_internal/v1/` routes remain unavailable. No ACME failure
may create a plaintext application fallback. Unrestricted admission requires a
trusted certificate, completed renewal and hosted qualification, a passing
30-day gate, and an explicit approval bound to the selected release digest.

## Residual risk and review triggers

Initial quotas, token lifetimes, retention windows, and edge resource profiles remain tunable and require load/abuse-test evidence before hosted launch. New identity providers, storage adapters, query operators, policy context fields, regions, dependency endpoints, edge runtime APIs, operator capabilities, or data export/import paths trigger threat-model review. So do any incident, control failure, or architectural change that adds a privileged identity, sensitive data class, or boundary.
