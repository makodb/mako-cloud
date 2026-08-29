// Storage buckets and objects against the loopback mock: request paths and
// bodies, idempotency and confirmation headers, rules from a file, partial
// updates, destructive confirmation, paging, and segment-wise path encoding.
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

const BASE = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const BUCKETS = `${BASE}/storage-buckets`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

const RULES = [
  { id: "owner-reads", effect: "allow", operations: ["read"], expression: "old.owner_id == identity.user_id" },
  { id: "owner-writes", effect: "allow", operations: ["create", "update", "delete"], expression: "new.owner_id == identity.user_id" },
];

function bucket(overrides = {}) {
  return {
    id: "avatars",
    access: "policy",
    maxObjectBytes: 1048576,
    allowedContentTypes: ["image/*"],
    rules: RULES,
    version: 1,
    objectCount: 3,
    totalBytes: 12345,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function object(overrides = {}) {
  return {
    path: "users/42/me.png",
    contentType: "image/png",
    sizeBytes: 4096,
    ownerId: "usr_42",
    digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
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

test("storage buckets list renders a table, --json the response verbatim, with the bearer token", async (t) => {
  const items = [bucket(), bucket({ id: "public-assets", access: "public", objectCount: 0, totalBytes: 0 })];
  const { api, cli } = await tenantCli(t, {
    [`GET ${BUCKETS}`]: () => ({ status: 200, json: { items } }),
  });
  const human = await cli(["storage", "buckets", "list"]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /^id\s+access\s+objects\s+bytes\s+maxObjectBytes\s+updatedAt\n/u);
  assert.match(human.stdout, /avatars\s+policy\s+3\s+12345\s+1048576\s+2026-08-06T12:00:00\.000Z/u);
  assert.match(human.stdout, /public-assets\s+public\s+0\s+0/u);
  assert.equal(api.find(BUCKETS, "GET")[0].headers.authorization, BEARER);

  const json = await cli(["storage", "buckets", "list", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), items);
});

test("storage buckets create sends access, limits, content types, and rules from a file with an idempotency key", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`POST ${BUCKETS}`]: (request) => ({ status: 201, json: bucket({ ...request.body, objectCount: 0, totalBytes: 0 }) }),
  });
  const rulesFile = join(directory, "rules.json");
  await writeFile(rulesFile, JSON.stringify(RULES));

  const full = await cli([
    "storage", "buckets", "create", "avatars",
    "--access", "public", "--max-object-bytes", "2097152",
    "--content-type", "image/*", "--content-type", "image/png", "--content-type", "image/*",
    "--rules", `@${rulesFile}`, "--json",
  ]);
  assert.equal(full.code, 0, full.stderr);
  const first = api.find(BUCKETS, "POST")[0];
  assert.deepEqual(first.body, {
    id: "avatars",
    access: "public",
    maxObjectBytes: 2097152,
    allowedContentTypes: ["image/*", "image/png"],
    rules: RULES,
  });
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(full.stdout).id, "avatars");

  const minimal = await cli(["storage", "buckets", "create", "uploads"]);
  assert.equal(minimal.code, 0, minimal.stderr);
  const second = api.find(BUCKETS, "POST")[1];
  assert.deepEqual(second.body, { id: "uploads", maxObjectBytes: 1048576 }, "the API's defaults are left to it");
  assert.match(second.headers["idempotency-key"], UUID);
  assert.notEqual(second.headers["idempotency-key"], first.headers["idempotency-key"]);
  assert.match(minimal.stdout, /^id\s+uploads\n/u);

  const fromStdin = await cli(["storage", "buckets", "create", "docs", "--rules", "-"], { stdin: JSON.stringify([RULES[0]]) });
  assert.equal(fromStdin.code, 0, fromStdin.stderr);
  assert.deepEqual(api.find(BUCKETS, "POST")[2].body.rules, [RULES[0]]);

  const badAccess = await cli(["storage", "buckets", "create", "x", "--access", "open"]);
  assert.equal(badAccess.code, 2);
  assert.match(badAccess.stderr, /--access must be one of policy, public/u);
  const tooBig = await cli(["storage", "buckets", "create", "x", "--max-object-bytes", "16777217"]);
  assert.equal(tooBig.code, 2);
  assert.match(tooBig.stderr, /--max-object-bytes/u);
  const badRules = await cli(["storage", "buckets", "create", "x", "--rules", '[{"id":"r","effect":"maybe","operations":["read"],"expression":"true"}]']);
  assert.equal(badRules.code, 2);
  assert.match(badRules.stderr, /--rules\[0\]\.effect must be allow or deny/u);
  const badOperation = await cli(["storage", "buckets", "create", "x", "--rules", '[{"id":"r","effect":"allow","operations":["list"],"expression":"true"}]']);
  assert.equal(badOperation.code, 2);
  assert.match(badOperation.stderr, /operations must list one or more of create, read, update, delete/u);
  const notArray = await cli(["storage", "buckets", "create", "x", "--rules", "{}"]);
  assert.equal(notArray.code, 2);
  assert.match(notArray.stderr, /--rules must be a JSON array/u);
  const badJson = await cli(["storage", "buckets", "create", "x", "--rules", "{not json"]);
  assert.equal(badJson.code, 2);
  assert.match(badJson.stderr, /not valid JSON/u);
  assert.equal(api.find(BUCKETS, "POST").length, 3, "usage errors send nothing");
});

