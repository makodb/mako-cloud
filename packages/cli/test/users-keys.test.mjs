// Application users and project credentials against the loopback mock:
// request paths and bodies, bearer and idempotency headers, one-time secrets
// printed exactly once, destructive confirmation, and exit codes.
import assert from "node:assert/strict";
import { readFile, stat, writeFile } from "node:fs/promises";
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
const USER_ID = "usr_abcdefgh";
const SESSION_ID = "ses_abcdefgh";
const PUBLIC_VALUE = "pkv_0123456789abcdefghijklmnop";
const SERVICE_VALUE = "skv_0123456789abcdefghijklmnop";
const REPLACEMENT_VALUE = "pkv_replacement0123456789abcdef";

function userSummary(overrides = {}) {
  return { id: USER_ID, email: "ann@example.test", status: "active", createdAt: NOW, updatedAt: NOW, ...overrides };
}

function userView(overrides = {}) {
  return {
    ...userSummary(),
    trustedMetadata: { role: "member" },
    profileMetadata: { displayName: "Ann" },
    sessionEpoch: 1,
    sessions: [{ id: SESSION_ID, status: "active", createdAt: NOW, expiresAt: "2099-01-01T00:00:00.000Z" }],
    sessionsTruncated: false,
    ...overrides,
  };
}

function credential(overrides = {}) {
  return { id: "pk_web", kind: "public", state: "active", createdAt: NOW, ...overrides };
}

function signingKey(overrides = {}) {
  return { keyId: "key_0001", state: "active", createdAt: NOW, ...overrides };
}

function router(routes) {
  return (request) => routes[`${request.method} ${request.path}`]?.(request);
}

async function tenantCli(t, routes) {
  const api = await startMockApi(authHandler({ fallback: router(routes) }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const cli = (argv, options = {}) => runCli([...argv, ...TENANT], { configDir: directory, ...options });
  return { api, cli, directory };
}

function occurrences(text, needle) {
  return text.split(needle).length - 1;
}

test("users search passes query and limit, tables the users, and reports truncation", async (t) => {
  const response = { users: [userSummary(), userSummary({ id: "usr_ijklmnop", email: "annie@example.test", status: "disabled" })], truncated: true };
  const { api, cli } = await tenantCli(t, {
    [`GET ${BASE}/users`]: () => ({ status: 200, json: response }),
  });
  const human = await cli(["users", "search", "--query", "ann", "--limit", "10"]);
  assert.equal(human.code, 0, human.stderr);
  const sent = api.find(`${BASE}/users`, "GET")[0];
  assert.deepEqual(sent.query, { query: "ann", limit: "10" });
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(human.stdout, /^id\s+email\s+status\s+createdAt\n/u);
  assert.match(human.stdout, /usr_ijklmnop\s+annie@example.test\s+disabled/u);
  assert.match(human.stderr, /more users match/u);

  const json = await cli(["users", "search", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(api.find(`${BASE}/users`, "GET")[1].query, {});
  assert.deepEqual(JSON.parse(json.stdout), response);
  assert.equal(json.stderr, "");

  const tooMany = await cli(["users", "search", "--limit", "500"]);
  assert.equal(tooMany.code, 2);
  assert.equal(api.find(`${BASE}/users`, "GET").length, 2);
});

test("users create and invite send the address and metadata with an idempotency key", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`POST ${BASE}/users`]: (request) => ({ status: 201, json: userView({ email: request.body.email, ...request.body }) }),
    [`POST ${BASE}/users/invitations`]: (request) => ({
      status: 201,
      json: userView({ email: request.body.email, status: "pending_verification" }),
    }),
  });
  const trustedFile = join(directory, "trusted.json");
  await writeFile(trustedFile, JSON.stringify({ role: "admin", tenant: "acme" }));

  const created = await cli(["users", "create", "--email", "bob@example.test", "--trusted-metadata", `@${trustedFile}`, "--json"]);
  assert.equal(created.code, 0, created.stderr);
  const sent = api.find(`${BASE}/users`, "POST")[0];
  assert.deepEqual(sent.body, {
    email: "bob@example.test",
    trustedMetadata: { role: "admin", tenant: "acme" },
    profileMetadata: {},
  });
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(created.stdout).email, "bob@example.test");

  const invited = await cli(["users", "invite", "--email", "cat@example.test", "--profile-metadata", "-"], {
    stdin: '{"displayName":"Cat"}',
  });
  assert.equal(invited.code, 0, invited.stderr);
  const invitation = api.find(`${BASE}/users/invitations`, "POST")[0];
  assert.deepEqual(invitation.body, { email: "cat@example.test", trustedMetadata: {}, profileMetadata: { displayName: "Cat" } });
  assert.match(invitation.headers["idempotency-key"], UUID);
  assert.match(invited.stdout, /status\s+pending_verification/u);

  const notEmail = await cli(["users", "create", "--email", "bob"]);
  assert.equal(notEmail.code, 2);
  const notObject = await cli(["users", "create", "--email", "bob@example.test", "--trusted-metadata", "[1]"]);
  assert.equal(notObject.code, 2);
  assert.match(notObject.stderr, /JSON object/u);
  assert.equal(api.find(`${BASE}/users`, "POST").length, 1);
});

