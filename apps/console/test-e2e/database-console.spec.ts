import { resolve } from "node:path";
import { expect, type Page, type Route, test } from "@playwright/test";

const PROJECT = "prj_abcdefgh";
const ENV = "env_abcdefgh";
const NEXT = "env_ijklmnop";
const base = `/projects/${PROJECT}/environments/${ENV}`;
const capability = "mx1.xcap-v1.console_test_only_capability";
const current = () => Math.floor(Date.now() / 1000);
const section = (payload: unknown, status = "current") => ({
  status,
  payload,
  observedAtUnixSeconds: current(),
  freshUntilUnixSeconds: current() + 60,
});
const collection = (id = "tasks") => ({
  id,
  metadataVersion: 1,
  schemaVersion: 3,
  primaryKey: { kind: "field", field: "id" },
  compatibility: "compatible",
  state: "active",
  jsonSchema: {
    type: "object",
    properties: {
      title: { type: "string" },
      done: { type: "boolean" },
      priority: { type: "number" },
      notes: { type: ["string", "null"] },
      optional: { type: "string" },
    },
  },
});
const doc = (id = "task-001") => ({
  documentId: id,
  revision: "rev-001",
  schemaVersion: 3,
  deleted: false,
  content: {
    title: "Ship the database console",
    done: false,
    priority: 2,
    notes: null,
    markup: "<b>untrusted</b>",
  },
});
const summary = () => ({
  tenant: { projectId: PROJECT, environmentId: ENV },
  sections: {
    lifecycle: section({
      ready: true,
      environment: "active",
      project: "active",
      region: "us-east-1",
    }),
    collections: section({ count: 3, active: 3, limited: false }),
    functions: section({ count: 2 }),
    backups: section({ verifiedCount: 4 }),
    dataJobs: section({ count: 5, active: 1 }),
    sync: section({
      recordCount: 0,
      limited: false,
      windowStartUnixSeconds: current() - 3600,
      windowEndUnixSeconds: current(),
    }),
    usage: section({
      recordCount: 4,
      limited: false,
      windowStartUnixSeconds: current() - 3600,
      windowEndUnixSeconds: current(),
      samples: [
        {
          resource: "storage_bytes",
          quantity: 245760,
          unit: "bytes",
          timestampUnixMilliseconds: Date.now(),
        },
      ],
    }),
    activity: section({
      recordCount: 1,
      limited: false,
      windowStartUnixSeconds: current() - 3600,
      windowEndUnixSeconds: current(),
      events: [
        {
          timestampUnixMilliseconds: Date.now(),
          actorId: "dev_abcdefgh",
          action: "collection.create",
          target: "tasks",
          outcome: "allowed",
        },
      ],
    }),
  },
});

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}
async function auth(page: Page) {
  await page.addInitScript(() => {
    window.__MAKO_CONSOLE__ = {
      managementEndpoint: "http://127.0.0.1:4174",
      developerAuth: {
        loadSession: async () => ({
          accessToken: "developer-session-token",
          expiresAt: "2099-01-01T00:00:00Z",
          audience: "mako-management",
          profile: {
            id: "dev_abcdefgh",
            email: "developer@example.test",
            displayName: "Developer",
          },
        }),
        beginSignIn: async () => {},
        signOut: async () => {},
        subscribe: () => () => {},
      },
    };
  });
}
async function shell(route: Route, path: string) {
  if (path === `/v1/projects/${PROJECT}`) {
    await json(route, {
      id: PROJECT,
      teamId: "org_abcdefgh",
      name: "Atlas workspace",
      region: "us-east-1",
      state: "active",
      createdAt: "2026-09-01T00:00:00Z",
      updatedAt: "2026-09-01T00:00:00Z",
    });
    return true;
  }
  if (path.endsWith("/environments")) {
    await json(route, {
      items: [ENV, NEXT].map((id, i) => ({
        id,
        projectId: PROJECT,
        name: i === 0 ? "production" : "staging",
        state: "active",
        createdAt: "2026-09-01T00:00:00Z",
        updatedAt: "2026-09-01T00:00:00Z",
      })),
    });
    return true;
  }
  if (path.endsWith("/workspace/navigation")) {
    const env = path.includes(NEXT) ? NEXT : ENV;
    await json(
      route,
      [
        ["overview", "Overview"],
        ["data", "Data browser"],
        ["collections", "Collections"],
        ["sync", "Sync"],
        ["policies", "Policies"],
        ["users", "Users"],
        ["functions", "Functions"],
        ["observability", "Observability"],
        ["backups", "Backups"],
        ["connect", "API & Connect"],
        ["settings", "Settings"],
      ].map(([id, label]) => ({
        id,
        label,
        path: `/projects/${PROJECT}/environments/${env}/${id}`,
        permitted: true,
      })),
    );
    return true;
  }
  return false;
}

