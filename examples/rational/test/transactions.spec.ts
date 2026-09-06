import { readFileSync } from "node:fs";

import { expect, type Page, test } from "@playwright/test";

/**
 * The transactions screen against the in-browser fake backend: filters that
 * live in the address, edits over a selection, transfer pairing, and the
 * review queue. Each test signs up a fresh person into the demo household,
 * whose fixed rows (three synced arrivals, paired transfers, a hidden
 * reimbursement) are what several of the assertions lean on.
 */
const EMAIL = "pat.transactions@rational.test";
const PASSWORD = "RationalDemo1!";

async function signInFresh(page: Page): Promise<void> {
  await page.goto("/#/transactions");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("heading", { name: "Transactions" })).toBeVisible();
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await page.waitForFunction(() => window.rational.state.household?.initialSynced === true);
  await page.evaluate(() => window.rational.waitForSync());
}

interface Seed {
  readonly account_id: string;
  readonly date: string;
  readonly amount: number;
  readonly description: string;
  readonly category_id?: string;
  readonly tags?: readonly string[];
  readonly reviewed?: boolean;
  readonly import_batch_id?: string;
}

/** Write rows through the same helpers the screens use; returns their ids in order. */
async function seed(page: Page, rows: readonly Seed[]): Promise<string[]> {
  return page.evaluate(async (entries) => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const ids: string[] = [];
    for (const row of entries) {
      const created = await writes.createTransaction({
        currency: "USD",
        tags: [],
        splits: [],
        ...row,
      });
      ids.push(created.id);
    }
    return ids;
  }, rows);
}

interface WireTransaction {
  readonly id: string;
  readonly account_id: string;
  readonly date: string;
  readonly amount: number;
  readonly category_id?: string;
  readonly tags: readonly string[];
  readonly splits: ReadonlyArray<{ readonly category_id?: string; readonly amount: number }>;
  readonly reviewed?: boolean;
  readonly hidden?: boolean;
  readonly adjustment?: boolean;
  readonly transfer_id?: string;
}

async function storedTransactions(page: Page): Promise<WireTransaction[]> {
  return page.evaluate(async () => {
    const collection = window.rational.household?.session?.collections.transactions;
    if (collection === undefined) throw new Error("transactions are not open");
    const documents = await collection.find().exec();
    return documents.map((document) => document.toJSON() as unknown as WireTransaction);
  });
}

/**
 * The month's cash flow as the cash-flow selector computes it -- counted
 * transactions only, filed by category kind -- so the test can say a pairing
 * changed nothing without depending on the cash-flow screen.
 */
async function cashFlowFor(
  page: Page,
  month: string,
): Promise<{ income: number; spending: number }> {
  return page.evaluate(async (key) => {
    const collections = window.rational.household?.session?.collections;
    const taxonomyCollection = collections?.taxonomy;
    const transactionCollection = collections?.transactions;
    if (taxonomyCollection === undefined || transactionCollection === undefined) {
      throw new Error("no household is open");
    }
    const taxonomy = (await taxonomyCollection.find().exec()).map((document) => document.toJSON());
    const kinds = new Map<string, unknown>();
    for (const entry of taxonomy) {
      if (entry.kind === "category") kinds.set(entry.id, entry.category_kind);
    }
    const transactions = (await transactionCollection.find().exec()).map((document) =>
      document.toJSON(),
    );
    let income = 0;
    let spending = 0;
    for (const transaction of transactions) {
      if (!String(transaction.date).startsWith(key)) continue;
      const transferId = transaction.transfer_id;
      if (
        transaction.hidden === true ||
        transaction.adjustment === true ||
        (typeof transferId === "string" && transferId !== "")
      ) {
        continue;
      }
      const splits = transaction.splits as Array<{ category_id?: string; amount: number }>;
      const effects =
        splits.length === 0
          ? [
              {
                category_id: transaction.category_id as string | undefined,
                amount: transaction.amount as number,
              },
            ]
          : splits;
      for (const effect of effects) {
        const kind = effect.category_id === undefined ? undefined : kinds.get(effect.category_id);
        if (kind === "income") income += effect.amount;
        else if (kind === "expense") spending -= effect.amount;
        else if (effect.amount >= 0) income += effect.amount;
        else spending -= effect.amount;
      }
    }
    return { income, spending };
  }, month);
}

async function balancesByAccount(page: Page): Promise<Record<string, number>> {
  const transactions = await storedTransactions(page);
  const balances: Record<string, number> = {};
  for (const transaction of transactions) {
    balances[transaction.account_id] = (balances[transaction.account_id] ?? 0) + transaction.amount;
  }
  return balances;
}