test("storage buckets get shows the record with its rules; a missing bucket exits 4", async (t) => {
  const { cli } = await tenantCli(t, {
    [`GET ${BUCKETS}/avatars`]: () => ({ status: 200, json: bucket() }),
    [`GET ${BUCKETS}/ghost`]: () => apiError("not_found", "no such bucket", 404),
  });
  const found = await cli(["storage", "buckets", "get", "avatars"]);
  assert.equal(found.code, 0, found.stderr);
  assert.match(found.stdout, /^id\s+avatars\n/u);
  assert.match(found.stdout, /access\s+policy/u);
  assert.match(found.stdout, /"id": "owner-reads"/u);
  assert.match(found.stdout, /old\.owner_id == identity\.user_id/u);

  const json = await cli(["storage", "buckets", "get", "avatars", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), bucket());

  const missing = await cli(["storage", "buckets", "get", "ghost"]);
  assert.equal(missing.code, 4);
  assert.equal(missing.stdout, "");
  assert.match(missing.stderr, /no such bucket/u);
});

test("storage buckets update sends only the options given", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`PATCH ${BUCKETS}/avatars`]: (request) => ({ status: 200, json: bucket({ ...request.body, version: 2 }) }),
  });
  const path = `${BUCKETS}/avatars`;

  const accessOnly = await cli(["storage", "buckets", "update", "avatars", "--access", "public", "--json"]);
  assert.equal(accessOnly.code, 0, accessOnly.stderr);
  const first = api.find(path, "PATCH")[0];
  assert.deepEqual(first.body, { access: "public" });
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(accessOnly.stdout).version, 2);

  const limits = await cli(["storage", "buckets", "update", "avatars", "--max-object-bytes", "4194304", "--content-type", "image/*", "--content-type", "text/plain"]);
  assert.equal(limits.code, 0, limits.stderr);
  assert.deepEqual(api.find(path, "PATCH")[1].body, { maxObjectBytes: 4194304, allowedContentTypes: ["image/*", "text/plain"] });
  assert.match(limits.stdout, /^id\s+avatars\n/u);

  const anyType = await cli(["storage", "buckets", "update", "avatars", "--content-type", ""]);
  assert.equal(anyType.code, 0, anyType.stderr);
  assert.deepEqual(api.find(path, "PATCH")[2].body, { allowedContentTypes: [] });

  const rules = await cli(["storage", "buckets", "update", "avatars", "--rules", JSON.stringify([RULES[1]])]);
  assert.equal(rules.code, 0, rules.stderr);
  assert.deepEqual(api.find(path, "PATCH")[3].body, { rules: [RULES[1]] });

  const nothing = await cli(["storage", "buckets", "update", "avatars"]);
  assert.equal(nothing.code, 2);
  assert.match(nothing.stderr, /nothing to update/u);
  assert.equal(api.find(path, "PATCH").length, 4, "an empty update sends nothing");
});

