// `envs promote` against the loopback mock: the plan it prints without
// changing anything, and the requests `--apply` makes, in order.
import assert from "node:assert/strict";
import test from "node:test";

import { authHandler, environment, NOW, PROJECT_ID, runCli, signedIn, startMockApi } from "./harness.mjs";

const DEV = "env_develop01";
const PROD = "env_product01";
const base = (env) => `/v1/projects/${PROJECT_ID}/environments/${env}`;

const schema = (fields) => ({
  type: "object",
  properties: { id: { type: "string" }, ...fields },
  required: ["id"],
});
const collection = (overrides) => ({
  id: "todos",
  metadataVersion: 1,
  schemaVersion: 1,
  jsonSchema: schema({}),
  primaryKey: { kind: "field", field: "id" },
  compatibility: "compatible",
  state: "active",
  ...overrides,
});
const rules = [{ id: "owner-read", effect: "allow", operations: ["read"], expression: "old.ownerId == identity.user_id" }];
const bucket = {
  id: "avatars",
  access: "policy",
  maxObjectBytes: 1048576,
  allowedContentTypes: ["image/*"],
  rules: [],
  version: 1,
  objectCount: 0,
  totalBytes: 0,
  createdAt: NOW,
  updatedAt: NOW,
};
const index = {
  collectionId: "todos",
  name: "by_owner",
  version: 1,
  kind: "non_unique",
  fields: [{ path: "ownerId", direction: "ascending" }],
  state: "active",
  activationFenced: false,
};

/** Development is a version ahead with an index, a policy, and a bucket; Production has none of them. */
function routes() {
  const target = { schemaVersion: 1, jsonSchema: schema({}), policy: null, indexes: [], buckets: [] };
  const table = {
    [`GET /v1/projects/${PROJECT_ID}/environments/${DEV}`]: () => ({ status: 200, json: environment({ id: DEV }) }),
    [`GET /v1/projects/${PROJECT_ID}/environments/${PROD}`]: () => ({
      status: 200,
      json: environment({ id: PROD, name: "production" }),
    }),
    [`GET ${base(DEV)}/collections`]: () => ({
      status: 200,
      json: { items: [collection({ schemaVersion: 2, jsonSchema: schema({ ownerId: { type: "string" } }) })] },
    }),
    [`GET ${base(PROD)}/collections`]: () => ({
      status: 200,
      json: { items: [collection({ schemaVersion: target.schemaVersion, jsonSchema: target.jsonSchema })] },
    }),
    [`GET ${base(DEV)}/collections/todos/indexes`]: () => ({ status: 200, json: { items: [index] } }),
    [`GET ${base(PROD)}/collections/todos/indexes`]: () => ({ status: 200, json: { items: target.indexes } }),
    [`GET ${base(DEV)}/collections/todos/policies`]: () => ({
      status: 200,
      json: { defaultDeny: true, authorizationEpoch: 1, policy: { version: 3, state: "active", rules, diagnostics: [] } },
    }),
    [`GET ${base(PROD)}/collections/todos/policies`]: () => ({
      status: 200,
      json: { defaultDeny: true, authorizationEpoch: 1, ...(target.policy ? { policy: target.policy } : {}) },
    }),
    [`GET ${base(DEV)}/storage-buckets`]: () => ({ status: 200, json: { items: [bucket] } }),
    [`GET ${base(PROD)}/storage-buckets`]: () => ({ status: 200, json: { items: target.buckets } }),
    [`GET ${base(DEV)}/functions`]: () => ({ status: 200, json: { items: [] } }),
    [`GET ${base(PROD)}/functions`]: () => ({ status: 200, json: { items: [] } }),
    [`POST ${base(PROD)}/collections/todos/schemas`]: (request) => {
      target.schemaVersion = request.body.schemaVersion;
      target.jsonSchema = request.body.jsonSchema;
      return { status: 200, json: { status: "published", collection: collection({ schemaVersion: 2 }) } };
    },
    [`POST ${base(PROD)}/collections/todos/indexes`]: (request) => {
      target.indexes = [{ ...index, ...request.body, state: "building" }];
      return { status: 202, json: target.indexes[0] };
    },
    [`POST ${base(PROD)}/collections/todos/policies`]: (request) => ({
      status: 201,
      json: { version: request.body.version, state: "draft", rules: request.body.rules, diagnostics: [] },
    }),
    [`POST ${base(PROD)}/collections/todos/policies/4/actions/validate`]: () => ({
      status: 200,
      json: { valid: true, policy: { version: 4, state: "draft", rules, diagnostics: [] } },
    }),
    [`POST ${base(PROD)}/collections/todos/policies/4/actions/activate`]: () => {
      target.policy = { version: 4, state: "active", rules, diagnostics: [] };
      return { status: 200, json: { defaultDeny: true, authorizationEpoch: 2, policy: target.policy } };
    },
    [`POST ${base(PROD)}/storage-buckets`]: (request) => {
      target.buckets = [{ ...bucket, ...request.body }];
      return { status: 201, json: target.buckets[0] };
    },
  };
  return (request) => table[`${request.method} ${request.path}`]?.(request);
}

