// Application email templates against the loopback mock: listing with the
// default flag, get, set from a file or stdin with an idempotency key, reset
// without a confirmation header, preview of stored and unsaved text, and the
// validation message a refused template comes back with.
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
const TEMPLATES = `${BASE}/email-templates`;
const TENANT = ["--project", PROJECT_ID, "--env", ENVIRONMENT_ID];
const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

function template(overrides = {}) {
  return {
    kind: "magic_link",
    subject: "Your sign-in link for {{project_name}}",
    textBody: "Use this link to sign in:\n\n{{link}}\n",
    isDefault: true,
    version: 0,
    updatedAt: null,
    ...overrides,
  };
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

test("email-templates list shows every kind with its default flag; --json is verbatim", async (t) => {
  const items = [
    template({ kind: "verification", subject: "Verify your email for {{project_name}}" }),
    template({ kind: "recovery", subject: "Reset your {{project_name}} password" }),
    template({ kind: "invitation", subject: "You are invited to {{project_name}}" }),
    template({ isDefault: false, version: 3, updatedAt: NOW }),
  ];
  const { api, cli } = await tenantCli(t, {
    [`GET ${TEMPLATES}`]: () => ({ status: 200, json: { items } }),
  });
  const human = await cli(["email-templates", "list"]);
  assert.equal(human.code, 0, human.stderr);
  assert.match(human.stdout, /^kind\s+default\s+version\s+subject\s+updatedAt\n/u);
  assert.match(human.stdout, /verification\s+true\s+0\s+Verify your email/u);
  assert.match(human.stdout, /magic_link\s+false\s+3\s+Your sign-in link/u);
  assert.equal(api.find(TEMPLATES, "GET")[0].headers.authorization, BEARER);

  const json = await cli(["email-templates", "list", "--json"]);
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), items);
});

test("email-templates get shows one kind and refuses a kind the API does not have", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`GET ${TEMPLATES}/magic_link`]: () => ({ status: 200, json: template() }),
  });
  const found = await cli(["email-templates", "get", "magic_link"]);
  assert.equal(found.code, 0, found.stderr);
  assert.match(found.stdout, /^kind\s+magic_link\n/u);
  assert.match(found.stdout, /isDefault\s+true/u);
  assert.match(found.stdout, /\{\{link\}\}/u);

  const unknown = await cli(["email-templates", "get", "newsletter"]);
  assert.equal(unknown.code, 2);
  assert.match(unknown.stderr, /<kind> must be one of verification, recovery, invitation, magic_link/u);
  assert.equal(api.find(`${TEMPLATES}/newsletter`, "GET").length, 0, "a usage error sends nothing");
});

test("email-templates set sends subject and body from a file, stdin, or inline with an idempotency key", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`PUT ${TEMPLATES}/magic_link`]: (request) => ({
      status: 200,
      json: template({ ...request.body, isDefault: false, version: 1, updatedAt: NOW }),
    }),
  });
  const path = `${TEMPLATES}/magic_link`;
  const bodyFile = join(directory, "magic-link.txt");
  await writeFile(bodyFile, "Open {{link}} before {{expires_at}}.\n");

  const fromFile = await cli([
    "email-templates", "set", "magic_link",
    "--subject", "Sign in to {{project_name}}", "--body", `@${bodyFile}`, "--json",
  ]);
  assert.equal(fromFile.code, 0, fromFile.stderr);
  const first = api.find(path, "PUT")[0];
  assert.deepEqual(first.body, {
    subject: "Sign in to {{project_name}}",
    textBody: "Open {{link}} before {{expires_at}}.\n",
  });
  assert.equal(first.headers.authorization, BEARER);
  assert.match(first.headers["idempotency-key"], UUID);
  assert.equal(JSON.parse(fromFile.stdout).version, 1);

  const fromStdin = await cli(
    ["email-templates", "set", "magic_link", "--subject", "Hello {{email}}", "--body", "-"],
    { stdin: "{{link}}\n" },
  );
  assert.equal(fromStdin.code, 0, fromStdin.stderr);
  const second = api.find(path, "PUT")[1];
  assert.deepEqual(second.body, { subject: "Hello {{email}}", textBody: "{{link}}\n" });
  assert.notEqual(second.headers["idempotency-key"], first.headers["idempotency-key"]);
  assert.match(fromStdin.stdout, /^kind\s+magic_link\n/u);

  const inline = await cli(["email-templates", "set", "magic_link", "--subject", "Hi", "--body", "Visit {{link}}"]);
  assert.equal(inline.code, 0, inline.stderr);
  assert.deepEqual(api.find(path, "PUT")[2].body, { subject: "Hi", textBody: "Visit {{link}}" });

  const missingBody = await cli(["email-templates", "set", "magic_link", "--subject", "Hi"]);
  assert.equal(missingBody.code, 2);
  assert.match(missingBody.stderr, /--subject <text> and --body <@file\|-\|text> are both required/u);
  assert.equal(api.find(path, "PUT").length, 3, "an incomplete set sends nothing");
});

