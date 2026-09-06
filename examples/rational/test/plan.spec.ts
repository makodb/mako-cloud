import { expect, type Page, test } from "@playwright/test";

/**
 * Recurring charges: what repeats, and what the household does about it.
 *
 * A detected recurrence is a suggestion until the person acts on it, so the
 * first test confirms one and watches it become an upcoming bill -- on the
 * list, in the monthly total, and on the calendar -- then adjusts and pauses
 * it. A dismissal has to stay dismissed even when the charge lands again,
 * which is the part a re-detection would get wrong; the second test checks
 * that, and that a charge marked recurring by hand says so.
 *
 * The seeded charges are dated relative to today so the detection's next date
 * falls in the current month whatever day the suite runs on.
 */
const PASSWORD = "RationalDemo1!";

const TODAY = new Date().toISOString().slice(0, 10);
const THIS_MONTH = TODAY.slice(0, 7);

// Each test signs up, provisions a space, and walks a recurrence through its
// life; on a busy host that is more than the suite's default half minute.
test.setTimeout(90_000);

/** The day the charges land on; capped so every month of the year has it. */
const CHARGE_DAY = Math.min(Number(TODAY.slice(8, 10)), 28);

const LONG_MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

/** `2026-11` plus 3 is `2027-02`, as the app's own `addMonths` has it. */
function shiftMonth(month: string, count: number): string {
  const [year = 0, index = 1] = month.split("-").map(Number);
  const total = year * 12 + (index - 1) + count;
  const resultYear = Math.floor(total / 12);
  const resultMonth = total - resultYear * 12 + 1;
  return `${resultYear}-${String(resultMonth).padStart(2, "0")}`;
}

/** The same day next month, or the month's last day when it has no such day. */
function nextMonthSameDay(date: string): string {
  const [year = 0, month = 1, day = 1] = date.split("-").map(Number);
  const target = new Date(Date.UTC(year, month, 1));
  const lastDay = new Date(
    Date.UTC(target.getUTCFullYear(), target.getUTCMonth() + 1, 0),
  ).getUTCDate();
  target.setUTCDate(Math.min(day, lastDay));
  return target.toISOString().slice(0, 10);
}

function daysBetween(from: string, to: string): number {
  return Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / 86_400_000);
}

function monthLabel(month: string): string {
  const [year = "", index = "01"] = month.split("-");
  return `${LONG_MONTHS[Number(index) - 1] ?? index} ${year}`;
}

/** "in 3 days", "today", "2 days late", as the upcoming list writes it. */
function daysText(daysAway: number): string {
  if (daysAway === 0) return "today";
  const count = Math.abs(daysAway);
  const unit = count === 1 ? "day" : "days";
  return daysAway < 0 ? `${count} ${unit} late` : `in ${count} ${unit}`;
}

/**
 * Sign up as somebody new who belongs to no household, and wait for the
 * space Rational provisions for them to open with its writes ready.
 */
async function openFreshSpace(page: Page, email: string): Promise<void> {
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
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await page.waitForFunction(
    () =>
      window.rational.state.directory?.initialSynced === true &&
      window.rational.state.currentHouseholdId !== null &&
      window.rational.state.household?.initialSynced === true &&
      window.rational.writes !== null,
    undefined,
    { timeout: 30_000 },
  );
}

/** Three months of one charge on one account, the most recent last month. */
async function seedMonthlyCharge(page: Page, description: string, amount: number): Promise<string> {
  return page.evaluate(
    async ([text, minor, months]) => {
      const writes = window.rational.writes;
      if (writes === null) throw new Error("no household is open");
      const account = await writes.createAccount({
        name: "Everyday",
        type: "checking",
        currency: "USD",
        opening_balance: 0,
        opening_date: "2026-01-01",
      });
      for (const [index, month] of (months as string[]).entries()) {
        await writes.createTransaction({
          account_id: account.id,
          date: month,
          amount: Number(minor),
          currency: "USD",
          description: `${text} ${index}`,
          tags: [],
          splits: [],
        });
      }
      return account.id;
    },
    [
      description,
      amount,
      [-3, -2, -1].map(
        (offset) => `${shiftMonth(THIS_MONTH, offset)}-${String(CHARGE_DAY).padStart(2, "0")}`,
      ),
    ] as const,
  );
}

