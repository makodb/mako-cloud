import type { Goal } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * Saving towards something, and what it takes to get there on time.
 *
 * A goal's progress is the sum of its own contributions, not a balance: a
 * savings account holds several goals at once and a goal may be saved for
 * across accounts, so tying progress to a balance would make two goals in one
 * account both look complete. The linked account is where the money is meant
 * to live, which is a note to the person rather than a source of truth.
 */

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

export function contributed(goal: Goal): number {
  return goal.contributions.reduce((total, contribution) => total + contribution.amount, 0);
}

/** Whole months from `today` to `target`, counting the target's own month. */
export function monthsUntil(today: string, target: string): number {
  const [todayYear = 0, todayMonth = 1] = today.split("-").map(Number);
  const [targetYear = 0, targetMonth = 1] = target.split("-").map(Number);
  return (targetYear - todayYear) * 12 + (targetMonth - todayMonth) + 1;
}

export function goalProgress(goal: Goal, today: string): GoalProgress {
  const saved = contributed(goal);
  const remaining = Math.max(0, goal.target_amount - saved);
  const percent =
    goal.target_amount <= 0 ? 100 : Math.min(100, Math.round((saved / goal.target_amount) * 100));
  if (goal.target_date === undefined) {
    return { goal, saved, remaining, percent, monthsLeft: null, monthlyContribution: null };
  }
  const monthsLeft = monthsUntil(today, goal.target_date);
  const monthlyContribution =
    remaining === 0 ? 0 : monthsLeft <= 0 ? remaining : Math.ceil(remaining / monthsLeft);
  return { goal, saved, remaining, percent, monthsLeft, monthlyContribution };
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
