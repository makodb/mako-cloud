// Collections, indexes, and policies against the loopback mock: request paths
// and bodies, bearer and idempotency headers, JSON inputs from files and stdin,
// destructive confirmation, output shapes, and exit codes.
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
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

const SCHEMA = {
  version: 0,
  primaryKey: "id",
  type: "object",
  properties: { id: { type: "string", maxLength: 64 }, title: { type: "string" } },
  required: ["id"],
};

function collection(overrides = {}) {
  return {
    id: "todos",
    metadataVersion: 1,
    schemaVersion: 1,
    jsonSchema: SCHEMA,
    primaryKey: { kind: "field", field: "id" },
    compatibility: "compatible",
    state: "active",
    ...overrides,
  };
}

function migration(overrides = {}) {
  return {
    id: "mig_abcdefgh",
    projectId: PROJECT_ID,
    environmentId: ENVIRONMENT_ID,
    collectionId: "todos",
    fromSchemaVersion: 1,
    toSchemaVersion: 2,
    state: "planned",
    reason: "add the done flag",
    compatibilityIssues: [],
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function index(overrides = {}) {
  return {
    collectionId: "todos",
    name: "by_owner",
    version: 1,
    kind: "non_unique",
    fields: [{ path: "ownerId", direction: "ascending" }],
    state: "building",
    activationFenced: false,
    ...overrides,
  };
}

function policySet(overrides = {}) {
  return {
    version: 1,
    state: "draft",
    rules: [{ id: "owner-reads", effect: "allow", operations: ["read"], expression: "doc.ownerId == user.id" }],
    diagnostics: [],
    ...overrides,
  };
}

function activePolicy(overrides = {}) {
  return { defaultDeny: true, authorizationEpoch: 3, policy: policySet({ state: "active" }), ...overrides };
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

test("collections list renders a table, --json the response verbatim, with the bearer token", async (t) => {
  const items = [collection(), collection({ id: "notes", state: "creating", compatibility: "pending_validation" })];
  const { api, cli } = await tenantCli(t, {
    [`GET ${BASE}/collections`]: () => ({ status: 200, json: { items } }),
  });
  const human = await cli(["collections", "list"]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /^id\s+state\s+schemaVersion\s+metadataVersion\s+compatibility\n/u);
  assert.match(human.stdout, /todos\s+active\s+1\s+1\s+compatible/u);
  assert.match(human.stdout, /notes\s+creating/u);
  assert.equal(api.find(`${BASE}/collections`, "GET")[0].headers.authorization, BEARER);

  const json = await cli(["collections", "list", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), items);
});

test("tenant comes from --project/--env or the environment, and is required", async (t) => {
  const { api, directory } = await tenantCli(t, {
    [`GET ${BASE}/collections`]: () => ({ status: 200, json: { items: [] } }),
  });
  const fromEnv = await runCli(["collections", "list"], {
    configDir: directory,
    env: { MAKO_PROJECT_ID: PROJECT_ID, MAKO_ENVIRONMENT_ID: ENVIRONMENT_ID },
  });
  assert.equal(fromEnv.code, 0, fromEnv.stderr);
  assert.equal(fromEnv.stdout, "(none)\n");
  assert.equal(api.find(`${BASE}/collections`, "GET").length, 1);

  const missing = await runCli(["collections", "list", "--project", PROJECT_ID], { configDir: directory });
  assert.equal(missing.code, 2);
  assert.match(missing.stderr, /--env/u);
  assert.equal(api.requests.filter((r) => r.path.startsWith("/v1/projects")).length, 1, "nothing is sent");
});

test("collections create sends the schema, a derived or explicit primary key, and an idempotency key", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`POST ${BASE}/collections`]: (request) => ({
      status: 201,
      json: collection({
        id: request.body.id,
        schemaVersion: request.body.schemaVersion,
        primaryKey: request.body.primaryKey,
        state: "active",
      }),
    }),
  });
  const schemaFile = join(directory, "todos.schema.json");
  await writeFile(schemaFile, JSON.stringify(SCHEMA));

  const fromFile = await cli(["collections", "create", "todos", "--schema", `@${schemaFile}`, "--json"]);
  assert.equal(fromFile.code, 0, fromFile.stderr);
  const first = api.find(`${BASE}/collections`, "POST")[0];
  assert.deepEqual(first.body, {
    id: "todos",
    schemaVersion: 1,
    jsonSchema: SCHEMA,
    primaryKey: { kind: "field", field: "id" },
  });
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(fromFile.stdout).id, "todos");

  const composite = { ...SCHEMA, primaryKey: { key: "id", fields: ["ownerId", "slug"], separator: "|" } };
  const fromStdin = await cli(["collections", "create", "notes", "--schema", "-", "--schema-version", "3"], {
    stdin: JSON.stringify(composite),
  });
  assert.equal(fromStdin.code, 0, fromStdin.stderr);
  const second = api.find(`${BASE}/collections`, "POST")[1];
  assert.equal(second.body.schemaVersion, 3);
  assert.deepEqual(second.body.primaryKey, {
    kind: "composite",
    key: "id",
    fields: ["ownerId", "slug"],
    separator: "|",
  });
  assert.match(second.headers["idempotency-key"], UUID);
  assert.notEqual(second.headers["idempotency-key"], first.headers["idempotency-key"]);
  assert.match(fromStdin.stdout, /^id\s+notes\n/u);

  const explicit = await cli(["collections", "create", "tags", "--schema", '{"type":"object"}', "--primary-key", "name"]);
  assert.equal(explicit.code, 0, explicit.stderr);
  assert.deepEqual(api.find(`${BASE}/collections`, "POST")[2].body.primaryKey, { kind: "field", field: "name" });

  const noKey = await cli(["collections", "create", "tags", "--schema", '{"type":"object"}']);
  assert.equal(noKey.code, 2);
  assert.match(noKey.stderr, /--primary-key/u);
  const badJson = await cli(["collections", "create", "tags", "--schema", "{not json"]);
  assert.equal(badJson.code, 2);
  assert.match(badJson.stderr, /not valid JSON/u);
  assert.equal(api.find(`${BASE}/collections`, "POST").length, 3, "usage errors send nothing");
});

