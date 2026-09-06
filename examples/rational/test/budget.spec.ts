import { expect, type Page, test } from "@playwright/test";

/**
 * The budget page against the in-browser fake backend: rollover, a group
 * budgeted as a whole, the flex presentation, and copying a month forward.
 *
 * Every test starts from a brand-new account that belongs nowhere, so the
 * space it is dropped into holds only the default categories and whatever
 * the test seeds -- the demo household's three months of budgets would make
 * "an empty month" impossible to find. Seeding goes through the same
 * `HouseholdWrites` the screens use, so the documents are the ones the app
 * would have written.
 */
const PASSWORD = "RationalDemo1!";

/**
 * What the seeding reaches of the writes. The shared browser typings stop at
 * the two-argument `createCategory` and know no `createGroup`, so this names
 * the wider signatures the budget scenarios need; the shapes are those of
 * `src/data/writes.ts`.
 */
interface BudgetSeedWrites {
  createAccount(input: Record<string, unknown>): Promise<{ id: string }>;
  createGroup(name: string, kind: string): Promise<{ id: string }>;
  createCategory(
    name: string,
    kind: string,
    options?: Record<string, unknown>,
  ): Promise<{ id: string }>;
  createTransaction(input: Record<string, unknown>): Promise<{ id: string }>;
  setBudget(input: Record<string, unknown>): Promise<unknown>;
  createGoal(input: Record<string, unknown>): Promise<unknown>;
}

interface HouseholdSettingsApp {
  updateHousehold(patch: { budget_mode: string }): Promise<unknown>;
}

async function openFreshBudget(page: Page, email: string): Promise<void> {
  await page.addInitScript(() => {
    const off = () => {
      if (window.rationalFake !== undefined) window.rationalFake.joinDemoOnSignup = false;
      else setTimeout(off, 0);
    };
    off();
  });
  await page.goto("/#/budget");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await page.waitForFunction(
    () =>
      window.rational.state.currentHouseholdId !== null &&
      window.rational.state.household?.initialSynced === true &&
      window.rational.writes !== null,
    undefined,
    { timeout: 30_000 },
  );
  await expect(page.getByRole("heading", { name: "Budget" })).toBeVisible();
}

/** Move the page to a month by its address, as a link from another screen would. */
async function showMonth(page: Page, month: string, title: string): Promise<void> {
  await page.evaluate((target) => {
    window.location.hash = `#/budget?month=${target}`;
  }, month);
  await expect(page.getByTestId("budget-month")).toHaveText(title);
}

/** A checking account to spend from; every scenario needs one. */
async function seedAccount(page: Page): Promise<string> {
  return page.evaluate(async () => {
    const writes = window.rational.writes as unknown as BudgetSeedWrites | null;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 0,
      opening_date: "2026-01-01",
    });
    return account.id;
  });
}

