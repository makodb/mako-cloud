import assert from "node:assert/strict";
import { test } from "node:test";

import { contributed, goalProgress, goalsByUrgency, monthsUntil } from "../dist/src/selectors/goals.js";

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
