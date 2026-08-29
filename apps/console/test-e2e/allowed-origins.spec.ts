import { expect, test, type Route } from "@playwright/test";

const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const SETTINGS_URL = `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/settings`;
const ORIGINS_PATH = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/allowed-origins`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const ORIGINS = ["https://app.example.com", "http://127.0.0.1:5173"];

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
      developerWorkspaceEnabled: true,
    };
  });
});

// The environment's browser-origin allowlist on its Settings screen: what is
// allowed now, the whole list edited one per line, local refusal of anything
// a browser would not send as an origin, and the save that replaces the list
// with an idempotency key. The API is mocked at the wire; every assertion is
// about what the page shows for a response or sends for an action.
test("the environment's Settings screen lists the allowed origins and says what they reach", async ({
  page,
}) => {
  const api = new OriginsApiHarness();
  await api.install(page);

  await page.goto(SETTINGS_URL);
  const section = page.getByRole("region", { name: "Allowed origins" });
  await expect(section.getByRole("heading", { name: "Allowed origins" })).toBeVisible();
  await expect(section).toContainText(
    "A browser application must list its own origin here before it can call this environment's application API",
  );
  await expect(section).toContainText(
    "The management and operator APIs never answer cross-origin calls, whatever is listed here.",
  );
  await expect(section.locator(".allowed-origins-list li")).toHaveText(ORIGINS);
  await expect(section.getByLabel("Origins, one per line")).toHaveValue(
    "https://app.example.com\nhttp://127.0.0.1:5173",
  );
  const reads = api.requests.filter((request) => request.path === ORIGINS_PATH);
  expect(reads.length).toBeGreaterThan(0);
  expect(reads.every((request) => request.method === "GET" && request.body === null)).toBe(true);
  expect(reads.every((request) => request.idempotencyKey === undefined)).toBe(true);
  expect(api.unhandled).toEqual([]);
});

test("saving the origins refuses a bad one locally, sends the exact list with an idempotency key, and clears it", async ({
  page,
}) => {
  const api = new OriginsApiHarness();
  await api.install(page);

  await page.goto(SETTINGS_URL);
  const editor = page.getByRole("form", { name: "Allowed origins" });
  const field = editor.getByLabel("Origins, one per line");
  await expect(field).toHaveValue("https://app.example.com\nhttp://127.0.0.1:5173");

  // Not origins: a trailing slash, plain http off loopback, the default
  // port. Each is refused with its reason and nothing is sent.
  await field.fill("https://app.example.com/");
  await editor.getByRole("button", { name: "Save origins" }).click();
  await expect(page.getByRole("alert")).toContainText(
    '"https://app.example.com/" is not an exact browser origin',
  );
  await field.fill("http://app.example.com");
  await editor.getByRole("button", { name: "Save origins" }).click();
  await expect(page.getByRole("alert")).toContainText(
    '"http://app.example.com" uses plain http, which is allowed only to loopback',
  );
  await field.fill("https://app.example.com:443");
  await editor.getByRole("button", { name: "Save origins" }).click();
  await expect(page.getByRole("alert")).toContainText(
    '"https://app.example.com:443" names the default port; a browser sends https://app.example.com.',
  );
  expect(api.requests.filter((request) => request.method === "PUT")).toHaveLength(0);

  // The list is sent whole, as the browser will present it, and shown back.
  await field.fill("https://App.Example.com\nhttp://127.0.0.1:5173\n\nhttps://app.example.com");
  await editor.getByRole("button", { name: "Save origins" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "This environment now allows cross-origin calls from 2 origins.",
  );
  const puts = api.requests.filter((request) => request.method === "PUT");
  expect(puts.map((request) => request.path)).toEqual([ORIGINS_PATH]);
  expect(puts[0]?.body).toEqual({ allowedOrigins: ORIGINS });
  expect(puts[0]?.idempotencyKey).toMatch(UUID);
  await expect(page.locator(".allowed-origins-list li")).toHaveText(ORIGINS);
  await expect(page.getByRole("alert")).toHaveCount(0);

  // An empty list means no cross-origin access.
  await field.fill("");
  await editor.getByRole("button", { name: "Save origins" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "This environment now allows no cross-origin access.",
  );
  expect(api.requests.filter((request) => request.method === "PUT")[1]?.body).toEqual({
    allowedOrigins: [],
  });
  await expect(page.locator(".allowed-origins-none")).toContainText("None");
  expect(api.allowedOrigins).toEqual([]);
  expect(api.unhandled).toEqual([]);
});

test("an origin the API refuses is shown with its message and the list is unchanged", async ({
  page,
}) => {
  const api = new OriginsApiHarness();
  api.updateRefusal = "origin http://192.168.1.10:5173 is not allowed: plain http only to loopback";
  await api.install(page);

  await page.goto(SETTINGS_URL);
  const editor = page.getByRole("form", { name: "Allowed origins" });
  await editor.getByLabel("Origins, one per line").fill("http://localhost:5173");
  await editor.getByRole("button", { name: "Save origins" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "origin http://192.168.1.10:5173 is not allowed: plain http only to loopback",
  );
  expect(api.requests.filter((request) => request.method === "PUT")).toHaveLength(1);
  await expect(page.locator(".allowed-origins-list li")).toHaveText(ORIGINS);
  expect(api.allowedOrigins).toEqual(ORIGINS);
  expect(api.unhandled).toEqual([]);
});

interface RecordedRequest {
  method: string;
  path: string;
  body: unknown;
  idempotencyKey: string | undefined;
}

class OriginsApiHarness {
  readonly requests: RecordedRequest[] = [];
  readonly unhandled: string[] = [];
  allowedOrigins: string[] = [...ORIGINS];
  updateRefusal: string | null = null;

  async install(page: import("@playwright/test").Page): Promise<void> {
    await page.route("**/v1/**", async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      const method = request.method();
      if (request.headers().authorization !== "Bearer developer-session-token") {
        await json(
          route,
          apiError("unauthenticated", "A valid developer session is required."),
          401,
        );
        return;
      }
      if (path === ORIGINS_PATH) {
        this.requests.push({
          method,
          path,
          body: method === "PUT" ? request.postDataJSON() : null,
          idempotencyKey: request.headers()["idempotency-key"],
        });
        if (method === "PUT") {
          if (this.updateRefusal !== null) {
            await json(route, apiError("invalid_request", this.updateRefusal), 400);
            return;
          }
          this.allowedOrigins = (
            request.postDataJSON() as { allowedOrigins: string[] }
          ).allowedOrigins;
        }
        await json(route, { allowedOrigins: this.allowedOrigins });
        return;
      }
      if (path === `/v1/projects/${PROJECT_ID}`) {
        await json(route, {
          id: PROJECT_ID,
          teamId: "org_abcdefgh",
          name: "Workspace project",
          region: "local",
          state: "active",
          createdAt: "2026-08-13T00:00:00Z",
          updatedAt: "2026-08-13T00:00:00Z",
        });
        return;
      }
      if (path === `/v1/projects/${PROJECT_ID}/environments`) {
        await json(route, { items: [environment()] });
        return;
      }
      if (path === `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`) {
        await json(route, environment());
        return;
      }
      if (path.endsWith("/workspace/navigation")) {
        await json(route, [
          {
            id: "settings",
            label: "settings",
            path: SETTINGS_URL,
            permitted: true,
          },
        ]);
        return;
      }
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${path}`), 404);
    });
  }
}

function environment() {
  return {
    id: ENVIRONMENT_ID,
    projectId: PROJECT_ID,
    name: "development",
    state: "active",
    createdAt: "2026-08-13T00:00:00Z",
    updatedAt: "2026-08-13T00:00:00Z",
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
