/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * Guessing which two transactions are one transfer.
 *
 * Money moved between two of the household's own accounts shows up twice: an
 * outflow on one and an inflow on the other, for the same amount, a few days
 * apart while the banks settle. Pairing them under one `transfer_id` is what
 * keeps a savings deposit out of "spending" and a credit-card payment out of
 * "income". A suggestion is only ever a suggestion -- an editor confirms it
 * -- because two identical amounts three days apart are sometimes a
 * coincidence, and a pairing made by a guess would hide real money.
 */

/** What pairing reads of a transaction. */
export interface TransferCandidate {
  readonly id: string;
  readonly account_id: string;
  readonly amount: number;
  readonly currency: string;
  readonly date: string;
  readonly transfer_id?: string;
  readonly hidden?: boolean;
  readonly adjustment?: boolean;
}

export interface TransferSuggestion {
  readonly outflowId: string;
  readonly inflowId: string;
  /** The amount moved, as a magnitude. */
  readonly amount: number;
  /** Days between the two legs, as a magnitude. */
  readonly days: number;
}

export interface TransferOptions {
  /** How far apart the two legs may land; three days is a bank's settlement. */
  readonly maxDays?: number;
}

function daysApart(left: string, right: string): number {
  const start = Date.parse(`${left}T00:00:00Z`);
  const end = Date.parse(`${right}T00:00:00Z`);
  if (Number.isNaN(start) || Number.isNaN(end)) return Number.NaN;
  return Math.abs(Math.round((end - start) / 86_400_000));
}

/** Unpaired, and the kind of transaction that could be a leg at all. */
function pairable(transaction: TransferCandidate): boolean {
  return (
    (transaction.transfer_id === undefined || transaction.transfer_id === "") &&
    transaction.hidden !== true &&
    transaction.adjustment !== true &&
    transaction.amount !== 0
  );
}

/**
 * The pairs worth proposing: an outflow and an inflow on different accounts,
 * in one currency, for exactly opposite amounts, within `maxDays` of each
 * other. Every candidate pair is ranked closest-in-time first and taken
 * greedily, so a transaction is in at most one suggestion and the tie between
 * two equally close partners is settled by id, the same on every device.
 */
export function suggestTransferPairs(
  transactions: readonly TransferCandidate[],
  options: TransferOptions = {},
): TransferSuggestion[] {
  const maxDays = options.maxDays ?? 3;
  const outflows = transactions.filter((entry) => pairable(entry) && entry.amount < 0);
  const inflows = transactions.filter((entry) => pairable(entry) && entry.amount > 0);
  // Inflows by (currency, amount) so each outflow looks only at what could
  // match it, rather than at every inflow of the household.
  const byAmount = new Map<string, TransferCandidate[]>();
  for (const inflow of inflows) {
    const key = `${inflow.currency}${inflow.amount}`;
    const bucket = byAmount.get(key);
    if (bucket === undefined) byAmount.set(key, [inflow]);
    else bucket.push(inflow);
  }
  const candidates: TransferSuggestion[] = [];
  for (const outflow of outflows) {
    const partners = byAmount.get(`${outflow.currency}${-outflow.amount}`) ?? [];
    for (const inflow of partners) {
      if (inflow.account_id === outflow.account_id) continue;
      const days = daysApart(outflow.date, inflow.date);
      if (Number.isNaN(days) || days > maxDays) continue;
      candidates.push({
        outflowId: outflow.id,
        inflowId: inflow.id,
        amount: -outflow.amount,
        days,
      });
    }
  }
  candidates.sort(
    (left, right) =>
      left.days - right.days ||
      left.outflowId.localeCompare(right.outflowId) ||
      left.inflowId.localeCompare(right.inflowId),
  );
  const taken = new Set<string>();
  const suggestions: TransferSuggestion[] = [];
  for (const candidate of candidates) {
    if (taken.has(candidate.outflowId) || taken.has(candidate.inflowId)) continue;
    taken.add(candidate.outflowId);
    taken.add(candidate.inflowId);
    suggestions.push(candidate);
  }
  return suggestions;
}
