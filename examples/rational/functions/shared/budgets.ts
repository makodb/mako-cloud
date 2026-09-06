/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * Monthly budgets: per category, per group, for expected income, and the flex
 * number.
 *
 * A budget is one subject in one month, in minor units. Spending counts the
 * same way the reports do -- outgoing amounts, splits against their own
 * categories, a refund reducing the total, transfers and hidden and
 * balance-update transactions not counted at all (the test `exclusions.ts`
 * defines, repeated here because this file may import nothing) -- so the
 * number here, the number on the reports screen, and the number an
 * over-budget alert is about are all the same number.
 *
 * Rollover carries what a month did not spend into the next, and a month that
 * overspent carries the overspend forward as a smaller allowance. It compounds
 * across consecutive months of the same subject, which is what makes it
 * useful for the categories people actually roll over: a quarterly bill saved
 * for monthly, a holiday fund.
 */

export type BudgetKind = "category" | "group" | "income" | "flex";
export type BudgetBucket = "fixed" | "flexible" | "non_monthly";

/** The subjects of the two household-level budgets, used as their `category_id`. */
export const INCOME_SUBJECT = "income";
export const FLEX_SUBJECT = "flex";

/**
 * What the budget math reads of a budget. `category_id` names a category, a
 * group, or one of the two household-level subjects; `kind` says which, and
 * a budget written before kinds existed is a category budget.
 */
export interface BudgetLike {
  readonly id: string;
  readonly category_id: string;
  readonly month: string;
  readonly amount: number;
  readonly currency: string;
  readonly rollover: boolean;
  readonly kind?: BudgetKind;
}

/** What the budget math reads of a transaction. */
export interface BudgetTransaction {
  readonly date: string;
  readonly amount: number;
  readonly currency: string;
  readonly category_id?: string;
  readonly splits: ReadonlyArray<{ readonly category_id?: string; readonly amount: number }>;
  readonly hidden?: boolean;
  readonly adjustment?: boolean;
  readonly transfer_id?: string;
}

/** What the month model reads of a category. */
export interface BudgetCategoryLike {
  readonly id: string;
  readonly name: string;
  readonly parent_id?: string;
  readonly budget_bucket?: BudgetBucket;
  readonly target_amount?: number;
  readonly target_months?: number;
  readonly category_kind?: string;
  readonly archived?: boolean;
  readonly sort_order?: number;
}

/** What the month model reads of a category group. */
export interface BudgetGroupLike {
  readonly id: string;
  readonly name: string;
  readonly category_kind?: string;
  readonly sort_order?: number;
}

/** `2026-08-14` -> `2026-08`. */
export function monthKey(date: string): string {
  return date.slice(0, 7);
}

/** The id of the one budget a subject may have in a month; `bud_<subject>.<month>`. */
export function budgetSubjectId(subject: string, month: string): string {
  return `bud_${subject}.${month}`;
}

/**
 * The monthly share of a non-monthly category's target: this much every
 * `target_months` is that divided by the months, rounded to the cent. Zero
 * without both numbers, so a category still being set up costs nothing.
 */