test("collections get shows a record; a missing collection exits 4", async (t) => {
  const { cli } = await tenantCli(t, {
    [`GET ${BASE}/collections/todos`]: () => ({ status: 200, json: collection() }),
    [`GET ${BASE}/collections/ghost`]: () => apiError("not_found", "no such collection", 404),
  });
  const found = await cli(["collections", "get", "todos"]);
  assert.equal(found.code, 0, found.stderr);
  assert.match(found.stdout, /^id\s+todos\n/u);
  assert.match(found.stdout, /compatibility\s+compatible/u);
  const missing = await cli(["collections", "get", "ghost"]);
  assert.equal(missing.code, 4);
  assert.equal(missing.stdout, "");
  assert.match(missing.stderr, /not_found/u);
});

test("collections schema publish reads the schema from stdin and reports migration_required", async (t) => {
  const report = { compatible: false, documentsChecked: 12, issues: ["3 documents lack required field done"] };
  let publication = { status: "published", collection: collection({ schemaVersion: 2, metadataVersion: 2 }) };
  const { api, cli } = await tenantCli(t, {
    [`POST ${BASE}/collections/todos/schemas`]: () => ({ status: 200, json: publication }),
  });
  const next = { ...SCHEMA, version: 1, properties: { ...SCHEMA.properties, done: { type: "boolean" } } };
  const ok = await cli(["collections", "schema", "publish", "todos", "--schema", "-", "--schema-version", "2", "--json"], {
    stdin: JSON.stringify(next),
  });
  assert.equal(ok.code, 0, ok.stderr);
  const sent = api.find(`${BASE}/collections/todos/schemas`, "POST")[0];
  assert.deepEqual(sent.body, { schemaVersion: 2, jsonSchema: next, primaryKey: { kind: "field", field: "id" } });
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.deepEqual(JSON.parse(ok.stdout), publication);
  assert.equal(ok.stderr, "");

  publication = { status: "migration_required", compatibility: report };
  const blocked = await cli(["collections", "schema", "publish", "todos", "--schema", JSON.stringify(next), "--schema-version", "2"]);
  assert.equal(blocked.code, 0, blocked.stderr);
  assert.match(blocked.stdout, /^status\s+migration_required\n/u);
  assert.match(blocked.stdout, /lack required field done/u);
  assert.match(blocked.stderr, /mako collections migrations create todos/u);

  const noVersion = await cli(["collections", "schema", "publish", "todos", "--schema", "{}"]);
  assert.equal(noVersion.code, 2);
  assert.match(noVersion.stderr, /--schema-version/u);
  assert.equal(api.find(`${BASE}/collections/todos/schemas`, "POST").length, 2);
});

