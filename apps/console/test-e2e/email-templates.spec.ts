import { expect, test, type Page, type Route } from "@playwright/test";

const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const TEMPLATES_PATH = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/email-templates`;
const SCREEN_URL = `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/email-templates`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;

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

// The Email templates screen edits the four application emails one kind at
// a time. The API is mocked at the wire: what the screen shows for stored
// and default templates, and what it sends to preview, save, and reset.
test("templates list their state, and a customized one is edited, previewed, saved, and reset", async ({
  page,
}) => {
  const api = new EmailTemplateHarness();
  await api.install(page);

  await page.goto(SCREEN_URL);
  await expect(page.getByRole("heading", { name: "Email templates", exact: true })).toBeVisible();
  const destinations = page.getByRole("navigation", { name: "Environment destinations" });
  const link = destinations.getByRole("link", { name: "Email templates", exact: true });
  await expect(link).toHaveAttribute("href", SCREEN_URL);
  await expect(link).toHaveAttribute("aria-current", "page");

  const kinds = page.getByRole("navigation", { name: "Template kinds" });
  await expect(kinds.getByRole("button", { name: /Address verification/u })).toContainText(
    "default",
  );
  await expect(kinds.getByRole("button", { name: /Magic link/u })).toContainText("customized v2");
  await expect(page.getByLabel("Subject")).toHaveValue("Verify your email for {{project_name}}");
  await expect(page.getByTestId("template-state")).toContainText("Using the built-in default.");
  await expect(page.getByRole("button", { name: "Reset to default" })).toBeDisabled();

  // The magic-link template is customized; its variables are listed.
  await kinds.getByRole("button", { name: /Magic link/u }).click();
  await expect(page.getByRole("heading", { name: "Magic link", exact: true })).toBeVisible();
  await expect(page.getByLabel("Subject")).toHaveValue("Sign in to Acme");
  await expect(page.getByLabel("Body")).toHaveValue(
    "Hi {{email}}, open {{link}} before {{expires_at}}.",
  );
  await expect(page.getByTestId("template-state")).toContainText("Customized (version 2");
  await expect(page.getByText("{{project_name}}", { exact: true })).toBeVisible();

  // Edit, preview the unsaved text, then save.
  await page.getByLabel("Subject").fill("Your Acme sign-in link");
  await page.getByLabel("Body").fill("Open {{link}} to sign in as {{email}}.");
  await page.getByRole("button", { name: "Preview" }).click();
  await expect(page.getByTestId("preview-subject")).toHaveText("Your Acme sign-in link");
  await expect(page.getByTestId("preview-body")).toHaveText(
    "Open https://app.example.com/auth/callback?token=preview-token to sign in as person@example.com.",
  );
  const previews = api.requests.filter((request) => request.method === "POST");
  expect(previews).toHaveLength(1);
  expect(previews[0]?.path).toBe(`${TEMPLATES_PATH}/magic_link/actions/preview`);
  expect(previews[0]?.body).toEqual({
    subject: "Your Acme sign-in link",
    textBody: "Open {{link}} to sign in as {{email}}.",
  });

  await page.getByRole("button", { name: "Save template" }).click();
  await expect(page.getByRole("status")).toContainText("Magic link template saved (version 3).");
  const puts = api.requests.filter((request) => request.method === "PUT");
  expect(puts).toHaveLength(1);
  expect(puts[0]?.path).toBe(`${TEMPLATES_PATH}/magic_link`);
  expect(puts[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(puts[0]?.body).toEqual({
    subject: "Your Acme sign-in link",
    textBody: "Open {{link}} to sign in as {{email}}.",
  });
  await expect(kinds.getByRole("button", { name: /Magic link/u })).toContainText("customized v3");

  // Reset asks first, then restores the default.
  page.once("dialog", (dialog) => void dialog.accept());
  await page.getByRole("button", { name: "Reset to default" }).click();
  await expect(page.getByRole("status")).toContainText("Magic link template reset to the default.");
  const deletes = api.requests.filter((request) => request.method === "DELETE");
  expect(deletes).toHaveLength(1);
  expect(deletes[0]?.path).toBe(`${TEMPLATES_PATH}/magic_link`);
  await expect(page.getByLabel("Subject")).toHaveValue("Your sign-in link for {{project_name}}");
  await expect(page.getByTestId("template-state")).toContainText("Using the built-in default.");
  expect(api.unhandled).toEqual([]);
});

test("a template the API refuses is reported and nothing is stored", async ({ page }) => {
  const api = new EmailTemplateHarness();
  api.refusal =
    "textBody: unknown variable {{token}}; magic_link templates may use link, expires_at, email, project_name, environment_name";
  await api.install(page);
  await page.goto(SCREEN_URL);
  await page
    .getByRole("navigation", { name: "Template kinds" })
    .getByRole("button", { name: /Magic link/u })
    .click();
  await page.getByLabel("Body").fill("Use {{token}}.");
  await page.getByRole("button", { name: "Save template" }).click();
  await expect(page.getByRole("alert")).toContainText("unknown variable {{token}}");
  await expect(
    page
      .getByRole("navigation", { name: "Template kinds" })
      .getByRole("button", { name: /Magic link/u }),
  ).toContainText("customized v2");
  expect(api.unhandled).toEqual([]);
});

interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

interface TemplateFixture {
  kind: string;
  subject: string;
  textBody: string;
  isDefault: boolean;
  version: number;
  updatedAt: string | null;
}

const DEFAULTS: Record<string, { subject: string; textBody: string }> = {
  verification: {
    subject: "Verify your email for {{project_name}}",
    textBody:
      "Hello,\n\nConfirm {{email}} for {{project_name}} ({{environment_name}}):\n\n{{link}}\n",
  },
  recovery: {
    subject: "Reset your {{project_name}} password",
    textBody: "Hello,\n\nReset the password for {{email}}:\n\n{{link}}\n",
  },
  invitation: {
    subject: "You are invited to {{project_name}}",
    textBody: "Hello,\n\n{{inviter}} invited you:\n\n{{link}}\n",
  },
  magic_link: {
    subject: "Your sign-in link for {{project_name}}",
    textBody: "Hello,\n\nSign in as {{email}}:\n\n{{link}}\n",
  },
};

const PLACEHOLDERS: Record<string, string> = {
  link: "https://app.example.com/auth/callback?token=preview-token",
  expires_at: "2030-01-01T12:00:00Z",
  email: "person@example.com",
  project_name: "Sign-in demo",
  environment_name: "production",
  inviter: "A teammate",
};

class EmailTemplateHarness {
  readonly unhandled: string[] = [];
  readonly requests: RecordedRequest[] = [];
  refusal: string | null = null;
  templates: TemplateFixture[] = ["verification", "recovery", "invitation", "magic_link"].map(
    (kind) =>
      kind === "magic_link"
        ? {
            kind,
            subject: "Sign in to Acme",
            textBody: "Hi {{email}}, open {{link}} before {{expires_at}}.",
            isDefault: false,
            version: 2,
            updatedAt: "2026-08-07T12:00:00.000Z",
          }
        : {
            kind,
            ...(DEFAULTS[kind] ?? { subject: "", textBody: "" }),
            isDefault: true,
            version: 0,
            updatedAt: null,
          },
  );

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
    if (path === TEMPLATES_PATH && method === "GET") {
      await json(route, { items: this.templates });
      return;
    }
    const kindMatch = path.match(/\/email-templates\/([a-z_]+)(\/actions\/preview)?$/u);
    const kind = kindMatch?.[1];
    const template = this.templates.find((candidate) => candidate.kind === kind);
    if (kind !== undefined && template !== undefined) {
      const input = JSON.parse(body ?? "{}") as { subject?: string; textBody?: string };
      if (kindMatch?.[2] !== undefined && method === "POST") {
        await json(route, {
          subject: render(input.subject ?? template.subject),
          textBody: render(input.textBody ?? template.textBody),
        });
        return;
      }
      if (method === "PUT") {
        if (this.refusal !== null) {
          await json(route, apiError("invalid_request", this.refusal), 400);
          return;
        }
        template.subject = input.subject ?? template.subject;
        template.textBody = input.textBody ?? template.textBody;
        template.isDefault = false;
        template.version += 1;
        template.updatedAt = "2026-08-08T12:00:00.000Z";
        await json(route, template);
        return;
      }
      if (method === "DELETE") {
        Object.assign(template, DEFAULTS[kind], { isDefault: true, version: 0, updatedAt: null });
        await json(route, template);
        return;
      }
    }
    this.unhandled.push(`${method} ${path}`);
    await json(route, apiError("not_found", `Unhandled ${method} ${path}`), 404);
  }
}

function render(text: string): string {
  return text.replace(/\{\{\s*([a-z_]+)\s*\}\}/gu, (_, name: string) => PLACEHOLDERS[name] ?? "");
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