test("email-templates set surfaces the API's validation message for a refused template", async (t) => {
  const { cli } = await tenantCli(t, {
    [`PUT ${TEMPLATES}/recovery`]: () =>
      apiError(
        "invalid_request",
        "textBody: unknown variable {{password}}; recovery templates may use link, expires_at, email, project_name, environment_name",
        400,
      ),
  });
  const refused = await cli(["email-templates", "set", "recovery", "--subject", "Reset", "--body", "{{password}}"]);
  assert.notEqual(refused.code, 0);
  assert.equal(refused.stdout, "");
  assert.match(refused.stderr, /unknown variable \{\{password\}\}; recovery templates may use link/u);
});

test("email-templates reset sends a plain DELETE and reports the default", async (t) => {
  const { api, cli } = await tenantCli(t, {
    [`DELETE ${TEMPLATES}/invitation`]: () => ({
      status: 200,
      json: template({ kind: "invitation", subject: "You are invited to {{project_name}}" }),
    }),
  });
  const reset = await cli(["email-templates", "reset", "invitation", "--json"]);
  assert.equal(reset.code, 0, reset.stderr);
  const request = api.find(`${TEMPLATES}/invitation`, "DELETE")[0];
  assert.equal(request.headers.authorization, BEARER);
  assert.equal(request.headers.confirmation, undefined, "a reset needs no confirmation header");
  assert.equal(request.text, "");
  assert.equal(JSON.parse(reset.stdout).isDefault, true);
  assert.match(reset.stderr, /invitation template reset to the built-in default/u);
});

test("email-templates preview renders the stored template or the unsaved text given", async (t) => {
  const { api, cli, directory } = await tenantCli(t, {
    [`POST ${TEMPLATES}/magic_link/actions/preview`]: (request) => ({
      status: 200,
      json: {
        subject: request.body?.subject ? "Sign in to Field Notes" : "Your sign-in link for Field Notes",
        textBody: request.body?.textBody ? "unsaved body https://example.test/link\n" : "Use this link to sign in:\n\nhttps://example.test/link\n",
      },
    }),
  });
  const path = `${TEMPLATES}/magic_link/actions/preview`;

  const stored = await cli(["email-templates", "preview", "magic_link"]);
  assert.equal(stored.code, 0, stored.stderr);
  assert.equal(
    stored.stdout,
    "Subject: Your sign-in link for Field Notes\n\nUse this link to sign in:\n\nhttps://example.test/link\n\n",
  );
  const first = api.find(path, "POST")[0];
  assert.equal(first.text, "", "no options previews the stored template");
  assert.equal(first.headers.authorization, BEARER);

  const bodyFile = join(directory, "draft.txt");
  await writeFile(bodyFile, "unsaved body {{link}}\n");
  const unsaved = await cli([
    "email-templates", "preview", "magic_link",
    "--subject", "Sign in to {{project_name}}", "--body", `@${bodyFile}`, "--json",
  ]);
  assert.equal(unsaved.code, 0, unsaved.stderr);
  assert.deepEqual(api.find(path, "POST")[1].body, {
    subject: "Sign in to {{project_name}}",
    textBody: "unsaved body {{link}}\n",
  });
  assert.deepEqual(JSON.parse(unsaved.stdout), {
    subject: "Sign in to Field Notes",
    textBody: "unsaved body https://example.test/link\n",
  });

  const subjectOnly = await cli(["email-templates", "preview", "magic_link", "--subject", "Hi {{email}}"]);
  assert.equal(subjectOnly.code, 0, subjectOnly.stderr);
  assert.deepEqual(api.find(path, "POST")[2].body, { subject: "Hi {{email}}" }, "only the given part is sent");
});
