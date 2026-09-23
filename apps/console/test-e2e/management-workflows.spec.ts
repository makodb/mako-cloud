import { expect, type Page, type Route, test } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const LATER = "2026-08-07T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PERSONAL_TEAM_ID = "org_personal";
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

test("role management, provisioning health, and deletion grace run through the API", async ({
  page,
}) => {
  const api = new ManagementApiHarness();
  await api.install(page);
  page.on("dialog", (dialog) => void dialog.accept());

  await page.goto(`/teams/${TEAM_ID}`);
  await expect(page.getByRole("heading", { name: "Mako Test Team" })).toBeVisible();

  // The bill renders with its non-payable notice before any number, shows the
  // metered quantities, and a balance that has gone negative -- shown, marked,
  // and never clamped, because hiding the number is the one thing this
  // surface must not do.
  await expect(page.getByRole("heading", { name: "Billing" })).toBeVisible();
  await expect(page.getByText("no charge will be made during the beta")).toBeVisible();
  await expect(page.getByRole("cell", { name: "storage bytes" })).toBeVisible();
  await expect(page.getByRole("cell", { name: "120.0 MiB" })).toBeVisible();
  await expect(page.getByText("$2.50")).toBeVisible();
  const negativeBalance = page.getByText("-$22.50");
  await expect(negativeBalance).toBeVisible();
  await expect(negativeBalance).toHaveAttribute("data-negative", "true");

  const memberRole = page.getByLabel("Role for dev_member01");
  await memberRole.selectOption("viewer");
  await expect(memberRole).toHaveValue("viewer");
  expect(api.members.find((member) => member.developerIdentityId === "dev_member01")?.role).toBe(
    "viewer",
  );

  api.project.state = "provisioning";
  await page.goto(`/projects/${PROJECT_ID}`);
  await expect(page.getByRole("heading", { name: "Mako Test Project" })).toBeVisible();
  await expect(page.getByText(/provisioning/u).first()).toBeVisible();
  api.project.state = "active";
  await page.waitForTimeout(5_100);
  await expect(page.getByText("active", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("storage in local: healthy")).toBeVisible();

  // The project's own lifecycle controls live in their panel; environments carry
  // their own Suspend/Restore buttons above it.
  const lifecycle = page.getByRole("region", { name: "Provisioning and lifecycle" });
  await lifecycle.getByRole("button", { name: "Suspend", exact: true }).click();
  await expect(lifecycle.getByText("suspended", { exact: true })).toBeVisible();
  expect(api.project.state).toBe("suspended");

  await lifecycle.getByRole("button", { name: "Restore", exact: true }).click();
  await expect(lifecycle.getByText("active", { exact: true })).toBeVisible();
  expect(api.project.state).toBe("active");

  await lifecycle.getByRole("button", { name: "Request deletion" }).click();
  await expect(page.getByText(/Restorable until/u)).toBeVisible();
  expect(api.project.state).toBe("deletion_grace");
  await lifecycle.getByRole("button", { name: "Restore", exact: true }).click();
  await expect.poll(() => api.project.state).toBe("active");
  await page.getByRole("button", { name: "Request deletion" }).click();
  await expect.poll(() => api.project.state).toBe("deletion_grace");
  expect(api.unhandled).toEqual([]);
});

test("a personal space leads the home screen and creates projects without naming a team", async ({
  page,
}) => {
  const api = new ManagementApiHarness();
  await api.install(page);

  // Before the first individual project exists there is no personal space:
  // the home screen leads with the first-project form, and the teams the
  // developer has joined follow it.
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Your projects" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Create your first project" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Teams" })).toBeVisible();
  await expect(page.getByRole("button", { name: /Mako Test Team/u })).toBeVisible();

  await page.getByLabel("Project name").fill("Side project");
  await page.getByLabel("Data region").selectOption("local");
  await page.getByRole("button", { name: "Create and provision" }).click();

  // The space now exists and its projects replace the empty state; it is
  // never listed among the teams.
  await expect(page.getByRole("button", { name: /Side project/u })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Create your first project" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Mako Test Team/u })).toHaveCount(1);
  await expect(page.locator(".resource-card")).toHaveCount(1);
  expect(api.personalSpace?.kind).toBe("personal");

  // Creating from the personal projects panel posts without a team as well.
  await page.getByText("Create project", { exact: true }).click();
  await page.getByLabel("Project name").fill("Second project");
  await page.getByLabel("Data region").selectOption("local");
  await page.getByRole("button", { name: "Create and provision" }).click();
  await expect(page).toHaveURL(/\/projects\/prj_persona2$/u);
  await expect(page.getByRole("heading", { name: "Second project" })).toBeVisible();
  expect(api.createProjectBodies).toEqual([
    { name: "Side project", region: "local" },
    { name: "Second project", region: "local" },
  ]);
  expect(api.createProjectBodies.map((body) => Object.keys(body).sort())).toEqual([
    ["name", "region"],
    ["name", "region"],
  ]);

  // The space's own screen is labelled as such and hides membership
  // management: a personal space has exactly one member and refuses changes.
  await page.goto(`/teams/${PERSONAL_TEAM_ID}`);
  await expect(page.getByText("Personal space", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Owner", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Billing" })).toBeVisible();
  await expect(page.getByRole("button", { name: /Side project/u })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Members" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Invite a member" })).toHaveCount(0);
  const switcher = page.getByLabel("Switch team");
  await expect(switcher).toHaveValue(PERSONAL_TEAM_ID);
  await expect(switcher.locator("option")).toHaveText(["Mako Test Team", "Your projects"]);

  // A joined team still manages its members.
  await switcher.selectOption(TEAM_ID);
  await expect(page.getByRole("heading", { name: "Mako Test Team" })).toBeVisible();
  await expect(page.getByText("Team", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Members" })).toBeVisible();
  expect(api.unhandled).toEqual([]);
});

test("policy and application-user administration enforce full management workflows", async ({
  page,
}) => {
  const api = new ManagementApiHarness();
  await api.install(page);
  page.on("dialog", (dialog) => void dialog.accept());

  await page.goto(
    `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/collections/todos/policies`,
  );
  await expect(page.getByText("Default deny is active.")).toBeVisible();
  await page.getByRole("button", { name: "Create draft" }).click();
  await expect(page.getByText("Policy v1", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Validate against schema" }).click();
  await expect(page.getByText("Schema-aware validation passed.")).toBeVisible();
  await page.getByRole("button", { name: "Activate" }).click();
  await expect(page.getByText("An active policy is installed.")).toBeVisible();
  await expect(page.getByText("Authorization epoch 2")).toBeVisible();

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/users`);
  await page.getByLabel("Action").selectOption("create");
  await page.getByLabel("Email", { exact: true }).fill("new-user@example.test");
  await page.getByRole("button", { name: "Create application user" }).click();
  await expect(page).toHaveURL(/\/users\/usr_newuser01$/u);
  await expect(page.getByRole("heading", { name: "new-user@example.test" })).toBeVisible();

  await page.getByRole("button", { name: "Disable user" }).click();
  await expect.poll(() => api.user.status).toBe("disabled");
  await expect(page.getByText(/disabled/u).first()).toBeVisible();
  await page.getByRole("button", { name: "Restore user" }).click();
  await expect.poll(() => api.user.status).toBe("active");
  await expect(page.getByText("Status: active", { exact: true }).first()).toBeVisible();
  await page.getByRole("button", { name: "Revoke all sessions" }).click();
  await expect.poll(() => api.user.sessions[0]?.status).toBe("revoked");
  await page.getByRole("button", { name: "Delete user" }).click();
  await expect.poll(() => api.user.status).toBe("deleted");
  await expect(page.getByText(/deleted/u).first()).toBeVisible();
  expect(api.unhandled).toEqual([]);
});

test("credentials, functions, logs, metrics, and audit export use the management contract", async ({
  page,
}) => {
  const api = new ManagementApiHarness();
  await api.install(page);
  page.on("dialog", (dialog) => void dialog.accept());

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/credentials`);
  await page.getByLabel("Credential ID", { exact: true }).first().fill("pk_browser01");
  await page.getByRole("button", { name: "Create credential" }).click();
  await expect(page.getByText("••••••••••••••••")).toBeVisible();
  await page.getByRole("button", { name: "Reveal value" }).click();
  await expect(page.getByText("mako_public_browser_secret")).toBeVisible();
  await page.getByRole("button", { name: "I have stored it securely" }).click();
  await page.getByRole("button", { name: "Retire credential" }).click();
  await expect(page.getByText(/retired/u).first()).toBeVisible();

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/functions`);
  await page.getByLabel("Function name").fill("hello-world");
  await page.getByRole("button", { name: "Create function" }).click();
  await expect(page).toHaveURL(/\/functions\/hello-world$/u);
  await page.getByLabel("Source files").setInputFiles({
    name: "index.ts",
    mimeType: "text/typescript",
    buffer: Buffer.from("export default () => new Response('ok');\n"),
  });
  await page.getByLabel("Entrypoint path").fill("index.ts");
  await page.getByRole("button", { name: "Upload and validate" }).click();
  await expect(page.getByText("Immutable bundle ready")).toBeVisible();
  await page.getByLabel("Runtime version").fill("deno-compatible-v1");
  await page.getByRole("button", { name: "Validate and deploy" }).click();
  await expect(page.getByText("Version 1")).toBeVisible();
  await page.getByRole("button", { name: "Promote" }).click();
  await expect.poll(() => api.edgeFunction?.activeVersion).toBe(1);
  await expect(
    page
      .locator("article.resource-card")
      .filter({ hasText: "Version 1" })
      .getByText(/active/u),
  ).toBeVisible();
  await page.getByRole("button", { name: "Invoke test" }).click();
  await expect(page.getByText("HTTP 200")).toBeVisible();
  await expect(page.getByText("function invocation completed")).toBeVisible();
  await expect(page.getByText(/1 calls · 0 errors/u)).toBeVisible();

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/observability`);
  await page.getByRole("tab", { name: "Audit history" }).click();
  await page.getByLabel("Search loaded records").fill("policy.activate");
  await expect(page.getByText("req_audit01")).toBeVisible();
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export JSON" }).click();
  await expect((await download).suggestedFilename()).toMatch(/^mako-audit-.*\.json$/u);
  expect(api.unhandled).toEqual([]);
});

class ManagementApiHarness {
  readonly unhandled: string[] = [];
  readonly members = [membership("dev_abcdefgh", "owner"), membership("dev_member01", "developer")];
  readonly project = projectFixture();
  readonly environment = environmentFixture();
  personalSpace: ReturnType<typeof personalSpaceFixture> | null = null;
  readonly personalProjects: ReturnType<typeof projectFixture>[] = [];
  readonly createProjectBodies: Record<string, unknown>[] = [];
  policy = policyFixture("draft");
  activePolicy: Record<string, unknown> = { defaultDeny: true, authorizationEpoch: 1 };
  user = userFixture();
  credential: Record<string, unknown> | null = null;
  edgeFunction: Record<string, unknown> | null = null;
  deployments: Record<string, unknown>[] = [];

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

    if (path === "/v1/teams" && method === "GET") {
      await json(route, {
        items: this.personalSpace === null ? [teamFixture()] : [teamFixture(), this.personalSpace],
      });
    } else if (path === `/v1/teams/${TEAM_ID}` && method === "GET") {
      await json(route, teamFixture());
    } else if (path === `/v1/teams/${TEAM_ID}/bill` && method === "GET") {
      await json(route, billFixture(TEAM_ID));
    } else if (
      path === `/v1/teams/${PERSONAL_TEAM_ID}` &&
      method === "GET" &&
      this.personalSpace !== null
    ) {
      await json(route, this.personalSpace);
    } else if (path === `/v1/teams/${PERSONAL_TEAM_ID}/bill` && method === "GET") {
      await json(route, billFixture(PERSONAL_TEAM_ID));
    } else if (path === `/v1/teams/${TEAM_ID}/members` && method === "GET") {
      await json(route, { items: this.members });
    } else if (path === `/v1/teams/${TEAM_ID}/members/dev_member01` && method === "PATCH") {
      const body = request.postDataJSON() as {
        role: "owner" | "administrator" | "developer" | "viewer";
      };
      this.members[1] = membership("dev_member01", body.role);
      await json(route, this.members[1]);
    } else if (path === "/v1/projects" && method === "GET") {
      await json(route, {
        items:
          url.searchParams.get("teamId") === PERSONAL_TEAM_ID
            ? this.personalProjects
            : [this.project],
      });
    } else if (path === "/v1/projects" && method === "POST") {
      // Without a teamId the project lands in the caller's personal space,
      // which is created on first use.
      const body = request.postDataJSON() as { name: string; region: string };
      this.createProjectBodies.push(body);
      this.personalSpace ??= personalSpaceFixture();
      const project = {
        ...projectFixture(),
        id: `prj_persona${this.personalProjects.length + 1}`,
        teamId: PERSONAL_TEAM_ID,
        name: body.name,
        region: body.region,
      };
      this.personalProjects.push(project);
      await json(route, project, 202);
    } else if (method === "GET" && /^\/v1\/projects\/prj_persona\d$/u.test(path)) {
      await json(
        route,
        this.personalProjects.find((project) => path.endsWith(project.id)),
      );
    } else if (method === "GET" && /^\/v1\/projects\/prj_persona\d\/environments$/u.test(path)) {
      await json(route, { items: [] });
    } else if (path === `/v1/projects/${PROJECT_ID}` && method === "GET") {
      await json(route, this.project);
    } else if (path === `/v1/projects/${PROJECT_ID}` && method === "DELETE") {
      this.project.state = "deletion_grace";
      this.project.deletionDeadline = LATER;
      await json(route, this.project, 202);
    } else if (path === `/v1/projects/${PROJECT_ID}/actions/suspend` && method === "POST") {
      this.project.state = "suspended";
      await json(route, this.project, 202);
    } else if (path === `/v1/projects/${PROJECT_ID}/actions/restore` && method === "POST") {
      this.project.state = "active";
      this.project.deletionDeadline = undefined;
      await json(route, this.project, 202);
    } else if (path === `/v1/projects/${PROJECT_ID}/environments` && method === "GET") {
      await json(route, { items: [this.environment] });
    } else if (
      path.endsWith(`/environments/${ENVIRONMENT_ID}/workspace/navigation`) &&
      method === "GET"
    ) {
      await json(
        route,
        [
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
        })),
      );
    } else if (path.endsWith("/workspace/summary") && method === "GET") {
      await json(route, {
        tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
        sections: {
          lifecycle: {
            status: "current",
            observedAtUnixSeconds: 1_786_579_200,
            freshUntilUnixSeconds: 1_786_579_260,
            payload: {
              ready: this.environment.state === "active",
              project: this.project.state,
              environment: this.environment.state,
              region: this.project.region,
            },
          },
        },
      });
    } else if (path.endsWith("/connect") && method === "GET") {
      await json(route, {
        tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
        publicEndpoint: "https://cloud-test.makodb.com",
        publicKeyId: "key_public01",
        publicKey: "",
        collections: [],
        rxdbClientRange: ">=17 <18",
        templateVersion: 1,
      });
    } else if (path.endsWith("/collections/todos/policies") && method === "GET") {
      await json(route, this.activePolicy);
    } else if (path.endsWith("/collections/todos/policies") && method === "POST") {
      this.policy = policyFixture("draft");
      await json(route, this.policy, 201);
    } else if (path.endsWith("/policies/1/actions/validate") && method === "POST") {
      this.policy = policyFixture("validated");
      await json(route, { valid: true, policy: this.policy });
    } else if (path.endsWith("/policies/1/actions/activate") && method === "POST") {
      this.policy = policyFixture("active");
      this.activePolicy = { defaultDeny: false, authorizationEpoch: 2, policy: this.policy };
      await json(route, this.activePolicy);
    } else if (path.endsWith(`/environments/${ENVIRONMENT_ID}/users`) && method === "GET") {
      await json(route, { users: [], truncated: false });
    } else if (path.endsWith(`/environments/${ENVIRONMENT_ID}/users`) && method === "POST") {
      const body = request.postDataJSON() as { email: string };
      this.user = userFixture(body.email);
      await json(route, this.user, 201);
    } else if (path.endsWith(`/users/${this.user.id}`) && method === "GET") {
      await json(route, this.user);
    } else if (path.endsWith(`/users/${this.user.id}/actions/disable`) && method === "POST") {
      this.user.status = "disabled";
      await json(route, this.user);
    } else if (path.endsWith(`/users/${this.user.id}/actions/restore`) && method === "POST") {
      this.user.status = "active";
      await json(route, this.user);
    } else if (
      path.endsWith(`/users/${this.user.id}/actions/revoke-sessions`) &&
      method === "POST"
    ) {
      this.user.sessions = this.user.sessions.map((session) => ({ ...session, status: "revoked" }));
      this.user.sessionEpoch += 1;
      await json(route, this.user);
    } else if (path.endsWith(`/users/${this.user.id}`) && method === "DELETE") {
      this.user.status = "deleted";
      await json(route, this.user);
    } else if (path === `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/signing-keys`) {
      await json(route, { items: [signingKeyFixture()] });
    } else if (path === `/v1/teams/${TEAM_ID}/automation-tokens`) {
      await json(route, { items: [] });
    } else if (path.endsWith("/credentials/public") && method === "POST") {
      const body = request.postDataJSON() as { id: string };
      this.credential = credentialFixture(body.id);
      await json(route, { credential: this.credential, value: "mako_public_browser_secret" }, 201);
    } else if (path.endsWith("/credentials/pk_browser01") && method === "DELETE") {
      if (this.credential !== null) {
        this.credential.state = "retired";
      }
      await noContent(route);
    } else if (path.endsWith(`/environments/${ENVIRONMENT_ID}/functions`) && method === "GET") {
      await json(route, { items: this.edgeFunction === null ? [] : [this.edgeFunction] });
    } else if (path.endsWith(`/environments/${ENVIRONMENT_ID}/functions`) && method === "POST") {
      const body = request.postDataJSON() as {
        name: string;
        configuration: Record<string, unknown>;
      };
      this.edgeFunction = functionFixture(body.name, body.configuration);
      await json(route, this.edgeFunction, 201);
    } else if (path.endsWith("/function-bundles") && method === "POST") {
      await json(route, bundleFixture(), 201);
    } else if (path.endsWith("/functions/hello-world/versions") && method === "GET") {
      await json(route, { items: this.deployments });
    } else if (path.endsWith("/functions/hello-world/versions") && method === "POST") {
      const body = request.postDataJSON() as {
        version: number;
        bundleDigest: string;
        entrypoint: string;
        runtimeVersion: string;
      };
      const deployment = deploymentFixture(body, this.edgeFunction?.configuration);
      this.deployments.push(deployment);
      await json(route, deployment, 201);
    } else if (path.endsWith("/functions/hello-world/versions/1/actions/promote")) {
      if (this.edgeFunction !== null) {
        this.edgeFunction.activeVersion = 1;
      }
      await json(route, this.edgeFunction);
    } else if (path.endsWith("/functions/hello-world/actions/test") && method === "POST") {
      await json(route, {
        status: 200,
        headers: { "content-type": "text/plain" },
        body: "b2s=",
        correlationId: "corr_test01",
      });
    } else if (path.endsWith("/functions/hello-world/schedules") && method === "GET") {
      await json(route, { items: [] });
    } else if (path.endsWith("/functions/hello-world/logs") && method === "GET") {
      await json(route, {
        items: [
          {
            timestamp: NOW,
            level: "info",
            message: "function invocation completed",
            correlationId: "corr_log01",
            version: 1,
            region: "local",
          },
        ],
        nextCursor: null,
      });
    } else if (path.endsWith("/functions/hello-world") && method === "GET") {
      await json(route, this.edgeFunction);
    } else if (path.includes("/observability/") && method === "GET") {
      await json(route, observabilityPage(path.split("/").at(-1) ?? ""));
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

function billFixture(teamId: string) {
  return {
    teamId,
    planId: "pro",
    periodStart: "2026-08-01T00:00:00Z",
    periodEnd: NOW,
    observedAt: NOW,
    finalized: false,
    closedAt: null,
    baseMicroDollars: 25_000_000,
    lineItems: [
      {
        resource: "storage_bytes",
        quantity: 120 * 1024 * 1024,
        included: 500 * 1024 * 1024,
        overage: 0,
        amountMicroDollars: 0,
      },
      {
        resource: "edge_invocations_per_month",
        quantity: 1200,
        included: 500000,
        overage: 0,
        amountMicroDollars: 0,
      },
    ],
    totalMicroDollars: 25_000_000,
    creditsMicroDollars: 2_500_000,
    balanceMicroDollars: -22_500_000,
    collectable: false,
    notice:
      "This bill is informational. Nothing is payable and no charge will be made during the beta.",
  };
}

function membership(developerIdentityId: string, role: string) {
  return {
    teamId: TEAM_ID,
    developerIdentityId,
    role,
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

function environmentFixture() {
  return {
    id: ENVIRONMENT_ID,
    projectId: PROJECT_ID,
    name: "development",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function policyFixture(state: string) {
  return {
    version: 1,
    state,
    rules: [
      {
        id: "owner-read",
        effect: "allow",
        operations: ["read"],
        expression: "oldDocument.ownerId == identity.userId",
      },
    ],
    diagnostics: [],
  };
}

function userFixture(email = "new-user@example.test") {
  return {
    id: "usr_newuser01",
    email,
    status: "active",
    trustedMetadata: {},
    profileMetadata: {},
    sessionEpoch: 1,
    sessions: [{ id: "ses_browser01", status: "active", createdAt: NOW, expiresAt: LATER }],
    sessionsTruncated: false,
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function credentialFixture(id: string) {
  return { id, kind: "public", state: "active", createdAt: NOW };
}

function signingKeyFixture() {
  return { keyId: "key_browser01", state: "active", createdAt: NOW };
}

function functionFixture(name: string, configuration: Record<string, unknown>) {
  return {
    name,
    state: "active",
    activeVersion: null,
    configuration,
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function bundleFixture() {
  return {
    status: "ready",
    artifact: {
      digest: `sha256:${"a".repeat(64)}`,
      format: "source_archive_v1",
      entrypoint: "index.ts",
      sizeBytes: 48,
      moduleCount: 1,
      createdAt: NOW,
    },
    diagnostics: [],
  };
}

function deploymentFixture(
  input: { version: number; bundleDigest: string; entrypoint: string; runtimeVersion: string },
  configuration: unknown,
) {
  return {
    functionName: "hello-world",
    ...input,
    bundleFormat: "source_archive_v1",
    bundleSizeBytes: 48,
    configuration,
    secretVersions: [],
    state: "healthy",
    diagnostic: null,
    createdAt: NOW,
  };
}

function observabilityPage(kind: string) {
  const payloads: Record<string, Record<string, unknown>> = {
    usage: { kind: "usage", resource: "storage_bytes", quantity: 1024, unit: "bytes" },
    quotas: {
      kind: "quota",
      resource: "storage_bytes",
      limit: 4096,
      consumed: 1024,
      retryAfter: null,
    },
    health: {
      kind: "health",
      service: "storage",
      region: "local",
      status: "healthy",
      diagnostic: null,
    },
    "replication-errors": {
      kind: "replication_error",
      collectionId: "todos",
      category: "conflict",
      retryable: true,
      message: "retry from checkpoint",
      correlationId: "corr_rep01",
    },
    "auth-events": {
      kind: "authentication_event",
      category: "sign_in",
      outcome: "allowed",
      applicationUserId: "usr_newuser01",
      message: "sign-in succeeded",
      correlationId: "corr_auth01",
    },
    "function-metrics": {
      kind: "function_metric",
      functionName: "hello-world",
      version: 1,
      region: "local",
      invocationCount: 1,
      errorCount: 0,
      latencyMilliseconds: 3,
      computeMilliseconds: 2,
    },
    "audit-events": {
      kind: "audit",
      teamId: TEAM_ID,
      actorId: "dev_abcdefgh",
      action: "policy.activate",
      target: "todos/policy/1",
      outcome: "allowed",
      requestId: "req_audit01",
      details: "authorization epoch 2",
    },
  };
  const payload = payloads[kind];
  return {
    items: payload === undefined ? [] : [{ timestamp: NOW, payload }],
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

async function noContent(route: Route) {
  await route.fulfill({ status: 204, body: "" });
}