test.beforeEach(async ({ page }) => auth(page));

test("a grant issued after a collection switch is revoked without becoming active", async ({
  page,
}) => {
  let requested = false;
  let revoked = false;
  let release!: () => void;
  const pending = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (await shell(route, path)) return;
    if (path.endsWith("/collections"))
      return json(route, { items: [collection(), collection("members")] });
    if (path.endsWith("/users"))
      return json(route, {
        users: [{ id: "usr_abcdefgh", email: "app@example.test", status: "active" }],
        truncated: false,
      });
    if (path.endsWith("/explorer/grants")) {
      const isOld = route.request().postDataJSON().collectionId === "tasks";
      if (isOld) {
        requested = true;
        await pending;
      }
      return json(
        route,
        {
          grantId: isOld ? "xgr_abcdefgh" : "xgr_members",
          capability,
          mode: "administrative",
          operations: ["get", "browse"],
          issuedAtUnixSeconds: current(),
          expiresAtUnixSeconds: current() + 300,
          authorizationEpoch: 1,
        },
        201,
      );
    }
    if (path.endsWith("/explorer/grants/xgr_abcdefgh") && route.request().method() === "DELETE") {
      revoked = true;
      return json(route, {});
    }
    if (path.endsWith("/browse")) {
      expect(path).toContain("/collections/members/");
      return json(route, {
        items: [doc("member-current")],
        nextCursor: null,
        snapshot: "member-snapshot",
        exhausted: true,
      });
    }
    return json(route, {}, 500);
  });
  await page.goto(`${base}/data`);
  await expect.poll(() => requested).toBe(true);
  await page
    .getByRole("navigation", { name: "Browse collections" })
    .getByRole("button", { name: "members", exact: true })
    .click();
  release();
  await expect.poll(() => revoked).toBe(true);
  await expect(page.getByRole("region", { name: "Document results" })).toContainText(
    "member-current",
  );
  await expect(page.getByRole("button", { name: "Create access grant" })).toHaveCount(0);
});

for (const state of ["failed", "denied"] as const) {
  test(`environment context survives ${state} navigation without adding denied destinations`, async ({
    page,
  }) => {
    await page.route("**/v1/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/workspace/navigation")) {
        if (state === "failed") return json(route, {}, 503);
        return json(route, [
          { id: "overview", label: "Overview", path: `${base}/overview`, permitted: true },
          { id: "settings", label: "Settings", path: `${base}/settings`, permitted: true },
          { id: "credentials", label: "API keys", path: `${base}/credentials`, permitted: false },
          { id: "storage", label: "Storage", path: `${base}/storage`, permitted: false },
        ]);
      }
      if (await shell(route, path)) return;
      if (path.endsWith("/workspace/summary")) return json(route, summary());
      if (path.endsWith("/collections")) return json(route, { items: [] });
      return json(route, {}, 500);
    });
    await page.goto(`${base}/overview`);
    await expect(page.getByRole("button", { name: "Atlas workspace" })).toBeVisible();
    await expect(page.getByLabel("Switch environment")).toHaveValue(ENV);
    const navigation = page.getByRole("navigation", { name: "Environment destinations" });
    await expect(navigation.getByRole("link", { name: "API keys", exact: true })).toHaveCount(0);
    await expect(navigation.getByRole("link", { name: "Storage", exact: true })).toHaveCount(0);
    if (state === "failed") await expect(navigation.getByRole("link")).toHaveCount(0);
  });
}

