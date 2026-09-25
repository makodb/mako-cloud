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

test("stays signed in across a reload, and signing out forgets the session", async ({ page }) => {
  const session = {
    accessToken: "access-token",
    refreshToken: "refresh-token-value-that-is-long-enough",
    expiresIn: 300,
    user: {
      id: "usr_signinform",
      email: "returning@example.test",
      status: "active",
      authorizationEpoch: 1,
    },
  };
  let signIns = 0;
  await page.route("**/auth/signin", async (route) => {
    signIns += 1;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(session),
    });
  });
  await page.route("**/auth/signout", (route) => route.fulfill({ status: 204 }));
  await page.route("**/replication/pull", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ documents: [], checkpoint: "mcp1.empty" }),
    }),
  );
  await page.route("**/replication/stream*", (route) =>
    route.fulfill({ status: 200, contentType: "text/event-stream", body: "" }),
  );
  await page.goto("/");
  await page.getByRole("textbox", { name: "Email" }).fill("returning@example.test");
  await page.getByRole("textbox", { name: "Password" }).fill("password1234");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page.locator("#account-email")).toHaveText("returning@example.test");

  await page.reload();
  await expect(page.locator("#account-email")).toHaveText("returning@example.test");
  await expect(page.locator("#sign-in")).toBeHidden();
  expect(signIns, "the stored session was resumed, not signed in again").toBe(1);

  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("button", { name: "Create account" })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("button", { name: "Create account" })).toBeVisible();
});

test("a todo written while offline survives closing the page and is pushed on the next visit", async ({
  page,
}) => {
  const session = {
    accessToken: "access-token",
    refreshToken: "refresh-token-value-that-is-long-enough",
    expiresIn: 300,
    user: {
      id: "usr_offlinewriter",
      email: "offline@example.test",
      status: "active",
      authorizationEpoch: 1,
    },
  };
  await page.route("**/auth/signin", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(session) }),
  );
  await page.route("**/replication/pull", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ documents: [], checkpoint: "mcp1.empty" }),
    }),
  );
  await page.route("**/replication/stream*", (route) =>
    route.fulfill({ status: 200, contentType: "text/event-stream", body: "" }),
  );
  // The network is down for pushes: the write can only stay on this device.
  let online = false;
  const pushed: string[] = [];
  await page.route("**/replication/push", async (route) => {
    if (!online) {
      await route.abort("internetdisconnected");
      return;
    }
    const body = JSON.parse(route.request().postData() ?? "{}") as {
      rows: { mutationId: string; newDocumentState: { title: string } }[];
    };
    pushed.push(...body.rows.map((row) => row.newDocumentState.title));
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        outcomes: body.rows.map((row) => ({ mutationId: row.mutationId, status: "accepted" })),
      }),
    });
  });
  await page.goto("/");
  await page.getByRole("textbox", { name: "Email" }).fill("offline@example.test");
  await page.getByRole("textbox", { name: "Password" }).fill("password1234");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await page.getByRole("textbox", { name: "Todo" }).fill("written with the wifi off");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page.getByText("written with the wifi off")).toBeVisible();

  await page.goto("about:blank");
  online = true;
  await page.goto("/");
  await expect(page.getByText("written with the wifi off")).toBeVisible();
  await expect.poll(() => pushed).toContain("written with the wifi off");
});
