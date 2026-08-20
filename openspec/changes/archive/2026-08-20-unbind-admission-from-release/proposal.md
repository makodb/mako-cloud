## Why

Public admission was bound to the exact deployed release digest, so every redeploy closed the public site within sixty seconds and fell back to a source-restricted IP allowlist. Reopening required re-deriving the whole evidence chain — deployment, streaming, benchmark, hosted security, admission stop — and issuing a fresh risk acceptance.

The intent is sound: do not serve the public a release nobody qualified. The effect was not. Shipping any fix cost an outage plus an hour of manual requalification, which is pressure to either not ship fixes or to widen the allowlist — and the allowlist is the control this deployment least wants to depend on, because it silently answers strangers with a broken page rather than a refusal.

This happened three times in one working session, most recently on a deploy that carried fixes for a data plane where document queries could not be answered at all.

## What Changes

- Stop comparing the deployed release digest against the approval in the admission guard, both in the dynamic check and in the approval-to-context comparison. The context file is rendered from the deployed release, so leaving that comparison in place would reimpose the binding by another route.
- Derive a preview approval from evidence that is mutually coherent — every artifact describing one and the same measured release — rather than from evidence that must describe the release currently built.
- Keep the release digest on the approval, the context, and the guard evidence as provenance: which release the safeguards were measured on, and which one is deployed.

Everything else still fails closed: plan hash, blocker set, typed confirmation, all eight non-waivable safeguards, service readiness, TLS and HSTS, the exact public route allowlist, the emergency admission stop, the manual pause, and the stickiness that requires a fresh acceptance after any fall-closed.

## Capabilities

### Modified Capabilities

- `operations/public-beta-environment`: Preview admission is scoped to a plan and a blocker set rather than to a release digest.
- `identity/developer-registration`: A redeploy no longer drifts the preview safety binding.

## Impact

- `infra/ansible/roles/runtime/files/mako-public-preview-admission`, `scripts/public-beta-preview-approval-lib.js`, `scripts/test/public-beta-preview-admission-guard.test.js`
- **What is given up:** the guarantee that the non-waivable safeguards were measured on the exact release now serving the public. An operator accepts residual risk for a deployment, not for a build.
- This is a deliberate reduction in a safety control for a beta that charges nobody and carries no production tenants. It should be revisited before general availability, when the answer is more likely to be fast automated requalification than a permanent unbinding.
