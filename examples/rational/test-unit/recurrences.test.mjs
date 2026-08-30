import assert from "node:assert/strict";
import { test } from "node:test";

import {
  addDays,
  daysBetween,
  detectRecurrences,
  intervalOf,
  nextOccurrence,
  upcomingBills,
} from "../dist/src/selectors/recurrences.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function transaction(id, date, amount, description, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_1",
    date,
    amount,
    currency: "USD",
    description,
    tags: [],
    splits: [],
    ...fields,
  };
}

test("dates move by days and by months, keeping the day where a month is shorter", () => {
  assert.equal(daysBetween("2026-08-01", "2026-08-15"), 14);
  assert.equal(addDays("2026-08-30", 7), "2026-09-06");
  assert.equal(nextOccurrence("2026-01-31", "monthly"), "2026-02-28");
  assert.equal(nextOccurrence("2026-02-28", "monthly"), "2026-03-28");
  assert.equal(nextOccurrence("2026-08-15", "quarterly"), "2026-11-15");
  assert.equal(nextOccurrence("2026-08-15", "yearly"), "2027-08-15");
  assert.equal(nextOccurrence("2026-08-15", "biweekly"), "2026-08-29");
});

test("an interval is the one every gap is close to, or none", () => {
  assert.equal(intervalOf([30, 31, 28]), "monthly");
  assert.equal(intervalOf([7, 7, 8]), "weekly");
  assert.equal(intervalOf([90, 92]), "quarterly");
  // A monthly bill and a one-off in the middle: the gaps disagree.
  assert.equal(intervalOf([30, 5, 25]), null);
});

test("three occurrences at a steady interval are a recurrence; two are a coincidence", () => {
  const monthly = [
    transaction("t1", "2026-06-01", -1_299, "NETFLIX.COM 4471"),
    transaction("t2", "2026-07-01", -1_299, "NETFLIX.COM 8802"),
    transaction("t3", "2026-08-01", -1_499, "NETFLIX COM 9911"),
  ];
  const [detected, ...rest] = detectRecurrences(monthly);
  assert.equal(rest.length, 0);
  assert.equal(detected?.interval, "monthly");
  assert.equal(detected?.occurrences, 3);
  // The median, so one month's price rise does not become the expectation.
  assert.equal(detected?.expectedAmount, -1_299);
  assert.equal(detected?.lastDate, "2026-08-01");
  assert.equal(detected?.nextDate, "2026-09-01");
  assert.equal(detected?.description, "NETFLIX COM 9911");

  assert.deepEqual(detectRecurrences(monthly.slice(0, 2)), []);
});

test("income, one-offs, and other accounts are not recurrences", () => {
  const mixed = [
    transaction("i1", "2026-06-01", 250_000, "SALARY"),
    transaction("i2", "2026-07-01", 250_000, "SALARY"),
    transaction("i3", "2026-08-01", 250_000, "SALARY"),
    transaction("o1", "2026-06-02", -1_299, "GYM", { account_id: "acc_1" }),
    transaction("o2", "2026-07-02", -1_299, "GYM", { account_id: "acc_2" }),
    transaction("o3", "2026-08-02", -1_299, "GYM", { account_id: "acc_1" }),
  ];
  // Income is not a bill, and the gym charges are split across two accounts,
  // so neither account has three.
  assert.deepEqual(detectRecurrences(mixed), []);
});

test("a dismissed or confirmed recurrence is not detected again", () => {
  const monthly = [
    transaction("t1", "2026-06-01", -1_299, "NETFLIX"),
    transaction("t2", "2026-07-01", -1_299, "NETFLIX"),
    transaction("t3", "2026-08-01", -1_299, "NETFLIX"),
  ];
  const known = [
    {
      ...base,
      id: "rec_1",
      account_id: "acc_1",
      normalized_description: "netflix",
      interval: "monthly",
      expected_amount: -1_299,
      currency: "USD",
      next_date: "2026-09-01",
      status: "dismissed",
      matched_count: 3,
    },
  ];
  assert.deepEqual(detectRecurrences(monthly, known), []);
});

test("upcoming bills are soonest first, and a missed one stays as late", () => {
  const recurrence = (id, nextDate, status = "confirmed") => ({
    ...base,
    id,
    account_id: "acc_1",
    normalized_description: id,
    interval: "monthly",
    expected_amount: -1_299,
    currency: "USD",
    next_date: nextDate,
    status,
    matched_count: 3,
  });
  const bills = upcomingBills(
    [
      recurrence("late", "2026-08-25"),
      recurrence("soon", "2026-09-02"),
      recurrence("far", "2026-12-01"),
      recurrence("suggested", "2026-09-03", "suggested"),
    ],
    "2026-09-01",
  );
  assert.deepEqual(
    bills.map((bill) => [bill.recurrence.id, bill.daysAway]),
    [
      ["late", -7],
      ["soon", 1],
    ],
  );
});
