import { expect, test } from "@playwright/test";

/**
 * What the design system promises in the console: the developer's theme
 * preference dresses the shell and every destination, and is remembered on
 * the device. The API is not mocked here on purpose -- the header and its
 * toggle must be there whatever the screen behind them is doing.
 */
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const session = {
      accessToken: "developer-session-token",
      expiresAt: "2099-01-01T00:00:00.000Z",
      audience: "mako-management",
      profile: { id: "dev_abcdefgh", email: "owner@example.test", displayName: "Owner" },
    };
    window.__MAKO_CONSOLE__ = {
      managementEndpoint: "http://127.0.0.1:4174",
      developerAuth: {
        loadSession: async () => session,
        beginSignIn: async () => {},
        signOut: async () => {},
        subscribe: () => () => {},
      },
    };
  });
});

const theme = (page: import("@playwright/test").Page) =>
  page.evaluate(() => ({
    attribute: document.documentElement.getAttribute("data-theme"),
    scheme: getComputedStyle(document.documentElement).colorScheme,
    stored: window.localStorage.getItem("mako.console.theme"),
  }));

test("the console follows the developer's theme and remembers it on the device", async ({
  page,
}) => {
  await page.goto("/");
  const toggle = page.getByTestId("theme-toggle");
  await expect(toggle).toHaveAccessibleName("Switch to the dark theme");
  expect((await theme(page)).attribute).toBeNull();

  await toggle.click();
  await expect(toggle).toHaveAccessibleName("Switch to the light theme");
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  expect(await theme(page)).toEqual({ attribute: "dark", scheme: "dark", stored: "dark" });

  // Another destination, then a reload: the choice follows.
  await page.goto("/projects/prj_abcdefgh/environments/env_abcdefgh/collections");
  await expect(page.getByTestId("theme-toggle")).toBeVisible();
  expect((await theme(page)).attribute).toBe("dark");
  await page.reload();
  await expect(page.getByTestId("theme-toggle")).toHaveAccessibleName("Switch to the light theme");
  expect(await theme(page)).toEqual({ attribute: "dark", scheme: "dark", stored: "dark" });

  await page.getByTestId("theme-toggle").click();
  expect(await theme(page)).toEqual({ attribute: "light", scheme: "light", stored: "light" });
});
