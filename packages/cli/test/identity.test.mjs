// Registration, verification, recovery, wait-list status, and automation tokens.
import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

import {
  apiError,
  authHandler,
  configDir,
  ENVIRONMENT_ID,
  NOW,
  passwordFile,
  PROJECT_ID,
  runCli,
  signedIn,
  startMockApi,
  TEAM_ID,
} from "./harness.mjs";

const REGISTRATION_PASSWORD = "Reg1stration-Passw0rd!";
const STORED_PASSWORD = "correct horse battery staple";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const TOKENS_PATH = `/v1/teams/${TEAM_ID}/automation-tokens`;

function accepted(message) {
  return { status: 202, json: { status: "accepted", message } };
}

/** The developer-auth routes a registration walks through. */
function registrationHandler(request) {
  if (request.method !== "POST") return undefined;
  switch (request.path) {
    case "/v1/developer-auth/registrations":
      return accepted("Check your inbox to verify the address.");
    case "/v1/developer-auth/verifications":
      return { status: 200, json: { status: "waitlisted" } };
    case "/v1/developer-auth/verification-resends":
      return accepted("Verification mail sent.");
    case "/v1/developer-auth/password-recovery-requests":
      return accepted("Recovery mail sent.");
    case "/v1/developer-auth/password-recoveries":
      return { status: 200, json: { status: "password_updated" } };
    default:
      return undefined;
  }
}

function includesOnce(text, needle) {
  return text.split(needle).length - 1 === 1;
}

test("auth register prompts for the password with echo off and never takes it as an argument", async (t) => {
  const api = await startMockApi(registrationHandler);
  t.after(() => api.close());
  const directory = await configDir(t);
  const base = ["auth", "register", "--endpoint", api.endpoint, "--email", "new@example.test", "--display-name", "Ada Lovelace"];

  const prompted = await runCli(base, { configDir: directory, stdin: `${REGISTRATION_PASSWORD}\n`, isTTY: true });
  assert.equal(prompted.code, 0, prompted.stderr);
  const registration = api.find("/v1/developer-auth/registrations", "POST")[0];
  assert.deepEqual(registration.body, {
    email: "new@example.test",
    displayName: "Ada Lovelace",
    password: REGISTRATION_PASSWORD,
  });
  assert.equal(registration.headers.origin, api.endpoint, "developer-auth calls carry the endpoint as Origin");
  assert.equal(registration.headers.authorization, undefined, "registration is unauthenticated");
  assert.match(prompted.stdout, /Check your inbox/u);
  assert.ok(!prompted.stdout.includes(REGISTRATION_PASSWORD));
  assert.ok(!prompted.stderr.includes(REGISTRATION_PASSWORD));

  const fromFile = await runCli([...base, "--password-file", await passwordFile(directory), "--json"], { configDir: directory });
  assert.equal(fromFile.code, 0, fromFile.stderr);
  assert.deepEqual(JSON.parse(fromFile.stdout), { status: "accepted", message: "Check your inbox to verify the address." });
  assert.equal(api.find("/v1/developer-auth/registrations", "POST")[1].body.password, STORED_PASSWORD);

  const sent = api.find("/v1/developer-auth/registrations").length;
  const noTty = await runCli(base, { configDir: directory });
  assert.equal(noTty.code, 3, "a password cannot be prompted for without a terminal");
  assert.match(noTty.stderr, /terminal is required/u);
  const asArgument = await runCli([...base, "--password", "x"], { configDir: directory });
  assert.equal(asArgument.code, 2);
  const noEndpoint = await runCli(["auth", "register", "--email", "new@example.test", "--display-name", "Ada"], {
    configDir: directory,
    stdin: "x\n",
    isTTY: true,
  });
  assert.equal(noEndpoint.code, 2);
  assert.match(noEndpoint.stderr, /--endpoint/u);
  assert.equal(api.find("/v1/developer-auth/registrations").length, sent, "refusals send nothing");
  await assert.rejects(stat(join(directory, "credentials.json")), "registration stores nothing");
});

