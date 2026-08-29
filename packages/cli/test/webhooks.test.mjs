// Webhook endpoints and their deliveries against the loopback mock: request
// paths and bodies, idempotency keys, the signing secret shown exactly once,
// partial updates, destructive confirmation, and delivery paging.
import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
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

const BASE = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const WEBHOOKS = `${BASE}/webhooks`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const SECRET = "whs_0123456789abcdef0123456789abcdef";
const ROTATED = "whs_fedcba9876543210fedcba9876543210";
const HOOK = "whk_orders000001";

function endpoint(overrides = {}) {
  return {
    id: HOOK,
    url: "https://hooks.example.test/mako",
    description: "orders to the warehouse",
    subscriptions: [{ collectionId: "orders", events: ["insert", "update"] }],
    state: "active",
    enabled: true,
    pausedReason: null,
    pausedAt: null,
    consecutiveFailures: 0,
    secretVersion: 1,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function delivery(overrides = {}) {
  return {
    id: "whd_000000000001",
    endpointId: HOOK,
    event: "insert",
    collectionId: "orders",
    documentId: "ord_1",
    revision: "1-a",
    commitPosition: 11,
    state: "delivered",
    attempts: 1,
    nextAttemptAt: null,
    lastResponseStatus: 200,
    lastError: null,
    redeliveryOf: null,
    createdAt: NOW,
    deliveredAt: NOW,
    ...overrides,
  };
}

function occurrences(text, needle) {
  return text.split(needle).length - 1;
}

/** Routes keyed by `METHOD /path`; anything else is unhandled (404). */
function router(routes) {
  return (request) => routes[`${request.method} ${request.path}`]?.(request);
}

/** A signed-in profile against a mock serving `routes`; `cli` appends the tenant options. */
async function tenantCli(t, routes) {
  const api = await startMockApi(authHandler({ fallback: router(routes) }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const cli = (argv, options = {}) => runCli([...argv, ...TENANT], { configDir: directory, ...options });
  return { api, cli, directory };
}

test("webhooks list renders a table without any secret, --json the response verbatim, with the bearer token", async (t) => {
  const items = [
    endpoint(),
    endpoint({
      id: "whk_paused000001",
      url: "https://down.example.test/hook",
      state: "paused",
      pausedReason: "12 consecutive failures",
      pausedAt: NOW,
      consecutiveFailures: 12,
      secretVersion: 3,
    }),
  ];
  const { api, cli } = await tenantCli(t, {
    [`GET ${WEBHOOKS}`]: () => ({ status: 200, json: { items } }),
  });
  const human = await cli(["webhooks", "list"]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /^id\s+url\s+state\s+failures\s+secretVersion\s+updatedAt\n/u);
  assert.match(human.stdout, /whk_orders000001\s+https:\/\/hooks\.example\.test\/mako\s+active\s+0\s+1\s+2026-08-06T12:00:00\.000Z/u);
  assert.match(human.stdout, /whk_paused000001\s+https:\/\/down\.example\.test\/hook\s+paused\s+12\s+3/u);
  assert.doesNotMatch(human.stdout, /shown once|whs_/u);
  assert.equal(api.find(WEBHOOKS, "GET")[0].headers.authorization, BEARER);

  const json = await cli(["webhooks", "list", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), items);
});

test("webhooks create sends url, subscriptions, description, and enabled with an idempotency key and prints the secret exactly once", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`POST ${WEBHOOKS}`]: (request) => ({
      status: 201,
      json: { endpoint: endpoint({ ...request.body }), signingSecret: SECRET },
    }),
  });

  const block = await cli([
    "webhooks", "create",
    "--url", "https://hooks.example.test/mako",
    "--subscribe", "orders:insert,update",
    "--subscribe", "orders:delete",
    "--subscribe", "shipments",
    "--description", "orders to the warehouse",
  ]);
  assert.equal(block.code, 0, block.stderr);
  const first = api.find(WEBHOOKS, "POST")[0];
  assert.deepEqual(first.body, {
    url: "https://hooks.example.test/mako",
    subscriptions: [
      { collectionId: "orders", events: ["insert", "update", "delete"] },
      { collectionId: "shipments", events: ["insert", "update", "delete"] },
    ],
    enabled: true,
    description: "orders to the warehouse",
  });
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.equal(occurrences(block.stdout, SECRET), 1, "the secret is printed exactly once");
  assert.match(block.stdout, /signing secret for webhook whk_orders000001 \(shown once\)/u);
  assert.match(block.stdout, /^id\s+whk_orders000001\n/u);
  assert.match(block.stderr, /signing secret of webhook whk_orders000001 is shown once and cannot be read back/u);
  assert.doesNotMatch(block.stderr, /whs_/u, "the warning names no secret");

  const disabled = await cli([
    "webhooks", "create", "--url", "https://hooks.example.test/later", "--subscribe", "orders:delete", "--disabled", "--json",
  ]);
  assert.equal(disabled.code, 0, disabled.stderr);
  const second = api.find(WEBHOOKS, "POST")[1];
  assert.deepEqual(second.body, {
    url: "https://hooks.example.test/later",
    subscriptions: [{ collectionId: "orders", events: ["delete"] }],
    enabled: false,
  });
  assert.notEqual(second.headers["idempotency-key"], first.headers["idempotency-key"]);
  const parsed = JSON.parse(disabled.stdout);
  assert.equal(parsed.secret, SECRET, "--json carries the secret as one field");
  assert.equal(parsed.id, HOOK);
  assert.equal(occurrences(disabled.stdout, SECRET), 1);

  const file = join(directory, "webhook-secret.txt");
  const toFile = await cli([
    "webhooks", "create", "--url", "https://hooks.example.test/ci", "--subscribe", "orders", "--secret-file", file,
  ]);
  assert.equal(toFile.code, 0, toFile.stderr);
  assert.doesNotMatch(toFile.stdout, /whs_/u);
  assert.doesNotMatch(toFile.stderr, /whs_/u);
  assert.equal(await readFile(file, "utf8"), `${SECRET}\n`);
  assert.equal((await stat(file)).mode & 0o777, 0o600);

  const noUrl = await cli(["webhooks", "create", "--subscribe", "orders"]);
  assert.equal(noUrl.code, 2);
  assert.match(noUrl.stderr, /--url <url> is required/u);
  const noSubscription = await cli(["webhooks", "create", "--url", "https://hooks.example.test/x"]);
  assert.equal(noSubscription.code, 2);
  assert.match(noSubscription.stderr, /--subscribe <collection>\[:<events>\] is required/u);
  const badUrl = await cli(["webhooks", "create", "--url", "hooks.example.test/x", "--subscribe", "orders"]);
  assert.equal(badUrl.code, 2);
  assert.match(badUrl.stderr, /--url must be an absolute URL/u);
  const badScheme = await cli(["webhooks", "create", "--url", "ftp://hooks.example.test/x", "--subscribe", "orders"]);
  assert.equal(badScheme.code, 2);
  assert.match(badScheme.stderr, /--url must use https/u);
  const badEvent = await cli(["webhooks", "create", "--url", "https://hooks.example.test/x", "--subscribe", "orders:insert,upsert"]);
  assert.equal(badEvent.code, 2);
  assert.match(badEvent.stderr, /--subscribe orders: unknown event "upsert"; use insert, update, delete/u);
  const badCollection = await cli(["webhooks", "create", "--url", "https://hooks.example.test/x", "--subscribe", "Orders:insert"]);
  assert.equal(badCollection.code, 2);
  assert.match(badCollection.stderr, /--subscribe must name a collection id/u);
  const noEvents = await cli(["webhooks", "create", "--url", "https://hooks.example.test/x", "--subscribe", "orders:"]);
  assert.equal(noEvents.code, 2);
  assert.match(noEvents.stderr, /--subscribe orders: list at least one of insert, update, delete/u);
  assert.equal(api.find(WEBHOOKS, "POST").length, 3, "usage errors send nothing");
});

