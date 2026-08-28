import { expect, test, type Page, type Route } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const EARLIER = "2026-08-06T11:00:00.000Z";
const LATER = "2026-08-07T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const PREVIEW_ENVIRONMENT_ID = "env_ijklmnop";
const PUBLIC_KEY = "mako_pk.browser_public_key_value";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const session = {
      accessToken: "developer-session-token",
      expiresAt: "2099-01-01T00:00:00.000Z",
      audience: "mako-management",
      profile: { id: "dev_abcdefgh", email: "owner@example.test", displayName: "Owner" },
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

test("a project with two environments shows both with readiness and the selected environment's keys and quickstart", async ({
  page,
}) => {
  const api = new ProjectHomeHarness();
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}`);
  await expect(page.getByRole("heading", { name: "Mako Test Project", level: 1 })).toBeVisible();

  // The sidebar names the owner and lists every environment with its state,
  // each linking into that environment's workspace.
  const sidebar = page.getByRole("complementary", { name: "Project navigation" });
  await expect(sidebar.getByRole("link", { name: "Mako Test Team" })).toHaveAttribute(
    "href",
    `/teams/${TEAM_ID}`,
  );
  await expect(sidebar.getByRole("link", { name: /development/u })).toHaveAttribute(
    "href",
    `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/overview`,
  );
  await expect(sidebar.getByRole("link", { name: /preview/u })).toHaveAttribute(
    "href",
    `/projects/${PROJECT_ID}/environments/${PREVIEW_ENVIRONMENT_ID}/overview`,
  );

  // Both environments are in the table with their own readiness, each
  // stamped with when it was observed.
  const environments = page.getByRole("region", { name: "Environments" });
  const developmentRow = environments.getByRole("row").filter({ hasText: "development" });
  await expect(developmentRow.getByText("Current", { exact: true })).toBeVisible();
  await expect(developmentRow.getByText("Ready", { exact: true })).toBeVisible();
  await expect(developmentRow.getByText("2 collections · 1 functions")).toBeVisible();
  await expect(developmentRow.getByText(/Observed /u)).toBeVisible();
  const previewRow = environments.getByRole("row").filter({ hasText: "preview" });
  await expect(previewRow.getByText("Stale", { exact: true })).toBeVisible();
  await expect(previewRow.getByText("Not ready", { exact: true })).toBeVisible();
  await expect(previewRow.getByText(/Observed /u)).toBeVisible();

  // The first active environment is selected by default and its keys, API
  // URL, and quickstart are shown.
  await expect(page.getByLabel("Select development")).toBeChecked();
  await expect(page.getByRole("heading", { name: "development", level: 2 })).toBeVisible();
  const keys = page.getByRole("region", { name: "Keys and API URL" });
  await expect(keys.getByText("https://cloud-test.makodb.com", { exact: true })).toBeVisible();
  await expect(keys.getByText("key_public01", { exact: true })).toBeVisible();
  await expect(keys.getByLabel("Public project key")).toHaveValue(PUBLIC_KEY);
  await expect(keys.locator("pre")).toContainText(`projectId: "${PROJECT_ID}"`);
  await expect(keys.locator("pre")).toContainText(`environmentId: "${ENVIRONMENT_ID}"`);
  await expect(keys.locator("pre")).toContainText('collectionId: "todos"');
  await expect(keys.locator("pre")).toContainText("schemaVersion: 3");
  await expect(keys.locator("pre")).toContainText(`publicProjectKey: "${PUBLIC_KEY}"`);

  // Usage is summed per resource, health is listed, and activity is newest
  // first; every summary names its observation time.
  const usage = page.getByRole("region", { name: "Usage", exact: true });
  await expect(usage.getByRole("cell", { name: "storage bytes" })).toBeVisible();
  await expect(usage.getByRole("cell", { name: "3.0 KiB" })).toBeVisible();
  await expect(usage.getByRole("cell", { name: "12 invocations" })).toBeVisible();
  const health = page.getByRole("region", { name: "Data-plane health" });
  await expect(health.getByText("storage in local: healthy")).toBeVisible();
  const activity = page.getByRole("region", { name: "Recent activity" });
  const events = activity.getByRole("listitem");
  await expect(events).toHaveCount(2);
  await expect(events.first()).toContainText("environment.create");
  await expect(events.first()).toContainText("dev_abcdefgh");
  await expect(events.nth(1)).toContainText("policy.activate");
  for (const region of [keys, usage, health, activity]) {
    await expect(region.getByText("Current", { exact: true })).toBeVisible();
    await expect(region.getByText(/^Observed /u)).toBeVisible();
  }

  // Selecting the other environment swaps every selected-environment
  // summary; the preview has no recoverable key and no collection yet.
  await page.getByLabel("Select preview").check();
  await expect(page.getByRole("heading", { name: "preview", level: 2 })).toBeVisible();
  await expect(keys.getByText("No recoverable public key is available")).toBeVisible();
  await expect(keys.getByText("No collection exists yet")).toBeVisible();
  await expect(keys.locator("pre")).toContainText(`environmentId: "${PREVIEW_ENVIRONMENT_ID}"`);
  await expect(keys.locator("pre")).toContainText(
    'publicProjectKey: "mako_pk.PASTE_ONE_TIME_PUBLIC_KEY"',
  );
  expect(api.unhandled).toEqual([]);
});

test("a failing summary source marks only its own panel unavailable", async ({ page }) => {
  const api = new ProjectHomeHarness();
  api.failing.add("health");
  api.failing.add(`summary:${PREVIEW_ENVIRONMENT_ID}`);
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}`);
  const health = page.getByRole("region", { name: "Data-plane health" });
  await expect(health.getByText("Unavailable", { exact: true })).toBeVisible();
  await expect(health.getByText("This summary is unavailable")).toBeVisible();
  await expect(health.getByRole("alert")).toContainText("The health source is down.");
  await expect(health.getByText(/Observed /u)).toHaveCount(0);

  // Keys, usage, and activity are untouched by the failing health source.
  const keys = page.getByRole("region", { name: "Keys and API URL" });
  await expect(keys.getByLabel("Public project key")).toHaveValue(PUBLIC_KEY);
  const usage = page.getByRole("region", { name: "Usage", exact: true });
  await expect(usage.getByText("Current", { exact: true })).toBeVisible();
  await expect(usage.getByRole("cell", { name: "3.0 KiB" })).toBeVisible();
  const activity = page.getByRole("region", { name: "Recent activity" });
  await expect(activity.getByText("Current", { exact: true })).toBeVisible();

  // One environment's failing workspace summary marks that row alone.
  const environments = page.getByRole("region", { name: "Environments" });
  const previewRow = environments.getByRole("row").filter({ hasText: "preview" });
  await expect(previewRow.getByText("Unavailable", { exact: true })).toBeVisible();
  await expect(previewRow.getByText("The workspace summary is down.")).toBeVisible();
  const developmentRow = environments.getByRole("row").filter({ hasText: "development" });
  await expect(developmentRow.getByText("Ready", { exact: true })).toBeVisible();
  expect(api.unhandled).toEqual([]);
});

