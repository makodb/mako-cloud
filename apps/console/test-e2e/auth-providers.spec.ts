import { expect, test, type Page, type Route } from "@playwright/test";

const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const SETTINGS_PATH = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/auth-settings`;
const SCREEN_URL = `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/auth-providers`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const SECRET = "google-client-secret-never-shown";

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

// The Auth providers screen edits one environment's sign-in settings as a
// unit. The API is mocked at the wire: what the screen shows for the
// installed settings, and exactly what it sends on save — above all that a
// client secret goes out once and is never displayed.
test("installed providers are listed without their secrets and Auth providers is a destination", async ({
  page,
}) => {
  const api = new AuthSettingsHarness();
  await api.install(page);

  await page.goto(SCREEN_URL);
  await expect(page.getByRole("heading", { name: "Auth providers", exact: true })).toBeVisible();
  const destinations = page.getByRole("navigation", { name: "Environment destinations" });
  const link = destinations.getByRole("link", { name: "Auth providers", exact: true });
  await expect(link).toHaveAttribute("href", SCREEN_URL);
  await expect(link).toHaveAttribute("aria-current", "page");

  const table = page.getByRole("table", { name: "Sign-in providers" });
  const google = table.getByRole("row", { name: /google/u });
  await expect(google).toContainText("OpenID Connect · https://accounts.google.com");
  await expect(google).toContainText("google-client-id");
  await expect(google).toContainText("stored");
  await expect(google.getByRole("checkbox", { name: "google enabled" })).toBeChecked();
  const github = table.getByRole("row", { name: /github/u });
  await expect(github).toContainText("GitHub");
  await expect(github.getByRole("checkbox", { name: "github enabled" })).not.toBeChecked();
  await expect(page.getByLabel("Redirect URLs")).toHaveValue(
    "https://app.example.test/auth/callback\nhttp://localhost:3000/auth/callback",
  );
  await expect(
    page.getByRole("checkbox", { name: "Let users sign in by emailed link" }),
  ).toBeChecked();
  await expect(page.getByLabel("Link lifetime (seconds)")).toHaveValue("900");
  await expect(page.getByText("Installed version 3")).toBeVisible();

  // Nothing on the page is the secret, and nothing was sent.
  expect(await page.content()).not.toContain(SECRET);
  expect(api.requests.filter((request) => request.method === "PUT")).toHaveLength(0);
  expect(api.unhandled).toEqual([]);
});

test("saving sends new secrets once, keeps stored ones by omission, and never shows them", async ({
  page,
}) => {
  const api = new AuthSettingsHarness();
  await api.install(page);
  await page.goto(SCREEN_URL);
  await expect(page.getByRole("table", { name: "Sign-in providers" })).toBeVisible();

  // Add an Okta provider with a secret, enable GitHub, add a redirect, and
  // shorten the link lifetime.
  const form = page.getByRole("form", { name: "Add a provider" });
  await form.getByLabel("Name").fill("okta");
  await form.getByLabel("Kind").selectOption("oidc");
  await form.getByLabel("Issuer (OpenID Connect only)").fill("https://acme.okta.com");
  await form.getByLabel("Client ID").fill("okta-client-id");
  await form.getByLabel("Client secret").fill(SECRET);
  await form.getByLabel("Scopes (space separated)").fill("openid email");
  await form.getByRole("button", { name: "Add provider" }).click();
  const table = page.getByRole("table", { name: "Sign-in providers" });
  await expect(table.getByRole("row", { name: /okta/u })).toContainText("will be replaced on save");
  await table.getByRole("checkbox", { name: "github enabled" }).check();
  await page
    .getByLabel("Redirect URLs")
    .fill(
      "https://app.example.test/auth/callback\nhttp://localhost:3000/auth/callback\nhttps://staging.example.test/auth/callback",
    );
  await page.getByLabel("Link lifetime (seconds)").fill("600");
  // Typed, the secret lives only in its password field: never in visible text.
  expect(await page.locator("main").innerText()).not.toContain(SECRET);

  await page.getByRole("button", { name: "Save sign-in settings" }).click();
  await expect(page.getByRole("status")).toContainText("Sign-in settings saved (version 4).");

  const puts = api.requests.filter((request) => request.method === "PUT");
  expect(puts).toHaveLength(1);
  const put = puts[0];
  expect(put?.path).toBe(SETTINGS_PATH);
  expect(put?.headers["idempotency-key"]).toMatch(UUID);
  expect(put?.body).toEqual({
    providers: [
      {
        name: "google",
        kind: { type: "oidc", issuer: "https://accounts.google.com" },
        clientId: "google-client-id",
        scopes: ["openid", "email", "profile"],
        enabled: true,
      },
      {
        name: "github",
        kind: { type: "git_hub" },
        clientId: "github-client-id",
        scopes: ["read:user", "user:email"],
        enabled: true,
      },
      {
        name: "okta",
        kind: { type: "oidc", issuer: "https://acme.okta.com" },
        clientId: "okta-client-id",
        scopes: ["openid", "email"],
        enabled: true,
        clientSecret: SECRET,
      },
    ],
    redirectUrls: [
      "https://app.example.test/auth/callback",
      "http://localhost:3000/auth/callback",
      "https://staging.example.test/auth/callback",
    ],
    magicLinks: { enabled: true, linkTtlSeconds: 600 },
  });
  // After the save the secret is gone from the page and the row reads as stored.
  await expect(table.getByRole("row", { name: /okta/u })).toContainText("stored");
  expect(await page.content()).not.toContain(SECRET);
  await expect(page.getByText("Installed version 4")).toBeVisible();
  expect(api.unhandled).toEqual([]);
});

test("a new provider without a secret is refused before anything is sent, and API refusals are shown", async ({
  page,
}) => {
  const api = new AuthSettingsHarness();
  await api.install(page);
  await page.goto(SCREEN_URL);
  await expect(page.getByRole("table", { name: "Sign-in providers" })).toBeVisible();

  const form = page.getByRole("form", { name: "Add a provider" });
  await form.getByLabel("Name").fill("azure");
  await form.getByLabel("Issuer (OpenID Connect only)").fill("https://login.example.test");
  await form.getByLabel("Client ID").fill("azure-client-id");
  await form.getByRole("button", { name: "Add provider" }).click();
  await page.getByRole("button", { name: "Save sign-in settings" }).click();
  await expect(page.getByRole("alert")).toContainText("Enter the client secret for azure");
  expect(api.requests.filter((request) => request.method === "PUT")).toHaveLength(0);

  // Removing it and saving a redirect the API refuses surfaces the refusal.
  await page.getByRole("button", { name: "Remove azure" }).click();
  api.updateRefusal = "redirect url must be https, or http to loopback";
  await page.getByLabel("Redirect URLs").fill("http://app.example.test/insecure");
  await page.getByRole("button", { name: "Save sign-in settings" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "redirect url must be https, or http to loopback",
  );
  expect(api.requests.filter((request) => request.method === "PUT")).toHaveLength(1);
  expect(api.unhandled).toEqual([]);
});

interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

interface ProviderFixture {
  name: string;
  kind: { type: "oidc"; issuer: string } | { type: "git_hub" };
  clientId: string;
  scopes: string[];
  enabled: boolean;
  hasSecret: boolean;
}

class AuthSettingsHarness {
  readonly unhandled: string[] = [];
  readonly requests: RecordedRequest[] = [];
  updateRefusal: string | null = null;
  settings = {
    providers: [
      {
        name: "google",
        kind: { type: "oidc", issuer: "https://accounts.google.com" },
        clientId: "google-client-id",
        scopes: ["openid", "email", "profile"],
        enabled: true,
        hasSecret: true,
      },
      {
        name: "github",
        kind: { type: "git_hub" },
        clientId: "github-client-id",
        scopes: ["read:user", "user:email"],
        enabled: false,
        hasSecret: true,
      },
    ] as ProviderFixture[],
    redirectUrls: ["https://app.example.test/auth/callback", "http://localhost:3000/auth/callback"],
    magicLinks: { enabled: true, linkTtlSeconds: 900 },
    version: 3,
  };

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
    const body = request.postData();
    this.requests.push({
      method,
      path,
      headers: request.headers(),
      body: body === null ? null : (JSON.parse(body) as unknown),
    });

    if (path === `/v1/projects/${PROJECT_ID}` && method === "GET") {
      await json(route, {
        id: PROJECT_ID,
        organizationId: "org_abcdefgh",
        name: "Sign-in demo",
        region: "local",
        state: "active",
        createdAt: "2026-08-06T12:00:00.000Z",
        updatedAt: "2026-08-06T12:00:00.000Z",
      });
      return;
    }
    if (path === `/v1/projects/${PROJECT_ID}/environments` && method === "GET") {
      await json(route, {
        items: [
          {
            id: ENVIRONMENT_ID,
            projectId: PROJECT_ID,
            name: "production",
            state: "active",
            createdAt: "2026-08-06T12:00:00.000Z",
            updatedAt: "2026-08-06T12:00:00.000Z",
          },
        ],
      });
      return;
    }
    if (path.endsWith("/workspace/navigation") && method === "GET") {
      await json(
        route,
        ["overview", "collections", "users"].map((id) => ({
          id,
          label: id,
          path: `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/${id}`,
          permitted: true,
        })),
      );
      return;
    }
    if (path === SETTINGS_PATH && method === "GET") {
      await json(route, this.settings);
      return;
    }
    if (path === SETTINGS_PATH && method === "PUT") {
      if (this.updateRefusal !== null) {
        await json(route, apiError("invalid_request", this.updateRefusal), 400);
        return;
      }
      const update = JSON.parse(body ?? "{}") as {
        providers: (Omit<ProviderFixture, "hasSecret"> & { clientSecret?: string })[];
        redirectUrls: string[];
        magicLinks: { enabled: boolean; linkTtlSeconds: number };
      };
      this.settings = {
        providers: update.providers.map(({ clientSecret, ...provider }) => ({
          ...provider,
          hasSecret:
            clientSecret !== undefined ||
            this.settings.providers.some(
              (installed) => installed.name === provider.name && installed.hasSecret,
            ),
        })),
        redirectUrls: update.redirectUrls,
        magicLinks: update.magicLinks,
        version: this.settings.version + 1,
      };
      await json(route, this.settings);
      return;
    }
    this.unhandled.push(`${method} ${path}`);
    await json(route, apiError("not_found", `Unhandled ${method} ${path}`), 404);
  }
}

async function json(route: Route, payload: unknown, status = 200) {
  await route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(payload),
  });
}

function apiError(code: string, message: string) {
  return {
    apiVersion: "v1",
    error: { code, message, requestId: "req_test", retry: { kind: "never" } },
  };
}
