import assert from "node:assert/strict";
import { test } from "node:test";

import {
  budgetMonth,
  budgetRemaining,
  isMonthKey,
  monthLabel,
  nextMonth,
  previousMonth,
} from "../dist/src/selectors/budget-month.js";

/**
 * The adapter's job is to take the household's documents apart correctly for
 * the shared engine: categories and groups out of the taxonomy, the planned
 * contributions out of the active goals in the budget's currency. The tests
 * check both presentations of one month from one set of documents.
 */

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function entry(id, kind, fields = {}) {
  return { ...base, id, kind, name: id, ...fields };
}

const taxonomy = [
  entry("grp_income", "group", { category_kind: "income", sort_order: 0 }),
  entry("grp_home", "group", { category_kind: "expense", sort_order: 1 }),
  entry("grp_food", "group", { category_kind: "expense", sort_order: 2 }),
  entry("grp_auto", "group", { category_kind: "expense", sort_order: 3 }),
  entry("cat_pay", "category", { category_kind: "income", parent_id: "grp_income" }),
  entry("cat_rent", "category", {
    category_kind: "expense",
    parent_id: "grp_home",
    budget_bucket: "fixed",
  }),
  entry("cat_groceries", "category", {
    category_kind: "expense",
    parent_id: "grp_food",
    budget_bucket: "flexible",
  }),
  entry("cat_maint", "category", {
    category_kind: "expense",
    parent_id: "grp_auto",
    budget_bucket: "non_monthly",
    target_amount: 120_000,
    target_months: 12,
  }),
  entry("tag_x", "tag"),
  entry("mer_x", "merchant"),
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
  budget("income", 500_000, { kind: "income" }),
  budget("cat_rent", 150_000),
  budget("cat_groceries", 60_000),
];

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

const transactions = [
  transaction("t1", "2026-08-01", 500_000, { category_id: "cat_pay" }),
  transaction("t2", "2026-08-02", -150_000, { category_id: "cat_rent" }),
  transaction("t3", "2026-08-03", -45_000, { category_id: "cat_groceries" }),
  transaction("t4", "2026-08-04", -8_000, { category_id: "cat_maint" }),
  transaction("t5", "2026-08-05", -1_000, { category_id: "cat_groceries", hidden: true }),
];

function goal(id, fields = {}) {
  return {
    ...base,
    id,
    name: id,
    target_amount: 1_000_000,
    currency: "USD",
    status: "active",
    contributions: [],
    ...fields,
  };
}

const goals = [
  goal("g_active", { planned_monthly: 50_000 }),
  goal("g_archived", { planned_monthly: 99_999, status: "archived" }),
  goal("g_euro", { planned_monthly: 1_000, currency: "EUR" }),
];

test("category mode: groups with their categories, totals, and what is left to budget", () => {
  const model = budgetMonth(budgets, transactions, taxonomy, "2026-08", "USD", goals);
  assert.equal(model.income.expected, 500_000);
  assert.equal(model.income.actual, 500_000);
  assert.deepEqual(
    model.groups.map((group) => [group.group.id, group.budgeted, group.spent]),
    [
      ["grp_home", 150_000, 150_000],
      ["grp_food", 60_000, 45_000],
      ["grp_auto", 0, 8_000],
    ],
  );
  assert.deepEqual(model.totals, {
    budgeted: 210_000,
    spent: 203_000,
    remaining: 7_000,
    leftToBudget: 240_000,
  });
  assert.deepEqual(model.unbudgeted, [{ categoryId: "cat_maint", spent: 8_000 }]);
  assert.equal(budgetRemaining(model, "category"), 7_000);
  assert.equal(budgetRemaining(model), 7_000, "category mode is the default");
});

test("flex mode: flexible is income less fixed budgets, non-monthly shares, and planned goals", () => {
  const model = budgetMonth(budgets, transactions, taxonomy, "2026-08", "USD", goals);
  assert.deepEqual(model.flex.fixed, { budgeted: 150_000, spent: 150_000 });
  assert.deepEqual(model.flex.nonMonthly, { share: 10_000, spent: 8_000 });
  // 500 000 − 150 000 − 10 000 − 50 000: the archived goal and the euro goal do not count.
  assert.deepEqual(model.flex.flexible, { allowance: 290_000, spent: 45_000, remaining: 245_000 });
  assert.equal(budgetRemaining(model, "flex"), 245_000);
});

test("months step forward and back across a year boundary", () => {
  assert.equal(nextMonth("2026-12"), "2027-01");
  assert.equal(previousMonth("2027-01"), "2026-12");
  assert.equal(isMonthKey("2026-08"), true);
  assert.equal(isMonthKey("2026-13"), false);
  assert.equal(isMonthKey("august"), false);
  assert.equal(monthLabel("2026-08"), "August 2026");
});
