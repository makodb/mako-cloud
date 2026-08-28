// Teams, membership, invitations, projects, and environments.
import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

import {
  apiError,
  authHandler,
  configDir,
  ENVIRONMENT_ID,
  environment,
  NOW,
  PERSONAL_TEAM_ID,
  personalTeam,
  PROJECT_ID,
  project,
  runCli,
  signedIn,
  startMockApi,
  TEAM_ID,
  team,
} from "./harness.mjs";

const BEARER = "Bearer developer-session-token-0001";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const DEADLINE = "2026-08-13T12:00:00.000Z";

function includesOnce(text, needle) {
  return text.split(needle).length - 1 === 1;
}

test("teams list, create, get, and rename", async (t) => {
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "POST" && request.path === "/v1/teams") {
          return { status: 201, json: team({ id: "org_newteam01", name: request.body.name }) };
        }
        if (request.method === "GET" && request.path === `/v1/teams/${TEAM_ID}`) return { status: 200, json: team() };
        if (request.method === "PATCH" && request.path === `/v1/teams/${TEAM_ID}`) {
          return { status: 200, json: team({ name: request.body.name }) };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const list = await runCli(["teams", "list"], { configDir: directory });
  assert.equal(list.code, 0, list.stderr);
  assert.equal(api.find("/v1/teams", "GET")[0].headers.authorization, BEARER);
  assert.match(list.stdout, /^id\s+name\s+kind\s+state\s+createdAt\n/u);
  assert.match(list.stdout, /org_personal\s+Your projects\s+personal\s+active/u);
  assert.match(list.stdout, /org_abcdefgh\s+Acme\s+team\s+active/u);
  const listJson = await runCli(["teams", "list", "--json"], { configDir: directory });
  assert.deepEqual(JSON.parse(listJson.stdout), [personalTeam(), team()]);

  const created = await runCli(["teams", "create", "Beta Team", "--json"], { configDir: directory });
  assert.equal(created.code, 0, created.stderr);
  const creation = api.find("/v1/teams", "POST")[0];
  assert.equal(creation.headers.authorization, BEARER);
  assert.deepEqual(creation.body, { name: "Beta Team" });
  assert.deepEqual(JSON.parse(created.stdout), team({ id: "org_newteam01", name: "Beta Team" }));
  const noName = await runCli(["teams", "create"], { configDir: directory });
  assert.equal(noName.code, 2);

  const got = await runCli(["teams", "get", TEAM_ID], { configDir: directory });
  assert.equal(got.code, 0, got.stderr);
  assert.equal(api.find(`/v1/teams/${TEAM_ID}`, "GET").length, 1);
  assert.match(got.stdout, /^id\s+org_abcdefgh\nname\s+Acme\nkind\s+team\n/u);

  const renamed = await runCli(["teams", "rename", TEAM_ID, "Acme Renamed"], { configDir: directory });
  assert.equal(renamed.code, 0, renamed.stderr);
  assert.deepEqual(api.find(`/v1/teams/${TEAM_ID}`, "PATCH")[0].body, { name: "Acme Renamed" });
  assert.match(renamed.stdout, /Acme Renamed/u);
});

