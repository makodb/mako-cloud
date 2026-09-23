// `mako-cloud functions …`: functions, deployments, logs, secrets, the test route,
// and the composed `functions deploy` flow, against the loopback mock API.
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  apiError,
  authHandler,
  configDir,
  ENVIRONMENT_ID,
  NOW,
  PROJECT_ID,
  runCli,
  signedIn,
  startMockApi,
} from "./harness.mjs";

const BASE = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const DIGEST = `sha256:${"ab".repeat(32)}`;
const PINNED_RUNTIME = JSON.parse(
  await readFile(new URL("../runtime/runtime-pin.json", import.meta.url), "utf8"),
).release;

function configuration(overrides = {}) {
  return {
    verifyJwt: true,
    regions: ["local"],
    secretNames: [],
    limits: {
      cpuMilliseconds: 1000,
      wallMilliseconds: 10000,
      memoryBytes: 134217728,
      requestBytes: 1048576,
      responseBytes: 1048576,
      concurrency: 4,
    },
    ...overrides,
  };
}

function edgeFunction(overrides = {}) {
  return {
    name: "hello",
    state: "active",
    activeVersion: 1,
    configuration: configuration(),
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function deployment(overrides = {}) {
  return {
    functionName: "hello",
    version: 1,
    bundleDigest: DIGEST,
    bundleFormat: "source_archive_v1",
    bundleSizeBytes: 128,
    entrypoint: "index.ts",
    runtimeVersion: PINNED_RUNTIME,
    configuration: configuration(),
    secretVersions: [],
    state: "healthy",
    diagnostic: null,
    createdAt: NOW,
    ...overrides,
  };
}

function artifact(overrides = {}) {
  return {
    digest: DIGEST,
    format: "source_archive_v1",
    entrypoint: "index.ts",
    sizeBytes: 128,
    moduleCount: 2,
    createdAt: NOW,
    ...overrides,
  };
}

function secret(overrides = {}) {
  return { name: "API_KEY", version: 1, state: "active", createdAt: NOW, ...overrides };
}

/** A mock of the function routes with a little state; `state.calls` lists `METHOD path` in order. */
function functionsApi(state) {
  return (request) => {
    const { method, path, body } = request;
    if (!path.startsWith(BASE)) return undefined;
    const rest = path.slice(BASE.length);
    state.calls.push(`${method} ${rest}`);
    if (rest === "/functions" && method === "GET") {
      return { status: 200, json: { items: state.functions } };
    }
    if (rest === "/functions" && method === "POST") {
      const created = edgeFunction({ name: body.name, configuration: body.configuration, activeVersion: null });
      state.functions.push(created);
      return { status: 201, json: created };
    }
    if (rest === "/functions/hello" && method === "GET") {
      const found = state.functions.find((item) => item.name === "hello");
      return found ? { status: 200, json: found } : apiError("not_found", "no such function", 404);
    }
    if (rest === "/functions/hello" && method === "PATCH") {
      return { status: 200, json: edgeFunction({ configuration: body }) };
    }
    if (rest === "/functions/hello" && method === "DELETE") {
      return { status: 200, json: edgeFunction({ state: "deleting" }) };
    }
    if (rest === "/function-bundles" && method === "POST") {
      return state.upload ?? { status: 201, json: { status: "ready", artifact: artifact(), diagnostics: [] } };
    }
    if (rest === "/functions/hello/versions" && method === "GET") {
      return { status: 200, json: { items: state.deployments } };
    }
    if (rest === "/functions/hello/versions" && method === "POST") {
      const created = deployment({ version: body.version, bundleDigest: body.bundleDigest, entrypoint: body.entrypoint, runtimeVersion: body.runtimeVersion });
      state.deployments.push(created);
      return { status: 201, json: created };
    }
    const versioned = /^\/functions\/hello\/versions\/(\d+)(?:\/actions\/([a-z-]+))?$/u.exec(rest);
    if (versioned) {
      const version = Number(versioned[1]);
      const action = versioned[2];
      if (action === undefined && method === "GET") return { status: 200, json: deployment({ version }) };
      if (action === undefined && method === "DELETE") return { status: 204 };
      if (action === "health-check") {
        const stored = state.deployments.find((item) => item.version === version) ?? deployment({ version });
        return { status: 200, json: { ...stored, ...(state.health ?? {}) } };
      }
      if (action === "promote" || action === "rollback") {
        return { status: 200, json: edgeFunction({ activeVersion: version }) };
      }
    }
    if (rest === "/functions/hello/actions/test" && method === "POST") {
      return { status: 200, json: { status: 201, headers: { "content-type": "text/plain" }, body: Buffer.from("created!").toString("base64"), correlationId: "corr_1" } };
    }
    if (rest === "/functions/hello/logs" && method === "GET") {
      const page = state.logs[request.query.cursor ?? ""];
      return page ? { status: 200, json: page } : apiError("not_found", "bad cursor", 404);
    }
    if (rest === "/function-secrets" && method === "POST") {
      return { status: 201, json: { secret: secret({ name: body.name }), value: "fs_live_do_not_log_0001" } };
    }
    if (rest === "/function-secrets/SERVICE_KEY" && method === "PUT") {
      return { status: 201, json: secret({ name: "SERVICE_KEY" }) };
    }
    if (rest === "/function-secrets/API_KEY" && method === "GET") return { status: 200, json: secret() };
    if (rest === "/function-secrets/API_KEY" && method === "DELETE") return { status: 200, json: secret({ state: "retired" }) };
    if (rest === "/function-secrets/API_KEY/actions/rotate" && method === "POST") {
      return { status: 201, json: { secret: secret({ version: 2 }), value: "fs_live_do_not_log_0002" } };
    }
    return undefined;
  };
}

function freshState() {
  return {
    calls: [],
    functions: [edgeFunction()],
    deployments: [deployment()],
    logs: {
      "": {
        items: [
          { timestamp: NOW, level: "info", message: "hello from v1", correlationId: "corr_a", version: 1, region: "local" },
        ],
        nextCursor: "c1",
      },
      c1: {
        items: [
          { timestamp: NOW, level: "error", message: 'boom {"not":"json-escaped"}', correlationId: "corr_b", version: 1, region: "local" },
        ],
        nextCursor: null,
      },
    },
  };
}

async function setup(t, state = freshState()) {
  const api = await startMockApi(authHandler({ fallback: functionsApi(state) }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  state.calls.length = 0;
  const cli = (argv, options = {}) => runCli(argv, { configDir: directory, ...options });
  return { api, state, cli, directory };
}

test("functions list, get, create, update, and delete", async (t) => {
  const { api, state, cli } = await setup(t);

  const list = await cli(["functions", "list", ...TENANT]);
  assert.equal(list.code, 0, list.stderr);
  assert.match(list.stdout, /^name\s+state\s+activeVersion\s+updatedAt\n/u);
  assert.match(list.stdout, /hello\s+active\s+1/u);

  const get = await cli(["functions", "get", "hello", ...TENANT, "--json"]);
  assert.equal(get.code, 0, get.stderr);
  assert.equal(JSON.parse(get.stdout).name, "hello");

  const create = await cli([
    "functions", "create", "greeter", ...TENANT, "--region", "local", "--region", "eu", "--secret", "API_KEY", "--no-verify-jwt", "--concurrency", "2", "--json",
  ]);
  assert.equal(create.code, 0, create.stderr);
  const createRequest = api.find(`${BASE}/functions`, "POST")[0];
  assert.deepEqual(createRequest.body, {
    name: "greeter",
    configuration: configuration({ verifyJwt: false, regions: ["local", "eu"], secretNames: ["API_KEY"], limits: { ...configuration().limits, concurrency: 2 } }),
  });
  assert.match(createRequest.headers["idempotency-key"], /^[0-9a-f-]{36}$/u);
  assert.equal(JSON.parse(create.stdout).name, "greeter");

  const fromInput = await cli(["functions", "create", "greeter", ...TENANT, "--input", JSON.stringify(configuration({ regions: ["eu"] }))]);
  assert.equal(fromInput.code, 0, fromInput.stderr);
  assert.deepEqual(api.find(`${BASE}/functions`, "POST")[1].body, { name: "greeter", configuration: configuration({ regions: ["eu"] }) });
  const mismatch = await cli(["functions", "create", "greeter", ...TENANT, "--input", JSON.stringify({ name: "other", configuration: configuration() })]);
  assert.equal(mismatch.code, 2);
  const noRegion = await cli(["functions", "create", "greeter", ...TENANT]);
  assert.equal(noRegion.code, 2);
  assert.match(noRegion.stderr, /--region/u);

  const update = await cli(["functions", "update", "hello", ...TENANT, "--config", "-", "--json"], { stdin: JSON.stringify(configuration({ regions: ["eu"] })) });
  assert.equal(update.code, 0, update.stderr);
  assert.deepEqual(api.find(`${BASE}/functions/hello`, "PATCH")[0].body, configuration({ regions: ["eu"] }));
  assert.deepEqual(JSON.parse(update.stdout).configuration.regions, ["eu"]);

  const refused = await cli(["functions", "delete", "hello", ...TENANT]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /without --yes/u);
  assert.equal(api.find(`${BASE}/functions/hello`, "DELETE").length, 0);
  const deleted = await cli(["functions", "delete", "hello", ...TENANT, "--yes"]);
  assert.equal(deleted.code, 0, deleted.stderr);
  assert.equal(api.find(`${BASE}/functions/hello`, "DELETE").length, 1);
  assert.match(deleted.stdout, /state\s+deleting/u);
  assert.ok(state.calls.includes("DELETE /functions/hello"));

  const noTenant = await cli(["functions", "list"]);
  assert.equal(noTenant.code, 2);
  assert.match(noTenant.stderr, /--project/u);
});

test("deployments: list, get, create, health, delete, and confirmed promote and rollback", async (t) => {
  const { api, cli } = await setup(t);

  const list = await cli(["functions", "deployments", "list", "hello", ...TENANT]);
  assert.equal(list.code, 0, list.stderr);
  assert.match(list.stdout, /^version\s+state\s+bundleDigest/u);
  assert.match(list.stdout, /1\s+healthy\s+sha256:/u);

  const get = await cli(["functions", "deployments", "get", "hello", "1", ...TENANT, "--json"]);
  assert.equal(get.code, 0, get.stderr);
  assert.equal(JSON.parse(get.stdout).version, 1);
  const badVersion = await cli(["functions", "deployments", "get", "hello", "one", ...TENANT]);
  assert.equal(badVersion.code, 2);

  const create = await cli(["functions", "deployments", "create", "hello", ...TENANT, "--bundle", DIGEST, "--json"]);
  assert.equal(create.code, 0, create.stderr);
  assert.deepEqual(api.find(`${BASE}/functions/hello/versions`, "POST")[0].body, {
    version: 2,
    bundleDigest: DIGEST,
    entrypoint: "index.ts",
    runtimeVersion: PINNED_RUNTIME,
  });
  assert.equal(JSON.parse(create.stdout).version, 2);

  const health = await cli(["functions", "deployments", "health", "hello", "2", ...TENANT]);
  assert.equal(health.code, 0, health.stderr);
  assert.equal(api.find(`${BASE}/functions/hello/versions/2/actions/health-check`, "POST").length, 1);
  assert.match(health.stdout, /state\s+healthy/u);

  for (const action of ["promote", "rollback"]) {
    const refused = await cli(["functions", "deployments", action, "hello", "2", ...TENANT]);
    assert.equal(refused.code, 2, refused.stderr);
    assert.match(refused.stderr, /hello@2/u);
    assert.equal(api.find(`${BASE}/functions/hello/versions/2/actions/${action}`, "POST").length, 0);

    const mistyped = await cli(["functions", "deployments", action, "hello", "2", ...TENANT], { isTTY: true, stdin: "hello@3\n" });
    assert.equal(mistyped.code, 2);
    assert.equal(api.find(`${BASE}/functions/hello/versions/2/actions/${action}`, "POST").length, 0);

    const typed = await cli(["functions", "deployments", action, "hello", "2", ...TENANT, "--json"], { isTTY: true, stdin: "hello@2\n" });
    assert.equal(typed.code, 0, typed.stderr);
    assert.equal(JSON.parse(typed.stdout).activeVersion, 2);

    const yes = await cli(["functions", "deployments", action, "hello", "2", ...TENANT, "--yes"]);
    assert.equal(yes.code, 0, yes.stderr);
    const requests = api.find(`${BASE}/functions/hello/versions/2/actions/${action}`, "POST");
    assert.equal(requests.length, 2);
    assert.match(requests[1].headers["idempotency-key"], /^[0-9a-f-]{36}$/u);
    assert.match(yes.stdout, /activeVersion\s+2/u);
  }

  const refusedDelete = await cli(["functions", "deployments", "delete", "hello", "1", ...TENANT]);
  assert.equal(refusedDelete.code, 2);
  const deleted = await cli(["functions", "deployments", "delete", "hello", "1", ...TENANT, "--yes", "--json"]);
  assert.equal(deleted.code, 0, deleted.stderr);
  assert.equal(api.find(`${BASE}/functions/hello/versions/1`, "DELETE").length, 1);
  assert.deepEqual(JSON.parse(deleted.stdout), { functionName: "hello", version: 1, deleted: true });
});

test("logs print one plain line per entry and --all follows cursors", async (t) => {
  const { api, cli } = await setup(t);

  const one = await cli(["functions", "logs", "hello", ...TENANT, "--limit", "50"]);
  assert.equal(one.code, 0, one.stderr);
  assert.equal(one.stdout, `${NOW} info v1 local corr_a hello from v1\nnext cursor: c1\n`);
  assert.equal(api.find(`${BASE}/functions/hello/logs`)[0].query.limit, "50");

  const all = await cli(["functions", "logs", "hello", ...TENANT, "--all", "--json"]);
  assert.equal(all.code, 0, all.stderr);
  const parsed = JSON.parse(all.stdout);
  assert.equal(parsed.items.length, 2);
  assert.equal(parsed.nextCursor, null);
  assert.equal(parsed.items[1].message, 'boom {"not":"json-escaped"}');
  const requests = api.find(`${BASE}/functions/hello/logs`);
  assert.deepEqual(requests.slice(1).map((r) => r.query.cursor ?? null), [null, "c1"]);

  const resumed = await cli(["functions", "logs", "hello", ...TENANT, "--cursor", "c1"]);
  assert.equal(resumed.code, 0, resumed.stderr);
  assert.equal(resumed.stdout, `${NOW} error v1 local corr_b boom {"not":"json-escaped"}\n`);

  const badLimit = await cli(["functions", "logs", "hello", ...TENANT, "--limit", "0"]);
  assert.equal(badLimit.code, 2);
});

test("a supplied secret value is sent write-once and never echoed", async (t) => {
  // The pattern the docs describe: a scoped service credential a function
  // needs, stored under the name the function reads. Before this, the only
  // way to give a function a credential was to write it into the uploaded
  // bundle -- a secret at rest in a stored artifact.
  const { api, cli, directory } = await setup(t);
  const value = "mako_sk.key_households.do_not_log_0001";

  const inline = await cli(["functions", "secrets", "create", "SERVICE_KEY", ...TENANT, "--value", value]);
  assert.equal(inline.code, 0, inline.stderr);
  const [put] = api.find(`${BASE}/function-secrets/SERVICE_KEY`, "PUT");
  assert.equal(put.body.value, value);
  assert.match(put.headers["idempotency-key"], /^[0-9a-f-]{36}$/u);
  // The caller already holds the value, so nothing shows it back.
  assert.doesNotMatch(inline.stdout, /do_not_log/u);
  assert.doesNotMatch(inline.stderr, /do_not_log/u);
  assert.match(inline.stdout, /name\s+SERVICE_KEY/u);
  // The generated path was not taken.
  assert.equal(api.find(`${BASE}/function-secrets`, "POST").length, 0);

  // --value-file keeps the value out of shell history, and round-trips a file
  // `--secret-file` wrote, which ends with a newline.
  const path = join(directory, "service.key");
  await writeFile(path, `${value}\n`);
  const fromFile = await cli(["functions", "secrets", "create", "SERVICE_KEY", ...TENANT, "--value-file", path]);
  assert.equal(fromFile.code, 0, fromFile.stderr);
  assert.equal(api.find(`${BASE}/function-secrets/SERVICE_KEY`, "PUT")[1].body.value, value);

  const both = await cli([
    "functions", "secrets", "create", "SERVICE_KEY", ...TENANT, "--value", value, "--value-file", path,
  ]);
  assert.equal(both.code, 2, "--value and --value-file are mutually exclusive");
  const missing = await cli(["functions", "secrets", "create", "SERVICE_KEY", ...TENANT, "--value-file", join(directory, "absent")]);
  assert.equal(missing.code, 2);
  const empty = await cli(["functions", "secrets", "create", "SERVICE_KEY", ...TENANT, "--value", ""]);
  assert.equal(empty.code, 2);
  const withSecretFile = await cli([
    "functions", "secrets", "create", "SERVICE_KEY", ...TENANT, "--value", value,
    "--secret-file", join(directory, "unused.txt"),
  ]);
  assert.equal(withSecretFile.code, 2, "--secret-file has nothing to write for a supplied value");
  const store = await readFile(join(directory, "credentials.json"), "utf8");
  assert.doesNotMatch(store, /do_not_log/u);
});

test("function secrets are shown exactly once and never on stderr", async (t) => {
  const { api, cli, directory } = await setup(t);

  const created = await cli(["functions", "secrets", "create", "API_KEY", ...TENANT]);
  assert.equal(created.code, 0, created.stderr);
  assert.equal(api.find(`${BASE}/function-secrets`, "POST")[0].body.name, "API_KEY");
  assert.equal(created.stdout.split("fs_live_do_not_log_0001").length - 1, 1);
  assert.match(created.stdout, /function secret value \(shown once\)/u);
  assert.match(created.stdout, /name\s+API_KEY/u);
  assert.doesNotMatch(created.stderr, /fs_live/u);

  const json = await cli(["functions", "secrets", "create", "API_KEY", ...TENANT, "--json"]);
  assert.deepEqual(JSON.parse(json.stdout), { ...secret(), secret: "fs_live_do_not_log_0001" });

  const file = join(directory, "secret.txt");
  const toFile = await cli(["functions", "secrets", "create", "API_KEY", ...TENANT, "--secret-file", file]);
  assert.equal(toFile.code, 0, toFile.stderr);
  assert.doesNotMatch(toFile.stdout, /fs_live/u);
  assert.doesNotMatch(toFile.stderr, /fs_live/u);
  assert.equal(await readFile(file, "utf8"), "fs_live_do_not_log_0001\n");
  assert.equal((await stat(file)).mode & 0o777, 0o600);

  const got = await cli(["functions", "secrets", "get", "API_KEY", ...TENANT]);
  assert.equal(got.code, 0, got.stderr);
  assert.match(got.stdout, /state\s+active/u);
  assert.doesNotMatch(got.stdout, /fs_live/u);

  const retireRefused = await cli(["functions", "secrets", "retire", "API_KEY", ...TENANT]);
  assert.equal(retireRefused.code, 2);
  assert.equal(api.find(`${BASE}/function-secrets/API_KEY`, "DELETE").length, 0);
  const retired = await cli(["functions", "secrets", "retire", "API_KEY", ...TENANT, "--yes"]);
  assert.equal(retired.code, 0, retired.stderr);
  assert.match(retired.stdout, /state\s+retired/u);

  const rotateRefused = await cli(["functions", "secrets", "rotate", "API_KEY", ...TENANT]);
  assert.equal(rotateRefused.code, 2);
  const rotated = await cli(["functions", "secrets", "rotate", "API_KEY", ...TENANT, "--yes"]);
  assert.equal(rotated.code, 0, rotated.stderr);
  assert.equal(rotated.stdout.split("fs_live_do_not_log_0002").length - 1, 1);
  assert.doesNotMatch(rotated.stderr, /fs_live/u);
  assert.match(api.find(`${BASE}/function-secrets/API_KEY/actions/rotate`, "POST")[0].headers["idempotency-key"], /^[0-9a-f-]{36}$/u);
  const store = await readFile(join(directory, "credentials.json"), "utf8");
  assert.doesNotMatch(store, /fs_live/u);
});

test("functions test sends the FunctionTestRequest shape and decodes the response", async (t) => {
  const { api, cli } = await setup(t);

  const result = await cli([
    "functions", "test", "hello", ...TENANT, "--method", "post", "--path", "/items?x=1", "--header", "content-type=application/json", "-H", "x-trace=abc", "--body", '{"title":"t"}', "--version", "2",
  ]);
  assert.equal(result.code, 0, result.stderr);
  assert.deepEqual(api.find(`${BASE}/functions/hello/actions/test`, "POST")[0].body, {
    version: 2,
    method: "POST",
    path: "/items?x=1",
    headers: { "content-type": "application/json", "x-trace": "abc" },
    body: Buffer.from('{"title":"t"}').toString("base64"),
  });
  assert.match(result.stdout, /status\s+201/u);
  assert.match(result.stdout, /correlationId\s+corr_1/u);
  assert.match(result.stdout, /body\s+created!/u);

  const defaults = await cli(["functions", "test", "hello", ...TENANT, "--json"]);
  assert.equal(defaults.code, 0, defaults.stderr);
  assert.deepEqual(api.find(`${BASE}/functions/hello/actions/test`, "POST")[1].body, { method: "GET", path: "/", headers: {}, body: "" });
  assert.equal(JSON.parse(defaults.stdout).body, Buffer.from("created!").toString("base64"));

  const badHeader = await cli(["functions", "test", "hello", ...TENANT, "--header", "nope"]);
  assert.equal(badHeader.code, 2);
});

async function functionDirectory(t) {
  const root = await mkdtemp(join(tmpdir(), "mako-cli-fn-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const directory = join(root, "hello");
  await mkdir(join(directory, "lib"), { recursive: true });
  await mkdir(join(directory, "node_modules", "left-pad"), { recursive: true });
  await writeFile(join(directory, "index.ts"), 'import { greet } from "./lib/greet.ts";\nDeno.serve(() => new Response(greet()));\n');
  await writeFile(join(directory, "lib", "greet.ts"), 'export const greet = () => "hello";\n');
  await writeFile(join(directory, "node_modules", "left-pad", "index.js"), "module.exports = 1;\n");
  await writeFile(join(directory, ".env"), "SECRET=do-not-upload\n");
  return directory;
}

test("functions deploy uploads, creates the version, checks health, and promotes in order", async (t) => {
  const { api, state, cli } = await setup(t);
  const directory = await functionDirectory(t);

  const result = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT, "--dependency", "left-pad=lib/greet.ts", "--yes"]);
  assert.equal(result.code, 0, result.stderr);
  assert.deepEqual(state.calls, [
    "GET /functions/hello",
    "POST /function-bundles",
    "GET /functions/hello/versions",
    "POST /functions/hello/versions",
    "POST /functions/hello/versions/2/actions/health-check",
    "POST /functions/hello/versions/2/actions/promote",
  ]);
  const upload = api.find(`${BASE}/function-bundles`, "POST")[0];
  assert.match(upload.headers["idempotency-key"], /^[0-9a-f-]{36}$/u);
  assert.equal(upload.body.kind, "source");
  assert.equal(upload.body.entrypoint, "index.ts");
  assert.deepEqual(upload.body.dependencies, { "left-pad": "lib/greet.ts" });
  assert.deepEqual(
    upload.body.files.map((file) => file.path),
    ["index.ts", "lib/greet.ts"],
    "dotfiles and node_modules are not uploaded",
  );
  assert.equal(Buffer.from(upload.body.files[1].contentBase64, "base64").toString("utf8"), 'export const greet = () => "hello";\n');
  assert.doesNotMatch(upload.text, /do-not-upload/u);
  const created = api.find(`${BASE}/functions/hello/versions`, "POST")[0];
  assert.deepEqual(created.body, { version: 2, bundleDigest: DIGEST, entrypoint: "index.ts", runtimeVersion: PINNED_RUNTIME });
  assert.equal(result.stdout, `bundle ${DIGEST}\nversion 2\nhealth healthy\nactive 2\n`);
  assert.match(result.stderr, new RegExp(`resume: mako-cloud functions deployments create hello --bundle ${DIGEST} --entrypoint index.ts --runtime ${PINNED_RUNTIME} --project ${PROJECT_ID} --env ${ENVIRONMENT_ID}`, "u"));
  assert.match(result.stderr, new RegExp(`resume: mako-cloud functions deployments health hello 2 --project ${PROJECT_ID} --env ${ENVIRONMENT_ID}`, "u"));
  assert.match(result.stderr, new RegExp(`resume: mako-cloud functions deployments promote hello 2 --project ${PROJECT_ID} --env ${ENVIRONMENT_ID} --yes`, "u"));
});

test("functions deploy promotes only with --yes; without it the version stands ready", async (t) => {
  const { state, cli } = await setup(t);
  const directory = await functionDirectory(t);

  const refused = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT]);
  assert.equal(refused.code, 2, refused.stderr);
  assert.match(refused.stderr, /without --yes/u);
  assert.ok(!state.calls.some((call) => call.endsWith("/actions/promote")), "nothing was promoted");
  assert.ok(state.calls.some((call) => call.endsWith("/actions/health-check")), "the version was created and checked");
  assert.match(refused.stdout, /^bundle .*\nversion 2\nhealth healthy\n$/u);
  assert.match(refused.stderr, /resume: mako-cloud functions deployments promote hello 2/u);
});

test("functions deploy --no-promote stops after the health check; --json prints one document", async (t) => {
  const { state, cli } = await setup(t);
  const directory = await functionDirectory(t);

  const result = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT, "--no-promote", "--version", "7", "--runtime", "v9.9.9", "--json"]);
  assert.equal(result.code, 0, result.stderr);
  assert.deepEqual(state.calls, [
    "GET /functions/hello",
    "POST /function-bundles",
    "POST /functions/hello/versions",
    "POST /functions/hello/versions/7/actions/health-check",
  ]);
  const report = JSON.parse(result.stdout);
  assert.equal(report.functionName, "hello");
  assert.equal(report.created, false);
  assert.equal(report.bundle.digest, DIGEST);
  assert.equal(report.version, 7);
  assert.equal(report.deployment.runtimeVersion, "v9.9.9");
  assert.equal(report.health, "healthy");
  assert.equal(report.promoted, false);
  assert.match(result.stderr, /not promoted/u);
});

