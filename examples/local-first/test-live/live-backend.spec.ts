import { readFileSync } from "node:fs";

import { expect, type Page, test } from "@playwright/test";

import { type LiveTenant, tenantFile } from "./global-setup.js";

/**
 * The same six scenarios the default suite runs against the in-browser fake,
 * driven here against a real data plane over HTTP. Nothing is intercepted: the
 * app signs in for real, its writes are persisted by the server, and the
 * "remote" edits come from a second authenticated application user.
 */
const tenant = JSON.parse(readFileSync(tenantFile, "utf8")) as LiveTenant;

interface Todo {
  id: string;
  ownerId: string;
  title: string;
  updatedAt: number;
}

const todo = (id: string, title: string, updatedAt: number): Todo => ({
  id,
  ownerId: "user-example",
  title,
  updatedAt,
});

/**
 * The server keeps every document a test writes, so ids are unique per test and
 * assertions look documents up by id rather than by position.
 */
let scope = 0;
function nextScope(): string {
  scope += 1;
  return `s${scope}-${Date.now().toString(36)}`;
}

async function openApplication(page: Page): Promise<void> {
  await page.addInitScript((options) => {
    window.__MAKO_EXAMPLE__ = options;
  }, liveOptions());
  await page.goto(tenant.appUrl);
  await page.waitForFunction(() => window.makoExample !== undefined, undefined, {
    timeout: 30_000,
  });
}

function liveOptions() {
  return {
    // Same-origin: the dev server proxies /v1 to the data plane, matching how a
    // deployment fronts both behind one reverse proxy.
    endpoint: tenant.appUrl,
    projectId: tenant.projectId,
    environmentId: tenant.environmentId,
    collectionId: tenant.collectionId,
    publicProjectKey: tenant.publicProjectKey,
    email: "reference-app@local.test",
    password: "ReferenceApp1!",
    remoteEmail: "reference-remote@local.test",
    remotePassword: "ReferenceRemote1!",
    schemaVersion: 1,
  };
}

const titleOf = (page: Page, id: string) =>
  page.evaluate(
    async (documentId) =>
      (await window.makoExample.listTodos()).find((item) => item.id === documentId)?.title,
    id,
  );

test.beforeEach(async ({ page }) => {
  await openApplication(page);
});

test("keeps offline writes locally and pushes them after reconnect", async ({ page }) => {
  const before = await page.evaluate(() => window.makoExample.diagnostics().acceptedWrites);
  await page.evaluate(() => window.makoExample.setOnline(false));
  await page.getByRole("textbox", { name: "Todo" }).fill("written while offline");
  await page.getByRole("button", { name: "Add" }).click();

  await expect(page.locator('[data-testid="todo-todo-1"]')).toHaveText("written while offline");
  expect(await page.evaluate(() => window.makoExample.diagnostics().acceptedWrites)).toBe(before);

  await page.evaluate(() => window.makoExample.setOnline(true));
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBeGreaterThan(before);
});

test("resolves concurrent conflicts using the collection conflict handler", async ({ page }) => {
  const id = `conflict-${nextScope()}`;
  await page.evaluate(
    (document) => window.makoExample.putRemote(document),
    todo(id, "original", 100),
  );
  await expect.poll(() => titleOf(page, id)).toBe("original");

  // Edit locally while offline, and have a genuinely different client edit the
  // same document meanwhile. Both edits are real writes competing for the same
  // document; reconnecting is what forces the server to arbitrate.
  await page.evaluate(() => window.makoExample.setOnline(false));
  await page.evaluate(
    ([documentId]) => window.makoExample.updateTodo(documentId as string, "local edit", 200),
    [id],
  );
  await page.evaluate(
    (document) => window.makoExample.putRemote(document),
    todo(id, "newer remote edit", 300),
  );
  await page.evaluate(() => window.makoExample.setOnline(true));

  // The conflict handler keeps whichever edit is newer, so the remote wins.
  await expect.poll(() => titleOf(page, id), { timeout: 30_000 }).toBe("newer remote edit");
});

test("applies remote tombstones instead of resurrecting documents", async ({ page }) => {
  const id = `deleted-${nextScope()}`;
  await page.evaluate(
    (document) => window.makoExample.putRemote(document),
    todo(id, "temporary", 100),
  );
  await expect.poll(() => titleOf(page, id)).toBe("temporary");

  await page.evaluate(
    ([documentId]) => window.makoExample.removeRemote(documentId as string, 200),
    [id],
  );
  await expect.poll(() => titleOf(page, id), { timeout: 30_000 }).toBeUndefined();
});

test("refreshes expiring tokens before replication", async ({ page }) => {
  const before = await page.evaluate(() => window.makoExample.diagnostics().refreshes);
  await page.evaluate(() => window.makoExample.forceTokenRefresh());
  // A real refresh exchanges the refresh credential at /auth/token.
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().refreshes))
    .toBeGreaterThan(before);
});

test("reconnects the live stream and requests a resync", async ({ page }) => {
  const before = await page.evaluate(() => window.makoExample.diagnostics().streamConnections);
  await page.evaluate(() => window.makoExample.forceReconnect());
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().streamConnections), {
      timeout: 30_000,
    })
    .toBeGreaterThan(before);
});

test("clears local data and requires authentication after access revocation", async ({ page }) => {
  const id = `private-${nextScope()}`;
  await page.evaluate(
    (document) => window.makoExample.putRemote(document),
    todo(id, "private data", 100),
  );
  await expect.poll(() => titleOf(page, id)).toBe("private data");

  await page.evaluate(() => window.makoExample.revokeAccess());

  expect(await page.evaluate(() => window.makoExample.listTodos())).toEqual([]);
  expect(await page.evaluate(() => window.makoExample.diagnostics().activity)).toBe(
    "authentication_required",
  );
});