test("teams delete needs confirmation and sends the confirmation header; restore carries an idempotency key", async (t) => {
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "DELETE" && request.path === `/v1/teams/${TEAM_ID}`) {
          return { status: 202, json: team({ state: "deletion_grace", deletionDeadline: DEADLINE }) };
        }
        if (request.method === "POST" && request.path === `/v1/teams/${TEAM_ID}/actions/restore`) {
          return { status: 202, json: team() };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const path = `/v1/teams/${TEAM_ID}`;

  const refused = await runCli(["teams", "delete", TEAM_ID], { configDir: directory });
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /without --yes/u);
  assert.equal(refused.stdout, "");
  const mistyped = await runCli(["teams", "delete", TEAM_ID], { configDir: directory, stdin: "org_other\n", isTTY: true });
  assert.equal(mistyped.code, 2);
  assert.equal(api.find(path, "DELETE").length, 0, "refusals send nothing");

  const deleted = await runCli(["teams", "delete", TEAM_ID, "--yes"], { configDir: directory });
  assert.equal(deleted.code, 0, deleted.stderr);
  const deletion = api.find(path, "DELETE")[0];
  assert.equal(deletion.headers.authorization, BEARER);
  assert.equal(deletion.headers.confirmation, TEAM_ID, "the confirmation header names the team");
  assert.match(deleted.stdout, /state\s+deletion_grace/u);
  assert.match(deleted.stderr, /mako teams restore org_abcdefgh/u);
  const typed = await runCli(["teams", "delete", TEAM_ID, "--json"], { configDir: directory, stdin: `${TEAM_ID}\n`, isTTY: true });
  assert.equal(typed.code, 0, typed.stderr);
  assert.equal(JSON.parse(typed.stdout).state, "deletion_grace");
  assert.equal(typed.stderr, `Type "${TEAM_ID}" to delete team: `, "JSON mode adds nothing beyond the prompt");

  const restored = await runCli(["teams", "restore", TEAM_ID, "--json"], { configDir: directory });
  assert.equal(restored.code, 0, restored.stderr);
  const restore = api.find(`${path}/actions/restore`, "POST")[0];
  assert.equal(restore.headers.authorization, BEARER);
  assert.match(restore.headers["idempotency-key"], UUID);
  assert.deepEqual(JSON.parse(restored.stdout), team());
});

