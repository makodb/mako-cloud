import assert from "node:assert/strict";
import { test } from "node:test";

import {
  budgetStatus,
  budgetTotals,
  monthBudgets,
  previousMonth,
  spentInMonth,
} from "../dist/src/selectors/budgets.js";
import {
  budgetKind,
  budgetSubjectId,
  copyMonthPlan,
  incomeInMonth,
  monthBudgetModel,
  nonMonthlyShare,
} from "../functions/shared/budgets.ts";

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

test("a transfer leg, a hidden transaction, and a balance update are not spending", () => {
  const transactions = [
    transaction({ id: "t1", date: "2026-08-02", amount: -5_000, category_id: "cat_food" }),
    transaction({ id: "t2", date: "2026-08-03", amount: -3_000, category_id: "cat_food", hidden: true }),
    transaction({ id: "t3", date: "2026-08-04", amount: -2_000, category_id: "cat_food", transfer_id: "tr_1" }),
    transaction({ id: "t4", date: "2026-08-05", amount: -1_000, category_id: "cat_food", adjustment: true }),
  ];
  assert.equal(spentInMonth(transactions, "cat_food", "2026-08", "USD"), 5_000);
});

test("a budget's id is one per subject per month, the same the app writes", () => {
  assert.equal(budgetSubjectId("cat_food", "2026-08"), "bud_cat_food.2026-08");
  assert.equal(budgetSubjectId("grp_home", "2026-08"), "bud_grp_home.2026-08");
  assert.equal(budgetSubjectId("income", "2026-08"), "bud_income.2026-08");
  assert.equal(budgetSubjectId("flex", "2026-08"), "bud_flex.2026-08");
});

test("a budget's kind is stated, or read from its subject when it was written before kinds", () => {
  const groups = new Set(["grp_home"]);
  assert.equal(budgetKind(budget({ id: "b", category_id: "cat_food", month: "2026-08", amount: 1 }), groups), "category");
  assert.equal(budgetKind(budget({ id: "b", category_id: "grp_home", month: "2026-08", amount: 1 }), groups), "group");
  assert.equal(budgetKind(budget({ id: "b", category_id: "income", month: "2026-08", amount: 1 }), groups), "income");
  assert.equal(budgetKind(budget({ id: "b", category_id: "flex", month: "2026-08", amount: 1 }), groups), "flex");
  assert.equal(
    budgetKind(budget({ id: "b", category_id: "grp_home", month: "2026-08", amount: 1, kind: "category" }), groups),
    "category",
    "a stated kind wins",
  );
});

test("a non-monthly category's share is its target over its months, rounded", () => {
  assert.equal(nonMonthlyShare({ target_amount: 120_000, target_months: 12 }), 10_000);
  assert.equal(nonMonthlyShare({ target_amount: 100_000, target_months: 3 }), 33_333);
  assert.equal(nonMonthlyShare({ target_amount: 5 }), 0);
  assert.equal(nonMonthlyShare({ target_months: 3 }), 0);
  assert.equal(nonMonthlyShare({}), 0);
});

/** One household's August, with every wrinkle the budget page has to render. */
function august() {
  const groups = [
    { id: "grp_food", name: "Food", category_kind: "expense", sort_order: 1 },
    { id: "grp_home", name: "Home", category_kind: "expense", sort_order: 0 },
    { id: "grp_income", name: "Income", category_kind: "income", sort_order: 2 },
    { id: "grp_transfers", name: "Transfers", category_kind: "transfer", sort_order: 3 },
  ];
  const categories = [
    { id: "cat_groceries", name: "Groceries", parent_id: "grp_food", budget_bucket: "flexible" },
    { id: "cat_restaurants", name: "Restaurants", parent_id: "grp_food", budget_bucket: "flexible" },
    { id: "cat_archived", name: "Archived", parent_id: "grp_food", archived: true },
    { id: "cat_rent", name: "Rent", parent_id: "grp_home", budget_bucket: "fixed" },
    {
      id: "cat_repairs",
      name: "Repairs",
      parent_id: "grp_home",
      budget_bucket: "non_monthly",
      target_amount: 120_000,
      target_months: 12,
    },
    { id: "cat_pay", name: "Paychecks", parent_id: "grp_income", category_kind: "income" },
    { id: "cat_transfer", name: "Transfer", parent_id: "grp_transfers", category_kind: "transfer" },
    { id: "cat_old", name: "Old", category_kind: "expense" },
  ];
  const budgets = [
    budget({ id: "bud_income.2026-08", category_id: "income", kind: "income", month: "2026-08", amount: 500_000 }),
    budget({ id: "bud_cat_groceries.2026-08", category_id: "cat_groceries", month: "2026-08", amount: 40_000 }),
    budget({ id: "bud_grp_home.2026-08", category_id: "grp_home", kind: "group", month: "2026-08", amount: 200_000 }),
    // A stray category budget under a budgeted group: the group's number is the one shown.
    budget({ id: "bud_cat_rent.2026-08", category_id: "cat_rent", month: "2026-08", amount: 150_000 }),
    budget({ id: "eur", category_id: "cat_groceries", month: "2026-08", amount: 999, currency: "EUR" }),
    budget({ id: "july", category_id: "cat_groceries", month: "2026-07", amount: 1 }),
  ];
  const transactions = [
    transaction({ id: "t1", date: "2026-08-02", amount: -30_000, category_id: "cat_groceries" }),
    transaction({ id: "t2", date: "2026-08-03", amount: 1_000, category_id: "cat_groceries" }),
    transaction({ id: "t3", date: "2026-08-04", amount: -12_000, category_id: "cat_restaurants" }),
    transaction({ id: "t4", date: "2026-08-01", amount: -150_000, category_id: "cat_rent" }),
    transaction({ id: "t5", date: "2026-08-09", amount: -8_000, category_id: "cat_repairs" }),
    transaction({ id: "t6", date: "2026-08-15", amount: 600_000, category_id: "cat_pay" }),
    transaction({ id: "t7", date: "2026-08-16", amount: 5_000 }),
    transaction({ id: "t8", date: "2026-08-17", amount: -2_500 }),
    transaction({ id: "t9", date: "2026-08-18", amount: -4_000, category_id: "cat_old" }),
    transaction({ id: "t10", date: "2026-08-19", amount: 50_000, transfer_id: "tr_1" }),
    transaction({ id: "t11", date: "2026-08-20", amount: -7_000, category_id: "cat_groceries", currency: "EUR" }),
  ];
  return monthBudgetModel({
    budgets,
    transactions,
    categories,
    groups,
    month: "2026-08",
    currency: "USD",
    plannedGoalMonthly: 20_000,
  });
}

