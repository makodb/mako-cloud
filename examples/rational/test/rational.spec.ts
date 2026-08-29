import { expect, type Page, test } from "@playwright/test";

/**
 * Every screen against the in-browser fake backend. The wire is the fake's
 * implementation of the public protocol, so each assertion is about what
 * Rational derives from replicated documents and what it puts on the wire.
 */
const EMAIL = "pat@rational.test";
const PASSWORD = "RationalDemo1!";

async function openApplication(page: Page): Promise<void> {
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
}

async function signInFresh(page: Page): Promise<void> {
  await openApplication(page);
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("combobox", { name: "Household" })).toContainText("Demo household");
  await expect(page.getByRole("heading", { name: "Accounts" })).toBeVisible();
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await page.evaluate(() => window.rational.waitForSync());
}

const diagnostics = (page: Page) => page.evaluate(() => window.rational.diagnostics());

test("a person creates an account, signs in, and sees the household", async ({ page }) => {
  await openApplication(page);
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();

  // A wrong password is refused with a message and nothing else happens.
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill("not-the-password");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("alert")).toContainText("do not match");

  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("combobox", { name: "Household" })).toContainText("owner");
  await expect(page.locator('[data-testid^="account-acc_demo_"]')).toHaveCount(5);
  await expect(page.getByTestId("net-worth")).toContainText("Net worth (USD)");

  // The session persists across a reload.
  await page.reload();
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  await expect(page.getByRole("heading", { name: "Accounts" })).toBeVisible();

  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
});

test("accounts are created, edited, closed, and their balances derive from transactions", async ({
  page,
}) => {
  await signInFresh(page);
  await page.getByRole("button", { name: "New account" }).click();
  const editor = page.getByRole("form", { name: "Account editor" });
  await editor.getByLabel("Name").fill("Travel fund");
  await editor.getByLabel("Type").selectOption("savings");
  await editor.getByLabel("Currency").fill("USD");
  await editor.getByLabel("Opening balance").fill("500.00");
  await editor.getByLabel("Opening date").fill("2026-08-01");
  await editor.getByRole("button", { name: "Save account" }).click();

  const row = page.locator('tr[data-name="Travel fund"]');
  await expect(row).toBeVisible();
  await expect(row.getByTestId("balance")).toHaveText("$500.00");
  await expect.poll(async () => (await diagnostics(page)).acceptedWrites).toBeGreaterThan(0);

  // A transaction on the account moves its derived balance.
  const accountId = await row.evaluate((element) =>
    (element as HTMLElement).dataset.testid?.replace("account-", ""),
  );
  await page.evaluate(
    (id) =>
      window.rational.writes?.createTransaction({
        account_id: id,
        date: "2026-08-10",
        amount: -12_345,
        currency: "USD",
        description: "Flight deposit",
      }),
    accountId,
  );
  await expect(row.getByTestId("balance")).toHaveText("$376.55");

  await row.getByRole("button", { name: "Edit" }).click();
  await editor.getByLabel("Name").fill("Holiday fund");
  await editor.getByRole("button", { name: "Save account" }).click();
  await expect(page.locator('tr[data-name="Holiday fund"]')).toBeVisible();

  await page.locator('tr[data-name="Holiday fund"]').getByRole("button", { name: "Close" }).click();
  await expect(page.getByRole("table", { name: "Closed accounts" })).toContainText("Holiday fund");
  await page
    .getByRole("table", { name: "Closed accounts" })
    .getByRole("button", { name: "Reopen" })
    .click();
  await expect(page.getByRole("table", { name: "Open accounts" })).toContainText("Holiday fund");

  // Every account type is accepted.
  for (const type of ["checking", "credit", "investment", "loan", "cash"]) {
    await page.getByRole("button", { name: "New account" }).click();
    await editor.getByLabel("Name").fill(`${type} account`);
    await editor.getByLabel("Type").selectOption(type);
    await editor.getByLabel("Opening balance").fill(type === "loan" ? "-1000" : "10");
    await editor.getByRole("button", { name: "Save account" }).click();
    await expect(page.locator(`tr[data-name="${type} account"]`)).toContainText(type);
  }
});

