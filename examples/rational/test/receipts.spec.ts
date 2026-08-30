import { expect, type Page, test } from "@playwright/test";

/**
 * A receipt belongs to a household, not to whoever photographed it.
 *
 * The object carries `household_id` as an attribute and the bucket's rules
 * read it — `claims.households[old.attributes.household_id] != null` — so the
 * other member opens the same file. Before objects could carry attributes the
 * rule could only reach the uploader, and this is the test that would have
 * failed (findings log #8).
 */
const OWNER = "dana@rational.test";
const MEMBER = "rio@rational.test";
const PASSWORD = "RationalDemo1!";

async function settle(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

async function enterCredentials(page: Page, email: string, action: string): Promise<void> {
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: action }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await settle(page);
}

test("a receipt is attached by one member and opened by another", async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  await enterCredentials(page, OWNER, "Create account");

  // A household with one member, one account, and one transaction to attach to.
  await page.getByRole("link", { name: "Household" }).click();
  const before = await page.evaluate(() => window.rational.state.currentHouseholdId);
  const creator = page.getByRole("form", { name: "New household" });
  await creator.getByLabel("Name").fill("Shared flat");
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
  const householdId = await page.evaluate(() => window.rational.state.currentHouseholdId);

  const invite = page.getByRole("form", { name: "Invite a member" });
  await invite.getByLabel("Email").fill(MEMBER);
  await invite.getByLabel("Role").selectOption("editor");
  await invite.getByRole("button", { name: "Send invitation" }).click();
  await expect(page.getByTestId(`member-${MEMBER}`)).toContainText("invited");

  const transactionId = await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Joint",
      type: "checking",
      currency: "USD",
      opening_balance: 0,
      opening_date: "2026-08-01",
    });
    const transaction = await writes.createTransaction({
      account_id: account.id,
      date: "2026-08-30",
      amount: -1250,
      currency: "USD",
      description: "Hardware store",
      tags: [],
      splits: [],
    });
    return transaction.id;
  });

  // The owner attaches a receipt.
  await page.getByRole("link", { name: "Transactions" }).click();
  await page
    .getByTestId(`transaction-${transactionId}`)
    .getByRole("button", { name: "Receipts" })
    .click();
  const panel = page.getByTestId("receipts-panel");
  await expect(panel).toBeVisible();
  await panel.getByLabel("Attach an image or a PDF").setInputFiles({
    name: "receipt.png",
    mimeType: "image/png",
    buffer: Buffer.from("PNG BYTES"),
  });
  await expect(panel.getByTestId("receipt-receipt.png")).toBeVisible();

  // The other member accepts, opens the same transaction, and finds the
  // receipt they did not upload.
  await page.getByRole("button", { name: "Sign out" }).click();
  await enterCredentials(page, MEMBER, "Create account");
  await page.getByRole("link", { name: "Household" }).click();
  await page
    .getByTestId(`invitation-${householdId}`)
    .getByRole("button", { name: "Accept" })
    .click();
  await page.waitForFunction(
    (id) => window.rational.state.memberships.some((entry) => entry.household_id === id),
    householdId,
    { timeout: 30_000 },
  );
  await settle(page);
  await page.getByRole("link", { name: "Transactions" }).click();
  await page
    .getByTestId(`transaction-${transactionId}`)
    .getByRole("button", { name: "Receipts" })
    .click();
  const theirPanel = page.getByTestId("receipts-panel");
  await expect(theirPanel.getByTestId("receipt-receipt.png")).toBeVisible();

  // And the bytes come back, which is the part a listing alone would not prove.
  const opened = await page.evaluate(
    async ([id]) => {
      const receipts = window.rational.receipts;
      if (receipts === null) throw new Error("no household is open");
      const [receipt] = await receipts.list(id as string);
      if (receipt === undefined) return null;
      const blob = await receipts.open(receipt.path);
      return blob === null ? null : await blob.text();
    },
    [transactionId],
  );
  expect(opened).toBe("PNG BYTES");
});