test("schema migrations are created from a file, read, and moved between states", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`POST ${BASE}/collections/todos/migrations`]: (request) => ({
      status: 201,
      json: migration({ toSchemaVersion: request.body.targetSchemaVersion, reason: request.body.reason }),
    }),
    [`GET ${BASE}/collections/todos/migrations/mig_abcdefgh`]: () => ({ status: 200, json: migration({ state: "running" }) }),
    [`PATCH ${BASE}/collections/todos/migrations/mig_abcdefgh`]: (request) => ({
      status: 200,
      json: migration({ state: request.body.state }),
    }),
  });
  const target = { ...SCHEMA, version: 1, properties: { ...SCHEMA.properties, done: { type: "boolean" } } };
  const inputFile = join(directory, "migration.json");
  await writeFile(inputFile, JSON.stringify({ targetSchemaVersion: 2, targetJsonSchema: target, reason: "add the done flag" }));

  const created = await cli(["collections", "migrations", "create", "todos", "--input", `@${inputFile}`, "--json"]);
  assert.equal(created.code, 0, created.stderr);
  const sent = api.find(`${BASE}/collections/todos/migrations`, "POST")[0];
  assert.deepEqual(sent.body, {
    targetSchemaVersion: 2,
    targetJsonSchema: target,
    targetPrimaryKey: { kind: "field", field: "id" },
    reason: "add the done flag",
  });
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(created.stdout).id, "mig_abcdefgh");

  const explicitKey = await cli(["collections", "migrations", "create", "todos", "--input", "-"], {
    stdin: JSON.stringify({
      targetSchemaVersion: 2,
      targetJsonSchema: { type: "object" },
      targetPrimaryKey: { kind: "field", field: "key" },
      reason: "rekey",
    }),
  });
  assert.equal(explicitKey.code, 0, explicitKey.stderr);
  assert.deepEqual(api.find(`${BASE}/collections/todos/migrations`, "POST")[1].body.targetPrimaryKey, { kind: "field", field: "key" });

  const incomplete = await cli(["collections", "migrations", "create", "todos", "--input", '{"targetSchemaVersion":2}']);
  assert.equal(incomplete.code, 2);
  assert.match(incomplete.stderr, /targetJsonSchema/u);

  const got = await cli(["collections", "migrations", "get", "todos", "mig_abcdefgh"]);
  assert.equal(got.code, 0, got.stderr);
  assert.match(got.stdout, /state\s+running/u);

  const updated = await cli(["collections", "migrations", "update", "todos", "mig_abcdefgh", "--state", "cancelled", "--json"]);
  assert.equal(updated.code, 0, updated.stderr);
  const patch = api.find(`${BASE}/collections/todos/migrations/mig_abcdefgh`, "PATCH")[0];
  assert.deepEqual(patch.body, { state: "cancelled" });
  assert.equal(patch.headers.authorization, BEARER);
  assert.equal(JSON.parse(updated.stdout).state, "cancelled");

  const badState = await cli(["collections", "migrations", "update", "todos", "mig_abcdefgh", "--state", "done"]);
  assert.equal(badState.code, 2);
  assert.match(badState.stderr, /planned, running, failed, completed, cancelled/u);
  assert.equal(api.find(`${BASE}/collections/todos/migrations/mig_abcdefgh`, "PATCH").length, 1);
});