test("a detected charge is confirmed into an upcoming bill, adjusted, paused, and put on the calendar", async ({
  page,
}) => {
  await openFreshSpace(page, "wren@rational.test");
  await seedMonthlyCharge(page, "STREAMING SERVICE", -1_299);
  const expectedNext = `${THIS_MONTH}-${String(CHARGE_DAY).padStart(2, "0")}`;
  const daysAway = daysBetween(TODAY, expectedNext);

  await page.getByRole("link", { name: "Recurring" }).click();
  await expect(page.getByRole("heading", { name: "Recurring", exact: true })).toBeVisible();
  await expect(page.getByTestId("monthly-total-USD")).toHaveText("$0.00");

  // Three charges a month apart look monthly, and this device is what noticed.
  const detected = page.getByTestId("detected-streaming-service");
  await expect(detected).toHaveAttribute("data-noticed-by", "this device");
  await expect(detected.getByTestId("interval")).toHaveText("monthly");
  await expect(detected.getByTestId("next")).toHaveText(expectedNext);

  // Confirming it makes it a bill: gone from the suggestions, on the list,
  // counted in the monthly total.
  await detected.getByRole("button", { name: "Confirm" }).click();
  await expect(page.getByTestId("detected-streaming-service")).toHaveCount(0);
  const upcoming = page.getByRole("list", { name: "Upcoming bills" });
  await expect(upcoming).toContainText("streaming service");
  await expect(page.getByTestId("monthly-total-USD")).toHaveText("-$12.99");
  const table = page.getByRole("table", { name: "All recurring" });
  const row = table.locator("tbody tr").first();
  await expect(row.getByTestId("name")).toHaveText("Streaming Service");
  await expect(row.getByTestId("interval")).toHaveText("monthly");
  await expect(row.getByTestId("amount")).toHaveText("-$12.99");
  await expect(row.getByTestId("next")).toHaveText(expectedNext);
  const recurrenceId = (await row.getAttribute("data-testid"))?.replace("recurrence-", "") ?? "";
  expect(recurrenceId).not.toBe("");

  // Its date is in the current month, at or before today, and within the
  // interval's slack: due, with no transaction near enough to have paid it.
  const bill = page.getByTestId(`bill-${recurrenceId}`);
  await expect(bill).toHaveAttribute("data-state", "due");
  await expect(bill.getByTestId("state")).toHaveText("due");
  await expect(bill.getByTestId("due")).toHaveText(expectedNext);
  await expect(bill.getByTestId("days")).toHaveText(daysText(daysAway));
  await expect(bill.getByTestId("amount")).toHaveText("-$12.99");

  // Adjusting it in place: a name of the household's own, shown everywhere.
  await row.getByRole("button", { name: "Edit" }).click();
  const editor = page.getByRole("form", { name: "Edit recurrence" });
  await editor.getByLabel("Name").fill("Streaming");
  await editor.getByLabel("Expected amount (USD)").fill("-13.99");
  await editor.getByRole("button", { name: "Save recurrence" }).click();
  await expect(row.getByTestId("name")).toHaveText("Streaming");
  await expect(row.getByTestId("amount")).toHaveText("-$13.99");
  await expect(bill).toContainText("Streaming");
  await expect(page.getByTestId("monthly-total-USD")).toHaveText("-$13.99");

  // Paused, it is neither due nor counted; resumed, it is both again.
  await row.getByRole("button", { name: "Pause" }).click();
  await expect(row).toHaveAttribute("data-status", "paused");
  await expect(row).toContainText("paused");
  await expect(page.getByTestId(`bill-${recurrenceId}`)).toHaveCount(0);
  await expect(page.getByTestId("monthly-total-USD")).toHaveText("$0.00");
  await row.getByRole("button", { name: "Resume" }).click();
  await expect(row).toHaveAttribute("data-status", "confirmed");
  await expect(page.getByTestId(`bill-${recurrenceId}`)).toBeVisible();

  // The calendar: the bill sits on its day, the month totals it, and the
  // address remembers the view and the month.
  await page.getByRole("link", { name: "Calendar" }).click();
  await expect(page).toHaveURL(/view=calendar/u);
  await expect(page.getByTestId("calendar-month")).toHaveText(monthLabel(THIS_MONTH));
  const calendar = page.getByRole("table", { name: `Bills in ${monthLabel(THIS_MONTH)}` });
  await expect(
    calendar
      .locator(`td[data-date="${expectedNext}"]`)
      .getByTestId(`calendar-bill-${recurrenceId}`),
  ).toContainText("Streaming");
  await expect(page.getByTestId("calendar-total-USD")).toHaveText("-$13.99");

  const nextMonth = shiftMonth(THIS_MONTH, 1);
  await page.getByRole("button", { name: "Next month" }).click();
  await expect(page).toHaveURL(new RegExp(`month=${nextMonth}`, "u"));
  await expect(page.getByTestId("calendar-month")).toHaveText(monthLabel(nextMonth));
  await expect(
    page
      .getByRole("table", { name: `Bills in ${monthLabel(nextMonth)}` })
      .locator(`td[data-date="${nextMonthSameDay(expectedNext)}"]`)
      .getByTestId(`calendar-bill-${recurrenceId}`),
  ).toBeVisible();

  // A bill on the grid leads back to its row in the list.
  await page.getByTestId(`calendar-bill-${recurrenceId}`).click();
  await expect(page).not.toHaveURL(/view=calendar/u);
  await expect(page.getByTestId(`recurrence-${recurrenceId}`)).toHaveClass(/highlighted/u);
});

