/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * Monthly budgets per category.
 *
 * A budget is one category in one month, in minor units. Spending counts the
 * same way the reports do -- outgoing amounts, splits against their own
 * categories, a refund reducing the total -- so the number here, the number
 * on the reports screen, and the number an over-budget alert is about are all
 * the same number.
 *
 * Rollover carries what a month did not spend into the next, and a month that
 * overspent carries the overspend forward as a smaller allowance. It compounds
 * across consecutive months of the same category, which is what makes it
 * useful for the categories people actually roll over: a quarterly bill saved
 * for monthly, a holiday fund.
 */

/** What the budget math reads of a budget. */
export interface BudgetLike {
  readonly id: string;
  readonly category_id: string;
  readonly month: string;
  readonly amount: number;
  readonly currency: string;
  readonly rollover: boolean;
}

/** What the budget math reads of a transaction. */
export interface BudgetTransaction {
  readonly date: string;
  readonly amount: number;
  readonly currency: string;
  readonly category_id?: string;
  readonly splits: ReadonlyArray<{ readonly category_id?: string; readonly amount: number }>;
}

/** `2026-08-14` -> `2026-08`. */
export function monthKey(date: string): string {
  return date.slice(0, 7);
}

export interface BudgetStatus {
  readonly budget: BudgetLike;
  /** The budget's own amount, before any rollover. */
  readonly amount: number;
  /** What earlier months carried in; zero when the budget does not roll over. */
  readonly carriedIn: number;
  /** What may be spent this month: amount plus what was carried in. */
  readonly allowance: number;
  readonly spent: number;
  readonly remaining: number;
  /** Of the allowance, rounded; 0 when the allowance is not positive. */
  readonly percent: number;
}

export interface BudgetTotals {
  readonly currency: string;
  readonly allowance: number;
  readonly spent: number;
  readonly remaining: number;
}

/** What was spent against one category in one month. */
export function spentInMonth(
  transactions: readonly BudgetTransaction[],
  categoryId: string,
  month: string,
  currency: string,
): number {
  let spent = 0;
  for (const transaction of transactions) {
    if (transaction.currency !== currency || monthKey(transaction.date) !== month) continue;
    if (transaction.splits.length === 0) {
      if ((transaction.category_id ?? "") === categoryId) spent += -transaction.amount;
      continue;
    }
    for (const split of transaction.splits) {
      if ((split.category_id ?? "") === categoryId) spent += -split.amount;
    }
  }
  return spent;
}

/**
 * A budget's month, including what earlier months of the same category rolled
 * over into it. Only budgets that themselves roll over contribute, and only
 * through an unbroken run of months ending at the one before this: a category
 * budgeted in January and again in June did not save five months of nothing.
 */
export function budgetStatus(
  budget: BudgetLike,
  budgets: readonly BudgetLike[],
  transactions: readonly BudgetTransaction[],
): BudgetStatus {
  const carriedIn = budget.rollover ? carriedInto(budget, budgets, transactions) : 0;
  const spent = spentInMonth(transactions, budget.category_id, budget.month, budget.currency);
  const allowance = budget.amount + carriedIn;
  return {
    budget,
    amount: budget.amount,
    carriedIn,
    allowance,
    spent,
    remaining: allowance - spent,
    percent: allowance > 0 ? Math.round((spent / allowance) * 100) : 0,
  };
}

function carriedInto(
  budget: BudgetLike,
  budgets: readonly BudgetLike[],
  transactions: readonly BudgetTransaction[],
): number {
  const previous = budgets.find(
    (candidate) =>
      candidate.category_id === budget.category_id &&
      candidate.currency === budget.currency &&
      candidate.month === previousMonth(budget.month),
  );
  if (previous === undefined || !previous.rollover) return 0;
  const earlier = carriedInto(previous, budgets, transactions);
  const spent = spentInMonth(transactions, previous.category_id, previous.month, previous.currency);
  return previous.amount + earlier - spent;
}

/** `2026-01` → `2025-12`. */
export function previousMonth(month: string): string {
  const [year = "", index = ""] = month.split("-");
  const numeric = Number(index);
  if (!Number.isInteger(numeric) || numeric < 1 || numeric > 12) return month;
  if (numeric === 1) return `${Number(year) - 1}-12`;
  return `${year}-${String(numeric - 1).padStart(2, "0")}`;
}

/** Every budget of one month, largest allowance first. */
export function monthBudgets(
  budgets: readonly BudgetLike[],
  transactions: readonly BudgetTransaction[],
  month: string,
): readonly BudgetStatus[] {
  return budgets
    .filter((budget) => budget.month === month)
    .map((budget) => budgetStatus(budget, budgets, transactions))
    .sort(
      (left, right) =>
        right.allowance - left.allowance ||
        left.budget.category_id.localeCompare(right.budget.category_id),
    );
}

/** The household's month, per currency, because nothing is converted. */
export function budgetTotals(statuses: readonly BudgetStatus[]): readonly BudgetTotals[] {
  const totals = new Map<string, { allowance: number; spent: number }>();
  for (const status of statuses) {
    const total = totals.get(status.budget.currency) ?? { allowance: 0, spent: 0 };
    total.allowance += status.allowance;
    total.spent += status.spent;
    totals.set(status.budget.currency, total);
  }
  return [...totals.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([currency, total]) => ({
      currency,
      allowance: total.allowance,
      spent: total.spent,
      remaining: total.allowance - total.spent,
    }));
}
