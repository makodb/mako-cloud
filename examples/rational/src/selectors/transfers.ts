import { suggestTransferPairs, type TransferSuggestion } from "../../functions/shared/transfers.js";
import type { Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * Transfer pairing as the transactions screen offers it.
 *
 * The guess -- which outflow and inflow are one movement of money -- is the
 * shared engine's, so the browser proposes exactly what the nightly job would.
 * What this file adds is the two ways a person reaches a pairing from one
 * transaction: the suggestion the engine made for it, if any, and the wider
 * list of opposite transactions they may pair it with by hand when the engine
 * was too strict (a payment that took a week to clear, a fee that made the
 * amounts differ by a dollar).
 */
export type { TransferSuggestion } from "../../functions/shared/transfers.js";

/** Every pair worth proposing across the household, computed once per list. */
export const selectTransferSuggestions = memoizeLast((transactions: readonly Transaction[]) =>
  suggestTransferPairs(transactions),
);

/** The suggestions naming one transaction; the engine is greedy, so at most one. */
export function suggestionsInvolving(
  suggestions: readonly TransferSuggestion[],
  transactionId: string,
): readonly TransferSuggestion[] {
  return suggestions.filter(
    (suggestion) => suggestion.outflowId === transactionId || suggestion.inflowId === transactionId,
  );
}

/** The other leg of a paired transfer, or null when the pair is incomplete on this device. */
export function otherLeg(
  transactions: readonly Transaction[],
  transaction: Transaction,
): Transaction | null {
  const transferId = transaction.transfer_id;
  if (transferId === undefined || transferId === "") return null;
  return (
    transactions.find(
      (candidate) => candidate.transfer_id === transferId && candidate.id !== transaction.id,
    ) ?? null
  );
}

function daysApart(left: string, right: string): number {
  const start = Date.parse(`${left}T00:00:00Z`);
  const end = Date.parse(`${right}T00:00:00Z`);
  if (Number.isNaN(start) || Number.isNaN(end)) return Number.POSITIVE_INFINITY;
  return Math.abs(Math.round((end - start) / 86_400_000));
}

export interface PairingOptions {
  /** How far apart the legs may be for a hand pairing; a month covers a slow cheque. */
  readonly maxDays?: number;
  readonly limit?: number;
}

/**
 * What a person may pair a transaction with by hand: unpaired transactions of
 * the opposite sign on another account in the same currency, the closest in
 * amount first and then the closest in time, so the right one is near the top
 * even when the engine's three-day, same-amount rule missed it.
 */
export function pairingCandidates(
  transactions: readonly Transaction[],
  transaction: Transaction,
  options: PairingOptions = {},
): Transaction[] {
  const maxDays = options.maxDays ?? 31;
  const limit = options.limit ?? 25;
  if (transaction.amount === 0) return [];
  const wantOutflow = transaction.amount > 0;
  return transactions
    .filter(
      (candidate) =>
        candidate.id !== transaction.id &&
        candidate.account_id !== transaction.account_id &&
        candidate.currency === transaction.currency &&
        (candidate.transfer_id === undefined || candidate.transfer_id === "") &&
        candidate.hidden !== true &&
        candidate.adjustment !== true &&
        (wantOutflow ? candidate.amount < 0 : candidate.amount > 0) &&
        daysApart(candidate.date, transaction.date) <= maxDays,
    )
    .sort(
      (left, right) =>
        Math.abs(left.amount + transaction.amount) - Math.abs(right.amount + transaction.amount) ||
        daysApart(left.date, transaction.date) - daysApart(right.date, transaction.date) ||
        left.id.localeCompare(right.id),
    )
    .slice(0, limit);
}
