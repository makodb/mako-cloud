import { expect, type Page, test } from "@playwright/test";

/**
 * The cash-flow page, over a household this test seeds itself.
 *
 * The fake joins every sign-up to the demo household so the screens have
 * data; `joinDemoOnSignup = false` gives this account an empty space of its
 * own instead, so every figure asserted here traces to a document the test
 * wrote. The quarter is a past one -- Q2 2025 -- so the "last month" part of
 * the test, which is relative to the real clock, can never land in it.
 */
const EMAIL = "flow@rational.test";
const PASSWORD = "RationalDemo1!";
const QUARTER = "2025-Q2";

/** `2026-08` for a run in September 2026: the month the "Last month" preset means. */
function lastMonthKey(): string {
  const now = new Date();
  return new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() - 1, 1))
    .toISOString()
    .slice(0, 7);
}

async function signUpIntoOwnSpace(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const off = () => {
      if (window.rationalFake !== undefined) window.rationalFake.joinDemoOnSignup = false;
      else setTimeout(off, 0);
    };
    off();
  });
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
  await page.waitForFunction(
    () =>
      window.rational.state.household?.initialSynced === true && window.rational.writes !== null,
    undefined,
    { timeout: 30_000 },
  );
}

/**
 * The writes this test makes beyond what `test-support/browser-globals.d.ts`
 * declares: groups, tags, a transfer pairing, and a tracked-asset update. The
 * application object has them; only the test-side declaration lags.
 */
interface CashFlowWrites extends RationalWritesWire {
  createGroup(name: string, kind: string): Promise<RationalDocumentWire>;
  createCategory(
    name: string,
    kind: string,
    options?: Record<string, unknown>,
  ): Promise<RationalDocumentWire>;
  createTag(name: string): Promise<RationalDocumentWire>;
  pairTransfer(outflowId: string, inflowId: string): Promise<string>;
  updateTrackedBalance(
    accountId: string,
    newBalance: number,
    date: string,
  ): Promise<RationalDocumentWire | null>;
}

interface Seeded {
  readonly checking: string;
  readonly savings: string;
  readonly house: string;
  readonly housing: string;
  readonly food: string;
  readonly rent: string;
  readonly groceries: string;
}

/**
 * A quarter of ordinary money -- three paychecks, three rents, three grocery
 * runs -- and the three things that must not count: a paired transfer to
 * savings, a hidden reimbursement, and a revaluation of the house. Then one
 * paycheck and one grocery run in last month, for the range picker.
 */
