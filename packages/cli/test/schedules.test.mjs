// Cron schedules of a function against the loopback mock: request paths and
// bodies under the function, idempotency keys, the request laid over the
// stored one on update, destructive confirmation, and run history paging.
import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

import {
  apiError,
  authHandler,
  ENVIRONMENT_ID,
  NOW,
  PROJECT_ID,
  runCli,
  signedIn,
  startMockApi,
} from "./harness.mjs";

const FUNCTION = "nightly-report";
const BASE = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const SCHEDULES = `${BASE}/functions/${FUNCTION}/schedules`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID, "--function", FUNCTION];
const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const SCHEDULE = "sch_nightly000001";
const NEXT = "2026-08-07T02:00:00.000Z";

function schedule(overrides = {}) {
  return {
    id: SCHEDULE,
    functionName: FUNCTION,
    name: "nightly",
    cron: "0 2 * * *",
    timezone: "UTC",
    request: { method: "POST", path: "/", headers: {}, contentType: "application/json", body: "{}" },
    enabled: true,
    state: "active",
    nextRunAt: NEXT,
    lastRun: null,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function run(overrides = {}) {
  return {
    id: "run_000000000001",
    scheduleId: SCHEDULE,
    functionName: FUNCTION,
    functionVersion: 3,
    dueAt: NOW,
    startedAt: NOW,
    completedAt: "2026-08-06T12:00:01.000Z",
    durationMilliseconds: 812,
    outcome: "succeeded",
    responseStatus: 200,
    error: null,
    manual: false,
    createdAt: NOW,
    ...overrides,
  };
}

/** Routes keyed by `METHOD /path`; anything else is unhandled (404). */
function router(routes) {
  return (request) => routes[`${request.method} ${request.path}`]?.(request);
}

/** A signed-in profile against a mock serving `routes`; `cli` appends the tenant and function. */
async function tenantCli(t, routes) {
  const api = await startMockApi(authHandler({ fallback: router(routes) }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const cli = (argv, options = {}) => runCli([...argv, ...TENANT], { configDir: directory, ...options });
  return { api, cli, directory };
}

test("schedules list renders next and last run per schedule, --json the response verbatim, and needs --function", async (t) => {
  const items = [
    schedule({
      lastRun: { id: "run_000000000001", dueAt: NOW, outcome: "succeeded", durationMilliseconds: 812, responseStatus: 200 },
    }),
    schedule({
      id: "sch_paused0000001",
      name: "weekly",
      cron: "0 6 * * 1",
      enabled: false,
      state: "paused",
      nextRunAt: null,
      lastRun: { id: "run_000000000002", dueAt: NOW, outcome: "error", durationMilliseconds: null, responseStatus: null },
    }),
  ];
  const { api, cli, directory } = await tenantCli(t, {
    [`GET ${SCHEDULES}`]: () => ({ status: 200, json: { items } }),
  });
  const human = await cli(["schedules", "list"]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /^id\s+name\s+cron\s+state\s+nextRunAt\s+lastRun\n/u);
  assert.match(
    human.stdout,
    /sch_nightly000001\s+nightly\s+0 2 \* \* \*\s+active\s+2026-08-07T02:00:00\.000Z\s+succeeded 200 812 ms at 2026-08-06T12:00:00\.000Z/u,
  );
  assert.match(human.stdout, /sch_paused0000001\s+weekly\s+0 6 \* \* 1\s+paused\s+—\s+error at 2026-08-06T12:00:00\.000Z/u);
  assert.equal(api.find(SCHEDULES, "GET")[0].headers.authorization, BEARER);

  const json = await cli(["schedules", "list", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), items, "--json keeps the last run as a record");

  const noFunction = await runCli(
    ["schedules", "list", "--project", PROJECT_ID, "--env", ENVIRONMENT_ID],
    { configDir: directory },
  );
  assert.equal(noFunction.code, 2);
  assert.match(noFunction.stderr, /--function is required/u);
  assert.equal(api.find(SCHEDULES, "GET").length, 2, "a usage error sends nothing");
});

test("schedules create sends the cron, name, request, and enabled flag with an idempotency key; the body comes from a file or stdin", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`POST ${SCHEDULES}`]: (request) => {
      if (request.body.cron === "every night") {
        return apiError("invalid_request", "cron must have five fields: minute hour day-of-month month day-of-week", 400);
      }
      return { status: 201, json: schedule({ ...request.body, state: request.body.enabled ? "active" : "paused" }) };
    },
  });

  const minimal = await cli(["schedules", "create", "--cron", "0 2 * * *"]);
  assert.equal(minimal.code, 0, minimal.stderr);
  const first = api.find(SCHEDULES, "POST")[0];
  assert.deepEqual(first.body, { cron: "0 2 * * *", enabled: true }, "no request is invented");
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.match(minimal.stdout, /^id\s+sch_nightly000001\n/u);
  assert.match(minimal.stdout, /nextRunAt\s+2026-08-07T02:00:00\.000Z/u);

  const file = join(directory, "body.json");
  await writeFile(file, '{"report":"daily"}\n');
  const full = await cli([
    "schedules", "create",
    "--cron", "*/15 * * * *",
    "--name", "quarter-hourly",
    "--method", "put",
    "--path", "/reports?kind=daily",
    "--header", "x-report=daily",
    "--header", "x-source=cli",
    "--content-type", "application/json",
    "--body", `@${file}`,
    "--disabled",
    "--json",
  ]);
  assert.equal(full.code, 0, full.stderr);
  const second = api.find(SCHEDULES, "POST")[1];
  assert.deepEqual(second.body, {
    cron: "*/15 * * * *",
    name: "quarter-hourly",
    request: {
      method: "PUT",
      path: "/reports?kind=daily",
      contentType: "application/json",
      headers: { "x-report": "daily", "x-source": "cli" },
      body: '{"report":"daily"}\n',
    },
    enabled: false,
  });
  assert.notEqual(second.headers["idempotency-key"], first.headers["idempotency-key"]);
  assert.equal(JSON.parse(full.stdout).state, "paused");

  const piped = await cli(["schedules", "create", "--cron", "0 * * * *", "--body", "-"], { stdin: "from stdin" });
  assert.equal(piped.code, 0, piped.stderr);
  assert.deepEqual(api.find(SCHEDULES, "POST")[2].body.request, {
    method: "POST",
    path: "/",
    contentType: "application/json",
    body: "from stdin",
  });

  const refused = await cli(["schedules", "create", "--cron", "every night"]);
  assert.equal(refused.code, 1);
  assert.match(refused.stderr, /error invalid_request: cron must have five fields/u);
  assert.equal(refused.stdout, "");

  const noCron = await cli(["schedules", "create", "--name", "nightly"]);
  assert.equal(noCron.code, 2);
  assert.match(noCron.stderr, /--cron <expr> is required/u);
  const badMethod = await cli(["schedules", "create", "--cron", "0 2 * * *", "--method", "HEAD"]);
  assert.equal(badMethod.code, 2);
  assert.match(badMethod.stderr, /--method must be one of GET, POST, PUT, PATCH, DELETE/u);
  const badPath = await cli(["schedules", "create", "--cron", "0 2 * * *", "--path", "reports"]);
  assert.equal(badPath.code, 2);
  assert.match(badPath.stderr, /--path must begin with \//u);
  const badHeader = await cli(["schedules", "create", "--cron", "0 2 * * *", "--header", "x-report"]);
  assert.equal(badHeader.code, 2);
  assert.match(badHeader.stderr, /--header must be key=value, got "x-report"/u);
  const reserved = await cli(["schedules", "create", "--cron", "0 2 * * *", "--header", "Authorization=Bearer x"]);
  assert.equal(reserved.code, 2);
  assert.match(reserved.stderr, /--header Authorization is set by the platform; authorization, host, content-length are refused/u);
  assert.equal(api.find(SCHEDULES, "POST").length, 4, "usage errors send nothing");
});

test("schedules get shows the record with its request; a missing schedule exits 4", async (t) => {
  const paused = schedule({
    enabled: false,
    state: "paused",
    nextRunAt: null,
    lastRun: { id: "run_000000000001", dueAt: NOW, outcome: "failed", durationMilliseconds: 40, responseStatus: 500 },
  });
  const { cli } = await tenantCli(t, {
    [`GET ${SCHEDULES}/${SCHEDULE}`]: () => ({ status: 200, json: paused }),
    [`GET ${SCHEDULES}/sch_ghost0000001`]: () => apiError("not_found", "no such schedule", 404),
  });
  const found = await cli(["schedules", "get", SCHEDULE]);
  assert.equal(found.code, 0, found.stderr);
  assert.match(found.stdout, /^id\s+sch_nightly000001\n/u);
  assert.match(found.stdout, /state\s+paused/u);
  assert.match(found.stdout, /nextRunAt\s+—/u);
  assert.match(found.stdout, /"method": "POST"/u);
  assert.match(found.stdout, /"outcome": "failed"/u);

  const json = await cli(["schedules", "get", SCHEDULE, "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), paused);

  const missing = await cli(["schedules", "get", "sch_ghost0000001"]);
  assert.equal(missing.code, 4);
  assert.equal(missing.stdout, "");
  assert.match(missing.stderr, /no such schedule/u);

  const noId = await cli(["schedules", "get"]);
  assert.equal(noId.code, 2);
  assert.match(noId.stderr, /<schedule-id> is required/u);
});

test("schedules update sends only the options given, lays a request change over the stored request, and refuses --enable with --disable", async (t) => {
  const stored = schedule({
    request: {
      method: "PUT",
      path: "/reports",
      headers: { "x-report": "daily" },
      contentType: "text/plain",
      body: "daily",
    },
  });
  const { api, cli } = await tenantCli(t, {
    [`GET ${SCHEDULES}/${SCHEDULE}`]: () => ({ status: 200, json: stored }),
    [`PATCH ${SCHEDULES}/${SCHEDULE}`]: (request) => ({
      status: 200,
      json: schedule({ ...stored, ...request.body, updatedAt: "2026-08-07T12:00:00.000Z" }),
    }),
  });
  const path = `${SCHEDULES}/${SCHEDULE}`;

  const cronOnly = await cli(["schedules", "update", SCHEDULE, "--cron", "30 2 * * *", "--json"]);
  assert.equal(cronOnly.code, 0, cronOnly.stderr);
  const first = api.find(path, "PATCH")[0];
  assert.deepEqual(first.body, { cron: "30 2 * * *" });
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(cronOnly.stdout).cron, "30 2 * * *");
  assert.equal(api.find(path, "GET").length, 0, "no request change, no read");

  const disabled = await cli(["schedules", "update", SCHEDULE, "--disable", "--name", "nightly (paused)"]);
  assert.equal(disabled.code, 0, disabled.stderr);
  assert.deepEqual(api.find(path, "PATCH")[1].body, { name: "nightly (paused)", enabled: false });
  assert.match(disabled.stdout, /^id\s+sch_nightly000001\n/u);

  const rerouted = await cli(["schedules", "update", SCHEDULE, "--enable", "--path", "/reports/v2", "--body", "weekly"]);
  assert.equal(rerouted.code, 0, rerouted.stderr);
  assert.equal(api.find(path, "GET").length, 1, "the stored request is read once");
  assert.deepEqual(api.find(path, "PATCH")[2].body, {
    request: {
      method: "PUT",
      path: "/reports/v2",
      headers: { "x-report": "daily" },
      contentType: "text/plain",
      body: "weekly",
    },
    enabled: true,
  });

  const reheaded = await cli(["schedules", "update", SCHEDULE, "--header", "x-source=cli"]);
  assert.equal(reheaded.code, 0, reheaded.stderr);
  assert.deepEqual(
    api.find(path, "PATCH")[3].body.request.headers,
    { "x-source": "cli" },
    "given headers replace the stored set",
  );

  const nothing = await cli(["schedules", "update", SCHEDULE]);
  assert.equal(nothing.code, 2);
  assert.match(nothing.stderr, /nothing to update/u);
  const both = await cli(["schedules", "update", SCHEDULE, "--enable", "--disable"]);
  assert.equal(both.code, 2);
  assert.match(both.stderr, /--enable and --disable exclude each other/u);
  assert.equal(api.find(path, "PATCH").length, 4, "a refused update sends nothing");
});

test("schedules delete is confirmed and sends the idempotency key; nothing is printed but the notice", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`DELETE ${SCHEDULES}/${SCHEDULE}`]: () => ({ status: 204 }),
  });
  const path = `${SCHEDULES}/${SCHEDULE}`;

  const refused = await cli(["schedules", "delete", SCHEDULE]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to delete schedule sch_nightly000001 without --yes/u);
  assert.equal(refused.stdout, "");
  assert.equal(api.find(path, "DELETE").length, 0, "nothing is sent without confirmation");

  const mistyped = await cli(["schedules", "delete", SCHEDULE], { stdin: "sch_other\n", isTTY: true });
  assert.equal(mistyped.code, 2);
  assert.equal(api.find(path, "DELETE").length, 0);

  const deleted = await cli(["schedules", "delete", SCHEDULE, "--yes"]);
  assert.equal(deleted.code, 0, deleted.stderr);
  const sent = api.find(path, "DELETE")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(sent.text, "");
  assert.equal(deleted.stdout, "");
  assert.match(deleted.stderr, /schedule sch_nightly000001 deleted/u);

  const json = await cli(["schedules", "delete", SCHEDULE, "--yes", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), { id: SCHEDULE, state: "deleted" });

  const typed = await cli(["schedules", "delete", SCHEDULE], { stdin: `${SCHEDULE}\n`, isTTY: true });
  assert.equal(typed.code, 0, typed.stderr);
  assert.equal(api.find(path, "DELETE").length, 3);
});

