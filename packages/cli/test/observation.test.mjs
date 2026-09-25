// Observability, workspace, sync, and backup commands against the loopback
// mock: query mapping, paging, human rendering, exit codes.
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

const ENVIRONMENT = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const OBSERVABILITY = `${ENVIRONMENT}/observability`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const RETENTION = {
  retainedFrom: "2026-05-08T12:00:00.000Z",
  observedAt: NOW,
  retentionSeconds: 90 * 86_400,
};

/** Unix seconds for an ISO instant, as the workspace APIs report time. */
function at(iso) {
  return Math.floor(Date.parse(iso) / 1000);
}

function page(items, nextCursor = null) {
  return { items, nextCursor, retention: RETENTION };
}

function record(timestamp, payload) {
  return { timestamp, payload };
}

function log(timestamp, level, source, message) {
  return record(timestamp, { kind: "project_log", source, level, message, correlationId: "corr_1" });
}

function audit(timestamp, action, outcome, details = null) {
  return record(timestamp, {
    kind: "audit",
    teamId: "org_abcdefgh",
    actorId: "dev_owner",
    action,
    target: `project:${PROJECT_ID}`,
    outcome,
    requestId: "req_audit",
    details,
  });
}

/** A mock that serves one observability path from a map of cursor to page. */
async function observabilityApi(t, path, pages, extra = () => undefined) {
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "GET" && request.path === `${OBSERVABILITY}/${path}`) {
          const found = pages[request.query.cursor ?? ""];
          return found === undefined ? undefined : { status: 200, json: found };
        }
        return extra(request);
      },
    }),
  );
  t.after(() => api.close());
  return api;
}

test("logs maps --since/--limit/--cursor onto the wire and renders lines newest first", async (t) => {
  const older = log("2026-08-06T11:00:00.000Z", "info", "data-plane", "started");
  const newer = log("2026-08-06T11:59:00.000Z", "error", "edge-function:hello", "boom\x1b[31m!");
  const api = await observabilityApi(t, "logs", { cur_1: page([older, newer], "cur_2") });
  const directory = await signedIn(t, api);
  const before = Date.now();
  const result = await runCli(
    ["logs", ...TENANT, "--since", "1h", "--limit", "50", "--cursor", "cur_1"],
    { configDir: directory },
  );
  assert.equal(result.code, 0, result.stderr);
  const request = api.find(`${OBSERVABILITY}/logs`)[0];
  assert.equal(request.headers.authorization, "Bearer developer-session-token-0001");
  assert.equal(request.query.limit, "50");
  assert.equal(request.query.cursor, "cur_1");
  assert.equal(request.query.until, undefined);
  const from = Date.parse(request.query.from);
  assert.ok(Math.abs(from - (before - 3_600_000)) < 5_000, `from ${request.query.from} is not an hour ago`);
  assert.deepEqual(result.stdout.split("\n"), [
    "2026-08-06T11:59:00.000Z  error  edge-function:hello  boom [31m!",
    "2026-08-06T11:00:00.000Z  info  data-plane  started",
    "next cursor: cur_2",
    "",
  ]);
  assert.match(
    result.stderr,
    /^retained from 2026-05-08T12:00:00\.000Z; observed at 2026-08-06T12:00:00\.000Z \(90 days retention\)\n$/u,
  );
});

