import { expect, type Page, test } from "@playwright/test";

// `window.makoExample` is typed once for both suites in
// test-support/browser-globals.d.ts.
interface Todo {
  id: string;
  ownerId: string;
  title: string;
  updatedAt: number;
}

/** Every todo title on the page, in list order. */
const titles = (page: Page) => page.locator('[data-testid^="todo-"]');

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.makoExample !== undefined);
});

test("keeps offline writes locally and pushes them after reconnect", async ({ page }) => {
  await page.evaluate(() => window.makoExample.setOnline(false));
  await page.getByRole("textbox", { name: "Todo" }).fill("written while offline");
  await page.getByRole("button", { name: "Add" }).click();

  await expect(titles(page)).toHaveText(["written while offline"]);
  expect(await page.evaluate(() => window.makoExample.diagnostics().acceptedWrites)).toBe(0);

  await page.evaluate(() => window.makoExample.setOnline(true));
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBe(1);
});

test("edits and deletes a todo from the page and pushes each change", async ({ page }) => {
  await page.getByRole("textbox", { name: "Todo" }).fill("draft");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(titles(page)).toHaveText(["draft"]);
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBe(1);

  await page.getByRole("button", { name: "Edit draft" }).click();
  const editor = page.getByRole("textbox", { name: "Edit draft" });
  await editor.fill("final");
  await editor.press("Enter");
  await expect(titles(page)).toHaveText(["final"]);
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBe(2);

  await page.getByRole("button", { name: "Delete final" }).click();
  await expect(titles(page)).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => window.makoExample.listTodos())).toEqual([]);
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBe(3);
});

test("gives each new todo its own id, stamped with the signed-in user", async ({ page }) => {
  for (const value of ["first", "second"]) {
    await page.getByRole("textbox", { name: "Todo" }).fill(value);
    await page.getByRole("button", { name: "Add" }).click();
  }
  await expect(titles(page)).toHaveCount(2);
  const todos = await page.evaluate(() => window.makoExample.listTodos());
  expect(new Set(todos.map((item) => item.id)).size).toBe(2);
  for (const item of todos) {
    expect(item.id).toMatch(/^todo-[0-9a-f-]{36}$/u);
    expect(item.ownerId).toBe("user-example");
  }
});

test("resolves concurrent conflicts using the collection conflict handler", async ({ page }) => {
  const original = todo("conflict", "original", 100);
  await page.evaluate((document) => window.makoExample.putRemote(document), original);
  await expect
    .poll(() => page.evaluate(async () => (await window.makoExample.listTodos())[0]?.title))
    .toBe("original");
  await page.evaluate(() => window.makoExample.setOnline(false));
  await page.evaluate(() => window.makoExample.updateTodo("conflict", "local edit", 200));
  await page.evaluate(
    (document) => window.makoExample.putRemote(document),
    todo("conflict", "newer remote edit", 300),
  );
  await page.evaluate(() => window.makoExample.setOnline(true));

  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().conflicts))
    .toBeGreaterThan(0);
  await expect
    .poll(() => page.evaluate(async () => (await window.makoExample.listTodos())[0]?.title))
    .toBe("newer remote edit");
});

test("applies remote tombstones instead of resurrecting documents", async ({ page }) => {
  await page.evaluate(
    (document) => window.makoExample.putRemote(document),
    todo("deleted", "temporary", 100),
  );
  await expect
    .poll(() => page.evaluate(async () => (await window.makoExample.listTodos())[0]?.title))
    .toBe("temporary");
  await page.evaluate(() => window.makoExample.removeRemote("deleted", 200));
  await expect.poll(() => page.evaluate(() => window.makoExample.listTodos())).toEqual([]);
});

test("refreshes expiring tokens before replication", async ({ page }) => {
  const before = await page.evaluate(() => window.makoExample.diagnostics().refreshes);
  await page.evaluate(() => window.makoExample.forceTokenRefresh());
  expect(await page.evaluate(() => window.makoExample.diagnostics().refreshes)).toBe(before + 1);
});

test("reconnects the live stream and requests a resync", async ({ page }) => {
  const before = await page.evaluate(() => window.makoExample.diagnostics().streamConnections);
  await page.evaluate(() => window.makoExample.forceReconnect());
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().streamConnections))
    .toBeGreaterThan(before);
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().reconnects))
    .toBeGreaterThan(0);
});

test("clears local data and requires authentication after access revocation", async ({ page }) => {
  await page.evaluate(
    (document) => window.makoExample.putRemote(document),
    todo("private", "private data", 100),
  );
  await page.evaluate(() => window.makoExample.revokeAccess());

  expect(await page.evaluate(() => window.makoExample.listTodos())).toEqual([]);
  expect(await page.evaluate(() => window.makoExample.diagnostics().activity)).toBe(
    "authentication_required",
  );
});

function todo(id: string, title: string, updatedAt: number): Todo {
  return { id, ownerId: "user-example", title, updatedAt };
}

test("stops and asks for an update when the server's schema moves on, keeping local writes", async ({
  page,
}) => {
  await page.getByRole("textbox", { name: "Todo" }).fill("synced before the change");
  await page.getByRole("button", { name: "Add" }).click();
  await page.evaluate(() => window.makoExample.waitForSync());

  await page.evaluate(() => window.makoExample.requireSchemaVersion(2));
  await page.getByRole("textbox", { name: "Todo" }).fill("written after the change");
  await page.getByRole("button", { name: "Add" }).click();

  await page.waitForFunction(
    () => window.makoExample.diagnostics().recovery === "schema_migration_required:2",
  );
  await expect(page.locator("#status")).toHaveText("update required");
  await expect(page.getByRole("alert")).toContainText("out of date");
  // Replication is paused rather than retrying the refused request.
  const errors = await page.evaluate(() => window.makoExample.diagnostics().errors);
  await page.waitForTimeout(1_000);
  expect(await page.evaluate(() => window.makoExample.diagnostics().errors)).toBe(errors);
  // The write made after the change is still on this device. Ids are random,
  // so the list's order is too.
  await expect
    .poll(async () => (await titles(page).allTextContents()).sort())
    .toEqual(["synced before the change", "written after the change"]);
});