test("webhooks get shows the record with its pause reason; a missing endpoint exits 4", async (t) => {
  const paused = endpoint({
    state: "paused",
    pausedReason: "12 consecutive failures",
    pausedAt: NOW,
    consecutiveFailures: 12,
  });
  const { cli } = await tenantCli(t, {
    [`GET ${WEBHOOKS}/${HOOK}`]: () => ({ status: 200, json: paused }),
    [`GET ${WEBHOOKS}/whk_ghost0000001`]: () => apiError("not_found", "no such webhook", 404),
  });
  const found = await cli(["webhooks", "get", HOOK]);
  assert.equal(found.code, 0, found.stderr);
  assert.match(found.stdout, /^id\s+whk_orders000001\n/u);
  assert.match(found.stdout, /state\s+paused/u);
  assert.match(found.stdout, /pausedReason\s+12 consecutive failures/u);
  assert.match(found.stdout, /"collectionId": "orders"/u);
  assert.doesNotMatch(found.stdout, /shown once|whs_/u);

  const json = await cli(["webhooks", "get", HOOK, "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), paused);

  const missing = await cli(["webhooks", "get", "whk_ghost0000001"]);
  assert.equal(missing.code, 4);
  assert.equal(missing.stdout, "");
  assert.match(missing.stderr, /no such webhook/u);
});

test("webhooks update sends only the options given and refuses --enable with --disable", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`PATCH ${WEBHOOKS}/${HOOK}`]: (request) => ({ status: 200, json: endpoint({ ...request.body, updatedAt: "2026-08-07T12:00:00.000Z" }) }),
  });
  const path = `${WEBHOOKS}/${HOOK}`;

  const urlOnly = await cli(["webhooks", "update", HOOK, "--url", "https://hooks.example.test/v2", "--json"]);
  assert.equal(urlOnly.code, 0, urlOnly.stderr);
  const first = api.find(path, "PATCH")[0];
  assert.deepEqual(first.body, { url: "https://hooks.example.test/v2" });
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(urlOnly.stdout).url, "https://hooks.example.test/v2");

  const disabled = await cli(["webhooks", "update", HOOK, "--disable", "--description", "paused for the migration"]);
  assert.equal(disabled.code, 0, disabled.stderr);
  assert.deepEqual(api.find(path, "PATCH")[1].body, { description: "paused for the migration", enabled: false });
  assert.match(disabled.stdout, /^id\s+whk_orders000001\n/u);

  const resubscribed = await cli(["webhooks", "update", HOOK, "--enable", "--subscribe", "orders:delete", "--subscribe", "invoices:insert"]);
  assert.equal(resubscribed.code, 0, resubscribed.stderr);
  assert.deepEqual(api.find(path, "PATCH")[2].body, {
    subscriptions: [
      { collectionId: "orders", events: ["delete"] },
      { collectionId: "invoices", events: ["insert"] },
    ],
    enabled: true,
  });

  const nothing = await cli(["webhooks", "update", HOOK]);
  assert.equal(nothing.code, 2);
  assert.match(nothing.stderr, /nothing to update/u);
  const both = await cli(["webhooks", "update", HOOK, "--enable", "--disable"]);
  assert.equal(both.code, 2);
  assert.match(both.stderr, /--enable and --disable exclude each other/u);
  assert.equal(api.find(path, "PATCH").length, 3, "a refused update sends nothing");
});

