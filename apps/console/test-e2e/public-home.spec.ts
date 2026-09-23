import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.__MAKO_CONSOLE__ = {
      managementEndpoint: "http://127.0.0.1:4174",
      developerAuth: {
        loadSession: async () => null,
        beginSignIn: async () => {},
        signOut: async () => {},
        subscribe: () => () => {},
      },
    };
  });
});

test("the public homepage introduces the cloud and links to user documentation", async ({
  page,
}) => {
  await page.goto("/");

  await expect(
    page.getByRole("heading", { name: "The cloud backend for apps that work offline." }),
  ).toBeVisible();
  await expect(page.getByText("Public preview", { exact: true })).toBeVisible();
  await expect(page.getByRole("img", { name: "Mako Cloud platform overview" })).toBeVisible();

  const docs = page.locator("#docs");
  await expect(
    docs.getByRole("heading", { name: "Start with the task in front of you." }),
  ).toBeVisible();
  await expect(docs.getByRole("link", { name: /Getting started/u })).toHaveAttribute(
    "href",
    "/docs/user-book#getting-started",
  );
  await expect(docs.getByRole("link", { name: /Connect an RxDB app/u })).toHaveAttribute(
    "href",
    "/docs/user-book#building-a-local-first-app-with-rxdb",
  );
  await expect(docs.getByRole("link", { name: /Open the full User Book/u })).toHaveAttribute(
    "href",
    "/docs/user-book",
  );

  const docsResponse = await page.request.get("/docs/user-book");
  expect(docsResponse.ok()).toBe(true);
  await expect(docsResponse.text()).resolves.toContain('id="getting-started"');

  await page.getByRole("link", { name: "Sign in", exact: true }).first().click();
  await expect(page).toHaveURL(/\/login$/u);
  await expect(
    page.getByRole("heading", { name: "Sign in to your developer account" }),
  ).toBeVisible();
});

test("the User Book keeps its chapters in responsive side navigation", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/docs/user-book");

  const sideNavigation = page.locator(".desktop-toc");
  await expect(sideNavigation).toBeVisible();
  await expect(sideNavigation.getByText("On this page", { exact: true })).toBeVisible();
  await expect(sideNavigation.getByRole("link")).toHaveCount(28);
  await expect(sideNavigation.getByRole("link", { name: "Concepts", exact: true })).toHaveAttribute(
    "href",
    "#concepts",
  );
  await expect(page.getByRole("heading", { name: "Table of contents" })).toHaveCount(0);

  await sideNavigation.getByRole("link", { name: "Application authentication" }).click();
  await expect(page).toHaveURL(/#application-authentication$/u);
  await expect(
    sideNavigation.getByRole("link", { name: "Application authentication" }),
  ).toHaveAttribute("aria-current", "location");

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(sideNavigation).toBeHidden();
  const mobileNavigation = page.locator(".mobile-toc");
  await expect(mobileNavigation).toBeVisible();
  await expect(mobileNavigation).not.toHaveAttribute("open", "");
  await mobileNavigation.locator("summary").click();
  await expect(mobileNavigation.getByRole("link", { name: "Concepts", exact: true })).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    ),
  ).toBeLessThanOrEqual(1);
});
