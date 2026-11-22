## 1. Specification

- [x] 1.1 Add the requirement "A corruption signal stops mutations until restart" with its four scenarios to `storage/production-rocksdb`.

## 2. Evidence that the code already meets it

- [x] 2.1 `crates/mako-storage/src/rocks.rs::a_read_that_meets_a_damaged_block_stops_mutations_and_readiness` damages a table file and reads through it.
- [x] 2.2 `crates/mako-storage/src/rocks.rs::a_corruption_signal_refuses_every_mutation_and_keeps_reads` covers refused writes, served reads and the unchanged signal count.
- [x] 2.3 `crates/mako-storage/src/rocks.rs::a_corruption_signal_fails_health_and_readiness_until_restart` covers repeated readiness checks and the restart.
- [x] 2.4 `crates/mako-storage/src/rocks.rs::an_io_signal_alone_neither_fails_readiness_nor_refuses_writes` covers the I/O rule.
- [x] 2.5 The readiness detail is covered by `a_storage_corruption_signal_names_the_runbook_in_the_readiness_detail` in `services/mako-data-plane/src/graph.rs` and `services/mako-edge-gateway/src/graph.rs`, and by `services/mako-telemetry-query/src/main.rs::readiness_passes_on_what_an_unavailable_store_tells_the_operator`.

## 3. Validation

- [x] 3.1 Run `openspec validate state-corruption-latch-rules --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`.