test("webhooks delete is confirmed and sends the idempotency key; nothing is printed but the notice", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`DELETE ${WEBHOOKS}/${HOOK}`]: () => ({ status: 204 }),
  });
  const path = `${WEBHOOKS}/${HOOK}`;

  const refused = await cli(["webhooks", "delete", HOOK]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to delete webhook whk_orders000001 without --yes/u);
  assert.equal(refused.stdout, "");
  assert.equal(api.find(path, "DELETE").length, 0, "nothing is sent without confirmation");

  const mistyped = await cli(["webhooks", "delete", HOOK], { stdin: "whk_other\n", isTTY: true });
  assert.equal(mistyped.code, 2);
  assert.equal(api.find(path, "DELETE").length, 0);

  const deleted = await cli(["webhooks", "delete", HOOK, "--yes"]);
  assert.equal(deleted.code, 0, deleted.stderr);
  const sent = api.find(path, "DELETE")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(sent.text, "");
  assert.equal(deleted.stdout, "");
  assert.match(deleted.stderr, /webhook whk_orders000001 deleted/u);

  const json = await cli(["webhooks", "delete", HOOK, "--yes", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), { id: HOOK, state: "deleted" });

  const typed = await cli(["webhooks", "delete", HOOK], { stdin: `${HOOK}\n`, isTTY: true });
  assert.equal(typed.code, 0, typed.stderr);
  assert.equal(api.find(path, "DELETE").length, 3);
});

