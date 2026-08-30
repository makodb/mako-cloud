# Scheduled functions

Run a deployed function on a cron schedule -- cleanup jobs, reports, syncs
-- without an external cron, with a history developers can read. An
authorized member attaches a *schedule* to a function: a five-field cron
expression evaluated in UTC and the request each due time sends. A
control-plane worker fires what has fallen due **through the edge gateway**,
so quotas, metering, function metrics, logs, and audit apply to a scheduled
run exactly as to any other invocation, and records every run.

A schedule targets the function's **active deployment** only: attaching one
to a function without an active deployment is refused with `409 conflict`,
and each run invokes whatever deployment is active when it starts.

## Attaching a schedule

`POST /v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/schedules`
with an idempotency key:

```json
{
  "name": "Nightly report",
  "cron": "0 3 * * *",
  "request": {
    "method": "POST",
    "path": "/reports?kind=daily",
    "headers": { "x-report": "nightly" },
    "contentType": "application/json",
    "body": "{\"day\":\"today\"}"
  },
  "enabled": true
}
```

- `cron` is required; everything else has a default (`name` empty,
  `request` a body-less `POST /`, `enabled` true).
- The response is `201` with the schedule, including `nextRunAt`.

```json
{
  "id": "sch_k3m9x2p7q4w8r5t1",
  "functionName": "nightly-report",
  "name": "Nightly report",
  "cron": "0 3 * * *",
  "timezone": "UTC",
  "request": { "method": "POST", "path": "/reports?kind=daily", "headers": { "x-report": "nightly" }, "contentType": "application/json", "body": "{\"day\":\"today\"}" },
  "enabled": true,
  "state": "active",
  "nextRunAt": "2026-08-30T03:00:00Z",
  "lastRun": null,
  "createdAt": "2026-08-29T10:00:00Z",
  "updatedAt": "2026-08-29T10:00:00Z"
}
```

`GET …/schedules` lists a function's schedules; `GET`, `PATCH`, and
`DELETE …/schedules/{scheduleId}` read, change, and remove one. A function
carries at most 100 schedules. Any member of the team may read; a role that
can change projects (developer, administrator, owner) may write. Every
mutation is audited as `function_schedule_create`,
`function_schedule_update`, `function_schedule_delete`, or
`function_schedule_run_now`; a refused read or write is audited as denied.

`PATCH` accepts any subset of `name`, `cron`, `request`, and `enabled`;
fields omitted keep their values, and an update that changes nothing is
refused. A changed `cron` recomputes `nextRunAt` from now. `enabled: false`
**pauses** the schedule: `state` becomes `paused`, `nextRunAt` becomes
`null`, and nothing fires until it is re-enabled, which requires an active
deployment again and recomputes `nextRunAt` from now.

`DELETE` removes the schedule and its whole run history.

## Cron syntax

Five whitespace-separated fields, **evaluated in UTC**:

| Field | Values | Names |
| --- | --- | --- |
| minute | `0`–`59` | |
| hour | `0`–`23` | |
| day of month | `1`–`31` | |
| month | `1`–`12` | `jan`–`dec` |
| day of week | `0`–`6` (Sunday is `0`), `7` also Sunday | `sun`–`sat` |

Each field is `*`, a value, a range (`1-5`), a list (`1,15,30`), or a step
over `*` or a range (`*/15`, `1-10/2`). Names are case-insensitive
three-letter abbreviations. Examples:

- `0 3 * * *` -- every day at 03:00 UTC
- `*/15 * * * 1-5` -- every fifteen minutes, Monday to Friday
- `0 0 29 2 *` -- midnight on the 29th of February, every leap year
- `30 6 1,15 * *` -- 06:30 on the 1st and 15th of every month

Day of month and day of week follow the classic vixie rule: when **both**
are restricted, a day matches if *either* does (`0 9 1 * mon` is the 1st of
the month *and* every Monday); when either field starts with `*`, both must
match (`0 9 * * 1` is Mondays only).

There is no timezone field and no daylight-saving arithmetic. A schedule
fires at the same UTC instant every day of the year; convert local times
yourself. Anything else -- six fields, `?`, `L`, `W`, `#`, `@daily`,
seconds, years, wrapped ranges such as `22-2` -- is refused at save time
with a message that names the field, for example
`cron expression is invalid at minute: value is out of range`. An
expression that never fires within the next five years (`0 0 30 2 *`) is
refused too.

## The request

`request` describes what each due time sends to the function:

- `method`: `GET`, `POST` (default), `PUT`, `PATCH`, or `DELETE`.
- `path`: a path under the function beginning with `/` (default `/`), at
  most 1024 bytes; a query string is allowed (`/reports?kind=daily`).
  Segments must be non-empty and neither `.` nor `..`, so no trailing
  slash except the root.
- `headers`: at most 16 extra headers, values at most 1024 bytes. Names
  are lowercased. `authorization`, `host`, `content-length`,
  `content-type` (set `contentType` instead), `transfer-encoding`, and any
  `x-mako-*` header are refused: the first because the scheduler is the
  credential, the rest because the platform sets them.
- `contentType`: the body's media type, default `application/json`.
- `body`: the body as text, at most 64 KiB. A `GET` request carries no
  body and is refused with one.

The scheduler sends the request through the edge gateway's own invocation
path, so it is subject to the function's request-size limit, the tenant's
function-invocation quota and rate limit, and the environment's function
metrics and logs. It is **not** counted as a public invocation and needs
no bearer token even when the function has `verifyJwt` on: the scheduler's
hop is authenticated by the platform's internal signature, not by an
application-user session.

## How a scheduled invocation looks to the function