test("a budget rolls over, is edited in place, and the month is paged", async ({ page }) => {
  await openFreshBudget(page, "wren.budget@rational.test");
  const accountId = await seedAccount(page);

  // Three monthly charges against one category, and a budget on two
  // consecutive months that rolls over: June's unspent 87.01 carries into July.
  const { groupId, categoryId } = await page.evaluate(async (account) => {
    const writes = window.rational.writes as unknown as BudgetSeedWrites | null;
    if (writes === null) throw new Error("no household is open");
    const group = await writes.createGroup("Entertainment", "expense");
    const category = await writes.createCategory("Subscriptions", "expense", {
      groupId: group.id,
      icon: "📺",
    });
    for (const [index, date] of ["2026-06-01", "2026-07-01", "2026-08-01"].entries()) {
      await writes.createTransaction({
        account_id: account,
        date,
        amount: -1_299,
        currency: "USD",
        description: `STREAMING SERVICE ${index}`,
        category_id: category.id,
        tags: [],
        splits: [],
      });
    }
    for (const month of ["2026-06", "2026-07"]) {
      await writes.setBudget({
        kind: "category",
        category_id: category.id,
        month,
        amount: 10_000,
        currency: "USD",
        rollover: true,
      });
    }
    return { groupId: group.id, categoryId: category.id };
  }, accountId);

  await showMonth(page, "2026-07", "July 2026");
  const row = page.getByTestId(`budget-${categoryId}`);
  await expect(row.getByTestId("carried-in")).toHaveText("$87.01");
  await expect(row.getByTestId("allowance")).toHaveText("$187.01");
  await expect(row.getByTestId("spent")).toHaveText("$12.99");
  await expect(row.getByTestId("remaining")).toHaveText("$174.02");
  await expect(row).toHaveAttribute("data-percent", "7");
  await expect(row.getByRole("textbox", { name: "Budget for Subscriptions" })).toHaveValue(
    "100.00",
  );
  await expect(row.getByRole("checkbox", { name: "Roll over Subscriptions" })).toBeChecked();
  // The group's figures are its categories' together.
  const group = page.getByTestId(`budget-group-${groupId}`);
  await expect(group.getByTestId("group-spent")).toHaveText("$12.99");
  await expect(group.getByTestId("group-budgeted")).toHaveText("$187.01");

  // Typing a new amount and leaving the field is the save; the carried-in
  // amount is June's and does not move.
  await row.getByRole("textbox", { name: "Budget for Subscriptions" }).fill("120.00");
  await row.getByRole("textbox", { name: "Budget for Subscriptions" }).press("Enter");
  await expect(row.getByTestId("allowance")).toHaveText("$207.01");
  await expect(row.getByTestId("remaining")).toHaveText("$194.02");
  await expect(row.getByTestId("carried-in")).toHaveText("$87.01");
  await expect(page.getByTestId("budget-total-USD").getByTestId("budgeted")).toHaveText("$207.01");

  // Paging moves the month in the address; June carried nothing in.
  await page.getByRole("button", { name: "Previous month" }).click();
  await expect(page.getByTestId("budget-month")).toHaveText("June 2026");
  await expect(page).toHaveURL(/month=2026-06/u);
  await expect(row.getByTestId("carried-in")).toHaveText("—");
  await expect(row.getByTestId("allowance")).toHaveText("$100.00");
  await expect(row.getByTestId("remaining")).toHaveText("$87.01");
  await page.getByRole("button", { name: "Next month" }).click();
  await expect(page.getByTestId("budget-month")).toHaveText("July 2026");
  await expect(row.getByTestId("allowance")).toHaveText("$207.01");
});

test("a group budgeted as a whole sums its categories and hides their own fields", async ({
  page,
}) => {
  await openFreshBudget(page, "wren.group@rational.test");
  const accountId = await seedAccount(page);
  const { groupId, groceriesId, diningId } = await page.evaluate(async (account) => {
    const writes = window.rational.writes as unknown as BudgetSeedWrites | null;
    if (writes === null) throw new Error("no household is open");
    const group = await writes.createGroup("Food", "expense");
    const groceries = await writes.createCategory("Groceries", "expense", { groupId: group.id });
    const dining = await writes.createCategory("Dining", "expense", { groupId: group.id });
    await writes.createTransaction({
      account_id: account,
      date: "2026-07-10",
      amount: -5_000,
      currency: "USD",
      description: "MARKET",
      category_id: groceries.id,
    });
    await writes.createTransaction({
      account_id: account,
      date: "2026-07-12",
      amount: -3_000,
      currency: "USD",
      description: "BISTRO",
      category_id: dining.id,
    });
    return { groupId: group.id, groceriesId: groceries.id, diningId: dining.id };
  }, accountId);

  await showMonth(page, "2026-07", "July 2026");
  const group = page.getByTestId(`budget-group-${groupId}`);
  const groceries = page.getByTestId(`budget-${groceriesId}`);
  const dining = page.getByTestId(`budget-${diningId}`);
  // Budgeted by category to begin with: each row has its own field, and with
  // nothing budgeted both are unbudgeted spending.
  await expect(groceries.getByRole("textbox", { name: "Budget for Groceries" })).toHaveCount(1);
  await expect(page.getByTestId(`unbudgeted-${groceriesId}`).getByTestId("spent")).toHaveText(
    "$50.00",
  );
  await expect(page.getByTestId("unbudgeted-total")).toHaveText("$80.00");

  await group.getByRole("button", { name: "Budget this group as a whole" }).click();
  const groupField = group.getByRole("textbox", { name: "Budget for Food" });
  await groupField.fill("200.00");
  await groupField.press("Enter");

  // The group's spent is the sum over its categories, its remaining the
  // budget less that, and the categories show no field of their own.
  await expect(group.getByTestId("group-spent")).toHaveText("$80.00");
  await expect(group.getByTestId("group-budgeted")).toHaveText("$200.00");
  await expect(group.getByTestId("group-remaining")).toHaveText("$120.00");
  await expect(groceries.getByRole("textbox", { name: "Budget for Groceries" })).toHaveCount(0);
  await expect(dining.getByRole("textbox", { name: "Budget for Dining" })).toHaveCount(0);
  await expect(groceries.getByTestId("spent")).toHaveText("$50.00");
  await expect(dining.getByTestId("spent")).toHaveText("$30.00");
  await expect(page.getByTestId("unbudgeted")).toHaveCount(0);
  await expect(page.getByTestId("budget-total-USD").getByTestId("budgeted")).toHaveText("$200.00");
  await expect(page.getByTestId("budget-total-USD").getByTestId("remaining")).toHaveText("$120.00");

  // Going back to budgeting by category gives the rows their fields again.
  await group.getByRole("button", { name: "Budget categories separately" }).click();
  await expect(groceries.getByRole("textbox", { name: "Budget for Groceries" })).toHaveCount(1);
  await expect(group.getByRole("button", { name: "Budget this group as a whole" })).toBeVisible();
});

