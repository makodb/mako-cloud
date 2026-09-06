import {
  type MonthBudgetModel,
  monthBudgetModel,
  previousMonth,
} from "../../functions/shared/budgets.js";
import {
  type Budget,
  type BudgetMode,
  type Goal,
  isCategory,
  isGroup,
  type TaxonomyEntry,
  type Transaction,
} from "../model/types.js";
import { addMonths, monthLabel } from "./cashflow.js";
import { plannedMonthlyTotal } from "./goals.js";
import { memoizeLast } from "./memo.js";

/**
 * The budget page's month, in either presentation.
 *
 * The model is the shared engine's (`functions/shared/budgets.ts`), which
 * also decides over-budget alerts; this adapter only takes the household's
 * documents apart for it -- categories and groups out of the taxonomy, the
 * planned goal contributions out of the goals -- and gives the page the
 * month arithmetic it navigates by.
 */
export {
  budgetKind,
  budgetSubjectId,
  copyMonthPlan,
  FLEX_SUBJECT,
  INCOME_SUBJECT,
  type MonthBudgetCategory,
  type MonthBudgetGroup,
  type MonthBudgetModel,
  monthBudgetModel,
  nonMonthlyShare,
  previousMonth,
  UNGROUPED_ID,
} from "../../functions/shared/budgets.js";
export { monthLabel } from "./cashflow.js";

export function nextMonth(month: string): string {
  return addMonths(month, 1);
}

/** Whether a month key is one the pager can move from; `previousMonth` returns its input when not. */
export function isMonthKey(value: string): boolean {
  return /^\d{4}-(0[1-9]|1[0-2])$/u.test(value) && previousMonth(value) !== value;
}

export function budgetMonth(
  budgets: readonly Budget[],
  transactions: readonly Transaction[],
  taxonomy: readonly TaxonomyEntry[],
  month: string,
  currency: string,
  goals: readonly Goal[],
): MonthBudgetModel {
  return monthBudgetModel({
    budgets,
    transactions,
    categories: taxonomy.filter(isCategory),
    groups: taxonomy.filter(isGroup),
    month,
    currency,
    plannedGoalMonthly: plannedMonthlyTotal(goals, currency),
  });
}

export const selectBudgetMonth = memoizeLast(budgetMonth);

/**
 * The one number the dashboard shows for the month's headroom: in flex mode
 * what is left of the flexible allowance, otherwise what is left of every
 * budget together.
 */
export function budgetRemaining(model: MonthBudgetModel, mode: BudgetMode = "category"): number {
  return mode === "flex" ? model.flex.flexible.remaining : model.totals.remaining;
}

export function monthTitle(month: string): string {
  return monthLabel(month);
}
