## 1. Benchmark gate

- [x] 1.1 Remove the measurement-window, backup-age, recovery-point and cost thresholds and observations from the `single-region-beta` stage in `docs/release-gates.json`. Leave the internal gate unchanged.
- [x] 1.2 Add `scripts/public-beta-benchmark-gate-lib.js`, which computes the gate result and exact blocker list, and change `scripts/validate-release-gates.js` to allow `passed`, to reject a recorded status or blocker list that disagrees with the computation, and to drop the window and cost assertions. Allow a passed stage in `scripts/validate-rollback-qualification.js`.
- [x] 1.3 Change the hosted benchmark harness to collect at least 100 steady-state samples for each gated path, measure pull in steady state, and record per-path sample counts. Report recovery timings separately.

## 2. Admission

- [x] 2.1 In the admission guard, remove the backup-verification and backup-age checks, `backupFreshnessRequired`, and the 30-day and cost waivable patterns. Remove `mako_public_preview_require_fresh_backup` from group vars and the context template.
- [x] 2.2 Update `scripts/public-beta-preview-approval-lib.js` and the guard, approval and validator tests for the new blocker set, including a case proving that a failed backup leaves admission unchanged.

## 3. Cutover and documentation

- [x] 3.1 With the guard timer stopped, install the new guard, the context with the new blocker digest, and a matching preview approval; verify, activate, and confirm the next guard tick keeps public admission open.
- [x] 3.2 Run the hosted benchmark on the deployed release and record the gate result. Release `b42b2bd4` passed on 2026-11-11 (streaming, hosted benchmark with 100 samples per path, and all eight hosted security suites), and the operator approved beta admission.
- [x] 3.3 Update the dev-book: the beta gate, the admission and backup runbooks, and the removal of the test-preview suspension exception.

## 4. Validation

- [x] 4.1 Run `openspec validate benchmark-only-beta-gate --strict`, the gate and preview tests, and the docs, release-gate, rollback and public-beta validators.
