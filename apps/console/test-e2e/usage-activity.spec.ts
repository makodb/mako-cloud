import { resolve } from "node:path";
import { expect, type Page, type Route, test } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const ENV_A = "env_developa";
const ENV_B = "env_producti";
const MIB = 1024 * 1024;

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

test("environment usage sums flows, averages levels, and names the allowance behind the notice", async ({
  page,
}) => {
  const api = new UsageActivityHarness();
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENV_A}/usage`);
  await expect(page.getByRole("heading", { name: "Usage", exact: true })).toBeVisible();

  // The bill's notice is the contract of the beta: it renders verbatim, and
  // before any figure the bill produced.
  const notice = page.getByRole("note");
  await expect(notice).toHaveText(
    "This bill is informational. Nothing is payable and no charge will be made during the beta.",
  );
  await expect(page.getByText(/period 2026-08-01 to 2026-08-06 \(live\)/u)).toBeVisible();
  const usageSection = page.locator(`section[data-environment-id="${ENV_A}"]`);
  await expect(usageSection).toHaveAttribute("data-state", "ready");
  expect(
    await page.evaluate(() => {
      const note = document.querySelector('[role="note"]');
      const table = document.querySelector("table.usage-table");
      return note !== null && table !== null
        ? (note.compareDocumentPosition(table) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0
        : null;
    }),
  ).toBe(true);

  // Two pages of records were read for the month and aggregated the way the
  // bill rates them: flows sum, levels average. 100 MiB and 300 MiB of
  // storage are 200 MiB, not 400; 700 and 500 invocations are 1,200.
  const row = (resource: string) => usageSection.locator(`tr[data-resource="${resource}"]`);
  await expect(row("storage_bytes")).toContainText("average of 2 samples");
  await expect(row("storage_bytes").locator("td").nth(1)).toHaveText("200.0 MiB");
  await expect(row("storage_bytes").locator("td").nth(2)).toHaveText("500.0 MiB");
  await expect(row("storage_bytes").locator("td").nth(3)).toHaveText("40%");
  await expect(row("edge_invocations_per_month")).toContainText("sum of 2 records");
  await expect(row("edge_invocations_per_month").locator("td").nth(1)).toHaveText("1,200");
  await expect(row("edge_invocations_per_month").locator("td").nth(2)).toHaveText("500,000");
  await expect(row("application_users").locator("td").nth(1)).toHaveText("4");
  await expect(row("replication_bytes_per_month").locator("td").nth(1)).toHaveText("2.0 MiB");
  await expect(row("replication_bytes_per_month").locator("td").nth(2)).toHaveText("1.0 MiB");
  await expect(row("replication_bytes_per_month")).toHaveAttribute("data-over", "true");
  await expect(row("replication_bytes_per_month").locator("td").nth(3)).toContainText("200%");
  await expect(row("replication_bytes_per_month").locator("td").nth(3)).toContainText("over");
  // A level with no sample is not zero; a flow with no record is.
  await expect(row("edge_functions").locator("td").nth(1)).toHaveText("no samples");
  await expect(row("replication_requests_per_minute").locator("td").nth(1)).toHaveText("0");
  await expect(usageSection.getByText(/8 records observed at/u)).toBeVisible();

  // The read asked for the calendar month in UTC, a thousand records a page,
  // and followed the cursor to the end.
  const now = new Date();
  const monthStart = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), 1)).toISOString();
  expect(distinct(api.usageQueries)).toEqual([
    { environmentId: ENV_A, cursor: null, from: monthStart, limit: "1000" },
    { environmentId: ENV_A, cursor: "usage-page-2", from: monthStart, limit: "1000" },
  ]);
  expect(api.unhandled).toEqual([]);
});

test("a failing usage or bill source marks only its own section unavailable", async ({ page }) => {
  const api = new UsageActivityHarness();
  await api.install(page);

  api.usageFailsFor.add(ENV_A);
  await page.goto(`/projects/${PROJECT_ID}/environments/${ENV_A}/usage`);
  const usageSection = page.locator(`section[data-environment-id="${ENV_A}"]`);
  await expect(usageSection).toHaveAttribute("data-state", "unavailable");
  await expect(usageSection.getByRole("alert")).toContainText(
    "Usage signals are unavailable for this environment.",
  );
  await expect(usageSection.locator("table")).toHaveCount(0);
  const planSection = page.locator("section[aria-labelledby='usage-plan-title']");
  await expect(planSection).toHaveAttribute("data-state", "ready");
  await expect(planSection.getByRole("note")).toContainText("no charge will be made");

  // The other way round: the usage still shows, without an allowance.
  api.usageFailsFor.clear();
  api.billFails = true;
  await page.goto(`/projects/${PROJECT_ID}/environments/${ENV_A}/usage`);
  await expect(planSection).toHaveAttribute("data-state", "unavailable");
  await expect(planSection.getByRole("alert")).toContainText("The bill cannot be rated right now.");
  await expect(usageSection).toHaveAttribute("data-state", "ready");
  await expect(usageSection.getByText(/plan allowance unavailable/u)).toBeVisible();
  const storage = usageSection.locator('tr[data-resource="storage_bytes"]');
  await expect(storage.locator("td").nth(1)).toHaveText("200.0 MiB");
  await expect(storage.locator("td").nth(2)).toHaveText("—");
  await expect(storage.locator("td").nth(3)).toHaveText("—");
  expect(api.unhandled).toEqual([]);
});

test("project usage lists every environment and links to its own billing", async ({ page }) => {
  const api = new UsageActivityHarness();
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}/usage`);
  await expect(page.getByRole("heading", { name: "Usage", exact: true })).toBeVisible();

  await expect(page.getByRole("heading", { name: "Team bill" })).toHaveCount(0);
  await expect(page.getByText("-$22.50")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "View project billing" })).toHaveAttribute(
    "href",
    `/projects/${PROJECT_ID}/billing`,
  );

  const sectionA = page.locator(`section[data-environment-id="${ENV_A}"]`);
  const sectionB = page.locator(`section[data-environment-id="${ENV_B}"]`);
  await expect(sectionA.getByRole("heading", { name: "development · active" })).toBeVisible();
  await expect(sectionB.getByRole("heading", { name: "production · active" })).toBeVisible();
  await expect(sectionA).toHaveAttribute("data-state", "ready");
  await expect(sectionB).toHaveAttribute("data-state", "ready");
  await expect(sectionA.locator('tr[data-resource="storage_bytes"] td').nth(1)).toHaveText(
    "200.0 MiB",
  );
  await expect(sectionB.locator('tr[data-resource="storage_bytes"] td').nth(1)).toHaveText(
    "50.0 MiB",
  );
  await expect(
    sectionB.locator('tr[data-resource="edge_invocations_per_month"] td').nth(1),
  ).toHaveText("10");
  // Each environment's month was read in full; the project home's own
  // summaries, if any, read smaller pages and are not counted here.
  expect(
    distinct(api.usageQueries.filter((query) => query.limit === "1000"))
      .map((query) => `${query.environmentId}:${query.cursor ?? ""}`)
      .sort(),
  ).toEqual([`${ENV_A}:`, `${ENV_A}:usage-page-2`, `${ENV_B}:`]);
});