test("users get and update-metadata; a missing user exits 4", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`GET ${BASE}/users/${USER_ID}`]: () => ({ status: 200, json: userView() }),
    [`GET ${BASE}/users/usr_missing0`]: () => apiError("not_found", "no such user", 404),
    [`PATCH ${BASE}/users/${USER_ID}`]: (request) => ({ status: 200, json: userView(request.body) }),
  });
  const got = await cli(["users", "get", USER_ID]);
  assert.equal(got.code, 0, got.stderr);
  assert.match(got.stdout, /^id\s+usr_abcdefgh\n/u);
  assert.match(got.stdout, /ses_abcdefgh/u);
  const missing = await cli(["users", "get", "usr_missing0"]);
  assert.equal(missing.code, 4);
  assert.equal(missing.stdout, "");

  const body = { trustedMetadata: { role: "owner" }, profileMetadata: { displayName: "Ann B." } };
  const updated = await cli(["users", "update-metadata", USER_ID, "--input", "-", "--json"], { stdin: JSON.stringify(body) });
  assert.equal(updated.code, 0, updated.stderr);
  const patch = api.find(`${BASE}/users/${USER_ID}`, "PATCH")[0];
  assert.deepEqual(patch.body, body);
  assert.equal(patch.headers.authorization, BEARER);
  assert.equal(JSON.parse(updated.stdout).trustedMetadata.role, "owner");

  const partial = await cli(["users", "update-metadata", USER_ID, "--input", '{"trustedMetadata":{}}']);
  assert.equal(partial.code, 2);
  assert.match(partial.stderr, /profileMetadata/u);
  assert.equal(api.find(`${BASE}/users/${USER_ID}`, "PATCH").length, 1);
});

