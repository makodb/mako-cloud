/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * The one definition of what counts as money moving.
 *
 * Three kinds of transaction sit in an account's history without being income
 * or spending: a leg of a transfer (the money went to another of the
 * household's own accounts), a balance update of a tracked asset (the house
 * was revalued, nothing was bought), and anything a member hid. Budgets,
 * reports, cash flow, recurrence detection, and alerts all ask this module
 * rather than each keeping a list, so the dashboard, the budget page, and the
 * nightly job agree to the cent about what was spent. Balances are the one
 * place these still count: an account's balance is what the bank says, and
 * the bank counts everything.
 *
 * The modules the browser also imports cannot import this one (see
 * `rules.ts`), so `budgets.ts`, `recurrences.ts`, and `alerts.ts` repeat the
 * three-field test inline; this is the definition they copy.
 */

/** What the exclusion test reads of a transaction. */
export interface ExclusionSubject {
  readonly hidden?: boolean;
  readonly adjustment?: boolean;
  readonly transfer_id?: string;
}

/** One of the two legs of a transfer between the household's own accounts. */
export function isTransferLeg(transaction: ExclusionSubject): boolean {
  return transaction.transfer_id !== undefined && transaction.transfer_id !== "";
}

/** A balance update of a tracked account: a revaluation, not a purchase. */
export function isAdjustment(transaction: ExclusionSubject): boolean {
  return transaction.adjustment === true;
}

/** Hidden by a member from budgets, reports, and cash flow. */
export function isHidden(transaction: ExclusionSubject): boolean {
  return transaction.hidden === true;
}

/** Money that moved in or out of the household, as far as any total is concerned. */
export function isCounted(transaction: ExclusionSubject): boolean {
  return !isHidden(transaction) && !isAdjustment(transaction) && !isTransferLeg(transaction);
}

/** The counted transactions of a list, in their order. */
export function counted<T extends ExclusionSubject>(transactions: readonly T[]): T[] {
  return transactions.filter(isCounted);
}
