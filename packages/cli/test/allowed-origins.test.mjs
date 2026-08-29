// The environment's browser-origin allowlist against the loopback mock: the
// read prints the list one per line, the replacement sends the whole list
// with an idempotency key, origins that a browser would never send are
// refused before anything leaves the process, and the scope of the list is
// said on stderr.
import assert from "node:assert/strict";
import test from "node:test";

import {
  apiError,
  authHandler,
  ENVIRONMENT_ID,
  PROJECT_ID,
  runCli,
  signedIn,
  startMockApi,
} from "./harness.mjs";

const ORIGINS_PATH = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/allowed-origins`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const ORIGINS = ["https://app.example.com", "http://127.0.0.1:5173"];
/** Not origins: paths, a trailing slash, no scheme, plain http off loopback, default and bad ports. */
const BAD_ORIGINS = [
  "https://app.example.com/",
  "https://app.example.com/app",
  "https://app.example.com?x=1",
  "app.example.com",
  "ftp://app.example.com",
  "http://app.example.com",
  "https://app.example.com:443",
  "http://127.0.0.1:80",
  "https://app.example.com:0",
  "https://app.example.com:70000",
  "https://app.example.com:05173",
  "https://app example.com",
];

/** Routes keyed by `METHOD /path`; anything else is unhandled (404). */
function router(routes) {
  return (request) => routes[`${request.method} ${request.path}`]?.(request);
}

async function tenantCli(t, routes) {
  const api = await startMockApi(authHandler({ fallback: router(routes) }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const cli = (argv, options = {}) =>
    runCli([...argv, ...TENANT], { configDir: directory, ...options });
  return { api, cli, directory };
}

test("allowed-origins get prints the environment's list one per line, (none) when empty, and --json verbatim", async (t) => {
  let held = { allowedOrigins: ORIGINS };
  const { api, cli } = await tenantCli(t, {
    [`GET ${ORIGINS_PATH}`]: () => ({ status: 200, json: held }),
  });

  const human = await cli(["allowed-origins", "get"]);
  assert.equal(human.code, 0, human.stderr);
  assert.equal(human.stdout, "https://app.example.com\nhttp://127.0.0.1:5173\n");
  assert.match(
    human.stderr,
    /environment env_abcdefgh allows cross-origin calls from 2 origins to its application API; the management and operator APIs never answer cross-origin, whatever is listed here/u,
  );
  const sent = api.find(ORIGINS_PATH, "GET")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.equal(sent.text, "");

  const json = await cli(["allowed-origins", "get", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), { allowedOrigins: ORIGINS });
  assert.equal(json.stderr, "", "--json prints nothing but the response");

  held = { allowedOrigins: [] };
  const empty = await cli(["allowed-origins", "get"]);
  assert.equal(empty.code, 0, empty.stderr);
  assert.equal(empty.stdout, "(none)\n");
  assert.match(empty.stderr, /environment env_abcdefgh allows no cross-origin access/u);

  const noEnv = await runCli(["allowed-origins", "get", "--project", PROJECT_ID], {
    configDir: (await tenantCli(t, {})).directory,
  });
  assert.equal(noEnv.code, 2);
  assert.match(noEnv.stderr, /--env <environment-id> is required/u);
});

test("allowed-origins set replaces the whole list with an idempotency key, lowercased and de-duplicated; --none clears it", async (t) => {
  let held = { allowedOrigins: [] };
  const { api, cli } = await tenantCli(t, {
    [`PUT ${ORIGINS_PATH}`]: (request) => {
      held = { allowedOrigins: request.body.allowedOrigins };
      return { status: 200, json: held };
    },
  });

  const set = await cli([
    "allowed-origins", "set",
    "--origin", "HTTPS://App.Example.com",
    "--origin", "http://127.0.0.1:5173",
    "--origin", "https://app.example.com",
  ]);
  assert.equal(set.code, 0, set.stderr);
  const first = api.find(ORIGINS_PATH, "PUT")[0];
  assert.equal(first.headers.authorization, BEARER);
  assert.equal(first.headers["content-type"], "application/json");
  assert.match(first.headers["idempotency-key"], UUID);
  assert.deepEqual(
    first.body,
    { allowedOrigins: ORIGINS },
    "the whole list is sent, lowercased and de-duplicated in the order given",
  );
  assert.equal(set.stdout, "https://app.example.com\nhttp://127.0.0.1:5173\n");
  assert.match(
    set.stderr,
    /environment env_abcdefgh allows cross-origin calls from 2 origins to its application API; the management and operator APIs never answer cross-origin/u,
  );

  const one = await cli(["allowed-origins", "set", "--origin", "https://app.example.com", "--json"]);
  assert.equal(one.code, 0, one.stderr);
  assert.deepEqual(JSON.parse(one.stdout), { allowedOrigins: ["https://app.example.com"] });
  assert.equal(one.stderr, "", "--json prints nothing but the response");
  assert.notEqual(
    api.find(ORIGINS_PATH, "PUT")[1].headers["idempotency-key"],
    first.headers["idempotency-key"],
  );

  const cleared = await cli(["allowed-origins", "set", "--none"]);
  assert.equal(cleared.code, 0, cleared.stderr);
  assert.deepEqual(api.find(ORIGINS_PATH, "PUT")[2].body, { allowedOrigins: [] });
  assert.equal(cleared.stdout, "(none)\n");
  assert.match(cleared.stderr, /environment env_abcdefgh allows no cross-origin access/u);
  assert.deepEqual(held, { allowedOrigins: [] });
});

test("allowed-origins set refuses anything a browser would not send as an origin, before sending", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`PUT ${ORIGINS_PATH}`]: () => ({ status: 200, json: { allowedOrigins: [] } }),
  });

  const neither = await cli(["allowed-origins", "set"]);
  assert.equal(neither.code, 2);
  assert.match(neither.stderr, /--origin <url> \(repeatable\) or --none is required/u);
  const both = await cli(["allowed-origins", "set", "--none", "--origin", "https://app.example.com"]);
  assert.equal(both.code, 2);
  assert.match(both.stderr, /--none and --origin are mutually exclusive/u);

  for (const bad of BAD_ORIGINS) {
    const refused = await cli(["allowed-origins", "set", "--origin", bad]);
    assert.equal(refused.code, 2, bad);
    assert.match(refused.stderr, /--origin (?:must|port)/u, bad);
    assert.equal(refused.stdout, "");
  }
  const trailingSlash = await cli([
    "allowed-origins", "set", "--origin", "https://app.example.com/",
  ]);
  assert.match(
    trailingSlash.stderr,
    /without a path, query, or trailing slash; got "https:\/\/app\.example\.com\/"/u,
  );
  const noScheme = await cli(["allowed-origins", "set", "--origin", "app.example.com"]);
  assert.match(noScheme.stderr, /as scheme:\/\/host\[:port\]; got "app\.example\.com"/u);
  const plainHttp = await cli(["allowed-origins", "set", "--origin", "http://app.example.com"]);
  assert.match(plainHttp.stderr, /must use https except to loopback \(localhost or 127\.0\.0\.1\)/u);
  const defaultPort = await cli(["allowed-origins", "set", "--origin", "https://app.example.com:443"]);
  assert.match(defaultPort.stderr, /must omit the default port, a browser sends https:\/\/app\.example\.com/u);
  const loopbackHttp = await cli([
    "allowed-origins", "set", "--origin", "http://localhost:5173", "--json",
  ]);
  assert.equal(loopbackHttp.code, 0, loopbackHttp.stderr);
  assert.deepEqual(api.find(ORIGINS_PATH, "PUT")[0].body, {
    allowedOrigins: ["http://localhost:5173"],
  });

  const seventeen = Array.from({ length: 17 }, (_, index) => [
    "--origin",
    `https://app${index}.example.com`,
  ]).flat();
  const tooMany = await cli(["allowed-origins", "set", ...seventeen]);
  assert.equal(tooMany.code, 2);
  assert.match(tooMany.stderr, /--origin may be given at most 16 times, got 17/u);

  assert.equal(api.find(ORIGINS_PATH, "PUT").length, 1, "usage errors send nothing");
});

test("allowed-origins set surfaces the API's refusal of an origin", async (t) => {
  const { cli } = await tenantCli(t, {
    [`PUT ${ORIGINS_PATH}`]: () =>
      apiError("invalid_request", "origin https://app.example.com is not allowed", 400),
  });
  const refused = await cli(["allowed-origins", "set", "--origin", "https://app.example.com"]);
  assert.notEqual(refused.code, 0);
  assert.equal(refused.stdout, "");
  assert.match(refused.stderr, /origin https:\/\/app\.example\.com is not allowed/u);
});
