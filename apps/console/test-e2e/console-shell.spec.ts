import { expect, test, type Page, type Route } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";

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

// The shell is what frames every environment screen: the context it shows,
// the destinations it offers, and the areas it admits it does not have yet.
// Screen bodies are covered by their own specs; this one asserts the frame.
test("a deep link opens inside the shell with its context and every destination", async ({
  page,
}) => {
  await installApi(page);

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/collections`);
  const sidebar = page.getByRole("complementary", { name: "Environment navigation" });
  await expect(sidebar).toBeVisible();
  // Context: the project is named and the environment switcher shows it.
  await expect(sidebar.getByRole("button", { name: "Mako Test Project" })).toBeVisible();
  await expect(sidebar.getByLabel("Switch environment")).toHaveValue(ENVIRONMENT_ID);

  const destinations = page.getByRole("navigation", { name: "Environment destinations" });
  // Backend-authorized destinations and the console-served ones are all there.
  for (const label of [
    "overview",
    "collections",
    "functions",
    "Storage",
    "Webhooks",
    "Logs",
    "Usage",
    "Activity",
  ]) {
    await expect(destinations.getByRole("link", { name: label, exact: true })).toBeVisible();
  }
  await expect(
    destinations.getByRole("link", { name: "collections", exact: true }),
  ).toHaveAttribute("aria-current", "page");

  // Areas the deployment lacks are shown, disabled, with a reason -- never hidden.
  for (const label of ["Domains"]) {
    const item = destinations.locator(".destination-unavailable", { hasText: label });
    await expect(item).toBeVisible();
    await expect(item).toHaveAttribute("aria-disabled", "true");
    await expect(item).toContainText("Not available on this deployment");
  }
  await expect(destinations.locator(".destination-unavailable")).toHaveCount(1);
  // Schedules are neither a destination nor unavailable: they live on each
  // function's page, so the sidebar says nothing about them.
  await expect(destinations.getByText("Schedules")).toHaveCount(0);
  await expect(
    destinations.getByRole("link", { name: "Auth providers", exact: true }),
  ).toHaveAttribute(
    "href",
    `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/auth-providers`,
  );
  await expect(
    destinations.getByRole("link", { name: "Email templates", exact: true }),
  ).toHaveAttribute(
    "href",
    `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/email-templates`,
  );
  // Webhooks moved from the unavailable list to a real destination.
  await expect(destinations.getByRole("link", { name: "Webhooks", exact: true })).toHaveAttribute(
    "href",
    `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/webhooks`,
  );
  await expect(
    destinations.locator(".destination-unavailable", { hasText: "Webhooks" }),
  ).toHaveCount(0);

  // Moving to a console-served destination keeps the shell and marks it current.
  await destinations.getByRole("link", { name: "Logs", exact: true }).click();
  await expect(page).toHaveURL(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/logs`);
  await expect(sidebar).toBeVisible();
  await expect(destinations.getByRole("link", { name: "Logs", exact: true })).toHaveAttribute(
    "aria-current",
    "page",
  );

  // The top bar always offers the way home.
  await expect(page.getByRole("link", { name: "Developer Console home" })).toBeVisible();
});

test("an unknown destination fails closed to not found", async ({ page }) => {
  await installApi(page);
  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/schedules`);
  await expect(page.getByRole("heading", { name: "Page not found" })).toBeVisible();
});

async function installApi(page: Page) {
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === `/v1/projects/${PROJECT_ID}`) {
      return json(route, project());
    }
    if (path === `/v1/projects/${PROJECT_ID}/environments`) {
      return json(route, { items: [environment()] });
    }
    if (path.endsWith("/workspace/navigation")) {
      return json(route, navigation());
    }
    if (path.endsWith("/collections") && request.method() === "GET") {
      return json(route, { items: [] });
    }
    if (path.includes("/observability/")) {
      return json(route, {
        items: [],
        nextCursor: null,
        retention: {
          observedAt: NOW,
          retainedFrom: "2026-05-08T12:00:00.000Z",
          retentionSeconds: 7_776_000,
        },
      });
    }
    return json(route, { items: [] });
  });
}

function project() {
  return {
    id: PROJECT_ID,
    teamId: TEAM_ID,
    name: "Mako Test Project",
    region: "us-east-1",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function environment() {
  return {
    id: ENVIRONMENT_ID,
    projectId: PROJECT_ID,
    name: "development",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function navigation() {
  return [
    "overview",
    "data",
    "collections",
    "sync",
    "users",
    "policies",
    "functions",
    "observability",
    "backups",
    "connect",
    "settings",
  ].map((id) => ({
    id,
    label: id,
    path: `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/${id}`,
    permitted: true,
  }));
}

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}
