## Why

The Dev Book says public admission "has four explicit values: `disabled`,
`pre_gate`, `risk_accepted_preview`, and `approved_beta`", and that the beta
runs in `approved_beta`, a mode that "has no preview guard, so a failed
safeguard no longer closes admission by itself: readiness, certificate, and
backup problems surface through alerts, and the Proxmox emergency stop
remains authoritative".

`operations/public-beta-environment` does not name those modes anywhere, and
one of its scenarios says the opposite of the Dev Book for the mode the beta
runs in: "WHEN storage, certificate, reverse-proxy, identity, policy, or
gateway readiness becomes unhealthy THEN public application admission fails
closed".

The deployment code follows the Dev Book. Convergence asserts the mode is one
of the four, refuses `pre_gate` without an authorized source, refuses
`approved_beta` without `APPROVE_PUBLIC_BETA:<release digest>`, renders all
four reverse-proxy configurations, and enables the guard timer only in
`risk_accepted_preview`.

## What Changes

- A new requirement, "Public admission has four explicit modes", states what
  each mode serves, what each needs before convergence accepts it, that
  approved beta has no guard, and that the emergency stop is authoritative in
  every mode.
- The scenario "A critical readiness dependency fails" under "Operations and
  teardown are observable and recoverable" says what happens per mode: an
  alert always, a fall back to source-restricted pre-gate admission for a
  risk-accepted preview, and no automatic change in approved beta.

No code changes: the playbook, the reverse-proxy template and the guard
already behave this way.

## Capabilities

### New Capabilities

### Modified Capabilities

- `operations/public-beta-environment`: the four admission modes are
  specified, and a readiness failure no longer claims to close approved beta
  admission.

## Impact

- `openspec/specs/operations/public-beta-environment/spec.md`. This
  capability is not indexed by the traceability matrix, so no rows change.