test("functions deploy --create makes a missing function first, and refuses without --create", async (t) => {
  const state = freshState();
  state.functions = [];
  const { api, cli } = await setup(t, state);
  const directory = await functionDirectory(t);

  const missing = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT]);
  assert.equal(missing.code, 4);
  assert.match(missing.stderr, /--create/u);
  assert.equal(api.find(`${BASE}/function-bundles`, "POST").length, 0);

  const needsRegion = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT, "--create"]);
  assert.equal(needsRegion.code, 2);
  assert.match(needsRegion.stderr, /--region/u);
  assert.equal(api.find(`${BASE}/functions`, "POST").length, 0);

  state.calls.length = 0;
  const created = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT, "--create", "--region", "local", "--yes"]);
  assert.equal(created.code, 0, created.stderr);
  assert.equal(state.calls[0], "GET /functions/hello");
  assert.equal(state.calls[1], "POST /functions");
  assert.equal(state.calls.at(-1), "POST /functions/hello/versions/2/actions/promote");
  assert.deepEqual(api.find(`${BASE}/functions`, "POST")[0].body, { name: "hello", configuration: configuration() });
  assert.match(created.stdout, /^function hello\nbundle sha256:/u);
});

test("functions deploy --allow-host sends a sorted declaration, and omits the field entirely when absent", async (t) => {
  const state = freshState();
  state.functions = [];
  const { api, cli } = await setup(t, state);
  const directory = await functionDirectory(t);

  const declared = await cli([
    "functions", "deploy", directory, "--name", "hello", ...TENANT, "--create",
    "--region", "local", "--yes",
    "--allow-host", "sandbox.plaid.com", "--allow-host", "api.example.com",
  ]);
  assert.equal(declared.code, 0, declared.stderr);
  const created = api.find(`${BASE}/functions`, "POST")[0].body;
  // Sorted like the control plane stores it; the API is the validator, the
  // CLI only carries the declaration.
  assert.deepEqual(created.configuration.allowedHosts, ["api.example.com", "sandbox.plaid.com"]);
  // An undeclared deploy is byte-identical to one from a CLI that predates
  // the field: no allowedHosts key at all, not an empty list.
  assert.equal(Object.hasOwn(configuration(), "allowedHosts"), false);
});

