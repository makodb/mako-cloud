import { expect, type Page, test } from "@playwright/test";

/**
 * The Plaid link story against the in-browser fake: a member links a real
 * institution, the exchange happens server-side, the connection appears
 * beside the simulator's, and the first sync's transactions replicate in.
 *
 * Plaid's widget is the one thing the fake cannot be: it is Plaid's own
 * script. So the test defines `window.Plaid` before the app loads -- the same
 * seam the UI uses -- and hands the flow a public token the way the real
 * widget would. What is being tested is everything on our side of that seam.
 *
 * The assertion with teeth is the last one: after the whole story, nothing
 * token-shaped exists anywhere the browser can see -- not in a replicated
 * document, not in the DOM. The access token lives in `plaid_items`, which
 * the app does not even open.
 */
const OWNER = "plaid-owner@rational.test";
const PASSWORD = "RationalDemo1!";

async function settle(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

test("a deployment without Plaid credentials never offers the option, and everything else stands", async ({
  page,
}) => {
  await page.addInitScript(() => {
    // Before the app asks: this copy of Rational was deployed without Plaid.
    const flip = () => {
      if (window.rationalFake !== undefined) window.rationalFake.plaidConfigured = false;
      else setTimeout(flip, 0);
    };
    flip();
  });
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  await page.getByLabel("Email").fill("unconfigured@rational.test");
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await settle(page);
  await page.evaluate(() =>
    window.rational.selectHousehold(window.rationalFake?.demoHouseholdId ?? null),
  );
  await page.waitForFunction(() => window.rational.state.currentHouseholdId !== null);

  await page.getByRole("link", { name: "Connections" }).click();
  await expect(page.getByTestId("connections-screen")).toBeVisible();
  // The simulator's own form is whole; the Plaid one simply is not offered.
  await expect(page.getByRole("form", { name: "Connect an account" })).toBeVisible();
  await expect(page.getByTestId("plaid-connect")).toHaveCount(0);
});

test("linking through Plaid lands a connection and its first sync, and no token ever shows", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as unknown as Record<string, unknown>).Plaid = {
      create(options: {
        token: string;
        onSuccess: (publicToken: string, metadata: unknown) => void;
      }) {
        return {
          open() {
            if (options.token !== "link-sandbox-fake-1") {
              throw new Error(`the widget got the wrong token: ${options.token}`);
            }
            options.onSuccess("public-sandbox-fake-1", {
              institution: { name: "First Platypus Bank" },
            });
          },
        };
      },
    };
  });
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  await page.getByLabel("Email").fill(OWNER);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await settle(page);

  const before = await page.evaluate(() => window.rational.state.currentHouseholdId);
  await page.evaluate(() => window.rational.createHousehold("Linked", "USD"));
  await page.waitForFunction(
    (previous) =>
      window.rational.state.currentHouseholdId !== previous &&
      window.rational.state.currentHouseholdId !== null,
    before,
    { timeout: 30_000 },
  );
  await settle(page);
  await page.waitForFunction(() => window.rational.writes !== null);
  await page.evaluate(() =>
    window.rational.writes?.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 500_000,
      opening_date: "2026-01-01",
    }),
  );

  await page.getByRole("link", { name: "Connections" }).click();
  await expect(page.getByTestId("connections-screen")).toBeVisible();

  // The offer appears only because the (fake) deployment says it is
  // configured; a deployment without Plaid credentials never shows this form.
  const plaidForm = page.getByTestId("plaid-connect");
  await expect(plaidForm).toBeVisible();
  await plaidForm.getByLabel("Account").selectOption({ label: "Everyday" });
  await plaidForm.getByRole("button", { name: "Connect through Plaid" }).click();

  // The connection lands with the institution's real name and the first
  // sync's outcome, and the imported transactions replicate like any others.
  const row = page.getByTestId("connection-con_plaid-item-fake-1");
  await expect(row).toContainText("First Platypus Bank");
  await expect(row.getByTestId("outcome")).toHaveText("imported 3, corrected 0, removed 0");
  await page.getByRole("link", { name: "Transactions" }).click();
  await expect(page.getByText("SparkFun")).toBeVisible();
  await expect(page.getByText("PAYROLL")).toBeVisible();

  // Nothing token-shaped anywhere the browser can see: not in any document of
  // any collection the app opened, and not in the page itself.
  const everything = await page.evaluate(async () => {
    const session = window.rational.household?.session;
    const collected: unknown[] = [];
    for (const id of ["connections", "transactions", "accounts"] as const) {
      const collection = session?.collections[id];
      if (collection === undefined) continue;
      const documents = await collection.find().exec();
      collected.push(documents.map((document) => document.toJSON()));
    }
    return `${JSON.stringify(collected)}\n${document.body.innerHTML}`;
  });
  expect(everything).not.toContain("access-sandbox");
  expect(everything).not.toContain("access_token");
  expect(everything).not.toContain("public-sandbox-fake-1");
});
