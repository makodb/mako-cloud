import { expect, test } from "@playwright/test";

// `window.makoExample` is typed once for both suites in
// test-support/browser-globals.d.ts.
interface Todo {
  id: string;
  ownerId: string;
  title: string;
  updatedAt: number;
}

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.makoExample !== undefined);
});

test("keeps offline writes locally and pushes them after reconnect", async ({ page }) => {
  await page.evaluate(() => window.makoExample.setOnline(false));
  await page.getByRole("textbox", { name: "Todo" }).fill("written while offline");
  await page.getByRole("button", { name: "Add" }).click();

  await expect(page.locator('[data-testid="todo-todo-1"]')).toHaveText("written while offline");
  expect(await page.evaluate(() => window.makoExample.diagnostics().acceptedWrites)).toBe(0);

  await page.evaluate(() => window.makoExample.setOnline(true));
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBe(1);
});

test("edits and deletes a todo from the page and pushes each change", async ({ page }) => {
  await page.getByRole("textbox", { name: "Todo" }).fill("draft");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page.locator('[data-testid="todo-todo-1"]')).toHaveText("draft");
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBe(1);

  await page.getByRole("button", { name: "Edit draft" }).click();
  const editor = page.getByRole("textbox", { name: "Edit draft" });
  await editor.fill("final");
  await editor.press("Enter");
  await expect(page.locator('[data-testid="todo-todo-1"]')).toHaveText("final");
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBe(2);

  await page.getByRole("button", { name: "Delete final" }).click();
  await expect(page.locator('[data-testid="todo-todo-1"]')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => window.makoExample.listTodos())).toEqual([]);
  await expect
    .poll(() => page.evaluate(() => window.makoExample.diagnostics().acceptedWrites))
    .toBe(3);
});

test("numbers a new todo above the ones already synced", async ({ page }) => {
  await page.evaluate(
    (document) => window.makoExample.putRemote(document),
    todo("todo-5", "from the server", 100),
  );
  await expect(page.locator('[data-testid="todo-todo-5"]')).toHaveText("from the server");
  await page.getByRole("textbox", { name: "Todo" }).fill("added afterwards");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page.locator('[data-testid="todo-todo-6"]')).toHaveText("added afterwards");
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