test("storage buckets delete is confirmed, carries the confirmation header, and only --delete-objects deletes contents", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`DELETE ${BUCKETS}/avatars`]: (request) =>
      request.query.deleteObjects === "true"
        ? { status: 200, json: { objectCount: 3, totalBytes: 12345 } }
        : apiError("conflict", "bucket avatars still holds 3 objects; pass deleteObjects=true to delete them with it", 409),
    [`DELETE ${BUCKETS}/empty`]: () => ({ status: 200, json: { objectCount: 0, totalBytes: 0 } }),
  });
  const path = `${BUCKETS}/avatars`;

  const refused = await cli(["storage", "buckets", "delete", "avatars"]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to delete bucket avatars without --yes/u);
  assert.equal(refused.stdout, "");
  assert.equal(api.find(path, "DELETE").length, 0, "nothing is sent without confirmation");

  const mistyped = await cli(["storage", "buckets", "delete", "avatars"], { stdin: "uploads\n", isTTY: true });
  assert.equal(mistyped.code, 2);
  assert.equal(api.find(path, "DELETE").length, 0);

  const empty = await cli(["storage", "buckets", "delete", "empty", "--yes", "--json"]);
  assert.equal(empty.code, 0, empty.stderr);
  const plain = api.find(`${BUCKETS}/empty`, "DELETE")[0];
  assert.equal(plain.headers.confirmation, "delete:empty");
  assert.equal(plain.headers.authorization, BEARER);
  assert.deepEqual(plain.query, {}, "deleteObjects is not sent unless asked for");
  assert.equal(plain.text, "");
  assert.deepEqual(JSON.parse(empty.stdout), { objectCount: 0, totalBytes: 0 });

  const conflict = await cli(["storage", "buckets", "delete", "avatars", "--yes"]);
  assert.equal(conflict.code, 5);
  assert.equal(conflict.stdout, "");
  assert.match(conflict.stderr, /error conflict: bucket avatars still holds 3 objects; pass deleteObjects=true/u);
  assert.deepEqual(api.find(path, "DELETE")[0].query, {});

  const forced = await cli(["storage", "buckets", "delete", "avatars", "--yes", "--delete-objects"]);
  assert.equal(forced.code, 0, forced.stderr);
  const withObjects = api.find(path, "DELETE")[1];
  assert.equal(withObjects.headers.confirmation, "delete:avatars");
  assert.deepEqual(withObjects.query, { deleteObjects: "true" });
  assert.match(forced.stdout, /^objectCount\s+3\ntotalBytes\s+12345\n$/u);
  assert.match(forced.stderr, /bucket avatars deleted: 3 objects, 12345 bytes removed/u);

  const typed = await cli(["storage", "buckets", "delete", "avatars", "--delete-objects"], { stdin: "avatars\n", isTTY: true });
  assert.equal(typed.code, 0, typed.stderr);
  assert.equal(api.find(path, "DELETE").length, 3);
});

