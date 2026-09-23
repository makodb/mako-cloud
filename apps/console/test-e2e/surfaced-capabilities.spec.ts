import { expect, type Page, type Route, test } from "@playwright/test";

// Capabilities the management API already serves, reached from the console:
// retained logs, index build state, data-job detail, JWT signing-key
// initialization, and team renaming. Every route is mocked to the generated
// schema's shapes; the console is the unit under test.

const NOW = "2026-08-06T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const ENVIRONMENT_PATH = `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const ENVIRONMENT_API = `/v1${ENVIRONMENT_PATH}`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

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
      developerExplorerAdminEnabled: true,
      developerDataJobsEnabled: true,
    };
  });
});

test("retained logs list newest first, filter by level and source, and page through the window", async ({
  page,
}) => {
  const api = new SurfacedApiHarness();
  await api.install(page);

  await page.goto(`${ENVIRONMENT_PATH}/logs`);
  await expect(page.getByRole("heading", { name: "Retained logs" })).toBeVisible();
  await expect(page.getByText(/Retained from/u)).toBeVisible();

  // The first page arrives out of order and renders newest first.
  const rows = page.locator("table.log-table tbody tr");
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(0)).toContainText("replication push rejected");
  await expect(rows.nth(1)).toContainText("function hello-world completed");
  await expect(rows.nth(2)).toContainText("checkpoint expired");
  await expect(page.getByText("Showing 3 of 3 loaded lines.")).toBeVisible();

  // Messages are text, never markup, even when they carry tags.
  await expect(rows.nth(0)).toContainText("<b>bold</b>");
  await expect(page.locator("table.log-table b")).toHaveCount(0);

  // The default window is the last hour, bounded and capped.
  const firstQuery = api.logQueries[0];
  expect(firstQuery?.get("limit")).toBe("200");
  expect(firstQuery?.has("cursor")).toBe(false);
  expect(Date.now() - Date.parse(firstQuery?.get("from") ?? "")).toBeGreaterThan(
    3_600_000 - 60_000,
  );
  expect(Date.now() - Date.parse(firstQuery?.get("from") ?? "")).toBeLessThan(3_600_000 + 60_000);

  // Level and source filters narrow the loaded lines without a request.
  const requestsBeforeFilters = api.logQueries.length;
  await page.getByLabel("Level").selectOption("error");
  await expect(rows).toHaveCount(1);
  await expect(rows.nth(0)).toContainText("replication push rejected");
  await page.getByLabel("Level").selectOption("warn");
  await expect(rows).toHaveCount(1);
  await expect(rows.nth(0)).toContainText("checkpoint expired");
  await page.getByLabel("Level").selectOption("debug");
  await expect(page.getByText("No loaded lines match the level and source filters.")).toBeVisible();
  await page.getByLabel("Level").selectOption("all");
  await page.getByLabel("Source").selectOption("sync");
  await expect(rows).toHaveCount(1);
  expect(api.logQueries.length).toBe(requestsBeforeFilters);

  // Load more follows the cursor and keeps the older lines after the newer ones.
  await page.getByRole("button", { name: "Load more" }).click();
  await expect(rows).toHaveCount(2);
  await page.getByLabel("Source").selectOption("all");
  await expect(rows).toHaveCount(5);
  await expect(rows.nth(3)).toContainText("opened snapshot");
  await expect(rows.nth(4)).toContainText("pull served 12 documents");
  expect(api.logQueries.at(-1)?.get("cursor")).toBe("logs-page-2");
  await expect(page.getByRole("button", { name: "Load more" })).toBeDisabled();
  await expect(page.getByText("Every retained line in this window is loaded.")).toBeVisible();

  // A preset re-queries with the matching window; a custom range sends both bounds.
  await page.getByLabel("Time range").selectOption("7d");
  await expect.poll(() => api.logQueries.length).toBe(requestsBeforeFilters + 2);
  const weekQuery = api.logQueries.at(-1);
  expect(Date.now() - Date.parse(weekQuery?.get("from") ?? "")).toBeGreaterThan(
    604_800_000 - 60_000,
  );
  expect(weekQuery?.has("until")).toBe(false);
  await page.getByLabel("Time range").selectOption("custom");
  await page.getByLabel("From", { exact: true }).fill("2026-08-06T11:00");
  await page.getByLabel("Until", { exact: true }).fill("2026-08-06T13:00");
  await page.getByRole("button", { name: "Apply time range" }).click();
  await expect.poll(() => api.logQueries.length).toBe(requestsBeforeFilters + 3);
  const customQuery = api.logQueries.at(-1);
  expect(Date.parse(customQuery?.get("until") ?? "")).toBeGreaterThan(
    Date.parse(customQuery?.get("from") ?? ""),
  );

  // An empty window shows the empty state rather than a blank table.
  api.logsAvailable = false;
  await page.getByLabel("Time range").selectOption("24h");
  await expect(page.getByText("No log lines are retained for this window.")).toBeVisible();
  expect(api.unhandled).toEqual([]);
});