test("logs accepts ISO instants, refuses a bad window, and filters by level and source", async (t) => {
  const lines = [
    log("2026-08-06T11:00:00.000Z", "WARNING", "sync", "slow"),
    log("2026-08-06T11:30:00.000Z", "error", "edge-function:hello", "boom"),
    log("2026-08-06T11:45:00.000Z", "info", "edge-function:hello", "ok"),
  ];
  const api = await observabilityApi(t, "logs", { "": page(lines) });
  const directory = await signedIn(t, api);
  const env = { MAKO_PROJECT_ID: PROJECT_ID, MAKO_ENVIRONMENT_ID: ENVIRONMENT_ID };

  const iso = await runCli(
    ["observability", "logs", "--since", "2026-08-06T10:00:00Z", "--until", "2026-08-06T12:00:00Z", "--json", "--level", "warn"],
    { configDir: directory, env },
  );
  assert.equal(iso.code, 0, iso.stderr);
  const request = api.find(`${OBSERVABILITY}/logs`)[0];
  assert.equal(request.query.from, "2026-08-06T10:00:00.000Z");
  assert.equal(request.query.until, "2026-08-06T12:00:00.000Z");
  const filtered = JSON.parse(iso.stdout);
  assert.deepEqual(filtered.items, [lines[0]], "warning normalizes to warn; other levels drop");
  assert.deepEqual(filtered.retention, RETENTION);
  assert.equal(filtered.nextCursor, null);

  const bySource = await runCli(["logs", "--source", "edge-function:hello"], { configDir: directory, env });
  assert.equal(bySource.code, 0, bySource.stderr);
  assert.deepEqual(bySource.stdout.trimEnd().split("\n"), [
    "2026-08-06T11:45:00.000Z  info  edge-function:hello  ok",
    "2026-08-06T11:30:00.000Z  error  edge-function:hello  boom",
  ]);

  const requests = api.find(`${OBSERVABILITY}/logs`).length;
  const reversed = await runCli(["logs", "--since", "1h", "--until", "2h"], { configDir: directory, env });
  assert.equal(reversed.code, 2);
  assert.match(reversed.stderr, /--since must be before --until/u);
  const garbage = await runCli(["logs", "--since", "yesterday-ish"], { configDir: directory, env });
  assert.equal(garbage.code, 2);
  assert.match(garbage.stderr, /--since must be a duration/u);
  const badLevel = await runCli(["logs", "--level", "loud"], { configDir: directory, env });
  assert.equal(badLevel.code, 2);
  const badLimit = await runCli(["logs", "--limit", "5000"], { configDir: directory, env });
  assert.equal(badLimit.code, 2);
  assert.equal(api.find(`${OBSERVABILITY}/logs`).length, requests, "usage errors send nothing");
});

test("--all follows next cursors into one document; without it one page is verbatim", async (t) => {
  const first = record("2026-08-06T11:00:00.000Z", {
    kind: "usage",
    resource: "storage_bytes",
    quantity: 1024,
    unit: "bytes",
  });
  const second = record("2026-08-06T11:30:00.000Z", {
    kind: "usage",
    resource: "replication_requests_per_minute",
    quantity: 7,
    unit: "requests",
  });
  const api = await observabilityApi(t, "usage", { "": page([first], "c1"), c1: page([second]) });
  const directory = await signedIn(t, api);

  const one = await runCli(["usage", ...TENANT, "--json"], { configDir: directory });
  assert.equal(one.code, 0, one.stderr);
  assert.deepEqual(JSON.parse(one.stdout), page([first], "c1"));

  const all = await runCli(["observability", "usage", ...TENANT, "--json", "--all"], {
    configDir: directory,
  });
  assert.equal(all.code, 0, all.stderr);
  assert.deepEqual(JSON.parse(all.stdout), { items: [first, second], nextCursor: null });
  const cursors = api.find(`${OBSERVABILITY}/usage`).map((request) => request.query.cursor);
  assert.deepEqual(cursors, [undefined, undefined, "c1"]);
  assert.match(all.stderr, /90 days retention/u);

  const table = await runCli(["usage", ...TENANT, "--all"], { configDir: directory });
  assert.equal(table.code, 0, table.stderr);
  const rows = table.stdout.trimEnd().split("\n");
  assert.match(rows[0], /^time\s+resource\s+kind\s+quantity\s+unit$/u);
  assert.match(rows[2], /storage_bytes\s+level\s+1024\s+bytes$/u);
  assert.match(rows[3], /replication_requests_per_minute\s+flow\s+7\s+requests$/u);
  assert.doesNotMatch(table.stdout, /next cursor/u);
});

