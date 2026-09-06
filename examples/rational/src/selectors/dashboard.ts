import { previousMonth } from "../../functions/shared/budgets.js";
import type {
  Account,
  AlertDocument,
  Budget,
  BudgetMode,
  Goal,
  NetWorthSnapshot,
  Recurrence,
  TaxonomyEntry,
  Transaction,
} from "../model/types.js";
import { unreadCount } from "./alerts.js";
import { accountBalances, type NetWorth, netWorthIn } from "./balances.js";
import { budgetMonth, budgetRemaining } from "./budget-month.js";
import { cashFlowSummary, monthRange, summaryFor } from "./cashflow.js";
import { applyTransactionQuery } from "./filters.js";
import { type GoalRow, goalRows } from "./goals.js";
import { netWorthChange } from "./history.js";
import { investmentsTotal } from "./holdings.js";
import { memoizeLast } from "./memo.js";
import { bills, type ResolvedBill } from "./recurrences.js";
import { reviewCount } from "./review.js";
import { monthKey } from "./transactions.js";

/**
 * The landing page's numbers, every one of them borrowed.
 *
 * A dashboard that computed its own "spent this month" would sooner or later
 * disagree with the cash-flow page by a transfer or a hidden charge, and a
 * household that notices two numbers for one thing stops trusting both. So
 * nothing is derived here: each figure is the selector its own screen renders
 * from, called the same way, and the test asserts they are equal. What this
 * module adds is only the choice of what goes on the page and how much of it.
 */

export interface DashboardInput {
  readonly accounts: readonly Account[];
  readonly transactions: readonly Transaction[];
  readonly taxonomy: readonly TaxonomyEntry[];
  readonly budgets: readonly Budget[];
  readonly recurrences: readonly Recurrence[];
  readonly goals: readonly Goal[];
  readonly alerts: readonly AlertDocument[];
  readonly snapshots: readonly NetWorthSnapshot[];
  readonly today: string;
  /** The household's currency; every figure is in it, nothing is converted. */
  readonly currency: string;
  readonly budgetMode?: BudgetMode;
}

export interface DashboardSummary {
  readonly netWorth: NetWorth;
  /** Against the snapshot thirty days back; null before the first snapshot. */
  readonly netWorthChange30d: number | null;
  readonly thisMonthSpending: number;
  readonly lastMonthSpending: number;
  readonly budgetRemaining: number;
  /** The three soonest bills not yet paid, late ones first. */
  readonly nextBills: readonly ResolvedBill<Recurrence>[];
  /** The three active goals the household ranked highest. */
  readonly goals: readonly GoalRow[];
  readonly reviewCount: number;
  readonly unreadAlerts: number;
  /** The five newest transactions the list would show. */
  readonly recentTransactions: readonly Transaction[];
  readonly investmentsTotal: number;
}

export const DASHBOARD_BILLS = 3;
export const DASHBOARD_GOALS = 3;
export const DASHBOARD_TRANSACTIONS = 5;

export function dashboardSummary(input: DashboardInput): DashboardSummary {
  const { accounts, transactions, taxonomy, today, currency } = input;
  const balances = accountBalances(accounts, transactions);
  const thisMonth = monthKey(today);
  const lastMonth = previousMonth(thisMonth);
  const spendingIn = (month: string): number =>
    summaryFor(cashFlowSummary(transactions, taxonomy, monthRange(month)), currency).spending;
  const model = budgetMonth(
    input.budgets,
    transactions,
    taxonomy,
    thisMonth,
    currency,
    input.goals,
  );
  return {
    netWorth: netWorthIn(currency, accounts, balances),
    netWorthChange30d: netWorthChange(input.snapshots, currency, 30, today),
    thisMonthSpending: spendingIn(thisMonth),
    lastMonthSpending: spendingIn(lastMonth),
    budgetRemaining: budgetRemaining(model, input.budgetMode),
    nextBills: bills(input.recurrences, transactions, today)
      .filter((bill) => bill.state !== "paid")
      .slice(0, DASHBOARD_BILLS),
    goals: goalRows(input.goals, balances, today)
      .filter((row) => row.goal.status === "active")
      .slice(0, DASHBOARD_GOALS),
    reviewCount: reviewCount(transactions),
    unreadAlerts: unreadCount(input.alerts),
    recentTransactions: applyTransactionQuery(
      transactions,
      {},
      { categories: taxonomy, merchants: taxonomy },
    ).slice(0, DASHBOARD_TRANSACTIONS),
    investmentsTotal:
      investmentsTotal(accounts, balances).find((total) => total.currency === currency)?.total ?? 0,
  };
}

export const selectDashboard = memoizeLast(dashboardSummary);