test("sidebar destinations navigate to usage and activity and mark the current one", async ({
  page,
}) => {
  const api = new ProjectHomeHarness();
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}`);
  const nav = page.getByRole("navigation", { name: "Project destinations" });
  await expect(nav.getByRole("link")).toHaveText(["Overview", "Usage", "Activity", "Settings"]);
  await expect(nav.getByRole("link", { name: "Overview" })).toHaveAttribute("aria-current", "page");
  await expect(nav.getByRole("link", { name: "Usage" })).not.toHaveAttribute(
    "aria-current",
    "page",
  );

  await nav.getByRole("link", { name: "Usage" }).click();
  await expect(page).toHaveURL(/\/projects\/prj_abcdefgh\/usage$/u);
  await expect(nav.getByRole("link", { name: "Usage" })).toHaveAttribute("aria-current", "page");
  await expect(nav.getByRole("link", { name: "Overview" })).not.toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(page.getByRole("heading", { name: "Mako Test Project", level: 1 })).toBeVisible();

  await nav.getByRole("link", { name: "Activity" }).click();
  await expect(page).toHaveURL(/\/projects\/prj_abcdefgh\/activity$/u);
  await expect(nav.getByRole("link", { name: "Activity" })).toHaveAttribute("aria-current", "page");
  await expect(nav.getByRole("link", { name: "Usage" })).not.toHaveAttribute(
    "aria-current",
    "page",
  );

  // A deep link opens inside the same shell with the destination marked.
  await page.goto(`/projects/${PROJECT_ID}/settings`);
  await expect(nav.getByRole("link", { name: "Settings" })).toHaveAttribute("aria-current", "page");
  await expect(
    page.getByRole("complementary", { name: "Project navigation" }).getByRole("link", {
      name: "Mako Test Team",
    }),
  ).toBeVisible();

  await nav.getByRole("link", { name: "Overview" }).click();
  await expect(page).toHaveURL(/\/projects\/prj_abcdefgh$/u);
  await expect(page.getByRole("region", { name: "Environments" })).toBeVisible();
});

test("settings shows identifiers and owner and offers deletion with grace", async ({ page }) => {
  const api = new ProjectHomeHarness();
  await api.install(page);
  page.on("dialog", (dialog) => void dialog.accept());

  await page.goto(`/projects/${PROJECT_ID}/settings`);
  const identifiers = page.getByRole("region", { name: "Identifiers and ownership" });
  await expect(identifiers.getByText(PROJECT_ID, { exact: true })).toBeVisible();
  await expect(identifiers.getByText(TEAM_ID, { exact: true })).toBeVisible();
  await expect(identifiers.getByText("Mako Test Team", { exact: true })).toBeVisible();
  await expect(identifiers.getByText("Team", { exact: true })).toBeVisible();
  await expect(identifiers.getByText("local", { exact: true })).toBeVisible();
  await expect(identifiers.getByText(new Date(NOW).toLocaleString())).toHaveCount(2);

  // Rename and transfer are announced, not offered: no editable control.
  await expect(page.getByText(/coming soon/u)).toBeVisible();
  await expect(page.getByRole("textbox")).toHaveCount(0);

  await page.getByRole("button", { name: "Request deletion" }).click();
  await expect(page.getByText(/Restorable until/u)).toBeVisible();
  expect(api.project.state).toBe("deletion_grace");
  expect(api.deletionConfirmations).toEqual(["delete:Mako Test Project"]);
  await page.getByRole("button", { name: "Restore" }).click();
  await expect.poll(() => api.project.state).toBe("active");
  await expect(page.getByText(/Restorable until/u)).toHaveCount(0);
  expect(api.unhandled).toEqual([]);
});

test("a personal space is named as the developer's own projects", async ({ page }) => {
  const api = new ProjectHomeHarness();
  api.team = { ...teamFixture(), name: "Owner", kind: "personal" };
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}/settings`);
  const sidebar = page.getByRole("complementary", { name: "Project navigation" });
  await expect(sidebar.getByRole("link", { name: "Your projects" })).toHaveAttribute(
    "href",
    `/teams/${TEAM_ID}`,
  );
  const identifiers = page.getByRole("region", { name: "Identifiers and ownership" });
  await expect(identifiers.getByText("Your personal space", { exact: true })).toBeVisible();
  await expect(identifiers.getByText("Personal space", { exact: true })).toBeVisible();
  expect(api.unhandled).toEqual([]);
});

