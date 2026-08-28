import assert from "node:assert/strict";
import { chmod, readFile, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

import {
  apiError,
  authHandler,
  configDir,
  passwordFile,
  REFRESH_COOKIE,
  runCli,
  session,
  signedIn,
  startMockApi,
  TEAM_ID,
  team,
} from "./harness.mjs";

test("login stores the session privately, later commands use it, logout revokes it", async (t) => {
  const api = await startMockApi(authHandler());
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const signIn = api.find("/v1/developer-auth/sessions", "POST")[0];
  assert.equal(signIn.headers.origin, api.endpoint, "developer-auth calls carry the endpoint as Origin");
  assert.deepEqual(signIn.body, { email: "owner@example.test", password: "correct horse battery staple" });

  const storePath = join(directory, "credentials.json");
  assert.equal((await stat(storePath)).mode & 0o777, 0o600);
  assert.equal((await stat(directory)).mode & 0o077, 0);
  const store = JSON.parse(await readFile(storePath, "utf8"));
  assert.equal(store.profiles.default.endpoint, api.endpoint);
  assert.equal(store.profiles.default.session.refreshCookie, "refresh-cookie-0001");
  assert.equal(store.profiles.default.session.email, "owner@example.test");

  const who = await runCli(["auth", "whoami", "--json"], { configDir: directory });
  assert.equal(who.code, 0, who.stderr);
  assert.equal(api.find("/v1/teams")[0].headers.authorization, "Bearer developer-session-token-0001");
  const parsed = JSON.parse(who.stdout);
  assert.equal(parsed.personalSpaceId, "org_personal");
  assert.equal(parsed.email, "owner@example.test");

  const status = await runCli(["auth", "status"], { configDir: directory });
  assert.match(status.stdout, /stored developer session/u);
  assert.match(status.stdout, /owner@example.test/u);

  const logout = await runCli(["auth", "logout"], { configDir: directory });
  assert.equal(logout.code, 0, logout.stderr);
  const signOut = api.find("/v1/developer-auth/sessions/current", "DELETE")[0];
  assert.equal(signOut.headers.cookie, `${REFRESH_COOKIE}=refresh-cookie-0001`);
  const after = JSON.parse(await readFile(storePath, "utf8"));
  assert.equal(after.profiles.default.session, undefined);
  assert.equal(after.profiles.default.endpoint, api.endpoint, "the endpoint stays for the next login");
});

test("a session near expiry is renewed through the refresh cookie and stored", async (t) => {
  const api = await startMockApi(authHandler());
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const storePath = join(directory, "credentials.json");
  const store = JSON.parse(await readFile(storePath, "utf8"));
  store.profiles.default.session.expiresAt = new Date(Date.now() + 10_000).toISOString();
  await writeFile(storePath, JSON.stringify(store), { mode: 0o600 });

  const who = await runCli(["auth", "whoami", "--json"], { configDir: directory });
  assert.equal(who.code, 0, who.stderr);
  const refresh = api.find("/v1/developer-auth/sessions/refresh", "POST")[0];
  assert.equal(refresh.headers.cookie, `${REFRESH_COOKIE}=refresh-cookie-0001`);
  assert.equal(refresh.headers.origin, api.endpoint);
  assert.equal(api.find("/v1/teams")[0].headers.authorization, "Bearer developer-session-token-0002");
  const renewed = JSON.parse(await readFile(storePath, "utf8")).profiles.default.session;
  assert.equal(renewed.accessToken, "developer-session-token-0002");
  assert.equal(renewed.refreshCookie, "refresh-cookie-0002");
  assert.equal(renewed.email, "owner@example.test");
});

test("MAKO_TOKEN authenticates without touching the store and names the missing permission", async (t) => {
  const api = await startMockApi((request) => {
    if (request.path === "/v1/teams") {
      if (request.headers.authorization === "Bearer ci-token-0123456789abcdef") return { status: 200, json: { items: [team()] } };
      return apiError("permission_denied", "automation token lacks organization_read", 403);
    }
    return undefined;
  });
  t.after(() => api.close());
  const directory = await configDir(t);
  const env = { MAKO_TOKEN: "ci-token-0123456789abcdef", MAKO_ENDPOINT: api.endpoint };

  const ok = await runCli(["auth", "whoami", "--json"], { configDir: directory, env });
  assert.equal(ok.code, 0, ok.stderr);
  assert.equal(JSON.parse(ok.stdout).credential, "MAKO_TOKEN");
  await assert.rejects(stat(join(directory, "credentials.json")), "nothing is written for an environment token");

  const denied = await runCli(["auth", "whoami"], { configDir: directory, env: { ...env, MAKO_TOKEN: "other-token-0123456789abcdef" } });
  assert.equal(denied.code, 3);
  assert.equal(denied.stdout, "");
  assert.match(denied.stderr, /permission_denied/u);
  assert.match(denied.stderr, /organization_read/u);
  assert.match(denied.stderr, /req_abcdefgh/u);

  const status = await runCli(["auth", "status", "--json"], { configDir: directory, env });
  assert.equal(JSON.parse(status.stdout).credential, "environment (MAKO_TOKEN)");
});

test("a credential store other users can read is refused", async (t) => {
  const api = await startMockApi(authHandler());
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  await chmod(join(directory, "credentials.json"), 0o644);
  const result = await runCli(["auth", "whoami"], { configDir: directory });
  assert.equal(result.code, 7);
  assert.match(result.stderr, /readable by other users/u);
});

test("a wait-listed sign-in is reported and nothing usable is stored", async (t) => {
  const api = await startMockApi(
    authHandler({ session: { audience: "mako-developer-waitlist", status: "waitlisted" } }),
  );
  t.after(() => api.close());
  const directory = await configDir(t);
  const result = await runCli(
    ["auth", "login", "--endpoint", api.endpoint, "--email", "new@example.test", "--password-file", await passwordFile(directory)],
    { configDir: directory },
  );
  assert.equal(result.code, 3);
  assert.match(result.stderr, /wait-list/u);
  await assert.rejects(stat(join(directory, "credentials.json")));
});

test("passwords are prompted with echo off and never accepted as arguments", async (t) => {
  const api = await startMockApi(authHandler());
  t.after(() => api.close());
  const directory = await configDir(t);
  const prompted = await runCli(["auth", "login", "--endpoint", api.endpoint, "--email", "owner@example.test"], {
    configDir: directory,
    stdin: "typed-password\n",
    isTTY: true,
  });
  assert.equal(prompted.code, 0, prompted.stderr);
  assert.equal(api.find("/v1/developer-auth/sessions", "POST")[0].body.password, "typed-password");
  assert.doesNotMatch(prompted.stderr, /typed-password/u);
  assert.doesNotMatch(prompted.stdout, /typed-password/u);

  const noTty = await runCli(["auth", "login", "--endpoint", api.endpoint, "--email", "owner@example.test"], {
    configDir: directory,
  });
  assert.equal(noTty.code, 3);
  const asArgument = await runCli(["auth", "login", "--password", "x"], { configDir: directory });
  assert.equal(asArgument.code, 2);
});

test("errors map to exit codes and carry the API's retry advice", async (t) => {
  const cases = [
    [apiError("not_found", "no such team", 404), 4, /not_found/u],
    [apiError("conflict", "already exists", 409), 5, /conflict/u],
    [apiError("rate_limited", "slow down", 429, { kind: "after_delay", afterMs: 61_000 }), 1, /retry after 61s/u],
    [apiError("unavailable", "try again", 503, { kind: "immediate" }), 1, /retryable/u],
  ];
  for (const [response, code, pattern] of cases) {
    const api = await startMockApi(authHandler({ fallback: () => undefined, teams: undefined }));
    const directory = await signedIn(t, api);
    await api.close();
    const failing = await startMockApi((request) => (request.path === "/v1/teams" ? response : undefined));
    const result = await runCli(["auth", "whoami"], {
      configDir: directory,
      env: { MAKO_TOKEN: "token-0123456789abcdef", MAKO_ENDPOINT: failing.endpoint },
    });
    await failing.close();
    assert.equal(result.code, code, `${response.json.error.code}: ${result.stderr}`);
    assert.match(result.stderr, pattern);
    assert.equal(result.stdout, "");
  }
});

test("usage errors exit 2 and help prints for groups and commands", async () => {
  const unknown = await runCli(["auth", "frobnicate"]);
  assert.equal(unknown.code, 2);
  assert.match(unknown.stderr, /unknown command/u);
  const badOption = await runCli(["auth", "status", "--nope"]);
  assert.equal(badOption.code, 2);
  const root = await runCli(["--help"]);
  assert.equal(root.code, 0);
  assert.match(root.stdout, /usage: mako <command>/u);
  const group = await runCli(["auth", "--help"]);
  assert.match(group.stdout, /login/u);
  const command = await runCli(["auth", "login", "--help"]);
  assert.match(command.stdout, /--password-file/u);
  assert.match(command.stdout, /global options:/u);
});

// The synthetic commands below exercise the context helpers every command
// group relies on, independently of any API-backed command.
const secretCommand = {
  path: ["probe", "secret"],
  summary: "prints a secret once",
  operations: [],
  options: { "secret-file": { type: "string", description: "write the secret here" } },
  run: (context, args) =>
    context.secret("api key", "sk_live_do_not_log", { id: "key_1", kind: "public" }, args.string("secret-file")),
};

const destructiveCommand = {
  path: ["probe", "delete"],
  summary: "deletes a thing",
  operations: [],
  positionals: [{ name: "id", description: "thing id", required: true }],
  destructive: { action: "delete thing", resource: (args) => args.requirePositional(0, "id") },
  run: async (context) => {
    context.out({ deleted: true });
  },
};

const waitCommand = {
  path: ["probe", "wait"],
  summary: "waits for a counter",
  operations: [],
  run: async (context) => {
    let count = 0;
    const value = await context.waitFor(
      async () => ++count,
      (n) => n >= (context.io.env.PROBE_TARGET === undefined ? 3 : Number(context.io.env.PROBE_TARGET)),
      (n) => `count=${n}`,
    );
    context.out({ value });
  },
};

const pagedCommand = {
  path: ["probe", "pages"],
  summary: "pages",
  operations: [],
  run: async (context) => {
    const pages = { "": { items: [{ n: 1 }], nextCursor: "c1" }, c1: { items: [{ n: 2 }], nextCursor: "c2" }, c2: { items: [{ n: 3 }] } };
    context.out(await context.collect(async (cursor) => pages[cursor ?? ""]), { columns: [{ key: "n" }] });
  },
};

const stepUpCommand = {
  path: ["probe", "step-up"],
  summary: "needs a step-up grant",
  operations: [],
  run: async (context) => {
    const token = await context.stepUp();
    context.out({ token });
  },
};

const probes = [secretCommand, destructiveCommand, waitCommand, pagedCommand, stepUpCommand];

test("secrets print exactly once: block, JSON field, or 0600 file", async (t) => {
  const block = await runCli(["probe", "secret"], { commands: probes });
  assert.equal(block.code, 0, block.stderr);
  assert.equal(block.stdout.split("sk_live_do_not_log").length - 1, 1);
  assert.match(block.stdout, /api key \(shown once\)/u);
  assert.doesNotMatch(block.stderr, /sk_live/u);

  const json = await runCli(["probe", "secret", "--json"], { commands: probes });
  const parsed = JSON.parse(json.stdout);
  assert.equal(parsed.secret, "sk_live_do_not_log");
  assert.equal(parsed.id, "key_1");

  const directory = await configDir(t);
  const file = join(directory, "key.txt");
  const toFile = await runCli(["probe", "secret", "--secret-file", file], { commands: probes });
  assert.equal(toFile.code, 0, toFile.stderr);
  assert.doesNotMatch(toFile.stdout, /sk_live/u);
  assert.equal(await readFile(file, "utf8"), "sk_live_do_not_log\n");
  assert.equal((await stat(file)).mode & 0o777, 0o600);
});

test("destructive commands need --yes or the typed resource name", async () => {
  const refused = await runCli(["probe", "delete", "thing_1"], { commands: probes });
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /without --yes/u);
  assert.equal(refused.stdout, "");

  const yes = await runCli(["probe", "delete", "thing_1", "--yes"], { commands: probes });
  assert.equal(yes.code, 0, yes.stderr);
  assert.match(yes.stdout, /deleted/u);

  const typed = await runCli(["probe", "delete", "thing_1"], { commands: probes, stdin: "thing_1\n", isTTY: true });
  assert.equal(typed.code, 0, typed.stderr);
  const mistyped = await runCli(["probe", "delete", "thing_1"], { commands: probes, stdin: "thing_2\n", isTTY: true });
  assert.equal(mistyped.code, 2);
  assert.equal(mistyped.stdout, "");
});

