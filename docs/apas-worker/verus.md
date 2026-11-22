# Verus notes

## Status

Verus is not available on Shuais-MacBook-Air-2.local: `verus --version` reports "command not found", and the project has no verification command yet. The report entry "2026-11-21: Verus is not available on Shuais-MacBook-Air-2.local" asks the person to make it available or to keep tests. Every Verus step waits for that entry: project setup, proofs, replacing tests, and judging the tests below.

The project does not use Verus yet. When it becomes available, the first Verus step is the setup: pin the Verus version and vstd, and write the verification command into the README and `.github/workflows/ci.yml`.

## Trusted functions

None. No proof exists yet.

## Rust tests judged

None yet. Judging waits for Verus to be available on a machine the worker runs on. The Rust test suites are `cargo test --workspace --lib --bins` (inline `#[cfg(test)]` modules) and `cargo test --workspace --tests` (the targets under `crates/*/tests/`).