test("indexes are listed, created from --field options, read, and deleted only with confirmation", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`GET ${BASE}/collections/todos/indexes`]: () => ({
      status: 200,
      json: { items: [index(), index({ name: "by_slug", version: 2, kind: "unique", state: "active" })] },
    }),
    [`POST ${BASE}/collections/todos/indexes`]: (request) => ({ status: 201, json: index(request.body) }),
    [`GET ${BASE}/collections/todos/indexes/by_owner/1`]: () => ({
      status: 200,
      json: index({ state: "failed", failure: { code: "duplicate_values", affectedValues: 2, message: "dup" } }),
    }),
    [`DELETE ${BASE}/collections/todos/indexes/by_owner/1`]: () => ({ status: 200, json: index({ state: "deleting" }) }),
    [`DELETE ${BASE}/collections/todos/indexes/by_slug/2`]: () => apiError("conflict", "index is fenced", 409),
  });
  const listed = await cli(["indexes", "list", "todos"]);
  assert.equal(listed.code, 0, listed.stderr);
  assert.match(listed.stdout, /^name\s+version\s+kind\s+state\s+fenced\n/u);
  assert.match(listed.stdout, /by_slug\s+2\s+unique\s+active\s+false/u);

  const created = await cli([
    "indexes", "create", "todos", "--name", "by_owner_created", "--version", "1",
    "--field", "ownerId", "--field", "createdAt:desc", "--unique", "--json",
  ]);
  assert.equal(created.code, 0, created.stderr);
  const sent = api.find(`${BASE}/collections/todos/indexes`, "POST")[0];
  assert.deepEqual(sent.body, {
    name: "by_owner_created",
    version: 1,
    kind: "unique",
    fields: [
      { path: "ownerId", direction: "ascending" },
      { path: "createdAt", direction: "descending" },
    ],
  });
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(created.stdout).name, "by_owner_created");

  const badField = await cli(["indexes", "create", "todos", "--name", "x", "--version", "1", "--field", "a:sideways"]);
  assert.equal(badField.code, 2);
  const noField = await cli(["indexes", "create", "todos", "--name", "x", "--version", "1"]);
  assert.equal(noField.code, 2);
  assert.match(noField.stderr, /--field/u);
  assert.equal(api.find(`${BASE}/collections/todos/indexes`, "POST").length, 1);

  const got = await cli(["indexes", "get", "todos", "by_owner", "1", "--json"]);
  assert.equal(got.code, 0, got.stderr);
  assert.equal(JSON.parse(got.stdout).failure.code, "duplicate_values");
  const badVersion = await cli(["indexes", "get", "todos", "by_owner", "one"]);
  assert.equal(badVersion.code, 2);

  const refused = await cli(["indexes", "delete", "todos", "by_owner", "1"]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to delete index by_owner\/1 without --yes/u);
  assert.equal(api.find(`${BASE}/collections/todos/indexes/by_owner/1`, "DELETE").length, 0);

  const deleted = await cli(["indexes", "delete", "todos", "by_owner", "1", "--yes", "--json"]);
  assert.equal(deleted.code, 0, deleted.stderr);
  assert.equal(api.find(`${BASE}/collections/todos/indexes/by_owner/1`, "DELETE").length, 1);
  assert.equal(JSON.parse(deleted.stdout).state, "deleting");

  const typed = await cli(["indexes", "delete", "todos", "by_owner", "1"], { stdin: "by_owner/1\n", isTTY: true });
  assert.equal(typed.code, 0, typed.stderr);

  const conflict = await cli(["indexes", "delete", "todos", "by_slug", "2", "--yes"]);
  assert.equal(conflict.code, 5);
  assert.equal(conflict.stdout, "");
  assert.match(conflict.stderr, /index is fenced/u);
});

