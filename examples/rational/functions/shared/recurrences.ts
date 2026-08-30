/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * Finding the bills that repeat, and saying when the next one is due.
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
 */

export type Interval = "weekly" | "biweekly" | "monthly" | "quarterly" | "yearly";

/** What detection reads of a transaction. */
export interface RecurrenceSubject {
  readonly account_id: string;
  readonly description: string;
  readonly amount: number;
  readonly currency: string;
  readonly date: string;
}

/** What detection reads of a recurrence the household already has. */
export interface KnownRecurrence {
  readonly account_id: string;
  readonly normalized_description: string;
}

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
 * recurrences -- detected, confirmed, or dismissed -- are excluded, so a
 * dismissal stays dismissed and the nightly job does not propose again what
 * it proposed last night.
 */
export function detectRecurrences(
  transactions: readonly RecurrenceSubject[],
  known: readonly KnownRecurrence[] = [],
): readonly DetectedRecurrence[] {
  const groups = new Map<string, RecurrenceSubject[]>();
  for (const transaction of transactions) {
    if (transaction.amount >= 0) continue;
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
