import { expect, type Page, test } from "@playwright/test";

/**
 * Alerts, from the household's side.
 *
 * The household says what it wants to be told about; the server decides and
 * writes. So the test sets the thresholds through the app, then delivers the
 * alerts the way they really arrive — as documents another writer committed —
 * and checks that the screen shows them, names what they are about, and lets
 * a person mark one read without ever offering to delete it.
 */
const USER = "vireo@rational.test";
const PASSWORD = "RationalDemo1!";

async function settle(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

test("a household sets its thresholds and is told what the server decided", async ({ page }) => {
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
  await creator.getByLabel("Name").fill("Watchful");
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

  const accountId = await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 4_000,
      opening_date: "2026-01-01",
    });
    return String(account.id);
  });

  // The household says what it wants to hear about.
  await page.getByRole("link", { name: "Alerts" }).click();
  await expect(page.getByTestId("alerts-empty")).toBeVisible();
  const large = page.getByRole("form", { name: "A large transaction" });
  await large.getByRole("textbox").fill("400.00");
  await large.getByRole("button", { name: "Save" }).click();
  const low = page.getByRole("form", { name: "An account running low" });
  await low.getByRole("textbox").fill("50.00");
  await low.getByRole("button", { name: "Save" }).click();

  // The settings are documents, one per kind, and they survive a re-read.
  const readSettings = async () =>
    page.evaluate(async () => {
      const collection = window.rational.household?.session?.collections.alerts;
      if (collection === undefined) return [];
      const documents = await collection.find().exec();
      return documents.map((document) => document.toJSON());
    });
  await expect.poll(async () => (await readSettings()).length).toBe(2);
  const settings = await readSettings();
  expect(settings.map((setting) => setting.alert_kind).sort()).toEqual([
    "large_transaction",
    "low_balance",
  ]);
  expect(settings.find((setting) => setting.alert_kind === "large_transaction")?.threshold).toBe(
    40_000,
  );

  // What the server decided overnight arrives as documents, like everything else.
  const householdId = await page.evaluate(() => String(window.rational.state.currentHouseholdId));
  await page.evaluate(
    (context) => {
      const fake = window.rationalFake;
      if (fake === undefined) throw new Error("the fake backend is not installed");
      const stamp = Date.now();
      fake.putRemote("alerts", {
        id: `alr_${context.householdId}.large.txn-roof`,
        household_id: context.householdId,
        created_at: stamp,
        updated_at: stamp,
        kind: "alert",
        alert_kind: "large_transaction",
        fired_at: stamp,
        message: "ROOF REPAIR on 2026-08-29",
        amount: -50_000,
        currency: "USD",
        transaction_id: "txn-roof",
        account_id: context.accountId,
        read: false,
      });
      fake.putRemote("alerts", {
        id: `alr_${context.householdId}.low.${context.accountId}.2026-08-30`,
        household_id: context.householdId,
        created_at: stamp,
        updated_at: stamp + 1,
        kind: "alert",
        alert_kind: "low_balance",
        fired_at: stamp + 1,
        message: "Everyday is down to 40.00",
        amount: 4_000,
        currency: "USD",
        account_id: context.accountId,
        read: false,
      });
    },
    { householdId, accountId },
  );

  const history = page.getByRole("table", { name: "Alert history" });
  await expect(history.getByRole("row")).toHaveCount(3);
  const largeRow = page.getByTestId(`alert-alr_${householdId}.large.txn-roof`);
  await expect(largeRow.getByTestId("alert-kind")).toHaveText("A large transaction");
  await expect(largeRow.getByTestId("alert-message")).toContainText("ROOF REPAIR");
  await expect(largeRow.getByTestId("alert-message")).toContainText("Everyday");
  await expect(largeRow).toHaveAttribute("data-read", "no");

  // Marking one read changes the document, and nothing offers to delete it.
  await largeRow.getByRole("button", { name: "Mark read" }).click();
  await expect(largeRow).toHaveAttribute("data-read", "yes");
  await expect(largeRow.getByRole("button", { name: "Delete" })).toHaveCount(0);
  await expect(history.getByRole("row")).toHaveCount(3);
});
