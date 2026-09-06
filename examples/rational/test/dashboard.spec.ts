import { expect, type Page, test } from "@playwright/test";

import type { Account, Budget, Goal, TaxonomyEntry, Transaction } from "../src/model/types.js";
import { accountBalances, netWorthIn } from "../src/selectors/balances.js";
import { budgetMonth, budgetRemaining, previousMonth } from "../src/selectors/budget-month.js";
import { cashFlowSummary, monthRange, summaryFor } from "../src/selectors/cashflow.js";
import { formatMinorUnits } from "../src/selectors/money.js";
import { monthKey } from "../src/selectors/transactions.js";

/**
 * The dashboard against the in-browser fake backend.
 *
 * The page's promise is that every number on it is some other screen's
 * number, so the test asks those screens' own selectors what they would
 * show -- over the documents the page actually holds -- and expects the
 * cards to agree to the cent. The demo household supplies the fixtures: three
 * synced arrivals nobody has reviewed, four confirmed bills of which the
 * streaming charge falls due first, and a trip goal three contributions in.
 */
const EMAIL = "pat.dashboard@rational.test";
const PASSWORD = "RationalDemo1!";
const CURRENCY = "USD";

async function signInFresh(page: Page): Promise<void> {
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await page.waitForFunction(() => window.rational.state.directory?.initialSynced === true);
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await page.evaluate(() => window.rational.waitForSync());
}

/** After a reload the session is restored from the device; wait for the household to be open again. */
async function waitForReturn(page: Page): Promise<void> {
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await page.evaluate(() => window.rational.waitForSync());
}

interface HouseholdDocuments {
  readonly accounts: Account[];
  readonly transactions: Transaction[];
  readonly taxonomy: TaxonomyEntry[];
  readonly budgets: Budget[];
  readonly goals: Goal[];
}

/** The documents the open household holds, as plain objects, for the selectors to read. */
async function householdDocuments(page: Page): Promise<HouseholdDocuments> {
  return page.evaluate(async () => {
    type Queryable = { find(): { exec(): Promise<Array<{ toJSON(): unknown }>> } };
    const collections = window.rational.household?.session?.collections as
      | Partial<Record<string, Queryable>>
      | undefined;
    if (collections === undefined) throw new Error("no household is open");
    const all = async <T>(id: string): Promise<T[]> => {
      const collection = collections[id];
      if (collection === undefined) throw new Error(`${id} is not open`);
      return (await collection.find().exec()).map((document) => document.toJSON() as T);
    };
    return {
      accounts: await all<Account>("accounts"),
      transactions: await all<Transaction>("transactions"),
      taxonomy: await all<TaxonomyEntry>("taxonomy"),
      budgets: await all<Budget>("budgets"),
      goals: await all<Goal>("goals"),
    };
  });
}

