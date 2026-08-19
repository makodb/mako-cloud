## Why

A developer holding a management session could not build a working application through the management API. Six independent gaps stopped the path between "create a project" and "an application user replicates a document":

- Projects and environments were created in `provisioning` and nothing ever advanced them, so they never became active.
- A new environment had no JWT signing key and no way to create its first one: rotation replaces an active key and fails when there is none.
- Policy activation wrote only to the control database, so the data plane that serves documents never saw the policy and denied every request by default deny.
- `mako-control-session` minted developer sessions without `credentialEpoch`, which the verifier requires, so every issued token was rejected.
- The data plane's internal-RPC address was compiled in, so a control plane could only ever talk to a data plane on the default port.
- No test covered the path, so none of this was visible.

## What Changes

- Advance queued provisioning workflows on a background pass in the control plane, and reconcile resources whose workflow already completed, so created projects and environments reach `active`.
- Add `POST /v1/projects/{projectId}/environments/{environmentId}/signing-keys/actions/initialize` to create an environment's first JWT signing key, alongside the existing rotation endpoint.
- Propagate policy activation to the owning data plane over internal RPC, under a new `InstallPolicy` identity-admin operation gated by a `ManagePolicies` permission, before the control plane reports the version active.
- Emit `credentialEpoch` in `mako-control-session` tokens.
- Make the control plane's data-plane address configurable through `MAKO_DATA_PLANE_ENDPOINT`, validated loopback-only.
- Add an end-to-end test that drives the whole path against the real service binaries.

## Capabilities

### Modified Capabilities

- `cloud/control-plane`: Require created projects and environments to reach a usable state, require an environment to be able to obtain its first signing key, and require policy activation to be visible to the data plane that enforces it.

## Impact

- `services/mako-control-plane`, `services/mako-data-plane`, `crates/mako-internal-rpc`, `crates/mako-config`
- `api/openapi/mako-cloud-v1.yaml`, `packages/api-types`, `packages/management-sdk`
- `crates/mako-smoke` — new `sample_app` end-to-end test
- No breaking wire change: the new endpoint and configuration key are additive, and `MAKO_DATA_PLANE_ENDPOINT` defaults to the address that was previously compiled in.
