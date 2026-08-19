## Context

The control plane owns projects, environments, collections, credentials, and policies in SQLite. The data plane serves documents from RocksDB. Everything a developer creates through the management API has to cross that boundary over `mako-internal-rpc` before an application can use it. `separate-control-plane-sqlite` established the split and the collection-propagation rule; this change completes the remaining crossings and the lifecycle steps around them.

## Goals / Non-Goals

- **Goal:** a developer with a management session can create a project and reach a data plane that serves documents for it, using only documented API calls.
- **Goal:** every step is covered by a test that runs against the real binaries rather than a mock.
- **Non-Goal:** changing how provisioning workflows are modelled, or making the internal-RPC transport reachable off-loopback.

## Decisions

### Provisioning advances on a background pass, not inline

Project and environment creation enqueue a provisioning workflow and report an asynchronous state; the API contract already says so. Running the workflow inline would make creation latency unbounded and would not survive a restart mid-workflow. A worker thread in the control plane advances queued and running workflows instead.

The worker polls rather than being signalled because control storage is a single-writer SQLite database shared with request handling and the readiness probe; a tight loop starves them. The cadence is slow enough not to contend for the write lock and fast enough that creation converges in seconds.

The pass also reconciles resources whose workflow already reached `Active` but whose record is still `Provisioning`. Without that, a workflow that completed while the resource update failed leaves the resource stuck forever, which is exactly what happened before this change.

### Policy propagation mirrors collection propagation

`InstallPolicy` is added to `IdentityAdminOperation` under a new `ManagePolicies` permission granted to Owner and Administrator, matching how `InstallCollection` works. The control plane activates locally, then propagates, then advances the tenant's authorization epoch.

The data plane records a version as a draft and activates it as a separate step, but the policy arrives already active because the control plane activated it first. The propagation handler normalises the received policy back to a draft before recording it, then activates that version. The alternative — propagating before the control plane activates — would leave the data plane enforcing a policy the control plane had not committed to.

### The data-plane address becomes configuration

It was a compiled-in `127.0.0.1:8080`. It is now `dependencies.data_plane_address` / `MAKO_DATA_PLANE_ENDPOINT`, defaulting to the same value and rejected at startup if it is not loopback for the control plane — the same rule the runtime supervisor and telemetry query addresses already carry. Internal RPC carries no network authentication beyond the shared secret, so a non-loopback address is a configuration error rather than a deployment option.

## Risks / Trade-offs

- **The provisioning worker adds a second writer to control SQLite.** Mitigated by a slow poll and by the storage layer's bounded transactions; the readiness probe and request handling continue to gate serving.
- **Policy propagation can fail after local activation.** The activation call then fails with a conflict and the control plane's local state is ahead of the data plane. This matches the existing collection behaviour, and re-activating the same version converges because the data plane skips a version it already holds.

## Migration Plan

Additive. Existing deployments keep the previous data-plane address by default, and environments with an active signing key are unaffected by the initialize endpoint.