The function receives an ordinary request at its configured path and
method with the configured headers and body, plus:

| Header | Value |
| --- | --- |
| `x-mako-schedule-id` | the schedule (`sch_…`) |
| `x-mako-schedule-run-id` | the run (`run_…`) |
| `x-mako-schedule-due-at` | the due time, RFC 3339 UTC |
| `user-agent` | `mako-cloud-scheduler/1` |
| `content-type` | `contentType`, on every method but `GET` |

There is no caller identity: the invocation is anonymous from the
function's point of view, and the gateway audits it with the actor
`function-scheduler/{scheduleId}/{runId}` rather than an application user.
A function that must behave differently when scheduled should check
`x-mako-schedule-id`.

**Those three headers are the scheduler's alone.** The gateway sets them on
the internal hop the scheduler invokes over, and strips them from every
public request before the function sees them — so a caller who sends
`x-mako-schedule-id` does not become a schedule, and the check above means
what it says.

What it does not mean is that a scheduled function's route is private: the
route is public like any other, and a stranger can still invoke it *without*
the header. A function whose work only the schedule should start must
therefore refuse an invocation that carries no `x-mako-schedule-id` — or,
when the developer also wants to start a run by hand, hold a secret of its
own and require it in a header the schedule is created with (`--header`).
`examples/rational/functions/nightly` does the second.

## When runs happen

The worker runs every five seconds. Each pass:

1. reads every schedule whose `nextRunAt` is at or before now, oldest
   first;
2. for each one, **advances `nextRunAt` to the next due time after now
   before invoking**, so a run that outlives its interval never re-fires
   its own due time;
3. invokes the function through the gateway, waiting at most **60
   seconds** for the response, and records the run;
4. prunes each schedule's history (below).

Invocations in one pass run one after another. Up to 20 due schedules are
started per pass; the rest are picked up by the next. The five-second
cadence is the floor on how promptly a due time is noticed, not a bound on
how long a pass takes: a slow function delays the other schedules due in
the same pass.

A schedule's run is recorded with:

- `dueAt` -- the cron due time (or, for run-now, when it was requested);
- `startedAt`, `completedAt`, `durationMilliseconds` -- as measured by the
  worker;
- `functionVersion` -- the deployment version that served, when one did;
- `outcome` -- `succeeded` for a 2xx response, `failed` for any other
  status, `error` when the invocation could not complete, or
  `skipped_overlap` (below); `null` while the run is queued or executing;
- `responseStatus` -- the function's status, for `succeeded` and `failed`;
- `error` -- a stable reason for `error` runs: `timeout`,
  `no_active_deployment` (the function has none, or no longer exists),
  `gateway_unavailable` (the gateway or its dependencies were down),
  `invalid_request` (the gateway refused the request's shape), or
  `throttled` (the tenant's invocation quota or rate limit refused it);
- `manual` -- true for a run-now run.

The schedule's `lastRun` summarises the most recent run, of either kind.

### Overlap is skipped, never run concurrently

A running invocation holds a per-schedule **lease** until it completes.
The lease expires after the invocation timeout plus a grace period (60 s +
30 s = 90 s), which bounds how long a worker that died mid-run can block
its schedule. A due time that arrives while the lease is held is recorded
as a run with `outcome: "skipped_overlap"`, no `startedAt`, and the due
time it was skipped for; `nextRunAt` advances as usual and the schedule
continues. A `*/1 * * * *` schedule whose function takes ninety seconds
therefore runs every other minute and shows a skip in between, rather than
two invocations at once.

### Missed due times run once

If the worker was down (a restart, a deploy) across one or more due times,
the schedule runs **once**, for the **latest** missed due time, and
`nextRunAt` is computed from now. The earlier missed slots are not
recorded at all: a `* * * * *` schedule that missed a weekend is one run,
not three thousand. A function that must not lose work should read
`x-mako-schedule-due-at` and reconcile from its own last checkpoint rather
than assume every slot ran.

## Run now

`POST …/schedules/{scheduleId}/actions/run-now` with an idempotency key
queues one invocation with the schedule's request outside its cron times
and answers `202` with the queued run (`outcome: null`, `manual: true`,
`dueAt` now). The worker executes it on its next pass, taking the same
lease as a cron run. It does not move `nextRunAt`. It is refused with
`409 conflict` while a run of the schedule is still executing, and, like
creation, when the function has no active deployment. A paused schedule can
still be run now.

## Run history

`GET …/schedules/{scheduleId}/runs` lists a schedule's runs newest first,
with `outcome` (one of the four), `cursor`, and `limit` (1–200, default 50)
query parameters and a `nextCursor` to continue. Every due time is
recorded, including skips.

History is retained like function metrics: **seven days**, and never more
than **1 000 runs** per schedule, oldest first. A queued or executing run is
never pruned. Deleting the schedule deletes its history.

## Operations

- The control plane reaches the gateway at
  `MAKO_EDGE_GATEWAY_ENDPOINT` (`dependencies.edge_gateway_address`, default
  `127.0.0.1:8082`), loopback only; see [configuration](configuration.md).
  The gateway serves the scheduler on the internal route
  `POST /_internal/v1/edge/function-schedule-invoke`, which Caddy never
  exposes.
- Metrics: `mako_function_schedule_runs_total{outcome}` counts completed
  runs by outcome; `mako_function_schedule_worker_failures_total` counts
  passes the worker could not complete (a storage failure -- a gateway that
  is down is an `error` outcome on the run, not a worker failure).
- Records live in the control store under
  `control/function-schedules/v1/…`: schedules per function, a node-wide
  due index, per-schedule leases, and per-schedule run histories.