test("schedules run-now posts the action with an idempotency key and prints the queued manual run; a running schedule is refused", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`POST ${SCHEDULES}/${SCHEDULE}/actions/run-now`]: () => ({
      status: 202,
      json: run({ id: "run_manual0000001", startedAt: null, completedAt: null, durationMilliseconds: null, outcome: null, responseStatus: null, manual: true }),
    }),
    [`POST ${SCHEDULES}/sch_running00001/actions/run-now`]: () =>
      apiError("conflict", "a run of this schedule is still executing", 409),
  });
  const path = `${SCHEDULES}/${SCHEDULE}/actions/run-now`;

  const queued = await cli(["schedules", "run-now", SCHEDULE]);
  assert.equal(queued.code, 0, queued.stderr);
  const sent = api.find(path, "POST")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(sent.text, "");
  assert.match(queued.stdout, /^id\s+run_manual0000001\n/u);
  assert.match(queued.stdout, /manual\s+true/u);
  assert.match(queued.stdout, /outcome\s+—/u);

  const json = await cli(["schedules", "run-now", SCHEDULE, "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.equal(JSON.parse(json.stdout).manual, true);

  const refused = await cli(["schedules", "run-now", "sch_running00001"]);
  assert.equal(refused.code, 5);
  assert.match(refused.stderr, /error conflict: a run of this schedule is still executing/u);
  assert.equal(api.requests.filter((r) => r.method === "POST" && r.path.endsWith("/actions/run-now")).length, 3);
});

