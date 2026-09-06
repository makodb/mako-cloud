import assert from "node:assert/strict";
import { test } from "node:test";

import {
  billsByDate,
  calendarTotals,
  monthGrid,
  monthlyTotal,
  weekdayLabels,
} from "../dist/src/selectors/calendar.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function recurrence(id, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_a",
    normalized_description: id,
    interval: "monthly",
    expected_amount: -1_000,
    currency: "USD",
    next_date: "2026-09-15",
    status: "confirmed",
    matched_count: 3,
    ...fields,
  };
}

test("the grid is whole weeks with the month's days marked and the padding not", () => {
  // August 2026 begins on a Saturday.
  const sunday = monthGrid("2026-08");
  assert.equal(sunday.length, 6);
  assert.ok(sunday.every((week) => week.length === 7));
  assert.deepEqual(sunday[0][0], { date: "2026-07-26", inMonth: false });
  assert.deepEqual(sunday[0][6], { date: "2026-08-01", inMonth: true });
  assert.deepEqual(sunday[5][6], { date: "2026-09-05", inMonth: false });
  assert.equal(sunday.flat().filter((cell) => cell.inMonth).length, 31);

  const monday = monthGrid("2026-08", 1);
  assert.deepEqual(monday[0][0], { date: "2026-07-27", inMonth: false });
  assert.deepEqual(monday[0][5], { date: "2026-08-01", inMonth: true });
  assert.deepEqual(weekdayLabels(1), ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]);

  // February 2026 begins on a Sunday and has 28 days: four weeks exactly.
  const february = monthGrid("2026-02");
  assert.equal(february.length, 4);
  assert.ok(february.flat().every((cell) => cell.inMonth));
});

test("bills fall on every occurrence in the month, confirmed ones only", () => {
  const recurrences = [
    recurrence("rent", { next_date: "2026-09-15", expected_amount: -150_000 }),
    recurrence("gym", { interval: "weekly", next_date: "2026-08-03", expected_amount: -2_500 }),
    recurrence("paused", { status: "paused" }),
    recurrence("guess", { status: "detected" }),
    recurrence("same-day", { next_date: "2026-08-15", expected_amount: -500 }),
  ];
  const byDate = billsByDate(recurrences, "2026-08");
  assert.deepEqual(
    [...byDate.keys()].sort(),
    ["2026-08-03", "2026-08-10", "2026-08-15", "2026-08-17", "2026-08-24", "2026-08-31"],
  );
  assert.deepEqual(
    byDate.get("2026-08-15").map((bill) => [bill.recurrence.id, bill.amount]),
    [
      ["rent", -150_000],
      ["same-day", -500],
    ],
  );
  assert.deepEqual(calendarTotals(byDate), [{ currency: "USD", total: -150_000 - 500 - 5 * 2_500 }]);
  assert.deepEqual(monthlyTotal(recurrences), [
    { currency: "USD", total: -150_000 - 500 - Math.round((2_500 * 52) / 12) },
  ]);
});
