import assert from "node:assert/strict";
import { test } from "node:test";

import { unreadCount } from "../dist/src/selectors/alerts.js";
import { accountBalances, netWorthIn } from "../dist/src/selectors/balances.js";
import { budgetMonth, budgetRemaining } from "../dist/src/selectors/budget-month.js";
import { cashFlowSummary, monthRange, summaryFor } from "../dist/src/selectors/cashflow.js";
import { dashboardSummary } from "../dist/src/selectors/dashboard.js";
import { applyTransactionQuery } from "../dist/src/selectors/filters.js";
import { goalRows } from "../dist/src/selectors/goals.js";
import { netWorthChange } from "../dist/src/selectors/history.js";
import { investmentsTotal } from "../dist/src/selectors/holdings.js";
import { bills } from "../dist/src/selectors/recurrences.js";
import { reviewCount } from "../dist/src/selectors/review.js";

/**
 * The dashboard borrows every number from the selector its own screen uses.
 * The test computes each the way that screen would and asserts equality, then
 * spot-checks the values so a shared mistake cannot hide behind agreement.
 */

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };
const today = "2026-08-20";

function entry(id, kind, fields = {}) {
  return { ...base, id, kind, name: id, ...fields };
}

const taxonomy = [
  entry("grp_income", "group", { category_kind: "income" }),
  entry("grp_food", "group", { category_kind: "expense" }),
  entry("grp_home", "group", { category_kind: "expense" }),
  entry("cat_pay", "category", { category_kind: "income", parent_id: "grp_income" }),
  entry("cat_food", "category", {
    category_kind: "expense",
    parent_id: "grp_food",
    budget_bucket: "flexible",
  }),
  entry("cat_rent", "category", {
    category_kind: "expense",
    parent_id: "grp_home",
    budget_bucket: "fixed",
  }),
];

function account(id, type, opening, fields = {}) {
  return {
    ...base,
    id,
    name: id,
    type,
    currency: "USD",
    opening_balance: opening,
    opening_date: "2026-01-01",
    ...fields,
  };
}

const accounts = [
  account("acc_chk", "checking", 100_000),
  account("acc_sav", "savings", 50_000),
  account("acc_card", "credit", -20_000),
  account("acc_inv", "investment", 1_000, {
    holdings: [{ id: "h1", symbol: "VT", name: "World", quantity: 2, price: 10_000, asset_class: "etf" }],
  }),
  account("acc_hidden", "checking", 999, { hide_from_net_worth: true }),
];

function transaction(id, date, amount, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_chk",
    date,
    amount,
    currency: "USD",
    description: id,
    tags: [],
    splits: [],
    ...fields,
  };
}

const transactions = [
  transaction("x1", "2026-08-01", 300_000, { category_id: "cat_pay" }),
  transaction("x2", "2026-08-02", -120_000, { category_id: "cat_rent", description: "RENT" }),
  transaction("x3", "2026-08-05", -20_000, {
    category_id: "cat_food",
    account_id: "acc_card",
    external_id: "e1",
  }),
  transaction("x4", "2026-08-06", -5_000, { hidden: true }),
  transaction("x5", "2026-08-07", -10_000, { transfer_id: "tr1" }),
  transaction("x6", "2026-08-07", 10_000, { account_id: "acc_sav", transfer_id: "tr1" }),
  transaction("x7", "2026-07-10", -30_000, { category_id: "cat_food" }),
  transaction("x8", "2026-07-12", -7_000, {
    category_id: "cat_food",
    account_id: "acc_card",
    import_batch_id: "b1",
    reviewed: true,
  }),
  transaction("x9", "2026-08-18", -1_599, { description: "NETFLIX.COM" }),
  transaction("x10", "2026-08-19", -2_000, { category_id: "cat_food" }),
  transaction("x11", "2026-08-19", -3_000, { category_id: "cat_food", description: "second" }),
];

function budget(subject, amount, fields = {}) {
  return {
    ...base,
    id: `bud_${subject}.2026-08`,
    category_id: subject,
    month: "2026-08",
    amount,
    currency: "USD",
    rollover: false,
    ...fields,
  };
}
const budgets = [
  budget("cat_rent", 120_000),
  budget("cat_food", 40_000),
  budget("income", 300_000, { kind: "income" }),
];

function recurrence(id, nextDate, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_chk",
    normalized_description: id,
    interval: "monthly",
    expected_amount: -1_000,
    currency: "USD",
    next_date: nextDate,
    status: "confirmed",
    matched_count: 3,
    ...fields,
  };
}
const recurrences = [
  recurrence("r1", "2026-08-25"),
  recurrence("r2", "2026-08-10"),
  recurrence("r3", "2026-08-18", { normalized_description: "netflix com", expected_amount: -1_599 }),
  recurrence("r4", "2026-09-01"),
  recurrence("r5", "2026-08-21", { status: "paused" }),
];