test("teams bill prints the notice before any figure and passes the period through", async (t) => {
  const bill = {
    teamId: TEAM_ID,
    planId: "starter",
    periodStart: "2026-07-01T00:00:00.000Z",
    periodEnd: "2026-08-01T00:00:00.000Z",
    observedAt: NOW,
    finalized: true,
    closedAt: "2026-08-01T00:00:00.000Z",
    baseMicroDollars: 25_000_000,
    lineItems: [{ resource: "storage_bytes", quantity: 12, included: 10, overage: 2, amountMicroDollars: 1_500_000 }],
    totalMicroDollars: 26_500_000,
    creditsMicroDollars: 500_000,
    balanceMicroDollars: 26_000_000,
    collectable: true,
    notice: "Beta billing: these figures are informational and nothing is charged.",
  };
  const api = await startMockApi(
    authHandler({
      fallback: (request) =>
        request.method === "GET" && request.path === `/v1/teams/${TEAM_ID}/bill` ? { status: 200, json: bill } : undefined,
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const path = `/v1/teams/${TEAM_ID}/bill`;

  const closed = await runCli(["teams", "bill", TEAM_ID, "--period", "2026-07"], { configDir: directory });
  assert.equal(closed.code, 0, closed.stderr);
  assert.deepEqual(api.find(path, "GET")[0].query, { period: "2026-07" });
  assert.equal(api.find(path, "GET")[0].headers.authorization, BEARER);
  assert.ok(closed.stdout.startsWith(bill.notice), "the notice comes first");
  assert.ok(closed.stdout.indexOf(bill.notice) < closed.stdout.indexOf("$26.50"));
  assert.match(closed.stdout, /total\s+\$26\.50/u);
  assert.match(closed.stdout, /balance\s+\$26\.00/u);
  assert.match(closed.stdout, /storage_bytes\s+12\s+10\s+2\s+\$1\.50/u);

  const live = await runCli(["teams", "bill", TEAM_ID, "--json"], { configDir: directory });
  assert.equal(live.code, 0, live.stderr);
  assert.deepEqual(api.find(path, "GET")[1].query, {});
  assert.deepEqual(JSON.parse(live.stdout), bill);

  for (const period of ["2026-13", "July", "2026-7"]) {
    const bad = await runCli(["teams", "bill", TEAM_ID, "--period", period], { configDir: directory });
    assert.equal(bad.code, 2, period);
    assert.match(bad.stderr, /YYYY-MM/u);
  }
  assert.equal(api.find(path).length, 2);
});

test("team members are listed, updated with a valid role, and removed with confirmation", async (t) => {
  const members = [
    { teamId: TEAM_ID, developerIdentityId: "dev_owner000", role: "owner", createdAt: NOW, updatedAt: NOW },
    { teamId: TEAM_ID, developerIdentityId: "dev_develop00", role: "developer", createdAt: NOW, updatedAt: NOW },
  ];
  const memberPath = `/v1/teams/${TEAM_ID}/members/dev_develop00`;
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "GET" && request.path === `/v1/teams/${TEAM_ID}/members`) {
          return { status: 200, json: { items: members } };
        }
        if (request.method === "PATCH" && request.path === memberPath) {
          return { status: 200, json: { ...members[1], role: request.body.role } };
        }
        if (request.method === "DELETE" && request.path === memberPath) return { status: 204 };
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const list = await runCli(["teams", "members", "list", TEAM_ID], { configDir: directory });
  assert.equal(list.code, 0, list.stderr);
  assert.match(list.stdout, /^developerIdentityId\s+role\s+createdAt\s+updatedAt\n/u);
  assert.match(list.stdout, /dev_owner000\s+owner/u);
  assert.match(list.stdout, /dev_develop00\s+developer/u);
  const listJson = await runCli(["teams", "members", "list", TEAM_ID, "--json"], { configDir: directory });
  assert.deepEqual(JSON.parse(listJson.stdout), members);

  const updated = await runCli(["teams", "members", "update", TEAM_ID, "dev_develop00", "--role", "administrator"], {
    configDir: directory,
  });
  assert.equal(updated.code, 0, updated.stderr);
  assert.deepEqual(api.find(memberPath, "PATCH")[0].body, { role: "administrator" });
  assert.match(updated.stdout, /role\s+administrator/u);
  const badRole = await runCli(["teams", "members", "update", TEAM_ID, "dev_develop00", "--role", "boss"], { configDir: directory });
  assert.equal(badRole.code, 2);
  assert.match(badRole.stderr, /valid roles: owner, administrator, developer, viewer/u);
  assert.equal(api.find(memberPath, "PATCH").length, 1);

  const refused = await runCli(["teams", "members", "remove", TEAM_ID, "dev_develop00"], { configDir: directory });
  assert.equal(refused.code, 2);
  assert.equal(api.find(memberPath, "DELETE").length, 0);
  const removed = await runCli(["teams", "members", "remove", TEAM_ID, "dev_develop00", "--yes"], { configDir: directory });
  assert.equal(removed.code, 0, removed.stderr);
  assert.equal(api.find(memberPath, "DELETE")[0].headers.authorization, BEARER);
  assert.equal(removed.stdout, "");
  assert.match(removed.stderr, /Removed dev_develop00/u);
  const typed = await runCli(["teams", "members", "remove", TEAM_ID, "dev_develop00", "--json"], {
    configDir: directory,
    stdin: "dev_develop00\n",
    isTTY: true,
  });
  assert.equal(typed.code, 0, typed.stderr);
  assert.deepEqual(JSON.parse(typed.stdout), { teamId: TEAM_ID, developerIdentityId: "dev_develop00", removed: true });
});

test("invitations are issued with a one-time token and accepted from an argument, stdin, or a file", async (t) => {
  const invitation = {
    id: "inv_abcdefgh",
    teamId: TEAM_ID,
    email: "friend@example.test",
    role: "developer",
    status: "pending",
    expiresAt: DEADLINE,
    createdAt: NOW,
  };
  const issue = { invitation, token: "invite-token-do-not-log-0001" };
  const membership = { teamId: TEAM_ID, developerIdentityId: "dev_friend000", role: "developer", createdAt: NOW, updatedAt: NOW };
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "POST" && request.path === `/v1/teams/${TEAM_ID}/invitations`) return { status: 201, json: issue };
        if (request.method === "POST" && request.path === "/v1/invitations/inv_abcdefgh/accept") {
          return { status: 200, json: membership };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const invitePath = `/v1/teams/${TEAM_ID}/invitations`;
  const base = ["teams", "invitations", "create", TEAM_ID, "--email", "friend@example.test", "--role", "developer"];

  const before = Date.now();
  const created = await runCli(base, { configDir: directory });
  assert.equal(created.code, 0, created.stderr);
  const request = api.find(invitePath, "POST")[0];
  assert.equal(request.headers.authorization, BEARER);
  assert.equal(request.body.email, "friend@example.test");
  assert.equal(request.body.role, "developer");
  const week = 7 * 86_400_000;
  assert.ok(Date.parse(request.body.expiresAt) >= before + week && Date.parse(request.body.expiresAt) <= Date.now() + week);
  assert.ok(includesOnce(created.stdout, issue.token), "the token is printed once");
  assert.ok(!created.stderr.includes(issue.token));
  assert.match(created.stdout, /shown once/u);
  assert.match(created.stdout, /inv_abcdefgh/u);

  const shorter = await runCli([...base, "--expires-in", "12h", "--json"], { configDir: directory });
  assert.equal(shorter.code, 0, shorter.stderr);
  const twelveHours = 12 * 3_600_000;
  const expiresAt = Date.parse(api.find(invitePath, "POST")[1].body.expiresAt);
  assert.ok(expiresAt >= before + twelveHours && expiresAt <= Date.now() + twelveHours);
  assert.deepEqual(JSON.parse(shorter.stdout), { invitation, secret: issue.token });

  const file = join(directory, "invite.txt");
  const toFile = await runCli([...base, "--secret-file", file], { configDir: directory });
  assert.equal(toFile.code, 0, toFile.stderr);
  assert.ok(!toFile.stdout.includes(issue.token));
  assert.equal(await readFile(file, "utf8"), `${issue.token}\n`);
  assert.equal((await stat(file)).mode & 0o777, 0o600);
  const badRole = await runCli([...base.slice(0, -1), "boss"], { configDir: directory });
  assert.equal(badRole.code, 2);
  assert.equal(api.find(invitePath).length, 3);

  const acceptPath = "/v1/invitations/inv_abcdefgh/accept";
  const accepted = await runCli(["teams", "invitations", "accept", "inv_abcdefgh", issue.token], { configDir: directory });
  assert.equal(accepted.code, 0, accepted.stderr);
  assert.deepEqual(api.find(acceptPath, "POST")[0].body, { token: issue.token });
  assert.equal(api.find(acceptPath, "POST")[0].headers.authorization, BEARER);
  assert.match(accepted.stdout, /developerIdentityId\s+dev_friend000/u);
  const fromStdin = await runCli(["teams", "invitations", "accept", "inv_abcdefgh", "-", "--json"], {
    configDir: directory,
    stdin: `${issue.token}\n`,
  });
  assert.equal(fromStdin.code, 0, fromStdin.stderr);
  assert.deepEqual(api.find(acceptPath, "POST")[1].body, { token: issue.token });
  assert.deepEqual(JSON.parse(fromStdin.stdout), membership);
  const fromFile = await runCli(["teams", "invitations", "accept", "inv_abcdefgh", `@${file}`], { configDir: directory });
  assert.equal(fromFile.code, 0, fromFile.stderr);
  assert.deepEqual(api.find(acceptPath, "POST")[2].body, { token: issue.token });
});

test("projects list covers every team with the personal space first, or one team on request", async (t) => {
  const personalProject = project({ id: "prj_personal1", teamId: PERSONAL_TEAM_ID, name: "Side project" });
  const api = await startMockApi(
    authHandler({
      teams: [team(), personalTeam()],
      fallback: (request) => {
        if (request.method === "GET" && request.path === "/v1/projects") {
          const items = { [PERSONAL_TEAM_ID]: [personalProject], [TEAM_ID]: [project()] }[request.query.teamId] ?? [];
          return { status: 200, json: { items } };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const all = await runCli(["projects", "list"], { configDir: directory });
  assert.equal(all.code, 0, all.stderr);
  assert.equal(api.find("/v1/teams", "GET").length, 1);
  assert.deepEqual(
    api.find("/v1/projects", "GET").map((request) => request.query.teamId),
    [PERSONAL_TEAM_ID, TEAM_ID],
    "the personal space is listed first",
  );
  assert.equal(api.find("/v1/projects", "GET")[0].headers.authorization, BEARER);
  assert.match(all.stdout, /^id\s+name\s+teamName\s+teamId\s+region\s+state\n/u);
  assert.match(all.stdout, /prj_personal1\s+Side project\s+Your projects\s+org_personal\s+us-east-1\s+active/u);
  assert.match(all.stdout, /prj_abcdefgh\s+Mako Test Project\s+Acme\s+org_abcdefgh/u);
  assert.ok(all.stdout.indexOf("prj_personal1") < all.stdout.indexOf("prj_abcdefgh"));
  const allJson = await runCli(["projects", "list", "--json"], { configDir: directory });
  assert.deepEqual(JSON.parse(allJson.stdout), [
    { ...personalProject, teamName: "Your projects" },
    { ...project(), teamName: "Acme" },
  ]);

  const one = await runCli(["projects", "list", "--team", TEAM_ID, "--json"], { configDir: directory });
  assert.equal(one.code, 0, one.stderr);
  assert.equal(api.find("/v1/teams", "GET").length, 2, "one team needs no team listing");
  assert.deepEqual(api.find("/v1/projects", "GET")[4].query, { teamId: TEAM_ID });
  assert.deepEqual(JSON.parse(one.stdout), [project()]);
});

test("projects create sends an idempotency key, waits for provisioning, and exits 6 on the deadline", async (t) => {
  let polls = 0;
  let stuck = false;
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "POST" && request.path === "/v1/projects") {
          return {
            status: 202,
            json: project({
              state: "provisioning",
              teamId: request.body.teamId ?? PERSONAL_TEAM_ID,
              name: request.body.name,
              region: request.body.region,
            }),
          };
        }
        if (request.method === "GET" && request.path === `/v1/projects/${PROJECT_ID}`) {
          polls += 1;
          return { status: 200, json: project({ state: stuck || polls < 3 ? "provisioning" : "active" }) };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);
  const env = { MAKO_WAIT_INTERVAL_MS: "1" };

  const created = await runCli(
    ["projects", "create", "Mako Test Project", "--region", "us-east-1", "--team", TEAM_ID, "--wait", "--json"],
    { configDir: directory, env },
  );
  assert.equal(created.code, 0, created.stderr);
  const creation = api.find("/v1/projects", "POST")[0];
  assert.equal(creation.headers.authorization, BEARER);
  assert.match(creation.headers["idempotency-key"], UUID);
  assert.deepEqual(creation.body, { teamId: TEAM_ID, name: "Mako Test Project", region: "us-east-1" });
  assert.ok(polls >= 3, "polled until active");
  assert.equal(JSON.parse(created.stdout).state, "active");
  assert.match(created.stderr, /project prj_abcdefgh: provisioning/u);
  assert.match(created.stderr, /project prj_abcdefgh: active/u);

  const pollsBefore = polls;
  const personal = await runCli(["projects", "create", "Side project", "--region", "us-east-1"], { configDir: directory, env });
  assert.equal(personal.code, 0, personal.stderr);
  assert.deepEqual(api.find("/v1/projects", "POST")[1].body, { name: "Side project", region: "us-east-1" });
  assert.equal(polls, pollsBefore, "without --wait nothing is polled");
  assert.match(personal.stdout, /state\s+provisioning/u);
  const noRegion = await runCli(["projects", "create", "Side project"], { configDir: directory });
  assert.equal(noRegion.code, 2);

  stuck = true;
  const late = await runCli(["projects", "create", "Slow", "--region", "us-east-1", "--wait", "--timeout", "1"], {
    configDir: directory,
    env: { MAKO_WAIT_INTERVAL_MS: "200" },
  });
  assert.equal(late.code, 6);
  assert.match(late.stderr, /timed out waiting; last observed: project prj_abcdefgh: provisioning/u);
  assert.equal(late.stdout, "");
});

test("projects get, suspend, restore, and delete", async (t) => {
  const path = `/v1/projects/${PROJECT_ID}`;
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "GET" && request.path === path) return { status: 200, json: project() };
        if (request.method === "POST" && request.path === `${path}/actions/suspend`) {
          return { status: 202, json: project({ state: "suspended" }) };
        }
        if (request.method === "POST" && request.path === `${path}/actions/restore`) return { status: 202, json: project() };
        if (request.method === "DELETE" && request.path === path) {
          return { status: 202, json: project({ state: "deletion_grace", deletionDeadline: DEADLINE }) };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const got = await runCli(["projects", "get", PROJECT_ID, "--json"], { configDir: directory });
  assert.equal(got.code, 0, got.stderr);
  assert.equal(api.find(path, "GET")[0].headers.authorization, BEARER);
  assert.deepEqual(JSON.parse(got.stdout), project());

  const refused = await runCli(["projects", "suspend", PROJECT_ID], { configDir: directory });
  assert.equal(refused.code, 2);
  assert.equal(api.find(`${path}/actions/suspend`).length, 0);
  const suspended = await runCli(["projects", "suspend", PROJECT_ID, "--yes"], { configDir: directory });
  assert.equal(suspended.code, 0, suspended.stderr);
  assert.match(api.find(`${path}/actions/suspend`, "POST")[0].headers["idempotency-key"], UUID);
  assert.match(suspended.stdout, /state\s+suspended/u);

  const restored = await runCli(["projects", "restore", PROJECT_ID, "--json"], { configDir: directory });
  assert.equal(restored.code, 0, restored.stderr);
  assert.match(api.find(`${path}/actions/restore`, "POST")[0].headers["idempotency-key"], UUID);
  assert.deepEqual(JSON.parse(restored.stdout), project());

  const deleteRefused = await runCli(["projects", "delete", PROJECT_ID], { configDir: directory });
  assert.equal(deleteRefused.code, 2);
  assert.equal(api.find(path, "DELETE").length, 0);
  const deleted = await runCli(["projects", "delete", PROJECT_ID, "--yes"], { configDir: directory });
  assert.equal(deleted.code, 0, deleted.stderr);
  assert.equal(api.find(path, "DELETE")[0].headers.confirmation, PROJECT_ID);
  assert.match(deleted.stdout, /state\s+deletion_grace/u);
  assert.match(deleted.stderr, /mako projects restore prj_abcdefgh/u);
});

test("projects rename sends the new name; transfer needs confirmation, names the new owner, and maps refusals to exit codes", async (t) => {
  const path = `/v1/projects/${PROJECT_ID}`;
  const transferPath = `${path}/actions/transfer`;
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "PATCH" && request.path === path) {
          return { status: 200, json: project({ name: request.body.name }) };
        }
        if (request.method === "POST" && request.path === transferPath) {
          const target = request.body.teamId;
          if (target === "org_notmine0") {
            return apiError("permission_denied", "you must administer both the current owner and the target", 403);
          }
          if (target === TEAM_ID) return apiError("conflict", `${TEAM_ID} already owns ${PROJECT_ID}`, 409);
          return { status: 200, json: project({ teamId: target ?? PERSONAL_TEAM_ID }) };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const renamed = await runCli(["projects", "rename", PROJECT_ID, "Mako Renamed"], { configDir: directory });
  assert.equal(renamed.code, 0, renamed.stderr);
  const rename = api.find(path, "PATCH")[0];
  assert.equal(rename.headers.authorization, BEARER);
  assert.deepEqual(rename.body, { name: "Mako Renamed" });
  assert.match(renamed.stdout, /name\s+Mako Renamed/u);
  const renamedJson = await runCli(["projects", "rename", PROJECT_ID, "Mako Renamed", "--json"], { configDir: directory });
  assert.equal(renamedJson.code, 0, renamedJson.stderr);
  assert.deepEqual(JSON.parse(renamedJson.stdout), project({ name: "Mako Renamed" }));
  const noName = await runCli(["projects", "rename", PROJECT_ID], { configDir: directory });
  assert.equal(noName.code, 2);
  assert.equal(api.find(path, "PATCH").length, 2);

  const refused = await runCli(["projects", "transfer", PROJECT_ID, "--team", "org_newteam01"], { configDir: directory });
  assert.equal(refused.code, 2);
  assert.match(refused.stderr, /without --yes/u);
  assert.equal(refused.stdout, "");
  const mistyped = await runCli(["projects", "transfer", PROJECT_ID], { configDir: directory, stdin: "prj_other\n", isTTY: true });
  assert.equal(mistyped.code, 2);
  assert.equal(api.find(transferPath).length, 0, "refusals send nothing");

  const toTeam = await runCli(["projects", "transfer", PROJECT_ID, "--team", "org_newteam01", "--yes"], { configDir: directory });
  assert.equal(toTeam.code, 0, toTeam.stderr);
  const transfer = api.find(transferPath, "POST")[0];
  assert.equal(transfer.headers.authorization, BEARER);
  assert.equal(transfer.headers.confirmation, PROJECT_ID, "the confirmation header names the project");
  assert.deepEqual(transfer.body, { teamId: "org_newteam01" });
  assert.match(toTeam.stdout, /teamId\s+org_newteam01/u);
  assert.match(toTeam.stderr, /now belongs to org_newteam01/u);
  assert.match(toTeam.stderr, /audited under both/u);

  const toPersonal = await runCli(["projects", "transfer", PROJECT_ID, "--json"], {
    configDir: directory,
    stdin: `${PROJECT_ID}\n`,
    isTTY: true,
  });
  assert.equal(toPersonal.code, 0, toPersonal.stderr);
  const personal = api.find(transferPath, "POST")[1];
  assert.deepEqual(personal.body, {}, "no --team means the personal space");
  assert.equal(personal.headers.confirmation, PROJECT_ID);
  assert.deepEqual(JSON.parse(toPersonal.stdout), project({ teamId: PERSONAL_TEAM_ID }));
  assert.equal(toPersonal.stderr, `Type "${PROJECT_ID}" to transfer project: `, "JSON mode adds nothing beyond the prompt");

  const forbidden = await runCli(["projects", "transfer", PROJECT_ID, "--team", "org_notmine0", "--yes"], { configDir: directory });
  assert.equal(forbidden.code, 3);
  assert.match(forbidden.stderr, /you must administer both the current owner and the target/u);
  assert.equal(forbidden.stdout, "");
  const conflict = await runCli(["projects", "transfer", PROJECT_ID, "--team", TEAM_ID, "--yes"], { configDir: directory });
  assert.equal(conflict.code, 5);
  assert.match(conflict.stderr, /already owns prj_abcdefgh/u);
  assert.equal(conflict.stdout, "");
  assert.equal(api.find(transferPath, "POST").length, 4);
  assert.equal(api.unhandled.length, 0, `unhandled: ${api.unhandled.join(", ")}`);
});

test("envs take the project from --project or MAKO_PROJECT_ID and follow the same lifecycle", async (t) => {
  const base = `/v1/projects/${PROJECT_ID}/environments`;
  const one = `${base}/${ENVIRONMENT_ID}`;
  let polls = 0;
  const api = await startMockApi(
    authHandler({
      fallback: (request) => {
        if (request.method === "GET" && request.path === base) return { status: 200, json: { items: [environment()] } };
        if (request.method === "POST" && request.path === base) {
          return { status: 202, json: environment({ id: "env_staging01", name: request.body.name, state: "provisioning" }) };
        }
        if (request.method === "GET" && request.path === `${base}/env_staging01`) {
          polls += 1;
          return { status: 200, json: environment({ id: "env_staging01", name: "staging", state: polls < 2 ? "provisioning" : "active" }) };
        }
        if (request.method === "GET" && request.path === one) return { status: 200, json: environment() };
        if (request.method === "POST" && request.path === `${one}/actions/suspend`) {
          return { status: 202, json: environment({ state: "suspended" }) };
        }
        if (request.method === "POST" && request.path === `${one}/actions/restore`) return { status: 202, json: environment() };
        if (request.method === "DELETE" && request.path === one) {
          return { status: 202, json: environment({ state: "deletion_grace", deletionDeadline: DEADLINE }) };
        }
        return undefined;
      },
    }),
  );
  t.after(() => api.close());
  const directory = await signedIn(t, api);

  const list = await runCli(["envs", "list", "--project", PROJECT_ID], { configDir: directory });
  assert.equal(list.code, 0, list.stderr);
  assert.equal(api.find(base, "GET")[0].headers.authorization, BEARER);
  assert.match(list.stdout, /^id\s+name\s+state\s+createdAt\s+updatedAt\n/u);
  assert.match(list.stdout, /env_abcdefgh\s+development\s+active/u);
  const noProject = await runCli(["envs", "list"], { configDir: directory });
  assert.equal(noProject.code, 2);
  assert.match(noProject.stderr, /--project/u);
  const fromEnvironment = await runCli(["envs", "list", "--json"], { configDir: directory, env: { MAKO_PROJECT_ID: PROJECT_ID } });
  assert.equal(fromEnvironment.code, 0, fromEnvironment.stderr);
  assert.deepEqual(JSON.parse(fromEnvironment.stdout), [environment()]);
  assert.equal(api.find(base, "GET").length, 2);

  const created = await runCli(["envs", "create", "staging", "-p", PROJECT_ID, "--wait", "--json"], {
    configDir: directory,
    env: { MAKO_WAIT_INTERVAL_MS: "1" },
  });
  assert.equal(created.code, 0, created.stderr);
  const creation = api.find(base, "POST")[0];
  assert.deepEqual(creation.body, { name: "staging" });
  assert.match(creation.headers["idempotency-key"], UUID);
  assert.ok(polls >= 2);
  assert.equal(JSON.parse(created.stdout).state, "active");
  assert.match(created.stderr, /environment env_staging01: provisioning/u);

  const got = await runCli(["envs", "get", ENVIRONMENT_ID, "--project", PROJECT_ID], { configDir: directory });
  assert.equal(got.code, 0, got.stderr);
  assert.match(got.stdout, /name\s+development/u);

  const refused = await runCli(["envs", "suspend", ENVIRONMENT_ID, "--project", PROJECT_ID], { configDir: directory });
  assert.equal(refused.code, 2);
  assert.equal(api.find(`${one}/actions/suspend`).length, 0);
  const suspended = await runCli(["envs", "suspend", ENVIRONMENT_ID, "--project", PROJECT_ID, "--yes"], { configDir: directory });
  assert.equal(suspended.code, 0, suspended.stderr);
  assert.match(api.find(`${one}/actions/suspend`, "POST")[0].headers["idempotency-key"], UUID);
  assert.match(suspended.stdout, /state\s+suspended/u);
  const restored = await runCli(["envs", "restore", ENVIRONMENT_ID, "--project", PROJECT_ID, "--json"], { configDir: directory });
  assert.equal(restored.code, 0, restored.stderr);
  assert.match(api.find(`${one}/actions/restore`, "POST")[0].headers["idempotency-key"], UUID);
  assert.deepEqual(JSON.parse(restored.stdout), environment());

  const deleteRefused = await runCli(["envs", "delete", ENVIRONMENT_ID, "--project", PROJECT_ID], { configDir: directory });
  assert.equal(deleteRefused.code, 2);
  assert.equal(api.find(one, "DELETE").length, 0);
  const deleted = await runCli(["envs", "delete", ENVIRONMENT_ID, "--project", PROJECT_ID, "--yes"], { configDir: directory });
  assert.equal(deleted.code, 0, deleted.stderr);
  assert.equal(api.find(one, "DELETE")[0].headers.confirmation, ENVIRONMENT_ID);
  assert.match(deleted.stdout, /state\s+deletion_grace/u);
  assert.match(deleted.stderr, /mako envs restore env_abcdefgh --project prj_abcdefgh/u);
  assert.equal(api.unhandled.length, 0, `unhandled: ${api.unhandled.join(", ")}`);
});
