import { expect, type Page, test } from "@playwright/test";

/**
 * Accounts against the in-browser fake backend: manual accounts of every
 * class grouped with their subtotals, balances derived from transactions,
 * closing and reopening; a tracked asset whose value is updated in place
 * without becoming spending; and an account hidden from net worth that stays
 * an account.
 */
const EMAIL = "pat.accounts@rational.test";
const PASSWORD = "RationalDemo1!";

/** Every account type with the label it is shown under and the class table it lands in. */
const TYPES: ReadonlyArray<readonly [type: string, label: string, table: string]> = [
  ["checking", "Checking", "Cash accounts"],
  ["credit", "Credit card", "Credit cards accounts"],
  ["investment", "Investment", "Investments accounts"],
  ["loan", "Loan", "Loans accounts"],
  ["cash", "Cash", "Cash accounts"],
  ["real_estate", "Real estate", "Real estate accounts"],
  ["vehicle", "Vehicle", "Vehicles accounts"],
  ["crypto", "Crypto", "Crypto accounts"],
  ["other_asset", "Other asset", "Other assets accounts"],
  ["other_liability", "Other liability", "Other liabilities accounts"],
];

async function signInFresh(page: Page): Promise<void> {
  await page.goto("/#/accounts");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await expect(page.getByRole("heading", { name: "Accounts", exact: true })).toBeVisible();
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await page.evaluate(() => window.rational.waitForSync());
}

const diagnostics = (page: Page) => page.evaluate(() => window.rational.diagnostics());

/** "$1,234.56" or "-$12.00" as minor units. */
function money(text: string | null): number {
  return Math.round(Number.parseFloat((text ?? "").replaceAll(/[^0-9.-]/gu, "")) * 100);
}

const netWorth = async (page: Page) =>
  money(await page.getByTestId("net-worth-USD").getByTestId("net").textContent());

/**
 * What the cash-flow figures are made of for one month, read from the local
 * database with the exclusion every total applies: no hidden transaction, no
 * balance update, no transfer leg. A balance update must leave this alone.
 */
const monthFlow = (page: Page, month: string) =>
  page.evaluate(async (key) => {
    const collection = window.rational.household?.session?.collections.transactions;
    if (collection === undefined) throw new Error("transactions are not open");
    const documents = await collection.find().exec();
    let counted = 0;
    let income = 0;
    let spending = 0;
    for (const document of documents.map((entry) => entry.toJSON())) {
      const { date, amount, hidden, adjustment, transfer_id: transferId } = document;
      if (typeof date !== "string" || typeof amount !== "number") continue;
      if (!date.startsWith(key)) continue;
      if (hidden === true || adjustment === true) continue;
      if (typeof transferId === "string" && transferId !== "") continue;
      counted += 1;
      if (amount >= 0) income += amount;
      else spending += -amount;
    }
    return { counted, income, spending };
  }, month);

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
  await expect(row).toContainText("Savings");
  await expect(page.getByRole("table", { name: "Cash accounts" })).toContainText("Travel fund");
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
  await expect(page.getByRole("table", { name: "Cash accounts" })).toContainText("Holiday fund");

  // Every account type is accepted, shown under its label, and filed in its class.
  for (const [type, label, table] of TYPES) {
    await page.getByRole("button", { name: "New account" }).click();
    await editor.getByLabel("Name").fill(`${type} account`);
    await editor.getByLabel("Type").selectOption(type);
    await editor
      .getByLabel("Opening balance")
      .fill(type === "loan" || type === "other_liability" ? "-1000" : "10");
    await editor.getByRole("button", { name: "Save account" }).click();
    const created = page
      .getByRole("table", { name: table })
      .locator(`tr[data-name="${type} account"]`);
    await expect(created).toBeVisible();
    await expect(created).toContainText(label);
  }
});