test("a dismissed charge stays dismissed when it lands again, and a charge marked by hand says so", async ({
  page,
}) => {
  await openFreshSpace(page, "wren.dismisses@rational.test");
  const accountId = await seedMonthlyCharge(page, "GYM MEMBERSHIP", -4_500);

  await page.getByRole("link", { name: "Recurring" }).click();
  const detected = page.getByTestId("detected-gym-membership");
  await expect(detected.getByTestId("interval")).toHaveText("monthly");
  await detected.getByRole("button", { name: "Dismiss" }).click();
  await expect(page.getByTestId("detected-gym-membership")).toHaveCount(0);

  // The charge lands again this month. Re-detection would propose it afresh;
  // the dismissal on record keeps it quiet.
  await page.evaluate(
    async ([id, date]) => {
      const writes = window.rational.writes;
      if (writes === null) throw new Error("no household is open");
      await writes.createTransaction({
        account_id: String(id),
        date: String(date),
        amount: -4_500,
        currency: "USD",
        description: "GYM MEMBERSHIP 3",
        tags: [],
        splits: [],
      });
    },
    [accountId, `${THIS_MONTH}-${String(CHARGE_DAY).padStart(2, "0")}`] as const,
  );
  await expect
    .poll(() =>
      page.evaluate(async () => {
        const collection = window.rational.household?.session?.collections.transactions;
        if (collection === undefined) throw new Error("transactions are not open");
        return (await collection.find().exec()).length;
      }),
    )
    .toBe(4);
  await expect(page.getByTestId("detected-gym-membership")).toHaveCount(0);
  const recurrences = await page.evaluate(async () => {
    const collection = window.rational.household?.session?.collections.recurrences;
    if (collection === undefined) throw new Error("recurrences are not open");
    return (await collection.find().exec()).map((document) => document.toJSON());
  });
  expect(recurrences).toHaveLength(1);
  expect(recurrences[0]?.status).toBe("dismissed");
  await expect(page.getByRole("table", { name: "All recurring" })).toHaveCount(0);

  // A person needs no three occurrences: one charge marked recurring by hand
  // is a confirmed bill at once, due one interval on, and says it was manual.
  // The document is the one `markRecurring` writes from the transaction panel.
  const manual = await page.evaluate(
    async ([id, date, next]) => {
      const writes = window.rational.writes;
      if (writes === null) throw new Error("no household is open");
      const transaction = await writes.createTransaction({
        account_id: String(id),
        date: String(date),
        amount: -2_000,
        currency: "USD",
        description: "PARKING GARAGE 0412",
        tags: [],
        splits: [],
      });
      const recurrence = await writes.saveRecurrence({
        account_id: String(id),
        normalized_description: "parking garage",
        interval: "monthly",
        expected_amount: -2_000,
        currency: "USD",
        next_date: String(next),
        last_date: String(date),
        status: "confirmed",
        matched_count: 1,
        source: "manual",
        last_paid_transaction_id: transaction.id,
      });
      return { id: recurrence.id, next: String(recurrence.next_date) };
    },
    [accountId, TODAY, nextMonthSameDay(TODAY)] as const,
  );
  const row = page.getByTestId(`recurrence-${manual.id}`);
  await expect(row.getByTestId("name")).toHaveText("Parking Garage");
  await expect(row).toContainText("manual");
  await expect(row.getByTestId("next")).toHaveText(manual.next);
  const bill = page.getByTestId(`bill-${manual.id}`);
  await expect(bill).toHaveAttribute("data-state", "upcoming");
  await expect(bill.getByTestId("days")).toHaveText(daysText(daysBetween(TODAY, manual.next)));
  await expect(bill).toContainText("parking garage");
  await expect(page.getByTestId("monthly-total-USD")).toHaveText("-$20.00");
});
