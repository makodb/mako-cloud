import { expect, type Page, test } from "@playwright/test";

/**
 * Goals: saving up and paying down.
 *
 * A save-up goal may follow the balance of the accounts it is linked to, in
 * which case a deposit into one of them is progress and the plan judges that
 * progress against what the planned contributions would have reached by now.
 * A pay-down goal follows a liability and projects the month the planned
 * payment clears it -- and, given a date, says what payment the date needs.
 * Goals are ranked by hand, and a contribution written against one moves it.
 *
 * Dates are computed from today, because the arithmetic counts months from
 * now; the amounts are chosen so no percentage lands on a half.
 */
const PASSWORD = "RationalDemo1!";

const TODAY = new Date().toISOString().slice(0, 10);
const THIS_MONTH = TODAY.slice(0, 7);

// Each test signs up, provisions a space, and walks a whole goal; on a busy
// host that is more than the suite's default half minute.
test.setTimeout(90_000);

/** `2026-11` plus 3 is `2027-02`, as the app's own `addMonths` has it. */
function shiftMonth(month: string, count: number): string {
  const [year = 0, index = 1] = month.split("-").map(Number);
  const total = year * 12 + (index - 1) + count;
  const resultYear = Math.floor(total / 12);
  const resultMonth = total - resultYear * 12 + 1;
  return `${resultYear}-${String(resultMonth).padStart(2, "0")}`;
}

/**
 * Sign up as somebody new who belongs to no household, and wait for the
 * space Rational provisions for them to open with its writes ready.
 */
async function openFreshSpace(page: Page, email: string): Promise<void> {
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
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await page.waitForFunction(
    () =>
      window.rational.state.directory?.initialSynced === true &&
      window.rational.state.currentHouseholdId !== null &&
      window.rational.state.household?.initialSynced === true &&
      window.rational.writes !== null,
    undefined,
    { timeout: 30_000 },
  );
}

async function openGoals(page: Page): Promise<void> {
  await page.getByRole("link", { name: "Goals" }).click();
  await expect(page.getByRole("heading", { name: "Goals" })).toBeVisible();
}

/** Book money on an account, dated today. */
async function book(page: Page, accountId: string, amount: number, text: string): Promise<void> {
  await page.evaluate(
    async ([id, minor, description, date]) => {
      const writes = window.rational.writes;
      if (writes === null) throw new Error("no household is open");
      await writes.createTransaction({
        account_id: String(id),
        date: String(date),
        amount: Number(minor),
        currency: "USD",
        description: String(description),
        tags: [],
        splits: [],
      });
    },
    [accountId, amount, text, TODAY] as const,
  );
}