test("index build state shows the latest state per index with its history", async ({ page }) => {
  const api = new SurfacedApiHarness();
  await api.install(page);

  await page.goto(`${ENVIRONMENT_PATH}/observability`);
  await expect(page.getByRole("heading", { name: "Indexes" })).toBeVisible();
  await expect(page.getByText("2 indexes · 3 events")).toBeVisible();

  const rows = page.locator("table.index-state-table tbody tr");
  await expect(rows).toHaveCount(2);
  // The index with the newest event leads, and its latest state wins over the
  // older "building" event, which stays available as history.
  await expect(rows.nth(0)).toContainText("by_owner");
  await expect(rows.nth(0).locator(".status-pill")).toHaveText("ready");
  await expect(rows.nth(0)).toContainText("100%");
  await expect(rows.nth(0).getByText("2 events")).toBeVisible();
  await expect(rows.nth(0).getByText(/building/u)).toBeHidden();
  await rows.nth(0).getByText("2 events").click();
  await expect(rows.nth(0).getByText(/building v1 \(40%\)/u)).toBeVisible();
  await expect(rows.nth(1)).toContainText("by_created");
  await expect(rows.nth(1).locator(".status-pill")).toHaveText("failed");
  await expect(rows.nth(1)).toContainText("unsupported field type");
  expect(api.unhandled).toEqual([]);
});

