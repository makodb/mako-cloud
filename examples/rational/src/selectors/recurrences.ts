import type { Recurrence, Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";
import { normalizeDescription } from "./transactions.js";

/**
 * Finding the bills that repeat, and saying when the next one is due.
 *
 * Detection is over what the household already has: transactions on one
 * account whose normalized descriptions agree, sorted by date, whose gaps are
 * consistently close to a known interval. Three occurrences is the smallest
 * number that can show an interval twice, and a single pair is a coincidence.
 *
 * A detection is a suggestion, never a fact: the person confirms, adjusts, or
 * dismisses it. Rational writes nothing about a recurrence until they do,
 * because a wrong guess that quietly becomes an upcoming bill is worse than
 * no guess.
 */

export type Interval = "weekly" | "biweekly" | "monthly" | "quarterly" | "yearly";

/** Nominal length in days, and how far a gap may be from it and still count. */
const INTERVALS: ReadonlyArray<{ interval: Interval; days: number; slack: number }> = [
  { interval: "weekly", days: 7, slack: 2 },
  { interval: "biweekly", days: 14, slack: 3 },
  { interval: "monthly", days: 30, slack: 5 },
  { interval: "quarterly", days: 91, slack: 10 },
  { interval: "yearly", days: 365, slack: 20 },
];

export interface DetectedRecurrence {
  readonly accountId: string;
  readonly normalizedDescription: string;
  /** The most recent description, for showing it as the person sees it. */
  readonly description: string;
  readonly interval: Interval;
  /** The median of the occurrences, so one odd month does not move it. */
  readonly expectedAmount: number;
  readonly currency: string;
  readonly lastDate: string;
  readonly nextDate: string;
  readonly occurrences: number;
}

export function daysBetween(from: string, to: string): number {
  const start = Date.parse(`${from}T00:00:00Z`);
  const end = Date.parse(`${to}T00:00:00Z`);
  if (Number.isNaN(start) || Number.isNaN(end)) return Number.NaN;
  return Math.round((end - start) / 86_400_000);
}

export function addDays(date: string, days: number): string {
  const parsed = Date.parse(`${date}T00:00:00Z`);
  if (Number.isNaN(parsed)) return date;
  return new Date(parsed + days * 86_400_000).toISOString().slice(0, 10);
}

/**
 * The next occurrence after `from`, keeping the day of the month for monthly,
 * quarterly, and yearly intervals -- a bill due on the 31st is due on the
 * 31st, and in a shorter month on its last day rather than sliding into the
 * next one.
 */
export function nextOccurrence(from: string, interval: Interval): string {
  if (interval === "weekly") return addDays(from, 7);
  if (interval === "biweekly") return addDays(from, 14);
  const months = interval === "monthly" ? 1 : interval === "quarterly" ? 3 : 12;
  const [year = 0, month = 1, day = 1] = from.split("-").map(Number);
  const target = new Date(Date.UTC(year, month - 1 + months, 1));
  const lastDay = new Date(
    Date.UTC(target.getUTCFullYear(), target.getUTCMonth() + 1, 0),
  ).getUTCDate();
  target.setUTCDate(Math.min(day, lastDay));
  return target.toISOString().slice(0, 10);
}

function median(values: readonly number[]): number {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 1) return sorted[middle] ?? 0;
  return Math.round(((sorted[middle - 1] ?? 0) + (sorted[middle] ?? 0)) / 2);
}

/** The interval every gap is close to, or null when they disagree. */
export function intervalOf(gaps: readonly number[]): Interval | null {
  for (const candidate of INTERVALS) {
    if (gaps.every((gap) => Math.abs(gap - candidate.days) <= candidate.slack)) {
      return candidate.interval;
    }
  }
  return null;
}

/**
 * Every repeating charge the transactions show, most recent first. Existing
 * recurrences -- confirmed or dismissed -- are excluded, so a dismissal
 * stays dismissed.
 */
export function detectRecurrences(
  transactions: readonly Transaction[],
  known: readonly Recurrence[] = [],
): readonly DetectedRecurrence[] {
  const groups = new Map<string, Transaction[]>();
  for (const transaction of transactions) {
    if (transaction.amount >= 0) continue;
    const normalized = normalizeDescription(transaction.description);
    if (normalized === "") continue;
    const key = `${transaction.account_id}\u0000${normalized}\u0000${transaction.currency}`;
    const group = groups.get(key) ?? [];
    group.push(transaction);
    groups.set(key, group);
  }
  const excluded = new Set(
    known.map((entry) => `${entry.account_id}\u0000${entry.normalized_description}`),
  );
  const detected: DetectedRecurrence[] = [];
  for (const [key, group] of groups) {
    const [accountId = "", normalized = "", currency = ""] = key.split("\u0000");
    if (excluded.has(`${accountId}\u0000${normalized}`)) continue;
    if (group.length < 3) continue;
    const ordered = [...group].sort((left, right) => left.date.localeCompare(right.date));
    const gaps: number[] = [];
    for (let index = 1; index < ordered.length; index += 1) {
      gaps.push(daysBetween(ordered[index - 1]?.date ?? "", ordered[index]?.date ?? ""));
    }
    const interval = intervalOf(gaps);
    if (interval === null) continue;
    const last = ordered[ordered.length - 1];
    if (last === undefined) continue;
    detected.push({
      accountId,
      normalizedDescription: normalized,
      description: last.description,
      interval,
      expectedAmount: median(ordered.map((entry) => entry.amount)),
      currency,
      lastDate: last.date,
      nextDate: nextOccurrence(last.date, interval),
      occurrences: ordered.length,
    });
  }
  return detected.sort((left, right) => right.lastDate.localeCompare(left.lastDate));
}

export interface UpcomingBill {
  readonly recurrence: Recurrence;
  readonly dueDate: string;
  readonly expectedAmount: number;
  readonly currency: string;
  /** Negative when the bill is late. */
  readonly daysAway: number;
}

/**
 * What is due, soonest first. A confirmed recurrence whose next date has
 * passed without a matching transaction stays on the list as late rather than
 * disappearing: a bill nobody paid is the one worth showing.
 */
export function upcomingBills(
  recurrences: readonly Recurrence[],
  today: string,
  withinDays = 45,
): readonly UpcomingBill[] {
  return recurrences
    .filter((recurrence) => recurrence.status === "confirmed")
    .map((recurrence) => ({
      recurrence,
      dueDate: recurrence.next_date,
      expectedAmount: recurrence.expected_amount,
      currency: recurrence.currency,
      daysAway: daysBetween(today, recurrence.next_date),
    }))
    .filter((bill) => bill.daysAway <= withinDays)
    .sort((left, right) => left.daysAway - right.daysAway);
}

export const selectDetectedRecurrences = memoizeLast(detectRecurrences);
export const selectUpcomingBills = memoizeLast(upcomingBills);
