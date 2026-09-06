import assert from "node:assert/strict";
import { test } from "node:test";

import {
  addMonths,
  goalProgress,
  monthsUntil,
  sortGoalsByPriority,
  wholeMonthsSince,
} from "../functions/shared/goals.ts";

/**
 * The shared goal arithmetic, which the nightly job uses to decide a goal is
 * reached and the goal page uses to draw it. The app's own selector has its
 * own test; this one is about the two kinds and the plan.
 */

/** Created on the 15th of March, so the 15th of August is five whole months on. */
const CREATED = Date.UTC(2026, 2, 15);
const TODAY = "2026-08-15";

function goal(fields) {
  return {
    id: "goal_1",
    name: "Holiday",
    target_amount: 200_000,
    status: "active",
    contributions: [],
    created_at: CREATED,
    ...fields,
  };
}

function context(balances = {}) {
  return { today: TODAY, balances: new Map(Object.entries(balances)) };
}

test("a save goal from contributions, with a plan to compare against", () => {
  const holiday = goal({
    planned_monthly: 10_000,
    contributions: [{ amount: 50_000 }, { amount: 25_000 }],
  });
  const progress = goalProgress(holiday, context());
  assert.equal(progress.saved, 75_000);
  assert.equal(progress.remaining, 125_000);
  assert.equal(progress.percent, 38);
  // Five whole months at 10 000 a month: ahead of the plan.
  assert.equal(progress.expectedByNow, 50_000);
  assert.equal(progress.onTrack, true);
  // Thirteen more months of the plan clears what is left.
  assert.equal(progress.payoffMonth, "2027-09");

  const unplanned = goalProgress(goal({ contributions: [{ amount: 1 }] }), context());
  assert.equal(unplanned.onTrack, null, "no plan is no verdict");
  assert.equal(unplanned.payoffMonth, null);
  assert.equal(unplanned.expectedByNow, 0);
});

test("a save goal from balances measures what changed since it began", () => {
  const linked = goal({
    progress_source: "balance",
    account_ids: ["acc_save", "acc_other"],
    starting_balance: 100_000,
    planned_monthly: 20_000,
  });
  const progress = goalProgress(linked, context({ acc_save: 150_000, acc_other: 20_000 }));
  assert.equal(progress.saved, 70_000);
  assert.equal(progress.remaining, 130_000);
  // 70 000 against 100 000 expected by now.
  assert.equal(progress.onTrack, false);

  // The original single link still counts when there is no list.
  const single = goal({ progress_source: "balance", account_id: "acc_save" });
  assert.equal(goalProgress(single, context({ acc_save: 30_000 })).saved, 30_000);
});

test("a pay-down goal measures the debt shrinking and projects the payoff", () => {
  const loan = goal({
    kind: "pay_down",
    target_amount: 0,
    account_ids: ["acc_loan"],
    // Recorded as the liability's balance, negative; read as what was owed.
    starting_balance: -300_000,
    planned_monthly: 30_000,
  });
  const progress = goalProgress(loan, context({ acc_loan: -180_000 }));
  assert.equal(progress.saved, 120_000, "paid so far");
  assert.equal(progress.remaining, 180_000, "still owed");
  assert.equal(progress.percent, 40);
  assert.equal(progress.expectedByNow, 150_000);
  assert.equal(progress.onTrack, false);
  assert.equal(progress.payoffMonth, "2027-02");

  const cleared = goalProgress(loan, context({ acc_loan: 0 }));
  assert.equal(cleared.remaining, 0);
  assert.equal(cleared.percent, 100);
  assert.equal(cleared.payoffMonth, null);

  // Without a starting figure the goal starts today: nothing paid yet, and
  // the target is what is owed.
  const fresh = goalProgress(
    goal({ kind: "pay_down", target_amount: 0, account_ids: ["acc_loan"] }),
    context({ acc_loan: -180_000 }),
  );
  assert.equal(fresh.saved, 0);
  assert.equal(fresh.percent, 0);
});

test("months to a date, the monthly figure, and the schedule's edges", () => {
  assert.equal(monthsUntil("2026-08-15", "2026-12-01"), 5);
  assert.equal(monthsUntil("2026-08-15", "2026-08-31"), 1);
  const dated = goal({ target_date: "2026-12-01", contributions: [{ amount: 50_000 }] });
  const progress = goalProgress(dated, context());
  assert.equal(progress.monthsLeft, 5);
  assert.equal(progress.monthlyNeeded, 30_000);
  // Late: all of it, now. Done: nothing. Undated: no schedule.
  assert.equal(goalProgress(goal({ target_date: "2026-06-01" }), context()).monthlyNeeded, 200_000);
  assert.equal(
    goalProgress(goal({ target_date: "2026-12-01", contributions: [{ amount: 250_000 }] }), context())
      .monthlyNeeded,
    0,
  );
  assert.equal(goalProgress(goal({}), context()).monthlyNeeded, null);
});

test("whole months since a moment turn over on the day, and months add across years", () => {
  assert.equal(wholeMonthsSince(CREATED, "2026-08-15"), 5);
  assert.equal(wholeMonthsSince(CREATED, "2026-08-14"), 4);
  assert.equal(wholeMonthsSince(CREATED, "2026-03-01"), 0, "never negative");
  assert.equal(addMonths("2026-11", 3), "2027-02");
  assert.equal(addMonths("2026-12", 1), "2027-01");
  assert.equal(addMonths("2026-01", -1), "2025-12");
});

test("goals sort active first, then by priority, then the soonest date", () => {
  const goals = [
    goal({ id: "g_archived", status: "archived", priority: 0, target_date: "2026-09-01" }),
    goal({ id: "g_second", priority: 2, target_date: "2026-09-01" }),
    goal({ id: "g_first_late", priority: 1, target_date: "2026-12-01" }),
    goal({ id: "g_unranked", target_date: "2026-01-01" }),
    goal({ id: "g_first_soon", priority: 1, target_date: "2026-10-01" }),
  ];
  assert.deepEqual(
    sortGoalsByPriority(goals).map((entry) => entry.id),
    ["g_first_soon", "g_first_late", "g_second", "g_unranked", "g_archived"],
  );
});