test("project billing excludes shared balances and links to overall usage and plan", async ({
  page,
}) => {
  const api = new UsageActivityHarness();
  await api.install(page);
  await page.goto(`/projects/${PROJECT_ID}/billing`);
  await expect(page.getByRole("heading", { name: "Billing", exact: true })).toBeVisible();
  await expect(page.getByRole("note")).toContainText("no charge will be made");
  await expect(page.getByText("Project usage total")).toBeVisible();
  await expect(page.getByRole("cell", { name: "250.0 MiB" })).toBeVisible();
  await expect(page.getByText("$0.25").first()).toBeVisible();
  await expect(page.getByText("-$22.50")).toHaveCount(0);
  await expect(page.getByText("$25.00")).toHaveCount(0);
  await page.screenshot({
    path: resolve(
      import.meta.dirname,
      "../../../.local/console-project-billing/project-desktop.png",
    ),
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  await page.screenshot({
    path: resolve(
      import.meta.dirname,
      "../../../.local/console-project-billing/project-mobile.png",
    ),
    fullPage: true,
  });
  await page.getByRole("link", { name: "Overall usage and plan" }).click();
  await expect(page.getByRole("heading", { name: "Usage and plan", exact: true })).toBeVisible();
  await expect(page.getByText("-$22.50")).toHaveAttribute("data-negative", "true");
  await expect(page.getByRole("link", { name: "Mako Test Project billing" })).toHaveAttribute(
    "href",
    `/projects/${PROJECT_ID}/billing`,
  );
  expect(api.unhandled).toEqual([]);
});

test("project billing failures do not show zero costs or an owner's bill", async ({ page }) => {
  const api = new UsageActivityHarness();
  api.billFails = true;
  await api.install(page);
  await page.goto(`/projects/${PROJECT_ID}/billing`);
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.getByText("Loading project billing…")).toHaveCount(0);
  await expect(page.getByText("Project usage total")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Overall usage and plan" })).toBeVisible();
  expect(api.unhandled).toEqual([]);
});

test("the activity feed lists newest first, filters by outcome and action, and loads more", async ({
  page,
}) => {
  const api = new UsageActivityHarness();
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENV_A}/activity`);
  await expect(page.getByRole("heading", { name: "Recent activity" })).toBeVisible();
  const feed = page.locator("section.activity-screen");
  await expect(feed).toHaveAttribute("data-state", "ready");
  const rows = feed.locator("table.activity-table tbody tr");

  // Served oldest first; rendered newest first, each naming actor, action,
  // target, outcome, and time.
  await expect(rows).toHaveCount(3);
  await expect(rows.locator("td:nth-child(3)")).toHaveText([
    "credential create",
    "function deploy",
    "policy.activate",
  ]);
  const newest = rows.first();
  await expect(newest.locator("td").nth(1)).toHaveText("dev_member01");
  await expect(newest.locator("td").nth(3)).toHaveText("credentials/pk_browser01");
  await expect(newest.locator("td").nth(4)).toHaveText("denied");
  await expect(newest.locator("time")).toHaveAttribute("datetime", "2026-08-06T12:00:00.000Z");
  // Details are text, never markup.
  await expect(rows.nth(1)).toContainText("<b>bundle</b> rejected");
  await expect(feed.locator("td b")).toHaveCount(0);

  await page.getByLabel("Outcome").selectOption("denied");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute("data-outcome", "denied");
  await expect(feed.getByText("Showing 1 of 3 loaded events.")).toBeVisible();
  await page.getByLabel("Outcome").selectOption("all");
  await page.getByLabel("Action").fill("function");
  await expect(rows).toHaveCount(1);
  await expect(rows.first().locator("td").nth(2)).toHaveText("function deploy");
  await page.getByLabel("Action").fill("");
  await expect(rows).toHaveCount(3);

  const loadMore = page.getByRole("button", { name: "Load more" });
  await expect(loadMore).toBeEnabled();
  await loadMore.click();
  await expect(rows).toHaveCount(4);
  await expect(rows.last().locator("td").nth(2)).toHaveText("project rename");
  await expect(loadMore).toBeDisabled();
  // Reads are left out unless asked for.
  expect(distinct(api.auditQueries)).toEqual([
    { environmentId: ENV_A, cursor: null, limit: "100", changes: "true" },
    { environmentId: ENV_A, cursor: "audit-older", limit: "100", changes: "true" },
  ]);
  expect(api.unhandled).toEqual([]);
});

test("the activity feed leaves reads out until they are asked for", async ({ page }) => {
  const api = new UsageActivityHarness();
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENV_A}/activity`);
  const feed = page.locator("section.activity-screen");
  await expect(feed).toHaveAttribute("data-state", "ready");
  await expect(page.getByLabel("Include reads")).not.toBeChecked();
  expect(api.auditQueries.at(-1)?.changes).toBe("true");

  await page.getByLabel("Include reads").check();
  await expect.poll(() => api.auditQueries.at(-1)?.changes).toBe("false");
  await expect(feed).toHaveAttribute("data-state", "ready");
  expect(api.unhandled).toEqual([]);
});