async function seed(page: Page): Promise<Seeded> {
  return page.evaluate(async (lastMonth) => {
    const writes = window.rational.writes as CashFlowWrites | null;
    if (writes === null) throw new Error("no household is open");
    const account = (name: string, type: "checking" | "savings" | "real_estate", opening: number) =>
      writes.createAccount({
        name,
        type,
        currency: "USD",
        opening_balance: opening,
        opening_date: "2025-01-01",
      });
    const checking = await account("Checking", "checking", 100_000);
    const savings = await account("Savings", "savings", 500_000);
    const house = await account("House", "real_estate", 30_000_000);
    const income = await writes.createGroup("Wages", "income");
    const housing = await writes.createGroup("Home", "expense");
    const food = await writes.createGroup("Food", "expense");
    const paychecks = await writes.createCategory("Paychecks", "income", { groupId: income.id });
    const rent = await writes.createCategory("Rent", "expense", { groupId: housing.id });
    const groceries = await writes.createCategory("Groceries", "expense", { groupId: food.id });
    const shared = await writes.createTag("shared");
    const book = (
      accountId: string,
      date: string,
      amount: number,
      description: string,
      extra: { category_id?: string; tags?: string[]; hidden?: boolean } = {},
    ) =>
      writes.createTransaction({
        account_id: accountId,
        date,
        amount,
        currency: "USD",
        description,
        tags: extra.tags ?? [],
        splits: [],
        ...(extra.category_id === undefined ? {} : { category_id: extra.category_id }),
        ...(extra.hidden === true ? { hidden: true } : {}),
      });
    for (const month of ["2025-04", "2025-05", "2025-06"]) {
      await book(checking.id, `${month}-01`, 400_000, "ACME payroll", {
        category_id: paychecks.id,
      });
      await book(checking.id, `${month}-02`, -150_000, "Rent", { category_id: rent.id });
      await book(checking.id, `${month}-12`, -30_000, "Corner Grocer", {
        category_id: groceries.id,
        tags: month === "2025-05" ? [shared.id] : [],
      });
    }
    // The three that must appear nowhere: a transfer (paired, so both legs
    // leave cash flow), a hidden inflow, and a balance update of the house.
    const out = await book(checking.id, "2025-05-15", -50_000, "Transfer to savings");
    const into = await book(savings.id, "2025-05-15", 50_000, "Transfer from checking");
    await writes.pairTransfer(out.id, into.id);
    await book(checking.id, "2025-05-20", 9_999, "Venmo reimbursement", { hidden: true });
    await writes.updateTrackedBalance(house.id, 31_000_000, "2025-05-25");
    // Last month, for the preset: one paycheck and one grocery run.
    await book(checking.id, `${lastMonth}-03`, 10_000, "ACME payroll", {
      category_id: paychecks.id,
    });
    await book(checking.id, `${lastMonth}-10`, -2_500, "Corner Grocer", {
      category_id: groceries.id,
    });
    return {
      checking: checking.id,
      savings: savings.id,
      house: house.id,
      housing: housing.id,
      food: food.id,
      rent: rent.id,
      groceries: groceries.id,
    };
  }, lastMonthKey());
}

test("a quarter's tiles count only the ordinary transactions", async ({ page }) => {
  await signUpIntoOwnSpace(page);
  const ids = await seed(page);
  await page.goto(`/#/cash-flow?range=${QUARTER}`);

  await expect(page.getByTestId("range-label")).toHaveText("Q2 2025");
  const tiles = page.getByTestId("cash-flow-summary-USD");
  // 3 × 4,000 in; 3 × 1,500 rent and 3 × 300 groceries out.
  await expect(tiles.getByTestId("income")).toHaveText("$12,000.00");
  await expect(tiles.getByTestId("spending")).toHaveText("$5,400.00");
  await expect(tiles.getByTestId("savings")).toHaveText("$6,600.00");
  await expect(tiles.getByTestId("savings-rate")).toHaveText("55%");

  // The transfer legs carry no category, so had they counted they would be
  // an "Uncategorized" row; the hidden inflow and the balance update would
  // have inflated income. None of them is anywhere.
  const categories = page.getByTestId("breakdown-table-category");
  await expect(categories.getByTestId("category-uncategorized")).toHaveCount(0);
  await expect(categories.getByTestId(`category-${ids.rent}`).getByTestId("amount")).toHaveText(
    "$4,500.00",
  );
  await expect(
    categories.getByTestId(`category-${ids.groceries}`).getByTestId("amount"),
  ).toHaveText("$900.00");

  await page.getByTestId("breakdown-account").click();
  const accounts = page.getByTestId("breakdown-table-account");
  await expect(accounts.getByTestId(`account-${ids.checking}`)).toBeVisible();
  await expect(accounts.getByTestId(`account-${ids.savings}`)).toHaveCount(0);
  await expect(accounts.getByTestId(`account-${ids.house}`)).toHaveCount(0);

  await page.getByTestId("breakdown-merchant").click();
  const merchants = page.getByTestId("breakdown-table-merchant");
  await expect(merchants).not.toContainText("Transfer");
  await expect(merchants).not.toContainText("Venmo");
  await expect(merchants).not.toContainText("Balance update");
  await expect(merchants.getByTestId("total")).toHaveText("$5,400.00");

  await expect(page.getByTestId("exclusions-note")).toContainText("balance updates");
});