test("schedules runs maps --outcome/--limit/--cursor to the query and --all follows the cursor", async (t) => {
  const skipped = run({
    id: "run_000000000002",
    dueAt: "2026-08-06T11:00:00.000Z",
    startedAt: null,
    completedAt: null,
    durationMilliseconds: null,
    outcome: "skipped_overlap",
    responseStatus: null,
    error: "previous_run_still_executing",
  });
  const pages = {
    undefined: { items: [run()], nextCursor: "c1" },
    c1: { items: [skipped], nextCursor: null },
  };
  const { api, cli } = await tenantCli(t, {
    [`GET ${SCHEDULES}/${SCHEDULE}/runs`]: (request) => ({ status: 200, json: pages[String(request.query.cursor)] }),
  });
  const path = `${SCHEDULES}/${SCHEDULE}/runs`;

  const human = await cli(["schedules", "runs", SCHEDULE]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /^id\s+dueAt\s+startedAt\s+ms\s+outcome\s+status\s+error\s+manual\n/u);
  assert.match(
    human.stdout,
    /run_000000000001\s+2026-08-06T12:00:00\.000Z\s+2026-08-06T12:00:00\.000Z\s+812\s+succeeded\s+200\s+—\s+false/u,
  );
  assert.match(human.stdout, /next cursor: c1\n$/u);
  assert.deepEqual(api.find(path, "GET")[0].query, {}, "no query parameter is invented");
  assert.equal(api.find(path, "GET")[0].headers.authorization, BEARER);

  const filtered = await cli(["schedules", "runs", SCHEDULE, "--outcome", "failed", "--limit", "25", "--cursor", "c1", "--json"]);
  assert.equal(filtered.code, 0, filtered.stderr);
  assert.deepEqual(api.find(path, "GET")[1].query, { outcome: "failed", limit: "25", cursor: "c1" });
  assert.deepEqual(JSON.parse(filtered.stdout), pages.c1, "--json is the page verbatim");

  const all = await cli(["schedules", "runs", SCHEDULE, "--outcome", "skipped_overlap", "--all", "--json"]);
  assert.equal(all.code, 0, all.stderr);
  const followed = api.find(path, "GET").slice(2);
  assert.equal(followed.length, 2);
  assert.deepEqual(followed[0].query, { outcome: "skipped_overlap" });
  assert.deepEqual(followed[1].query, { outcome: "skipped_overlap", cursor: "c1" }, "the outcome is kept while following");
  const collected = JSON.parse(all.stdout);
  assert.deepEqual(collected.items.map((item) => item.id), ["run_000000000001", "run_000000000002"]);
  assert.equal(collected.nextCursor, null);

  const allHuman = await cli(["schedules", "runs", SCHEDULE, "--all"]);
  assert.equal(allHuman.code, 0, allHuman.stderr);
  assert.match(allHuman.stdout, /run_000000000002\s+2026-08-06T11:00:00\.000Z\s+—\s+—\s+skipped_overlap\s+—\s+previous_run_still_executing\s+false/u);
  assert.doesNotMatch(allHuman.stdout, /next cursor/u);

  const badOutcome = await cli(["schedules", "runs", SCHEDULE, "--outcome", "lost"]);
  assert.equal(badOutcome.code, 2);
  assert.match(badOutcome.stderr, /--outcome must be one of succeeded, failed, error, skipped_overlap/u);
  const badLimit = await cli(["schedules", "runs", SCHEDULE, "--limit", "201"]);
  assert.equal(badLimit.code, 2);
  assert.match(badLimit.stderr, /--limit must be between 1 and 200/u);
  assert.equal(api.find(path, "GET").length, 6);
});