test("webhooks rotate-secret is confirmed and prints the new secret exactly once; resume posts the action", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`POST ${WEBHOOKS}/${HOOK}/actions/rotate-secret`]: () => ({
      status: 200,
      json: { endpoint: endpoint({ secretVersion: 2 }), signingSecret: ROTATED },
    }),
    [`POST ${WEBHOOKS}/${HOOK}/actions/resume`]: () => ({
      status: 200,
      json: endpoint({ state: "active", consecutiveFailures: 0 }),
    }),
  });
  const rotatePath = `${WEBHOOKS}/${HOOK}/actions/rotate-secret`;

  const refused = await cli(["webhooks", "rotate-secret", HOOK]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to rotate the signing secret of webhook whk_orders000001 without --yes/u);
  assert.equal(api.find(rotatePath, "POST").length, 0);

  const rotated = await cli(["webhooks", "rotate-secret", HOOK, "--yes"]);
  assert.equal(rotated.code, 0, rotated.stderr);
  const sent = api.find(rotatePath, "POST")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(sent.text, "");
  assert.equal(occurrences(rotated.stdout, ROTATED), 1);
  assert.match(rotated.stdout, /signing secret for webhook whk_orders000001 \(shown once\)/u);
  assert.match(rotated.stdout, /secretVersion\s+2/u);
  assert.match(rotated.stderr, /shown once and cannot be read back/u);
  assert.doesNotMatch(rotated.stderr, /whs_/u);

  const json = await cli(["webhooks", "rotate-secret", HOOK, "--yes", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.equal(JSON.parse(json.stdout).secret, ROTATED);
  assert.equal(occurrences(json.stdout, ROTATED), 1);

  const resumed = await cli(["webhooks", "resume", HOOK]);
  assert.equal(resumed.code, 0, resumed.stderr);
  const resume = api.find(`${WEBHOOKS}/${HOOK}/actions/resume`, "POST")[0];
  assert.match(resume.headers["idempotency-key"], UUID);
  assert.equal(resume.text, "");
  assert.match(resumed.stdout, /state\s+active/u);
  assert.doesNotMatch(resumed.stdout, /shown once|whs_/u, "resuming shows no secret");
});