test("functions deploy exits non-zero on a failed health check or a rejected bundle, with the resume command printed", async (t) => {
  const state = freshState();
  state.health = { state: "failed", diagnostic: "worker exited with status 1" };
  const { api, cli } = await setup(t, state);
  const directory = await functionDirectory(t);

  const unhealthy = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT]);
  assert.equal(unhealthy.code, 5);
  assert.equal(unhealthy.stdout, `bundle ${DIGEST}\nversion 2\nhealth failed\n`);
  assert.match(unhealthy.stderr, /worker exited with status 1/u);
  assert.match(unhealthy.stderr, /CLI_DEPLOYMENT_UNHEALTHY/u);
  assert.match(unhealthy.stderr, new RegExp(`resume: mako-cloud functions deployments health hello 2 --project ${PROJECT_ID} --env ${ENVIRONMENT_ID}`, "u"));
  assert.equal(api.find(`${BASE}/functions/hello/versions/2/actions/promote`, "POST").length, 0);

  state.upload = {
    status: 200,
    json: { status: "rejected", diagnostics: [{ severity: "error", code: "invalid_module_path", message: "bad path", path: "lib/greet.ts", line: 1 }] },
  };
  const rejected = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT]);
  assert.equal(rejected.code, 5);
  assert.equal(rejected.stdout, "");
  assert.match(rejected.stderr, /error invalid_module_path: bad path \(lib\/greet.ts:1\)/u);
  assert.match(rejected.stderr, /CLI_BUNDLE_REJECTED/u);

  state.upload = apiError("permission_denied", "automation token lacks function_deploy", 403);
  const denied = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT]);
  assert.equal(denied.code, 3);
  assert.match(denied.stderr, /function_deploy/u);

  const noEntrypoint = await cli(["functions", "deploy", directory, "--name", "hello", ...TENANT, "--entrypoint", "main.ts"]);
  assert.equal(noEntrypoint.code, 2);
  assert.match(noEntrypoint.stderr, /main.ts/u);
  const badName = await cli(["functions", "deploy", directory, "--name", "Hello", ...TENANT]);
  assert.equal(badName.code, 2);
});

test("functions --help lists the group and serve stays outside the registry", async () => {
  const help = await runCli(["functions", "--help"]);
  assert.equal(help.code, 0);
  assert.match(help.stdout, /deploy/u);
  assert.match(help.stdout, /serve/u);
  const deploy = await runCli(["functions", "deploy", "--help"]);
  assert.match(deploy.stdout, /--no-promote/u);
  assert.match(deploy.stdout, /--create/u);
  const dir = await configDir({ after: () => {} });
  await rm(dir, { recursive: true, force: true });
});