test("project activity unions every environment's feed newest first and labels each", async ({
  page,
}) => {
  const api = new UsageActivityHarness();
  await api.install(page);

  await page.goto(`/projects/${PROJECT_ID}/activity`);
  await expect(page.getByRole("heading", { name: "Recent activity" })).toBeVisible();
  const feed = page.locator("section.activity-screen");
  await expect(feed).toHaveAttribute("data-state", "ready");
  const rows = feed.locator("table.activity-table tbody tr");
  await expect(rows).toHaveCount(4);
  await expect(rows.locator("td:nth-child(2)")).toHaveText([
    "production",
    "development",
    "development",
    "development",
  ]);
  await expect(rows.first().locator("td").nth(3)).toHaveText("environment suspend");
  await expect(feed.getByText(/the 100 most recent across 2 environments/u)).toBeVisible();
  await expect(page.getByRole("button", { name: "Load more" })).toHaveCount(0);
  expect(
    distinct(api.auditQueries)
      .map((query) => query.environmentId)
      .sort(),
  ).toEqual([ENV_A, ENV_B]);
});

class UsageActivityHarness {
  readonly unhandled: string[] = [];
  readonly usageQueries: {
    environmentId: string;
    cursor: string | null;
    from: string | null;
    limit: string | null;
  }[] = [];
  readonly auditQueries: {
    environmentId: string;
    cursor: string | null;
    limit: string | null;
    changes: string | null;
  }[] = [];
  readonly usageFailsFor = new Set<string>();
  billFails = false;

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
    const observability = path.match(
      /^\/v1\/projects\/prj_[A-Za-z0-9]+\/environments\/(env_[A-Za-z0-9]+)\/observability\/([a-z-]+)$/u,
    );