export function nonMonthlyShare(category: {
  readonly target_amount?: number;
  readonly target_months?: number;
}): number {
  if (
    category.target_amount === undefined ||
    category.target_months === undefined ||
    category.target_months <= 0
  ) {
    return 0;
  }
  return Math.round(category.target_amount / category.target_months);
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

/** The same test `exclusions.ts` states; repeated because this file imports nothing. */
function isCounted(transaction: BudgetTransaction): boolean {
  return (
    transaction.hidden !== true &&
    transaction.adjustment !== true &&
    (transaction.transfer_id === undefined || transaction.transfer_id === "")
  );
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
    if (!isCounted(transaction)) continue;
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
 *
 * `spent` is the subject's own category spending; for a group, an income, or
 * a flex budget it is zero here, and `monthBudgetModel` supplies the figure
 * that makes sense for the subject.
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

/**
 * What kind of budget a document is. Documents written before kinds existed
 * carry none; their subject says: the two household-level subjects by name,
 * a known group id as a group budget, anything else a category budget.
 */
export function budgetKind(budget: BudgetLike, groupIds: ReadonlySet<string>): BudgetKind {
  if (budget.kind !== undefined) return budget.kind;
  if (budget.category_id === INCOME_SUBJECT) return "income";
  if (budget.category_id === FLEX_SUBJECT) return "flex";
  return groupIds.has(budget.category_id) ? "group" : "category";
}

export interface MonthBudgetInput {
  readonly budgets: readonly BudgetLike[];
  readonly transactions: readonly BudgetTransaction[];
  readonly categories: readonly BudgetCategoryLike[];
  readonly groups: readonly BudgetGroupLike[];
  readonly month: string;
  readonly currency: string;
  /** What the household's goals plan to put aside this month, in total. */
  readonly plannedGoalMonthly: number;
}

export interface MonthBudgetCategory {
  readonly category: BudgetCategoryLike;
  /** The category's own budget; null under a budgeted group, whatever documents exist. */
  readonly budget: BudgetStatus | null;
  readonly spent: number;
  /** The monthly share of a non-monthly target; zero for other buckets. */
  readonly share: number;
}

export interface MonthBudgetGroup {
  readonly group: BudgetGroupLike;
  /** The group's own budget, when it is budgeted as a whole. */
  readonly budget: BudgetStatus | null;
  readonly categories: readonly MonthBudgetCategory[];
  /** Over the group's categories. */
  readonly spent: number;
  /** The group budget's allowance when set, else the sum of its categories' allowances. */
  readonly budgeted: number;
  readonly remaining: number;
}

export interface MonthBudgetModel {
  readonly month: string;
  readonly currency: string;
  readonly income: {
    readonly budget: BudgetStatus | null;
    /** The income budget's allowance; zero when the month has none. */
    readonly expected: number;
    /** Counted money arriving in income categories or none. */
    readonly actual: number;
  };
  readonly groups: readonly MonthBudgetGroup[];
  /** Spending in expense categories (or none) with no budget and no budgeted group. */
  readonly unbudgeted: ReadonlyArray<{ readonly categoryId: string; readonly spent: number }>;
  readonly totals: {
    readonly budgeted: number;
    readonly spent: number;
    readonly remaining: number;
    /** Expected income less what is budgeted and what the goals plan to take. */
    readonly leftToBudget: number;
  };
  readonly flex: {
    /** The flex budget document, when one was written; the allowance below is derived regardless. */
    readonly budget: BudgetStatus | null;
    readonly fixed: { readonly budgeted: number; readonly spent: number };
    readonly nonMonthly: { readonly share: number; readonly spent: number };
    readonly flexible: {
      /** Income less fixed budgets, non-monthly shares, and planned goal contributions. */
      readonly allowance: number;
      /** Spending in flexible-bucket categories, categories with no bucket, and none. */
      readonly spent: number;
      readonly remaining: number;
    };
  };
}

/** The group categories without a parent, or with one that no longer exists, are shown under. */
export const UNGROUPED_ID = "";

/**
 * Counted money arriving this month whose category is an income category or
 * none. A refund into an expense category is not income: it already reduced
 * that category's spending, and counting it twice would flatter the month.
 */
export function incomeInMonth(
  transactions: readonly BudgetTransaction[],
  incomeCategoryIds: ReadonlySet<string>,
  month: string,
  currency: string,
): number {
  let total = 0;
  const counts = (categoryId: string | undefined): boolean =>
    categoryId === undefined || categoryId === "" || incomeCategoryIds.has(categoryId);
  for (const transaction of transactions) {
    if (!isCounted(transaction)) continue;
    if (transaction.currency !== currency || monthKey(transaction.date) !== month) continue;
    if (transaction.splits.length === 0) {
      if (transaction.amount > 0 && counts(transaction.category_id)) total += transaction.amount;
      continue;
    }
    for (const split of transaction.splits) {
      if (split.amount > 0 && counts(split.category_id)) total += split.amount;
    }
  }
  return total;
}

/** Counted money leaving under no category, as a positive figure. */
function uncategorizedOutflows(
  transactions: readonly BudgetTransaction[],
  month: string,
  currency: string,
): number {
  let total = 0;
  for (const transaction of transactions) {
    if (!isCounted(transaction)) continue;
    if (transaction.currency !== currency || monthKey(transaction.date) !== month) continue;
    if (transaction.splits.length === 0) {
      if ((transaction.category_id ?? "") === "" && transaction.amount < 0) {
        total += -transaction.amount;
      }
      continue;
    }
    for (const split of transaction.splits) {
      if ((split.category_id ?? "") === "" && split.amount < 0) total += -split.amount;
    }
  }
  return total;
}

function byOrder(
  left: { readonly sort_order?: number; readonly name: string; readonly id: string },
  right: { readonly sort_order?: number; readonly name: string; readonly id: string },
): number {
  return (
    (left.sort_order ?? Number.MAX_SAFE_INTEGER) - (right.sort_order ?? Number.MAX_SAFE_INTEGER) ||
    left.name.localeCompare(right.name) ||
    left.id.localeCompare(right.id)
  );
}

/**
 * One month of the household's budget, in the shape the budget page renders
 * in either mode. Only budgets of the given month and currency count: nothing
 * is converted, and a household with a euro account budgets it apart.
 *
 * Groups are the expense groups in their order, each with its expense
 * categories in theirs; categories with no group, or a group that is gone,
 * gather under an ungrouped entry at the end so a budget written for one is
 * never invisible. A category under a budgeted group reports no budget of its
 * own even when a stray category document exists, because the group's number
 * is the one the household chose to watch. Income is one expected number,
 * not a category list, and transfer groups are not budgeted at all.
 */
export function monthBudgetModel(input: MonthBudgetInput): MonthBudgetModel {
  const { month, currency, transactions, plannedGoalMonthly } = input;
  const groupIds = new Set(input.groups.map((group) => group.id));
  const monthly = input.budgets.filter(
    (budget) => budget.month === month && budget.currency === currency,
  );
  const bySubject = new Map<string, BudgetStatus>();
  for (const budget of monthly) {
    bySubject.set(
      `${budgetKind(budget, groupIds)}:${budget.category_id}`,
      budgetStatus(budget, input.budgets, transactions),
    );
  }
  const statusOf = (kind: BudgetKind, subject: string): BudgetStatus | null =>
    bySubject.get(`${kind}:${subject}`) ?? null;

  const incomeBudget = statusOf("income", INCOME_SUBJECT);
  const incomeCategoryIds = new Set(
    input.categories
      .filter((category) => category.category_kind === "income")
      .map((category) => category.id),
  );
  const income = {
    budget: incomeBudget,
    expected: incomeBudget?.allowance ?? 0,
    actual: incomeInMonth(transactions, incomeCategoryIds, month, currency),
  };

  const expenseCategories = input.categories.filter(
    (category) =>
      category.archived !== true &&
      (category.category_kind === undefined || category.category_kind === "expense"),
  );
  const expenseGroups = [...input.groups]
    .filter((group) => group.category_kind === undefined || group.category_kind === "expense")
    .sort(byOrder);
  const orphaned = expenseCategories.filter(
    (category) => category.parent_id === undefined || !groupIds.has(category.parent_id),
  );
  const shownGroups: BudgetGroupLike[] =
    orphaned.length === 0
      ? expenseGroups
      : [...expenseGroups, { id: UNGROUPED_ID, name: "Ungrouped" }];

  const groups: MonthBudgetGroup[] = [];
  const unbudgeted: Array<{ categoryId: string; spent: number }> = [];
  let fixedBudgeted = 0;
  let fixedSpent = 0;
  let nonMonthlyShareTotal = 0;
  let nonMonthlySpent = 0;
  let flexibleSpent = 0;
  for (const group of shownGroups) {
    const groupBudget = group.id === UNGROUPED_ID ? null : statusOf("group", group.id);
    const members = (
      group.id === UNGROUPED_ID
        ? orphaned
        : expenseCategories.filter((category) => category.parent_id === group.id)
    ).sort(byOrder);
    const categories: MonthBudgetCategory[] = members.map((category) => {
      const spent = spentInMonth(transactions, category.id, month, currency);
      const share = category.budget_bucket === "non_monthly" ? nonMonthlyShare(category) : 0;
      const budget = groupBudget === null ? statusOf("category", category.id) : null;
      if (category.budget_bucket === "fixed") {
        fixedBudgeted += budget?.allowance ?? 0;
        fixedSpent += spent;
      } else if (category.budget_bucket === "non_monthly") {
        nonMonthlyShareTotal += share;
        nonMonthlySpent += spent;
      } else {
        flexibleSpent += spent;
      }
      if (budget === null && groupBudget === null && spent > 0) {
        unbudgeted.push({ categoryId: category.id, spent });
      }
      return { category, budget, spent, share };
    });
    const spent = categories.reduce((total, entry) => total + entry.spent, 0);
    const budgeted =
      groupBudget?.allowance ??
      categories.reduce((total, entry) => total + (entry.budget?.allowance ?? 0), 0);
    groups.push({
      group,
      budget: groupBudget,
      categories,
      spent,
      budgeted,
      remaining: budgeted - spent,
    });
  }

  // Spending filed under no category at all is spending too, and the month's
  // total would be short without it. Only the outflows: an uncategorized
  // deposit was counted as income above, and a deposit cannot also be a
  // refund of nothing in particular.
  const uncategorized = uncategorizedOutflows(transactions, month, currency);
  if (uncategorized > 0) unbudgeted.push({ categoryId: "", spent: uncategorized });
  flexibleSpent += uncategorized;

  const budgeted = groups.reduce((total, group) => total + group.budgeted, 0);
  const spent = groups.reduce((total, group) => total + group.spent, 0) + uncategorized;
  const flexibleAllowance =
    income.expected - fixedBudgeted - nonMonthlyShareTotal - plannedGoalMonthly;
  return {
    month,
    currency,
    income,
    groups,
    unbudgeted,
    totals: {
      budgeted,
      spent,
      remaining: budgeted - spent,
      leftToBudget: income.expected - budgeted - plannedGoalMonthly,
    },
    flex: {
      budget: statusOf("flex", FLEX_SUBJECT),
      fixed: { budgeted: fixedBudgeted, spent: fixedSpent },
      nonMonthly: { share: nonMonthlyShareTotal, spent: nonMonthlySpent },
      flexible: {
        allowance: flexibleAllowance,
        spent: flexibleSpent,
        remaining: flexibleAllowance - flexibleSpent,
      },
    },
  };
}

/**
 * The budgets to create so `toMonth` has what `fromMonth` had: one for every
 * subject budgeted in the source month and not yet in the target, with the
 * same amount, rollover, kind, and currency. What the target month already
 * has is never touched -- copying is for the month a household has not
 * planned yet, not for undoing what it planned. Ids are derived, and the
 * household and timestamps are the caller's to stamp.
 */
export function copyMonthPlan(
  budgets: readonly BudgetLike[],
  fromMonth: string,
  toMonth: string,
): BudgetLike[] {
  if (fromMonth === toMonth) return [];
  const taken = new Set(
    budgets.filter((budget) => budget.month === toMonth).map((budget) => budget.category_id),
  );
  return budgets
    .filter((budget) => budget.month === fromMonth && !taken.has(budget.category_id))
    .sort((left, right) => left.category_id.localeCompare(right.category_id))
    .map((budget) => ({
      id: budgetSubjectId(budget.category_id, toMonth),
      category_id: budget.category_id,
      month: toMonth,
      amount: budget.amount,
      currency: budget.currency,
      rollover: budget.rollover,
      ...(budget.kind === undefined ? {} : { kind: budget.kind }),
    }));
}
