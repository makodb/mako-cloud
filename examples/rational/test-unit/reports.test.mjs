import assert from "node:assert/strict";
import { test } from "node:test";

import { accountBalances, netWorthByCurrency } from "../dist/src/selectors/balances.js";
import {
  cashFlowByMonth,
  spendingByAccount,
  spendingByCategory,
  spendingByMonth,
} from "../dist/src/selectors/reports.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function transaction(fields) {
  return {
    ...base,
    currency: "USD",
    tags: [],
    splits: [],
    description: "x",
    ...fields,
  };
}

test("cash flow separates money in from money out, newest month first", () => {
  const rows = cashFlowByMonth([
    transaction({ id: "t1", account_id: "a", date: "2026-08-02", amount: 250_000 }),
    transaction({ id: "t2", account_id: "a", date: "2026-08-14", amount: -4_250 }),
    transaction({ id: "t3", account_id: "a", date: "2026-07-30", amount: -1_000 }),
  ]);
  assert.deepEqual(rows, [
    { month: "2026-08", currency: "USD", income: 250_000, expense: 4_250, net: 245_750 },
    { month: "2026-07", currency: "USD", income: 0, expense: 1_000, net: -1_000 },
  ]);
});

test("a currency is never converted into another", () => {
  const rows = cashFlowByMonth([
    transaction({ id: "t1", account_id: "a", date: "2026-08-02", amount: -1_000 }),
    transaction({ id: "t2", account_id: "b", date: "2026-08-03", amount: -2_000, currency: "EUR" }),
  ]);
  assert.deepEqual(
    rows.map((row) => [row.currency, row.expense]),
    [
      ["EUR", 2_000],
      ["USD", 1_000],
    ],
  );
});

test("splits count against their own categories, and a refund reduces one", () => {
  const rows = spendingByCategory([
    transaction({
      id: "t1",
      account_id: "a",
      date: "2026-08-02",
      amount: -5_000,
      splits: [
        { id: "s1", category_id: "cat_food", amount: -3_000 },
        { id: "s2", category_id: "cat_home", amount: -2_000 },
      ],
    }),
    transaction({ id: "t2", account_id: "a", date: "2026-08-03", amount: -1_000, category_id: "cat_food" }),
    // A refund against the same category: spending is what went out, net of
    // what came back, rather than a separate income row.
    transaction({ id: "t3", account_id: "a", date: "2026-08-04", amount: 500, category_id: "cat_food" }),
    transaction({ id: "t4", account_id: "a", date: "2026-08-05", amount: -700 }),
  ]);
  assert.deepEqual(
    rows.map((row) => [row.key, row.amount]),
    [
      ["cat_food", 4_000],
      ["cat_home", 2_000],
      ["", 700],
    ],
  );
});

test("a month narrows a category breakdown but never the months report", () => {
  const transactions = [
    transaction({ id: "t1", account_id: "a", date: "2026-08-02", amount: -1_000, category_id: "c" }),
    transaction({ id: "t2", account_id: "a", date: "2026-07-02", amount: -2_000, category_id: "c" }),
  ];
  assert.deepEqual(
    spendingByCategory(transactions, "2026-08").map((row) => row.amount),
    [1_000],
  );
  assert.deepEqual(
    spendingByAccount(transactions, "2026-07").map((row) => row.amount),
    [2_000],
  );
  assert.deepEqual(
    spendingByMonth(transactions).map((row) => [row.key, row.amount]),
    [
      ["2026-08", 1_000],
      ["2026-07", 2_000],
    ],
  );
});

/**
 * What client-side reporting costs, measured rather than assumed.
 *
 * There is no aggregation API, so every report is a pass over the documents
 * the device already holds. This is the number that decides whether that is a
 * design or a problem, and it belongs in the findings log with its value
 * rather than as an assertion nobody reads. The bound here is deliberately
 * loose -- it exists to catch an accidental quadratic, not to police the
 * milliseconds of whatever machine runs it.
 */
test("every report over 50 000 transactions stays interactive", () => {
  const accounts = Array.from({ length: 8 }, (unused, index) => ({
    ...base,
    id: `acc_${index}`,
    name: `Account ${index}`,
    type: index === 7 ? "credit_card" : "checking",
    currency: "USD",
    opening_balance: 100_000,
    opening_date: "2026-01-01",
  }));
  const transactions = Array.from({ length: 50_000 }, (unused, index) => {
    const month = 1 + (index % 12);
    const day = 1 + (index % 28);
    return transaction({
      id: `txn_${index}`,
      account_id: `acc_${index % 8}`,
      date: `2026-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`,
      amount: index % 11 === 0 ? 12_345 : -((index % 97) + 1) * 13,
      category_id: `cat_${index % 25}`,
      splits:
        index % 17 === 0
          ? [
              { id: `${index}-a`, category_id: `cat_${index % 25}`, amount: -100 },
              { id: `${index}-b`, category_id: `cat_${(index + 1) % 25}`, amount: -200 },
            ]
          : [],
    });
  });

  const measured = {};
  const measure = (name, run) => {
    const started = process.hrtime.bigint();
    const result = run();
    measured[name] = Number(process.hrtime.bigint() - started) / 1e6;
    return result;
  };

  const balances = measure("accountBalances", () => accountBalances(accounts, transactions));
  measure("netWorth", () => netWorthByCurrency(accounts, balances));
  const flow = measure("cashFlow", () => cashFlowByMonth(transactions));
  const byCategory = measure("spendingByCategory", () => spendingByCategory(transactions));
  measure("spendingByAccount", () => spendingByAccount(transactions));
  measure("spendingByMonth", () => spendingByMonth(transactions));

  // The numbers are the point; print them so a run records them.
  console.log(
    `reports over 50000 transactions (ms): ${Object.entries(measured)
      .map(([name, milliseconds]) => `${name}=${milliseconds.toFixed(1)}`)
      .join(" ")}`,
  );

  assert.equal(flow.length, 12, "one row per month of the seeded year");
  assert.equal(byCategory.length, 25, "one row per seeded category");
  const total = Object.values(measured).reduce((sum, value) => sum + value, 0);
  assert.ok(
    total < 5_000,
    `every report together took ${total.toFixed(0)}ms, which is past interactive`,
  );
});