test("in flex mode the flexible amount is what income leaves", async ({ page }) => {
  await openFreshBudget(page, "wren.flex@rational.test");
  const accountId = await seedAccount(page);
  const { rentId, housingId } = await page.evaluate(async (account) => {
    const writes = window.rational.writes as unknown as BudgetSeedWrites | null;
    if (writes === null) throw new Error("no household is open");
    const housing = await writes.createGroup("Housing", "expense");
    const rent = await writes.createCategory("Rent", "expense", {
      groupId: housing.id,
      budgetBucket: "fixed",
    });
    const travel = await writes.createGroup("Travel", "expense");
    await writes.createCategory("Vacation", "expense", {
      groupId: travel.id,
      budgetBucket: "non_monthly",
      targetAmount: 120_000,
      targetMonths: 12,
    });
    const food = await writes.createGroup("Food", "expense");
    const groceries = await writes.createCategory("Groceries", "expense", {
      groupId: food.id,
      budgetBucket: "flexible",
    });
    await writes.createTransaction({
      account_id: account,
      date: "2026-07-01",
      amount: -150_000,
      currency: "USD",
      description: "RENT",
      category_id: rent.id,
    });
    await writes.createTransaction({
      account_id: account,
      date: "2026-07-08",
      amount: -20_000,
      currency: "USD",
      description: "MARKET",
      category_id: groceries.id,
    });
    await writes.setBudget({
      kind: "income",
      category_id: "income",
      month: "2026-07",
      amount: 500_000,
      currency: "USD",
      rollover: false,
    });
    await writes.setBudget({
      kind: "category",
      category_id: rent.id,
      month: "2026-07",
      amount: 150_000,
      currency: "USD",
      rollover: false,
    });
    await writes.createGoal({
      name: "Emergency fund",
      target_amount: 1_000_000,
      currency: "USD",
      planned_monthly: 50_000,
    });
    return { rentId: rent.id, housingId: housing.id };
  }, accountId);

  // The mode is the household's setting; the page follows it live.
  await page.evaluate(() =>
    (window.rational as unknown as HouseholdSettingsApp).updateHousehold({ budget_mode: "flex" }),
  );
  await showMonth(page, "2026-07", "July 2026");
  const flexible = page.getByTestId("bucket-flexible");
  await expect(flexible).toBeVisible();
  // 5,000 income − 1,500 fixed − 100 a month for the vacation − 500 to the goal.
  await expect(flexible.getByTestId("flexible-allowance")).toHaveText("$2,900.00");
  await expect(flexible.getByTestId("flexible-spent")).toHaveText("$200.00");
  await expect(flexible.getByTestId("flexible-remaining")).toHaveText("$2,700.00");
  await expect(flexible.getByTestId("term-income")).toHaveText("$5,000.00");
  await expect(flexible.getByTestId("term-fixed")).toHaveText("$1,500.00");
  await expect(flexible.getByTestId("term-non-monthly")).toHaveText("$100.00");
  await expect(flexible.getByTestId("term-goals")).toHaveText("$500.00");
  const fixed = page.getByTestId("bucket-fixed");
  await expect(fixed.getByTestId("bucket-budgeted")).toHaveText("$1,500.00");
  await expect(fixed.getByTestId("bucket-spent")).toHaveText("$1,500.00");
  await expect(page.getByTestId("bucket-non-monthly").getByTestId("bucket-share")).toHaveText(
    "$100.00",
  );
  await expect(page.getByRole("link", { name: "Change in Settings › Household" })).toHaveAttribute(
    "href",
    "#/settings/household",
  );

  // Back in category mode the same budgets show as groups, unchanged.
  await page.evaluate(() =>
    (window.rational as unknown as HouseholdSettingsApp).updateHousehold({
      budget_mode: "category",
    }),
  );
  await expect(page.getByTestId(`budget-group-${housingId}`)).toBeVisible();
  await expect(page.getByTestId("bucket-flexible")).toHaveCount(0);
  await expect(
    page.getByTestId(`budget-${rentId}`).getByRole("textbox", { name: "Budget for Rent" }),
  ).toHaveValue("1500.00");
  await expect(page.getByTestId("expected-income")).toContainText("$5,000.00");
});

