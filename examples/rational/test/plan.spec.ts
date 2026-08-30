import { expect, type Page, test } from "@playwright/test";

/**
 * Budgets, recurring charges, and goals: the forward-looking half.
 *
 * A detected recurrence is a suggestion until the person acts on it, so the
 * test confirms one and watches it become an upcoming bill; a dismissal has
 * to stay dismissed, which is the part a re-detection would get wrong.
 */
const USER = "wren@rational.test";
const PASSWORD = "RationalDemo1!";

async function settle(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

test("a budget rolls over, a recurring charge is confirmed, and a goal is saved towards", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  await page.getByLabel("Email").fill(USER);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await settle(page);

  await page.getByRole("link", { name: "Household" }).click();
  const before = await page.evaluate(() => window.rational.state.currentHouseholdId);
  const creator = page.getByRole("form", { name: "New household" });
  await creator.getByLabel("Name").fill("Planning");
  await creator.getByLabel("Currency").fill("USD");
  await creator.getByRole("button", { name: "Create household" }).click();
  await page.waitForFunction(
    (previous) =>
      window.rational.state.currentHouseholdId !== previous &&
      window.rational.state.currentHouseholdId !== null,
    before,
    { timeout: 30_000 },
  );
  await settle(page);
  await page.waitForFunction(() => window.rational.writes !== null, undefined, { timeout: 30_000 });

  // Three monthly charges, and one month of spending against a category.
  const categoryId = await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 0,
      opening_date: "2026-01-01",
    });
    const category = await writes.createCategory("Subscriptions", "expense");
    for (const [index, date] of ["2026-06-01", "2026-07-01", "2026-08-01"].entries()) {
      await writes.createTransaction({
        account_id: account.id,
        date,
        amount: -1_299,
        currency: "USD",
        description: `STREAMING SERVICE ${index}`,
        category_id: category.id,
        tags: [],
        splits: [],
      });
    }
    return category.id;
  });

  // A budget that rolls over: June's unspent 87.01 carries into July.
  await page.getByRole("link", { name: "Budgets" }).click();
  const setBudget = async (month: string, amount: string) => {
    await page.evaluate(
      async ([id, budgetMonth, budgetAmount]) => {
        const writes = window.rational.writes;
        if (writes === null) throw new Error("no household is open");
        await writes.setBudget({
          category_id: id,
          month: budgetMonth,
          amount: Number(budgetAmount),
          currency: "USD",
          rollover: true,
        });
      },
      [categoryId, month, amount],
    );
  };
  await setBudget("2026-06", "10000");
  await setBudget("2026-07", "10000");
  await page.getByRole("combobox", { name: "Month" }).selectOption("2026-07");
  const row = page.getByTestId(`budget-${categoryId}`);
  await expect(row.getByTestId("carried-in")).toHaveText("$87.01");
  await expect(row.getByTestId("allowance")).toHaveText("$187.01");
  await expect(row.getByTestId("spent")).toHaveText("$12.99");
  await expect(row.getByTestId("remaining")).toHaveText("$174.02");

  // The three charges look monthly; confirming one makes it an upcoming bill.
  await page.getByRole("link", { name: "Plan" }).click();
  const detected = page.getByTestId("detected-streaming-service");
  await expect(detected.getByTestId("interval")).toHaveText("monthly");
  await expect(detected.getByTestId("next")).toHaveText("2026-09-01");
  await detected.getByRole("button", { name: "Confirm" }).click();
  // It leaves the suggestions and appears among the bills.
  await expect(page.getByTestId("detected-streaming-service")).toHaveCount(0);
  await expect(page.getByRole("list", { name: "Upcoming bills" })).toContainText(
    "streaming service",
  );

  // A goal, and a contribution against it.
  const goalForm = page.getByRole("form", { name: "New goal" });
  await goalForm.getByLabel("Name").fill("New bicycle");
  await goalForm.getByLabel("Target").fill("500.00");
  await goalForm.getByRole("button", { name: "Add goal" }).click();
  const goalRow = page.getByRole("row", { name: /New bicycle/u });
  await expect(goalRow.getByTestId("saved")).toHaveText("$0.00");
  page.once("dialog", (dialog) => void dialog.accept("125.00"));
  await goalRow.getByRole("button", { name: "Add" }).click();
  await expect(goalRow.getByTestId("saved")).toHaveText("$125.00");
  await expect(goalRow).toHaveAttribute("data-percent", "25");
});
