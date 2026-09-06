/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * Finding the bills that repeat, saying when the next one is due, and telling
 * a paid one from a late one.
 *
 * Detection is over what the household already has: transactions on one
 * account whose normalized descriptions agree, sorted by date, whose gaps are
 * consistently close to a known interval. Three occurrences is the smallest
 * number that can show an interval twice, and a single pair is a coincidence.
 *
 * A detection is a suggestion, never a fact: the person confirms, adjusts, or
 * dismisses it. Neither the application nor the nightly job turns one into a
 * bill on its own, because a wrong guess that quietly becomes an upcoming
 * bill is worse than no guess.
 *
 * A confirmed bill lives on: it is paid when a transaction with its
 * description lands on its account within the interval's slack of the
 * expected date, which moves the expected date on by one interval; it is
 * late when the slack has passed with no such transaction. The application
 * and the nightly job both advance paid bills, so a laptop left closed does
 * not leave a bill "late" that the bank shows as paid.
 */

export type Interval = "weekly" | "biweekly" | "monthly" | "quarterly" | "yearly";
export type RecurrenceStatus = "detected" | "confirmed" | "dismissed" | "paused";

/**
 * What detection reads of a transaction. The three exclusion fields are the
 * test `exclusions.ts` states, repeated here because this file imports
 * nothing: a transfer leg, a balance update, or a hidden charge is not a bill.
 */
export interface RecurrenceSubject {
  readonly account_id: string;
  readonly description: string;
  readonly amount: number;
  readonly currency: string;
  readonly date: string;
  readonly hidden?: boolean;
  readonly adjustment?: boolean;
  readonly transfer_id?: string;
}

/** What detection reads of a recurrence the household already has. */
export interface KnownRecurrence {
  readonly account_id: string;
  readonly normalized_description: string;
}

/** What the bill life cycle reads of a recurrence. */
export interface BillRecurrence {
  readonly id: string;
  readonly account_id: string;
  readonly normalized_description: string;
  readonly interval: Interval;
  readonly expected_amount: number;
  readonly currency: string;
  readonly next_date: string;
  readonly status: RecurrenceStatus | string;
  readonly matched_count: number;
  readonly name?: string;
  readonly last_date?: string;
  readonly last_paid_transaction_id?: string;
}

/** What the bill life cycle reads of a transaction. */
export interface BillTransaction extends RecurrenceSubject {
  readonly id: string;
  readonly normalized_description?: string;
}

/** Nominal length in days, and how far a gap may be from it and still count. */
const INTERVALS: ReadonlyArray<{ interval: Interval; days: number; slack: number }> = [
  { interval: "weekly", days: 7, slack: 2 },
  { interval: "biweekly", days: 14, slack: 3 },
  { interval: "monthly", days: 30, slack: 5 },
  { interval: "quarterly", days: 91, slack: 10 },
  { interval: "yearly", days: 365, slack: 20 },
];

/** How many days either side of the expected date a payment may land. */
export function slackDays(interval: Interval): number {
  return INTERVALS.find((entry) => entry.interval === interval)?.slack ?? 5;
}

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

/**
 * Descriptions as banks write them carry a card's last four digits, a store
 * number, a date -- none of which name the merchant. What survives dropping
 * digits and punctuation is what two occurrences of one bill share.
 */