test("transactions are created with splits that must add up, edited, and deleted", async ({
  page,
}) => {
  await signInFresh(page);
  await page.goto("/#/transactions");
  await page.getByRole("button", { name: "New transaction" }).click();
  const editor = page.getByRole("form", { name: "Transaction editor" });
  await editor.getByLabel("Account").selectOption({ label: "Everyday checking" });
  await editor.getByLabel("Date").fill("2026-08-20");
  await editor.getByLabel("Amount (USD)").fill("-45.67");
  await editor.getByLabel("Description").fill("Hardware and paint");
  await editor.getByLabel("Notes").fill("for the fence");
  await editor.getByRole("checkbox", { name: "shared" }).check();

  await editor.getByRole("button", { name: "Add split" }).click();
  await editor.getByRole("button", { name: "Add split" }).click();
  await editor.getByLabel("Split 1 category").selectOption({ label: "Shopping" });
  await editor.getByLabel("Split 1 amount").fill("-30.00");
  await editor.getByLabel("Split 2 category").selectOption({ label: "Groceries" });
  await editor.getByLabel("Split 2 amount").fill("-10.00");
  await expect(editor.getByTestId("split-difference")).toContainText("short by $5.67");

  // Saving with splits that do not add up is refused; nothing is written.
  const before = (await diagnostics(page)).household?.pendingWrites ?? 0;
  await editor.getByRole("button", { name: "Save transaction" }).click();
  await expect(editor.getByTestId("form-error")).toContainText("short by $5.67");
  expect((await diagnostics(page)).household?.pendingWrites ?? 0).toBe(before);
  await expect(page.locator('tr[data-description="Hardware and paint"]')).toHaveCount(0);

  await editor.getByLabel("Split 2 amount").fill("-15.67");
  await expect(editor.getByTestId("split-difference")).toContainText("add up");
  await editor.getByRole("button", { name: "Save transaction" }).click();
  const row = page.locator('tr[data-description="Hardware and paint"]');
  await expect(row).toBeVisible();
  await expect(row.getByTestId("amount")).toHaveText("-$45.67");
  await expect(row).toContainText("split");
  await expect(row.getByRole("list", { name: "Splits" })).toContainText("Shopping");
  await expect(row.getByRole("list", { name: "Splits" })).toContainText("-$15.67");
  await expect(row).toContainText("shared");
  await expect(row).toContainText("for the fence");

  // The month filter narrows the list; the account filter too.
  await page.getByRole("combobox", { name: "Filter by month" }).selectOption("2026-08");
  await expect(row).toBeVisible();
  await page.getByRole("combobox", { name: "Filter by month" }).selectOption("2026-06");
  await expect(row).toHaveCount(0);
  await page.getByRole("combobox", { name: "Filter by month" }).selectOption("");
  await page.getByRole("combobox", { name: "Filter by account" }).selectOption({ label: "Wallet" });
  await expect(row).toHaveCount(0);
  await page.getByRole("combobox", { name: "Filter by account" }).selectOption("");

  await row.getByRole("button", { name: "Edit" }).click();
  await editor.getByLabel("Description").fill("Hardware, paint, and brushes");
  await editor.getByRole("button", { name: "Save transaction" }).click();
  const edited = page.locator('tr[data-description="Hardware, paint, and brushes"]');
  await expect(edited).toBeVisible();

  await edited.getByRole("button", { name: "Delete" }).click();
  await expect(edited).toHaveCount(0);
  await expect.poll(async () => (await diagnostics(page)).acceptedWrites).toBeGreaterThanOrEqual(3);
});