test("verification and password recovery reach the developer-auth routes with the stored endpoint", async (t) => {
  const api = await startMockApi(authHandler({ fallback: registrationHandler }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const verify = await runCli(["auth", "verify-email", "verify-token-0001"], { configDir: directory });
  assert.equal(verify.code, 0, verify.stderr);
  const verification = api.find("/v1/developer-auth/verifications", "POST")[0];
  assert.deepEqual(verification.body, { token: "verify-token-0001" });
  assert.equal(verification.headers.origin, api.endpoint);
  assert.match(verify.stdout, /wait-list/u);

  const resend = await runCli(["auth", "resend-verification", "--email", "new@example.test"], { configDir: directory });
  assert.equal(resend.code, 0, resend.stderr);
  assert.deepEqual(api.find("/v1/developer-auth/verification-resends", "POST")[0].body, { email: "new@example.test" });
  assert.match(resend.stdout, /Verification mail sent/u);
  const resendWithoutEmail = await runCli(["auth", "resend-verification"], { configDir: directory });
  assert.equal(resendWithoutEmail.code, 2);

  const recover = await runCli(["auth", "recover-password", "--email", "new@example.test", "--json"], { configDir: directory });
  assert.equal(recover.code, 0, recover.stderr);
  assert.deepEqual(api.find("/v1/developer-auth/password-recovery-requests", "POST")[0].body, { email: "new@example.test" });
  assert.deepEqual(JSON.parse(recover.stdout), { status: "accepted", message: "Recovery mail sent." });

  const reset = await runCli(
    ["auth", "reset-password", "recovery-token-0001", "--password-file", await passwordFile(directory)],
    { configDir: directory },
  );
  assert.equal(reset.code, 0, reset.stderr);
  assert.deepEqual(api.find("/v1/developer-auth/password-recoveries", "POST")[0].body, {
    token: "recovery-token-0001",
    password: STORED_PASSWORD,
  });
  assert.match(reset.stdout, /Password updated/u);
  assert.ok(!reset.stdout.includes(STORED_PASSWORD));
  assert.ok(!reset.stderr.includes(STORED_PASSWORD));
  const typed = await runCli(["auth", "reset-password", "recovery-token-0002", "--json"], {
    configDir: directory,
    stdin: "new-password-typed\n",
    isTTY: true,
  });
  assert.equal(typed.code, 0, typed.stderr);
  assert.equal(api.find("/v1/developer-auth/password-recoveries", "POST")[1].body.password, "new-password-typed");
  assert.deepEqual(JSON.parse(typed.stdout), { status: "password_updated" });
  assert.ok(!typed.stderr.includes("new-password-typed"));

  const resets = api.find("/v1/developer-auth/password-recoveries").length;
  const asArgument = await runCli(["auth", "reset-password", "recovery-token-0001", "--password", "x"], { configDir: directory });
  assert.equal(asArgument.code, 2);
  const noTty = await runCli(["auth", "reset-password", "recovery-token-0001"], { configDir: directory });
  assert.equal(noTty.code, 3);
  assert.equal(api.find("/v1/developer-auth/password-recoveries").length, resets);
});

test("auth waitlist-status uses the stored or environment token, or signs in without storing anything", async (t) => {
  const waitlisted = { developerIdentityId: "dev_waitlist00", status: "waitlisted" };
  const known = new Set(["developer-session-token-0001", "ci-token-0123456789abcdef", "waitlist-token-0123456789"]);
  const fallback = (request) => {
    if (request.method === "GET" && request.path === "/v1/developer-auth/wait-list-status") {
      const token = String(request.headers.authorization ?? "").replace(/^Bearer /u, "");
      if (known.has(token)) return { status: 200, json: waitlisted };
      return apiError("unauthorized", "not a wait-list token", 401);
    }
    return undefined;
  };
  const api = await startMockApi(authHandler({ fallback }));
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const stored = await runCli(["auth", "waitlist-status", "--json"], { configDir: directory });
  assert.equal(stored.code, 0, stored.stderr);
  const first = api.find("/v1/developer-auth/wait-list-status", "GET")[0];
  assert.equal(first.headers.authorization, "Bearer developer-session-token-0001");
  assert.equal(first.headers.origin, api.endpoint);
  assert.deepEqual(JSON.parse(stored.stdout), waitlisted);

  const fromEnvironment = await runCli(["auth", "waitlist-status"], {
    configDir: await configDir(t),
    env: { MAKO_TOKEN: "ci-token-0123456789abcdef", MAKO_ENDPOINT: api.endpoint },
  });
  assert.equal(fromEnvironment.code, 0, fromEnvironment.stderr);
  assert.equal(api.find("/v1/developer-auth/wait-list-status", "GET")[1].headers.authorization, "Bearer ci-token-0123456789abcdef");
  assert.match(fromEnvironment.stdout, /dev_waitlist00/u);

  const approved = await runCli(
    ["auth", "waitlist-status", "--email", "owner@example.test", "--password-file", await passwordFile(directory), "--json"],
    { configDir: directory },
  );
  assert.equal(approved.code, 0, approved.stderr);
  assert.deepEqual(JSON.parse(approved.stdout), { status: "active", email: "owner@example.test" });

  const nothing = await runCli(["auth", "waitlist-status", "--endpoint", api.endpoint], { configDir: await configDir(t) });
  assert.equal(nothing.code, 3);
  assert.match(nothing.stderr, /--email/u);
  assert.equal(api.find("/v1/developer-auth/wait-list-status").length, 2, "no token means no request");

  const waitlistApi = await startMockApi(
    authHandler({
      fallback,
      session: { accessToken: "waitlist-token-0123456789", audience: "mako-developer-waitlist", status: "waitlisted" },
    }),
  );
  t.after(() => waitlistApi.close());
  const fresh = await configDir(t);
  const signedInToCheck = await runCli(
    [
      "auth", "waitlist-status", "--endpoint", waitlistApi.endpoint,
      "--email", "new@example.test", "--password-file", await passwordFile(fresh), "--json",
    ],
    { configDir: fresh },
  );
  assert.equal(signedInToCheck.code, 0, signedInToCheck.stderr);
  const signIn = waitlistApi.find("/v1/developer-auth/sessions", "POST")[0];
  assert.deepEqual(signIn.body, { email: "new@example.test", password: STORED_PASSWORD });
  assert.equal(
    waitlistApi.find("/v1/developer-auth/wait-list-status", "GET")[0].headers.authorization,
    "Bearer waitlist-token-0123456789",
  );
  assert.deepEqual(JSON.parse(signedInToCheck.stdout), waitlisted);
  assert.ok(!signedInToCheck.stdout.includes("waitlist-token"), "the wait-list token is never printed");
  assert.ok(!signedInToCheck.stderr.includes("waitlist-token"));
  await assert.rejects(stat(join(fresh, "credentials.json")), "a wait-list session is never stored");
});

const issued = {
  token: {
    id: "atm_ci00000001",
    teamId: TEAM_ID,
    name: "ci",
    scope: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID, permissions: ["project_read", "function_deploy"] },
    status: "active",
    expiresAt: "2026-09-05T12:00:00.000Z",
    createdAt: NOW,
  },
  secret: "mat_secret_do_not_log_0001",
};

test("auth token create validates permissions, scopes the token, and shows the secret exactly once", async (t) => {
  const api = await startMockApi(
    authHandler({
      fallback: (request) =>
        request.method === "POST" && request.path === TOKENS_PATH ? { status: 201, json: issued } : undefined,
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const base = ["auth", "token", "create", "--team", TEAM_ID, "--name", "ci", "--expires-in", "30d"];

  const bogus = await runCli([...base, "--permission", "project_read", "--permission", "bogus"], { configDir: directory });
  assert.equal(bogus.code, 2);
  assert.match(bogus.stderr, /unknown permission "bogus"/u);
  assert.match(bogus.stderr, /organization_read/u);
  assert.match(bogus.stderr, /audit_read/u);
  const environmentOnly = await runCli([...base, "--permission", "project_read", "--env", ENVIRONMENT_ID], { configDir: directory });
  assert.equal(environmentOnly.code, 2);
  assert.match(environmentOnly.stderr, /--env needs --project/u);
  const badDuration = await runCli([...base.slice(0, -1), "30x", "--permission", "project_read"], { configDir: directory });
  assert.equal(badDuration.code, 2);
  assert.match(badDuration.stderr, /duration like 30d/u);
  const noPermission = await runCli(base, { configDir: directory });
  assert.equal(noPermission.code, 2);
  assert.match(noPermission.stderr, /--permission is required/u);
  assert.equal(api.find(TOKENS_PATH).length, 0, "invalid input sends nothing");

  const before = Date.now();
  const created = await runCli(
    [...base, "--permission", "project_read,function_deploy", "--project", PROJECT_ID, "--env", ENVIRONMENT_ID],
    { configDir: directory },
  );
  assert.equal(created.code, 0, created.stderr);
  const request = api.find(TOKENS_PATH, "POST")[0];
  assert.equal(request.headers.authorization, "Bearer developer-session-token-0001");
  assert.equal(request.body.name, "ci");
  assert.deepEqual(request.body.scope, {
    permissions: ["project_read", "function_deploy"],
    projectId: PROJECT_ID,
    environmentId: ENVIRONMENT_ID,
  });
  const thirtyDays = 30 * 86_400_000;
  const expiresAt = Date.parse(request.body.expiresAt);
  assert.ok(expiresAt >= before + thirtyDays && expiresAt <= Date.now() + thirtyDays, "expiresAt is 30d from now");
  assert.ok(includesOnce(created.stdout, issued.secret), "the secret is printed once");
  assert.ok(!created.stderr.includes(issued.secret));
  assert.match(created.stdout, /shown once/u);
  assert.match(created.stdout, /project_read,function_deploy/u);

  const json = await runCli([...base, "--permission", "project_read", "--json"], { configDir: directory });
  assert.equal(json.code, 0, json.stderr);
  assert.deepEqual(JSON.parse(json.stdout), issued, "--json is the API's issue with the secret field");
  assert.ok(!json.stderr.includes(issued.secret));

  const file = join(directory, "token.txt");
  const toFile = await runCli([...base, "--permission", "project_read", "--secret-file", file], { configDir: directory });
  assert.equal(toFile.code, 0, toFile.stderr);
  assert.ok(!toFile.stdout.includes(issued.secret));
  assert.ok(!toFile.stderr.includes(issued.secret));
  assert.equal(await readFile(file, "utf8"), `${issued.secret}\n`);
  assert.equal((await stat(file)).mode & 0o777, 0o600);

  const teamWide = await runCli([...base, "--permission", "organization_read", "--json"], { configDir: directory });
  assert.equal(teamWide.code, 0, teamWide.stderr);
  assert.deepEqual(api.find(TOKENS_PATH, "POST")[3].body.scope, { permissions: ["organization_read"] });
});

test("auth token list, revoke, and rotate", async (t) => {
  const revokedToken = {
    ...issued.token,
    id: "atm_old0000001",
    name: "old",
    status: "revoked",
    scope: { permissions: ["organization_read"] },
    revokedAt: NOW,
  };
  const tokens = [issued.token, revokedToken];
  const rotated = { token: { ...issued.token, id: "atm_replacement1" }, secret: "mat_secret_rotated_0002" };
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "GET" && request.path === TOKENS_PATH) return { status: 200, json: { items: tokens } };
        if (request.method === "DELETE" && request.path === `${TOKENS_PATH}/atm_ci00000001`) return { status: 204 };
        if (request.method === "POST" && request.path === `${TOKENS_PATH}/atm_ci00000001/actions/rotate`) {
          return { status: 201, json: rotated };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const list = await runCli(["auth", "token", "list", "--team", TEAM_ID], { configDir: directory });
  assert.equal(list.code, 0, list.stderr);
  assert.equal(api.find(TOKENS_PATH, "GET")[0].headers.authorization, "Bearer developer-session-token-0001");
  assert.match(list.stdout, /^id\s+name\s+status\s+permissions\s+projectId\s+environmentId\s+expiresAt/u);
  assert.match(list.stdout, /atm_ci00000001\s+ci\s+active\s+project_read,function_deploy\s+prj_abcdefgh\s+env_abcdefgh/u);
  assert.match(list.stdout, /atm_old0000001\s+old\s+revoked\s+organization_read\s+—\s+—/u);
  const listJson = await runCli(["auth", "token", "list", "--team", TEAM_ID, "--json"], { configDir: directory });
  assert.deepEqual(JSON.parse(listJson.stdout), tokens);
  const listWithoutTeam = await runCli(["auth", "token", "list"], { configDir: directory });
  assert.equal(listWithoutTeam.code, 2);

  const revokePath = `${TOKENS_PATH}/atm_ci00000001`;
  const refused = await runCli(["auth", "token", "revoke", "atm_ci00000001", "--team", TEAM_ID], { configDir: directory });
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /without --yes/u);
  assert.equal(api.find(revokePath, "DELETE").length, 0, "a refused revocation sends nothing");
  const revoked = await runCli(["auth", "token", "revoke", "atm_ci00000001", "--team", TEAM_ID, "--yes"], { configDir: directory });
  assert.equal(revoked.code, 0, revoked.stderr);
  assert.equal(api.find(revokePath, "DELETE")[0].headers.authorization, "Bearer developer-session-token-0001");
  assert.equal(revoked.stdout, "");
  assert.match(revoked.stderr, /Revoked automation token atm_ci00000001/u);
  const typed = await runCli(["auth", "token", "revoke", "atm_ci00000001", "--team", TEAM_ID, "--json"], {
    configDir: directory,
    stdin: "atm_ci00000001\n",
    isTTY: true,
  });
  assert.equal(typed.code, 0, typed.stderr);
  assert.deepEqual(JSON.parse(typed.stdout), { id: "atm_ci00000001", teamId: TEAM_ID, status: "revoked" });
  assert.equal(api.find(revokePath, "DELETE").length, 2);

  const rotatePath = `${TOKENS_PATH}/atm_ci00000001/actions/rotate`;
  const rotateBase = ["auth", "token", "rotate", "atm_ci00000001", "--team", TEAM_ID, "--replacement-id", "atm_replacement1", "--expires-in", "12h"];
  const rotateRefused = await runCli(rotateBase, { configDir: directory });
  assert.equal(rotateRefused.code, 2);
  const badReplacement = await runCli([...rotateBase.slice(0, -4), "--replacement-id", "replacement", "--expires-in", "12h", "--yes"], { configDir: directory });
  assert.equal(badReplacement.code, 2);
  assert.match(badReplacement.stderr, /--replacement-id/u);
  assert.equal(api.find(rotatePath).length, 0);
  const before = Date.now();
  const rotate = await runCli([...rotateBase, "--yes"], { configDir: directory });
  assert.equal(rotate.code, 0, rotate.stderr);
  const rotation = api.find(rotatePath, "POST")[0];
  assert.match(rotation.headers["idempotency-key"], UUID);
  assert.equal(rotation.body.replacementId, "atm_replacement1");
  const expiresAt = Date.parse(rotation.body.expiresAt);
  assert.ok(expiresAt >= before + 12 * 3_600_000 && expiresAt <= Date.now() + 12 * 3_600_000);
  assert.ok(includesOnce(rotate.stdout, rotated.secret));
  assert.ok(!rotate.stderr.includes(rotated.secret));
  assert.match(rotate.stdout, /atm_replacement1/u);
  const rotateJson = await runCli([...rotateBase, "--yes", "--json"], { configDir: directory });
  assert.deepEqual(JSON.parse(rotateJson.stdout), rotated);
  assert.notEqual(
    api.find(rotatePath, "POST")[1].headers["idempotency-key"],
    rotation.headers["idempotency-key"],
    "each rotation carries its own idempotency key",
  );
});
