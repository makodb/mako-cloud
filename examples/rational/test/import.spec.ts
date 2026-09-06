import { expect, type Page, test } from "@playwright/test";

/**
 * Importing a bank export, and a rule filing what arrives.
 *
 * Nothing is written until the person has seen what would be, so this drives
 * the whole path: choose an account and a file, confirm the columns the file
 * suggested, read the preview, and import. A row already stored is offered as
 * a duplicate rather than written twice, and a row that cannot be read is
 * named by its line rather than dropped. What is imported arrives needing
 * review -- nobody has looked at the rows, only at the file.
 */
const USER = "kit@rational.test";
const PASSWORD = "RationalDemo1!";

const EXPORT = [
  "Posted Date,Payee,Amount",
  "08/02/2026,CORNER MARKET #1234,-42.50",
  '08/03/2026,"COFFEE, LARGE",-4.25',
  "08/04/2026,SALARY,2500.00",
  "not-a-date,,x",
].join("\n");

async function settle(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

/** Settings is a hub: the sidebar entry first, then the page in its own navigation. */
async function openSettingsPage(page: Page, label: string): Promise<void> {
  await page
    .getByRole("navigation", { name: "Sections" })
    .getByRole("link", { name: "Settings" })
    .click();
  await page
    .getByRole("navigation", { name: "Settings pages" })
    .getByRole("link", { name: label, exact: true })
    .click();
}

test("a bank export is previewed, deduplicated, categorized by a rule, and imported", async ({
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

  await openSettingsPage(page, "Members");
  const before = await page.evaluate(() => window.rational.state.currentHouseholdId);
  const creator = page.getByRole("form", { name: "New household" });
  await creator.getByLabel("Name").fill("Import test");
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
  // Creating a household re-opens the scope, so the write helpers appear a
  // moment after the directory settles. Waiting for them is what keeps this
  // from being a race.
  await page.waitForFunction(() => window.rational.writes !== null, undefined, {
    timeout: 30_000,
  });

  // An account, a category, and one transaction the file will repeat.
  await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 0,
      opening_date: "2026-01-01",
    });
    const category = await writes.createCategory("Groceries", "expense");
    await writes.createTransaction({
      account_id: account.id,
      date: "2026-08-03",
      amount: -425,
      currency: "USD",
      description: "Coffee large",
      tags: [],
      splits: [],
    });
    await writes.createRule({
      name: "Market is groceries",
      match: { description_contains: "market" },
      set_category_id: category.id,
      priority: 10,
    });
  });

  await openSettingsPage(page, "Import");
  await expect(page.getByRole("heading", { name: "Import", exact: true })).toBeVisible();
  await page.getByLabel("Account").selectOption({ label: "Everyday" });
  await page.getByLabel("CSV file").setInputFiles({
    name: "export.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(EXPORT),
  });

  // The columns the file suggested, and the order its dates are in.
  const column = (name: string) => page.getByRole("combobox", { name, exact: true });
  await expect(column("date")).toHaveValue("Posted Date");
  await expect(column("description")).toHaveValue("Payee");
  await expect(column("amount")).toHaveValue("Amount");
  await expect(column("date order")).toHaveValue("MDY");

  // Line 3 repeats the stored coffee; line 5 cannot be read.
  await expect(page.getByTestId("import-summary")).toHaveText(
    "2 to import, 1 already here, 1 unreadable.",
  );
  await expect(page.getByTestId("preview-3").getByTestId("outcome")).toHaveText("already here");
  await expect(page.getByTestId("preview-5").getByTestId("outcome")).toHaveText(
    "the date could not be read",
  );

  await page.getByRole("button", { name: "Import 2 transactions" }).click();
  const outcome = page.getByTestId("import-outcome");
  await expect(outcome).toHaveText(
    "Imported 2 of 4 rows; 1 were already here. 2 are waiting in Needs review.",
  );
  // The outcome leads to the review queue itself, not to a description of it.
  await expect(outcome.getByRole("link", { name: "Needs review" })).toHaveAttribute(
    "href",
    "#/transactions?review=needs",
  );

  // The rule filed the market transaction as it arrived, and recorded itself;
  // neither imported row is reviewed, because nobody has looked at them yet,
  // while the one typed by hand was reviewed by the typing.
  const stored = await page.evaluate(async () => {
    const collections = window.rational.household?.session?.collections;
    if (collections === undefined) throw new Error("no household is open");
    const transactions = collections.transactions;
    if (transactions === undefined) throw new Error("no transactions collection");
    const documents = await transactions.find().exec();
    return documents.map((document) => {
      const transaction = document.toJSON();
      return {
        description: transaction.description,
        category: transaction.category_id,
        rule: transaction.rule_id,
        batch: transaction.import_batch_id,
        reviewed: transaction.reviewed,
      };
    });
  });
  const filed = stored.filter((entry) => entry.description === "CORNER MARKET #1234");
  expect(filed).toHaveLength(1);
  expect(filed[0]?.category).toBeTruthy();
  expect(filed[0]?.rule).toBeTruthy();
  expect(filed[0]?.batch).toBeTruthy();
  expect(filed[0]?.reviewed).toBeUndefined();
  expect(stored.find((entry) => entry.description === "SALARY")?.reviewed).toBeUndefined();
  expect(stored.find((entry) => entry.description === "Coffee large")?.reviewed).toBe(true);

  // Re-importing the same file writes nothing: every row is now already here.
  await page.getByLabel("CSV file").setInputFiles({
    name: "export.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(EXPORT),
  });
  await expect(page.getByTestId("import-summary")).toHaveText(
    "0 to import, 3 already here, 1 unreadable.",
  );

  // The batch is on record under Connections, with what it did.
  await openSettingsPage(page, "Connections");
  await expect(page.getByRole("list", { name: "Imports" })).toContainText(
    "export.csv into Everyday — 2 of 4 rows, 1 already here",
  );
});
