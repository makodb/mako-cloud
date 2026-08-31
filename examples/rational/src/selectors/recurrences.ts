import {
  type DetectedRecurrence,
  daysBetween,
  detectRecurrences as detect,
  type Interval,
} from "../../functions/shared/recurrences.js";
import type { Recurrence, Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * Recurrence detection is shared with the `nightly` function, so the engine
 * lives in `functions/shared/`: a household that has not opened Rational in a
 * month still gets its repeating bills noticed. What stays here is what only
 * the application has -- its document types, and the upcoming list it draws.
 */
export {
  addDays,
  type DetectedRecurrence,
  daysBetween,
  detectionId,
  type Interval,
  intervalOf,
  nextOccurrence,
  normalizeDescription,
} from "../../functions/shared/recurrences.js";

export function detectRecurrences(
  transactions: readonly Transaction[],
  known: readonly Recurrence[] = [],
): readonly DetectedRecurrence[] {
  return detect(transactions, known);
}

/**
 * A detection the nightly job already wrote down, shown the same way a fresh
 * one is -- the person still confirms or dismisses it.
 */
export function storedDetection(recurrence: Recurrence): DetectedRecurrence {
  return {
    accountId: recurrence.account_id,
    normalizedDescription: recurrence.normalized_description,
    description: recurrence.normalized_description,
    interval: recurrence.interval satisfies Interval,
    expectedAmount: recurrence.expected_amount,
    currency: recurrence.currency,
    lastDate: recurrence.last_date ?? recurrence.next_date,
    nextDate: recurrence.next_date,
    occurrences: recurrence.matched_count,
  };
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