test("the flow conserves: groups and categories both total the spending tile", async ({ page }) => {
  await signUpIntoOwnSpace(page);
  const ids = await seed(page);
  await page.goto(`/#/cash-flow?range=${QUARTER}`);

  const tiles = page.getByTestId("cash-flow-summary-USD");
  await expect(tiles.getByTestId("spending")).toHaveText("$5,400.00");
  await expect(page.getByRole("img", { name: "Where the money went" })).toBeVisible();

  // Groups is the default tab; its rows and its total are the Sankey's middle column.
  const groups = page.getByTestId("breakdown-table-group");
  await expect(groups.getByTestId(`group-${ids.housing}`).getByTestId("amount")).toHaveText(
    "$4,500.00",
  );
  await expect(groups.getByTestId(`group-${ids.food}`).getByTestId("amount")).toHaveText("$900.00");
  await expect(groups.getByTestId("total")).toHaveText("$5,400.00");
  // The categories are the right column, and they total the same.
  await expect(page.getByTestId("breakdown-table-category").getByTestId("total")).toHaveText(
    "$5,400.00",
  );

  // Tags overlap by design and say so; only one grocery run was tagged.
  await page.getByTestId("breakdown-tag").click();
  await expect(page.getByTestId("breakdown-table-tag").getByTestId("total")).toHaveText("$300.00");
  await expect(page.getByRole("tabpanel")).toContainText("need not add up");

  // The breakdown export is the table as a file.
  const downloading = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export breakdown CSV" }).click();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe(`spending-by-tag-${QUARTER}.csv`);
});

test("the range picker moves the address and the tiles", async ({ page }) => {
  await signUpIntoOwnSpace(page);
  await seed(page);
  await page.goto(`/#/cash-flow?range=${QUARTER}`);
  const tiles = page.getByTestId("cash-flow-summary-USD");
  await expect(tiles.getByTestId("income")).toHaveText("$12,000.00");

  await page.getByRole("button", { name: "Last month" }).click();
  const lastMonth = lastMonthKey();
  await expect(page).toHaveURL(new RegExp(`#/cash-flow\\?range=${lastMonth}$`, "u"));
  await expect(tiles.getByTestId("income")).toHaveText("$100.00");
  await expect(tiles.getByTestId("spending")).toHaveText("$25.00");
  await expect(tiles.getByTestId("savings")).toHaveText("$75.00");
  await expect(tiles.getByTestId("savings-rate")).toHaveText("75%");

  // The arrows step a month at a time; a month before last month holds nothing.
  await page.getByRole("button", { name: "Previous period" }).click();
  await expect(page).not.toHaveURL(new RegExp(`range=${lastMonth}$`, "u"));
  await expect(tiles.getByTestId("income")).toHaveText("$0.00");
  await expect(tiles.getByTestId("savings-rate")).toHaveText("—");

  // A reload shows the same range, because the range is the address.
  await page.goto(`/#/cash-flow?range=${QUARTER}`);
  await expect(page.getByTestId("range-label")).toHaveText("Q2 2025");
  await expect(tiles.getByTestId("income")).toHaveText("$12,000.00");
});

test("picking a category in the treemap trends it", async ({ page }) => {
  await signUpIntoOwnSpace(page);
  const ids = await seed(page);
  await page.goto(`/#/cash-flow?range=${QUARTER}`);

  // The largest category is trended until one is picked.
  await expect(page.getByTestId("trend-title")).toHaveText("Trend: Rent");
  await page.getByRole("button", { name: /^Groceries:/u }).click();
  await expect(page.getByTestId("trend-title")).toHaveText("Trend: Groceries");
  await expect(page.getByTestId(`category-${ids.groceries}`)).toHaveClass(/selected/u);
  await expect(page.getByTestId("trend-transactions")).toHaveAttribute(
    "href",
    `#/transactions?from=2025-04-01&to=2025-06-30&category=${ids.groceries}`,
  );

  // The table's own names pick too.
  await page.getByTestId(`category-${ids.rent}`).getByRole("button", { name: "Rent" }).click();
  await expect(page.getByTestId("trend-title")).toHaveText("Trend: Rent");
});
