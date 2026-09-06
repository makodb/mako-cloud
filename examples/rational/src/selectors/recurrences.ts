import { cleanDescription } from "../../functions/shared/merchants.js";
import {
  type DetectedRecurrence,
  daysBetween,
  detectRecurrences as detect,
  type Interval,
  type ResolveBillsOptions,
  type ResolvedBill,
  resolveBills,
} from "../../functions/shared/recurrences.js";
import type { Recurrence, Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * Recurrence detection and the bill life cycle are shared with the `nightly`
 * function, so the engine lives in `functions/shared/`: a household that has
 * not opened Rational in a month still gets its repeating bills noticed and
 * its paid bills advanced. What stays here is what only the application has
 * -- its document types, and the lists it draws.
 */
export {
  addDays,
  advanceAfterPayment,
  type BillState,
  type DetectedRecurrence,
  daysBetween,
  detectionId,
  type Interval,
  intervalOf,
  monthlyEquivalent,
  monthlyTotal,
  nextOccurrence,
  normalizeDescription,
  occurrencesInMonth,
  type PaymentAdvance,
  paymentFor,
  previousOccurrence,
  type ResolveBillsOptions,
  type ResolvedBill,
  shiftOccurrence,
  slackDays,
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

/** What the household calls a bill, or the statement text made readable. */
export function recurrenceName(recurrence: Recurrence): string {
  if (recurrence.name !== undefined && recurrence.name.trim() !== "") return recurrence.name;
  const cleaned = cleanDescription(recurrence.normalized_description);
  return cleaned === "" ? recurrence.normalized_description : cleaned;
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

/**
 * Every confirmed bill with where it stands today -- upcoming, due, paid, or
 * late -- soonest first, decided by the same engine the nightly job advances
 * them with.
 */
export function bills(
  recurrences: readonly Recurrence[],
  transactions: readonly Transaction[],
  today: string,
  options?: ResolveBillsOptions,
): readonly ResolvedBill<Recurrence>[] {
  return resolveBills(recurrences, transactions, today, options);
}

export const selectDetectedRecurrences = memoizeLast(detectRecurrences);
export const selectUpcomingBills = memoizeLast(upcomingBills);
export const selectBills = memoizeLast(bills);