test("mobile navigation opens the collection policy workspace", async ({ page }) => {
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (await shell(route, path)) return;
    if (path.endsWith("/workspace/summary")) return json(route, summary());
    if (path.endsWith("/collections")) return json(route, { items: [collection()] });
    return json(route, {}, 500);
  });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${base}/overview`);
  await page.getByLabel("Go to", { exact: true }).selectOption("policies");
  await expect(page).toHaveURL(`${base}/policies`);
  await page.getByRole("button", { name: "Manage policy" }).click();
  await expect(page).toHaveURL(`${base}/collections/tasks/policies`);
});

test("overview provides database actions and real metadata across desktop, dark, and mobile", async ({
  page,
}) => {
  const unhandled: string[] = [];
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (await shell(route, path)) return;
    if (path.endsWith("/workspace/summary")) return json(route, summary());
    if (path.endsWith("/collections"))
      return json(route, { items: [collection(), collection("members"), collection("projects")] });
    unhandled.push(path);
    return json(route, {}, 500);
  });
  await page.setViewportSize({ width: 1440, height: 1100 });
  await page.goto(`${base}/overview`);
  await expect(page.getByRole("heading", { name: "Database overview" })).toBeVisible();
  await expect(
    page.getByRole("article", { name: "Collections", exact: true }).getByText("3", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("region", { name: "Usage observations" })).toContainText("245,760");
  await expect(page.getByRole("region", { name: "Recent database activity" })).toContainText(
    "collection create",
  );
  await expect(
    page.getByRole("region", { name: "Collection inventory" }).getByRole("row"),
  ).toHaveCount(4);
  await page.screenshot({
    path: resolve(import.meta.dirname, "../../../.local/console-redesign/overview-desktop.png"),
    fullPage: true,
  });
  await page.getByTestId("theme-toggle").click();
  await page.screenshot({
    path: resolve(import.meta.dirname, "../../../.local/console-redesign/overview-dark.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
    true,
  );
  await page.screenshot({
    path: resolve(import.meta.dirname, "../../../.local/console-redesign/overview-mobile.png"),
    fullPage: true,
  });
  expect(unhandled).toEqual([]);
  await page.getByRole("button", { name: "Create collection" }).click();
  await expect(page).toHaveURL(`${base}/collections`);
});

test("empty inventory offers setup while failed telemetry stays unavailable", async ({ page }) => {
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (await shell(route, path)) return;
    if (path.endsWith("/workspace/summary"))
      return json(
        route,
        { code: "unavailable", message: "Telemetry temporarily unavailable" },
        503,
      );
    if (path.endsWith("/collections")) return json(route, { items: [] });
    return json(route, {}, 500);
  });
  await page.goto(`${base}/overview`);
  await expect(page.getByText("Create your first collection", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Define a collection" })).toBeVisible();
  await expect(page.getByRole("article", { name: "Collections", exact: true })).toContainText(
    "Unavailable",
  );
  await expect(page.getByRole("article", { name: "Environment", exact: true })).not.toContainText(
    "Active",
  );
});

test("expired observations turn stale and partial measurements are identified", async ({
  page,
}) => {
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (await shell(route, path)) return;
    if (path.endsWith("/workspace/summary")) {
      const data = summary();
      data.sections.collections.freshUntilUnixSeconds = current() - 1;
      data.sections.usage = section({ samples: [], limited: true });
      return json(route, data);
    }
    if (path.endsWith("/collections")) return json(route, { items: [collection()] });
    return json(route, {}, 500);
  });
  await page.goto(`${base}/overview`);
  await expect(
    page
      .getByRole("article", { name: "Collections", exact: true })
      .getByText("Stale", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("region", { name: "Usage observations" })).toContainText(
    "Partial results",
  );
});

test("a late overview response cannot replace the new environment", async ({ page }) => {
  let release!: () => void;
  const pending = new Promise<void>((resolve) => {
    release = resolve;
  });
  let requested = false;
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (await shell(route, path)) return;
    if (path.endsWith("/workspace/summary")) {
      if (!path.includes(NEXT)) {
        requested = true;
        await pending;
      }
      const data = summary();
      data.sections.collections = section({ count: path.includes(NEXT) ? 9 : 111 });
      return json(route, data);
    }
    if (path.endsWith("/collections"))
      return json(route, {
        items: [collection(path.includes(NEXT) ? "staging_only" : "production_only")],
      });
    return json(route, {}, 500);
  });
  await page.goto(`${base}/overview`);
  await expect.poll(() => requested).toBe(true);
  await page.getByLabel("Switch environment").selectOption(NEXT);
  await expect(
    page.getByRole("article", { name: "Collections", exact: true }).getByText("9", { exact: true }),
  ).toBeVisible();
  release();
  await expect(page.getByRole("link", { name: "staging_only", exact: true })).toBeVisible();
  await expect(page.getByText("production_only", { exact: true })).toHaveCount(0);
  await expect(page.getByText("111", { exact: true })).toHaveCount(0);
});

test("the workbench displays escaped document fields and discards a delayed read after a collection switch", async ({
  page,
}) => {
  let delayed = false;
  let requested = false;
  let release!: () => void;
  const pending = new Promise<void>((resolve) => {
    release = resolve;
  });
  const revoked: string[] = [];
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (await shell(route, path)) return;
    if (path.endsWith("/collections"))
      return json(route, { items: [collection(), collection("members")] });
    if (path.endsWith("/users"))
      return json(route, {
        users: [{ id: "usr_abcdefgh", email: "app@example.test", status: "active" }],
        truncated: false,
      });
    if (path.endsWith("/explorer/grants"))
      return json(
        route,
        {
          grantId: "xgr_abcdefgh",
          capability,
          mode: "administrative",
          operations: ["browse", "get", "plan", "query", "simulate"],
          applicationUserId: null,
          issuedAtUnixSeconds: current(),
          expiresAtUnixSeconds: current() + 300,
          authorizationEpoch: 1,
        },
        201,
      );
    if (path.includes("/explorer/grants/") && request.method() === "DELETE") {
      revoked.push(path);
      return json(route, {});
    }
    if (path.endsWith("/browse")) {
      if (path.includes("/collections/members/"))
        return json(route, { items: [], nextCursor: null, snapshot: "members", exhausted: true });
      if (delayed) {
        requested = true;
        await pending;
      }
      return json(route, {
        items: [doc(delayed ? "late-private-document" : "task-001")],
        nextCursor: null,
        snapshot: "snapshot-1",
        exhausted: true,
      });
    }
    if (path.endsWith("/data-jobs")) return json(route, { items: [], nextCursor: null });
    return json(route, {}, 500);
  });
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.goto(`${base}/data`);
  const results = page.getByRole("region", { name: "Document results" });
  await expect(results.getByRole("columnheader", { name: "title", exact: true })).toBeVisible();
  await expect(
    results.getByRole("cell", { name: "Ship the database console", exact: true }),
  ).toBeVisible();
  await expect(results.getByRole("cell", { name: "false", exact: true })).toBeVisible();
  await expect(results.getByRole("cell", { name: "null", exact: true })).toBeVisible();
  await expect(results.getByRole("cell", { name: "missing", exact: true })).toBeVisible();
  await expect(results.getByRole("cell", { name: "<b>untrusted</b>", exact: true })).toBeVisible();
  await expect(results.locator("b")).toHaveCount(0);
  await page.screenshot({
    path: resolve(import.meta.dirname, "../../../.local/console-redesign/data-desktop.png"),
    fullPage: true,
  });
  delayed = true;
  await page.getByRole("button", { name: "Refresh documents", exact: true }).click();
  await expect.poll(() => requested).toBe(true);
  await page
    .getByRole("navigation", { name: "Browse collections" })
    .getByRole("button", { name: "members", exact: true })
    .click();
  release();
  await expect(page.getByRole("button", { name: "Refresh documents" })).toBeEnabled();
  await expect(page.getByText("late-private-document")).toHaveCount(0);
  await expect(page.getByText("Ship the database console")).toHaveCount(0);
  await expect.poll(() => revoked.length).toBe(1);
  const stored = await page.evaluate(() =>
    JSON.stringify({
      local: Object.entries(localStorage),
      session: Object.entries(sessionStorage),
      url: location.href,
    }),
  );
  expect(stored).not.toContain(capability);
});
