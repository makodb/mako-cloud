import {
  type GoalMath,
  goalProgress as sharedGoalProgress,
  sortGoalsByPriority,
} from "../../functions/shared/goals.js";
import type { Goal } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * Saving towards something, paying something down, and what it takes to get
 * there on time.
 *
 * The arithmetic is the shared engine's (`functions/shared/goals.ts`): the
 * nightly job decides `goal_reached` from the same functions the bars here
 * are drawn from, and an alert about a goal the page shows as unfinished
 * would be worse than no alert. What stays here is the application's own
 * types and the two orders the screens show goals in.
 */
export {
  addMonths,
  contributed,
  type GoalMath,
  monthsUntil,
  sortGoalsByPriority,
  wholeMonthsSince,
} from "../../functions/shared/goals.js";

/** A goal's progress as the original goal page read it: contributions only. */
export interface GoalProgress {
  readonly goal: Goal;
  readonly saved: number;
  readonly remaining: number;
  /** Of the target, rounded, and never above 100 for a goal that overshot. */
  readonly percent: number;
  /** Whole months left, counting the target month itself; null with no date. */
  readonly monthsLeft: number | null;
  /**
   * What must go in each remaining month to arrive on time. Null when there
   * is no target date; zero when the goal is already there; the whole
   * remainder when the date has passed, because the answer to "how much a
   * month" for a goal that is late is "all of it, now".
   */
  readonly monthlyContribution: number | null;
}

const NO_BALANCES: ReadonlyMap<string, number> = new Map();

/**
 * Progress by contributions alone, whatever the goal's source says: the
 * contribution ledger a member keeps by hand, read without any balance.
 */
export function goalProgress(goal: Goal, today: string): GoalProgress {
  const math = sharedGoalProgress(
    { ...goal, kind: "save", progress_source: "contributions" },
    { today, balances: NO_BALANCES },
  );
  return {
    goal,
    saved: math.saved,
    remaining: math.remaining,
    percent: math.percent,
    monthsLeft: math.monthsLeft,
    monthlyContribution: math.monthlyNeeded,
  };
}

/** Active goals first, then the soonest target date, then by name. */
export function goalsByUrgency(goals: readonly Goal[], today: string): readonly GoalProgress[] {
  return goals
    .map((goal) => goalProgress(goal, today))
    .sort((left, right) => {
      if (left.goal.status !== right.goal.status) {
        return left.goal.status === "active" ? -1 : right.goal.status === "active" ? 1 : 0;
      }
      const leftDate = left.goal.target_date ?? "9999-12-31";
      const rightDate = right.goal.target_date ?? "9999-12-31";
      return leftDate.localeCompare(rightDate) || left.goal.name.localeCompare(right.goal.name);
    });
}

export const selectGoals = memoizeLast(goalsByUrgency);

/** A goal with the whole of its arithmetic, as the goals page and the dashboard show it. */
export interface GoalRow {
  readonly goal: Goal;
  readonly math: GoalMath;
}

/**
 * Every goal with its progress -- from its linked balances or its
 * contributions, as the goal says -- in the order the household ranked them:
 * active first, then priority, then the soonest date, then name.
 */
export function goalRows(
  goals: readonly Goal[],
  balances: ReadonlyMap<string, number>,
  today: string,
): readonly GoalRow[] {
  return sortGoalsByPriority(goals).map((goal) => ({
    goal,
    math: sharedGoalProgress(goal, { today, balances }),
  }));
}

export const selectGoalProgress = memoizeLast(goalRows);

/**
 * What the active goals plan to put aside each month, in total -- the number
 * the flex budget takes off the top. Given a currency, only goals in it, since
 * a budget is in one currency and nothing is converted.
 */
export function plannedMonthlyTotal(goals: readonly Goal[], currency?: string): number {
  let total = 0;
  for (const goal of goals) {
    if (goal.status !== "active") continue;
    if (currency !== undefined && goal.currency !== currency) continue;
    total += goal.planned_monthly ?? 0;
  }
  return total;
}
