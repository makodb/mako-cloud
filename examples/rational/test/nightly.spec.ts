import { expect, type Page, test } from "@playwright/test";

/**
 * What the household sees the morning after.
 *
 * The nightly job runs on the server: it files transactions with the
 * household's rules, notices repeating charges, and records the day's net
 * worth. None of that is visible until replication delivers it, so this test
 * writes exactly what the job writes -- through the fake backend, as another
 * writer -- and then looks at Rational the way a person would after opening
 * it in the morning.
 *
 * The point is that the automatic work is legible: a category that appeared
 * overnight says which rule chose it, a suggestion says it was not this
 * device that noticed it, and the net-worth history is drawn from what the
 * snapshots recorded rather than recomputed from today.
 */
const USER = "rook@rational.test";
const PASSWORD = "RationalDemo1!";

async function settle(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

test("the morning after: a filed transaction, a noticed bill, and a net-worth history", async ({
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
  await creator.getByLabel("Name").fill("Overnight");
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

  // An account, a category, a rule, and a transaction nobody filed.
  const seeded = await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 250_000,
      opening_date: "2026-01-01",
    });
    const category = await writes.createCategory("Coffee", "expense");
    const rule = await writes.createRule({
      name: "Coffee shops",
      match: { description_contains: "blue bottle" },
      set_category_id: String(category.id),
      add_tags: [],
      priority: 10,
      enabled: true,
    });
    const transaction = await writes.createTransaction({
      account_id: account.id,
      date: "2026-08-28",
      amount: -1_250,
      currency: "USD",
      description: "BLUE BOTTLE #221",
      tags: [],
      splits: [],
    });
    return {
      accountId: String(account.id),
      categoryId: String(category.id),
      ruleId: String(rule.id),
      transactionId: String(transaction.id),
      householdId: String(window.rational.state.currentHouseholdId),
    };
  });

  // Before the job runs, nothing says who filed it -- because nobody did.
  await page.getByRole("link", { name: "Transactions" }).click();
  const row = page.locator(`tr[data-testid="transaction-${seeded.transactionId}"]`);
  await expect(row).toBeVisible();
  await expect(row.getByTestId("filed-by")).toHaveCount(0);

  // The night's work, written server-side exactly as the function writes it.
  await page.evaluate(async (context) => {
    const fake = window.rationalFake;
    if (fake === undefined) throw new Error("the fake backend is not installed");
    const stamp = Date.now();
    const existing = fake.remoteDocument("transactions", context.transactionId);
    if (existing === undefined) throw new Error("the transaction never replicated");
    fake.putRemote("transactions", {
      ...existing,
      updated_at: stamp,
      category_id: context.categoryId,
      rule_id: context.ruleId,
    });
    for (const [index, day] of ["2026-08-28", "2026-08-29", "2026-08-30"].entries()) {
      fake.putRemote("net_worth_snapshots", {
        id: `nws_${context.householdId}.${day}`,
        household_id: context.householdId,
        created_at: stamp,
        updated_at: stamp + index,
        date: day,
        assets: 250_000 + index * 10_000,
        liabilities: 0,
        net_worth: 250_000 + index * 10_000,
        currency: "USD",
      });
    }
    fake.putRemote("recurrences", {
      id: `rec_${context.householdId}.${context.accountId}.1a2b3c4d`,
      household_id: context.householdId,
      created_at: stamp,
      updated_at: stamp,
      account_id: context.accountId,
      normalized_description: "city power",
      interval: "monthly",
      expected_amount: -8_400,
      currency: "USD",
      next_date: "2026-09-15",
      last_date: "2026-08-15",
      status: "detected",
      matched_count: 4,
    });
  }, seeded);

  // The category arrived overnight, and says which rule chose it.
  await expect(row.getByTestId("filed-by")).toHaveText("by Coffee shops");

  // The history is the snapshots' own, drawn as a line with the change spelled out.
  await page.getByRole("link", { name: "Reports" }).click();
  const history = page.getByTestId("net-worth-history-USD");
  await expect(history).toBeVisible();
  await expect(history.getByTestId("net-worth-change")).toHaveText(
    "3 snapshots, 2026-08-28 to 2026-08-30: up $200.00.",
  );

  // The bill the job noticed is a suggestion, and says it was not this device.
  await page.getByRole("link", { name: "Plan" }).click();
  const suggestion = page.getByTestId("detected-city-power");
  await expect(suggestion).toHaveAttribute("data-noticed-by", "the nightly job");
  await expect(suggestion.getByTestId("next")).toHaveText("2026-09-15");

  // Confirming it settles the job's own document rather than writing a second one.
  await suggestion.getByRole("button", { name: "Confirm" }).click();
  await expect(page.getByTestId("detected-city-power")).toHaveCount(0);
  await expect(page.getByRole("list", { name: "Upcoming bills" })).toContainText("city power");
  const recurrences = await page.evaluate(async () => {
    const collection = window.rational.household?.session?.collections.recurrences;
    if (collection === undefined) throw new Error("recurrences are not open");
    const documents = await collection.find().exec();
    return documents.map((document) => document.toJSON());
  });
  expect(recurrences).toHaveLength(1);
  expect(recurrences[0]?.status).toBe("confirmed");
});
