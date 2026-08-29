import { expect, test, type Page, type Route } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const LATER = "2026-08-07T02:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const FUNCTION_NAME = "nightly-report";
const FUNCTION_PATH = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/functions/${FUNCTION_NAME}`;
const SCHEDULES_PATH = `${FUNCTION_PATH}/schedules`;
const FUNCTION_URL = `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/functions/${FUNCTION_NAME}`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const ACTIVE = "sch_nightly000001";
const PAUSED = "sch_weekly0000001";
const CREATED = "sch_created000001";
const MANUAL_RUN = "run_manual0000001";
const CRON_REFUSAL = "cron must have five fields: minute hour day-of-month month day-of-week";

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

// Schedules live on the function's page. The API is mocked at the wire:
// every assertion is about what the section shows for a response, or what
// it sends for an action -- the exact create body, a pause as one patch, a
// run-now as one post, and the history paged from the API newest first.
test("the function page lists each schedule with its state, next run, and last run", async ({
  page,
}) => {
  const api = new ScheduleApiHarness();
  await api.install(page);

  await page.goto(FUNCTION_URL);
  await expect(page.getByRole("heading", { name: FUNCTION_NAME, exact: true })).toBeVisible();
  const panel = page.getByRole("region", { name: "Schedules", exact: true });
  await expect(panel.getByRole("heading", { name: "Schedules", exact: true })).toBeVisible();
  await expect(panel).toContainText("five-field cron evaluated in UTC");

  const active = panel.locator(`tr[data-schedule-id="${ACTIVE}"]`);
  await expect(active).toContainText("nightly");
  await expect(active.locator(".schedule-cron")).toHaveText("0 2 * * *");
  await expect(active).toContainText("POST /reports/nightly");
  await expect(active.locator(".schedule-state")).toHaveText("active");
  await expect(active.locator(".schedule-next-run time")).toHaveAttribute("datetime", LATER);
  await expect(active.locator(".schedule-last-run .run-outcome")).toHaveText("succeeded");
  await expect(active.locator(".schedule-last-run")).toContainText("HTTP 200 · 812 ms");
  await expect(active.locator(".schedule-last-run time")).toHaveAttribute("datetime", NOW);

  const paused = panel.locator(`tr[data-schedule-id="${PAUSED}"]`);
  await expect(paused).toContainText("weekly");
  await expect(paused.locator(".schedule-state")).toHaveText("paused");
  await expect(paused.locator(".schedule-next-run")).toContainText("none while paused");
  await expect(paused.locator(".schedule-next-run time")).toHaveCount(0);
  await expect(paused.locator(".schedule-last-run .run-outcome")).toHaveText("error");
  await expect(paused.locator(".schedule-last-run")).not.toContainText("HTTP");
  await expect(paused.getByRole("button", { name: "Resume weekly" })).toBeVisible();
  await expect(paused.getByRole("button", { name: "Pause weekly" })).toHaveCount(0);

  // The form explains the syntax before anything is typed.
  const form = page.getByRole("region", { name: "Add schedule" });
  await expect(form.getByLabel("Cron expression")).toHaveAccessibleDescription(
    /Five fields in UTC — minute, hour, day of month, month, day of week\. Examples: 0 2 \* \* \* every night at 02:00/u,
  );
  expect(api.unhandled).toEqual([]);
});

test("creating a schedule sends the exact request with an idempotency key and lists it", async ({
  page,
}) => {
  const api = new ScheduleApiHarness();
  await api.install(page);

  await page.goto(FUNCTION_URL);
  const panel = page.getByRole("region", { name: "Schedules", exact: true });
  await expect(panel.locator(`tr[data-schedule-id="${ACTIVE}"]`)).toBeVisible();

  const form = page.getByRole("region", { name: "Add schedule" });
  await form.getByLabel("Name", { exact: true }).fill("hourly sync");
  await form.getByLabel("Cron expression").fill("0 * * * *");
  await form.getByLabel("Method").selectOption("PUT");
  await form.getByLabel("Path", { exact: true }).fill("/sync?full=false");
  await form.getByLabel("Content type").fill("text/plain");
  await form
    .getByLabel("Headers (one name=value per line)")
    .fill("x-source=console\n\nx-mode = full\n");
  await form.getByLabel("Body (sent as text; omitted for GET)").fill("sync now");
  await form.getByRole("button", { name: "Create schedule" }).click();

  await expect(page.getByRole("status")).toContainText(`Schedule ${CREATED} created; next run`);
  const creates = api.requests.filter(
    (request) => request.method === "POST" && request.path === SCHEDULES_PATH,
  );
  expect(creates).toHaveLength(1);
  expect(creates[0]?.body).toEqual({
    cron: "0 * * * *",
    name: "hourly sync",
    request: {
      method: "PUT",
      path: "/sync?full=false",
      contentType: "text/plain",
      headers: { "x-source": "console", "x-mode": "full" },
      body: "sync now",
    },
    enabled: true,
  });
  expect(creates[0]?.headers["idempotency-key"]).toMatch(UUID);

  // The new schedule is listed from the API and the form is ready for the next one.
  const row = panel.locator(`tr[data-schedule-id="${CREATED}"]`);
  await expect(row).toContainText("hourly sync");
  await expect(row.locator(".schedule-cron")).toHaveText("0 * * * *");
  await expect(row).toContainText("PUT /sync?full=false");
  await expect(row.locator(".schedule-state")).toHaveText("active");
  await expect(row.locator(".schedule-last-run")).toContainText("never");
  await expect(form.getByLabel("Cron expression")).toHaveValue("");
  await expect(form.getByLabel("Name", { exact: true })).toHaveValue("");
  await expect(form.getByLabel("Method")).toHaveValue("POST");
  await expect(form.getByLabel("Path", { exact: true })).toHaveValue("/");

  // A GET carries no body; an unticked Enabled saves the schedule paused.
  await form.getByLabel("Cron expression").fill("30 3 * * *");
  await form.getByLabel("Method").selectOption("GET");
  await form.getByLabel("Body (sent as text; omitted for GET)").fill("ignored for GET");
  await form.getByLabel("Enabled — start running at the next due time").uncheck();
  await form.getByRole("button", { name: "Create schedule" }).click();
  await expect(page.getByRole("status")).toHaveText("Schedule sch_created000002 created, paused.");
  const second = api.requests.filter(
    (request) => request.method === "POST" && request.path === SCHEDULES_PATH,
  )[1];
  expect(second?.body).toEqual({
    cron: "30 3 * * *",
    request: { method: "GET", path: "/", contentType: "application/json" },
    enabled: false,
  });
  await expect(
    panel.locator('tr[data-schedule-id="sch_created000002"] .schedule-state'),
  ).toHaveText("paused");
  expect(api.unhandled).toEqual([]);
});

test("a malformed or refused schedule is shown inline and creates nothing", async ({ page }) => {
  const api = new ScheduleApiHarness();
  await api.install(page);

  await page.goto(FUNCTION_URL);
  const panel = page.getByRole("region", { name: "Schedules", exact: true });
  const form = page.getByRole("region", { name: "Add schedule" });
  await expect(panel.locator(`tr[data-schedule-id="${ACTIVE}"]`)).toBeVisible();

  // A header line without a value never leaves the browser.
  await form.getByLabel("Cron expression").fill("0 2 * * *");
  await form.getByLabel("Headers (one name=value per line)").fill("x-source");
  await form.getByRole("button", { name: "Create schedule" }).click();
  await expect(page.getByRole("alert")).toContainText(
    'Header lines must be name=value; "x-source" is not.',
  );
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(0);

  // Nor does a header the platform sets itself.
  await form.getByLabel("Headers (one name=value per line)").fill("Authorization=Bearer x");
  await form.getByRole("button", { name: "Create schedule" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "The Authorization header is set by the platform",
  );
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(0);

  // An expression the API refuses is shown with its message, verbatim.
  await form.getByLabel("Headers (one name=value per line)").fill("");
  await form.getByLabel("Cron expression").fill("every night");
  await form.getByRole("button", { name: "Create schedule" }).click();
  await expect(page.getByRole("alert")).toContainText(CRON_REFUSAL);
  await expect(page.getByRole("alert")).toContainText("req_e2e");
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(1);
  await expect(page.getByRole("status")).toHaveCount(0);
  await expect(form.getByLabel("Cron expression")).toHaveValue("every night");
  await expect(panel.locator("tr[data-schedule-id]")).toHaveCount(2);
  expect(api.schedules.map((schedule) => schedule.id)).toEqual([ACTIVE, PAUSED]);
  expect(api.unhandled).toEqual([]);
});

test("pausing patches enabled false, resuming enabled true, run now posts the action, and delete is confirmed", async ({
  page,
}) => {
  const api = new ScheduleApiHarness();
  await api.install(page);
  const dialogs: string[] = [];
  const decisions: boolean[] = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    void (decisions.shift() === true ? dialog.accept() : dialog.dismiss());
  });

  await page.goto(FUNCTION_URL);
  const panel = page.getByRole("region", { name: "Schedules", exact: true });
  const active = panel.locator(`tr[data-schedule-id="${ACTIVE}"]`);
  await expect(active.locator(".schedule-state")).toHaveText("active");

  await active.getByRole("button", { name: "Pause nightly" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Schedule nightly paused; it keeps its history and runs again once resumed.",
  );
  await expect(active.locator(".schedule-state")).toHaveText("paused");
  await expect(active.locator(".schedule-next-run")).toContainText("none while paused");
  const patches = api.requests.filter((request) => request.method === "PATCH");
  expect(patches.map((request) => request.path)).toEqual([`${SCHEDULES_PATH}/${ACTIVE}`]);
  expect(patches[0]?.body).toEqual({ enabled: false });
  expect(patches[0]?.headers["idempotency-key"]).toMatch(UUID);

  await active.getByRole("button", { name: "Resume nightly" }).click();
  await expect(page.getByRole("status")).toContainText("Schedule nightly resumed; next run");
  await expect(active.locator(".schedule-state")).toHaveText("active");
  await expect(active.locator(".schedule-next-run time")).toHaveAttribute("datetime", LATER);
  expect(api.requests.filter((request) => request.method === "PATCH")[1]?.body).toEqual({
    enabled: true,
  });

  await active.getByRole("button", { name: "Run nightly now" }).click();
  await expect(page.getByRole("status")).toHaveText(
    `Run ${MANUAL_RUN} queued for schedule nightly; it is recorded as manual.`,
  );
  const runNows = api.requests.filter((request) => request.path.endsWith("/actions/run-now"));
  expect(runNows.map((request) => [request.method, request.path])).toEqual([
    ["POST", `${SCHEDULES_PATH}/${ACTIVE}/actions/run-now`],
  ]);
  expect(runNows[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(runNows[0]?.body).toBeNull();

  // A refused run-now (one is still executing) is shown, and nothing changes.
  api.runNowRefusal = "a run of this schedule is still executing";
  await active.getByRole("button", { name: "Run nightly now" }).click();
  await expect(page.getByRole("alert")).toContainText("a run of this schedule is still executing");
  api.runNowRefusal = null;

  // Dismissing the delete confirmation sends nothing.
  decisions.push(false);
  await active.getByRole("button", { name: "Delete nightly" }).click();
  expect(dialogs).toHaveLength(1);
  expect(dialogs[0]).toContain(`Delete schedule nightly (${ACTIVE})?`);
  expect(dialogs[0]).toContain("Its run history is removed with it");
  expect(dialogs[0]).toContain("This action will be audited.");
  expect(api.requests.filter((request) => request.method === "DELETE")).toHaveLength(0);
  await expect(active).toBeVisible();

  // Confirming removes the schedule from the list.
  decisions.push(true);
  await active.getByRole("button", { name: "Delete nightly" }).click();
  await expect(page.getByRole("status")).toHaveText("Schedule nightly deleted.");
  await expect(active).toHaveCount(0);
  await expect(panel.locator(`tr[data-schedule-id="${PAUSED}"]`)).toBeVisible();
  const deletes = api.requests.filter((request) => request.method === "DELETE");
  expect(deletes.map((request) => request.path)).toEqual([`${SCHEDULES_PATH}/${ACTIVE}`]);
  expect(deletes[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(api.schedules.map((schedule) => schedule.id)).toEqual([PAUSED]);
  expect(api.unhandled).toEqual([]);
});

test("a schedule's history opens beneath it newest first, filters by outcome, loads more, and shows a manual run", async ({
  page,
}) => {
  const api = new ScheduleApiHarness();
  await api.install(page);

  await page.goto(FUNCTION_URL);
  const panel = page.getByRole("region", { name: "Schedules", exact: true });
  const active = panel.locator(`tr[data-schedule-id="${ACTIVE}"]`);
  await expect(page.getByRole("region", { name: "Run history of nightly" })).toHaveCount(0);

  await active.getByRole("button", { name: "Show history of nightly" }).click();
  const history = page.getByRole("region", { name: "Run history of nightly" });
  await expect(history.locator("tbody tr")).toHaveCount(2);
  const newest = history.locator("tbody tr").first();
  await expect(newest).toHaveAttribute("data-run-id", "run_nightly000005");
  await expect(newest.locator("th time")).toHaveAttribute("datetime", "2026-08-06T05:00:00.000Z");
  await expect(newest).toContainText("v3");
  await expect(newest.locator(".run-outcome")).toHaveText("succeeded");
  await expect(newest).toContainText("812 ms");
  await expect(newest.locator("td.numeric").nth(1)).toHaveText("200");
  await expect(newest).toContainText("cron");
  const failed = history.locator('tr[data-run-id="run_nightly000004"]');
  await expect(failed.locator(".run-outcome")).toHaveText("failed");
  await expect(failed.locator("td.numeric").nth(1)).toHaveText("500");

  await history.getByRole("button", { name: "Load more" }).click();
  await expect(history.locator("tbody tr")).toHaveCount(4);
  const skipped = history.locator('tr[data-run-id="run_nightly000003"]');
  await expect(skipped.locator(".run-outcome")).toHaveText("skipped (overlap)");
  await expect(skipped).toContainText("not started");
  await expect(skipped).toContainText("previous_run_still_executing");
  const errored = history.locator('tr[data-run-id="run_nightly000002"]');
  await expect(errored.locator(".run-outcome")).toHaveText("error");
  await expect(errored).toContainText("timeout");
  await history.getByRole("button", { name: "Load more" }).click();
  await expect(history.locator("tbody tr")).toHaveCount(5);
  await expect(history.locator('tr[data-run-id="run_nightly000001"]')).toContainText("manual");
  await expect(history.getByRole("button", { name: "Load more" })).toBeDisabled();
  await expect(history.getByText("Every retained run is listed.")).toBeVisible();

  // The outcome filter narrows the listing at the API.
  await history.getByLabel("Outcome").selectOption("failed");
  await expect(history.locator("tbody tr")).toHaveCount(1);
  await expect(history.locator("tbody tr").first()).toHaveAttribute(
    "data-run-id",
    "run_nightly000004",
  );
  await history.getByLabel("Outcome").selectOption("skipped_overlap");
  await expect(history.locator("tbody tr")).toHaveCount(1);
  await history.getByLabel("Outcome").selectOption("");
  await expect(history.locator("tbody tr")).toHaveCount(2);

  // The dev server mounts under StrictMode, which runs the listing effect
  // twice on mount; the distinct requests are what the section asked for.
  const listings = distinct(
    api.requests
      .filter((request) => request.method === "GET" && request.path.endsWith("/runs"))
      .map((request) => ({
        outcome: request.query.get("outcome"),
        cursor: request.query.get("cursor"),
        limit: request.query.get("limit"),
      })),
  );
  expect(listings).toEqual([
    { outcome: null, cursor: null, limit: "50" },
    { outcome: null, cursor: "after-2", limit: "50" },
    { outcome: null, cursor: "after-4", limit: "50" },
    { outcome: "failed", cursor: null, limit: "50" },
    { outcome: "skipped_overlap", cursor: null, limit: "50" },
  ]);

  // Run now queues a manual run, which the open history shows on top.
  await active.getByRole("button", { name: "Run nightly now" }).click();
  await expect(page.getByRole("status")).toContainText(`Run ${MANUAL_RUN} queued`);
  const queued = history.locator("tbody tr").first();
  await expect(queued).toHaveAttribute("data-run-id", MANUAL_RUN);
  await expect(queued.locator(".run-outcome")).toHaveText("queued");
  await expect(queued).toContainText("manual");

  await active.getByRole("button", { name: "Hide history of nightly" }).click();
  await expect(history).toHaveCount(0);
  expect(api.unhandled).toEqual([]);
});

interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly query: URLSearchParams;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

type Outcome = "succeeded" | "failed" | "error" | "skipped_overlap";

interface RequestFixture {
  method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  path: string;
  headers?: Record<string, string>;
  contentType: string;
  body?: string;
}

interface RunSummaryFixture {
  id: string;
  dueAt: string;
  outcome: Outcome;
  durationMilliseconds: number | null;
  responseStatus: number | null;
}

interface ScheduleFixture {
  id: string;
  functionName: string;
  name: string;
  cron: string;
  timezone: "UTC";
  request: RequestFixture;
  enabled: boolean;
  state: "active" | "paused";
  nextRunAt: string | null;
  lastRun: RunSummaryFixture | null;
  createdAt: string;
  updatedAt: string;
}

interface RunFixture {
  id: string;
  scheduleId: string;
  functionName: string;
  functionVersion: number | null;
  dueAt: string;
  startedAt: string | null;
  completedAt: string | null;
  durationMilliseconds: number | null;
  outcome: Outcome | null;
  responseStatus: number | null;
  error: string | null;
  manual: boolean;
  createdAt: string;
}

class ScheduleApiHarness {
  readonly unhandled: string[] = [];
  readonly requests: RecordedRequest[] = [];
  schedules: ScheduleFixture[] = [
    scheduleFixture({
      id: ACTIVE,
      name: "nightly",
      cron: "0 2 * * *",
      request: { method: "POST", path: "/reports/nightly", contentType: "application/json" },
      nextRunAt: LATER,
      lastRun: {
        id: "run_nightly000005",
        dueAt: NOW,
        outcome: "succeeded",
        durationMilliseconds: 812,
        responseStatus: 200,
      },
    }),
    scheduleFixture({
      id: PAUSED,
      name: "weekly",
      cron: "0 6 * * 1",
      enabled: false,
      state: "paused",
      nextRunAt: null,
      lastRun: {
        id: "run_weekly00000001",
        dueAt: NOW,
        outcome: "error",
        durationMilliseconds: null,
        responseStatus: null,
      },
    }),
  ];
  runs: Record<string, RunFixture[]> = {
    // Newest first, as the API lists them.
    [ACTIVE]: [
      runFixture(5, "succeeded", { durationMilliseconds: 812, responseStatus: 200 }),
      runFixture(4, "failed", { durationMilliseconds: 40, responseStatus: 500 }),
      runFixture(3, "skipped_overlap", {
        startedAt: null,
        completedAt: null,
        durationMilliseconds: null,
        responseStatus: null,
        error: "previous_run_still_executing",
      }),
      runFixture(2, "error", {
        durationMilliseconds: 1_000,
        responseStatus: null,
        error: "timeout",
      }),
      runFixture(1, "succeeded", { durationMilliseconds: 640, responseStatus: 200, manual: true }),
    ],
    [PAUSED]: [],
  };
  runNowRefusal: string | null = null;
  readonly pageSize = 2;
  private created = 0;

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
      query: url.searchParams,
      headers: request.headers(),
      body: body === null ? null : (JSON.parse(body) as unknown),
    });

    if (path === `/v1/projects/${PROJECT_ID}` && method === "GET") {
      await json(route, project());
    } else if (path === `/v1/projects/${PROJECT_ID}/environments` && method === "GET") {
      await json(route, { items: [environment()] });
    } else if (path.endsWith("/workspace/navigation") && method === "GET") {
      await json(route, navigation());
    } else if (path === FUNCTION_PATH && method === "GET") {
      await json(route, edgeFunction());
    } else if (path === `${FUNCTION_PATH}/versions` && method === "GET") {
      await json(route, { items: [] });
    } else if (path === `${FUNCTION_PATH}/logs` && method === "GET") {
      await json(route, { items: [], nextCursor: null });
    } else if (path.includes("/observability/") && method === "GET") {
      await json(route, {
        items: [],
        nextCursor: null,
        retention: {
          retainedFrom: "2026-07-07T12:00:00.000Z",
          observedAt: NOW,
          retentionSeconds: 2_592_000,
        },
      });
    } else if (path === SCHEDULES_PATH && method === "GET") {
      await json(route, { items: this.schedules });
    } else if (path === SCHEDULES_PATH && method === "POST") {
      await this.create(route);
    } else if (path.startsWith(`${SCHEDULES_PATH}/`)) {
      await this.handleSchedule(route, path.slice(SCHEDULES_PATH.length + 1), method, url);
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${path}`), 404);
    }
  }

  private async create(route: Route) {
    if (route.request().headers()["idempotency-key"] === undefined) {
      await json(route, apiError("invalid_request", "Idempotency-Key is required"), 400);
      return;
    }
    const input = route.request().postDataJSON() as {
      cron: string;
      name?: string;
      request?: RequestFixture;
      enabled?: boolean;
    };
    if (input.cron.trim().split(/\s+/u).length !== 5) {
      await json(route, apiError("invalid_request", CRON_REFUSAL), 400);
      return;
    }
    this.created += 1;
    const enabled = input.enabled ?? true;
    const schedule = scheduleFixture({
      id: `sch_created00000${this.created}`,
      name: input.name ?? FUNCTION_NAME,
      cron: input.cron,
      request: input.request ?? { method: "POST", path: "/", contentType: "application/json" },
      enabled,
      state: enabled ? "active" : "paused",
      nextRunAt: enabled ? LATER : null,
      lastRun: null,
      createdAt: LATER,
      updatedAt: LATER,
    });
    this.schedules.push(schedule);
    this.runs[schedule.id] = [];
    await json(route, schedule, 201);
  }

  private async handleSchedule(route: Route, rest: string, method: string, url: URL) {
    const [scheduleId, ...tail] = rest.split("/");
    const schedule = this.schedules.find((item) => item.id === scheduleId);
    if (scheduleId === undefined || schedule === undefined) {
      await json(route, apiError("not_found", "schedule not found"), 404);
      return;
    }
    const log = this.runs[schedule.id] ?? [];
    const idempotent = route.request().headers()["idempotency-key"] !== undefined;
    if (tail.length === 0 && method === "GET") {
      await json(route, schedule);
    } else if (tail.length === 0 && method === "PATCH" && idempotent) {
      const patch = route.request().postDataJSON() as Partial<ScheduleFixture>;
      Object.assign(schedule, patch, { updatedAt: LATER });
      if (patch.enabled !== undefined) {
        schedule.state = patch.enabled ? "active" : "paused";
        schedule.nextRunAt = patch.enabled ? LATER : null;
      }
      await json(route, schedule);
    } else if (tail.length === 0 && method === "DELETE" && idempotent) {
      this.schedules = this.schedules.filter((item) => item.id !== schedule.id);
      delete this.runs[schedule.id];
      await route.fulfill({ status: 204 });
    } else if (tail.join("/") === "actions/run-now" && method === "POST" && idempotent) {
      if (this.runNowRefusal !== null) {
        await json(route, apiError("conflict", this.runNowRefusal), 409);
        return;
      }
      const queued: RunFixture = {
        id: MANUAL_RUN,
        scheduleId: schedule.id,
        functionName: FUNCTION_NAME,
        functionVersion: null,
        dueAt: LATER,
        startedAt: null,
        completedAt: null,
        durationMilliseconds: null,
        outcome: null,
        responseStatus: null,
        error: null,
        manual: true,
        createdAt: LATER,
      };
      log.unshift(queued);
      await json(route, queued, 202);
    } else if (tail[0] === "runs" && tail.length === 1 && method === "GET") {
      const outcome = url.searchParams.get("outcome");
      const cursor = url.searchParams.get("cursor");
      const offset = cursor === null ? 0 : Number(cursor.replace("after-", ""));
      const matching = log.filter((run) => outcome === null || run.outcome === outcome);
      const items = matching.slice(offset, offset + this.pageSize);
      const end = offset + items.length;
      await json(route, { items, nextCursor: end < matching.length ? `after-${end}` : null });
    } else {
      this.unhandled.push(`${method} ${url.pathname}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${url.pathname}`), 404);
    }
  }
}