test("activity renders audited actions newest first, one line each", async (t) => {
  const events = [
    audit("2026-08-06T11:00:00.000Z", "policy.activate", "allowed", "policy v3\nactive"),
    audit("2026-08-06T11:30:00.000Z", "project.delete", "denied"),
  ];
  const api = await observabilityApi(t, "audit-events", { "": page(events) });
  const directory = await signedIn(t, api);
  const result = await runCli(["activity", ...TENANT, "--since", "24h"], { configDir: directory });
  assert.equal(result.code, 0, result.stderr);
  assert.deepEqual(result.stdout.trimEnd().split("\n"), [
    `2026-08-06T11:30:00.000Z  dev_owner  project.delete  project:${PROJECT_ID}  denied`,
    `2026-08-06T11:00:00.000Z  dev_owner  policy.activate  project:${PROJECT_ID}  allowed  policy v3 active (request req_audit)`,
  ]);
  const long = await runCli(["observability", "audit", ...TENANT, "--json"], { configDir: directory });
  assert.equal(long.code, 0, long.stderr);
  assert.deepEqual(JSON.parse(long.stdout).items, events, "JSON keeps the served order");
  assert.equal(api.find(`${OBSERVABILITY}/audit-events`).length, 2);
});

test("teams activity reads the team's own audit trail, newest first, with --changes on the wire", async (t) => {
  const events = [
    audit("2026-08-06T11:00:00.000Z", "invitation_create", "allowed", "target=inv_abcdefgh"),
    audit("2026-08-06T11:30:00.000Z", "membership_delete", "allowed", "target=dev_member01"),
  ];
  const seen = [];
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "GET" && request.path === "/v1/teams/org_abcdefgh/activity") {
          seen.push(request.query);
          return { status: 200, json: page(events) };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const result = await runCli(["teams", "activity", "org_abcdefgh", "--changes"], { configDir: directory });
  assert.equal(result.code, 0, result.stderr);
  assert.deepEqual(result.stdout.trimEnd().split("\n"), [
    `2026-08-06T11:30:00.000Z  dev_owner  membership_delete  project:${PROJECT_ID}  allowed  target=dev_member01 (request req_audit)`,
    `2026-08-06T11:00:00.000Z  dev_owner  invitation_create  project:${PROJECT_ID}  allowed  target=inv_abcdefgh (request req_audit)`,
  ]);
  assert.equal(seen[0].changes, "true");
  assert.equal(seen[0].order, "newest");
});

test("each observability kind reaches its endpoint and renders the console's columns", async (t) => {
  const cases = [
    [
      "quotas",
      "quotas",
      { kind: "quota", resource: "storage_bytes", limit: 200, consumed: 50, retryAfter: null },
      /storage_bytes\s+50\s+200\s+25\.0%\s+hard limit \/ no retry time/u,
    ],
    [
      "health",
      "health",
      { kind: "health", service: "data-plane", region: "us-east-1", status: "degraded", diagnostic: "disk\tpressure" },
      /data-plane\s+us-east-1\s+degraded\s+disk\tpressure/u,
    ],
    [
      "replication-errors",
      "replication-errors",
      { kind: "replication_error", collectionId: "todos", category: "checkpoint_expired", retryable: false, message: "resync", correlationId: "corr_9" },
      /todos\s+checkpoint_expired\s+false\s+resync\s+corr_9/u,
    ],
    [
      "auth-events",
      "auth-events",
      { kind: "authentication_event", category: "password", outcome: "failed", applicationUserId: null, message: "bad password", correlationId: "corr_2" },
      /password\s+failed\s+—\s+bad password\s+corr_2/u,
    ],
    [
      "function-metrics",
      "function-metrics",
      { kind: "function_metric", functionName: "hello", version: 3, region: "us-east-1", invocationCount: 10, errorCount: 1, latencyMilliseconds: 40, computeMilliseconds: 12 },
      /hello\s+3\s+us-east-1\s+10\s+1\s+40\s+12/u,
    ],
    [
      "index-state",
      "index-states",
      { kind: "index_state", collectionId: "todos", indexName: "by_owner", indexVersion: 2, state: "building", progressPercent: 40, message: null },
      /todos\s+by_owner\s+2\s+building\s+40%\s+—/u,
    ],
  ];
  for (const [command, path, payload, pattern] of cases) {
    const api = await observabilityApi(t, path, { "": page([record(NOW, payload)]) });
    const directory = await signedIn(t, api);
    const result = await runCli(["observability", command, ...TENANT, "--limit", "1"], { configDir: directory });
    assert.equal(result.code, 0, `${command}: ${result.stderr}`);
    assert.match(result.stdout, pattern, command);
    assert.equal(api.find(`${OBSERVABILITY}/${path}`)[0].query.limit, "1");
    assert.match(result.stderr, /retained from/u, command);
    const json = await runCli(["observability", command, ...TENANT, "--json"], { configDir: directory });
    assert.deepEqual(JSON.parse(json.stdout), page([record(NOW, payload)]), command);
  }
});

