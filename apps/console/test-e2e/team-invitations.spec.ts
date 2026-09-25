import { expect, type Page, type Route, test } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const PERSONAL_TEAM_ID = "org_personal";
const TEAM_ID = "org_websiteteam";
const INVITATION_ID = "inv_abcdefgh1234";
const TOKEN = "invitation-token-that-is-at-least-thirty-two-characters";

declare global {
  interface Window {
    __MAKO_CONSOLE__?: {
      managementEndpoint: string;
      developerAuth: {
        loadSession(): Promise<unknown>;
        beginSignIn(): Promise<void>;
        signOut(): Promise<void>;
        subscribe(): () => void;
      };
      developerWorkspaceEnabled?: boolean;
      developerExplorerAdminEnabled?: boolean;
      developerDataJobsEnabled?: boolean;
      developerSyncDetailsEnabled?: boolean;
      developerRestoreEnabled?: boolean;
    };
  }
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const session = {
      accessToken: "developer-session-token",
      expiresAt: "2099-01-01T00:00:00.000Z",
      audience: "mako-management",
      profile: { id: "dev_abcdefgh", email: "lead@example.test", displayName: "Lead" },
    };
    window.__MAKO_CONSOLE__ = {
      managementEndpoint: "http://127.0.0.1:4174",
      developerAuth: {
        loadSession: async () => session,
        beginSignIn: async () => {},
        signOut: async () => {},
        subscribe: () => () => {},
      },
    };
  });
});

// A lead setting up a team from nothing: the console used to offer no way to
// create one, and an invitation handed out only a bare token, with nowhere for
// the invitee to use it.
test("a lead creates a team from Home and hands out an invitation link", async ({ page }) => {
  const api = new TeamApi();
  await api.install(page);

  await page.goto("/");
  await page.getByRole("button", { name: "New team" }).click();
  await page.getByLabel("Team name").fill("Website team");
  await page.getByRole("button", { name: "Create team" }).click();
  await expect(page).toHaveURL(new RegExp(`/teams/${TEAM_ID}$`, "u"));
  await expect(page.getByRole("heading", { name: "Website team" })).toBeVisible();
  expect(api.created).toEqual([{ name: "Website team" }]);
  // The lead's own row says so; members are otherwise listed by identity id.
  await expect(page.getByLabel("Role for dev_abcdefgh (you)")).toHaveValue("owner");

  await page.getByLabel("Email", { exact: true }).fill("teammate@example.test");
  await page.getByLabel("Role", { exact: true }).selectOption("developer");
  await page.getByRole("button", { name: "Create invitation" }).click();
  await expect(page.getByText("Copy this invitation link now.")).toBeVisible();
  await page.getByRole("button", { name: "Reveal value" }).click();
  const origin = new URL(page.url()).origin;
  await expect(
    page.getByRole("complementary", { name: /Copy this invitation link now/u }).locator("code"),
  ).toHaveText(`${origin}/invitations/${INVITATION_ID}#token=${encodeURIComponent(TOKEN)}`);
  expect(api.invited).toMatchObject([{ email: "teammate@example.test", role: "developer" }]);
  expect(api.unhandled).toEqual([]);
});

test("the invitation link fills in its token, leaves no token in the address bar, and joins the team", async ({
  page,
}) => {
  const api = new TeamApi();
  await api.install(page);

  await page.goto(`/invitations/${INVITATION_ID}#token=${encodeURIComponent(TOKEN)}`);
  await expect(page.getByLabel("Invitation token")).toHaveValue(TOKEN);
  expect(new URL(page.url()).hash).toBe("");
  await page.getByRole("button", { name: "Accept invitation" }).click();
  await expect(page).toHaveURL(new RegExp(`/teams/${TEAM_ID}$`, "u"));
  expect(api.accepted).toEqual([{ token: TOKEN }]);
  expect(api.unhandled).toEqual([]);
});

test("a teammate sees the owner as owner, and an administrator cannot edit the owner's row", async ({
  page,
}) => {
  const api = new TeamApi();
  api.teams = [...api.teams, team(TEAM_ID, "Website team", "team")];
  // The signed-in developer (dev_abcdefgh) is a developer; the owner is someone else.
  api.members = [membership("dev_owner0001", "owner"), membership("dev_abcdefgh", "developer")];
  await api.install(page);
  await page.goto(`/teams/${TEAM_ID}`);
  await expect(page.getByLabel("Role for dev_owner0001")).toHaveValue("owner");
  await expect(page.getByLabel("Role for dev_owner0001")).toBeDisabled();

  api.members = [
    membership("dev_owner0001", "owner"),
    membership("dev_abcdefgh", "administrator"),
    membership("dev_member001", "viewer"),
  ];
  await page.reload();
  await expect(page.getByLabel("Role for dev_owner0001")).toHaveValue("owner");
  await expect(page.getByLabel("Role for dev_owner0001")).toBeDisabled();
  await expect(page.getByRole("button", { name: "Remove dev_owner0001" })).toBeDisabled();
  await expect(page.getByLabel("Role for dev_member001")).toBeEnabled();
  await expect(page.getByRole("button", { name: "Remove dev_member001" })).toBeEnabled();
  expect(api.unhandled).toEqual([]);
});

