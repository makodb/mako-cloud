// Application sign-in settings against the loopback mock: the read renders
// what the API holds, the replacement sends the document whole with an
// idempotency key, shape mistakes are refused before anything is sent, and a
// client secret appears nowhere but the request body.
import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
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

const BASE = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const SETTINGS = `${BASE}/auth-settings`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const SECRET = "GOCSPX-never-printed-0001";

function installed(overrides = {}) {
  return {
    providers: [
      {
        name: "google",
        kind: { type: "oidc", issuer: "https://accounts.google.com" },
        clientId: "client-id.apps.googleusercontent.com",
        scopes: [],
        enabled: true,
        hasSecret: true,
      },
      {
        name: "github",
        kind: { type: "git_hub" },
        clientId: "Iv1.github",
        scopes: [],
        enabled: false,
        hasSecret: true,
      },
    ],
    redirectUrls: ["https://app.example.test/auth/callback"],
    magicLinks: { enabled: true, linkTtlSeconds: 900 },
    version: 3,
    ...overrides,
  };
}

function update() {
  return {
    providers: [
      {
        name: "google",
        kind: { type: "oidc", issuer: "https://accounts.google.com" },
        clientId: "client-id.apps.googleusercontent.com",
        clientSecret: SECRET,
        scopes: ["https://www.googleapis.com/auth/calendar.readonly"],
        enabled: true,
      },
      { name: "github", kind: { type: "git_hub" }, clientId: "Iv1.github", enabled: false },
    ],
    redirectUrls: ["https://app.example.test/auth/callback"],
    magicLinks: { enabled: true, linkTtlSeconds: 900 },
  };
}

/** Routes keyed by `METHOD /path`; anything else is unhandled (404). */
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

test("auth-settings get renders the installed settings; --json is the response verbatim", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`GET ${SETTINGS}`]: () => ({ status: 200, json: installed() }),
  });
  const human = await cli(["auth-settings", "get"]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /"name": "google"/u);
  assert.match(human.stdout, /"hasSecret": true/u);
  assert.match(human.stdout, /https:\/\/app\.example\.test\/auth\/callback/u);
  assert.match(human.stdout, /version\s+3/u);
  const sent = api.find(SETTINGS, "GET")[0];
  assert.equal(sent.headers.authorization, BEARER);
  assert.equal(sent.text, "");

  const json = await cli(["auth-settings", "get", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), installed());

  const never = await runCli(["auth-settings", "get", "--project", PROJECT_ID], { configDir: (await tenantCli(t, {})).directory });
  assert.equal(never.code, 2);
  assert.match(never.stderr, /--env <environment-id> is required/u);
});

test("auth-settings set sends the document whole with an idempotency key and never prints a secret", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`PUT ${SETTINGS}`]: (request) => ({
      status: 200,
      json: installed({
        providers: request.body.providers.map(({ clientSecret, scopes, ...provider }) => ({
          ...provider,
          scopes: scopes ?? [],
          hasSecret: true,
        })),
        version: 4,
      }),
    }),
  });
  const file = join(directory, "auth-settings.json");
  await writeFile(file, JSON.stringify(update()));

  const fromFile = await cli(["auth-settings", "set", "--input", `@${file}`]);
  assert.equal(fromFile.code, 0, fromFile.stderr);
  const first = api.find(SETTINGS, "PUT")[0];
  assert.deepEqual(first.body, update(), "the secret travels once, in the body");
  assert.equal(first.headers.authorization, BEARER);
  assert.equal(first.headers["content-type"], "application/json");
  assert.match(first.headers["idempotency-key"], UUID);
  assert.match(fromFile.stdout, /"hasSecret": true/u);
  assert.match(fromFile.stdout, /version\s+4/u);
  assert.doesNotMatch(fromFile.stdout, /GOCSPX/u, "the secret is not printed");
  assert.doesNotMatch(fromFile.stderr, /GOCSPX/u);

  const fromStdin = await cli(["auth-settings", "set", "--input", "-", "--json"], { stdin: JSON.stringify(update()) });
  assert.equal(fromStdin.code, 0, fromStdin.stderr);
  const second = api.find(SETTINGS, "PUT")[1];
  assert.deepEqual(second.body, update());
  assert.notEqual(second.headers["idempotency-key"], first.headers["idempotency-key"]);
  const printed = JSON.parse(fromStdin.stdout);
  assert.equal(printed.version, 4);
  assert.deepEqual(printed.providers.map((provider) => provider.hasSecret), [true, true]);
  assert.doesNotMatch(fromStdin.stdout, /GOCSPX/u);
  assert.doesNotMatch(fromStdin.stderr, /GOCSPX/u);

  const inline = await cli([
    "auth-settings", "set", "--input",
    JSON.stringify({
      providers: [],
      redirectUrls: ["https://app.example.test/"],
      magicLinks: { enabled: false, linkTtlSeconds: 600 },
      emailVerification: { required: true },
    }),
    "--json",
  ]);
  assert.equal(inline.code, 0, inline.stderr);
  assert.deepEqual(api.find(SETTINGS, "PUT")[2].body, {
    providers: [],
    redirectUrls: ["https://app.example.test/"],
    magicLinks: { enabled: false, linkTtlSeconds: 600 },
    emailVerification: { required: true },
  });
});

