## 1. Provisioning

- [x] 1.1 Add a control-plane pass that runs queued and running provisioning workflows and reports how many advanced and how many failed.
- [x] 1.2 Transition project and environment records from `provisioning` to `active` when their workflow is complete, including workflows that were already complete.
- [x] 1.3 Run the pass on a background worker with a cadence that does not contend for the control database write lock, and join it on shutdown.

## 2. Signing key initialization

- [x] 2.1 Add `initializeJwtSigningKey` to the OpenAPI contract and regenerate the checked API types.
- [x] 2.2 Add the control-plane route and map it to an `InitializeSigningKey` identity-admin operation.
- [x] 2.3 Expose the operation on the management SDK.

## 3. Policy propagation

- [x] 3.1 Add `InstallPolicy` to the internal-RPC identity-admin contract with an `InstallPolicyInput` payload.
- [x] 3.2 Add a `ManagePolicies` permission and grant it to Owner and Administrator.
- [x] 3.3 Handle the operation in the data plane: verify scope and version, normalise the received policy to a draft, record it, and activate it against the collection's schema.
- [x] 3.4 Propagate on policy activation in the control plane before advancing the tenant's authorization epoch.

## 4. Session and configuration

- [x] 4.1 Emit `credentialEpoch` in `mako-control-session` and accept `--credential-epoch`.
- [x] 4.2 Add `dependencies.data_plane_address` / `MAKO_DATA_PLANE_ENDPOINT`, validated loopback-only for the control plane, and use it in the control-plane graph.
- [x] 4.3 Document the new configuration key.

## 5. Coverage

- [x] 5.1 Add a harness helper that mints a developer management session for a locally seeded developer.
- [x] 5.2 Add an end-to-end test that creates a project, environment, collection, signing key, public key, and policy through the management API, then signs up an application user and replicates a document through the resulting backend.
- [x] 5.3 Record the new scenarios in the requirements traceability matrix.