    if (path === "/v1/teams" && method === "GET") {
      await json(route, { items: [teamFixture()] });
    } else if (path === `/v1/teams/${TEAM_ID}` && method === "GET") {
      await json(route, teamFixture());
    } else if (path === `/v1/teams/${TEAM_ID}/bill` && method === "GET") {
      if (this.billFails) {
        await json(route, apiError("unavailable", "The bill cannot be rated right now."), 503);
      } else {
        await json(route, billFixture());
      }
    } else if (path === `/v1/projects/${PROJECT_ID}/bill` && method === "GET") {
      if (this.billFails) {
        await json(route, apiError("unavailable", "Project billing is unavailable."), 503);
      } else {
        await json(route, {
          projectId: PROJECT_ID,
          teamId: TEAM_ID,
          planId: "pro",
          periodStart: "2026-08-01T00:00:00.000Z",
          periodEnd: NOW,
          retainedFrom: "2026-08-01T00:00:00.000Z",
          observedAt: NOW,
          lineItems: [
            { resource: "storage_bytes", quantity: 250 * MIB, amountMicroDollars: 250_000 },
          ],
          totalMicroDollars: 250_000,
          collectable: false,
          allocation: "proportional_resource_usage",
          notice: billFixture().notice,
        });
      }
    } else if (path === "/v1/projects" && method === "GET") {
      await json(route, { items: [projectFixture()] });
    } else if (path === `/v1/projects/${PROJECT_ID}` && method === "GET") {
      await json(route, projectFixture());
    } else if (path === `/v1/projects/${PROJECT_ID}/environments` && method === "GET") {
      await json(route, { items: [environmentFixture(ENV_A), environmentFixture(ENV_B)] });
    } else if (path.endsWith("/workspace/navigation") && method === "GET") {
      await json(route, navigationFixture(path));
    } else if (observability?.[1] !== undefined && observability[2] === "usage") {
      const environmentId = observability[1];
      const cursor = url.searchParams.get("cursor");
      this.usageQueries.push({
        environmentId,
        cursor,
        from: url.searchParams.get("from"),
        limit: url.searchParams.get("limit"),
      });
      if (this.usageFailsFor.has(environmentId)) {
        await json(
          route,
          apiError("unavailable", "Usage signals are unavailable for this environment."),
          503,
        );
      } else {
        await json(route, usagePage(environmentId, cursor));
      }
    } else if (observability?.[1] !== undefined && observability[2] === "audit-events") {
      const environmentId = observability[1];
      const cursor = url.searchParams.get("cursor");
      this.auditQueries.push({
        environmentId,
        cursor,
        limit: url.searchParams.get("limit"),
        changes: url.searchParams.get("changes"),
      });
      await json(route, auditPage(environmentId, cursor));
    } else if (observability?.[1] !== undefined && method === "GET") {
      // Other signals the project home may summarize are outside this spec.
      await json(route, emptyPage());
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

function projectFixture() {
  return {
    id: PROJECT_ID,
    teamId: TEAM_ID,
    name: "Mako Test Project",
    region: "local",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function environmentFixture(id: string) {
  return {
    id,
    projectId: PROJECT_ID,
    name: id === ENV_A ? "development" : "production",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function navigationFixture(path: string) {
  const base = path.replace(/\/workspace\/navigation$/u, "");
  return ["overview", "data", "collections", "sync", "users", "policies", "functions"].map(
    (id) => ({ id, label: id, path: `${base}/${id}`, permitted: true }),
  );
}

/** The same shape the team page's bill fixture uses: line items carry the
 * resource, quantity, included allowance, overage, and amount; the balance
 * is credits minus charges, unclamped. */
function billFixture() {
  return {
    teamId: TEAM_ID,
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
        quantity: 250 * MIB,
        included: 500 * MIB,
        overage: 0,
        amountMicroDollars: 0,
      },
      {
        resource: "edge_invocations_per_month",
        quantity: 1210,
        included: 500_000,
        overage: 0,
        amountMicroDollars: 0,
      },
      {
        resource: "application_users",
        quantity: 4,
        included: 50_000,
        overage: 0,
        amountMicroDollars: 0,
      },
      {
        resource: "replication_bytes_per_month",
        quantity: 2 * MIB,
        included: 1 * MIB,
        overage: 1 * MIB,
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

function usage(timestamp: string, resource: string, quantity: number) {
  return {
    timestamp,
    payload: {
      kind: "usage",
      resource,
      quantity,
      unit: resource.includes("bytes") ? "bytes" : "count",
    },
  };
}

function usagePage(environmentId: string, cursor: string | null) {
  if (environmentId === ENV_B) {
    return {
      items: [
        usage("2026-08-02T00:00:00.000Z", "storage_bytes", 50 * MIB),
        usage("2026-08-02T00:00:00.000Z", "edge_invocations_per_month", 10),
      ],
      nextCursor: null,
      retention: retention(),
    };
  }
  if (cursor === null) {
    return {
      items: [
        usage("2026-08-02T00:00:00.000Z", "storage_bytes", 100 * MIB),
        usage("2026-08-04T00:00:00.000Z", "storage_bytes", 300 * MIB),
        usage("2026-08-02T00:00:00.000Z", "edge_invocations_per_month", 700),
      ],
      nextCursor: "usage-page-2",
      retention: retention(),
    };
  }
  return {
    items: [
      usage("2026-08-05T00:00:00.000Z", "edge_invocations_per_month", 500),
      usage("2026-08-02T00:00:00.000Z", "application_users", 3),
      usage("2026-08-05T00:00:00.000Z", "application_users", 5),
      usage("2026-08-03T00:00:00.000Z", "replication_bytes_per_month", 1 * MIB),
      usage("2026-08-05T00:00:00.000Z", "replication_bytes_per_month", 1 * MIB),
    ],
    nextCursor: null,
    retention: retention(),
  };
}

function audit(
  timestamp: string,
  actorId: string,
  action: string,
  target: string,
  outcome: "allowed" | "denied" | "failed",
  details: string | null,
) {
  return {
    timestamp,
    payload: {
      kind: "audit",
      teamId: TEAM_ID,
      actorId,
      action,
      target,
      outcome,
      requestId: `req_${action.replace(/[^a-z]/gu, "")}`,
      details,
    },
  };
}

function auditPage(environmentId: string, cursor: string | null) {
  if (environmentId === ENV_B) {
    return {
      items: [
        audit(
          "2026-08-06T13:00:00.000Z",
          "dev_abcdefgh",
          "environment_suspend",
          `environments/${ENV_B}`,
          "allowed",
          null,
        ),
      ],
      nextCursor: null,
      retention: retention(),
    };
  }
  if (cursor === null) {
    // Served oldest first on purpose: the feed orders, not the fixture.
    return {
      items: [
        audit(
          "2026-08-06T10:00:00.000Z",
          "dev_abcdefgh",
          "policy.activate",
          "todos/policy/1",
          "allowed",
          "authorization epoch 2",
        ),
        audit(
          "2026-08-06T12:00:00.000Z",
          "dev_member01",
          "credential_create",
          "credentials/pk_browser01",
          "denied",
          null,
        ),
        audit(
          "2026-08-06T11:00:00.000Z",
          "dev_abcdefgh",
          "function_deploy",
          "functions/hello-world",
          "failed",
          "<b>bundle</b> rejected",
        ),
      ],
      nextCursor: "audit-older",
      retention: retention(),
    };
  }
  return {
    items: [
      audit(
        "2026-08-06T09:00:00.000Z",
        "dev_abcdefgh",
        "project_rename",
        `projects/${PROJECT_ID}`,
        "allowed",
        null,
      ),
    ],
    nextCursor: null,
    retention: retention(),
  };
}

function emptyPage() {
  return { items: [], nextCursor: null, retention: retention() };
}

function retention() {
  return {
    retainedFrom: "2026-07-07T12:00:00.000Z",
    observedAt: NOW,
    retentionSeconds: 2_592_000,
  };
}

/** The dev server renders under React StrictMode, which mounts every effect
 * twice, so the harness sees each initial request twice; what the screens
 * ask for is judged on the distinct requests. */
function distinct<T>(queries: readonly T[]): T[] {
  const seen = new Set<string>();
  return queries.filter((query) => {
    const key = JSON.stringify(query);
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
    return true;
  });
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