test("--wait polls to a terminal state and exits 6 on the deadline", async () => {
  const done = await runCli(["probe", "wait", "--json"], { commands: probes, env: { MAKO_WAIT_INTERVAL_MS: "1" } });
  assert.equal(done.code, 0, done.stderr);
  assert.equal(JSON.parse(done.stdout).value, 3);
  assert.match(done.stderr, /count=1/u);
  const late = await runCli(["probe", "wait", "--timeout", "1"], {
    commands: probes,
    env: { MAKO_WAIT_INTERVAL_MS: "400", PROBE_TARGET: "1000" },
  });
  assert.equal(late.code, 6);
  assert.match(late.stderr, /timed out waiting; last observed: count=/u);
});

test("--all follows cursors into one document; without it one page and its cursor", async () => {
  const one = await runCli(["probe", "pages", "--json"], { commands: probes });
  assert.deepEqual(JSON.parse(one.stdout), { items: [{ n: 1 }], nextCursor: "c1" });
  const all = await runCli(["probe", "pages", "--json", "--all"], { commands: probes });
  assert.deepEqual(JSON.parse(all.stdout), { items: [{ n: 1 }, { n: 2 }, { n: 3 }], nextCursor: null });
  const human = await runCli(["probe", "pages"], { commands: probes });
  assert.match(human.stdout, /^n\n-\n1\nnext cursor: c1\n$/u);
});

