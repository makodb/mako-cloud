## 1. Specification

- [x] 1.1 Rewrite the requirement "Invocation history and overlap control" in `functions/scheduled-functions` with the seven-day window, the 1 000-run ceiling, the protection of queued and executing runs, and removal with the schedule; add the scenario "Run history is pruned".

## 2. Evidence that the code already meets it

- [x] 2.1 Confirm `RUN_RETENTION_SECONDS` is seven days and `MAXIMUM_RUNS_PER_SCHEDULE` is 1 000 in `crates/mako-control-plane/src/function_schedule.rs`, and that `retention_prunes_old_and_excess_runs_but_never_a_queued_one` covers the ceiling, the window and the kept queued run.

## 3. Traceability and validation

- [x] 3.1 Add traceability row SF-04 for the new scenario in `docs/requirements-traceability.md`.
- [x] 3.2 Run `openspec validate bound-schedule-run-history --strict --no-interactive`, `npm run validate:traceability`, `npm run validate:docs`, and the Rust test the new row cites.