export function normalizeDescription(description: string): string {
  return description
    .toLowerCase()
    .replaceAll(/[0-9#*]+/gu, " ")
    .replaceAll(/[^a-z ]+/gu, " ")
    .replaceAll(/\s+/gu, " ")
    .trim();
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
 * The occurrence `count` intervals from `from` -- forward when positive,
 * back when negative -- keeping the day of the month for monthly, quarterly,
 * and yearly intervals: a bill due on the 31st is due on the 31st, and in a
 * shorter month on its last day rather than sliding into the next one.
 * Stepping several intervals at once, rather than one at a time, is what
 * keeps the 31st from drifting to the 28th after passing through February.
 */
export function shiftOccurrence(from: string, interval: Interval, count: number): string {
  if (interval === "weekly") return addDays(from, 7 * count);
  if (interval === "biweekly") return addDays(from, 14 * count);
  const months = (interval === "monthly" ? 1 : interval === "quarterly" ? 3 : 12) * count;
  const [year = 0, month = 1, day = 1] = from.split("-").map(Number);
  const target = new Date(Date.UTC(year, month - 1 + months, 1));
  const lastDay = new Date(
    Date.UTC(target.getUTCFullYear(), target.getUTCMonth() + 1, 0),
  ).getUTCDate();
  target.setUTCDate(Math.min(day, lastDay));
  return target.toISOString().slice(0, 10);
}

/** The next occurrence after `from`. */
export function nextOccurrence(from: string, interval: Interval): string {
  return shiftOccurrence(from, interval, 1);
}

/** The occurrence before `from`. */
export function previousOccurrence(from: string, interval: Interval): string {
  return shiftOccurrence(from, interval, -1);
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

/** The same test `exclusions.ts` states; repeated because this file imports nothing. */
function isCounted(transaction: RecurrenceSubject): boolean {
  return (
    transaction.hidden !== true &&
    transaction.adjustment !== true &&
    (transaction.transfer_id === undefined || transaction.transfer_id === "")
  );
}

/**
 * Every repeating charge the transactions show, most recent first. Existing
 * recurrences -- detected, confirmed, paused, or dismissed -- are excluded,
 * so a dismissal stays dismissed and the nightly job does not propose again
 * what it proposed last night. Transfers, balance updates, and hidden
 * transactions are not charges and are not looked at.
 */
export function detectRecurrences(
  transactions: readonly RecurrenceSubject[],
  known: readonly KnownRecurrence[] = [],
): readonly DetectedRecurrence[] {
  const groups = new Map<string, RecurrenceSubject[]>();
  for (const transaction of transactions) {
    if (transaction.amount >= 0 || !isCounted(transaction)) continue;
    const normalized = normalizeDescription(transaction.description);
    if (normalized === "") continue;
    const key = groupKey(transaction.account_id, normalized, transaction.currency);
    const group = groups.get(key);
    if (group === undefined) groups.set(key, [transaction]);
    else group.push(transaction);
  }
  const excluded = new Set(
    known.map((entry) => pairKey(entry.account_id, entry.normalized_description)),
  );
  const detected: DetectedRecurrence[] = [];
  for (const [key, group] of groups) {
    const [accountId = "", normalized = "", currency = ""] = key.split(SEPARATOR);
    if (excluded.has(pairKey(accountId, normalized))) continue;
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

/** A separator no account id, description, or currency code can contain. */
const SEPARATOR = "\u001f";

function groupKey(accountId: string, normalized: string, currency: string): string {
  return `${accountId}${SEPARATOR}${normalized}${SEPARATOR}${currency}`;
}

function pairKey(accountId: string, normalized: string): string {
  return `${accountId}${SEPARATOR}${normalized}`;
}

/**
 * A stable id for a detection, so the nightly job proposing the same bill on
 * two nights writes one document rather than two. `.` rather than `:`, as
 * everything a function writes must be.
 */
export function detectionId(householdId: string, detection: DetectedRecurrence): string {
  return `rec_${householdId}.${detection.accountId}.${digest(detection.normalizedDescription)}`;
}

/** FNV-1a, for a short id-safe digest of a description. */
function digest(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, "0");
}

export type BillState = "upcoming" | "due" | "paid" | "late";

export interface ResolvedBill<T extends BillRecurrence = BillRecurrence> {
  readonly recurrence: T;
  readonly dueDate: string;
  /** Days from today to the due date; negative once it has passed. */
  readonly daysAway: number;
  readonly state: BillState;
  /** The transaction that paid it, when one did. */
  readonly paidBy?: string;
}

export interface ResolveBillsOptions {
  /** Drop upcoming bills further away than this; late, due, and paid ones always show. */
  readonly withinDays?: number;
}

/**
 * The transaction that paid a bill: counted, on the bill's account, with its
 * normalized description and the same sign as its expected amount, landing
 * within the interval's slack of the expected date. The one closest to the
 * date wins when two do, and ties go to the earlier one by id so every
 * device advances the bill with the same transaction.
 */
export function paymentFor<T extends BillTransaction>(
  recurrence: BillRecurrence,
  transactions: readonly T[],
): T | null {
  const slack = slackDays(recurrence.interval);
  const expectedSign = Math.sign(recurrence.expected_amount);
  let best: T | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const transaction of transactions) {
    if (transaction.account_id !== recurrence.account_id || !isCounted(transaction)) continue;
    if (expectedSign !== 0 && Math.sign(transaction.amount) !== expectedSign) continue;
    const normalized =
      transaction.normalized_description ?? normalizeDescription(transaction.description);
    if (normalized !== recurrence.normalized_description) continue;
    const distance = Math.abs(daysBetween(recurrence.next_date, transaction.date));
    if (Number.isNaN(distance) || distance > slack) continue;
    if (
      distance < bestDistance ||
      (distance === bestDistance && best !== null && transaction.id.localeCompare(best.id) < 0)
    ) {
      best = transaction;
      bestDistance = distance;
    }
  }
  return best;
}

/**
 * Every confirmed bill with where it stands today, soonest first. A bill is
 * paid when its transaction has landed; otherwise it is upcoming until its
 * date, due from its date through the interval's slack (the bank may still
 * be posting it), and late once the slack has passed with nothing. Paused,
 * dismissed, and merely detected recurrences are not bills.
 */
export function resolveBills<T extends BillRecurrence>(
  recurrences: readonly T[],
  transactions: readonly BillTransaction[],
  today: string,
  options: ResolveBillsOptions = {},
): ResolvedBill<T>[] {
  const bills: ResolvedBill<T>[] = [];
  for (const recurrence of recurrences) {
    if (recurrence.status !== "confirmed") continue;
    const daysAway = daysBetween(today, recurrence.next_date);
    const payment = paymentFor(recurrence, transactions);
    const state: BillState =
      payment !== null
        ? "paid"
        : daysAway < -slackDays(recurrence.interval)
          ? "late"
          : daysAway <= 0
            ? "due"
            : "upcoming";
    if (state === "upcoming" && options.withinDays !== undefined && daysAway > options.withinDays) {
      continue;
    }
    bills.push({
      recurrence,
      dueDate: recurrence.next_date,
      daysAway,
      state,
      ...(payment === null ? {} : { paidBy: payment.id }),
    });
  }
  return bills.sort(
    (left, right) =>
      left.dueDate.localeCompare(right.dueDate) ||
      left.recurrence.id.localeCompare(right.recurrence.id),
  );
}

export interface PaymentAdvance {
  readonly next_date: string;
  readonly last_date: string;
  readonly last_paid_transaction_id: string;
  readonly matched_count: number;
}

/**
 * How a recurrence changes once a transaction has paid it: the next date is
 * one interval on from the payment's own date rather than from the expected
 * one, because a bill that posted three days late this month will post three
 * days late next month too.
 */
export function advanceAfterPayment(
  recurrence: { readonly matched_count: number },
  transaction: { readonly id: string; readonly date: string },
  interval: Interval,
): PaymentAdvance {
  return {
    next_date: nextOccurrence(transaction.date, interval),
    last_date: transaction.date,
    last_paid_transaction_id: transaction.id,
    matched_count: recurrence.matched_count + 1,
  };
}

/** What an amount at an interval comes to per month, rounded to the cent. */
export function monthlyEquivalent(amount: number, interval: Interval): number {
  switch (interval) {
    case "weekly":
      return Math.round((amount * 52) / 12);
    case "biweekly":
      return Math.round((amount * 26) / 12);
    case "monthly":
      return amount;
    case "quarterly":
      return Math.round(amount / 3);
    case "yearly":
      return Math.round(amount / 12);
  }
}

/**
 * The confirmed recurrences' monthly total per currency, signed as stored --
 * bills negative, a recurring paycheck positive -- so the screen decides how
 * to show it and nothing is converted.
 */
export function monthlyTotal(
  recurrences: ReadonlyArray<{
    readonly status: string;
    readonly interval: Interval;
    readonly expected_amount: number;
    readonly currency: string;
  }>,
): Array<{ readonly currency: string; readonly total: number }> {
  const totals = new Map<string, number>();
  for (const recurrence of recurrences) {
    if (recurrence.status !== "confirmed") continue;
    totals.set(
      recurrence.currency,
      (totals.get(recurrence.currency) ?? 0) +
        monthlyEquivalent(recurrence.expected_amount, recurrence.interval),
    );
  }
  return [...totals.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([currency, total]) => ({ currency, total }));
}

/**
 * The dates a recurrence falls on in one month, for the calendar: the
 * occurrences on either side of `next_date`, each measured from it in whole
 * intervals so the day of the month holds. Bounded, because a weekly bill
 * looked at from a decade away is still a finite walk.
 */
export function occurrencesInMonth(
  recurrence: { readonly next_date: string; readonly interval: Interval },
  month: string,
): string[] {
  const LIMIT = 2_000;
  const dates: string[] = [];
  const inMonth = (date: string): number => date.slice(0, 7).localeCompare(month);
  // Walk back from the anchor until a date lands before the month, then
  // forward through it: the anchor itself may be in, before, or after it.
  let step = 0;
  while (
    step > -LIMIT &&
    inMonth(shiftOccurrence(recurrence.next_date, recurrence.interval, step)) >= 0
  ) {
    step -= 1;
  }
  for (step += 1; step < LIMIT; step += 1) {
    const date = shiftOccurrence(recurrence.next_date, recurrence.interval, step);
    const position = inMonth(date);
    if (position > 0) break;
    if (position === 0) dates.push(date);
  }
  return dates;
}