function goal(id, priority, fields = {}) {
  return {
    ...base,
    id,
    name: id,
    target_amount: 100_000,
    currency: "USD",
    status: "active",
    contributions: [],
    priority,
    ...fields,
  };
}
const goals = [
  goal("g3", 3),
  goal("g1", 1, { planned_monthly: 20_000 }),
  goal("g4", 4),
  goal("g2", 2),
  goal("g5", 0, { status: "archived" }),
];

const alerts = [
  { ...base, id: "a1", kind: "alert", alert_kind: "large_transaction", fired_at: 5, read: false },
  { ...base, id: "a2", kind: "alert", alert_kind: "low_balance", fired_at: 6, read: true },
  { ...base, id: "a3", kind: "setting", alert_kind: "large_transaction", threshold: 1 },
];

const snapshots = [
  { ...base, id: "s1", date: "2026-07-20", assets: 100_000, liabilities: 0, net_worth: 100_000, currency: "USD" },
  { ...base, id: "s2", date: "2026-08-19", assets: 151_000, liabilities: 0, net_worth: 151_000, currency: "USD" },
];

const input = {
  accounts,
  transactions,
  taxonomy,
  budgets,
  recurrences,
  goals,
  alerts,
  snapshots,
  today,
  currency: "USD",
};

test("every dashboard number is the number its own screen shows", () => {
  const summary = dashboardSummary(input);
  const balances = accountBalances(accounts, transactions);
  const spending = (month) =>
    summaryFor(cashFlowSummary(transactions, taxonomy, monthRange(month)), "USD").spending;

  assert.deepEqual(summary.netWorth, netWorthIn("USD", accounts, balances));
  assert.equal(summary.netWorthChange30d, netWorthChange(snapshots, "USD", 30, today));
  assert.equal(summary.thisMonthSpending, spending("2026-08"));
  assert.equal(summary.lastMonthSpending, spending("2026-07"));
  assert.equal(
    summary.budgetRemaining,
    budgetRemaining(budgetMonth(budgets, transactions, taxonomy, "2026-08", "USD", goals)),
  );
  assert.deepEqual(
    summary.nextBills,
    bills(recurrences, transactions, today)
      .filter((bill) => bill.state !== "paid")
      .slice(0, 3),
  );
  assert.deepEqual(
    summary.goals,
    goalRows(goals, balances, today)
      .filter((row) => row.goal.status === "active")
      .slice(0, 3),
  );
  assert.equal(summary.reviewCount, reviewCount(transactions));
  assert.equal(summary.unreadAlerts, unreadCount(alerts));
  assert.deepEqual(
    summary.recentTransactions,
    applyTransactionQuery(transactions, {}, { categories: taxonomy, merchants: taxonomy }).slice(
      0,
      5,
    ),
  );
  assert.equal(
    summary.investmentsTotal,
    investmentsTotal(accounts, balances).find((total) => total.currency === "USD").total,
  );
});

test("and those numbers are the right ones", () => {
  const summary = dashboardSummary(input);
  assert.deepEqual(summary.netWorth, {
    currency: "USD",
    assets: 228_401 + 60_000 + 21_000,
    liabilities: 47_000,
    netWorth: 262_401,
  });
  assert.equal(summary.netWorthChange30d, 51_000);
  assert.equal(summary.thisMonthSpending, 146_599, "hidden and transfer legs left out");
  assert.equal(summary.lastMonthSpending, 37_000);
  assert.equal(summary.budgetRemaining, 160_000 - 146_599);
  assert.equal(dashboardSummary({ ...input, budgetMode: "flex" }).budgetRemaining, 133_401);
  assert.deepEqual(
    summary.nextBills.map((bill) => [bill.recurrence.id, bill.state]),
    [
      ["r2", "late"],
      ["r1", "upcoming"],
      ["r4", "upcoming"],
    ],
  );
  assert.deepEqual(
    summary.goals.map((row) => row.goal.id),
    ["g1", "g2", "g3"],
  );
  assert.equal(summary.reviewCount, 1);
  assert.equal(summary.unreadAlerts, 1);
  assert.deepEqual(
    summary.recentTransactions.map((entry) => entry.id),
    ["x11", "x10", "x9", "x6", "x5"],
  );
  assert.equal(summary.investmentsTotal, 21_000);
});