test("the month model: income is one expected number against what actually arrived", () => {
  const model = august();
  assert.equal(model.income.expected, 500_000);
  assert.equal(model.income.budget?.budget.id, "bud_income.2026-08");
  // The paycheck and the uncategorized deposit; not the transfer, not the refund.
  assert.equal(model.income.actual, 605_000);
  assert.equal(incomeInMonth([transaction({ id: "r", date: "2026-08-01", amount: 1_000, category_id: "cat_groceries" })], new Set(), "2026-08", "USD"), 0);
});

test("the month model: expense groups in order, a budgeted group over its categories, the ungrouped last", () => {
  const model = august();
  assert.deepEqual(
    model.groups.map((entry) => entry.group.id),
    ["grp_home", "grp_food", ""],
    "income and transfer groups are not budgeted; orphans gather at the end",
  );
  const [home, food, ungrouped] = model.groups;
  assert.equal(home.budget?.allowance, 200_000);
  assert.deepEqual(
    home.categories.map((entry) => [entry.category.id, entry.budget, entry.spent, entry.share]),
    [
      ["cat_rent", null, 150_000, 0],
      ["cat_repairs", null, 8_000, 10_000],
    ],
    "under a budgeted group a category has no budget of its own, stray document or not",
  );
  assert.deepEqual(
    { spent: home.spent, budgeted: home.budgeted, remaining: home.remaining },
    { spent: 158_000, budgeted: 200_000, remaining: 42_000 },
  );
  assert.equal(food.budget, null);
  assert.deepEqual(
    food.categories.map((entry) => [entry.category.id, entry.budget?.allowance ?? null, entry.spent]),
    [
      ["cat_groceries", 40_000, 29_000],
      ["cat_restaurants", null, 12_000],
    ],
    "an archived category is not shown; a refund reduces spending",
  );
  assert.deepEqual(
    { spent: food.spent, budgeted: food.budgeted, remaining: food.remaining },
    { spent: 41_000, budgeted: 40_000, remaining: -1_000 },
  );
  assert.deepEqual(ungrouped.categories.map((entry) => entry.category.id), ["cat_old"]);
});

test("the month model: unbudgeted spending, totals, and what is left to budget", () => {
  const model = august();
  assert.deepEqual(model.unbudgeted, [
    { categoryId: "cat_restaurants", spent: 12_000 },
    { categoryId: "cat_old", spent: 4_000 },
    { categoryId: "", spent: 2_500 },
  ]);
  assert.deepEqual(model.totals, {
    budgeted: 240_000,
    spent: 205_500,
    remaining: 34_500,
    leftToBudget: 240_000,
  });
});

test("the month model: flex mode is the same budgets read by bucket", () => {
  const model = august();
  assert.equal(model.flex.budget, null);
  // Rent is fixed but budgeted through its group, so no fixed budget of its own.
  assert.deepEqual(model.flex.fixed, { budgeted: 0, spent: 150_000 });
  assert.deepEqual(model.flex.nonMonthly, { share: 10_000, spent: 8_000 });
  // Flexible: groceries, restaurants, the bucketless old category, and the uncategorized charge.
  assert.deepEqual(model.flex.flexible, {
    allowance: 470_000,
    spent: 47_500,
    remaining: 422_500,
  });
});

test("copying a month fills what the target lacks and never overwrites", () => {
  const budgets = [
    budget({ id: "bud_cat_food.2026-08", category_id: "cat_food", month: "2026-08", amount: 40_000, rollover: true }),
    budget({ id: "bud_grp_home.2026-08", category_id: "grp_home", kind: "group", month: "2026-08", amount: 200_000 }),
    budget({ id: "bud_income.2026-08", category_id: "income", kind: "income", month: "2026-08", amount: 500_000 }),
    budget({ id: "bud_cat_food.2026-09", category_id: "cat_food", month: "2026-09", amount: 45_000 }),
  ];
  assert.deepEqual(copyMonthPlan(budgets, "2026-08", "2026-09"), [
    {
      id: "bud_grp_home.2026-09",
      category_id: "grp_home",
      month: "2026-09",
      amount: 200_000,
      currency: "USD",
      rollover: false,
      kind: "group",
    },
    {
      id: "bud_income.2026-09",
      category_id: "income",
      month: "2026-09",
      amount: 500_000,
      currency: "USD",
      rollover: false,
      kind: "income",
    },
  ]);
  assert.deepEqual(copyMonthPlan(budgets, "2026-08", "2026-08"), []);
  assert.deepEqual(copyMonthPlan(budgets, "2026-06", "2026-09"), [], "nothing to copy from an empty month");
});
