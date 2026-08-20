## 1. Guard

- [x] 1.1 Remove the dynamic comparison between the selected release and the approval.
- [x] 1.2 Narrow the static approval-to-context comparison to the plan hash and blocker digest, so the Ansible-rendered context cannot reimpose the binding.
- [x] 1.3 Keep the release digest on the approval, the context, and the guard evidence as provenance.

## 2. Approval derivation

- [x] 2.1 Derive the measured release from the release gate's observation instead of the current release manifest.
- [x] 2.2 Require every evidence artifact to agree with that measured release.

## 3. Coverage

- [x] 3.1 Remove release drift from the fail-closed drift cases.
- [x] 3.2 Add a case proving a redeploy leaves preview admission open.
- [x] 3.3 Keep every other drift case failing closed.