test("the dashboard shows the numbers its screens own and leads to them", async ({ page }) => {
  await signInFresh(page);
  const today = new Date().toISOString().slice(0, 10);
  const thisMonth = monthKey(today);
  const { accounts, transactions, taxonomy, budgets, goals } = await householdDocuments(page);
  expect(accounts.length).toBeGreaterThan(0);
  expect(transactions.length).toBeGreaterThan(0);

  // Net worth: what the accounts page derives from the same documents.
  const balances = accountBalances(accounts, transactions);
  const netWorth = formatMinorUnits(netWorthIn(CURRENCY, accounts, balances).netWorth, CURRENCY);
  await expect(page.getByTestId("net-worth-figure")).toHaveText(netWorth);
  await expect(page.getByTestId("net-worth-change")).toHaveAttribute("data-direction", /up|down/u);
  await expect(
    page.getByTestId("widget-net-worth").getByRole("img", { name: /Net worth over the last/u }),
  ).toBeVisible();

  // Spending: the cash-flow page's figure for this month and for last month.
  const spendingIn = (month: string): string =>
    formatMinorUnits(
      summaryFor(cashFlowSummary(transactions, taxonomy, monthRange(month)), CURRENCY).spending,
      CURRENCY,
    );
  await expect(page.getByTestId("spending-this-month")).toHaveText(spendingIn(thisMonth));
  await expect(page.getByTestId("spending-last-month")).toHaveText(
    spendingIn(previousMonth(thisMonth)),
  );

  // Budget: the budget page's total for the month, in the household's mode.
  const model = budgetMonth(budgets, transactions, taxonomy, thisMonth, CURRENCY, goals);
  await expect(page.getByTestId("budget-remaining")).toHaveText(
    formatMinorUnits(budgetRemaining(model, "category"), CURRENCY),
  );

  // Review: the three synced arrivals the demo leaves unreviewed.
  await expect(page.getByTestId("review-count")).toHaveText("3");

  // Bills: the streaming charge falls due first, and the card says so first.
  const bills = page.getByTestId("widget-bills").locator('[data-testid^="bill-"]');
  await expect(bills.first()).toHaveAttribute("data-testid", "bill-rec_demo_streaming");
  await expect(bills.first()).toContainText("Streaming Service");
  await expect(bills.first()).toContainText("-$15.99");
  await expect(bills).toHaveCount(3);

  // Goals: the trip has three contributions of sixty against six hundred.
  const trip = page.getByTestId("goal-goa_demo_trip");
  await expect(trip).toHaveAttribute("data-percent", "30");
  await expect(trip).toContainText("30%");
  await expect(trip).toContainText("$1,800.00 of $6,000.00 saved");

  // Recent transactions name their merchant, and investments total the brokerage.
  const recent = page.getByTestId("widget-recent").locator('[data-testid^="recent-"]');
  await expect(recent).toHaveCount(5);
  await expect(page.getByTestId("investments-total")).toHaveText(
    formatMinorUnits(balances.get("acc_demo_brokerage") ?? 0, CURRENCY),
  );
  await expect(page.getByTestId("unread-alerts")).toHaveText("0");

  // Each card leads to the screen that owns its number, opened on what it showed.
  await page.getByTestId("widget-review").getByTestId("widget-link").click();
  await expect(page).toHaveURL(/#\/transactions\?review=needs$/u);
  await page.goto("/#/dashboard");
  await page.getByTestId("widget-spending").getByTestId("widget-link").click();
  await expect(page).toHaveURL(new RegExp(`#/cash-flow\\?range=${thisMonth}$`, "u"));
  await page.goto("/#/dashboard");
  await page.getByTestId("widget-net-worth").getByTestId("widget-link").click();
  await expect(page).toHaveURL(/#\/accounts$/u);
  // And the accounts page shows the very same figure.
  await expect(page.locator("main")).toContainText(netWorth);
});

test("cards are hidden and reordered from Customize, and the layout survives a reload", async ({
  page,
}) => {
  await signInFresh(page);
  const cards = page.locator(".dashboard-grid .widget");
  await expect(cards).toHaveCount(9);
  await expect(cards.first()).toHaveAttribute("data-widget", "net-worth");
  await expect(page.getByTestId("widget-investments")).toBeVisible();

  await page.getByRole("button", { name: "Customize" }).click();
  await page.getByRole("button", { name: "Hide Investments" }).click();
  // While customizing the card stays, dimmed, so it can be shown again.
  await expect(page.getByTestId("widget-investments")).toHaveAttribute("data-hidden", "true");
  await page.getByRole("button", { name: "Move Spending up" }).click();
  await expect(cards.first()).toHaveAttribute("data-widget", "spending");
  await expect(cards.nth(1)).toHaveAttribute("data-widget", "net-worth");
  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.getByTestId("widget-investments")).toHaveCount(0);
  await expect(cards).toHaveCount(8);

  // The layout is the device's, and comes back with the page.
  const stored = await page.evaluate(() =>
    window.localStorage.getItem("rational.dashboard.layout"),
  );
  expect(stored).toContain("investments");
  await page.reload();
  await waitForReturn(page);
  await expect(page.getByTestId("widget-investments")).toHaveCount(0);
  await expect(cards.first()).toHaveAttribute("data-widget", "spending");

  // Shown again, and reset puts everything back where it started.
  await page.getByRole("button", { name: "Customize" }).click();
  await page.getByRole("button", { name: "Show Investments" }).click();
  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.getByTestId("widget-investments")).toBeVisible();
  await expect(cards).toHaveCount(9);
  await page.getByRole("button", { name: "Customize" }).click();
  await page.getByRole("button", { name: "Reset layout" }).click();
  await page.getByRole("button", { name: "Done" }).click();
  await expect(cards.first()).toHaveAttribute("data-widget", "net-worth");
  expect(
    await page.evaluate(() => window.localStorage.getItem("rational.dashboard.layout")),
  ).toBeNull();
});