class ProjectHomeHarness {
  readonly unhandled: string[] = [];
  readonly failing = new Set<string>();
  readonly deletionConfirmations: string[] = [];
  readonly project = projectFixture();
  readonly environments = [
    environmentFixture(ENVIRONMENT_ID, "development"),
    environmentFixture(PREVIEW_ENVIRONMENT_ID, "preview"),
  ];
  team = teamFixture();

  async install(page: Page) {
    await page.route("**/v1/**", (route) => void this.handle(route));
  }

  async handle(route: Route) {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    if (request.headers().authorization !== "Bearer developer-session-token") {
      await json(route, apiError("unauthenticated", "A valid developer session is required."), 401);
      return;
    }
    const environment = path.match(/^\/v1\/projects\/prj_abcdefgh\/environments\/(env_[a-z]+)\//u);
    const environmentId = environment?.[1];

    if (path === `/v1/teams/${TEAM_ID}` && method === "GET") {
      await json(route, this.team);
    } else if (path === `/v1/projects/${PROJECT_ID}` && method === "GET") {
      await json(route, this.project);
    } else if (path === `/v1/projects/${PROJECT_ID}` && method === "DELETE") {
      this.deletionConfirmations.push(request.headers().confirmation ?? "");
      this.project.state = "deletion_grace";
      this.project.deletionDeadline = LATER;
      await json(route, this.project, 202);
    } else if (path === `/v1/projects/${PROJECT_ID}/actions/restore` && method === "POST") {
      this.project.state = "active";
      this.project.deletionDeadline = undefined;
      await json(route, this.project, 202);
    } else if (path === `/v1/projects/${PROJECT_ID}/environments` && method === "GET") {
      await json(route, { items: this.environments });
    } else if (environmentId !== undefined && path.endsWith("/workspace/summary")) {
      if (this.failing.has(`summary:${environmentId}`)) {
        await json(route, apiError("unavailable", "The workspace summary is down."), 503);
      } else {
        await json(route, workspaceSummary(environmentId));
      }
    } else if (environmentId !== undefined && path.endsWith("/connect") && method === "GET") {
      await json(route, connectMetadata(environmentId));
    } else if (environmentId !== undefined && path.includes("/observability/")) {
      const kind = path.split("/").at(-1) ?? "";
      if (this.failing.has(kind)) {
        await json(route, apiError("unavailable", `The ${kind} source is down.`), 503);
      } else {
        await json(route, observabilityPage(kind));
      }
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("internal", "Unhandled project home test route."), 500);
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

function projectFixture() {
  return {
    id: PROJECT_ID,
    teamId: TEAM_ID,
    name: "Mako Test Project",
    region: "local",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
    deletionDeadline: undefined as string | undefined,
  };
}

function environmentFixture(id: string, name: string) {
  return {
    id,
    projectId: PROJECT_ID,
    name,
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function workspaceSummary(environmentId: string) {
  const primary = environmentId === ENVIRONMENT_ID;
  return {
    tenant: { projectId: PROJECT_ID, environmentId },
    sections: {
      lifecycle: {
        status: primary ? "current" : "stale",
        observedAtUnixSeconds: 1_786_579_200,
        freshUntilUnixSeconds: 1_786_579_260,
        retainedSinceUnixSeconds: null,
        payload: {
          project: "active",
          environment: primary ? "active" : "provisioning",
          region: "local",
          ready: primary,
        },
        remediationCode: null,
      },
      collections: {
        status: "current",
        observedAtUnixSeconds: 1_786_579_200,
        freshUntilUnixSeconds: 1_786_579_260,
        payload: { count: primary ? 2 : 0, active: primary ? 2 : 0 },
      },
      functions: {
        status: "current",
        observedAtUnixSeconds: 1_786_579_200,
        freshUntilUnixSeconds: 1_786_579_260,
        payload: { count: primary ? 1 : 0 },
      },
    },
  };
}

function connectMetadata(environmentId: string) {
  const primary = environmentId === ENVIRONMENT_ID;
  return {
    tenant: { projectId: PROJECT_ID, environmentId },
    publicEndpoint: "https://cloud-test.makodb.com",
    publicKeyId: primary ? "key_public01" : "key_public02",
    publicKey: primary ? PUBLIC_KEY : "",
    collections: primary ? [{ collectionId: "todos", activeSchemaVersion: 3 }] : [],
    rxdbClientRange: ">=17 <18",
    templateVersion: 1,
  };
}

function observabilityPage(kind: string) {
  const records: Record<string, { timestamp: string; payload: Record<string, unknown> }[]> = {
    usage: [
      {
        timestamp: EARLIER,
        payload: { kind: "usage", resource: "storage_bytes", quantity: 1024, unit: "bytes" },
      },
      {
        timestamp: NOW,
        payload: { kind: "usage", resource: "storage_bytes", quantity: 2048, unit: "bytes" },
      },
      {
        timestamp: NOW,
        payload: {
          kind: "usage",
          resource: "edge_invocations_per_month",
          quantity: 12,
          unit: "invocations",
        },
      },
    ],
    health: [
      {
        timestamp: NOW,
        payload: {
          kind: "health",
          service: "storage",
          region: "local",
          status: "healthy",
          diagnostic: null,
        },
      },
    ],
    "audit-events": [
      {
        timestamp: EARLIER,
        payload: {
          kind: "audit",
          teamId: TEAM_ID,
          actorId: "dev_member01",
          action: "policy.activate",
          target: "todos/policy/1",
          outcome: "allowed",
          requestId: "req_audit01",
          details: "authorization epoch 2",
        },
      },
      {
        timestamp: NOW,
        payload: {
          kind: "audit",
          teamId: TEAM_ID,
          actorId: "dev_abcdefgh",
          action: "environment.create",
          target: PREVIEW_ENVIRONMENT_ID,
          outcome: "allowed",
          requestId: "req_audit02",
          details: null,
        },
      },
    ],
  };
  return {
    items: records[kind] ?? [],
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