test("storage objects list maps --prefix/--limit/--cursor to the query and --all follows the cursor", async (t) => {
  const pages = {
    undefined: { items: [object()], nextCursor: "c1" },
    c1: { items: [object({ path: "users/43/me.png", ownerId: null, sizeBytes: 8192 })], nextCursor: null },
  };
  const { api, cli } = await tenantCli(t, {
    [`GET ${BUCKETS}/avatars/objects`]: (request) => ({ status: 200, json: pages[String(request.query.cursor)] }),
  });
  const path = `${BUCKETS}/avatars/objects`;

  const human = await cli(["storage", "objects", "list", "avatars"]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /^path\s+contentType\s+sizeBytes\s+ownerId\s+updatedAt\n/u);
  assert.match(human.stdout, /users\/42\/me\.png\s+image\/png\s+4096\s+usr_42\s+2026-08-06T12:00:00\.000Z/u);
  assert.match(human.stdout, /next cursor: c1\n$/u);
  assert.deepEqual(api.find(path, "GET")[0].query, {}, "no query parameter is invented");
  assert.equal(api.find(path, "GET")[0].headers.authorization, BEARER);

  const filtered = await cli(["storage", "objects", "list", "avatars", "--prefix", "users/", "--limit", "10", "--cursor", "c1", "--json"]);
  assert.equal(filtered.code, 0, filtered.stderr);
  assert.deepEqual(api.find(path, "GET")[1].query, { prefix: "users/", limit: "10", cursor: "c1" });
  assert.deepEqual(JSON.parse(filtered.stdout), pages.c1, "--json is the page verbatim");

  const all = await cli(["storage", "objects", "list", "avatars", "--prefix", "users/", "--all", "--json"]);
  assert.equal(all.code, 0, all.stderr);
  const followed = api.find(path, "GET").slice(2);
  assert.equal(followed.length, 2);
  assert.deepEqual(followed[0].query, { prefix: "users/" });
  assert.deepEqual(followed[1].query, { prefix: "users/", cursor: "c1" }, "the prefix is kept while following");
  const collected = JSON.parse(all.stdout);
  assert.deepEqual(collected.items.map((item) => item.path), ["users/42/me.png", "users/43/me.png"]);
  assert.equal(collected.nextCursor, null);

  const allHuman = await cli(["storage", "objects", "list", "avatars", "--all"]);
  assert.equal(allHuman.code, 0, allHuman.stderr);
  assert.match(allHuman.stdout, /users\/43\/me\.png\s+image\/png\s+8192\s+—/u);
  assert.doesNotMatch(allHuman.stdout, /next cursor/u);

  const badLimit = await cli(["storage", "objects", "list", "avatars", "--limit", "0"]);
  assert.equal(badLimit.code, 2);
  assert.match(badLimit.stderr, /--limit must be between 1 and 1000/u);
  assert.equal(api.find(path, "GET").length, 6);
});

test("storage objects delete is confirmed and encodes the path segment-wise", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`DELETE ${BUCKETS}/avatars/objects/users/42/me.png`]: () => ({ status: 200, json: object() }),
    [`DELETE ${BUCKETS}/avatars/objects/photos/summer%202026/me%23final.png`]: () => ({
      status: 200,
      json: object({ path: "photos/summer 2026/me#final.png" }),
    }),
    [`DELETE ${BUCKETS}/avatars/objects/users/44/me.png`]: () => apiError("not_found", "no such object", 404),
  });
  const path = `${BUCKETS}/avatars/objects/users/42/me.png`;

  const refused = await cli(["storage", "objects", "delete", "avatars", "users/42/me.png"]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to delete object users\/42\/me\.png without --yes/u);
  assert.equal(api.find(path, "DELETE").length, 0, "nothing is sent without confirmation");

  const deleted = await cli(["storage", "objects", "delete", "avatars", "users/42/me.png", "--yes", "--json"]);
  assert.equal(deleted.code, 0, deleted.stderr);
  const sent = api.find(path, "DELETE")[0];
  assert.ok(sent, "the request path keeps the slashes of the object path");
  assert.equal(sent.headers.authorization, BEARER);
  assert.equal(sent.text, "");
  assert.equal(JSON.parse(deleted.stdout).path, "users/42/me.png");

  const typed = await cli(["storage", "objects", "delete", "avatars", "users/42/me.png"], { stdin: "users/42/me.png\n", isTTY: true });
  assert.equal(typed.code, 0, typed.stderr);
  assert.match(typed.stdout, /^path\s+users\/42\/me\.png\n/u);

  const encoded = await cli(["storage", "objects", "delete", "avatars", "photos/summer 2026/me#final.png", "--yes"]);
  assert.equal(encoded.code, 0, encoded.stderr);
  assert.equal(api.find(`${BUCKETS}/avatars/objects/photos/summer%202026/me%23final.png`, "DELETE").length, 1);

  const missing = await cli(["storage", "objects", "delete", "avatars", "users/44/me.png", "--yes"]);
  assert.equal(missing.code, 4);
  assert.match(missing.stderr, /no such object/u);

  for (const bad of ["/users/42/me.png", "users//me.png", "users/../etc/passwd", "users/./me.png"]) {
    const escape = await cli(["storage", "objects", "delete", "avatars", bad, "--yes"]);
    assert.equal(escape.code, 2, bad);
    assert.match(escape.stderr, /<path> must not have empty, \. or \.\. segments/u);
  }
  assert.equal(api.requests.filter((r) => r.method === "DELETE").length, 4, "an unsafe path is never sent");
});