async function promoteCli(t) {
  const api = await startMockApi(authHandler({ fallback: routes() }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const cli = (extra) =>
    runCli(["envs", "promote", "--project", PROJECT_ID, "--from", DEV, "--to", PROD, ...extra], {
      configDir: directory,
    });
  return { api, cli };
}

test("envs promote plans without changing anything", async (t) => {
  const { api, cli } = await promoteCli(t);
  const result = await cli(["--json"]);
  assert.equal(result.code, 0, result.stderr);
  assert.deepEqual(
    JSON.parse(result.stdout).map((step) => [step.kind, step.resource]),
    [
      ["publish_schema", "todos"],
      ["create_index", "todos/by_owner"],
      ["activate_policy", "todos"],
      ["create_bucket", "avatars"],
    ],
  );
  assert.deepEqual(
    api.requests.filter((request) => request.method !== "GET" && !request.path.startsWith("/v1/developer-auth")),
    [],
    "a plan sends no writes",
  );
});

test("envs promote --apply brings the target up to date, then plans nothing", async (t) => {
  const { api, cli } = await promoteCli(t);
  const applied = await cli(["--apply", "--json"]);
  assert.equal(applied.code, 0, applied.stderr);
  assert.deepEqual(
    JSON.parse(applied.stdout).map((step) => step.result),
    ["published", "created", "activated as v4", "created"],
  );
  const writes = api.requests
    .filter((request) => request.method !== "GET" && !request.path.startsWith("/v1/developer-auth"))
    .map((request) => `${request.method} ${request.path.replace(base(PROD), "")}`);
  assert.deepEqual(writes, [
    "POST /collections/todos/schemas",
    "POST /collections/todos/indexes",
    "POST /collections/todos/policies",
    "POST /collections/todos/policies/4/actions/validate",
    "POST /collections/todos/policies/4/actions/activate",
    "POST /storage-buckets",
  ]);
  assert.ok(
    api.requests.every((request) => request.method === "GET" || !request.path.startsWith(base(DEV))),
    "the source is only read",
  );
  const again = await cli(["--json"]);
  assert.equal(again.code, 0, again.stderr);
  assert.deepEqual(JSON.parse(again.stdout), [], "nothing left to promote");
});

test("envs promote refuses the same environment on both sides", async (t) => {
  const { cli } = await promoteCli(t);
  const result = await runCli(["envs", "promote", "--project", PROJECT_ID, "--from", DEV, "--to", DEV]);
  assert.equal(result.code, 2);
  assert.match(result.stderr, /same environment/u);
  void cli;
});

test("envs promote --collection limits the plan to the named collections", async (t) => {
  const { cli } = await promoteCli(t);
  const only = await cli(["--collection", "todos", "--json"]);
  assert.equal(only.code, 0, only.stderr);
  assert.deepEqual(
    JSON.parse(only.stdout).map((step) => step.kind),
    ["publish_schema", "create_index", "activate_policy"],
    "the bucket is left out when only a collection is named",
  );
  const unknown = await cli(["--collection", "nothing-here"]);
  assert.equal(unknown.code, 2);
  assert.match(unknown.stderr, /has no collection nothing-here/u);
});
