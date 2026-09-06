import type { Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";
import { sortTransactions } from "./transactions.js";

/**
 * What arrived without anyone looking at it.
 *
 * A transaction a member typed was reviewed by the typing; sync and import
 * leave `reviewed` unset, and those are the ones worth a glance -- a wrong
 * category, a merchant nobody named, a charge nobody recognizes. The queue is
 * therefore not "everything unreviewed" but everything unreviewed that came
 * from outside: a transaction with neither an external id nor an import batch
 * was made by hand, and asking a member to review their own typing would be
 * noise the flag would soon be turned off to silence.
 */
export function needsReview(transaction: Transaction): boolean {
  if (transaction.reviewed === true) return false;
  return (
    (transaction.external_id !== undefined && transaction.external_id !== "") ||
    (transaction.import_batch_id !== undefined && transaction.import_batch_id !== "")
  );
}

/** Newest first, so what arrived last night is at the top. */
export function reviewQueue(transactions: readonly Transaction[]): Transaction[] {
  return sortTransactions(transactions.filter(needsReview));
}

export function reviewCount(transactions: readonly Transaction[]): number {
  let count = 0;
  for (const transaction of transactions) if (needsReview(transaction)) count += 1;
  return count;
}

export const selectReviewQueue = memoizeLast(reviewQueue);
export const selectReviewCount = memoizeLast(reviewCount);
