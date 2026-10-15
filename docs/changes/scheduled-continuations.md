# Bounded scheduled continuations

A successful scheduled function may return JSON containing
`"makoContinuation": {"delaySeconds": 5}`. The value must be an integer between
5 and 300; the complete response must fit the scheduler's 4 KiB body preview.
This explicitly opts into a durable follow-up under the same schedule, request,
active deployment and overlap lease. No management credential is needed.

Completion, lease release and enqueue share one atomic write. A follow-up survives
control-plane restart; it stops when the schedule is disabled, its request changes,
the next cron slot arrives, or 256 continuations have executed. Malformed responses,
errors and absent requests do not enqueue. The recurring cron remains the recovery
path for process interruption or lost invocation responses. Jobs still need their
own idempotency, leases and durable checkpoints. Public function calls cannot
create scheduler continuations. Existing responses retain their behavior.

Run histories expose follow-ups as automatic runs. Internal optional continuation
metadata is stored with the run. Before rolling back to an older binary that cannot
read that metadata, pause affected schedules and migrate these history records;
application jobs remain authoritative throughout.
