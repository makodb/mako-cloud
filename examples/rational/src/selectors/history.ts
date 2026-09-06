import { addDays, shiftOccurrence } from "../../functions/shared/recurrences.js";
import type { NetWorthSnapshot } from "../model/types.js";
import type { DateRange } from "./cashflow.js";
import { inRange } from "./cashflow.js";
import { memoizeLast } from "./memo.js";
import { type NetWorthHistory, netWorthHistory } from "./reports.js";

/**
 * What the nightly snapshots say about the past.
 *
 * History is read from stored snapshots rather than recomputed from today's
 * transactions, for the reason `reports.ts` gives: a transaction edited last
 * week would rewrite history, and a closed account would erase its own past.
 * This module only chooses which snapshots a chart is over and what one
 * account's line through them is; the drawing is the chart's business.
 */

export const HISTORY_RANGE_KEYS = ["1M", "3M", "6M", "1Y", "ALL"] as const;
export type HistoryRangeKey = (typeof HISTORY_RANGE_KEYS)[number];

/**
 * The first day a history range shows; null for all of it. A month back is a
 * calendar month, not thirty days, so "1M" on the 31st still means the whole
 * of last month.
 */
export function historyCutoff(key: HistoryRangeKey, today: string): string | null {
  switch (key) {
    case "1M":
      return shiftOccurrence(today, "monthly", -1);
    case "3M":
      return shiftOccurrence(today, "quarterly", -1);
    case "6M":
      return shiftOccurrence(today, "monthly", -6);
    case "1Y":
      return shiftOccurrence(today, "yearly", -1);
    case "ALL":
      return null;
  }
}

export function historyRange(key: HistoryRangeKey, today: string): DateRange | null {
  const start = historyCutoff(key, today);
  return start === null ? null : { start, end: today };
}

/** The net-worth history of one currency over a range, positions scaled to the range shown. */
export function netWorthHistoryInRange(
  snapshots: readonly NetWorthSnapshot[],
  currency: string,
  range: DateRange | null,
): NetWorthHistory | null {
  return netWorthHistory(
    snapshots.filter((snapshot) => inRange(snapshot.date, range)),
    currency,
  );
}

export const selectNetWorthHistoryInRange = memoizeLast(netWorthHistoryInRange);

export interface BalancePoint {
  readonly date: string;
  readonly balance: number;
}

/**
 * One account's balance night by night, oldest first. A snapshot from before
 * the nightly job recorded balances, or from before the account existed, has
 * nothing to say about it and is skipped rather than drawn as zero.
 */
export function accountBalanceHistory(
  snapshots: readonly NetWorthSnapshot[],
  accountId: string,
  range: DateRange | null = null,
): readonly BalancePoint[] {
  const points: BalancePoint[] = [];
  for (const snapshot of snapshots) {
    if (!inRange(snapshot.date, range)) continue;
    const entry = snapshot.balances?.find((balance) => balance.account_id === accountId);
    if (entry === undefined) continue;
    points.push({ date: snapshot.date, balance: entry.balance });
  }
  return points.sort((left, right) => left.date.localeCompare(right.date));
}

export const selectAccountBalanceHistory = memoizeLast(accountBalanceHistory);

/**
 * How net worth moved over the last so many days: the latest snapshot against
 * the last one on or before the cutoff -- or, for a household younger than
 * that, against its first. Null with no snapshot to read.
 */
export function netWorthChange(
  snapshots: readonly NetWorthSnapshot[],
  currency: string,
  days: number,
  today: string,
): number | null {
  const ordered = snapshots
    .filter((snapshot) => snapshot.currency === currency && snapshot.date <= today)
    .sort((left, right) => left.date.localeCompare(right.date));
  const latest = ordered.at(-1);
  const earliest = ordered[0];
  if (latest === undefined || earliest === undefined) return null;
  const cutoff = addDays(today, -days);
  let baseline = earliest;
  for (const snapshot of ordered) {
    if (snapshot.date > cutoff) break;
    baseline = snapshot;
  }
  return latest.net_worth - baseline.net_worth;
}
