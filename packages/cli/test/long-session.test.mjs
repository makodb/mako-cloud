import assert from "node:assert/strict";
import test from "node:test";
import { CommandContext } from "../dist/cli/context.js";
import { authHandler, signedIn, startMockApi } from "./harness.mjs";

test("a long-lived management client renews before later calls and shares concurrent renewal", async (t) => {
  const api = await startMockApi(authHandler());
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const context = new CommandContext({ stdout: { write() {} }, stderr: { write() {} }, stdin: null,
    isTTY: false, env: {}, cwd: process.cwd() }, {
    endpoint: api.endpoint, profile: "default", configDir: directory, json: true, all: false, yes: false, wait: false, timeoutMs: 1000,
  });
  const client = await context.management();
  await client.listTeams();
  const profile = await context.loadProfile();
  await context.saveProfile({ ...profile, session: { ...profile.session, expiresAt: "2000-01-01T00:00:00.000Z" } });
  await Promise.all([client.listTeams(), client.listTeams(), client.listTeams()]);
  assert.equal(api.find("/v1/developer-auth/sessions/refresh", "POST").length, 1);
  const tokens = api.find("/v1/teams", "GET").map((request) => request.headers.authorization);
  assert.deepEqual(tokens, ["Bearer developer-session-token-0001", ...Array(3).fill("Bearer developer-session-token-0002")]);
  assert.equal((await context.loadProfile()).session.accessToken, "developer-session-token-0002");
});