test("a save-up goal follows its linked savings account and judges itself against the plan", async ({
  page,
}) => {
  await openFreshSpace(page, "juno@rational.test");
  // The account already holds money: the goal must start at zero and measure
  // what is saved from here, not claim the thousand as progress.
  const accountId = await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Rainy day",
      type: "savings",
      currency: "USD",
      opening_balance: 100_000,
      opening_date: "2026-01-01",
    });
    return account.id;
  });

  await openGoals(page);
  await page.getByRole("button", { name: "New goal" }).click();
  const form = page.getByRole("form", { name: "New goal" });
  await form.getByLabel("Kind").selectOption("save");
  await form.getByLabel("Name").fill("Rainy day fund");
  await form.getByLabel("Target (USD)").fill("6000.00");
  await form.getByLabel("Planned monthly (USD)").fill("500.00");
  await form.getByLabel("Accounts").selectOption({ label: "Rainy day" });
  await form.getByLabel("Progress").selectOption("balance");
  await expect(form).toContainText("The linked balance is $1,000.00 today");
  await form.getByRole("button", { name: "Add goal" }).click();
  await expect(page.getByRole("form", { name: "New goal" })).toHaveCount(0);

  const card = page.getByRole("article", { name: "Rainy day fund" });
  await expect(card).toHaveAttribute("data-percent", "0");
  await expect(card.getByTestId("kind")).toHaveText("save-up");
  await expect(card.getByTestId("source")).toHaveText("follows the balance");
  await expect(card.getByTestId("saved")).toHaveText("$0.00");
  await expect(card.getByTestId("remaining")).toHaveText("$6,000.00");
  await expect(card.getByTestId("target")).toHaveText("$6,000.00");
  await expect(card.getByTestId("monthly")).toHaveText("$500.00");
  await expect(card.getByTestId("on-track")).toHaveText("on track");
  await expect(card.getByTestId("linked")).toContainText("Rainy day");
  await expect(page.getByTestId("planned-monthly")).toHaveText("$500.00");

  // A deposit into the linked account is progress, by exactly the deposit.
  await book(page, accountId, 60_000, "Transfer from checking");
  await expect(card.getByTestId("saved")).toHaveText("$600.00");
  await expect(card.getByTestId("remaining")).toHaveText("$5,400.00");
  await expect(card).toHaveAttribute("data-percent", "10");

  // Four months into the plan, 500 a month should have reached 2,000: the
  // goal was written from another device four months ago, and 600 is behind.
  const goalId = (await card.getAttribute("data-testid"))?.replace("goal-", "") ?? "";
  expect(goalId).not.toBe("");
  await expect
    .poll(
      () =>
        page.evaluate(
          (id) => window.rationalFake?.remoteDocument("goals", id) !== undefined,
          goalId,
        ),
      { timeout: 15_000 },
    )
    .toBe(true);
  const now = new Date(`${TODAY}T00:00:00Z`);
  const fourMonthsAgo = Date.UTC(now.getUTCFullYear(), now.getUTCMonth() - 4, 1);
  await page.evaluate(
    ([id, createdAt]) => {
      const fake = window.rationalFake;
      if (fake === undefined) throw new Error("the fake backend is not installed");
      const existing = fake.remoteDocument("goals", String(id));
      if (existing === undefined) throw new Error("the goal never replicated");
      fake.putRemote("goals", {
        ...existing,
        created_at: Number(createdAt),
        updated_at: existing.updated_at + 1_000,
      });
    },
    [goalId, fourMonthsAgo] as const,
  );
  await expect(card.getByTestId("on-track")).toHaveText("behind");
  await expect(card).toContainText("$2,000.00 expected by now");

  // Another deposit brings it past what the plan expected.
  await book(page, accountId, 150_000, "Bonus");
  await expect(card.getByTestId("saved")).toHaveText("$2,100.00");
  await expect(card).toHaveAttribute("data-percent", "35");
  await expect(card.getByTestId("on-track")).toHaveText("on track");
});

test("a pay-down goal shows what is owed, when the planned payment clears it, and what a date needs", async ({
  page,
}) => {
  await openFreshSpace(page, "juno.owes@rational.test");
  const loanId = await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 50_000,
      opening_date: "2026-01-01",
    });
    const loan = await writes.createAccount({
      name: "Car loan",
      type: "loan",
      currency: "USD",
      opening_balance: -1_450_000,
      opening_date: "2026-01-01",
    });
    return loan.id;
  });

  await openGoals(page);
  await page.getByRole("button", { name: "New goal" }).click();
  const form = page.getByRole("form", { name: "New goal" });
  await form.getByLabel("Kind").selectOption("pay_down");
  // Only a liability can be paid down: the checking account is not offered.
  await expect(form.getByLabel("Accounts").locator("option")).toHaveCount(1);
  await form.getByLabel("Accounts").selectOption({ label: "Car loan" });
  await expect(form).toContainText("Owed today: $14,500.00");
  await form.getByLabel("Name").fill("Pay off the car");
  await form.getByLabel("Planned monthly (USD)").fill("385.00");
  // The target is left blank: paying it all off is what a pay-down goal means.
  await form.getByRole("button", { name: "Add goal" }).click();

  const card = page.getByRole("article", { name: "Pay off the car" });
  await expect(card.getByTestId("kind")).toHaveText("pay-down");
  await expect(card.getByTestId("saved")).toHaveText("$0.00");
  await expect(card.getByTestId("remaining")).toHaveText("$14,500.00");
  await expect(card.getByTestId("target")).toHaveText("$14,500.00");
  await expect(card.getByTestId("monthly")).toHaveText("$385.00");
  await expect(card.getByTestId("on-track")).toHaveText("on track");
  // ceil(14,500 / 385) is 38 payments from this month.
  await expect(card.getByTestId("payoff")).toHaveAttribute("datetime", shiftMonth(THIS_MONTH, 38));
  await expect(card.getByTestId("linked")).toContainText("Car loan");

  // A payment on the loan is progress, and brings the payoff a month closer.
  await book(page, loanId, 38_500, "Payment");
  await expect(card.getByTestId("saved")).toHaveText("$385.00");
  await expect(card.getByTestId("remaining")).toHaveText("$14,115.00");
  await expect(card).toHaveAttribute("data-percent", "3");
  await expect(card.getByTestId("payoff")).toHaveAttribute("datetime", shiftMonth(THIS_MONTH, 37));

  // Given a date twelve months out, the goal says what each month must pay
  // to make it: 14,115 over twelve months.
  await card.getByRole("button", { name: "Edit" }).click();
  const editor = page.getByRole("form", { name: "Edit Pay off the car" });
  await editor.getByLabel("By").fill(`${shiftMonth(THIS_MONTH, 11)}-01`);
  await editor.getByRole("button", { name: "Save goal" }).click();
  await expect(page.getByRole("form", { name: "Edit Pay off the car" })).toHaveCount(0);
  await expect(card.getByTestId("target-date")).toHaveText(`${shiftMonth(THIS_MONTH, 11)}-01`);
  await expect(card.getByTestId("monthly")).toHaveText("$1,176.25");
  await expect(card).toContainText("planned $385.00");
});

