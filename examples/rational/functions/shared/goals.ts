/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * Goal arithmetic for both kinds of goal.
 *
 * A save goal measures money put aside, either as the contributions recorded
 * against it or as the movement of its linked accounts since it began (the
 * balance when it was created is remembered, so a goal opened against a
 * savings account that already held money starts at zero). A pay-down goal
 * measures a liability shrinking: what was owed when it began less what is
 * owed now.
 *
 * The nightly job decides `goal_reached` from the same functions the goal
 * page draws its bars from, which is why they live here.
 */

export type GoalKind = "save" | "pay_down";
export type GoalProgressSource = "contributions" | "balance";

/** What the math reads of a goal. */
export interface GoalLike {
  readonly kind?: GoalKind;
  readonly target_amount: number;
  readonly target_date?: string;
  readonly planned_monthly?: number;
  readonly progress_source?: GoalProgressSource;
  readonly starting_balance?: number;
  readonly account_ids?: readonly string[];
  /** The original single link, still honoured when `account_ids` is absent. */
  readonly account_id?: string;
  readonly contributions: ReadonlyArray<{ readonly amount: number }>;
  readonly created_at: number;
  readonly status: string;
}

export interface GoalContext {
  readonly today: string;
  /** Balance per account id, derived the one way `balances.ts` derives them. */
  readonly balances: ReadonlyMap<string, number>;
}

export interface GoalMath {
  /** Put aside so far, or paid down so far. */
  readonly saved: number;
  /** Still to save, or still owed. */
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
  readonly monthlyNeeded: number | null;
  /** Whether saving has kept pace with the plan; null when there is no plan. */
  readonly onTrack: boolean | null;
  /** `planned_monthly` for every whole month since the goal began. */
  readonly expectedByNow: number;
  /** The month the plan clears what remains; null without a plan or with nothing left. */
  readonly payoffMonth: string | null;
}

/** Whole months from `today` to `target`, counting the target's own month. */
export function monthsUntil(today: string, target: string): number {
  const [todayYear = 0, todayMonth = 1] = today.split("-").map(Number);
  const [targetYear = 0, targetMonth = 1] = target.split("-").map(Number);
  return (targetYear - todayYear) * 12 + (targetMonth - todayMonth) + 1;
}

/**
 * Whole months from a moment to a day: a goal created on the 20th has been
 * going one month on the 20th of the next month, not on the 1st.
 */
export function wholeMonthsSince(createdAt: number, today: string): number {
  const created = new Date(createdAt);
  if (Number.isNaN(created.getTime())) return 0;
  const [year = 0, month = 1, day = 1] = today.split("-").map(Number);
  let months = (year - created.getUTCFullYear()) * 12 + (month - 1 - created.getUTCMonth());
  if (day < created.getUTCDate()) months -= 1;
  return Math.max(0, months);
}

/** `2026-11` plus 3 → `2027-02`. */
export function addMonths(month: string, count: number): string {
  const [year = 0, index = 1] = month.split("-").map(Number);
  const total = year * 12 + (index - 1) + count;
  const resultYear = Math.floor(total / 12);
  const resultMonth = total - resultYear * 12 + 1;
  return `${resultYear}-${String(resultMonth).padStart(2, "0")}`;
}

function linkedIds(goal: GoalLike): readonly string[] {
  if (goal.account_ids !== undefined && goal.account_ids.length > 0) return goal.account_ids;
  if (goal.account_id !== undefined && goal.account_id !== "") return [goal.account_id];
  return [];
}

function linkedBalance(goal: GoalLike, balances: ReadonlyMap<string, number>): number {
  let total = 0;
  for (const id of linkedIds(goal)) total += balances.get(id) ?? 0;
  return total;
}

export function contributed(goal: GoalLike): number {
  return goal.contributions.reduce((total, contribution) => total + contribution.amount, 0);
}

export function goalProgress(goal: GoalLike, context: GoalContext): GoalMath {
  let saved: number;
  let remaining: number;
  let target: number;
  if (goal.kind === "pay_down") {
    // Owed is a magnitude: a liability's balance is negative from the
    // household's side, and the starting figure may have been recorded
    // either way, so both are read as magnitudes.
    const owed = Math.abs(linkedBalance(goal, context.balances));
    const startedAt = goal.starting_balance === undefined ? owed : Math.abs(goal.starting_balance);
    target = goal.target_amount > 0 ? goal.target_amount : startedAt;
    saved = Math.max(0, startedAt - owed);
    remaining = owed;
  } else {
    saved =
      goal.progress_source === "balance"
        ? linkedBalance(goal, context.balances) - (goal.starting_balance ?? 0)
        : contributed(goal);
    target = goal.target_amount;
    remaining = Math.max(0, target - saved);
  }
  const percent =
    target <= 0
      ? remaining === 0
        ? 100
        : 0
      : Math.min(100, Math.max(0, Math.round((saved / target) * 100)));
  const monthsLeft =
    goal.target_date === undefined ? null : monthsUntil(context.today, goal.target_date);
  const monthlyNeeded =
    monthsLeft === null
      ? null
      : remaining === 0
        ? 0
        : monthsLeft <= 0
          ? remaining
          : Math.ceil(remaining / monthsLeft);
  const planned = goal.planned_monthly;
  const expectedByNow =
    planned === undefined ? 0 : planned * wholeMonthsSince(goal.created_at, context.today);
  const onTrack = planned === undefined ? null : saved >= expectedByNow;
  const payoffMonth =
    planned === undefined || planned <= 0 || remaining <= 0
      ? null
      : addMonths(context.today.slice(0, 7), Math.ceil(remaining / planned));
  return {
    saved,
    remaining,
    percent,
    monthsLeft,
    monthlyNeeded,
    onTrack,
    expectedByNow,
    payoffMonth,
  };
}

/**
 * Active goals first, then by the priority a member gave them (unranked ones
 * last), then the soonest target date, then name, then id -- so every device
 * shows the same order without a tie to break by hand.
 */
export function sortGoalsByPriority<
  T extends {
    readonly id: string;
    readonly status: string;
    readonly priority?: number;
    readonly target_date?: string;
    readonly name?: string;
  },
>(goals: readonly T[]): T[] {
  return [...goals].sort((left, right) => {
    if (left.status !== right.status) {
      return left.status === "active" ? -1 : right.status === "active" ? 1 : 0;
    }
    const leftPriority = left.priority ?? Number.POSITIVE_INFINITY;
    const rightPriority = right.priority ?? Number.POSITIVE_INFINITY;
    if (leftPriority !== rightPriority) return leftPriority < rightPriority ? -1 : 1;
    const leftDate = left.target_date ?? "9999-12-31";
    const rightDate = right.target_date ?? "9999-12-31";
    return (
      leftDate.localeCompare(rightDate) ||
      (left.name ?? "").localeCompare(right.name ?? "") ||
      left.id.localeCompare(right.id)
    );
  });
}
