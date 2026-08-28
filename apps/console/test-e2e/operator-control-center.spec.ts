import { expect, test, type Route } from "@playwright/test";

const PROJECT_ID = "prj_abcdefgh";

test("operator shell is permission-aware, responsive, partial-failure safe, and URL-clean", async ({
  page,
}) => {
  let signedIn = false;
  const session = {
    operatorId: "opr_abcdefgh",
    developerIdentityId: "dev_abcdefgh",
    email: "operator@example.test",
    displayName: "Operator",
    developerStatus: "active",
    permissions: [
      "overview_read",
      "tenant_read",
      "operations_read",
      "activity_read",
      "support_access",
    ],
    passwordVerifiedAt: new Date().toISOString(),
    expiresAt: new Date(Date.now() + 30 * 60_000).toISOString(),
  };
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    if (path === "/v1/operator-auth/sessions/current" && request.method() === "GET") {
      return signedIn ? json(route, session) : apiError(route, 401, "unauthenticated");
    }
    if (path === "/v1/operator-auth/sessions" && request.method() === "POST") {
      signedIn = true;
      return json(route, session);
    }
    if (path === "/v1/operator-auth/sessions/current" && request.method() === "DELETE") {
      signedIn = false;
      return route.fulfill({ status: 204, body: "" });
    }
    if (!signedIn) return apiError(route, 401, "unauthenticated");
    if (path === "/v1/operator/support-sessions/current") {
      return json(route, {
        items: [
          {
            id: "sup_abcdefgh",
            operatorId: "opr_abcdefgh",
            projectId: PROJECT_ID,
            environmentId: "env_abcdefgh",
            permissions: ["logs_read"],
            reason: "Investigate scoped beta issue",
            state: "active",
            expiresAt: new Date(Date.now() + 600_000).toISOString(),
            createdAt: new Date().toISOString(),
            revokedAt: null,
            version: 1,
            history: [],
          },
        ],
      });
    }
    if (path === "/v1/operator/overview") {
      return json(route, {
        sections: [
          {
            id: "telemetry",
            freshness: "unavailable",
            observedAt: null,
            provider: "telemetry",
            message: "Source unavailable; no healthy state is implied.",
            metrics: {},
            links: [],
          },
          {
            id: "release",
            freshness: "current",
            observedAt: new Date().toISOString(),
            provider: "release",
            message: null,
            metrics: { ready: true },
            links: [],
          },
        ],
        activeTenants: 1,
        attentionTenants: 1,
        observedAt: new Date().toISOString(),
        partial: true,
      });
    }
    if (path === "/v1/operator/tenants") {
      return json(route, {
        items: [
          {
            projectId: PROJECT_ID,
            projectName: "Mako Test",
            teamId: "org_abcdefgh",
            teamName: "Mako",
            lifecycle: "active",
            region: "us-east-1",
            environmentCount: 1,
            health: "current",
            plan: "preview",
            updatedAt: new Date().toISOString(),
          },
        ],
        nextCursor: null,
        observedAt: new Date().toISOString(),
        examinedRecords: 1,
      });
    }
    if (path === `/v1/operator/tenants/${PROJECT_ID}`) {
      return json(route, {
        project: {
          projectId: PROJECT_ID,
          projectName: "Mako Test",
          teamId: "org_abcdefgh",
          teamName: "Mako",
          lifecycle: "active",
          region: "us-east-1",
          environmentCount: 1,
          health: "current",
          plan: "preview",
          updatedAt: new Date().toISOString(),
        },
        environments: [
          {
            id: "env_abcdefgh",
            projectId: PROJECT_ID,
            name: "Production",
            state: "active",
            createdAt: new Date().toISOString(),
            updatedAt: new Date().toISOString(),
          },
        ],
        sections: {
          sync: {
            id: "sync",
            freshness: "unavailable",
            observedAt: null,
            provider: "telemetry",
            message: "Unavailable",
            metrics: {},
            links: [],
          },
          backups: {
            id: "backups",
            freshness: "current",
            observedAt: new Date().toISOString(),
            provider: "backup",
            message: null,
            metrics: { integrityVerified: true },
            links: [],
          },
        },
        partial: true,
      });
    }
    if (path === "/v1/operator/security") return apiError(route, 403, "permission_denied");
    return apiError(route, 404, "not_found");
  });

  await page.goto("/operator");
  await page.getByLabel("Email").fill("operator@example.test");
  await page.getByLabel("Password").fill("operator password");
  await page.getByRole("button", { name: "Open operator console" }).click();
  await expect(page.getByRole("heading", { name: "Platform overview" })).toBeVisible();
  await expect(page.getByText("Support mode: active (1)")).toBeVisible();
  await expect(page.getByRole("button", { name: "Developer wait list" })).toHaveCount(0);
  await expect(page.getByText(/successful sections remain visible/iu)).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "release" })).toBeVisible();

  await page.getByRole("button", { name: "Tenants" }).click();
  await page.getByLabel(/developer email/iu).fill("Owner@Example.Test");
  await page.getByRole("button", { name: "Search" }).click();
  expect(page.url()).not.toContain("Owner");
  expect(page.url()).not.toContain("example.test");
  await page.getByRole("button", { name: "Open Tenant 360" }).click();
  await expect(page.getByText(/successful sections remain visible/iu)).toBeVisible();
  await expect(page.getByRole("heading", { name: "backups" })).toBeVisible();

  await page.setViewportSize({ width: 375, height: 812 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= document.documentElement.clientWidth,
    ),
  ).toBe(true);
  await expect(page.locator("#operator-main-content")).toHaveAttribute("tabindex", "-1");
  await expect(page.getByRole("link", { name: "Skip to operator workspace" })).toHaveAttribute(
    "href",
    "#operator-main-content",
  );
  expect(
    await page.evaluate(() => ({ local: { ...localStorage }, session: { ...sessionStorage } })),
  ).toEqual({ local: {}, session: {} });

  await page.goto("/operator/security");
  await expect(page.getByRole("alert")).toContainText(/forbidden|permission|available/iu);
  await page.goto("/operator/documents");
  await expect(page.locator("#operator-main-content")).toHaveCount(0);
});

