import { readFile } from "node:fs/promises";

import { type Download, expect, type Page, test } from "@playwright/test";

/**
 * The settings hub against the in-browser fake, over the demo household: the
 * data export, the budget-mode switch, and the notification kinds that are
 * not a money threshold.
 *
 * The export is asserted from the file itself. Playwright intercepts the
 * download the browser would have saved, so the test reads the bytes the
 * household would have opened in a spreadsheet: the header row, a row it
 * knows, and how many rows there are.
 */
const EMAIL = "settings@rational.test";
const PASSWORD = "RationalDemo1!";

/**
 * The columns `transactionsCsv` and `accountsCsv` write, spelled out here on
 * purpose: the file is a way out of Rational, so its shape is asserted from
 * outside the app rather than read back from the module that produces it.
 */
const TRANSACTION_CSV_COLUMNS = [
  "date",
  "account",
  "description",
  "merchant",
  "category",
  "group",
  "tags",
  "amount",
  "currency",
  "notes",
  "hidden",
  "transfer_id",
];
const ACCOUNT_CSV_COLUMNS = [
  "name",
  "class",
  "type",
  "currency",
  "balance",
  "institution",
  "hidden",
  "closed",
];

async function signInFresh(page: Page): Promise<void> {
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => window.rational.state.households[0]?.name ?? ""))
    .toBe("Demo household");
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await page.evaluate(() => window.rational.waitForSync());
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

/**
 * The household's budget mode as the app's state carries it. The browser
 * globals declare only what every suite reads of a household, so the field is
 * read off the document in the page rather than widened for one test.
 */
function budgetMode(page: Page): Promise<string> {
  return page.evaluate(() => {
    const household = window.rational.state.households[0] as { budget_mode?: unknown } | undefined;
    return typeof household?.budget_mode === "string" ? household.budget_mode : "";
  });
}

/** The rows of a CSV download, RFC 4180's CRLF and the trailing line break dropped. */
async function downloadedLines(download: Download): Promise<string[]> {
  const path = await download.path();
  const text = await readFile(path, "utf8");
  return text.split("\r\n").filter((line) => line !== "");
}

test("the household's transactions and accounts are exported as CSV files", async ({ page }) => {
  await signInFresh(page);
  await openSettingsPage(page, "Data");
  await expect(page.getByRole("heading", { name: "Data" })).toBeVisible();
  // The preview says what the file will hold before anything is downloaded.
  await expect(page.getByTestId("transactions-row-count")).toHaveText("60 rows");
  await expect(page.getByTestId("accounts-row-count")).toHaveText("7 rows");

  const [transactions] = await Promise.all([
    page.waitForEvent("download"),
    page.getByRole("button", { name: "Export transactions" }).click(),
  ]);
  expect(transactions.suggestedFilename()).toMatch(
    /^rational-transactions-\d{4}-\d{2}-\d{2}\.csv$/u,
  );
  const transactionLines = await downloadedLines(transactions);
  expect(transactionLines[0]).toBe(TRANSACTION_CSV_COLUMNS.join(","));
  expect(transactionLines).toHaveLength(61);
  // Names stand in for ids: the account, the merchant, the category and its
  // group; the amount is decimal text in the row's own currency.
  const payroll = transactionLines.find((line) => line.includes("ACME Corp payroll"));
  expect(payroll).toBeDefined();
  expect(payroll).toContain(",Everyday checking,ACME Corp payroll,ACME Corp,Salary,Income,");
  expect(payroll).toContain(",5200.00,USD,");
  expect(payroll?.endsWith(",false,")).toBe(true);
  // The transfer legs name their pairing, so a spreadsheet can leave them out too.
  expect(
    transactionLines
      .filter((line) => line.includes("Transfer to savings"))
      .every((line) => /,trf_demo_savings_\d+$/u.test(line)),
  ).toBe(true);

  const [accounts] = await Promise.all([
    page.waitForEvent("download"),
    page.getByRole("button", { name: "Export accounts" }).click(),
  ]);
  expect(accounts.suggestedFilename()).toMatch(/^rational-accounts-\d{4}-\d{2}-\d{2}\.csv$/u);
  const accountLines = await downloadedLines(accounts);
  expect(accountLines[0]).toBe(ACCOUNT_CSV_COLUMNS.join(","));
  expect(accountLines).toHaveLength(8);
  // Every row is one of the demo household's seven accounts and nothing else's.
  expect(accountLines.slice(1).map((line) => line.split(",")[0])).toEqual([
    "Brokerage",
    "Car loan",
    "Everyday checking",
    "Maple Street house",
    "Rainy-day savings",
    "Rewards card",
    "Wallet",
  ]);
  expect(accountLines).toContainEqual(
    expect.stringMatching(
      /^Everyday checking,Cash,Checking,USD,[-\d.]+,First Rational Bank,false,false$/u,
    ),
  );
  expect(accountLines).toContainEqual(
    expect.stringMatching(/^Car loan,Loans,Loan,USD,-[\d.]+,Rational Auto Finance,false,false$/u),
  );
});