test("a removed member opening the team is told plainly, not left on Loading", async ({ page }) => {
  const api = new TeamApi();
  api.removed = true;
  await api.install(page);
  await page.goto(`/teams/${TEAM_ID}`);
  await expect(page.getByRole("heading", { name: "Team unavailable" })).toBeVisible();
  await expect(page.getByText("You are not a member of this team.")).toBeVisible();
  await expect(page.getByText("Loading…")).toHaveCount(0);
  await expect(page.getByText("forbidden")).toHaveCount(0);
  expect(api.unhandled).toEqual([]);
});

class TeamApi {
  readonly unhandled: string[] = [];
  readonly created: unknown[] = [];
  readonly invited: unknown[] = [];
  readonly accepted: unknown[] = [];
  teams: Record<string, unknown>[] = [team(PERSONAL_TEAM_ID, "Lead", "personal")];
  members = [membership("dev_abcdefgh", "owner")];
  removed = false;

  async install(page: Page) {
    await page.route("**/v1/**", (route) => void this.handle(route));
  }

  async handle(route: Route) {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();
    if (path === "/v1/teams" && method === "GET") {
      await json(route, { items: this.teams });
    } else if (path === "/v1/teams" && method === "POST") {
      const body = request.postDataJSON() as { name: string };
      this.created.push(body);
      const created = team(TEAM_ID, body.name, "team");
      this.teams = [...this.teams, created];
      await json(route, created, 201);
    } else if (path.startsWith(`/v1/teams/${TEAM_ID}`) && method === "GET" && this.removed) {
      await json(
        route,
        {
          apiVersion: "v1",
          error: {
            code: "forbidden",
            message: "team action is forbidden",
            requestId: "req_e2e",
            retry: { kind: "never" },
          },
        },
        403,
      );
    } else if (path === "/v1/projects" && method === "GET" && this.removed) {
      await json(
        route,
        {
          apiVersion: "v1",
          error: {
            code: "forbidden",
            message: "project action is forbidden",
            requestId: "req_e2e",
            retry: { kind: "never" },
          },
        },
        403,
      );
    } else if (path === `/v1/teams/${TEAM_ID}` && method === "GET") {
      await json(
        route,
        this.teams.find((item) => item.id === TEAM_ID) ?? team(TEAM_ID, "Website team", "team"),
      );
    } else if (path === `/v1/teams/${TEAM_ID}/members` && method === "GET") {
      await json(route, { items: this.members });
    } else if (path === `/v1/teams/${TEAM_ID}/invitations` && method === "POST") {
      const body = request.postDataJSON() as { email: string; role: string; expiresAt: string };
      this.invited.push(body);
      await json(
        route,
        {
          invitation: {
            id: INVITATION_ID,
            teamId: TEAM_ID,
            email: body.email,
            role: body.role,
            status: "pending",
            expiresAt: body.expiresAt,
            createdAt: NOW,
          },
          token: TOKEN,
        },
        201,
      );
    } else if (path === `/v1/invitations/${INVITATION_ID}/accept` && method === "POST") {
      this.accepted.push(request.postDataJSON());
      await json(route, membership("dev_abcdefgh", "developer"));
    } else if (path === "/v1/projects" && method === "GET") {
      await json(route, { items: [] });
    } else if (/^\/v1\/teams\/[^/]+\/bill$/u.test(path) && method === "GET") {
      await json(route, { error: { code: "not_found", message: "no bill" } }, 404);
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(
        route,
        {
          apiVersion: "v1",
          error: {
            code: "not_found",
            message: `Unhandled ${method} ${path}`,
            requestId: "req_e2e",
            retry: { kind: "never" },
          },
        },
        404,
      );
    }
  }
}

function team(id: string, name: string, kind: "team" | "personal") {
  return { id, name, kind, state: "active", createdAt: NOW, updatedAt: NOW };
}

function membership(developerIdentityId: string, role: string) {
  return { teamId: TEAM_ID, developerIdentityId, role, createdAt: NOW, updatedAt: NOW };
}

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}