test("user lifecycle actions hit their routes; the destructive ones need --yes", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`POST ${BASE}/users/${USER_ID}/actions/disable`]: () => ({ status: 200, json: userView({ status: "disabled" }) }),
    [`POST ${BASE}/users/${USER_ID}/actions/restore`]: () => ({ status: 200, json: userView() }),
    [`DELETE ${BASE}/users/${USER_ID}`]: () => ({ status: 200, json: userView({ status: "deleted" }) }),
    [`POST ${BASE}/users/${USER_ID}/actions/revoke-sessions`]: () => ({ status: 200, json: userView({ sessionEpoch: 2, sessions: [] }) }),
    [`DELETE ${BASE}/users/${USER_ID}/sessions/${SESSION_ID}`]: () => ({
      status: 200,
      json: userView({ sessions: [{ ...userView().sessions[0], status: "revoked", revokedAt: NOW }] }),
    }),
  });
  const destructive = [
    [["users", "disable", USER_ID], `${BASE}/users/${USER_ID}/actions/disable`, "POST", /disable user usr_abcdefgh/u, "disabled"],
    [["users", "delete", USER_ID], `${BASE}/users/${USER_ID}`, "DELETE", /delete user usr_abcdefgh/u, "deleted"],
    [["users", "revoke-sessions", USER_ID], `${BASE}/users/${USER_ID}/actions/revoke-sessions`, "POST", /revoke every session of user usr_abcdefgh/u, "active"],
    [["users", "revoke-session", USER_ID, SESSION_ID], `${BASE}/users/${USER_ID}/sessions/${SESSION_ID}`, "DELETE", /revoke session ses_abcdefgh/u, "active"],
  ];
  for (const [argv, path, method, pattern, status] of destructive) {
    const refused = await cli(argv);
    assert.equal(refused.code, 2, `${argv.join(" ")}: ${refused.stderr}`);
    assert.match(refused.stderr, pattern);
    assert.equal(refused.stdout, "");
    assert.equal(api.find(path, method).length, 0, `${argv.join(" ")} sent nothing`);
    const done = await cli([...argv, "--yes", "--json"]);
    assert.equal(done.code, 0, done.stderr);
    assert.equal(api.find(path, method).length, 1);
    assert.equal(api.find(path, method)[0].headers.authorization, BEARER);
    assert.equal(JSON.parse(done.stdout).status, status);
  }
  const revoked = JSON.parse((await cli(["users", "revoke-session", USER_ID, SESSION_ID, "--yes", "--json"])).stdout);
  assert.equal(revoked.sessions[0].status, "revoked");

  const restored = await cli(["users", "restore", USER_ID]);
  assert.equal(restored.code, 0, restored.stderr);
  assert.equal(api.find(`${BASE}/users/${USER_ID}/actions/restore`, "POST").length, 1);
  assert.match(restored.stdout, /status\s+active/u);
});

test("public keys print their secret exactly once: block, JSON field, or 0600 file", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`POST ${BASE}/credentials/public`]: (request) => ({
      status: 201,
      json: { credential: credential({ id: request.body.id }), value: PUBLIC_VALUE },
    }),
  });
  const block = await cli(["keys", "public", "create", "--id", "pk_web"]);
  assert.equal(block.code, 0, block.stderr);
  const sent = api.find(`${BASE}/credentials/public`, "POST")[0];
  assert.deepEqual(sent.body, { id: "pk_web" });
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(occurrences(block.stdout, PUBLIC_VALUE), 1);
  assert.match(block.stdout, /public key pk_web \(shown once\)/u);
  assert.match(block.stdout, /^id\s+pk_web\n/u);
  assert.doesNotMatch(block.stderr, /pkv_/u);

  const json = await cli(["keys", "public", "create", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  const parsed = JSON.parse(json.stdout);
  assert.equal(parsed.secret, PUBLIC_VALUE);
  assert.match(parsed.id, /^pk_[0-9a-f]{12}$/u, "an omitted id is generated like the console's");
  assert.equal(api.find(`${BASE}/credentials/public`, "POST")[1].body.id, parsed.id);
  assert.equal(occurrences(json.stdout, PUBLIC_VALUE), 1);
  assert.equal(json.stderr, "");

  const file = join(directory, "public-key.txt");
  const toFile = await cli(["keys", "public", "create", "--id", "pk_ci", "--secret-file", file]);
  assert.equal(toFile.code, 0, toFile.stderr);
  assert.doesNotMatch(toFile.stdout, /pkv_/u);
  assert.doesNotMatch(toFile.stderr, /pkv_/u);
  assert.equal(await readFile(file, "utf8"), `${PUBLIC_VALUE}\n`);
  assert.equal((await stat(file)).mode & 0o777, 0o600);
  assert.doesNotMatch(await readFile(join(directory, "credentials.json"), "utf8"), /pkv_/u);

  const badId = await cli(["keys", "public", "create", "--id", "has space"]);
  assert.equal(badId.code, 2);
  assert.equal(api.find(`${BASE}/credentials/public`, "POST").length, 3);
});

