import { expect, type Page, test } from "@playwright/test";

/**
 * The sign-in screen against the in-browser fake: the provider round trip
 * (start → the provider's own page → back with a code in the fragment), a
 * magic link that works once, and a provider the environment knows but has
 * not enabled.
 */
async function openApplication(page: Page): Promise<void> {
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
}

test("a provider round trip signs the person in and the session survives a reload", async ({
  page,
}) => {
  await openApplication(page);
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();

  // Leaving for the provider is a real navigation; coming back is a fresh
  // load of the app with a one-time code in the fragment.
  await page.getByRole("button", { name: "Continue with Demo IdP" }).click();
  await page.waitForURL(/provider=demo-idp/u);
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  await expect(page.getByRole("combobox", { name: "Household" })).toContainText("Demo household");
  await expect(page.getByRole("heading", { name: "Accounts" })).toBeVisible();

  // The code is out of the address bar, so a reload cannot replay it.
  expect(page.url()).not.toContain("code=");
  expect(await page.evaluate(() => window.rational.state.user?.email)).toBe(
    "pat.provider@rational.test",
  );

  await page.reload();
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  await expect(page.getByRole("heading", { name: "Accounts" })).toBeVisible();
});

test("a provider the environment has not enabled is shown as unavailable", async ({ page }) => {
  await openApplication(page);
  const provider = page.getByTestId("provider-example-sso");
  await expect(provider).toContainText("not enabled for this environment");
  await expect(provider.getByRole("button", { name: "Continue with Example SSO" })).toBeDisabled();
});

test("a magic link signs the person in once and is refused the second time", async ({ page }) => {
  await openApplication(page);
  await page.getByLabel("Email").fill("mo@rational.test");
  await page.getByTestId("send-magic-link").click();
  await expect(page.getByTestId("magic-link-sent")).toContainText("mo@rational.test");

  const link = await page.evaluate(() => window.rationalFake?.lastMagicLink() ?? null);
  expect(link).not.toBeNull();
  await page.goto(link as string);
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  expect(await page.evaluate(() => window.rational.state.user?.email)).toBe("mo@rational.test");
  await expect(page.getByRole("heading", { name: "Accounts" })).toBeVisible();

  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();

  // The same link again: refused, and nobody is signed in.
  await page.goto(link as string);
  await expect(page.getByRole("alert")).toContainText("already been used");
  expect(await page.evaluate(() => window.rational.state.phase)).toBe("signed_out");
});