test("a data job opens in detail from the jobs list", async ({ page }) => {
  const api = new SurfacedApiHarness();
  await api.install(page);

  await page.goto(`${ENVIRONMENT_PATH}/data`);
  await page.getByRole("button", { name: "Create access grant" }).click();
  await page.getByRole("tab", { name: "Import / export" }).click();
  await expect(page.getByRole("heading", { name: "Import and export jobs" })).toBeVisible();
  await expect(page.getByText("djob_export01", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Details for djob_export01" }).click();
  const detail = page.locator("article.job-detail");
  await expect(detail.getByRole("heading", { name: "Job djob_export01" })).toBeVisible();
  await expect(detail).toContainText("Status");
  await expect(detail).toContainText("succeeded");
  await expect(detail).toContainText("export");
  await expect(detail).toContainText("exported");
  await expect(detail).toContainText("Manifest: 2 rows, 64 bytes");
  await expect(detail).toContainText("sha256:export_manifest");
  await expect(detail.getByRole("button", { name: "Download and verify" })).toBeVisible();
  await expect(detail.getByText("Failure diagnostic")).toHaveCount(0);

  await page.getByRole("button", { name: "Details for djob_import02" }).click();
  await expect(detail.getByRole("heading", { name: "Job djob_import02" })).toBeVisible();
  await expect(detail).toContainText("failed");
  await expect(detail).toContainText("create only");
  await expect(detail.getByText("Failure diagnostic")).toBeVisible();
  await expect(detail).toContainText("row 3: schema validation failed: ownerId is required");
  await expect(detail.getByRole("button", { name: "Download and verify" })).toHaveCount(0);
  expect(api.jobDetailRequests).toEqual(["djob_export01", "djob_import02"]);

  await detail.getByRole("button", { name: "Close" }).click();
  await expect(detail).toHaveCount(0);
  expect(api.unhandled).toEqual([]);
});

test("signing-key initialization is offered only without a key and calls the endpoint", async ({
  page,
}) => {
  const api = new SurfacedApiHarness();
  await api.install(page);
  page.on("dialog", (dialog) => void dialog.accept());

  await page.goto(`${ENVIRONMENT_PATH}/credentials`);
  await expect(page.getByRole("heading", { name: "JWT signing keys" })).toBeVisible();
  await expect(page.getByText("No signing key exists for this environment yet.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Rotate signing key" })).toHaveCount(0);

  await page.getByRole("button", { name: "Initialize signing key" }).click();
  await expect(page.locator("li.resource-row", { hasText: "key_init01" })).toBeVisible();
  await expect(page.getByText(/Signing key key_init01 initialized \(active\)/u)).toBeVisible();
  await expect(page.getByRole("button", { name: "Initialize signing key" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Rotate signing key" })).toBeVisible();
  expect(api.initializeCalls).toHaveLength(1);
  expect(api.initializeCalls[0]).toMatch(UUID);

  // With a key present from the start, the action is never offered.
  await page.reload();
  await expect(page.locator("li.resource-row", { hasText: "key_init01" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Initialize signing key" })).toHaveCount(0);
  expect(api.initializeCalls).toHaveLength(1);
  expect(api.unhandled).toEqual([]);
});

test("a team administrator renames the team and the name follows everywhere", async ({ page }) => {
  const api = new SurfacedApiHarness();
  await api.install(page);

  await page.goto(`/teams/${TEAM_ID}`);
  await expect(page.getByRole("heading", { name: "Mako Test Team" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Team name" })).toBeVisible();

  await page.getByLabel("New name").fill("Renamed Team");
  await page.getByRole("button", { name: "Rename team" }).click();
  await expect(page.getByRole("heading", { name: "Renamed Team" })).toBeVisible();
  await expect(page.getByText("Renamed to Renamed Team. The change is audited.")).toBeVisible();
  await expect(page.getByLabel("Switch team").locator("option")).toHaveText(["Renamed Team"]);
  expect(api.renameBodies).toEqual([{ name: "Renamed Team" }]);
  expect(api.team.name).toBe("Renamed Team");

  // A member who cannot manage the team is not offered the rename.
  api.members[0] = membership("dev_abcdefgh", "developer");
  await page.reload();
  await expect(page.getByRole("heading", { name: "Renamed Team" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Members" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Team name" })).toHaveCount(0);
  expect(api.unhandled).toEqual([]);
});

class SurfacedApiHarness {
  readonly unhandled: string[] = [];
  readonly logQueries: URLSearchParams[] = [];
  readonly initializeCalls: string[] = [];
  readonly renameBodies: Record<string, unknown>[] = [];
  readonly jobDetailRequests: string[] = [];
  readonly members = [membership("dev_abcdefgh", "owner"), membership("dev_member01", "developer")];
  readonly team = teamFixture();
  readonly signingKeys: Record<string, unknown>[] = [];
  logsAvailable = true;

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
      await json(route, { items: [this.team] });
    } else if (path === `/v1/teams/${TEAM_ID}` && method === "GET") {
      await json(route, this.team);
    } else if (path === `/v1/teams/${TEAM_ID}` && method === "PATCH") {
      const body = request.postDataJSON() as { name: string };
      this.renameBodies.push(body);
      this.team.name = body.name;
      this.team.updatedAt = "2026-08-06T12:05:00.000Z";
      await json(route, this.team);
    } else if (path === `/v1/teams/${TEAM_ID}/members` && method === "GET") {
      await json(route, { items: this.members });
    } else if (path === `/v1/teams/${TEAM_ID}/bill` && method === "GET") {
      await json(route, billFixture());
    } else if (path === `/v1/teams/${TEAM_ID}/automation-tokens` && method === "GET") {
      await json(route, { items: [] });
    } else if (path === "/v1/projects" && method === "GET") {
      await json(route, { items: [projectFixture()] });
    } else if (path === `/v1/projects/${PROJECT_ID}` && method === "GET") {
      await json(route, projectFixture());
    } else if (path === `/v1/projects/${PROJECT_ID}/environments` && method === "GET") {
      await json(route, { items: [environmentFixture()] });
    } else if (path === `${ENVIRONMENT_API}/workspace/navigation` && method === "GET") {
      await json(route, navigationFixture());
    } else if (path === `${ENVIRONMENT_API}/observability/logs` && method === "GET") {
      this.logQueries.push(url.searchParams);
      await json(
        route,
        this.logsAvailable ? logsPage(url.searchParams.get("cursor")) : emptyPage(),
      );
    } else if (path === `${ENVIRONMENT_API}/observability/index-states` && method === "GET") {
      await json(route, indexStatesPage());
    } else if (path.startsWith(`${ENVIRONMENT_API}/observability/`) && method === "GET") {
      await json(route, emptyPage());
    } else if (path === `${ENVIRONMENT_API}/signing-keys` && method === "GET") {
      await json(route, { items: this.signingKeys });
    } else if (path === `${ENVIRONMENT_API}/signing-keys/actions/initialize` && method === "POST") {
      this.initializeCalls.push(request.headers()["idempotency-key"] ?? "");
      const key = { keyId: "key_init01", state: "active", createdAt: NOW };
      this.signingKeys.push(key);
      await json(route, key, 201);
    } else if (path === `${ENVIRONMENT_API}/collections` && method === "GET") {
      await json(route, { items: [collectionFixture()] });
    } else if (path === `${ENVIRONMENT_API}/users` && method === "GET") {
      await json(route, {
        users: [
          {
            id: "usr_abcdefgh",
            email: "app-user@example.test",
            status: "active",
            createdAt: NOW,
            updatedAt: NOW,
          },
        ],
        truncated: false,
      });
    } else if (path === `${ENVIRONMENT_API}/explorer/grants` && method === "POST") {
      await json(route, grantFixture(), 201);
    } else if (path.startsWith(`${ENVIRONMENT_API}/explorer/grants/`) && method === "DELETE") {
      await json(route, { grantId: grantFixture().grantId, revokedAtUnixSeconds: 1_786_579_201 });
    } else if (path === `${ENVIRONMENT_API}/data-jobs` && method === "GET") {
      await json(route, { items: [exportJob(), failedImportJob()], nextCursor: null });
    } else if (path.startsWith(`${ENVIRONMENT_API}/data-jobs/`) && method === "GET") {
      const jobId = path.split("/").at(-1) ?? "";
      this.jobDetailRequests.push(jobId);
      const job = [exportJob(), failedImportJob()].find((item) => item.jobId === jobId);
      if (job === undefined) {
        await json(route, apiError("not_found", "No such data job."), 404);
      } else {
        await json(route, job);
      }
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("internal", "Unhandled surfaced-capability test route."), 500);
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

function membership(developerIdentityId: string, role: string) {
  return { teamId: TEAM_ID, developerIdentityId, role, createdAt: NOW, updatedAt: NOW };
}

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
        quantity: 120 * 1024 * 1024,
        included: 500 * 1024 * 1024,
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

function navigationFixture() {
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
  ].map((id) => ({ id, label: id, path: `${ENVIRONMENT_PATH}/${id}`, permitted: true }));
}

function retention() {
  return {
    retainedFrom: "2026-07-07T12:00:00.000Z",
    observedAt: NOW,
    retentionSeconds: 2_592_000,
  };
}

function emptyPage() {
  return { items: [], nextCursor: null, retention: retention() };
}

function logRecord(
  timestamp: string,
  source: string,
  level: string,
  message: string,
  correlationId: string,
) {
  return {
    timestamp,
    payload: { kind: "project_log", source, level, message, correlationId },
  };
}

// Page one is deliberately out of order so the console's ordering is what the
// test observes; page two is older and closes the window.
function logsPage(cursor: string | null) {
  if (cursor === "logs-page-2") {
    return {
      items: [
        logRecord("2026-08-06T11:59:59.000Z", "data-plane", "debug", "opened snapshot", "c_04"),
        logRecord("2026-08-06T11:59:58.000Z", "sync", "info", "pull served 12 documents", "c_05"),
      ],
      nextCursor: null,
      retention: retention(),
    };
  }
  return {
    items: [
      logRecord(
        "2026-08-06T12:00:02.000Z",
        "edge-functions",
        "info",
        "function hello-world completed in 3 ms",
        "c_02",
      ),
      logRecord(
        "2026-08-06T12:00:03.000Z",
        "data-plane",
        "error",
        "replication push rejected: policy denied <b>bold</b>",
        "c_03",
      ),
      logRecord("2026-08-06T12:00:01.000Z", "sync", "warn", "checkpoint expired for c_01", "c_01"),
    ],
    nextCursor: "logs-page-2",
    retention: retention(),
  };
}

function indexRecord(
  timestamp: string,
  indexName: string,
  indexVersion: number,
  state: string,
  progressPercent: number,
  message: string | null,
) {
  return {
    timestamp,
    payload: {
      kind: "index_state",
      collectionId: "todos",
      indexName,
      indexVersion,
      state,
      progressPercent,
      message,
    },
  };
}

function indexStatesPage() {
  return {
    items: [
      indexRecord("2026-08-06T12:00:00.000Z", "by_owner", 1, "building", 40, null),
      indexRecord(
        "2026-08-06T12:00:03.000Z",
        "by_created",
        2,
        "failed",
        0,
        "unsupported field type",
      ),
      indexRecord("2026-08-06T12:00:05.000Z", "by_owner", 1, "ready", 100, null),
    ],
    nextCursor: null,
    retention: retention(),
  };
}

function collectionFixture() {
  return {
    id: "todos",
    metadataVersion: 1,
    schemaVersion: 1,
    jsonSchema: {
      type: "object",
      properties: { id: { type: "string" }, ownerId: { type: "string" } },
      required: ["id", "ownerId"],
    },
    primaryKey: { kind: "path", path: "id" },
    compatibility: "compatible",
    state: "active",
  };
}

function grantFixture() {
  return {
    grantId: "xgr_0123456789abcdef0123456789abcdef",
    capability: "mx1_sensitive_explorer_capability_never_persisted",
    mode: "policy_preview",
    operations: ["get", "browse", "query", "plan", "simulate"],
    applicationUserId: "usr_abcdefgh",
    issuedAtUnixSeconds: 1_786_579_200,
    expiresAtUnixSeconds: 4_102_444_800,
    authorizationEpoch: 1,
  };
}

function dataJob<T extends { readonly jobId: string }>(input: T) {
  return {
    tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
    collectionId: "todos",
    creatorId: "dev_abcdefgh",
    errors: [],
    createdAtUnixSeconds: 1_786_579_200,
    updatedAtUnixSeconds: 1_786_579_260,
    expiresAtUnixSeconds: 4_102_444_800,
    ...input,
  };
}

function exportJob() {
  return dataJob({
    jobId: "djob_export01",
    kind: "export",
    state: "succeeded",
    conflictStrategy: null,
    progress: { processed: 2, committed: 0, failed: 0, skipped: 0, exported: 2, bytes: 64 },
    manifest: {
      formatVersion: 1,
      tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
      collectionId: "todos",
      schemaVersion: 1,
      snapshot: "snapshot-export",
      rowCount: 2,
      byteCount: 64,
      digest: "sha256:export_manifest",
      finalizedAtUnixSeconds: 1_786_579_200,
    },
  });
}

function failedImportJob() {
  return dataJob({
    jobId: "djob_import02",
    kind: "import",
    state: "failed",
    conflictStrategy: "create_only",
    progress: { processed: 3, committed: 2, failed: 1, skipped: 0, exported: 0, bytes: 96 },
    errors: ["row 3: schema validation failed: ownerId is required"],
    manifest: null,
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
