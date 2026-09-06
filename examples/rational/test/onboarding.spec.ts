import { expect, type Page, test } from "@playwright/test";

/**
 * A person who has never used Rational should land in their money, not in a
 * lesson about "households". The sharing model is real -- one space, several
 * people, roles -- but a newcomer sees none of it: sign up, and a space is
 * provisioned silently under the name "Personal", with no picker in the
 * chrome because there is nothing to pick between.
 *
 * The fake normally joins every sign-up to the seeded demo household so the
 * screens have data; `joinDemoOnSignup = false` reproduces the real thing --
 * a brand-new account that belongs nowhere.
 */
async function settle(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

test("a brand-new account is dropped straight into a space of its own", async ({ page }) => {
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
  await page.getByLabel("Email").fill("newcomer@rational.test");
  await page.getByLabel("Password").fill("RationalDemo1!");
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await settle(page);

  // A space appears without the person asking for one, and it is theirs.
  await page.waitForFunction(() => window.rational.state.currentHouseholdId !== null, undefined, {
    timeout: 30_000,
  });
  await expect
    .poll(() => page.evaluate(() => window.rational.state.households[0]?.name ?? ""))
    .toBe("Personal");
  expect(await page.evaluate(() => window.rational.state.memberships[0]?.role)).toBe("owner");

  // No switcher in the chrome: one space needs no chrome. The person is just
  // looking at their accounts.
  await expect(page.getByRole("combobox", { name: "Space" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Accounts" })).toBeVisible();

  // Exactly one space -- the auto-provision does not fire twice.
  expect(await page.evaluate(() => window.rational.state.households.length)).toBe(1);
});

test("a second sign-in on the same fresh account does not make a second space", async ({
  page,
}) => {
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
  await page.getByLabel("Email").fill("returning@rational.test");
  await page.getByLabel("Password").fill("RationalDemo1!");
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await settle(page);
  await page.waitForFunction(() => window.rational.state.currentHouseholdId !== null, undefined, {
    timeout: 30_000,
  });
  const firstId = await page.evaluate(() => window.rational.state.currentHouseholdId);

  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  await page.getByLabel("Email").fill("returning@rational.test");
  await page.getByLabel("Password").fill("RationalDemo1!");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await settle(page);
  await page.waitForFunction(() => window.rational.state.currentHouseholdId !== null, undefined, {
    timeout: 30_000,
  });

  // The same space, not a fresh "Personal" every visit.
  expect(await page.evaluate(() => window.rational.state.households.length)).toBe(1);
  expect(await page.evaluate(() => window.rational.state.currentHouseholdId)).toBe(firstId);
});