test("copying last month fills an empty month and leaves what it already had", async ({ page }) => {
  await openFreshBudget(page, "wren.copy@rational.test");
  const { groceriesId, diningId } = await page.evaluate(async () => {
    const writes = window.rational.writes as unknown as BudgetSeedWrites | null;
    if (writes === null) throw new Error("no household is open");
    const food = await writes.createGroup("Food", "expense");
    const groceries = await writes.createCategory("Groceries", "expense", { groupId: food.id });
    const dining = await writes.createCategory("Dining", "expense", { groupId: food.id });
    const budget = (input: Record<string, unknown>) =>
      writes.setBudget({ currency: "USD", rollover: false, kind: "category", ...input });
    await budget({ category_id: groceries.id, month: "2026-06", amount: 40_000, rollover: true });
    await budget({ category_id: dining.id, month: "2026-06", amount: 20_000 });
    await budget({ kind: "income", category_id: "income", month: "2026-06", amount: 500_000 });
    // July already has its own idea about dining; copying must not touch it.
    await budget({ category_id: dining.id, month: "2026-07", amount: 25_000 });
    return { groceriesId: groceries.id, diningId: dining.id };
  });

  await showMonth(page, "2026-07", "July 2026");
  const groceries = page.getByTestId(`budget-${groceriesId}`);
  const dining = page.getByTestId(`budget-${diningId}`);
  await expect(groceries.getByRole("textbox", { name: "Budget for Groceries" })).toHaveValue("");
  await expect(page.getByTestId("expected-income")).toContainText("Set");

  await page.getByRole("button", { name: "Copy last month" }).click();
  await expect(page.getByTestId("copy-status")).toHaveText("Copied 2 budgets from June 2026.");
  // Groceries arrived with its amount and its rollover flag; the income too;
  // July's own dining budget is as it was.
  await expect(groceries.getByRole("textbox", { name: "Budget for Groceries" })).toHaveValue(
    "400.00",
  );
  await expect(groceries.getByRole("checkbox", { name: "Roll over Groceries" })).toBeChecked();
  await expect(dining.getByRole("textbox", { name: "Budget for Dining" })).toHaveValue("250.00");
  await expect(dining.getByRole("checkbox", { name: "Roll over Dining" })).not.toBeChecked();
  await expect(page.getByTestId("expected-income")).toContainText("$5,000.00");
  // Nothing is left to copy, so the button says so by refusing.
  await expect(page.getByRole("button", { name: "Copy last month" })).toBeDisabled();

  // Expected income is set from its tile; what is left to budget follows.
  // Groceries rolls over, so June's unspent 400 is July's to spend as well:
  // 6,000 less 800 for groceries and 250 for dining.
  await page.getByRole("button", { name: "Set expected income" }).click();
  const income = page.getByRole("textbox", { name: "Expected income" });
  await income.fill("6000");
  await income.press("Enter");
  await expect(page.getByTestId("expected-income")).toContainText("$6,000.00");
  await expect(groceries.getByTestId("allowance")).toHaveText("$800.00");
  await expect(page.getByTestId("left-to-budget")).toContainText("$4,950.00");
});
