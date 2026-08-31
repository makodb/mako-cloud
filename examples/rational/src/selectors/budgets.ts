import {
  type BudgetStatus,
  monthBudgets as month,
  budgetStatus as status,
} from "../../functions/shared/budgets.js";
import type { Budget, Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * The budget math is shared with the `nightly` function, which decides
 * over-budget alerts: an alert about a number the screen does not show would
 * be worse than no alert. What stays here is the application's own types.
 */
export {
  type BudgetStatus,
  type BudgetTotals,
  budgetTotals,
  previousMonth,
  spentInMonth,
} from "../../functions/shared/budgets.js";

/** A status, narrowed to the application's own budget document. */
export type HouseholdBudgetStatus = Omit<BudgetStatus, "budget"> & { readonly budget: Budget };

export function budgetStatus(
  budget: Budget,
  budgets: readonly Budget[],
  transactions: readonly Transaction[],
): HouseholdBudgetStatus {
  return { ...status(budget, budgets, transactions), budget };
}

export function monthBudgets(
  budgets: readonly Budget[],
  transactions: readonly Transaction[],
  monthName: string,
): readonly HouseholdBudgetStatus[] {
  return month(budgets, transactions, monthName).map((entry) => ({
    ...entry,
    budget: entry.budget as Budget,
  }));
}

export const selectMonthBudgets = memoizeLast(monthBudgets);
