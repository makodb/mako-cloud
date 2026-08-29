import type { Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";

export const DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/u;

export function isIsoDate(value: string): boolean {
  if (!DATE_PATTERN.test(value)) return false;
  const [year, month, day] = value.split("-").map((part) => Number.parseInt(part, 10));
  if (year === undefined || month === undefined || day === undefined) return false;
  const date = new Date(Date.UTC(year, month - 1, day));
  return (
    date.getUTCFullYear() === year && date.getUTCMonth() === month - 1 && date.getUTCDate() === day
  );
}

/** `2026-08-14` → `2026-08`. */
export function monthKey(date: string): string {
  return date.slice(0, 7);
}

export interface TransactionFilter {
  readonly accountId?: string;
  readonly month?: string;
}

/** Newest first; ties broken by updated_at and then id so every device agrees. */
export function sortTransactions(transactions: readonly Transaction[]): Transaction[] {
  return [...transactions].sort(
    (left, right) =>
      right.date.localeCompare(left.date) ||
      right.updated_at - left.updated_at ||
      right.id.localeCompare(left.id),
  );
}

export function filterTransactions(
  transactions: readonly Transaction[],
  filter: TransactionFilter,
): Transaction[] {
  return sortTransactions(
    transactions.filter(
      (transaction) =>
        (filter.accountId === undefined || transaction.account_id === filter.accountId) &&
        (filter.month === undefined || monthKey(transaction.date) === filter.month),
    ),
  );
}

export const selectFilteredTransactions = memoizeLast(filterTransactions);

/** Every month with at least one transaction, newest first. */
export function availableMonths(transactions: readonly Transaction[]): string[] {
  return [...new Set(transactions.map((transaction) => monthKey(transaction.date)))].sort(
    (left, right) => right.localeCompare(left),
  );
}

export const selectAvailableMonths = memoizeLast(availableMonths);

/** Sum of the amounts in a list, for totals shown next to a filtered list. */
export function sumAmounts(transactions: readonly Transaction[]): number {
  return transactions.reduce((total, transaction) => total + transaction.amount, 0);
}

/** Description normalization shared by rules, dedupe, and recurrence detection. */
export function normalizeDescription(description: string): string {
  return description
    .toLowerCase()
    .replaceAll(/[0-9#*]+/gu, " ")
    .replaceAll(/[^a-z ]+/gu, " ")
    .replaceAll(/\s+/gu, " ")
    .trim();
}
