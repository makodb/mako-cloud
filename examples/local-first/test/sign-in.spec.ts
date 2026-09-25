import { expect, test } from "@playwright/test";

/**
 * The sign-in form of a live page that names no credentials. The auth routes
 * are answered here, so this needs no deployment: it covers the form itself,
 * which the fake-backend scenarios never show.
 */
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.__MAKO_EXAMPLE__ = {
      endpoint: window.location.origin,
      projectId: "prj_sign_in_form",
      environmentId: "env_sign_in_form",
      collectionId: "todos",
      publicProjectKey: "mako_pk.key_signinform.0000",
    };
  });
});

test("reports a refused sign-up in the form and keeps the page for another try", async ({
  page,
}) => {
  const signUps: string[] = [];
  await page.route("**/auth/signup", async (route) => {
    signUps.push(route.request().postData() ?? "");
    await route.fulfill({ status: 400, contentType: "application/json", body: "{}" });
  });
  await page.goto("/");
  await page.getByRole("textbox", { name: "Email" }).fill("new-person@example.test");
  await page.getByRole("textbox", { name: "Password" }).fill("password1234");

  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.locator("#sign-in-error")).toContainText("Could not create the account");
  expect(signUps).toHaveLength(1);

  // A second click is another attempt, not the browser's own form submit,
  // which would reload the page with the password in the URL.
  await page.getByRole("button", { name: "Create account" }).click();
  await expect.poll(() => signUps.length).toBe(2);
  expect(page.url()).not.toContain("password");
  await expect(page.getByRole("textbox", { name: "Email" })).toHaveValue("new-person@example.test");
});

test("asks for 12 characters before sending a sign-up", async ({ page }) => {
  let signUps = 0;
  await page.route("**/auth/signup", async (route) => {
    signUps += 1;
    await route.fulfill({ status: 400, contentType: "application/json", body: "{}" });
  });
  await page.goto("/");
  await page.getByRole("textbox", { name: "Email" }).fill("new-person@example.test");
  await page.getByRole("textbox", { name: "Password" }).fill("password1");
  await page.getByRole("button", { name: "Create account" }).click();

  await expect(page.getByText("At least 12 characters.")).toBeVisible();
  expect(
    await page
      .getByRole("textbox", { name: "Password" })
      .evaluate((input: HTMLInputElement) => input.validity.tooShort),
  ).toBe(true);
  expect(signUps).toBe(0);
});

test("a verifying environment asks for the mailed link, and the link confirms the address", async ({
  page,
}) => {
  const signUps: Array<Record<string, string>> = [];
  await page.route("**/auth/signup", async (route) => {
    signUps.push(JSON.parse(route.request().postData() ?? "{}") as Record<string, string>);
    await route.fulfill({
      status: 202,
      contentType: "application/json",
      body: JSON.stringify({ accepted: true, verificationRequired: true }),
    });
  });
  const verifications: string[] = [];
  await page.route("**/auth/verify-email", async (route) => {
    verifications.push(route.request().postData() ?? "");
    await route.fulfill({
      status: verifications.length === 1 ? 200 : 401,
      contentType: "application/json",
      body: verifications.length === 1 ? JSON.stringify({ verified: true }) : "{}",
    });
  });
  await page.goto("/");
  await page.getByRole("textbox", { name: "Email" }).fill("new-person@example.test");
  await page.getByRole("textbox", { name: "Password" }).fill("password1234");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.locator("#sign-in-error")).toHaveText(
    "Check new-person@example.test for a link to confirm the address, then sign in.",
  );
  expect(signUps[0]?.redirectUrl).toBe(new URL("/", page.url()).href);

  // Opened from the mail: a fresh load, not a hash change on the same page.
  await page.goto("about:blank");
  await page.goto("/#verification_token=evc-token");
  await expect(page.locator("#sign-in-error")).toHaveText("Email confirmed. Sign in to continue.");
  expect(JSON.parse(verifications[0] ?? "{}")).toEqual({ token: "evc-token" });
  expect(page.url()).not.toContain("verification_token");

  await page.goto("about:blank");
  await page.goto("/#verification_token=evc-token");
  await expect(page.locator("#sign-in-error")).toContainText("expired or was already used");
});
