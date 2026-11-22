## Why

`functions/scheduled-functions` says each scheduled invocation is recorded and
"retained like function metrics". Function metrics are telemetry, which the
User Book retains for ninety days on production deployments. The same book
gives schedule run history a window of its own: seven days and never more than
1 000 runs per schedule, a queued or executing run is never pruned, and
deleting the schedule deletes its history. The spec contradicts both numbers.
The code follows the User Book (`RUN_RETENTION_SECONDS`,
`MAXIMUM_RUNS_PER_SCHEDULE` in `crates/mako-control-plane/src/function_schedule.rs`).

## What Changes

- The requirement "Invocation history and overlap control" states the run
  history bounds: seven days, at most 1 000 runs per schedule, never a queued
  or executing run, and removal together with the schedule.
- One scenario is added for pruning, with its traceability row (SF-04).

No code changes: the implementation already behaves this way and an existing
test covers the new scenario.

## Capabilities

### New Capabilities

### Modified Capabilities

- `functions/scheduled-functions`: run history has its own retention window
  and ceiling instead of following function metrics.

## Impact

- `openspec/specs/functions/scheduled-functions/spec.md`
- `docs/requirements-traceability.md` (new row SF-04)