test("the household switches budget mode from its settings and back", async ({ page }) => {
  await signInFresh(page);
  const readBudgets = () =>
    page.evaluate(async () => {
      const collection = window.rational.household?.session?.collections.budgets;
      if (collection === undefined) throw new Error("no household is open");
      const documents = await collection.find().exec();
      return documents
        .map((document) => document.toJSON())
        .map((budget) => [budget.id, budget.amount, budget.rollover] as const)
        .sort((left, right) => left[0].localeCompare(right[0]));
    });
  const budgetsBefore = await readBudgets();
  expect(budgetsBefore.length).toBeGreaterThan(0);

  await openSettingsPage(page, "Household");
  const form = page.getByRole("form", { name: "Household settings" });
  const mode = form.getByTestId("budget-mode");
  await expect(mode).toHaveValue("category");
  await mode.selectOption("flex");
  await form.getByRole("button", { name: "Save settings" }).click();
  await expect(page.getByTestId("household-saved")).toBeVisible();
  await expect.poll(() => budgetMode(page)).toBe("flex");
  // The change is the household document's, so it replicates like any other
  // -- the owner may write it -- and every device presents the budget the same way.
  await expect
    .poll(() =>
      page.evaluate(
        () => window.rationalFake?.remoteDocument("households", "hh_demo")?.budget_mode ?? "",
      ),
    )
    .toBe("flex");
  // The mode is a presentation: not one budget changed.
  expect(await readBudgets()).toEqual(budgetsBefore);

  // The form reads the saved mode back, and switches back the same way.
  await expect(form.getByTestId("budget-mode")).toHaveValue("flex");
  await form.getByTestId("budget-mode").selectOption("category");
  await form.getByRole("button", { name: "Save settings" }).click();
  await expect.poll(() => budgetMode(page)).toBe("category");
  expect(await readBudgets()).toEqual(budgetsBefore);
});

test("a bill's days ahead and a goal reached are saved as notification settings", async ({
  page,
}) => {
  await signInFresh(page);
  await openSettingsPage(page, "Notifications");
  await expect(page.getByRole("heading", { name: "Notifications" })).toBeVisible();
  await expect(page.getByTestId("alerts-screen")).toBeVisible();

  // A bill's threshold is a number of days, not money, and starts at three.
  const bill = page.getByTestId("alert-setting-bill_due");
  const days = bill.getByRole("spinbutton", {
    name: "Tell me this many days before a bill is due",
  });
  await expect(days).toHaveValue("3");
  await days.fill("5");
  await bill.getByRole("button", { name: "Save" }).click();

  // A goal reached and a failing connection have nothing to type: they are
  // on or off.
  const goal = page.getByTestId("alert-setting-goal_reached");
  await expect(goal.getByRole("textbox")).toHaveCount(0);
  await expect(goal.getByRole("spinbutton")).toHaveCount(0);
  await goal.getByRole("checkbox", { name: "Enabled" }).check();
  await goal.getByRole("button", { name: "Save" }).click();
  const sync = page.getByTestId("alert-setting-sync_error");
  await expect(sync.getByRole("textbox")).toHaveCount(0);
  await expect(sync.getByRole("spinbutton")).toHaveCount(0);

  // Each setting is one document named by its kind; the thresholds are
  // stored as the days and as zero, never as money.
  const readSettings = () =>
    page.evaluate(async () => {
      const collection = window.rational.household?.session?.collections.alerts;
      if (collection === undefined) throw new Error("no household is open");
      const documents = await collection.find().exec();
      return documents
        .map((document) => document.toJSON())
        .filter((document) => document.kind === "setting")
        .map((setting) => ({
          id: setting.id,
          alert_kind: setting.alert_kind,
          threshold: setting.threshold,
          enabled: setting.enabled,
        }))
        .sort((left, right) => left.id.localeCompare(right.id));
    });
  await expect.poll(async () => (await readSettings()).length).toBe(2);
  expect(await readSettings()).toEqual([
    { id: "als_bill_due", alert_kind: "bill_due", threshold: 5, enabled: true },
    { id: "als_goal_reached", alert_kind: "goal_reached", threshold: 0, enabled: true },
  ]);
  // And they are what the server will read next time it syncs them down.
  await expect
    .poll(() =>
      page.evaluate(
        () => window.rationalFake?.remoteDocument("alerts", "als_bill_due")?.threshold ?? null,
      ),
    )
    .toBe(5);

  // The screen reads the saved values back.
  await expect(days).toHaveValue("5");
  await expect(goal.getByRole("checkbox", { name: "Enabled" })).toBeChecked();
});
