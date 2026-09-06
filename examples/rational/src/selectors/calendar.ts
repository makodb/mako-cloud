import { addDays, occurrencesInMonth } from "../../functions/shared/recurrences.js";
import type { Recurrence } from "../model/types.js";
import { lastDayOfMonth } from "./cashflow.js";
import { memoizeLast } from "./memo.js";

/**
 * The recurring page's calendar: a month of cells, and the bills that fall
 * on each day.
 *
 * The grid is whole weeks, so the first of the month sits under its weekday
 * and the days that pad the first and last rows belong to the neighbouring
 * months. Which day a week starts on is the household's habit, not the
 * calendar's, so it is a parameter. The bills are the confirmed recurrences'
 * occurrences in the month -- every one, not just the next -- from the same
 * engine the nightly job advances them with.
 */
export { monthlyEquivalent, monthlyTotal } from "../../functions/shared/recurrences.js";

export type WeekStart = 0 | 1;

export interface CalendarCell {
  readonly date: string;
  /** False for the padding days of the neighbouring months. */
  readonly inMonth: boolean;
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const;

/** The column headings, starting on the given day. */
export function weekdayLabels(weekStart: WeekStart = 0): readonly string[] {
  return [...WEEKDAYS.slice(weekStart), ...WEEKDAYS.slice(0, weekStart)];
}

function weekdayOf(date: string): number {
  return new Date(`${date}T00:00:00Z`).getUTCDay();
}

/** Whole weeks of seven cells covering the month; six rows at most. */
export function monthGrid(
  month: string,
  weekStart: WeekStart = 0,
): ReadonlyArray<readonly CalendarCell[]> {
  const first = `${month}-01`;
  const last = lastDayOfMonth(month);
  const lead = (weekdayOf(first) - weekStart + 7) % 7;
  const weeks: CalendarCell[][] = [];
  let date = addDays(first, -lead);
  while (date <= last && weeks.length < 6) {
    const week: CalendarCell[] = [];
    for (let index = 0; index < 7; index += 1) {
      week.push({ date, inMonth: date.slice(0, 7) === month });
      date = addDays(date, 1);
    }
    weeks.push(week);
  }
  return weeks;
}

export interface CalendarBill {
  readonly recurrence: Recurrence;
  readonly date: string;
  /** The expected amount, signed as stored: a bill is negative. */
  readonly amount: number;
}

/** The confirmed bills by the day they fall on, biggest first within a day. */
export function billsByDate(
  recurrences: readonly Recurrence[],
  month: string,
): ReadonlyMap<string, readonly CalendarBill[]> {
  const byDate = new Map<string, CalendarBill[]>();
  for (const recurrence of recurrences) {
    if (recurrence.status !== "confirmed") continue;
    for (const date of occurrencesInMonth(recurrence, month)) {
      const bills = byDate.get(date) ?? [];
      bills.push({ recurrence, date, amount: recurrence.expected_amount });
      byDate.set(date, bills);
    }
  }
  for (const bills of byDate.values()) {
    bills.sort(
      (left, right) =>
        Math.abs(right.amount) - Math.abs(left.amount) ||
        left.recurrence.id.localeCompare(right.recurrence.id),
    );
  }
  return byDate;
}

export const selectBillsByDate = memoizeLast(billsByDate);

/** What the calendar's month adds up to, per currency, signed as the bills are. */
export function calendarTotals(
  byDate: ReadonlyMap<string, readonly CalendarBill[]>,
): ReadonlyArray<{ readonly currency: string; readonly total: number }> {
  const totals = new Map<string, number>();
  for (const bills of byDate.values()) {
    for (const bill of bills) {
      const currency = bill.recurrence.currency;
      totals.set(currency, (totals.get(currency) ?? 0) + bill.amount);
    }
  }
  return [...totals.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([currency, total]) => ({ currency, total }));
}
