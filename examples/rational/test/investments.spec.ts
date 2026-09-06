import { expect, type Page, test } from "@playwright/test";

/**
 * Holdings against the in-browser fake backend. A position is valued into
 * its account's balance by the same engine the accounts page uses, so the
 * proof is cross-screen: add a holding here, watch the balance there; update
 * its price, watch the balance move again with no transaction written.
 */
const EMAIL = "pat.investments@rational.test";
const PASSWORD = "RationalDemo1!";

async function signInFresh(page: Page): Promise<void> {
  await page.goto("/#/investments");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await expect(page.getByRole("heading", { name: "Investments", exact: true })).toBeVisible();
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await page.evaluate(() => window.rational.waitForSync());
}

/** "$1,234.56" or "-$12.00" as minor units. */
function money(text: string | null): number {
  return Math.round(Number.parseFloat((text ?? "").replaceAll(/[^0-9.-]/gu, "")) * 100);
}

const transactionCount = (page: Page) =>
  page.evaluate(async () => {
    const collection = window.rational.household?.session?.collections.transactions;
    if (collection === undefined) throw new Error("transactions are not open");
    return (await collection.find().exec()).length;
  });

/** The brokerage's balance as the accounts page shows it. */
async function brokerageBalance(page: Page): Promise<number> {
  await page.goto("/#/accounts");
  const row = page.getByTestId("account-acc_demo_brokerage");
  await expect(row).toBeVisible();
  return money(await row.getByTestId("balance").textContent());
}

test("a holding values its account, and a price update moves it with no transaction", async ({
  page,
}) => {
  await signInFresh(page);
  const before = await brokerageBalance(page);
  const transactionsBefore = await transactionCount(page);

  await page.goto("/#/investments");
  await expect(page.getByRole("heading", { name: "Investments", exact: true })).toBeVisible();
  // The demo's two positions are an ETF and a bond; the total is the brokerage's balance.
  const allocation = page.getByTestId("allocation-USD");
  await expect(allocation.locator("li[data-key]")).toHaveCount(2);
  await expect(allocation.locator('li[data-key="etf"]')).toBeVisible();
  await expect(allocation.locator('li[data-key="bond"]')).toBeVisible();
  expect(
    money(await page.getByTestId("investments-total-USD").getByTestId("total").textContent()),
  ).toBe(before);
  const demo = page.locator('tr[data-symbol="VTI"]');
  await expect(demo.getByTestId("value")).toHaveText("$11,262.50");
  await expect(demo.getByTestId("gain")).toContainText("+$1,062.50");

  // Ten units at 100.00: the account gains 1,000.00 and nothing is booked.
  await page
    .getByTestId("investment-account-acc_demo_brokerage")
    .getByRole("button", { name: "Add holding" })
    .click();
  const editor = page.getByRole("form", { name: "Holding editor" });
  await expect(editor).toContainText("Add holding to Brokerage");
  await editor.getByLabel("Symbol").fill("VXUS");
  await editor.getByLabel("Name").fill("Vanguard Total International Stock ETF");
  await editor.getByLabel("Quantity").fill("10");
  await editor.getByLabel("Price per unit").fill("100.00");
  await editor.getByLabel("Cost basis").fill("950.00");
  await editor.getByLabel("Asset class").selectOption("stock");
  await editor.getByRole("button", { name: "Save holding" }).click();
  const row = page.locator('tr[data-symbol="VXUS"]');
  await expect(row.getByTestId("value")).toHaveText("$1,000.00");
  await expect(row.getByTestId("gain")).toHaveText("+$50.00 (5.3%)");
  await expect(allocation.locator("li[data-key]")).toHaveCount(3);
  await expect(allocation.locator('li[data-key="stock"]')).toContainText("$1,000.00");
  expect(
    money(await page.getByTestId("investments-total-USD").getByTestId("total").textContent()),
  ).toBe(before + 100_000);
  expect(await brokerageBalance(page)).toBe(before + 100_000);
  expect(await transactionCount(page)).toBe(transactionsBefore);

  // The price moves to 110.00: another 100.00, still no transaction.
  await page.goto("/#/investments");
  await row.getByRole("button", { name: "Update price" }).click();
  await row.getByLabel("New price").fill("110.00");
  await row.getByLabel("Price date").fill("2026-09-01");
  await row.getByRole("button", { name: "Save price" }).click();
  await expect(row.getByTestId("value")).toHaveText("$1,100.00");
  await expect(row.getByTestId("gain")).toHaveText("+$150.00 (15.8%)");
  await expect(row.getByTestId("price")).toHaveAttribute("title", "as of 2026-09-01");
  expect(await brokerageBalance(page)).toBe(before + 110_000);
  expect(await transactionCount(page)).toBe(transactionsBefore);

  // The account's own page shows the same position and the same balance.
  await page.goto("/#/accounts/acc_demo_brokerage");
  await expect(page.getByRole("heading", { name: "Brokerage" })).toBeVisible();
  const holdings = page.getByRole("table", { name: "Holdings" });
  await expect(holdings.locator('tr[data-symbol="VXUS"]').getByTestId("value")).toHaveText(
    "$1,100.00",
  );
  expect(
    money(await page.getByTestId("account-balance").getByTestId("balance").textContent()),
  ).toBe(before + 110_000);
  await expect(page.getByTestId("holdings-value")).toContainText("$");

  // Removing the position gives the balance back.
  await page.goto("/#/investments");
  await row.getByRole("button", { name: "Remove" }).click();
  await expect(row).toHaveCount(0);
  await expect(allocation.locator("li[data-key]")).toHaveCount(2);
  expect(await brokerageBalance(page)).toBe(before);
  expect(await transactionCount(page)).toBe(transactionsBefore);
});
