# Verus notes

## Status

Verus is not available on Shuais-MacBook-Air-2.local: `verus --version` reports "command not found", and the project has no verification command yet. The report entry "2026-11-21: Verus is not available on Shuais-MacBook-Air-2.local" asks the person to make it available or to keep tests. Every Verus step waits for that entry: project setup, proofs, replacing tests, and judging the tests below.

The project does not use Verus yet. When it becomes available, the first Verus step is the setup: pin the Verus version and vstd, and write the verification command into the README and `.github/workflows/ci.yml`.

## Trusted functions

None. No proof exists yet.

## Rust tests judged

None yet. Judging waits for Verus to be available on a machine the worker runs on. The Rust test suites are `cargo test --workspace --lib --bins` (inline `#[cfg(test)]` modules) and `cargo test --workspace --tests` (the targets under `crates/*/tests/`).

## Rust tests added while Verus is unavailable

These were written as tests because no proof could be verified here. Judge them with the rest once Verus is available.

- `services/mako-control-plane/src/management_http.rs::a_closed_period_and_the_live_bill_report_the_same_balance` (2026-11-21): builds HTTP responses and reads JSON, so it is unlikely to be replaceable.
- `services/mako-control-plane/src/management_http.rs::the_balance_is_credits_minus_closed_and_live_charges_unclamped` (2026-11-21): pure saturating integer arithmetic in `TeamLedger::balance`; a candidate for a Verus specification taken from the requirement "A team has a running balance that may be negative".