test("an unknown environment is exit 4 and a missing tenant is exit 2", async (t) => {
  const api = await startMockApi(
    authHandler({
      fallback: (request) =>
        request.path.endsWith("/observability/health")
          ? apiError("not_found", "no such environment", 404)
          : undefined,
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const missing = await runCli(["observability", "health", "--project", PROJECT_ID, "--env", "env_nope"], {
    configDir: directory,
  });
  assert.equal(missing.code, 4);
  assert.match(missing.stderr, /not_found/u);
  assert.equal(missing.stdout, "");
  const noTenant = await runCli(["observability", "health", "--project", PROJECT_ID], { configDir: directory });
  assert.equal(noTenant.code, 2);
  assert.match(noTenant.stderr, /--env <environment-id> is required/u);
});

const SUMMARY = {
  tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
  sections: {
    documents: {
      status: "current",
      observedAtUnixSeconds: at("2026-08-06T12:00:00Z"),
      freshUntilUnixSeconds: at("2026-08-06T12:01:00Z"),
      retainedSinceUnixSeconds: null,
      payload: { count: 12 },
      remediationCode: null,
    },
    replication: {
      status: "unavailable",
      observedAtUnixSeconds: at("2026-08-06T11:50:00Z"),
      freshUntilUnixSeconds: at("2026-08-06T11:51:00Z"),
      remediationCode: "telemetry_unavailable",
    },
  },
};

const CONNECT = {
  tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
  publicEndpoint: "https://api.example.test",
  publicKeyId: "pk_abcdefgh",
  publicKey: "",
  collections: [{ collectionId: "todos", activeSchemaVersion: 2 }],
  rxdbClientRange: ">=16 <18",
  templateVersion: 1,
};

function workspaceHandler(request) {
  if (request.method === "GET" && request.path === `${ENVIRONMENT}/workspace/summary`) {
    return { status: 200, json: SUMMARY };
  }
  if (request.method === "GET" && request.path === `${ENVIRONMENT}/workspace/navigation`) {
    return {
      status: 200,
      json: [
        { id: "data", label: "Data", path: `/projects/${PROJECT_ID}/data`, permitted: true },
        { id: "billing", label: "Billing", path: "/billing", permitted: false },
      ],
    };
  }
  if (request.method === "GET" && request.path === `${ENVIRONMENT}/connect`) {
    return { status: 200, json: CONNECT };
  }
  return undefined;
}

test("workspace summary, nav, and connect render the console's views without key material", async (t) => {
  const api = await startMockApi(authHandler({ fallback: workspaceHandler }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const summary = await runCli(["workspace", "summary", ...TENANT], { configDir: directory });
  assert.equal(summary.code, 0, summary.stderr);
  const lines = summary.stdout.trimEnd().split("\n");
  assert.equal(lines.length, 4, "header, rule, one line per section");
  assert.match(lines[2], /^documents\s+current\s+2026-08-06T12:00:00\.000Z\s+2026-08-06T12:01:00\.000Z\s+—\s+—$/u);
  assert.match(lines[3], /^replication\s+unavailable\s+2026-08-06T11:50:00\.000Z\s+.*telemetry_unavailable$/u);
  const summaryJson = await runCli(["workspace", "summary", ...TENANT, "--json"], { configDir: directory });
  assert.deepEqual(JSON.parse(summaryJson.stdout), SUMMARY);

  const nav = await runCli(["workspace", "nav", ...TENANT], { configDir: directory });
  assert.equal(nav.code, 0, nav.stderr);
  assert.match(nav.stdout, /^id\s+label\s+path\s+permitted\n/u);
  assert.match(nav.stdout, /billing\s+Billing\s+\/billing\s+false/u);

  const connect = await runCli(["workspace", "connect", ...TENANT], { configDir: directory });
  assert.equal(connect.code, 0, connect.stderr);
  assert.match(connect.stdout, /publicEndpoint\s+https:\/\/api\.example\.test/u);
  assert.match(connect.stdout, /publicKeyId\s+pk_abcdefgh/u);
  assert.match(connect.stdout, /collections\s+todos \(schema v2\)/u);
  assert.doesNotMatch(connect.stdout, /^publicKey\s/mu, "only the key id is shown; key material is never here");
  const connectJson = await runCli(["workspace", "connect", ...TENANT, "--json"], { configDir: directory });
  assert.deepEqual(JSON.parse(connectJson.stdout), CONNECT);
});

test("workspace check sends the request, lists steps, and exits 5 when one failed", async (t) => {
  let steps = [
    { id: "dns", state: "passed", remediationCode: null, retryable: false },
    { id: "public_key_recognition", state: "failed", remediationCode: "public_key_unknown", retryable: false },
    { id: "schema", state: "skipped", remediationCode: null, retryable: true },
  ];
  const api = await startMockApi(
    authHandler({
      fallback: (request) =>
        request.method === "POST" && request.path === `${ENVIRONMENT}/connect/check`
          ? { status: 200, json: { checkedAtUnixSeconds: at("2026-08-06T12:00:00Z"), steps } }
          : undefined,
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const failed = await runCli(
    ["workspace", "check", ...TENANT, "--public-key-id", "pk_abcdefgh", "--rxdb-version", "17.0.0", "--collection", "todos", "--schema-version", "2"],
    { configDir: directory },
  );
  assert.equal(failed.code, 5);
  assert.deepEqual(api.find(`${ENVIRONMENT}/connect/check`, "POST")[0].body, {
    publicKeyId: "pk_abcdefgh",
    collectionId: "todos",
    schemaVersion: 2,
    rxdbVersion: "17.0.0",
  });
  assert.deepEqual(failed.stdout.trimEnd().split("\n"), [
    "passed   dns",
    "failed   public_key_recognition  remediation: public_key_unknown (configuration change required)",
    "skipped  schema",
  ]);
  assert.match(failed.stderr, /checked at 2026-08-06T12:00:00\.000Z/u);
  assert.match(failed.stderr, /1 of 3 connection check steps failed: public_key_recognition/u);

  steps = steps.map((step) => ({ ...step, state: "passed" }));
  const passed = await runCli(["workspace", "check", ...TENANT, "--json"], { configDir: directory });
  assert.equal(passed.code, 0, passed.stderr);
  assert.deepEqual(api.find(`${ENVIRONMENT}/connect/check`, "POST")[1].body, {});
  assert.equal(JSON.parse(passed.stdout).steps.length, 3);

  const orphan = await runCli(["workspace", "check", ...TENANT, "--schema-version", "2"], { configDir: directory });
  assert.equal(orphan.code, 2);
});

test("sync summary maps the window to unix seconds and reports the recommended actions", async (t) => {
  const summary = {
    tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
    collectionId: "todos",
    windowStartUnixSeconds: at("2026-08-06T11:00:00Z"),
    windowEndUnixSeconds: at("2026-08-06T12:00:00Z"),
    observedAtUnixSeconds: at("2026-08-06T12:00:00Z"),
    retainedSinceUnixSeconds: at("2026-05-08T12:00:00Z"),
    pullCount: 12,
    pushCount: 3,
    throttled: 2,
    schemaMismatches: 1,
    clientVersionClasses: { "rxdb_16": 4 },
  };
  const api = await startMockApi(
    authHandler({
      fallback: (request) =>
        request.method === "GET" && request.path === `${ENVIRONMENT}/sync/summary`
          ? { status: 200, json: summary }
          : undefined,
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const result = await runCli(
    ["sync", "summary", ...TENANT, "--collection", "todos", "--from", "2026-08-06T11:00:00Z", "--until", "2026-08-06T12:00:00Z"],
    { configDir: directory },
  );
  assert.equal(result.code, 0, result.stderr);
  assert.deepEqual(api.find(`${ENVIRONMENT}/sync/summary`)[0].query, {
    collectionId: "todos",
    from: String(at("2026-08-06T11:00:00Z")),
    until: String(at("2026-08-06T12:00:00Z")),
  });
  assert.match(result.stdout, /pulls\s+12/u);
  assert.match(result.stdout, /windowStart\s+2026-08-06T11:00:00\.000Z/u);
  assert.match(result.stdout, /conflicts\s+—/u);
  assert.match(result.stderr, /recommended: throttling is retryable/u);
  assert.match(result.stderr, /recommended: schema mismatches require an RxDB migration/u);

  const defaults = await runCli(["sync", "summary", ...TENANT, "--json"], { configDir: directory });
  assert.equal(defaults.code, 0, defaults.stderr);
  assert.deepEqual(api.find(`${ENVIRONMENT}/sync/summary`)[1].query, {}, "the API's default window");
  assert.deepEqual(JSON.parse(defaults.stdout), summary);
});

const BACKUP = {
  backupId: "bkp_0001",
  tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
  recoveryPointUnixSeconds: at("2026-08-06T11:00:00Z"),
  verifiedAtUnixSeconds: at("2026-08-06T11:10:00Z"),
  retainedUntilUnixSeconds: at("2026-09-05T11:00:00Z"),
  lastRestoreDrillUnixSeconds: null,
  recoveryObjectiveStatus: "within_objective",
};

const RESTORE = {
  requestId: "rst_0001",
  backupId: "bkp_0001",
  target: { projectId: PROJECT_ID, environmentId: "env_restored" },
  state: "requested",
  accessible: false,
  overwritePermitted: false,
  promotionPermitted: false,
  requestedAtUnixSeconds: at("2026-08-06T12:00:00Z"),
  updatedAtUnixSeconds: at("2026-08-06T12:00:00Z"),
};

function backupHandler(request) {
  if (request.method === "GET" && request.path === `${ENVIRONMENT}/backups`) {
    return { status: 200, json: [BACKUP] };
  }
  if (request.method === "GET" && request.path === `/v1/projects/${PROJECT_ID}/restore-requests`) {
    return { status: 200, json: [RESTORE] };
  }
  if (request.method === "POST" && request.path === `/v1/projects/${PROJECT_ID}/restore-requests`) {
    return { status: 202, json: RESTORE };
  }
  if (request.method === "POST" && request.path === "/v1/developer-auth/sessions/current/actions/verify-password") {
    return { status: 200, json: { token: "dst1_grant", expiresAtUnixSeconds: at("2026-08-06T12:05:00Z") } };
  }
  return undefined;
}

test("backups list and restore requests render, and a restore needs --yes and a step-up", async (t) => {
  const api = await startMockApi(authHandler({ fallback: backupHandler }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const restorePath = `/v1/projects/${PROJECT_ID}/restore-requests`;

  const backups = await runCli(["backups", "list", ...TENANT], { configDir: directory });
  assert.equal(backups.code, 0, backups.stderr);
  assert.match(backups.stdout, /bkp_0001\s+2026-08-06T11:00:00\.000Z\s+2026-08-06T11:10:00\.000Z\s+2026-09-05T11:00:00\.000Z\s+no recorded drill\s+within_objective/u);

  const list = await runCli(["backups", "restore-requests", "list", "--project", PROJECT_ID], { configDir: directory });
  assert.equal(list.code, 0, list.stderr);
  assert.match(list.stdout, /rst_0001\s+bkp_0001\s+env_restored\s+requested\s+false/u);
  const listJson = await runCli(["backups", "restore-requests", "list"], {
    configDir: directory,
    env: { MAKO_PROJECT_ID: PROJECT_ID },
  });
  assert.match(listJson.stdout, /rst_0001/u, "MAKO_PROJECT_ID stands in for --project");
  assert.equal(listJson.code, 0);

  const input = JSON.stringify({
    environmentId: ENVIRONMENT_ID,
    backupId: "bkp_0001",
    targetEnvironmentName: "restored",
    reason: "drill",
    stepUpToken: "dst1_from_input",
  });
  const refused = await runCli(
    ["backups", "restore-requests", "create", "--project", PROJECT_ID, "--input", input],
    { configDir: directory },
  );
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, new RegExp(`refusing to restore a backup of project ${PROJECT_ID} without --yes`, "u"));
  assert.equal(refused.stdout, "");
  assert.equal(api.find(restorePath, "POST").length, 0, "nothing is sent without confirmation");

  const created = await runCli(
    ["backups", "restore-requests", "create", "--project", PROJECT_ID, "--input", input, "--yes", "--json"],
    { configDir: directory },
  );
  assert.equal(created.code, 0, created.stderr);
  const post = api.find(restorePath, "POST")[0];
  assert.deepEqual(post.body, JSON.parse(input));
  assert.match(post.headers["idempotency-key"], /^[0-9a-f-]{36}$/u);
  assert.deepEqual(JSON.parse(created.stdout), RESTORE);
  assert.match(created.stderr, /restore request rst_0001 accepted/u);

  const withoutToken = JSON.stringify({ backupId: "bkp_0001", targetEnvironmentName: "restored", reason: "drill" });
  const noStepUp = await runCli(
    ["backups", "restore-requests", "create", "--project", PROJECT_ID, "--env", ENVIRONMENT_ID, "--input", withoutToken, "--yes"],
    { configDir: directory },
  );
  assert.equal(noStepUp.code, 3, noStepUp.stderr);
  assert.match(noStepUp.stderr, /step-up/u);
  assert.equal(api.find(restorePath, "POST").length, 1, "no request without a grant");

  const passwordFile = join(directory, "step-up.txt");
  await writeFile(passwordFile, "correct horse battery staple\n", { mode: 0o600 });
  const inputFile = join(directory, "restore.json");
  await writeFile(inputFile, withoutToken);
  const stepped = await runCli(
    ["backups", "restore-requests", "create", "--project", PROJECT_ID, "--env", ENVIRONMENT_ID, "--input", `@${inputFile}`, "--yes"],
    { configDir: directory, env: { MAKO_STEP_UP_PASSWORD_FILE: passwordFile } },
  );
  assert.equal(stepped.code, 0, stepped.stderr);
  const verify = api.find("/v1/developer-auth/sessions/current/actions/verify-password", "POST")[0];
  assert.deepEqual(verify.body, { password: "correct horse battery staple" });
  const second = api.find(restorePath, "POST")[1];
  assert.equal(second.body.stepUpToken, "dst1_grant");
  assert.equal(second.body.environmentId, ENVIRONMENT_ID);
  assert.match(stepped.stdout, /requestId\s+rst_0001/u);
  assert.match(stepped.stdout, /targetEnvironmentId\s+env_restored/u);

  const incomplete = await runCli(
    ["backups", "restore-requests", "create", "--project", PROJECT_ID, "--input", '{"backupId":"bkp_0001"}', "--yes"],
    { configDir: directory },
  );
  assert.equal(incomplete.code, 2);
  assert.match(incomplete.stderr, /environmentId/u);
});