test("a real-estate account's value is updated in place: net worth moves, cash flow does not", async ({
  page,
}) => {
  await signInFresh(page);
  const netBefore = await netWorth(page);
  const flowBefore = await monthFlow(page, "2026-08");
  expect(flowBefore.counted).toBeGreaterThan(0);

  await page.getByRole("button", { name: "New account" }).click();
  const editor = page.getByRole("form", { name: "Account editor" });
  await editor.getByLabel("Name").fill("Lake cabin");
  await editor.getByLabel("Type").selectOption("real_estate");
  await editor.getByLabel("Opening balance").fill("250000.00");
  await editor.getByLabel("Opening date").fill("2026-08-01");
  await editor.getByRole("button", { name: "Save account" }).click();

  const table = page.getByRole("table", { name: "Real estate accounts" });
  const row = table.locator('tr[data-name="Lake cabin"]');
  await expect(row.getByTestId("balance")).toHaveText("$250,000.00");
  await expect(row).toContainText("Real estate");
  await expect.poll(() => netWorth(page)).toBe(netBefore + 25_000_000);

  // The account's own page: the value is set in place, not by a transaction.
  await row.getByRole("link", { name: "Lake cabin" }).click();
  await expect(page.getByRole("heading", { name: "Lake cabin" })).toBeVisible();
  const balance = page.getByTestId("account-balance").getByTestId("balance");
  await expect(balance).toHaveText("$250,000.00");
  await page.getByRole("button", { name: "Update value" }).click();
  const form = page.getByTestId("update-value");
  await form.getByLabel("New value").fill("262500.00");
  await form.getByLabel("As of").fill("2026-08-20");
  await form.getByRole("button", { name: "Save value" }).click();
  await expect(balance).toHaveText("$262,500.00");

  // The update appears in the account's history as a balance update.
  const history = page.getByRole("table", { name: "Balance updates" });
  const update = history.locator('tr[data-description="Balance update"]');
  await expect(update).toHaveCount(1);
  await expect(update.getByTestId("amount")).toHaveText("$12,500.00");
  await expect(update).toContainText("balance update");

  // Net worth moved by the difference; the month's income and spending did not.
  await page.goto("/#/accounts");
  await expect.poll(() => netWorth(page)).toBe(netBefore + 26_250_000);
  expect(await monthFlow(page, "2026-08")).toEqual(flowBefore);
  const booked = await page.evaluate(async () => {
    const collection = window.rational.household?.session?.collections.transactions;
    if (collection === undefined) throw new Error("transactions are not open");
    const documents = await collection
      .find({ selector: { description: "Balance update", amount: 1_250_000 } })
      .exec();
    return documents.map((document) => document.toJSON());
  });
  expect(booked).toHaveLength(1);
  expect(booked[0]?.adjustment).toBe(true);
  expect(booked[0]?.hidden).toBe(true);
  expect(booked[0]?.date).toBe("2026-08-20");
});

test("hiding an account takes its balance out of net worth and keeps the account", async ({
  page,
}) => {
  await signInFresh(page);
  const savings = page.getByTestId("account-acc_demo_savings");
  await expect(savings).toBeVisible();
  const balance = money(await savings.getByTestId("balance").textContent());
  expect(balance).toBeGreaterThan(0);
  const netBefore = await netWorth(page);
  const cash = page.getByRole("table", { name: "Cash accounts" });
  const subtotalBefore = money(await cash.getByTestId("subtotal").textContent());

  await savings.getByRole("link", { name: "Rainy-day savings" }).click();
  await expect(page.getByRole("heading", { name: "Rainy-day savings" })).toBeVisible();
  await page.getByRole("button", { name: "Hide from net worth" }).click();
  await expect(page.getByRole("button", { name: "Unhide from net worth" })).toBeVisible();
  await expect(page.getByTestId("account-meta")).toContainText("hidden from net worth");
  // Its transactions and its history are still there to read.
  await expect(
    page.getByRole("table", { name: "Transactions" }).locator("tr[data-description]").first(),
  ).toBeVisible();
  await expect(page.getByTestId("balance-history")).toBeVisible();

  await page.goto("/#/accounts");
  await expect(savings).toBeVisible();
  await expect(savings).toContainText("hidden");
  expect(money(await savings.getByTestId("balance").textContent())).toBe(balance);
  await expect.poll(() => netWorth(page)).toBe(netBefore - balance);
  expect(money(await cash.getByTestId("subtotal").textContent())).toBe(subtotalBefore - balance);

  // Unhiding puts it back, to the cent.
  await page.goto("/#/accounts/acc_demo_savings");
  await page.getByRole("button", { name: "Unhide from net worth" }).click();
  await expect(page.getByRole("button", { name: "Hide from net worth" })).toBeVisible();
  await page.goto("/#/accounts");
  await expect.poll(() => netWorth(page)).toBe(netBefore);
  await expect(savings).not.toContainText("hidden");
});