test("service credentials carry their scope and are shown once", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`POST ${BASE}/credentials/service`]: (request) => ({
      status: 201,
      json: {
        credential: credential({ id: request.body.id, kind: "service", serviceScope: request.body.scope }),
        value: SERVICE_VALUE,
      },
    }),
  });
  const issued = await cli([
    "keys", "service", "create", "--id", "sk_worker",
    "--collection", "todos,notes", "--collection", "todos",
    "--operation", "read", "--operation", "update,delete",
  ]);
  assert.equal(issued.code, 0, issued.stderr);
  const sent = api.find(`${BASE}/credentials/service`, "POST")[0];
  assert.deepEqual(sent.body, {
    id: "sk_worker",
    scope: { collections: ["todos", "notes"], operations: ["read", "update", "delete"] },
  });
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(occurrences(issued.stdout, SERVICE_VALUE), 1);
  assert.match(issued.stdout, /service credential sk_worker \(shown once\)/u);
  assert.match(issued.stdout, /serviceScope/u);
  assert.doesNotMatch(issued.stderr, /skv_/u);

  const badOperation = await cli(["keys", "service", "create", "--collection", "todos", "--operation", "admin"]);
  assert.equal(badOperation.code, 2);
  assert.match(badOperation.stderr, /create, read, update, delete/u);
  const noCollection = await cli(["keys", "service", "create", "--operation", "read"]);
  assert.equal(noCollection.code, 2);
  assert.match(noCollection.stderr, /--collection/u);
  assert.equal(api.find(`${BASE}/credentials/service`, "POST").length, 1);
});

test("credentials are read, retired, and rotated with confirmation; the replacement secret prints once", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`GET ${BASE}/credentials/pk_web`]: () => ({ status: 200, json: credential() }),
    [`GET ${BASE}/credentials/pk_gone`]: () => apiError("not_found", "no such credential", 404),
    [`DELETE ${BASE}/credentials/pk_web`]: () => ({ status: 204 }),
    [`POST ${BASE}/credentials/pk_web/actions/rotate`]: (request) => ({
      status: 201,
      json: { credential: credential({ id: request.body.replacementId }), value: REPLACEMENT_VALUE },
    }),
    [`POST ${BASE}/credentials/pk_old/actions/rotate`]: () => apiError("conflict", "credential is retired", 409),
  });
  const got = await cli(["keys", "get", "pk_web", "--json"]);
  assert.equal(got.code, 0, got.stderr);
  assert.deepEqual(JSON.parse(got.stdout), credential());
  const missing = await cli(["keys", "get", "pk_gone"]);
  assert.equal(missing.code, 4);

  const refused = await cli(["keys", "retire", "pk_web"]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to retire credential pk_web without --yes/u);
  assert.equal(api.find(`${BASE}/credentials/pk_web`, "DELETE").length, 0);
  const retired = await cli(["keys", "retire", "pk_web", "--yes"]);
  assert.equal(retired.code, 0, retired.stderr);
  assert.equal(api.find(`${BASE}/credentials/pk_web`, "DELETE")[0].headers.authorization, BEARER);
  assert.equal(retired.stdout, "");
  assert.match(retired.stderr, /pk_web retired/u);
  const retiredJson = await cli(["keys", "retire", "pk_web"], { stdin: "pk_web\n", isTTY: true, env: {} });
  assert.equal(retiredJson.code, 0, retiredJson.stderr);
  const asJson = await cli(["keys", "retire", "pk_web", "--yes", "--json"]);
  assert.deepEqual(JSON.parse(asJson.stdout), { id: "pk_web", state: "retired" });

  const rotatePath = `${BASE}/credentials/pk_web/actions/rotate`;
  const notConfirmed = await cli(["keys", "rotate", "pk_web", "--replacement-id", "pk_web2", "--overlap", "3600"]);
  assert.equal(notConfirmed.code, 2);
  assert.equal(api.find(rotatePath, "POST").length, 0);
  const rotated = await cli(["keys", "rotate", "pk_web", "--replacement-id", "pk_web2", "--overlap", "3600", "--yes"]);
  assert.equal(rotated.code, 0, rotated.stderr);
  const sent = api.find(rotatePath, "POST")[0];
  assert.deepEqual(sent.body, { replacementId: "pk_web2", overlapSeconds: 3600 });
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(occurrences(rotated.stdout, REPLACEMENT_VALUE), 1);
  assert.match(rotated.stdout, /replacement credential pk_web2 \(shown once\)/u);
  assert.doesNotMatch(rotated.stderr, /pkv_/u);

  const file = join(directory, "replacement.txt");
  const toFile = await cli(["keys", "rotate", "pk_web", "--replacement-id", "pk_web3", "--overlap", "0", "--yes", "--secret-file", file]);
  assert.equal(toFile.code, 0, toFile.stderr);
  assert.equal(api.find(rotatePath, "POST")[1].body.overlapSeconds, 0);
  assert.doesNotMatch(toFile.stdout, /pkv_/u);
  assert.equal(await readFile(file, "utf8"), `${REPLACEMENT_VALUE}\n`);
  assert.equal((await stat(file)).mode & 0o777, 0o600);

  const tooLong = await cli(["keys", "rotate", "pk_web", "--replacement-id", "pk_web4", "--overlap", "9999999", "--yes"]);
  assert.equal(tooLong.code, 2);
  assert.equal(api.find(rotatePath, "POST").length, 2);

  const conflict = await cli(["keys", "rotate", "pk_old", "--replacement-id", "pk_new", "--overlap", "60", "--yes"]);
  assert.equal(conflict.code, 5);
  assert.equal(conflict.stdout, "");
  assert.match(conflict.stderr, /credential is retired/u);
});