test("policies are read, drafted from stdin, validated, and tested from a file", async (t) => {
  const examples = [
    { operation: "read", identity: { role: "user", userId: "usr_owner", trustedClaims: {} }, oldDocument: { ownerId: "usr_owner" } },
    { operation: "delete", identity: { role: "user", trustedClaims: {} }, oldDocument: { ownerId: "usr_other" } },
  ];
  const results = [
    { allowed: true, code: "allowed", matchedRuleIds: ["owner-reads"], evaluatedRules: 1 },
    { allowed: false, code: "default_deny", matchedRuleIds: [], evaluatedRules: 1 },
  ];
  const { api, cli, directory } = await tenantCli(t, {
    [`GET ${BASE}/collections/todos/policies`]: () => ({ status: 200, json: activePolicy() }),
    [`GET ${BASE}/collections/todos/policies/2`]: () => ({ status: 200, json: policySet({ version: 2 }) }),
    [`GET ${BASE}/collections/todos/policies/9`]: () => apiError("not_found", "no such policy version", 404),
    [`POST ${BASE}/collections/todos/policies`]: (request) => ({ status: 201, json: policySet(request.body) }),
    [`POST ${BASE}/collections/todos/policies/2/actions/validate`]: () => ({
      status: 200,
      json: { valid: true, policy: policySet({ version: 2, state: "validated" }) },
    }),
    [`POST ${BASE}/collections/todos/policies/2/actions/test`]: () => ({ status: 200, json: { results } }),
  });

  const active = await cli(["policies", "get", "todos", "--json"]);
  assert.equal(active.code, 0, active.stderr);
  assert.deepEqual(JSON.parse(active.stdout), activePolicy());
  assert.equal(api.find(`${BASE}/collections/todos/policies`, "GET")[0].headers.authorization, BEARER);
  const versioned = await cli(["policies", "get", "todos", "--version", "2"]);
  assert.equal(versioned.code, 0, versioned.stderr);
  assert.match(versioned.stdout, /^version\s+2\n/u);
  assert.equal(api.find(`${BASE}/collections/todos/policies/2`, "GET").length, 1);
  const missing = await cli(["policies", "get", "todos", "--version", "9"]);
  assert.equal(missing.code, 4);

  const draft = await cli(["policies", "draft", "todos", "--input", "-", "--json"], {
    stdin: JSON.stringify({ version: 2, rules: policySet().rules }),
  });
  assert.equal(draft.code, 0, draft.stderr);
  const sent = api.find(`${BASE}/collections/todos/policies`, "POST")[0];
  assert.deepEqual(sent.body, { version: 2, rules: policySet().rules });
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(draft.stdout).version, 2);
  const noRules = await cli(["policies", "draft", "todos", "--input", '{"version":2,"rules":[]}']);
  assert.equal(noRules.code, 2);
  assert.match(noRules.stderr, /rules/u);
  assert.equal(api.find(`${BASE}/collections/todos/policies`, "POST").length, 1);

  const validated = await cli(["policies", "validate", "todos", "2", "--json"]);
  assert.equal(validated.code, 0, validated.stderr);
  assert.equal(JSON.parse(validated.stdout).valid, true);
  assert.equal(api.find(`${BASE}/collections/todos/policies/2/actions/validate`, "POST")[0].text, "");

  const examplesFile = join(directory, "examples.json");
  await writeFile(examplesFile, JSON.stringify(examples));
  const tested = await cli(["policies", "test", "todos", "2", "--examples", `@${examplesFile}`]);
  assert.equal(tested.code, 0, tested.stderr);
  assert.deepEqual(api.find(`${BASE}/collections/todos/policies/2/actions/test`, "POST")[0].body, { examples });
  assert.match(tested.stdout, /^allowed\s+code\s+evaluatedRules\s+matchedRuleIds\n/u);
  assert.match(tested.stdout, /true\s+allowed\s+1\s+\["owner-reads"\]/u);
  assert.match(tested.stdout, /false\s+default_deny/u);

  const wrapped = await cli(["policies", "test", "todos", "2", "--examples", JSON.stringify({ examples }), "--json"]);
  assert.equal(wrapped.code, 0, wrapped.stderr);
  assert.deepEqual(JSON.parse(wrapped.stdout), results);
  assert.deepEqual(api.find(`${BASE}/collections/todos/policies/2/actions/test`, "POST")[1].body, { examples });
});

test("policy activation and rollback are confirmed, idempotent, and refused on conflict", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`POST ${BASE}/collections/todos/policies/2/actions/activate`]: () => ({
      status: 200,
      json: activePolicy({ authorizationEpoch: 4, policy: policySet({ version: 2, state: "active" }) }),
    }),
    [`POST ${BASE}/collections/todos/policies/1/actions/rollback`]: () => ({
      status: 200,
      json: activePolicy({ authorizationEpoch: 5 }),
    }),
    [`POST ${BASE}/collections/todos/policies/3/actions/activate`]: () =>
      apiError("conflict", "validate version 3 first", 409),
  });
  const activatePath = `${BASE}/collections/todos/policies/2/actions/activate`;

  const refused = await cli(["policies", "activate", "todos", "2"]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to activate the policy of collection todos without --yes/u);
  assert.equal(refused.stdout, "");
  assert.equal(api.find(activatePath, "POST").length, 0, "nothing is sent without confirmation");

  const activated = await cli(["policies", "activate", "todos", "2", "--yes", "--json"]);
  assert.equal(activated.code, 0, activated.stderr);
  const sent = api.find(activatePath, "POST")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(activated.stdout).authorizationEpoch, 4);

  const mistyped = await cli(["policies", "activate", "todos", "2"], { stdin: "notes\n", isTTY: true });
  assert.equal(mistyped.code, 2);
  assert.equal(api.find(activatePath, "POST").length, 1);

  const rolledBack = await cli(["policies", "rollback", "todos", "1"], { stdin: "todos\n", isTTY: true });
  assert.equal(rolledBack.code, 0, rolledBack.stderr);
  const rollback = api.find(`${BASE}/collections/todos/policies/1/actions/rollback`, "POST")[0];
  assert.match(rollback.headers["idempotency-key"], UUID);
  assert.match(rolledBack.stdout, /authorizationEpoch\s+5/u);

  const conflict = await cli(["policies", "activate", "todos", "3", "--yes"]);
  assert.equal(conflict.code, 5);
  assert.equal(conflict.stdout, "");
  assert.match(conflict.stderr, /conflict: validate version 3 first/u);
});
