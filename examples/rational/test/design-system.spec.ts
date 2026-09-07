import { expect, type Page, test } from "@playwright/test";

/**
 * What the design system promises on Rational's screens: the theme preference
 * dresses every screen and is remembered on the device; money is set in
 * tabular figures, right-aligned, told apart by sign and colour; a dialog is
 * operable from the keyboard; a chart names itself and explains a point.
 */
async function openDemoHousehold(page: Page): Promise<void> {
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  await page.getByRole("button", { name: "Continue with Demo IdP" }).click();
  await page.waitForURL(/provider=demo-idp/u);
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  await page.waitForFunction(() => window.rational.state.currentHouseholdId !== null);
  await page.waitForFunction(() => window.rational.household?.session !== null);
  await page.evaluate(() => window.rational.waitForSync());
}

const theme = (page: Page) =>
  page.evaluate(() => ({
    attribute: document.documentElement.getAttribute("data-theme"),
    scheme: getComputedStyle(document.documentElement).colorScheme,
  }));

test("the theme preference dresses every screen and is remembered on the device", async ({
  page,
}) => {
  await openDemoHousehold(page);
  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
  expect((await theme(page)).attribute).toBeNull();

  const toggle = page.getByTestId("theme-toggle");
  await expect(toggle).toHaveAccessibleName("Switch to the dark theme");
  await toggle.click();
  await expect(toggle).toHaveAccessibleName("Switch to the light theme");
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  expect(await theme(page)).toEqual({ attribute: "dark", scheme: "dark" });

  // Another screen, then a reload: the choice follows.
  await page.goto("/#/accounts");
  await expect(page.getByRole("heading", { name: "Accounts" })).toBeVisible();
  expect(await theme(page)).toEqual({ attribute: "dark", scheme: "dark" });
  await page.reload();
  await page.waitForFunction(() => window.rational?.state.phase === "ready");
  await expect(page.getByRole("heading", { name: "Accounts" })).toBeVisible();
  expect(await theme(page)).toEqual({ attribute: "dark", scheme: "dark" });
  expect(await page.evaluate(() => window.localStorage.getItem("rational.theme"))).toBe("dark");

  await page.getByTestId("theme-toggle").click();
  expect(await theme(page)).toEqual({ attribute: "light", scheme: "light" });
});

test("a money table reads at a glance: tabular, right-aligned, signed and coloured", async ({
  page,
}) => {
  await openDemoHousehold(page);
  await page.goto("/#/transactions");
  const table = page.getByRole("table", { name: "Transactions" });
  await expect(table).toBeVisible();
  const amounts = table.locator("tbody td.money");
  await expect(amounts.first()).toBeVisible();
  const styles = await amounts.first().evaluate((element) => {
    const computed = getComputedStyle(element);
    return { numeric: computed.fontVariantNumeric, align: computed.textAlign };
  });
  expect(styles.numeric).toContain("tabular-nums");
  expect(styles.align).toBe("right");

  // Income and spending are told apart by sign and colour.
  const income = table.locator("tbody td.money.text-positive").first();
  await expect(income).toBeVisible();
  await expect(income).not.toContainText("-");
  const spending = table.locator("tbody td.money:not(.text-positive)").first();
  await expect(spending).toContainText("-");
  const [incomeColor, spendingColor] = await Promise.all([
    income.evaluate((element) => getComputedStyle(element).color),
    spending.evaluate((element) => getComputedStyle(element).color),
  ]);
  expect(incomeColor).not.toBe(spendingColor);

  // The column of amounts is right-aligned under its heading.
  const heading = table.getByRole("columnheader", { name: "Amount" });
  await expect(heading).toBeVisible();
});

test("a dialog is opened from the keyboard, closes on Escape, and gives focus back", async ({
  page,
}) => {
  await openDemoHousehold(page);
  await page.goto("/#/accounts");
  const opener = page.getByRole("button", { name: "New account" });
  await opener.focus();
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveAccessibleName(/account/iu);
  // Focus moved inside the dialog.
  expect(
    await page.evaluate(() => {
      const active = document.activeElement;
      return active !== null && active.closest('[role="dialog"]') !== null;
    }),
  ).toBe(true);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
});

test("a chart settles instead of redrawing itself forever", async ({ page }) => {
  await openDemoHousehold(page);
  await page.goto("/#/investments");
  const chart = page.getByRole("img", { name: /Allocation of USD holdings/u });
  await expect(chart).toBeVisible();
  const sectors = chart.locator(".recharts-pie-sector path");
  await expect(sectors).toHaveCount(2);

  // The screen builds its slices inline, so the chart is handed a new array on
  // every render; the drawing must still come to rest, and the ring must be
  // whole rather than frozen part-way through its entry.
  const geometry = () => sectors.evaluateAll((paths) => paths.map((p) => p.getAttribute("d")));
  await page.waitForTimeout(1500);
  const settled = await geometry();
  await page.waitForTimeout(1200);
  expect(await geometry()).toEqual(settled);

  // Whole: the ring is drawn all the way round. Sampled at the middle of its
  // thickness at twelve angles -- a chart frozen part-way through its entry
  // leaves a gap, which is what this guards against.
  const box = await chart.locator(".recharts-surface").first().boundingBox();
  if (box === null) throw new Error("the chart has no drawing");
  const centre = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  const radius = (Math.min(box.width, box.height) / 2) * 0.77;
  const onRing = await page.evaluate(
    ({ centre: middle, radius: distance }) =>
      Array.from({ length: 12 }, (_unused, index) => {
        const angle = (index / 12) * 2 * Math.PI;
        const element = document.elementFromPoint(
          middle.x + distance * Math.sin(angle),
          middle.y - distance * Math.cos(angle),
        );
        return element?.closest(".recharts-pie-sector") != null;
      }),
    { centre, radius },
  );
  expect(onRing).toEqual(Array.from({ length: 12 }, () => true));
});

test("a chart names what it shows and explains the point under the pointer", async ({ page }) => {
  await openDemoHousehold(page);
  await page.goto("/#/accounts");
  const chart = page.getByRole("img", { name: /Net worth in USD over time/u });
  await expect(chart).toBeVisible();
  // Formatted axis values.
  await expect(
    chart.locator(".recharts-cartesian-axis-tick-value", { hasText: "$" }).first(),
  ).toBeVisible();
  // A point under the pointer: the tooltip names the series and the amount.
  const box = await chart.locator(".recharts-surface").first().boundingBox();
  if (box === null) throw new Error("the chart has no drawing");
  await page.mouse.move(box.x + box.width * 0.6, box.y + box.height * 0.5);
  await page.mouse.move(box.x + box.width * 0.62, box.y + box.height * 0.5);
  const tooltip = chart.locator(".recharts-tooltip-wrapper");
  await expect(tooltip).toBeVisible();
  await expect(tooltip).toContainText("$");
});