test("signing keys are initialized, listed, and rotated with confirmation; no secret is ever shown", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`POST ${BASE}/signing-keys/actions/initialize`]: () => ({ status: 201, json: signingKey() }),
    [`GET ${BASE}/signing-keys`]: () => ({
      status: 200,
      json: { items: [signingKey({ keyId: "key_0002" }), signingKey({ state: "retiring", retireAt: "2026-08-07T12:00:00.000Z" })] },
    }),
    [`POST ${BASE}/signing-keys`]: () => ({ status: 201, json: signingKey({ keyId: "key_0002" }) }),
  });
  const initialized = await cli(["keys", "signing", "init", "--json"]);
  assert.equal(initialized.code, 0, initialized.stderr);
  const init = api.find(`${BASE}/signing-keys/actions/initialize`, "POST")[0];
  assert.equal(init.headers.authorization, BEARER);
  assert.match(init.headers["idempotency-key"], UUID);
  assert.equal(init.text, "");
  assert.deepEqual(JSON.parse(initialized.stdout), signingKey());

  const listed = await cli(["keys", "signing", "list"]);
  assert.equal(listed.code, 0, listed.stderr);
  assert.match(listed.stdout, /^keyId\s+state\s+createdAt\s+retireAt\n/u);
  assert.match(listed.stdout, /key_0001\s+retiring\s+\S+\s+2026-08-07T12:00:00.000Z/u);
  assert.doesNotMatch(listed.stdout, /shown once/u);

  const refused = await cli(["keys", "signing", "rotate", "--overlap", "86400"]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to rotate the JWT signing key signing-key without --yes/u);
  assert.equal(api.find(`${BASE}/signing-keys`, "POST").length, 0);
  const rotated = await cli(["keys", "signing", "rotate", "--overlap", "86400", "--yes", "--json"]);
  assert.equal(rotated.code, 0, rotated.stderr);
  const sent = api.find(`${BASE}/signing-keys`, "POST")[0];
  assert.deepEqual(sent.body, { overlapSeconds: 86400 });
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.deepEqual(JSON.parse(rotated.stdout), signingKey({ keyId: "key_0002" }));
  const zero = await cli(["keys", "signing", "rotate", "--overlap", "0", "--yes"]);
  assert.equal(zero.code, 2);
  assert.equal(api.find(`${BASE}/signing-keys`, "POST").length, 1);
});