test("contextual repair preserves its operation key across password step-up retry", async ({
  page,
}) => {
  let signedIn = false;
  let repairAttempts = 0;
  const repairBodies: string[] = [];
  const now = new Date();
  const session = {
    operatorId: "opr_abcdefgh",
    developerIdentityId: "dev_abcdefgh",
    email: "operator@example.test",
    displayName: "Operator",
    developerStatus: "waitlisted",
    permissions: ["tenant_read", "operations_read", "provisioning_repair", "support_access"],
    passwordVerifiedAt: new Date(now.getTime() - 10 * 60_000).toISOString(),
    expiresAt: new Date(now.getTime() + 30 * 60_000).toISOString(),
  };
  const steppedUp = { ...session, passwordVerifiedAt: new Date().toISOString() };
  const workflow = {
    id: "wf_abcdefgh",
    resource: { kind: "project", projectId: PROJECT_ID },
    operation: "create",
    state: "queued",
    diagnostics: [],
    operatorRepairs: [],
    updatedAt: new Date().toISOString(),
  };
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/v1/operator-auth/sessions/current" && request.method() === "GET") {
      return signedIn ? json(route, session) : apiError(route, 401, "unauthenticated");
    }
    if (path === "/v1/operator-auth/sessions" && request.method() === "POST") {
      signedIn = true;
      return json(route, session);
    }
    if (
      path === "/v1/operator-auth/sessions/current/actions/verify-password" &&
      request.method() === "POST"
    ) {
      return json(route, steppedUp);
    }
    if (!signedIn) return apiError(route, 401, "unauthenticated");
    if (path === "/v1/operator/support-sessions/current") return json(route, { items: [] });
    if (path === "/v1/operator/inventory/operations") {
      return json(route, {
        items: [
          {
            id: "operations",
            freshness: "current",
            observedAt: new Date().toISOString(),
            provider: "control-plane",
            message: null,
            metrics: { pendingCount: 1 },
            links: [],
          },
        ],
        observedAt: new Date().toISOString(),
      });
    }
    if (path === `/v1/operator/projects/${PROJECT_ID}`) {
      return json(route, {
        project: {
          id: PROJECT_ID,
          teamId: "org_abcdefgh",
          name: "Mako Test",
          region: "us-east-1",
          state: "active",
          createdAt: new Date().toISOString(),
          updatedAt: new Date().toISOString(),
        },
        environments: [],
      });
    }
    if (path === "/v1/operator/provisioning-workflows") {
      return json(route, { items: [workflow] });
    }
    if (
      path === `/v1/operator/projects/${PROJECT_ID}/quota-overrides` ||
      path === `/v1/operator/projects/${PROJECT_ID}/abuse-responses` ||
      path === `/v1/operator/projects/${PROJECT_ID}/support-sessions`
    ) {
      return json(route, { items: [] });
    }
    if (
      path === `/v1/operator/projects/${PROJECT_ID}/provisioning/wf_abcdefgh/actions/repair` &&
      request.method() === "POST"
    ) {
      repairAttempts += 1;
      repairBodies.push(request.postData() ?? "");
      if (repairAttempts === 1) return apiError(route, 401, "operator_step_up_required");
      return json(route, {
        ...workflow,
        state: "queued",
        operatorRepairs: [
          {
            operatorId: "opr_abcdefgh",
            reason: "Reviewed failed provisioning workflow",
            action: "requeue",
            timestamp: new Date().toISOString(),
            operationKey: JSON.parse(repairBodies[0] ?? "{}").operationKey,
          },
        ],
      });
    }
    return apiError(route, 404, "not_found");
  });

  await page.goto("/operator/operations");
  await page.getByLabel("Email").fill("operator@example.test");
  await page.getByLabel("Password").fill("operator password");
  await page.getByRole("button", { name: "Open operator console" }).click();
  await page.getByLabel("Optional project scope").fill(PROJECT_ID);
  await page.getByRole("button", { name: "Apply scope" }).click();
  await expect(page.getByRole("heading", { name: "Provisioning repair" })).toBeVisible();
  await page.getByLabel("Workflow ID").fill("wf_abcdefgh");
  await page
    .locator('section[aria-labelledby="repair-title"] textarea[name="reason"]')
    .fill("Reviewed failed provisioning workflow");
  await page.getByRole("button", { name: "Apply reasoned repair" }).click();
  await expect(page.getByRole("dialog", { name: "Verify your operator password" })).toBeVisible();
  await page.getByRole("dialog").getByLabel("Password").fill("operator password");
  await page.getByRole("button", { name: "Verify and continue" }).click();
  await expect(page.getByText("Provisioning workflow updated")).toBeVisible();
  expect(repairAttempts).toBe(2);
  expect(repairBodies[1]).toBe(repairBodies[0]);
  expect(JSON.parse(repairBodies[0] ?? "{}").operationKey).toMatch(/^operator-repair-/u);
});

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}

async function apiError(route: Route, status: number, code: string) {
  await json(
    route,
    {
      apiVersion: "v1",
      error: {
        code,
        message: code.replaceAll("_", " "),
        requestId: "req_operatorcc",
        retry: { kind: "never" },
      },
    },
    status,
  );
}
