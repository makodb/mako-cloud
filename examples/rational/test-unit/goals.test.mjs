import assert from "node:assert/strict";
import { test } from "node:test";

import {
  contributed,
  goalProgress,
  goalRows,
  goalsByUrgency,
  monthsUntil,
  plannedMonthlyTotal,
} from "../dist/src/selectors/goals.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function goal(fields) {
  return {
    ...base,
    id: "goal_1",
    name: "Holiday",
    target_amount: 200_000,
    currency: "USD",
    status: "active",
    contributions: [],
    ...fields,
  };
}

test("progress is the sum of a goal's own contributions, not an account balance", () => {
  const holiday = goal({
    contributions: [
      { id: "c1", date: "2026-06-01", amount: 50_000 },
      { id: "c2", date: "2026-07-01", amount: 25_000 },
    ],
  });
  assert.equal(contributed(holiday), 75_000);
  const progress = goalProgress(holiday, "2026-08-01");
  assert.equal(progress.saved, 75_000);
  assert.equal(progress.remaining, 125_000);
  assert.equal(progress.percent, 38);
});

test("months are counted inclusively, and the monthly figure arrives on time", () => {
  assert.equal(monthsUntil("2026-08-15", "2026-12-01"), 5);
  assert.equal(monthsUntil("2026-08-15", "2026-08-31"), 1);
  assert.equal(monthsUntil("2026-08-15", "2027-08-01"), 13);

  const dated = goal({ target_date: "2026-12-01", contributions: [{ id: "c1", date: "2026-07-01", amount: 50_000 }] });
  const progress = goalProgress(dated, "2026-08-15");
  assert.equal(progress.monthsLeft, 5);
  // 150 000 over five months, rounded up so the last month is not short.
  assert.equal(progress.monthlyContribution, 30_000);
});

test("a goal that is late needs all of it now, and one that is there needs nothing", () => {
  const late = goal({ target_date: "2026-06-01" });
  assert.equal(goalProgress(late, "2026-08-15").monthlyContribution, 200_000);

  const done = goal({
    target_date: "2026-12-01",
    contributions: [{ id: "c1", date: "2026-07-01", amount: 250_000 }],
  });
  const progress = goalProgress(done, "2026-08-15");
  assert.equal(progress.remaining, 0);
  assert.equal(progress.monthlyContribution, 0);
  assert.equal(progress.percent, 100, "overshooting is not 125%");

  // No date is no schedule, rather than a schedule of zero.
  assert.equal(goalProgress(goal({}), "2026-08-15").monthlyContribution, null);
});

test("goal rows follow the household's priorities and read progress from balances when asked", () => {
  const goals = [
    goal({ id: "g_low", name: "Later", priority: 3, contributions: [{ id: "c", date: "2026-07-01", amount: 20_000 }] }),
    goal({
      id: "g_high",
      name: "Emergency fund",
      priority: 1,
      target_amount: 50_000,
      progress_source: "balance",
      account_ids: ["acc_sav"],
      starting_balance: 10_000,
    }),
    goal({
      id: "g_pay",
      name: "Car loan",
      priority: 2,
      kind: "pay_down",
      target_amount: 0,
      account_ids: ["acc_loan"],
      starting_balance: 100_000,
      planned_monthly: 10_000,
    }),
    goal({ id: "g_done", name: "Old", status: "archived", priority: 0 }),
  ];
  const balances = new Map([
    ["acc_sav", 30_000],
    ["acc_loan", -60_000],
  ]);
  const rows = goalRows(goals, balances, "2026-08-15");
  assert.deepEqual(
    rows.map((row) => row.goal.id),
    ["g_high", "g_pay", "g_low", "g_done"],
  );
  const [fund, loan, later] = rows;
  assert.equal(fund.math.saved, 20_000, "what the account gained since the goal began");
  assert.equal(fund.math.percent, 40);
  assert.equal(loan.math.saved, 40_000, "paid down so far");
  assert.equal(loan.math.remaining, 60_000, "still owed");
  assert.equal(loan.math.payoffMonth, "2027-02", "six more payments of the plan");
  assert.equal(later.math.saved, 20_000, "contributions, by default");
});

test("the planned monthly total is over active goals in the currency asked about", () => {
  const goals = [
    goal({ id: "a", planned_monthly: 50_000 }),
    goal({ id: "b", planned_monthly: 20_000 }),
    goal({ id: "c", planned_monthly: 99_999, status: "completed" }),
    goal({ id: "d", planned_monthly: 1_000, currency: "EUR" }),
    goal({ id: "e" }),
  ];
  assert.equal(plannedMonthlyTotal(goals, "USD"), 70_000);
  assert.equal(plannedMonthlyTotal(goals), 71_000);
});

test("active goals come first, then the soonest date", () => {
  const goals = [
    goal({ id: "g_archived", name: "Old", status: "archived", target_date: "2026-09-01" }),
    goal({ id: "g_late", name: "Roof", target_date: "2026-10-01" }),
    goal({ id: "g_soon", name: "Trip", target_date: "2026-09-01" }),
    goal({ id: "g_undated", name: "Rainy day" }),
  ];
  assert.deepEqual(
    goalsByUrgency(goals, "2026-08-15").map((progress) => progress.goal.id),
    ["g_soon", "g_late", "g_undated", "g_archived"],
  );
});
