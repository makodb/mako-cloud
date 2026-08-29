// Custom domains against the loopback mock: project-level paths, the TXT
// record printed on add, idempotency keys, hostname checks that send
// nothing, the verify outcome, and destructive confirmation on remove.
import assert from "node:assert/strict";
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

const DOMAINS = `/v1/projects/${PROJECT_ID}/domains`;
const PROJECT = ["--project", PROJECT_ID];
const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const DOMAIN = "dom_api000000001";
const LATER = "2026-08-07T12:00:00.000Z";
const RECORD_VALUE = "mako-verify=0123456789abcdef0123456789abcdef";

function domain(overrides = {}) {
  return {
    id: DOMAIN,
    projectId: PROJECT_ID,
    environmentId: ENVIRONMENT_ID,
    hostname: "api.example.com",
    state: "pending",
    verification: {
      recordName: "_mako-verify.api.example.com",
      recordType: "TXT",
      recordValue: RECORD_VALUE,
    },
    verifiedAt: null,
    lastCheckedAt: null,
    lastError: null,
    createdAt: NOW,
    updatedAt: NOW,
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

/** A signed-in profile against a mock serving `routes`; `cli` appends --project. */
async function projectCli(t, routes) {
  const api = await startMockApi(authHandler({ fallback: router(routes) }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const cli = (argv, options = {}) => runCli([...argv, ...PROJECT], { configDir: directory, ...options });
  return { api, cli, directory };
}

test("domains list renders every state with its error under the project, --json the response verbatim, and needs only --project", async (t) => {
  const items = [
    domain(),
    domain({
      id: "dom_app000000001",
      hostname: "app.example.com",
      state: "verified",
      verifiedAt: NOW,
      lastCheckedAt: LATER,
    }),
    domain({
      id: "dom_old000000001",
      hostname: "old.example.com",
      state: "failed",
      verifiedAt: NOW,
      lastCheckedAt: LATER,
      lastError: "record_missing",
    }),
  ];
  const { api, cli } = await projectCli(t, {
    [`GET ${DOMAINS}`]: () => ({ status: 200, json: { items } }),
  });
  const human = await cli(["domains", "list"]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(
    human.stdout,
    /^id\s+hostname\s+environmentId\s+state\s+error\s+verifiedAt\s+lastCheckedAt\n/u,
  );
  assert.match(human.stdout, /dom_api000000001\s+api\.example\.com\s+env_abcdefgh\s+pending\s+—\s+—\s+—/u);
  assert.match(human.stdout, /dom_app000000001\s+app\.example\.com\s+env_abcdefgh\s+verified\s+—\s+2026-08-06T12:00:00\.000Z\s+2026-08-07T12:00:00\.000Z/u);
  assert.match(human.stdout, /dom_old000000001\s+old\.example\.com\s+env_abcdefgh\s+failed\s+record_missing/u);
  assert.equal(api.find(DOMAINS, "GET")[0].headers.authorization, BEARER);

  const json = await cli(["domains", "list", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), items);

  const noProject = await runCli(["domains", "list"], { configDir: (await projectCli(t, {})).directory });
  assert.equal(noProject.code, 2);
  assert.match(noProject.stderr, /--project <project-id> is required/u);

  const fromEnv = await runCli(["domains", "list", "--json"], {
    configDir: (await projectCli(t, { [`GET ${DOMAINS}`]: () => ({ status: 200, json: { items } }) })).directory,
    env: { MAKO_PROJECT_ID: PROJECT_ID },
  });
  assert.equal(fromEnv.code, 0, fromEnv.stderr);
});

test("domains add sends hostname and environment with an idempotency key and prints the TXT record to publish", async (t) => {
  const { api, cli } = await projectCli(t, {
    [`POST ${DOMAINS}`]: (request) => ({
      status: 201,
      json: domain({
        hostname: request.body.hostname,
        environmentId: request.body.environmentId,
        verification: {
          recordName: `_mako-verify.${request.body.hostname}`,
          recordType: "TXT",
          recordValue: RECORD_VALUE,
        },
      }),
    }),
  });

  const added = await cli(["domains", "add", "--hostname", "API.Example.com.", "--env", ENVIRONMENT_ID]);
  assert.equal(added.code, 0, added.stderr);
  const first = api.find(DOMAINS, "POST")[0];
  assert.deepEqual(first.body, { hostname: "api.example.com", environmentId: ENVIRONMENT_ID }, "the hostname is lowercased and its trailing dot dropped");
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.match(added.stdout, /^id\s+dom_api000000001\n/u);
  assert.match(added.stdout, /state\s+pending/u);
  assert.match(added.stdout, /DNS record to publish for api\.example\.com\n/u);
  assert.match(added.stdout, /\nname\s+_mako-verify\.api\.example\.com\n/u);
  assert.match(added.stdout, /\ntype\s+TXT\n/u);
  assert.match(added.stdout, new RegExp(`\\nvalue\\s+${RECORD_VALUE}\\n`, "u"));
  assert.equal(occurrences(added.stdout, RECORD_VALUE), 1, "the record is printed once, in its block");
  assert.match(added.stderr, /domain dom_api000000001 is pending; publish the DNS record below, then run: mako domains verify dom_api000000001 --project prj_abcdefgh/u);

  const fromEnv = await cli(["domains", "add", "--hostname", "app.example.com", "--json"], {
    env: { MAKO_ENVIRONMENT_ID: ENVIRONMENT_ID },
  });
  assert.equal(fromEnv.code, 0, fromEnv.stderr);
  const second = api.find(DOMAINS, "POST")[1];
  assert.deepEqual(second.body, { hostname: "app.example.com", environmentId: ENVIRONMENT_ID });
  assert.notEqual(second.headers["idempotency-key"], first.headers["idempotency-key"]);
  const parsed = JSON.parse(fromEnv.stdout);
  assert.equal(parsed.hostname, "app.example.com");
  assert.deepEqual(parsed.verification, {
    recordName: "_mako-verify.app.example.com",
    recordType: "TXT",
    recordValue: RECORD_VALUE,
  });
  assert.equal(fromEnv.stderr, "", "--json prints nothing but the response");

  const noHostname = await cli(["domains", "add", "--env", ENVIRONMENT_ID]);
  assert.equal(noHostname.code, 2);
  assert.match(noHostname.stderr, /--hostname <hostname> is required/u);
  const noEnvironment = await cli(["domains", "add", "--hostname", "api.example.com"]);
  assert.equal(noEnvironment.code, 2);
  assert.match(noEnvironment.stderr, /--env <environment-id> is required/u);
  for (const bad of ["example", "api-.example.com", "api..example.com", "api.example.com/path", "api_x.example.com"]) {
    const refused = await cli(["domains", "add", "--hostname", bad, "--env", ENVIRONMENT_ID]);
    assert.equal(refused.code, 2, bad);
    assert.match(refused.stderr, /--hostname must be a fully qualified DNS name/u);
  }
  assert.equal(api.find(DOMAINS, "POST").length, 2, "usage errors send nothing");
});

test("domains add surfaces a conflict for a hostname another project claims", async (t) => {
  const { cli } = await projectCli(t, {
    [`POST ${DOMAINS}`]: () => apiError("conflict", "hostname api.example.com is already claimed", 409),
  });
  const taken = await cli(["domains", "add", "--hostname", "api.example.com", "--env", ENVIRONMENT_ID]);
  assert.notEqual(taken.code, 0);
  assert.equal(taken.stdout, "");
  assert.match(taken.stderr, /already claimed/u);
});

test("domains get shows the record with its verification and error; a missing domain exits 4", async (t) => {
  const failed = domain({
    state: "failed",
    verifiedAt: NOW,
    lastCheckedAt: LATER,
    lastError: "record_mismatch",
  });
  const { cli } = await projectCli(t, {
    [`GET ${DOMAINS}/${DOMAIN}`]: () => ({ status: 200, json: failed }),
    [`GET ${DOMAINS}/dom_ghost0000001`]: () => apiError("not_found", "no such domain", 404),
  });
  const found = await cli(["domains", "get", DOMAIN]);
  assert.equal(found.code, 0, found.stderr);
  assert.match(found.stdout, /^id\s+dom_api000000001\n/u);
  assert.match(found.stdout, /state\s+failed/u);
  assert.match(found.stdout, /lastError\s+record_mismatch/u);
  assert.match(found.stdout, /"recordName": "_mako-verify\.api\.example\.com"/u);

  const json = await cli(["domains", "get", DOMAIN, "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), failed);

  const missing = await cli(["domains", "get", "dom_ghost0000001"]);
  assert.equal(missing.code, 4);
  assert.equal(missing.stdout, "");
  assert.match(missing.stderr, /no such domain/u);

  const noId = await cli(["domains", "get"]);
  assert.equal(noId.code, 2);
});

test("domains verify posts the action with an idempotency key and no body, and states the outcome", async (t) => {
  let outcome = domain({ lastCheckedAt: LATER, lastError: "record_missing" });
  const { api, cli } = await projectCli(t, {
    [`POST ${DOMAINS}/${DOMAIN}/actions/verify`]: () => ({ status: 200, json: outcome }),
  });
  const path = `${DOMAINS}/${DOMAIN}/actions/verify`;

  const stillPending = await cli(["domains", "verify", DOMAIN]);
  assert.equal(stillPending.code, 0, stillPending.stderr);
  const sent = api.find(path, "POST")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(sent.text, "");
  assert.match(stillPending.stdout, /state\s+pending/u);
  assert.match(stillPending.stdout, /lastError\s+record_missing/u);
  assert.match(stillPending.stderr, /domain dom_api000000001 \(api\.example\.com\) is pending: record_missing/u);

  outcome = domain({ state: "verified", verifiedAt: LATER, lastCheckedAt: LATER });
  const verified = await cli(["domains", "verify", DOMAIN]);
  assert.equal(verified.code, 0, verified.stderr);
  assert.match(verified.stdout, /state\s+verified/u);
  assert.match(verified.stderr, /domain dom_api000000001 \(api\.example\.com\) is verified and served/u);

  outcome = domain({ state: "failed", verifiedAt: NOW, lastCheckedAt: LATER, lastError: "record_mismatch" });
  const failed = await cli(["domains", "verify", DOMAIN, "--json"]);
  assert.equal(failed.code, 0, failed.stderr);
  assert.deepEqual(JSON.parse(failed.stdout), outcome);
  assert.equal(failed.stderr, "", "--json prints nothing but the response");
  const human = await cli(["domains", "verify", DOMAIN]);
  assert.match(human.stderr, /is failed: record_mismatch; serving stopped until it verifies again/u);
  assert.equal(api.find(path, "POST").length, 4);
});

test("domains remove is confirmed and sends the idempotency key; nothing is printed but the notice", async (t) => {
  const { api, cli } = await projectCli(t, {
    [`DELETE ${DOMAINS}/${DOMAIN}`]: () => ({ status: 204 }),
  });
  const path = `${DOMAINS}/${DOMAIN}`;

  const refused = await cli(["domains", "remove", DOMAIN]);
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /refusing to remove domain dom_api000000001 without --yes/u);
  assert.equal(refused.stdout, "");
  assert.equal(api.find(path, "DELETE").length, 0, "nothing is sent without confirmation");

  const mistyped = await cli(["domains", "remove", DOMAIN], { stdin: "dom_other\n", isTTY: true });
  assert.equal(mistyped.code, 2);
  assert.equal(api.find(path, "DELETE").length, 0);

  const removed = await cli(["domains", "remove", DOMAIN, "--yes"]);
  assert.equal(removed.code, 0, removed.stderr);
  const sent = api.find(path, "DELETE")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.match(sent.headers["idempotency-key"], UUID);
  assert.equal(sent.text, "");
  assert.equal(removed.stdout, "");
  assert.match(removed.stderr, /domain dom_api000000001 removed/u);

  const json = await cli(["domains", "remove", DOMAIN, "--yes", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), { id: DOMAIN, state: "removed" });

  const typed = await cli(["domains", "remove", DOMAIN], { stdin: `${DOMAIN}\n`, isTTY: true });
  assert.equal(typed.code, 0, typed.stderr);
  assert.equal(api.find(path, "DELETE").length, 3);
});