function scheduleFixture(
  overrides: Partial<ScheduleFixture> & { id: string; name: string; cron: string },
): ScheduleFixture {
  return {
    functionName: FUNCTION_NAME,
    timezone: "UTC",
    request: { method: "POST", path: "/", contentType: "application/json" },
    enabled: true,
    state: "active",
    nextRunAt: LATER,
    lastRun: null,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function runFixture(
  sequence: number,
  outcome: Outcome,
  overrides: Partial<RunFixture>,
): RunFixture {
  const dueAt = `2026-08-06T0${sequence}:00:00.000Z`;
  return {
    id: `run_nightly00000${sequence}`,
    scheduleId: ACTIVE,
    functionName: FUNCTION_NAME,
    functionVersion: 3,
    dueAt,
    startedAt: dueAt,
    completedAt: `2026-08-06T0${sequence}:00:01.000Z`,
    durationMilliseconds: 500,
    outcome,
    responseStatus: 200,
    error: null,
    manual: false,
    createdAt: dueAt,
    ...overrides,
  };
}

function edgeFunction() {
  return {
    name: FUNCTION_NAME,
    state: "active",
    activeVersion: 3,
    configuration: {
      verifyJwt: true,
      regions: ["local"],
      secretNames: [],
      limits: {
        cpuMilliseconds: 100,
        wallMilliseconds: 1_000,
        memoryBytes: 134_217_728,
        requestBytes: 1_048_576,
        responseBytes: 1_048_576,
        concurrency: 10,
      },
    },
    createdAt: NOW,
    updatedAt: NOW,
  };
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
  return ["overview", "collections", "functions"].map((id) => ({
    id,
    label: id,
    path: `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/${id}`,
    permitted: true,
  }));
}

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
