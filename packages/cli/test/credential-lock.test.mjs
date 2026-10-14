import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { readFile, writeFile, access } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { CommandContext } from "../dist/cli/context.js";
import { withStoreLock } from "../dist/cli/credentials.js";
import { authHandler, apiError, signedIn, startMockApi, configDir } from "./harness.mjs";
const execute = promisify(execFile);
const entry = fileURLToPath(new URL("../dist/main.js", import.meta.url));

function context(directory, endpoint, profile = "default") {
  return new CommandContext({ stdout: { write() {} }, stderr: { write() {} }, stdin: null, isTTY: false, env: {}, cwd: process.cwd() }, {
    endpoint, profile, configDir: directory, json: true, all: false, yes: false, wait: false, timeoutMs: 1000,
  });
}

test("separate CLI processes consume one rotating refresh cookie and share the saved session", async (t) => {
  const handler = authHandler(); let refreshes = 0;
  const api = await startMockApi(async request => {
    if (request.path.endsWith("/sessions/refresh")) {
      if (++refreshes > 1) return apiError("unauthenticated", "cookie already consumed", 401);
      await delay(400);
    }
    return handler(request);
  });
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const ctx = context(directory, api.endpoint), profile = await ctx.loadProfile();
  await ctx.saveProfile({ ...profile, session: { ...profile.session, expiresAt: "2000-01-01T00:00:00.000Z" } });
  await Promise.all(Array.from({ length: 6 }, () => execute(process.execPath, [entry, "teams", "list", "--config-dir", directory, "--json"], { env: { ...process.env, MAKO_TOKEN: "", MAKO_PROFILE: "default", MAKO_ENDPOINT: api.endpoint } })));
  assert.equal(refreshes, 1);
  assert.equal(api.find("/v1/teams", "GET").length, 6);
  assert.ok(api.find("/v1/teams", "GET").every(r => r.headers.authorization === "Bearer developer-session-token-0002"));
  assert.equal((await ctx.loadProfile()).session.refreshCookie, "refresh-cookie-0002");
  await assert.rejects(access(join(directory, "credentials.lock")), { code: "ENOENT" });
});

test("concurrent profile saves preserve every profile", async (t) => {
  const directory = await configDir(t);
  await Promise.all(Array.from({ length: 12 }, (_, i) => context(directory, "https://fixture.invalid", `profile-${i}`).saveProfile({ endpoint: "https://fixture.invalid" })));
  const store = JSON.parse(await readFile(join(directory, "credentials.json"), "utf8"));
  assert.equal(Object.keys(store.profiles).length, 12);
});

test("a failed operation releases its lock and retains the original error", async (t) => {
  const directory = await configDir(t), error = Object.assign(Error("operation failed"), { code: "EEXIST" });
  await assert.rejects(withStoreLock(directory, async () => { throw error; }), e => e === error);
  assert.equal(await withStoreLock(directory, async () => "next"), "next");
});

test("an abandoned lock fails explicitly without stealing it or attempting authentication", async (t) => {
  const directory = await configDir(t), child = spawn(process.execPath, ["-e", ""]);
  await once(child, "close");
  const path = join(directory, "credentials.lock");
  await writeFile(path, String(child.pid), { mode: 0o600 });
  await assert.rejects(withStoreLock(directory, async () => assert.fail("must not enter")), e => e.code === "CLI_CREDENTIAL_LOCK" && e.message.includes("abandoned"));
  assert.equal(await readFile(path, "utf8"), String(child.pid));
});