test("offline writes are kept locally, shown as waiting, and pushed on reconnect", async ({
  page,
}) => {
  await signInFresh(page);
  await page.goto("/#/transactions");
  const accepted = (await diagnostics(page)).acceptedWrites;
  await page.getByRole("button", { name: "Go offline" }).click();
  await expect(page.getByTestId("offline-banner")).toContainText("offline");

  await page.getByRole("button", { name: "New transaction" }).click();
  const editor = page.getByRole("form", { name: "Transaction editor" });
  await editor.getByLabel("Amount (USD)").fill("-8.00");
  await editor.getByLabel("Description").fill("Written while offline");
  await editor.getByRole("button", { name: "Save transaction" }).click();
  await expect(page.locator('tr[data-description="Written while offline"]')).toBeVisible();
  await expect(page.getByTestId("pending-writes")).toContainText("1 change waiting");
  expect((await diagnostics(page)).acceptedWrites).toBe(accepted);

  await page.getByRole("button", { name: "Go online" }).click();
  await expect(page.getByTestId("offline-banner")).toHaveCount(0);
  await expect.poll(async () => (await diagnostics(page)).acceptedWrites).toBe(accepted + 1);
  await expect.poll(async () => (await diagnostics(page)).pendingWrites).toBe(0);
  const remote = await page.evaluate(() => {
    const fake = window.rationalFake;
    if (fake === undefined) return undefined;
    return window.rational.household?.session?.collections.transactions
      ?.find({ selector: { description: "Written while offline" } })
      .exec()
      .then((documents) => fake.remoteDocument("transactions", documents[0]?.toJSON().id ?? ""));
  });
  expect(remote?.description).toBe("Written while offline");
});

test("a remote edit arrives live and a conflicting edit resolves to the newer state", async ({
  page,
}) => {
  await signInFresh(page);
  await page.goto("/#/transactions");
  const row = page.locator('tr[data-testid="transaction-txn_demo_0001"]');
  await expect(row).toBeVisible();
  const original = await page.evaluate(() =>
    window.rationalFake?.remoteDocument("transactions", "txn_demo_0001"),
  );
  expect(original).toBeDefined();

  // Another device edits the same transaction; the live stream delivers it.
  await page.evaluate(
    (document) => {
      window.rationalFake?.putRemote("transactions", {
        ...document,
        description: "Edited elsewhere",
        updated_at: document.updated_at + 1_000,
      });
    },
    original as Record<string, unknown> & {
      updated_at: number;
      id: string;
      household_id: string;
      created_at: number;
    },
  );
  await expect(row.getByTestId("description")).toHaveText("Edited elsewhere");

  // Offline here, a newer edit there: the newer one wins on reconnect.
  await page.evaluate(() => window.rational.setOnline(false));
  await page.evaluate(() =>
    window.rational.writes?.updateTransaction(
      "txn_demo_0001",
      { description: "Local edit" },
      5_000,
    ),
  );
  await expect(row.getByTestId("description")).toHaveText("Local edit");
  await page.evaluate(
    (document) => {
      window.rationalFake?.putRemote("transactions", {
        ...document,
        description: "Newer remote edit",
        updated_at: 9_000,
      });
    },
    original as Record<string, unknown> & {
      updated_at: number;
      id: string;
      household_id: string;
      created_at: number;
    },
  );
  await page.evaluate(() => window.rational.setOnline(true));
  await expect.poll(async () => (await diagnostics(page)).conflicts).toBeGreaterThan(0);
  await expect(row.getByTestId("description")).toHaveText("Newer remote edit");
});

test("a security reset clears the household's local data and syncs it again", async ({ page }) => {
  await signInFresh(page);
  const before = await page.evaluate(() => window.rational.household?.session?.identifier);
  await page.evaluate(() => window.rationalFake?.setRole("pat@rational.test", "hh_demo", "editor"));
  await expect.poll(async () => (await diagnostics(page)).securityResets).toBeGreaterThan(0);
  await expect(page.getByTestId("notice")).toContainText("access changed");
  await page.waitForFunction(() => window.rational.household?.session !== null);
  const after = await page.evaluate(() => window.rational.household?.session?.identifier);
  expect(after).not.toBe(before);
  await expect(page.locator('[data-testid^="account-acc_demo_"]')).toHaveCount(5);
  await expect(page.getByRole("combobox", { name: "Household" })).toContainText("editor");
});