test("auth-settings set refuses a malformed document before sending anything, without echoing values", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`PUT ${SETTINGS}`]: () => ({ status: 200, json: installed() }),
  });
  const base = update();
  const cases = [
    [{ ...base, providers: "google" }, /--input\.providers must be an array/u],
    [{ ...base, redirectUrls: ["https://ok.example.test", 7] }, /--input\.redirectUrls must be an array of absolute URLs/u],
    [{ ...base, magicLinks: { enabled: true, linkTtlSeconds: 5 } }, /--input\.magicLinks\.linkTtlSeconds must be an integer between 60 and 3600/u],
    [{ ...base, magicLinks: { enabled: "yes", linkTtlSeconds: 600 } }, /--input\.magicLinks\.enabled must be true or false/u],
    [{ ...base, providers: [{ ...base.providers[0], name: "Google" }] }, /--input\.providers\[0\]\.name must be lowercase/u],
    [{ ...base, providers: [{ ...base.providers[0], kind: { type: "saml" } }] }, /--input\.providers\[0\]\.kind\.type must be oidc or git_hub/u],
    [{ ...base, providers: [{ ...base.providers[0], kind: { type: "oidc" } }] }, /--input\.providers\[0\]\.kind\.issuer must be the provider's issuer URL/u],
    [{ ...base, providers: [{ ...base.providers[0], clientId: "" }] }, /--input\.providers\[0\]\.clientId must be a non-empty string/u],
    [{ ...base, providers: [{ ...base.providers[0], clientSecret: 42 }] }, /--input\.providers\[0\]\.clientSecret must be a non-empty string when given/u],
    [{ ...base, providers: [{ ...base.providers[0], scopes: "openid" }] }, /--input\.providers\[0\]\.scopes must be an array of non-empty strings/u],
    [{ ...base, providers: [base.providers[0], { ...base.providers[1], enabled: "no" }] }, /--input\.providers\[1\]\.enabled must be true or false/u],
    [{ ...base, emailVerification: { required: "yes" } }, /--input\.emailVerification must be an object \{required: true\|false\}/u],
    [[], /--input must be a JSON object/u],
  ];
  for (const [document, expected] of cases) {
    const refused = await cli(["auth-settings", "set", "--input", JSON.stringify(document)]);
    assert.equal(refused.code, 2, refused.stderr);
    assert.match(refused.stderr, expected);
    assert.doesNotMatch(refused.stderr, /GOCSPX/u, "a usage error never echoes the document");
    assert.equal(refused.stdout, "");
  }
  const notJson = await cli(["auth-settings", "set", "--input", "{not json"]);
  assert.equal(notJson.code, 2);
  assert.match(notJson.stderr, /--input is not valid JSON/u);
  const missing = await cli(["auth-settings", "set"]);
  assert.equal(missing.code, 2);
  assert.match(missing.stderr, /--input/u);
  assert.equal(api.find(SETTINGS, "PUT").length, 0, "nothing is sent for a refused document");
});

test("auth-settings set surfaces the API's refusal with its message and exit code", async (t) => {
  const { cli } = await tenantCli(t, {
    [`PUT ${SETTINGS}`]: () =>
      apiError("invalid_request", "provider github has no installed client secret; clientSecret is required", 400),
  });
  const refused = await cli(["auth-settings", "set", "--input", JSON.stringify(update())]);
  assert.equal(refused.code, 1);
  assert.equal(refused.stdout, "");
  assert.match(refused.stderr, /error invalid_request: provider github has no installed client secret/u);
  assert.doesNotMatch(refused.stderr, /GOCSPX/u);
});
