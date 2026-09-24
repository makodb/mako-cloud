import { resolve } from "node:path";
import { expect, type Page, type Route, test } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const EARLIER = "2026-08-05T09:30:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PERSONAL_TEAM_ID = "org_personal";
const TEAM_PROJECT_ID = "prj_abcdefgh";
const PERSONAL_PROJECT_ID = "prj_persona1";
const NEW_PROJECT_ID = "prj_newfirst";
const PUBLIC_KEY_VALUE = "mako_public_onboarding_secret";
const PUBLIC_ENDPOINT = "https://cloud-test.makodb.com";

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
      profile: {
        id: "dev_abcdefgh",
        email: "owner@example.test",
        displayName: "Owner",
      },
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

test("personal projects and team projects appear in separate lists with project summaries", async ({
  page,
}) => {
  const api = new HomeApiHarness();
  api.withProjects();
  await api.install(page);

  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Home" })).toBeVisible();
  // Personal projects lead the page; shared projects are grouped under Teams.
  await expect(page.getByRole("heading", { name: "Personal projects" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Mako Test Team" })).toBeVisible();
  const teamNavigation = page.getByRole("navigation", { name: "Teams", exact: true });
  await expect(teamNavigation.getByRole("link")).toHaveText(["Mako Test Team"]);
  await expect(
    page
      .getByRole("navigation", { name: "Workspace destinations" })
      .getByRole("link", { name: "Personal projects", exact: true }),
  ).toHaveAttribute("href", "/");

  const personalRow = page.getByRole("article", { name: "Side project" });
  await expect(personalRow).toBeVisible();
  await expect(personalRow.getByText("Status: active")).toBeVisible();
  await expect(personalRow.getByText("eu-west")).toBeVisible();
  await expect(personalRow.getByText("Personal", { exact: true })).toBeVisible();
  await expect(personalRow.getByText("free", { exact: true })).toBeVisible();
  await expect(personalRow.getByText("Storage 1.0 MiB")).toBeVisible();

  const teamRow = page.getByRole("article", { name: "Mako Test Project" });
  await expect(teamRow).toBeVisible();
  await expect(teamRow.getByText("Status: provisioning")).toBeVisible();
  await expect(teamRow.getByText("local", { exact: true })).toBeVisible();
  await expect(teamRow.getByText("Mako Test Team")).toBeVisible();
  await expect(teamRow.getByText("pro", { exact: true })).toBeVisible();
  await expect(teamRow.getByText("Storage 120.0 MiB · Replication 2.0 MiB")).toBeVisible();
  await expect(page.getByRole("article").and(page.locator("[aria-busy='true']"))).toHaveCount(0);

  // The plan was read once per owner, not per row, and every call the page
  // made is part of the management contract.
  expect(api.billCalls).toEqual({ [PERSONAL_TEAM_ID]: 1, [TEAM_ID]: 1 });
  expect(api.unhandled).toEqual([]);

  // Project names open the project.
  await page.getByRole("button", { name: "Side project", exact: true }).click();
  await expect(page).toHaveURL(/\/projects\/prj_persona1$/u);
});

test("an unavailable owner listing does not pretend the developer has no projects", async ({
  page,
}) => {
  await page.route("**/v1/**", async (route) => {
    await route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({
        apiVersion: "v1",
        error: {
          code: "service_unavailable",
          message: "Project owners unavailable",
          requestId: "req_test01",
          retry: { kind: "never" },
        },
      }),
    });
  });
  await page.goto("/");
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Create your first project" })).toHaveCount(0);
  await expect(page.getByText("No individual projects yet.")).toHaveCount(0);
  await expect(page.getByText("No projects match your search")).toHaveCount(0);
});

test("project search and direct database entry avoid intermediate landing pages", async ({
  page,
}) => {
  const api = new HomeApiHarness();
  api.withProjects();
  await api.install(page);
  await page.goto("/");
  await expect(page.getByRole("complementary", { name: "Workspace navigation" })).toBeVisible();
  await page.getByRole("textbox", { name: "Search projects" }).fill("eu-west");
  const row = page.getByRole("article", { name: "Side project" });
  await expect(row).toBeVisible();
  await expect(page.getByRole("article", { name: "Mako Test Project" })).toHaveCount(0);
  await page.getByRole("textbox", { name: "Search projects" }).fill("no-match");
  await expect(page.getByText("No projects match your search")).toBeVisible();
  await page.getByRole("button", { name: "Clear search" }).click();
  await expect(page.getByRole("article", { name: "Mako Test Project" })).toBeVisible();
  await page.screenshot({
    path: resolve(import.meta.dirname, "../../../.local/console-redesign/home-desktop.png"),
    fullPage: true,
  });
  await row.getByRole("button", { name: "Browse data" }).click();
  await expect(page).toHaveURL(/\/projects\/prj_persona1\/environments\/env_[^/]+\/data$/u);
});

test("a usage source that fails marks only its own row", async ({ page }) => {
  const api = new HomeApiHarness();
  api.withProjects();
  api.usageStatus[PERSONAL_PROJECT_ID] = 503;
  await api.install(page);

  await page.goto("/");
  const personalRow = page.getByRole("article", { name: "Side project" });
  await expect(personalRow.getByText("Usage unavailable")).toBeVisible();
  await expect(personalRow.getByText("free", { exact: true })).toBeVisible();
  await expect(personalRow.getByText("Status: active")).toBeVisible();

  const teamRow = page.getByRole("article", { name: "Mako Test Project" });
  await expect(teamRow.getByText("Storage 120.0 MiB · Replication 2.0 MiB")).toBeVisible();
  await expect(teamRow.getByText("Usage unavailable")).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("a developer creates the first project and continues in its workspace", async ({ page }) => {
  const api = new HomeApiHarness();
  await api.install(page);
  await page.goto("/");
  const guide = page.getByRole("region", { name: "Connect your first project" });
  await expect(guide).toBeVisible();
  await expect(guide.getByLabel("Owner")).toHaveValue("");
  await guide.getByLabel("Project name").fill("First app");
  await guide.getByLabel("Data region").selectOption("local");
  await guide.getByRole("button", { name: "Create and provision" }).click();
  await expect(page).toHaveURL(/\/projects\/prj_newfirst$/u);
  expect(api.createProjectBodies).toEqual([{ name: "First app", region: "local" }]);
  expect(api.issuedKeys).toEqual([]);
  await page.goto("/");
  await expect(page.getByRole("article", { name: "First app" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Connect your first project" })).toHaveCount(0);
});

test("dismissing the guide leaves the ordinary empty state that can reopen it", async ({
  page,
}) => {
  const api = new HomeApiHarness();
  await api.install(page);

  await page.goto("/");
  await expect(page.getByRole("region", { name: "Connect your first project" })).toBeVisible();
  await page.getByRole("button", { name: "Dismiss" }).click();
  await expect(page.getByRole("region", { name: "Connect your first project" })).toHaveCount(0);
  const empty = page.getByRole("region", { name: "Create a project" });
  await expect(empty).toBeVisible();
  await expect(empty.getByRole("button", { name: "Create project" })).toBeVisible();

  // The dismissal is remembered across reloads.
  await page.reload();
  await expect(page.getByRole("region", { name: "Create a project" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Connect your first project" })).toHaveCount(0);

  await page.getByRole("button", { name: "Show the guide again" }).click();
  await expect(page.getByRole("region", { name: "Connect your first project" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Create a project" })).toHaveCount(0);
  expect(api.createProjectBodies).toEqual([]);
  expect(api.unhandled).toEqual([]);
});

interface ProjectFixture {
  id: string;
  teamId: string;
  name: string;
  region: string;
  state: string;
  createdAt: string;
  updatedAt: string;
}

class HomeApiHarness {
  readonly unhandled: string[] = [];
  readonly createProjectBodies: Record<string, unknown>[] = [];
  readonly issuedKeys: string[] = [];
  readonly checkInputs: unknown[] = [];
  readonly billCalls: Record<string, number> = {};
  readonly usageStatus: Record<string, number> = {};
  getProjectCalls = 0;
  teams: Record<string, unknown>[] = [teamFixture()];
  projects: ProjectFixture[] = [];
  environments: Record<string, Record<string, unknown>[]> = {};

  withProjects() {
    this.teams = [teamFixture(), personalSpaceFixture()];
    this.projects = [
      projectFixture(TEAM_PROJECT_ID, TEAM_ID, "Mako Test Project", "local", "provisioning"),
      projectFixture(PERSONAL_PROJECT_ID, PERSONAL_TEAM_ID, "Side project", "eu-west", "active"),
    ];
    this.environments = {
      [TEAM_PROJECT_ID]: [environmentFixture("env_abcdefgh", TEAM_PROJECT_ID)],
      [PERSONAL_PROJECT_ID]: [environmentFixture("env_persona1", PERSONAL_PROJECT_ID)],
    };
  }

  async install(page: Page) {
    await page.route("**/v1/**", (route) => void this.handle(route));
  }

  async handle(route: Route) {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();
    if (request.headers().authorization !== "Bearer developer-session-token") {
      await json(route, apiError("unauthenticated", "A valid developer session is required."), 401);
      return;
    }
    const bill = /^\/v1\/teams\/([^/]+)\/bill$/u.exec(path);
    const project = /^\/v1\/projects\/([^/]+)$/u.exec(path);
    const environments = /^\/v1\/projects\/([^/]+)\/environments$/u.exec(path);
    const environmentScoped =
      /^\/v1\/projects\/([^/]+)\/environments\/([^/]+)\/(observability\/[a-z-]+|connect|connect\/check|credentials\/public)$/u.exec(
        path,
      );

    if (path === "/v1/teams" && method === "GET") {
      await json(route, { items: this.teams });
    } else if (bill !== null && method === "GET") {
      const teamId = bill[1] ?? "";
      this.billCalls[teamId] = (this.billCalls[teamId] ?? 0) + 1;
      await json(route, billFixture(teamId, teamId === PERSONAL_TEAM_ID ? "free" : "pro"));
    } else if (path === "/v1/projects" && method === "GET") {
      const teamId = url.searchParams.get("teamId");
      await json(route, { items: this.projects.filter((item) => item.teamId === teamId) });
    } else if (path === "/v1/projects" && method === "POST") {
      const body = request.postDataJSON() as { name: string; region: string; teamId?: string };
      this.createProjectBodies.push(body);
      if (body.teamId === undefined && !this.teams.some((team) => team.kind === "personal")) {
        this.teams = [...this.teams, personalSpaceFixture()];
      }
      const created = projectFixture(
        NEW_PROJECT_ID,
        body.teamId ?? PERSONAL_TEAM_ID,
        body.name,
        body.region,
        "provisioning",
      );
      this.projects.push(created);
      this.environments[NEW_PROJECT_ID] = [];
      await json(route, created, 202);
    } else if (project !== null && method === "GET") {
      const found = this.projects.find((item) => item.id === project[1]);
      if (found === undefined) {
        await json(route, apiError("not_found", "No such project."), 404);
        return;
      }
      if (found.id === NEW_PROJECT_ID) {
        // The guide's project becomes active on the second poll, and its
        // environment follows one poll later.
        this.getProjectCalls += 1;
        if (this.getProjectCalls >= 2) {
          found.state = "active";
          this.environments[NEW_PROJECT_ID] = [environmentFixture("env_newfirst", NEW_PROJECT_ID)];
        }
      }
      await json(route, found);
    } else if (environments !== null && method === "GET") {
      await json(route, { items: this.environments[environments[1] ?? ""] ?? [] });
    } else if (environmentScoped !== null) {
      const projectId = environmentScoped[1] ?? "";
      const environmentId = environmentScoped[2] ?? "";
      const kind = environmentScoped[3];
      if (kind === "observability/usage" && method === "GET") {
        const status = this.usageStatus[projectId];
        if (status !== undefined) {
          await json(route, apiError("unavailable", "Usage is temporarily unavailable."), status);
          return;
        }
        await json(route, observabilityPage(usageRecords(projectId)));
      } else if (kind === "observability/audit-events" && method === "GET") {
        await json(route, observabilityPage(auditRecords(projectId)));
      } else if (kind?.startsWith("observability/") === true && method === "GET") {
        await json(route, observabilityPage([]));
      } else if (kind === "connect" && method === "GET") {
        await json(route, {
          tenant: { projectId, environmentId },
          publicEndpoint: PUBLIC_ENDPOINT,
          publicKeyId: "key_public01",
          publicKey: "",
          collections: [],
          rxdbClientRange: ">=17 <18",
          templateVersion: 1,
        });
      } else if (kind === "credentials/public" && method === "POST") {
        const body = request.postDataJSON() as { id: string };
        this.issuedKeys.push(body.id);
        await json(
          route,
          {
            credential: { id: body.id, kind: "public", state: "active", createdAt: NOW },
            value: PUBLIC_KEY_VALUE,
          },
          201,
        );
      } else if (kind === "connect/check" && method === "POST") {
        this.checkInputs.push(request.postDataJSON());
        await json(route, {
          checkedAtUnixSeconds: 1_786_579_200,
          steps: [
            { id: "dns_tls", state: "passed", remediationCode: null, retryable: false },
            { id: "public_routing", state: "passed", remediationCode: null, retryable: false },
            { id: "key_metadata", state: "passed", remediationCode: null, retryable: false },
            {
              id: "schema_compatibility",
              state: "skipped",
              remediationCode: null,
              retryable: true,
            },
          ],
        });
      } else {
        this.unhandled.push(`${method} ${path}`);
        await json(route, apiError("internal", "Unhandled management test route."), 500);
      }
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("internal", "Unhandled management test route."), 500);
    }
  }
}

function teamFixture() {
  return {
    id: TEAM_ID,
    name: "Mako Test Team",
    kind: "team",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function personalSpaceFixture() {
  return {
    id: PERSONAL_TEAM_ID,
    name: "Owner",
    kind: "personal",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function projectFixture(
  id: string,
  teamId: string,
  name: string,
  region: string,
  state: string,
): ProjectFixture {
  return { id, teamId, name, region, state, createdAt: NOW, updatedAt: NOW };
}

function environmentFixture(id: string, projectId: string) {
  return { id, projectId, name: "development", state: "active", createdAt: NOW, updatedAt: NOW };
}

function billFixture(teamId: string, planId: string) {
  return {
    teamId,
    planId,
    periodStart: "2026-08-01T00:00:00Z",
    periodEnd: NOW,
    observedAt: NOW,
    finalized: false,
    closedAt: null,
    baseMicroDollars: 0,
    lineItems: [],
    totalMicroDollars: 0,
    creditsMicroDollars: 0,
    balanceMicroDollars: 0,
    collectable: false,
    notice:
      "This bill is informational. Nothing is payable and no charge will be made during the beta.",
  };
}

function usageRecords(projectId: string) {
  if (projectId === TEAM_PROJECT_ID) {
    return [
      // Two storage samples: the newest level is the headline, not their sum.
      usage(EARLIER, "storage_bytes", 90 * 1024 * 1024),
      usage(NOW, "storage_bytes", 120 * 1024 * 1024),
      // Replication is a flow: the period's samples add up.
      usage(EARLIER, "replication_bytes_per_month", 1024 * 1024),
      usage(NOW, "replication_bytes_per_month", 1024 * 1024),
      usage(NOW, "edge_invocations_per_month", 12),
    ];
  }
  if (projectId === PERSONAL_PROJECT_ID) {
    return [usage(NOW, "storage_bytes", 1024 * 1024)];
  }
  return [];
}

function usage(timestamp: string, resource: string, quantity: number) {
  return { timestamp, payload: { kind: "usage", resource, quantity, unit: "bytes" } };
}

function auditRecords(projectId: string) {
  if (projectId === TEAM_PROJECT_ID) {
    return [
      {
        timestamp: NOW,
        payload: {
          kind: "audit",
          teamId: TEAM_ID,
          actorId: "dev_abcdefgh",
          action: "policy.activate",
          target: "todos/policy/1",
          outcome: "allowed",
          requestId: "req_audit01",
          details: "authorization epoch 2",
        },
      },
    ];
  }
  if (projectId === PERSONAL_PROJECT_ID) {
    return [
      {
        timestamp: EARLIER,
        payload: {
          kind: "audit",
          teamId: PERSONAL_TEAM_ID,
          actorId: "dev_abcdefgh",
          action: "project.create",
          target: PERSONAL_PROJECT_ID,
          outcome: "allowed",
          requestId: "req_audit02",
          details: null,
        },
      },
    ];
  }
  return [];
}

function observabilityPage(items: unknown[]) {
  return {
    items,
    nextCursor: null,
    retention: {
      retainedFrom: "2026-07-07T12:00:00.000Z",
      observedAt: NOW,
      retentionSeconds: 2_592_000,
    },
  };
}

function apiError(code: string, message: string) {
  return {
    apiVersion: "v1",
    error: { code, message, requestId: "req_e2e", retry: { kind: "never" } },
  };
}

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}