test("search and filters narrow the list, the summary describes it, and a reload shows the same list", async ({
  page,
}) => {
  await signInFresh(page);
  // Five rows that share a word; each of four fails exactly one filter.
  const [kept] = await seed(page, [
    {
      account_id: "acc_demo_checking",
      date: "2026-07-10",
      amount: -4_200,
      description: "Zebra Print Shop",
      category_id: "cat_demo_shopping",
      tags: ["tag_demo_shared"],
    },
    {
      account_id: "acc_demo_checking",
      date: "2026-07-12",
      amount: -95_000,
      description: "Zebra Print Shop large order",
      category_id: "cat_demo_shopping",
      tags: ["tag_demo_shared"],
    },
    {
      account_id: "acc_demo_checking",
      date: "2026-07-11",
      amount: -4_300,
      description: "Zebra Print Shop",
      category_id: "cat_demo_groceries",
      tags: ["tag_demo_shared"],
    },
    {
      account_id: "acc_demo_checking",
      date: "2026-06-02",
      amount: -4_100,
      description: "Zebra Print Shop",
      category_id: "cat_demo_shopping",
      tags: ["tag_demo_shared"],
    },
    {
      account_id: "acc_demo_checking",
      date: "2026-07-13",
      amount: -4_400,
      description: "Zebra Print Shop",
      category_id: "cat_demo_shopping",
      tags: [],
    },
  ]);
  const zebra = page.locator('tr[data-description^="Zebra Print Shop"]');

  await page.getByLabel("Search transactions").fill("zebra");
  await expect(zebra).toHaveCount(5);
  await expect(page).toHaveURL(/q=zebra/);
  await page
    .getByRole("combobox", { name: "Filter by category" })
    .selectOption({ label: "Shopping" });
  await expect(zebra).toHaveCount(4);
  await page.getByRole("combobox", { name: "Filter by tag" }).selectOption({ label: "shared" });
  await expect(zebra).toHaveCount(3);
  await page.getByLabel("From", { exact: true }).fill("2026-07-01");
  await page.getByLabel("To", { exact: true }).fill("2026-07-31");
  await expect(zebra).toHaveCount(2);
  await page.getByLabel("Min amount").fill("10");
  await page.getByLabel("Max amount").fill("100");
  await expect(zebra).toHaveCount(1);
  await expect(page.getByTestId(`transaction-${kept}`)).toBeVisible();
  await expect(page.getByTestId("transaction-summary")).toHaveText("1 transaction · net -$42.00");
  await expect(page.getByTestId("active-filter-count")).toHaveText("7 filters");

  // Every filter rides in the address.
  const hash = await page.evaluate(() => window.location.hash);
  for (const pair of [
    "q=zebra",
    "category=cat_demo_shopping",
    "tag=tag_demo_shared",
    "from=2026-07-01",
    "to=2026-07-31",
    "min=1000",
    "max=10000",
  ]) {
    expect(hash).toContain(pair);
  }

  // The export is the list as filtered: a header and the one row.
  const downloading = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export CSV" }).click();
  const download = await downloading;
  expect(download.suggestedFilename()).toMatch(/^rational-transactions-\d{4}-\d{2}-\d{2}\.csv$/u);
  const csv = readFileSync(await download.path(), "utf8");
  const lines = csv.trim().split("\r\n");
  expect(lines).toHaveLength(2);
  expect(lines[0]).toBe(
    "date,account,description,merchant,category,group,tags,amount,currency,notes,hidden,transfer_id",
  );
  expect(lines[1]).toContain("2026-07-10,Everyday checking,Zebra Print Shop,");
  expect(lines[1]).toContain(",Shopping,Shopping,shared,-42.00,USD,");

  // The same address opens the same list.
  await page.reload();
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await expect(page.getByRole("heading", { name: "Transactions" })).toBeVisible();
  await expect(page.getByLabel("Search transactions")).toHaveValue("zebra");
  await expect(zebra).toHaveCount(1);
  await expect(page.getByTestId(`transaction-${kept}`)).toBeVisible();
  await expect(page.getByTestId("transaction-summary")).toHaveText("1 transaction · net -$42.00");

  // Clearing the filters clears the address too.
  await page.getByRole("button", { name: "Clear filters" }).click();
  await expect(page).toHaveURL(/#\/transactions$/u);
  await expect(zebra).toHaveCount(5);
});

test("several transactions get a category, a tag, and a review in one action", async ({ page }) => {
  await signInFresh(page);
  // Four arrivals nobody has looked at: three to change, one to leave alone.
  const rows = ["alpha", "beta", "gamma", "delta"].map((name, index) => ({
    account_id: "acc_demo_checking",
    date: `2026-08-0${index + 3}`,
    amount: -1_500 - index * 100,
    description: `Bulk ${name}`,
    reviewed: false,
    import_batch_id: "imp_test_bulk",
  }));
  const [alpha, beta, gamma, delta] = await seed(page, rows);

  await page.getByLabel("Search transactions").fill("bulk");
  await expect(page.locator('tr[data-description^="Bulk"]')).toHaveCount(4);
  await expect(page.getByTestId("marker-needs-review")).toHaveCount(4);
  for (const name of ["Bulk alpha", "Bulk beta", "Bulk gamma"]) {
    await page.getByRole("checkbox", { name: `Select ${name}` }).check();
  }
  const bar = page.getByTestId("bulk-bar");
  await expect(bar.getByTestId("bulk-count")).toHaveText("3 selected");
  await bar.getByRole("combobox", { name: "Bulk category" }).selectOption({ label: "Groceries" });
  await bar.getByRole("combobox", { name: "Bulk tag" }).selectOption({ label: "vacation" });
  await bar.getByRole("combobox", { name: "Bulk review" }).selectOption("reviewed");
  await bar.getByRole("button", { name: "Apply changes" }).click();
  await expect(page.getByTestId("bulk-outcome")).toHaveText("Updated 3 transactions.");
  await expect(page.getByTestId("bulk-bar")).toHaveCount(0);

  for (const id of [alpha, beta, gamma]) {
    const row = page.getByTestId(`transaction-${id}`);
    await expect(row).toContainText("Groceries");
    await expect(row).toContainText("vacation");
    await expect(row.getByTestId("marker-needs-review")).toHaveCount(0);
  }
  const control = page.getByTestId(`transaction-${delta}`);
  await expect(control).not.toContainText("Groceries");
  await expect(control).not.toContainText("vacation");
  await expect(control.getByTestId("marker-needs-review")).toHaveCount(1);

  // The documents say the same as the rows.
  const stored = await storedTransactions(page);
  const byId = new Map(stored.map((transaction) => [transaction.id, transaction]));
  for (const id of [alpha, beta, gamma]) {
    const transaction = byId.get(id ?? "");
    expect(transaction?.category_id).toBe("cat_demo_groceries");
    expect(transaction?.tags).toContain("tag_demo_vacation");
    expect(transaction?.reviewed).toBe(true);
  }
  const untouched = byId.get(delta ?? "");
  expect(untouched?.category_id).toBeUndefined();
  expect(untouched?.tags).toEqual([]);
  expect(untouched?.reviewed).toBeUndefined();
});

test("a transfer is paired from the panel and leaves cash flow", async ({ page }) => {
  await signInFresh(page);
  const before = await cashFlowFor(page, "2026-08");

  // Money moved from checking to the wallet a day apart, booked as two rows.
  const [outflow, inflow] = await seed(page, [
    {
      account_id: "acc_demo_checking",
      date: "2026-08-12",
      amount: -25_000,
      description: "Move to wallet (xfer)",
    },
    {
      account_id: "acc_demo_cash",
      date: "2026-08-13",
      amount: 25_000,
      description: "Top up from checking (xfer)",
    },
  ]);
  // Unpaired, they read as spending on one side and income on the other.
  const during = await cashFlowFor(page, "2026-08");
  expect(during).toEqual({
    income: before.income + 25_000,
    spending: before.spending + 25_000,
  });
  const balancesBefore = await balancesByAccount(page);

  await page.getByLabel("Search transactions").fill("xfer");
  await expect(page.locator('tr[data-description$="(xfer)"]')).toHaveCount(2);
  await page.getByTestId(`transaction-${outflow}`).getByRole("button", { name: "Details" }).click();
  const panel = page.getByTestId("transaction-panel");
  await expect(panel).toHaveAttribute("data-transaction-id", outflow ?? "");
  const suggestion = panel.getByTestId(`suggestion-${inflow}`);
  await expect(suggestion).toContainText("Top up from checking (xfer)");
  await expect(suggestion).toContainText("1 day apart");
  await suggestion.getByRole("button", { name: "Pair" }).click();

  // Both legs name the same transfer, in the panel, in the rows, and in the documents.
  await expect(panel.getByTestId("transfer-other-leg")).toContainText("Top up from checking");
  await expect(
    page.getByTestId(`transaction-${outflow}`).getByTestId("marker-transfer"),
  ).toBeVisible();
  await expect(
    page.getByTestId(`transaction-${inflow}`).getByTestId("marker-transfer"),
  ).toBeVisible();
  const stored = await storedTransactions(page);
  const legs = stored.filter(
    (transaction) => transaction.id === outflow || transaction.id === inflow,
  );
  expect(legs).toHaveLength(2);
  expect(legs[0]?.transfer_id).toMatch(/^trf_/u);
  expect(legs[1]?.transfer_id).toBe(legs[0]?.transfer_id);

  // Cash flow is what it was before the two rows existed; balances did not move.
  expect(await cashFlowFor(page, "2026-08")).toEqual(before);
  expect(await balancesByAccount(page)).toEqual(balancesBefore);

  // A slower transfer the engine will not suggest is paired by hand, and unpaired.
  await panel.getByRole("button", { name: "Close" }).click();
  const [slowOut, slowIn] = await seed(page, [
    {
      account_id: "acc_demo_checking",
      date: "2026-08-01",
      amount: -12_000,
      description: "Slow cheque out (xfer)",
    },
    {
      account_id: "acc_demo_savings",
      date: "2026-08-07",
      amount: 12_000,
      description: "Slow cheque in (xfer)",
    },
  ]);
  await page.getByTestId(`transaction-${slowIn}`).getByRole("button", { name: "Details" }).click();
  await expect(panel).toHaveAttribute("data-transaction-id", slowIn ?? "");
  await expect(panel.getByRole("list", { name: "Transfer suggestions" })).toHaveCount(0);
  await panel.getByRole("combobox", { name: "Pair with" }).selectOption(slowOut ?? "");
  await panel.getByRole("button", { name: "Pair selected" }).click();
  await expect(panel.getByTestId("transfer-other-leg")).toContainText("Slow cheque out (xfer)");
  await expect(
    page.getByTestId(`transaction-${slowOut}`).getByTestId("marker-transfer"),
  ).toBeVisible();
  await panel.getByRole("button", { name: "Unpair" }).click();
  await expect(
    page.getByTestId(`transaction-${slowOut}`).getByTestId("marker-transfer"),
  ).toHaveCount(0);
  await expect(
    page.getByTestId(`transaction-${slowIn}`).getByTestId("marker-transfer"),
  ).toHaveCount(0);
});

test("synced transactions wait in the review queue until someone looks; typed ones never do", async ({
  page,
}) => {
  await signInFresh(page);
  const quick = page.getByTestId("needs-review-filter");
  await expect(quick).toHaveText("Needs review (3)");
  await quick.click();
  await expect(page).toHaveURL(/review=needs/u);
  const rows = page.locator('tr[data-testid^="transaction-"]');
  await expect(rows).toHaveCount(3);
  await expect(page.getByTestId("marker-needs-review")).toHaveCount(3);
  await expect(page.getByTestId("transaction-summary")).toContainText("3 transactions");

  // Looking at one takes it out of the queue.
  const first = rows.first();
  const firstId = (await first.getAttribute("data-testid"))?.replace("transaction-", "") ?? "";
  await first.getByRole("button", { name: "Details" }).click();
  const panel = page.getByTestId("transaction-panel");
  await expect(panel).toHaveAttribute("data-transaction-id", firstId);
  // The box reflects the stored document, so it flips when the write lands
  // rather than on the click itself; `click` rather than `check` for that reason.
  const reviewed = panel.getByLabel("Reviewed", { exact: true });
  await expect(reviewed).not.toBeChecked();
  await reviewed.click();
  await expect(reviewed).toBeChecked();
  await expect(rows).toHaveCount(2);
  await expect(quick).toHaveText("Needs review (2)");
  await expect(page.getByTestId(`transaction-${firstId}`)).toHaveCount(0);
  await panel.getByRole("button", { name: "Close" }).click();

  // Something a member types was looked at by the typing.
  await page.getByRole("button", { name: "New transaction" }).click();
  const editor = page.getByRole("form", { name: "Transaction editor" });
  await editor.getByLabel("Account").selectOption({ label: "Everyday checking" });
  await editor.getByLabel("Amount (USD)").fill("-3.50");
  await editor.getByLabel("Description").fill("Typed by hand");
  await editor.getByRole("button", { name: "Save transaction" }).click();
  await expect(editor).toHaveCount(0);
  await expect(quick).toHaveText("Needs review (2)");
  await expect(page.locator('tr[data-description="Typed by hand"]')).toHaveCount(0);

  await page.getByRole("button", { name: "Clear filters" }).click();
  const typed = page.locator('tr[data-description="Typed by hand"]');
  await expect(typed).toBeVisible();
  await expect(typed.getByTestId("marker-needs-review")).toHaveCount(0);
  await expect(
    page.getByTestId(`transaction-${firstId}`).getByTestId("marker-needs-review"),
  ).toHaveCount(0);
});