test("a stored session for another endpoint is not sent elsewhere", async (t) => {
  const api = await startMockApi(authHandler());
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const elsewhere = await runCli(["auth", "whoami", "--endpoint", "http://127.0.0.1:1"], { configDir: directory });
  assert.equal(elsewhere.code, 3);
  assert.match(elsewhere.stderr, /signed in to/u);
  assert.equal(api.find("/v1/teams").length, 0);
  assert.equal(team().id, TEAM_ID);
  assert.equal(session().tokenType, "Bearer");
});

test("a step-up action without a terminal fails closed", async (t) => {
  const api = await startMockApi(
    authHandler({
      fallback: (request) =>
        request.path.endsWith("/actions/verify-password")
          ? { status: 200, json: { token: "dst1_grant_0123456789", expiresAtUnixSeconds: 1_786_579_260 } }
          : undefined,
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const refused = await runCli(["probe", "step-up"], { commands: probes, configDir: directory });
  assert.equal(refused.code, 3);
  assert.match(refused.stderr, /step-up/u);
  assert.equal(refused.stdout, "");
  assert.equal(api.find("/v1/developer-auth/sessions/current/actions/verify-password").length, 0, "nothing was sent");

  const passwordFile = join(directory, "step-up.txt");
  await writeFile(passwordFile, "correct horse battery staple\n", { mode: 0o600 });
  api.requests.length = 0;
  const granted = await runCli(["probe", "step-up", "--json"], {
    commands: probes,
    configDir: directory,
    env: { MAKO_STEP_UP_PASSWORD_FILE: passwordFile },
  });
  assert.equal(granted.code, 0, granted.stderr);
  assert.equal(JSON.parse(granted.stdout).token, "dst1_grant_0123456789");
  const verify = api.find("/v1/developer-auth/sessions/current/actions/verify-password", "POST")[0];
  assert.deepEqual(verify.body, { password: "correct horse battery staple" });
  assert.equal(verify.headers.origin, api.endpoint);
});
