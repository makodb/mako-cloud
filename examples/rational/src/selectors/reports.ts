import type { Account, NetWorthSnapshot, Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";
import { monthKey } from "./transactions.js";

/**
 * Reports are computed on the device, over the documents replication already
 * delivered. There is no aggregation API and this is deliberate: the numbers
 * are the ones the person can see, they update the moment a write lands
 * locally, and they are correct offline. What they cost is measured in
 * `test-unit/reports.test.mjs` against a seeded household, and recorded in
 * the findings log with the numbers rather than asserted away.
 *
 * Every total is minor units of one currency. A household with accounts in
 * two currencies gets two rows, never a conversion.
 */

/** Money in and out over one month, by currency. */
export interface CashFlowMonth {
  readonly month: string;
  readonly currency: string;
  readonly income: number;
  readonly expense: number;
  readonly net: number;
}

/** One bucket of a spending breakdown: a category, an account, or a month. */
export interface SpendingSlice {
  readonly key: string;
  readonly currency: string;
  readonly amount: number;
}

/**
 * A transaction's effect, split by split. A transaction without splits counts
 * once against its own category; one with splits counts each split against
 * its own, because that is what a split is for.
 */
function* effects(transaction: Transaction): Generator<{ categoryId: string; amount: number }> {
  if (transaction.splits.length === 0) {
    yield { categoryId: transaction.category_id ?? "", amount: transaction.amount };
    return;
  }
  for (const split of transaction.splits) {
    yield { categoryId: split.category_id ?? "", amount: split.amount };
  }
}

export function cashFlowByMonth(transactions: readonly Transaction[]): readonly CashFlowMonth[] {
  const months = new Map<string, { income: number; expense: number }>();
  for (const transaction of transactions) {
    const key = `${monthKey(transaction.date)}\u0000${transaction.currency}`;
    const total = months.get(key) ?? { income: 0, expense: 0 };
    if (transaction.amount >= 0) total.income += transaction.amount;
    else total.expense += -transaction.amount;
    months.set(key, total);
  }
  return [...months.entries()]
    .map(([key, total]) => {
      const [month = "", currency = ""] = key.split("\u0000");
      return {
        month,
        currency,
        income: total.income,
        expense: total.expense,
        net: total.income - total.expense,
      };
    })
    .sort(
      (left, right) =>
        right.month.localeCompare(left.month) || left.currency.localeCompare(right.currency),
    );
}

/**
 * What was spent per category. Only outgoing amounts count -- a refund
 * against a category reduces it rather than appearing as income -- and an
 * uncategorized transaction lands under the empty key, which the screen shows
 * as "uncategorized" rather than hiding.
 */
export function spendingByCategory(
  transactions: readonly Transaction[],
  month?: string,
): readonly SpendingSlice[] {
  const totals = new Map<string, number>();
  for (const transaction of transactions) {
    if (month !== undefined && monthKey(transaction.date) !== month) continue;
    for (const effect of effects(transaction)) {
      if (effect.amount >= 0) continue;
      const key = `${effect.categoryId}\u0000${transaction.currency}`;
      totals.set(key, (totals.get(key) ?? 0) + -effect.amount);
    }
  }
  return slices(totals);
}

export function spendingByAccount(
  transactions: readonly Transaction[],
  month?: string,
): readonly SpendingSlice[] {
  const totals = new Map<string, number>();
  for (const transaction of transactions) {
    if (month !== undefined && monthKey(transaction.date) !== month) continue;
    if (transaction.amount >= 0) continue;
    const key = `${transaction.account_id}\u0000${transaction.currency}`;
    totals.set(key, (totals.get(key) ?? 0) + -transaction.amount);
  }
  return slices(totals);
}

export function spendingByMonth(transactions: readonly Transaction[]): readonly SpendingSlice[] {
  const totals = new Map<string, number>();
  for (const transaction of transactions) {
    if (transaction.amount >= 0) continue;
    const key = `${monthKey(transaction.date)}\u0000${transaction.currency}`;
    totals.set(key, (totals.get(key) ?? 0) + -transaction.amount);
  }
  return [...totals.entries()]
    .map(([key, amount]) => {
      const [bucket = "", currency = ""] = key.split("\u0000");
      return { key: bucket, currency, amount };
    })
    .sort((left, right) => right.key.localeCompare(left.key));
}

/** Largest first, so a report leads with what the money went on. */
function slices(totals: ReadonlyMap<string, number>): readonly SpendingSlice[] {
  return [...totals.entries()]
    .map(([key, amount]) => {
      const [bucket = "", currency = ""] = key.split("\u0000");
      return { key: bucket, currency, amount };
    })
    .sort((left, right) => right.amount - left.amount || left.key.localeCompare(right.key));
}

/** Which accounts a spending-by-account report is about, for naming them. */
export function accountNames(accounts: readonly Account[]): ReadonlyMap<string, string> {
  return new Map(accounts.map((account) => [account.id, account.name]));
}

/**
 * The net-worth history the nightly job wrote, oldest first, with the shape
 * a chart needs.
 *
 * Rational draws it from stored snapshots rather than recomputing the past
 * from today's transactions: a transaction edited last week would rewrite
 * history, and a household that closed an account would watch its own past
 * disappear. What the snapshot recorded on a night is what that night was.
 */
export interface NetWorthPoint {
  readonly date: string;
  readonly netWorth: number;
  readonly assets: number;
  readonly liabilities: number;
  /** 0 at the lowest point of the series, 1 at the highest. */
  readonly position: number;
}

export interface NetWorthHistory {
  readonly currency: string;
  readonly points: readonly NetWorthPoint[];
  readonly low: number;
  readonly high: number;
  readonly change: number;
}

/** One history per currency the snapshots record, as the net-worth table is. */
export function netWorthHistories(
  snapshots: readonly NetWorthSnapshot[],
): readonly NetWorthHistory[] {
  const currencies = [...new Set(snapshots.map((snapshot) => snapshot.currency))].sort();
  const histories: NetWorthHistory[] = [];
  for (const currency of currencies) {
    const history = netWorthHistory(snapshots, currency);
    if (history !== null) histories.push(history);
  }
  return histories;
}

export function netWorthHistory(
  snapshots: readonly NetWorthSnapshot[],
  currency: string,
): NetWorthHistory | null {
  const ordered = snapshots
    .filter((snapshot) => snapshot.currency === currency)
    .sort((left, right) => left.date.localeCompare(right.date));
  if (ordered.length === 0) return null;
  const values = ordered.map((snapshot) => snapshot.net_worth);
  const low = Math.min(...values);
  const high = Math.max(...values);
  const span = high - low;
  const first = ordered[0];
  const last = ordered[ordered.length - 1];
  return {
    currency,
    points: ordered.map((snapshot) => ({
      date: snapshot.date,
      netWorth: snapshot.net_worth,
      assets: snapshot.assets,
      liabilities: snapshot.liabilities,
      position: span === 0 ? 0.5 : (snapshot.net_worth - low) / span,
    })),
    low,
    high,
    change: (last?.net_worth ?? 0) - (first?.net_worth ?? 0),
  };
}

/**
 * The polyline of a history, in a 0-100 by 0-100 box. One point draws a flat
 * line across the middle rather than nothing, so a household's first snapshot
 * still shows up.
 */
export function netWorthPath(history: NetWorthHistory): string {
  const { points } = history;
  if (points.length === 1) return "0,50 100,50";
  return points
    .map((point, index) => {
      const x = (index / (points.length - 1)) * 100;
      const y = 100 - point.position * 100;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}

export const selectNetWorthHistories = memoizeLast(netWorthHistories);
export const selectCashFlow = memoizeLast(cashFlowByMonth);
export const selectSpendingByCategory = memoizeLast(spendingByCategory);
export const selectSpendingByAccount = memoizeLast(spendingByAccount);
export const selectSpendingByMonth = memoizeLast(spendingByMonth);
