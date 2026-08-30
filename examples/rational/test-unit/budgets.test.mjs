import assert from "node:assert/strict";
import { test } from "node:test";

import {
  budgetStatus,
  budgetTotals,
  monthBudgets,
  previousMonth,
  spentInMonth,
} from "../dist/src/selectors/budgets.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function budget(fields) {
  return { ...base, currency: "USD", rollover: false, ...fields };
}

function transaction(fields) {
  return {
    ...base,
    account_id: "acc_1",
    currency: "USD",
    description: "x",
    tags: [],
    splits: [],
    ...fields,
  };
}

test("spending counts splits against their own categories and refunds against the total", () => {
  const transactions = [
    transaction({ id: "t1", date: "2026-08-02", amount: -5_000, category_id: "cat_food" }),
    transaction({
      id: "t2",
      date: "2026-08-03",
      amount: -3_000,
      splits: [
        { id: "s1", category_id: "cat_food", amount: -1_000 },
        { id: "s2", category_id: "cat_home", amount: -2_000 },
      ],
    }),
    transaction({ id: "t3", date: "2026-08-04", amount: 500, category_id: "cat_food" }),
    transaction({ id: "t4", date: "2026-07-31", amount: -9_999, category_id: "cat_food" }),
    transaction({ id: "t5", date: "2026-08-05", amount: -1_000, category_id: "cat_food", currency: "EUR" }),
  ];
  assert.equal(spentInMonth(transactions, "cat_food", "2026-08", "USD"), 5_500);
  assert.equal(spentInMonth(transactions, "cat_home", "2026-08", "USD"), 2_000);
  assert.equal(spentInMonth(transactions, "cat_food", "2026-07", "USD"), 9_999);
});

test("a budget reports allowance, spent, remaining, and percent", () => {
  const august = budget({ id: "b1", category_id: "cat_food", month: "2026-08", amount: 40_000 });
  const status = budgetStatus(august, [august], [
    transaction({ id: "t1", date: "2026-08-02", amount: -10_000, category_id: "cat_food" }),
  ]);
  assert.deepEqual(
    { allowance: status.allowance, spent: status.spent, remaining: status.remaining, percent: status.percent },
    { allowance: 40_000, spent: 10_000, remaining: 30_000, percent: 25 },
  );
});

test("rollover compounds across an unbroken run, and an overspend carries forward", () => {
  const june = budget({ id: "b6", category_id: "cat_gift", month: "2026-06", amount: 10_000, rollover: true });
  const july = budget({ id: "b7", category_id: "cat_gift", month: "2026-07", amount: 10_000, rollover: true });
  const august = budget({ id: "b8", category_id: "cat_gift", month: "2026-08", amount: 10_000, rollover: true });
  const budgets = [june, july, august];
  const transactions = [
    transaction({ id: "t1", date: "2026-06-10", amount: -2_000, category_id: "cat_gift" }),
    transaction({ id: "t2", date: "2026-07-10", amount: -1_000, category_id: "cat_gift" }),
  ];
  // June had 10 000 and spent 2 000, so July carries in 8 000. July then has
  // 18 000 to spend, spends 1 000, and carries 17 000 into August.
  assert.equal(budgetStatus(july, budgets, transactions).carriedIn, 8_000);
  assert.equal(budgetStatus(august, budgets, transactions).carriedIn, 17_000);
  assert.equal(budgetStatus(august, budgets, transactions).allowance, 27_000);

  // An overspent month carries the overspend forward as a smaller allowance:
  // July's 18 000 against 25 000 spent leaves August 10 000 - 7 000.
  const overspent = [
    transaction({ id: "t1", date: "2026-06-10", amount: -2_000, category_id: "cat_gift" }),
    transaction({ id: "t3", date: "2026-07-10", amount: -25_000, category_id: "cat_gift" }),
  ];
  assert.equal(budgetStatus(august, budgets, overspent).carriedIn, -7_000);
  assert.equal(budgetStatus(august, budgets, overspent).allowance, 3_000);
});

test("a gap in the months breaks the run, and a budget that does not roll over carries nothing", () => {
  const january = budget({ id: "b1", category_id: "cat_gift", month: "2026-01", amount: 10_000, rollover: true });
  const june = budget({ id: "b6", category_id: "cat_gift", month: "2026-06", amount: 10_000, rollover: true });
  // Five months of nothing were not saved.
  assert.equal(budgetStatus(june, [january, june], []).carriedIn, 0);

  const plain = budget({ id: "b7", category_id: "cat_gift", month: "2026-07", amount: 10_000 });
  assert.equal(budgetStatus(plain, [june, plain], []).carriedIn, 0);
  assert.equal(previousMonth("2026-01"), "2025-12");
  assert.equal(previousMonth("2026-08"), "2026-07");
});

test("a month's budgets total per currency, never across them", () => {
  const budgets = [
    budget({ id: "b1", category_id: "cat_food", month: "2026-08", amount: 40_000 }),
    budget({ id: "b2", category_id: "cat_home", month: "2026-08", amount: 60_000 }),
    budget({ id: "b3", category_id: "cat_food", month: "2026-08", amount: 20_000, currency: "EUR" }),
    budget({ id: "b4", category_id: "cat_food", month: "2026-07", amount: 99_000 }),
  ];
  const statuses = monthBudgets(budgets, [], "2026-08");
  assert.deepEqual(
    statuses.map((status) => status.budget.id),
    ["b2", "b1", "b3"],
    "largest allowance first",
  );
  assert.deepEqual(budgetTotals(statuses), [
    { currency: "EUR", allowance: 20_000, spent: 0, remaining: 20_000 },
    { currency: "USD", allowance: 100_000, spent: 0, remaining: 100_000 },
  ]);
});
