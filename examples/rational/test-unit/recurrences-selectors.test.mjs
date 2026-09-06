import assert from "node:assert/strict";
import { test } from "node:test";

import {
  bills,
  monthlyTotal,
  recurrenceName,
  upcomingBills,
} from "../dist/src/selectors/recurrences.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function recurrence(id, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_a",
    normalized_description: id,
    interval: "monthly",
    expected_amount: -5_000,
    currency: "USD",
    next_date: "2026-08-15",
    status: "confirmed",
    matched_count: 3,
    ...fields,
  };
}

function transaction(id, date, amount, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_a",
    date,
    amount,
    currency: "USD",
    description: id,
    tags: [],
    splits: [],
    ...fields,
  };
}

const recurrences = [
  recurrence("netflix", { normalized_description: "netflix com", expected_amount: -1_599 }),
  recurrence("water", { next_date: "2026-08-01" }),
  recurrence("gym", { interval: "weekly", next_date: "2026-08-25", expected_amount: -700 }),
  recurrence("guess", { status: "detected" }),
  recurrence("rest", { status: "paused" }),
];
const transactions = [transaction("t1", "2026-08-16", -1_599, { description: "NETFLIX.COM 0815" })];

test("bills wrap the shared life cycle: paid, late, and upcoming, soonest first", () => {
  const resolved = bills(recurrences, transactions, "2026-08-20");
  assert.deepEqual(
    resolved.map((bill) => [bill.recurrence.id, bill.state, bill.daysAway, bill.paidBy]),
    [
      ["water", "late", -19, undefined],
      ["netflix", "paid", -5, "t1"],
      ["gym", "upcoming", 5, undefined],
    ],
  );
  // The original list still works for the screens that read it.
  assert.deepEqual(
    upcomingBills(recurrences, "2026-08-20").map((bill) => bill.recurrence.id),
    ["water", "netflix", "gym"],
  );
});

test("a bill is called what the household named it, or its statement text made readable", () => {
  assert.equal(recurrenceName(recurrence("x", { name: "Netflix" })), "Netflix");
  assert.equal(recurrenceName(recurrence("x", { name: "  " , normalized_description: "sq corner grocer" })), "Corner Grocer");
  assert.equal(recurrenceName(recurrence("x", { normalized_description: "a" })), "a");
});

test("the monthly total is over confirmed recurrences, each at its monthly equivalent", () => {
  assert.deepEqual(monthlyTotal(recurrences), [
    { currency: "USD", total: -1_599 - 5_000 - Math.round((700 * 52) / 12) },
  ]);
});