test("goals are ranked by hand and a recorded contribution advances one", async ({ page }) => {
  await openFreshSpace(page, "juno.ranks@rational.test");
  await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    await writes.createGoal({
      name: "New bicycle",
      target_amount: 50_000,
      currency: "USD",
      kind: "save",
      priority: 0,
      progress_source: "contributions",
    });
    await writes.createGoal({
      name: "Winter coat",
      target_amount: 20_000,
      currency: "USD",
      kind: "save",
      priority: 1,
      progress_source: "contributions",
    });
  });

  await openGoals(page);
  const cards = page.getByTestId("active-goals").getByRole("article");
  await expect(cards).toHaveCount(2);
  await expect(cards.nth(0)).toHaveAttribute("aria-label", "New bicycle");
  await expect(cards.nth(1)).toHaveAttribute("aria-label", "Winter coat");
  // The first cannot move up and the last cannot move down.
  await expect(cards.nth(0).getByRole("button", { name: "Move up" })).toBeDisabled();
  await expect(cards.nth(1).getByRole("button", { name: "Move down" })).toBeDisabled();

  await cards.nth(1).getByRole("button", { name: "Move up" }).click();
  await expect(cards.nth(0)).toHaveAttribute("aria-label", "Winter coat");
  await expect(cards.nth(1)).toHaveAttribute("aria-label", "New bicycle");
  // The order is stored as positions, one write per goal, so the second write
  // may still be landing when the first has already flipped the list.
  await expect
    .poll(() =>
      page.evaluate(async () => {
        const collection = window.rational.household?.session?.collections.goals;
        if (collection === undefined) throw new Error("goals are not open");
        const documents = await collection.find().exec();
        return Object.fromEntries(
          documents.map((document) => [document.toJSON().name, document.toJSON().priority]),
        );
      }),
    )
    .toEqual({ "Winter coat": 0, "New bicycle": 1 });

  // A contribution against a goal that counts them: a quarter of the way.
  const bicycle = page.getByRole("article", { name: "New bicycle" });
  await expect(bicycle.getByTestId("source")).toHaveText("by contributions");
  await expect(bicycle.getByTestId("saved")).toHaveText("$0.00");
  await expect(bicycle.getByTestId("on-track")).toHaveText("no plan");
  await bicycle.getByText(/^Contributions/u).click();
  const contribute = page.getByRole("form", { name: "Add contribution to New bicycle" });
  await contribute.getByLabel("Amount (USD)").fill("125.00");
  await contribute.getByLabel("Note").fill("birthday money");
  await contribute.getByRole("button", { name: "Add contribution" }).click();
  await expect(bicycle.getByTestId("saved")).toHaveText("$125.00");
  await expect(bicycle.getByTestId("remaining")).toHaveText("$375.00");
  await expect(bicycle).toHaveAttribute("data-percent", "25");
  await expect(bicycle.getByRole("table", { name: "Contributions to New bicycle" })).toContainText(
    "birthday money",
  );
  await expect(bicycle).toContainText("Contributions (1, $125.00)");
});