test("webhooks deliveries maps --state/--limit/--cursor to the query and --all follows the cursor", async (t) => {
  const failed = delivery({
    id: "whd_000000000002",
    event: "update",
    documentId: "ord_2",
    state: "failed",
    attempts: 5,
    lastResponseStatus: 503,
    lastError: "status_503",
    deliveredAt: null,
  });
  const pages = {
    undefined: { items: [delivery()], nextCursor: "c1" },
    c1: { items: [failed], nextCursor: null },
  };
  const { api, cli } = await tenantCli(t, {
    [`GET ${WEBHOOKS}/${HOOK}/deliveries`]: (request) => ({ status: 200, json: pages[String(request.query.cursor)] }),
  });
  const path = `${WEBHOOKS}/${HOOK}/deliveries`;

  const human = await cli(["webhooks", "deliveries", HOOK]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /^id\s+event\s+collectionId\s+documentId\s+state\s+attempts\s+status\s+error\s+createdAt\n/u);
  assert.match(human.stdout, /whd_000000000001\s+insert\s+orders\s+ord_1\s+delivered\s+1\s+200\s+—\s+2026-08-06T12:00:00\.000Z/u);
  assert.match(human.stdout, /next cursor: c1\n$/u);
  assert.deepEqual(api.find(path, "GET")[0].query, {}, "no query parameter is invented");
  assert.equal(api.find(path, "GET")[0].headers.authorization, BEARER);

  const filtered = await cli(["webhooks", "deliveries", HOOK, "--state", "failed", "--limit", "25", "--cursor", "c1", "--json"]);
  assert.equal(filtered.code, 0, filtered.stderr);
  assert.deepEqual(api.find(path, "GET")[1].query, { state: "failed", limit: "25", cursor: "c1" });
  assert.deepEqual(JSON.parse(filtered.stdout), pages.c1, "--json is the page verbatim");

  const all = await cli(["webhooks", "deliveries", HOOK, "--state", "failed", "--all", "--json"]);
  assert.equal(all.code, 0, all.stderr);
  const followed = api.find(path, "GET").slice(2);
  assert.equal(followed.length, 2);
  assert.deepEqual(followed[0].query, { state: "failed" });
  assert.deepEqual(followed[1].query, { state: "failed", cursor: "c1" }, "the state is kept while following");
  const collected = JSON.parse(all.stdout);
  assert.deepEqual(collected.items.map((item) => item.id), ["whd_000000000001", "whd_000000000002"]);
  assert.equal(collected.nextCursor, null);

  const allHuman = await cli(["webhooks", "deliveries", HOOK, "--all"]);
  assert.equal(allHuman.code, 0, allHuman.stderr);
  assert.match(allHuman.stdout, /whd_000000000002\s+update\s+orders\s+ord_2\s+failed\s+5\s+503\s+status_503/u);
  assert.doesNotMatch(allHuman.stdout, /next cursor/u);

  const badState = await cli(["webhooks", "deliveries", HOOK, "--state", "lost"]);
  assert.equal(badState.code, 2);
  assert.match(badState.stderr, /--state must be one of pending, delivered, failed/u);
  const badLimit = await cli(["webhooks", "deliveries", HOOK, "--limit", "201"]);
  assert.equal(badLimit.code, 2);
  assert.match(badLimit.stderr, /--limit must be between 1 and 200/u);
  assert.equal(api.find(path, "GET").length, 6);
});

test("webhooks redeliver posts to the delivery's action with an idempotency key and prints the new delivery", async (t) => {
  const original = "whd_000000000002";
  const { api, cli } = await tenantCli(t, {
    [`POST ${WEBHOOKS}/${HOOK}/deliveries/${original}/actions/redeliver`]: () => ({
      status: 202,
      json: delivery({ id: "whd_000000000009", state: "pending", attempts: 0, lastResponseStatus: null, deliveredAt: null, redeliveryOf: original }),
    }),
    [`POST ${WEBHOOKS}/${HOOK}/deliveries/whd_000000000003/actions/redeliver`]: () =>
      apiError("conflict", "endpoint is paused; resume it first", 409),
  });
  const path = `${WEBHOOKS}/${HOOK}/deliveries/${original}/actions/redeliver`;

  const queued = await cli(["webhooks", "redeliver", HOOK, original]);
  assert.equal(queued.code, 0, queued.stderr);
  const sent = api.find(path, "POST")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(sent.text, "");
  assert.match(queued.stdout, /^id\s+whd_000000000009\n/u);
  assert.match(queued.stdout, /redeliveryOf\s+whd_000000000002/u);
  assert.match(queued.stdout, /state\s+pending/u);

  const json = await cli(["webhooks", "redeliver", HOOK, original, "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.equal(JSON.parse(json.stdout).redeliveryOf, original);

  const refused = await cli(["webhooks", "redeliver", HOOK, "whd_000000000003"]);
  assert.equal(refused.code, 5);
  assert.match(refused.stderr, /error conflict: endpoint is paused; resume it first/u);

  const missingDelivery = await cli(["webhooks", "redeliver", HOOK]);
  assert.equal(missingDelivery.code, 2);
  assert.match(missingDelivery.stderr, /delivery-id/u);
  assert.equal(api.requests.filter((r) => r.method === "POST" && r.path.endsWith("/actions/redeliver")).length, 3);
});
