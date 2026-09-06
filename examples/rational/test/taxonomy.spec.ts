import { expect, type Page, test } from "@playwright/test";

/**
 * Categories, merchants, and rules against the in-browser fake backend: the
 * default set a new household is seeded with, a category deleted with its
 * transactions moved, merchants renamed and merged, and rules applied in
 * their listed order with every action they carry.
 */
const PASSWORD = "RationalDemo1!";

/**
 * The household session's first pull is done and the writes are usable. The
 * app object is read through `?.` because the dev server reloads the page
 * whenever a source file changes, and a predicate must survive that.
 */
async function settle(page: Page): Promise<void> {
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  await page.waitForFunction(
    () => window.rational?.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
  await page.waitForFunction(
    () => typeof window.rational?.state.currentHouseholdId === "string",
    undefined,
    { timeout: 30_000 },
  );
  await page.waitForFunction(
    () =>
      window.rational?.household?.session != null &&
      window.rational.state.household?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

/**
 * Sign up a new person. With `fresh`, the fake joins them to no household, so
 * a space of their own is provisioned and seeded -- the real newcomer's path;
 * otherwise they join the demo household, which has data to work on.
 */
async function signUp(page: Page, email: string, options: { fresh: boolean }): Promise<void> {
  if (options.fresh) {
    await page.addInitScript(() => {
      const off = () => {
        if (window.rationalFake !== undefined) window.rationalFake.joinDemoOnSignup = false;
        else setTimeout(off, 0);
      };
      off();
    });
  }
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational?.state.phase !== "starting");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await settle(page);
}

const taxonomyCount = (page: Page) =>
  page.evaluate(async () => {
    const taxonomy = window.rational.household?.session?.collections.taxonomy;
    if (taxonomy === undefined) return -1;
    return (await taxonomy.find().exec()).length;
  });

test("a new household starts with the default categories and is not seeded twice", async ({
  page,
}) => {
  // Two sign-ins, a provisioned space, and a seed: longer than the other tests.
  test.setTimeout(60_000);
  await signUp(page, "fresh.categories@rational.test", { fresh: true });
  const hh = await page.evaluate(() => window.rational.state.currentHouseholdId);
  await page.goto("/#/settings/categories");
  await expect(page.getByRole("heading", { name: "Categories" })).toBeVisible();

  // The default groups, in their order, with their categories under them.
  await expect(page.getByTestId(`group-grp_${hh}.food_and_dining`)).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByTestId(`group-grp_${hh}.housing`)).toBeVisible();
  await expect(page.getByTestId(`group-grp_${hh}.income`)).toBeVisible();
  const food = page.getByTestId(`group-grp_${hh}.food_and_dining`);
  await expect(food.getByTestId(`category-cat_${hh}.groceries`)).toContainText("Groceries");
  await expect(food.getByTestId(`category-cat_${hh}.coffee_shops`)).toContainText("Coffee shops");
  await expect(
    page.getByTestId(`group-grp_${hh}.housing`).getByTestId(`category-cat_${hh}.rent`),
  ).toContainText("Rent");
  const groups = page.locator(`[data-testid^="group-grp_${hh}."]`);
  const firstGroup = await groups.first().getAttribute("data-name");
  expect(firstGroup).toBe("Income");

  // Every seeded document reached the backend, so a second device would pull
  // them rather than seed its own.
  const seeded = await taxonomyCount(page);
  expect(seeded).toBeGreaterThan(0);
  const remote = (id: string) =>
    page.evaluate((id) => window.rationalFake?.remoteDocument("taxonomy", id), id);
  await expect.poll(() => remote(`grp_${hh}.housing`)).toBeDefined();
  await expect.poll(() => remote(`cat_${hh}.groceries`)).toBeDefined();
  await expect.poll(() => remote(`cat_${hh}.postage_and_shipping`)).toBeDefined();

  // Opening the household again seeds nothing. Signing out removes the local
  // database, so signing back in pulls the household from the backend the way
  // a second device would -- and its first pull must find the set complete
  // rather than empty. (A reload would not do: the in-browser fake is a new,
  // empty instance on every page load, which no real deployment is.)
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  await page.getByLabel("Email").fill("fresh.categories@rational.test");
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await settle(page);
  // The address still names the categories page from before signing out; a
  // goto to the very same address would reload the page, and a reload throws
  // the in-page fake away with the household in it. Set the hash instead.
  await page.evaluate(() => {
    window.location.hash = "#/settings/categories";
  });
  await expect(page.getByTestId(`group-grp_${hh}.food_and_dining`)).toBeVisible({
    timeout: 15_000,
  });
  await page.evaluate(() => window.rational.waitForSync());
  expect(await taxonomyCount(page)).toBe(seeded);
  expect(await groups.count()).toBe(
    await page.evaluate(async () => {
      const taxonomy = window.rational.household?.session?.collections.taxonomy;
      if (taxonomy === undefined) return -1;
      return (await taxonomy.find({ selector: { kind: "group" } }).exec()).length;
    }),
  );
});

test("deleting a category moves its transactions and removes its budgets", async ({ page }) => {
  await signUp(page, "fresh.delete@rational.test", { fresh: true });
  const hh = await page.evaluate(() => window.rational.state.currentHouseholdId);
  await page.goto("/#/settings/categories");
  await expect(page.getByTestId(`group-grp_${hh}.travel_and_lifestyle`)).toBeVisible({
    timeout: 15_000,
  });

  // Two categories through the form, in the same group.
  for (const [name, icon] of [
    ["Board games", "🎲"],
    ["Puzzles", "🧩"],
  ]) {
    await page.getByRole("button", { name: "New category" }).click();
    const form = page.getByRole("form", { name: "New category" });
    await form.getByLabel("Name").fill(name ?? "");
    await form.getByLabel("Group").selectOption({ label: "Travel & Lifestyle" });
    await form.getByLabel("Icon").fill(icon ?? "");
    await form.getByRole("button", { name: "Add category" }).click();
    await expect(page.locator(`tr[data-name="${name}"]`)).toBeVisible();
    await page.getByRole("button", { name: "Cancel" }).click();
  }
  const travel = page.getByTestId(`group-grp_${hh}.travel_and_lifestyle`);
  await expect(travel.locator('tr[data-name="Board games"]')).toContainText("🎲");
  const idOf = (name: string) =>
    page
      .locator(`tr[data-name="${name}"]`)
      .evaluate((element) => (element as HTMLElement).dataset.testid?.replace("category-", ""));
  const boardGames = (await idOf("Board games")) ?? "";
  const puzzles = (await idOf("Puzzles")) ?? "";
  expect(boardGames).not.toBe("");
  expect(puzzles).not.toBe("");

  // An account, two transactions in the category -- one filed directly, one
  // only through a split -- and a budget for it.
  const month = new Date().toISOString().slice(0, 7);
  await page.evaluate(
    async ({ categoryId, month }) => {
      const writes = window.rational.writes;
      if (writes === null) throw new Error("no writes");
      const account = await writes.createAccount({
        name: "Everyday",
        type: "checking",
        currency: "USD",
        opening_balance: 100_000,
        opening_date: "2026-01-01",
      });
      await writes.createTransaction({
        account_id: account.id,
        date: `${month}-03`,
        amount: -4_500,
        currency: "USD",
        description: "Game shop",
        category_id: categoryId,
      });
      await writes.createTransaction({
        account_id: account.id,
        date: `${month}-04`,
        amount: -6_000,
        currency: "USD",
        description: "Game shop and snacks",
        splits: [
          { id: "", category_id: categoryId, amount: -4_000 },
          { id: "", amount: -2_000 },
        ],
      });
      await writes.setBudget({
        category_id: categoryId,
        month,
        amount: 10_000,
        currency: "USD",
        rollover: false,
        kind: "category",
      });
    },
    { categoryId: boardGames, month },
  );

  // Delete, choosing where the transactions go.
  await travel
    .locator('tr[data-name="Board games"]')
    .getByRole("button", { name: "Delete" })
    .click();
  const dialog = page.getByRole("dialog", { name: "Delete Board games" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("Move its transactions to").selectOption({
    label: "Travel & Lifestyle › Puzzles",
  });
  await dialog.getByRole("button", { name: "Delete category" }).click();

  const status = page.getByTestId("category-status");
  await expect(status).toContainText("Deleted Board games");
  await expect(status).toContainText("2 transactions moved to Puzzles");
  await expect(status).toContainText("1 budget removed");
  await expect(page.locator('tr[data-name="Board games"]')).toHaveCount(0);
  await expect(travel.locator('tr[data-name="Puzzles"]')).toBeVisible();

  // Every transaction and split names Puzzles; the budget is gone.
  const after = await page.evaluate(
    async ({ from, to }) => {
      const collections = window.rational.household?.session?.collections;
      const store = collections?.transactions;
      const budgetStore = collections?.budgets;
      const taxonomy = collections?.taxonomy;
      if (store === undefined || budgetStore === undefined || taxonomy === undefined) {
        throw new Error("no household is open");
      }
      type Stored = { category_id?: string; splits: Array<{ category_id?: string }> };
      const transactions = (await store.find().exec()).map(
        (document) => document.toJSON() as unknown as Stored,
      );
      const budgets = (await budgetStore.find().exec()).map((document) => document.toJSON());
      return {
        direct: transactions.filter((transaction) => transaction.category_id === to).length,
        stale: transactions.filter(
          (transaction) =>
            transaction.category_id === from ||
            transaction.splits.some((split) => split.category_id === from),
        ).length,
        splitMoved: transactions.some((transaction) =>
          transaction.splits.some((split) => split.category_id === to),
        ),
        budgetsForDeleted: budgets.filter((budget) => budget.category_id === from).length,
        category: await taxonomy.findOne(from).exec(),
      };
    },
    { from: boardGames, to: puzzles },
  );
  expect(after.direct).toBe(1);
  expect(after.stale).toBe(0);
  expect(after.splitMoved).toBe(true);
  expect(after.budgetsForDeleted).toBe(0);
  expect(after.category).toBeNull();
});

test("merchants are renamed and merged, and the transactions follow", async ({ page }) => {
  await signUp(page, "pat.merchants@rational.test", { fresh: false });
  await page.goto("/#/settings/merchants");
  await expect(page.getByRole("heading", { name: "Merchants" })).toBeVisible();

  // Rename a merchant the household named.
  const blueBottle = page.getByTestId("merchant-mer_demo_blue_bottle_coffee");
  await expect(blueBottle).toBeVisible();
  await blueBottle.getByRole("button", { name: "Rename" }).click();
  await blueBottle.getByLabel("Rename Blue Bottle Coffee").fill("Blue Bottle Roasters");
  await blueBottle.getByRole("button", { name: "Save" }).click();
  await expect(blueBottle.getByTestId("name")).toHaveText("Blue Bottle Roasters");

  // The synced "SQ *CORNER GROCER 0412" reads as a Corner Grocer nobody named
  // yet, beside the Corner Grocer the household did name. Fold the unnamed
  // one into the named one, then the named one into Whole Harvest Market.
  const unnamed = page.getByTestId("merchant-corner-grocer");
  await expect(unnamed).toContainText("from statement text");
  await unnamed.getByRole("button", { name: "Merge into…" }).click();
  await unnamed.getByLabel("Merge Corner Grocer into").selectOption({ label: "Corner Grocer" });
  await unnamed.getByRole("button", { name: "Merge", exact: true }).click();
  await expect(page.getByTestId("merchant-status")).toContainText(
    "Merged Corner Grocer into Corner Grocer: 1 transaction refiled.",
  );
  await expect(unnamed).toHaveCount(0);
  const cornerGrocer = page.getByTestId("merchant-mer_demo_corner_grocer");
  await expect(cornerGrocer.getByTestId("count")).toHaveText("1");

  await cornerGrocer.getByRole("button", { name: "Merge into…" }).click();
  await cornerGrocer
    .getByLabel("Merge Corner Grocer into")
    .selectOption({ label: "Whole Harvest Market" });
  await cornerGrocer.getByRole("button", { name: "Merge", exact: true }).click();
  await expect(page.getByTestId("merchant-status")).toContainText(
    "Merged Corner Grocer into Whole Harvest Market: 1 transaction refiled.",
  );
  await expect(cornerGrocer).toHaveCount(0);
  await expect(page.getByTestId("merchant-mer_demo_whole_harvest_market")).toBeVisible();

  // The search narrows the list.
  await page.getByLabel("Search merchants").fill("harvest");
  await expect(page.getByTestId("merchant-mer_demo_whole_harvest_market")).toBeVisible();
  await expect(page.getByTestId("merchant-mer_demo_blue_bottle_coffee")).toHaveCount(0);
  await page.getByLabel("Search merchants").fill("");

  // The merged merchant no longer exists; the winner learned its patterns and
  // holds every transaction that used to resolve to it.
  const documents = await page.evaluate(async () => {
    const collections = window.rational.household?.session?.collections;
    const taxonomy = collections?.taxonomy;
    const store = collections?.transactions;
    if (taxonomy === undefined || store === undefined) throw new Error("no household is open");
    const loser = await taxonomy.findOne("mer_demo_corner_grocer").exec();
    const winner = await taxonomy.findOne("mer_demo_whole_harvest_market").exec();
    const transactions = (await store.find().exec()).map((document) => document.toJSON());
    return {
      loser,
      winnerPatterns: (winner?.toJSON().patterns as string[] | undefined) ?? [],
      withLoser: transactions.filter(
        (transaction) => transaction.merchant_id === "mer_demo_corner_grocer",
      ).length,
      arrival: transactions.find(
        (transaction) => transaction.description === "SQ *CORNER GROCER 0412",
      )?.merchant_id,
    };
  });
  expect(documents.loser).toBeNull();
  expect(documents.winnerPatterns).toEqual(
    expect.arrayContaining(["whole harvest market", "corner grocer", "sq corner grocer"]),
  );
  expect(documents.withLoser).toBe(0);
  expect(documents.arrival).toBe("mer_demo_whole_harvest_market");

  // The transactions list shows the new name and never the merged one.
  await page.goto("/#/transactions");
  await expect(page.getByRole("heading", { name: "Transactions" })).toBeVisible();
  const main = page.getByRole("main");
  await expect(main).toContainText("Blue Bottle Roasters");
  await expect(main.getByText("Corner Grocer", { exact: true })).toHaveCount(0);
});

test("rules apply in their listed order, and a rule sets the merchant and reviews", async ({
  page,
}) => {
  await signUp(page, "pat.rules@rational.test", { fresh: false });
  await page.goto("/#/settings/rules");
  await expect(page.getByRole("heading", { name: "Rules" })).toBeVisible();
  await expect(page.getByTestId("rule-rul_demo_coffee")).toBeVisible();

  // Two rules on the same statement text with different categories: the
  // first through the writes, the second through the editor.
  const first = await page.evaluate(() =>
    window.rational.writes?.createRule({
      name: "Hardware is health",
      match: { description_contains: "hardware store" },
      set_category_id: "cat_demo_health",
      priority: 100,
    }),
  );
  expect(first).toBeDefined();
  await page.getByRole("button", { name: "New rule" }).click();
  const editor = page.getByRole("form", { name: "New rule" });
  await editor.getByLabel("Name").fill("Hardware is fun");
  await editor.getByLabel("Statement text").fill("hardware store");
  await editor.getByLabel("Set category").selectOption({ label: "Entertainment" });
  await editor.getByRole("button", { name: "Save rule" }).click();
  await expect(editor).toHaveCount(0);
  const second = page.locator('tr[data-name="Hardware is fun"]');
  await expect(second).toBeVisible();
  await expect(second.getByTestId("sentence")).toContainText(
    "statement text contains “hardware store”, set category to Entertainment",
  );
  await expect(second.getByTestId("match-count")).toHaveText("2");
  await expect(second.getByTestId("pending")).toHaveText("2");

  // The rows sit in priority order: demo coffee rule, health, fun.
  const names = page.locator('tr[data-testid^="rule-"]');
  await expect(names).toHaveCount(3);
  expect(await names.nth(1).getAttribute("data-name")).toBe("Hardware is health");
  expect(await names.nth(2).getAttribute("data-name")).toBe("Hardware is fun");

  // Move the second above the first; the priorities are rewritten in tens.
  await second.getByRole("button", { name: "Move up" }).click();
  await expect(names.nth(1)).toHaveAttribute("data-name", "Hardware is fun");
  await expect(names.nth(2)).toHaveAttribute("data-name", "Hardware is health");
  const priorities = await page.evaluate(async () => {
    const store = window.rational.household?.session?.collections.rules;
    if (store === undefined) throw new Error("no household is open");
    const rules = (await store.find().exec()).map((document) => document.toJSON());
    return Object.fromEntries(rules.map((rule) => [String(rule.name), Number(rule.priority)]));
  });
  expect(priorities["Hardware is fun"]).toBeLessThan(priorities["Hardware is health"] ?? 0);

  // Every rule in order: the moved rule is the one that files the transactions.
  await page.getByRole("button", { name: "Apply all rules" }).click();
  await expect(page.getByTestId("rule-applied")).toContainText("changed 2 transactions");
  const secondId = (await second.getAttribute("data-testid"))?.replace("rule-", "");
  const filed = await page.evaluate(async () => {
    const store = window.rational.household?.session?.collections.transactions;
    if (store === undefined) throw new Error("no household is open");
    return (await store.find({ selector: { description: "Hardware Store" } }).exec()).map(
      (document) => {
        const transaction = document.toJSON();
        return { category: transaction.category_id, rule: transaction.rule_id };
      },
    );
  });
  expect(filed).toHaveLength(2);
  for (const transaction of filed) {
    expect(transaction.category).toBe("cat_demo_entertainment");
    expect(transaction.rule).toBe(secondId);
  }
  await expect(second.getByTestId("pending")).toHaveText("0");

  // A rule that sets the merchant and marks reviewed, applied to a synced
  // arrival: it gets the merchant, leaves the review queue, and names the rule.
  await page.getByRole("button", { name: "New rule" }).click();
  await editor.getByLabel("Name").fill("Uber rides");
  await editor.getByLabel("Statement text").fill("uber");
  await editor.getByLabel("Set merchant").selectOption({ label: "Ride Share" });
  await editor.getByRole("checkbox", { name: "Mark reviewed" }).check();
  await editor.getByRole("button", { name: "Save rule" }).click();
  const uber = page.locator('tr[data-name="Uber rides"]');
  await expect(uber.getByTestId("sentence")).toContainText(
    "set merchant to Ride Share, mark it reviewed",
  );
  await expect(uber.getByTestId("match-count")).toHaveText("1");
  await uber.getByRole("button", { name: "Apply" }).click();
  await expect(page.getByTestId("rule-applied")).toContainText("Uber rides changed 1 transaction");
  const uberId = (await uber.getAttribute("data-testid"))?.replace("rule-", "");
  const arrival = await page.evaluate(async () => {
    const store = window.rational.household?.session?.collections.transactions;
    if (store === undefined) throw new Error("no household is open");
    const documents = await store.find({ selector: { description: "UBER *TRIP 4X2" } }).exec();
    return documents[0]?.toJSON() ?? null;
  });
  expect(arrival).not.toBeNull();
  expect(arrival?.external_id).toBe("sim-demo-2");
  expect(arrival?.merchant_id).toBe("mer_demo_ride_share");
  expect(arrival?.reviewed).toBe(true);
  expect(arrival?.rule_id).toBe(uberId);
  await expect(uber.getByTestId("pending")).toHaveText("0");

  // Disabling takes a rule out of the count; deleting removes it.
  await uber.getByRole("button", { name: "Disable" }).click();
  await expect(uber).toContainText("disabled");
  await expect(uber.getByTestId("match-count")).toHaveText("0");
  await uber.getByRole("button", { name: "Delete" }).click();
  await expect(uber).toHaveCount(0);
});
