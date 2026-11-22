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
- `crates/mako-identity/src/session_store.rs` (2026-11-22), six tests of refresh replay: `a_credential_consumed_two_rotations_ago_revokes_the_family`, `every_earlier_credential_stays_a_replay_however_often_the_family_rotates`, `an_older_consumed_credential_is_a_replay_even_inside_the_grace_window`, `a_credential_no_family_issued_is_invalid_and_revokes_nothing`, `a_made_up_credential_under_a_family_tag_revokes_that_family`, `a_family_rotated_before_tags_existed_is_protected_from_its_next_rotation`. They drive the storage adapter, random credentials and BLAKE3, so they are unlikely to be replaceable.
- `crates/mako-identity/src/refresh.rs` (2026-11-22), three tests of the in-memory family model: `a_credential_consumed_more_than_one_rotation_ago_is_a_replay`, `a_credential_the_family_never_issued_is_invalid_and_revokes_nothing`, `rotation_keeps_the_family_tag_and_renews_the_rest`. The state machine of `RefreshTokenFamily::exchange` (which outcome follows from current, last consumed, tagged, or unknown) is a candidate for a Verus specification taken from the requirement "Rotating refresh sessions", once its random and hashing calls are separated out.
- `crates/mako-storage/src/rocks.rs` (2026-11-22), five tests of the corruption latch: `a_corruption_signal_fails_health_and_readiness_until_restart`, `a_corruption_signal_refuses_every_mutation_and_keeps_reads`, `an_io_signal_alone_neither_fails_readiness_nor_refuses_writes`, `rocksdb_error_kinds_map_to_corruption_and_io`, `a_read_that_meets_a_damaged_block_stops_mutations_and_readiness`. They open RocksDB and damage a file on disk, so they are not replaceable; `classify_rocks_error` is a pure match and a candidate, though it names another crate's type.
- `crates/mako-storage/src/readiness.rs::a_not_ready_report_carries_the_adapters_operator_diagnostic`, `services/mako-data-plane/src/graph.rs::a_storage_corruption_signal_names_the_runbook_in_the_readiness_detail`, the test of the same name in `services/mako-edge-gateway/src/graph.rs`, and `services/mako-telemetry-query/src/main.rs::readiness_passes_on_what_an_unavailable_store_tells_the_operator` (2026-11-22): string formatting over readiness reports; not judged yet.
