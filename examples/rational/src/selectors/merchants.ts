import { isCounted } from "../../functions/shared/exclusions.js";
import {
  type MerchantLike,
  type ResolvedMerchant,
  resolveMerchant,
} from "../../functions/shared/merchants.js";
import { normalizeDescription } from "../../functions/shared/recurrences.js";
import { isMerchant, type Merchant, type TaxonomyEntry, type Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * Who a transaction was with, as the screens show it.
 *
 * The decision -- the merchant it was filed under, else the one whose
 * patterns include its normalized description, else a cleaned reading of the
 * statement text -- is the shared resolver's, so the nightly job and the list
 * name a transaction the same way. What this file adds is speed: resolving a
 * list of fifty thousand transactions against a few hundred merchants would
 * otherwise scan every merchant's patterns for every row, so the merchants
 * are indexed once and the shared resolver is handed only the one or two that
 * could possibly match. The fallback chain and the cleaning never leave the
 * shared module.
 */
export {
  cleanDescription,
  type MergePlan,
  mergePlan,
  type ResolvedMerchant,
} from "../../functions/shared/merchants.js";

export type MerchantResolver = (transaction: Transaction) => ResolvedMerchant;

/**
 * A resolver over one set of merchants. The entries may be the whole taxonomy;
 * only the merchants in it are read.
 */
export function merchantResolver(
  entries: readonly TaxonomyEntry[],
  normalize: (text: string) => string = normalizeDescription,
): MerchantResolver {
  const byId = new Map<string, Merchant>();
  const byPattern = new Map<string, Merchant>();
  for (const entry of entries) {
    if (!isMerchant(entry)) continue;
    byId.set(entry.id, entry);
    for (const pattern of entry.patterns ?? []) {
      // The first merchant to claim a pattern keeps it, as a scan would find it.
      if (!byPattern.has(pattern)) byPattern.set(pattern, entry);
    }
  }
  return (transaction) => {
    const candidates: MerchantLike[] = [];
    const chosen =
      transaction.merchant_id === undefined ? undefined : byId.get(transaction.merchant_id);
    if (chosen !== undefined) candidates.push(chosen);
    const normalized = transaction.normalized_description ?? normalize(transaction.description);
    const byText = byPattern.get(normalized);
    if (byText !== undefined && byText !== chosen) candidates.push(byText);
    return resolveMerchant(transaction, candidates, normalize);
  };
}

/** The shared resolver over the application's own types, for one transaction. */
export function resolveTransactionMerchant(
  transaction: Transaction,
  merchants: readonly TaxonomyEntry[],
): ResolvedMerchant {
  return resolveMerchant(transaction, merchants.filter(isMerchant), normalizeDescription);
}

/** The name a list shows for a transaction. */
export function merchantDisplay(
  transaction: Transaction,
  merchants: readonly TaxonomyEntry[],
): string {
  return resolveTransactionMerchant(transaction, merchants).name;
}

export interface MerchantRow {
  /** The merchant document, or null when the name is only a cleaned description. */
  readonly id: string | null;
  readonly name: string;
  /** Every transaction that resolves to it, hidden ones included. */
  readonly count: number;
  /** Counted money that went out to it, as a positive figure. */
  readonly total: number;
  /** The most recent transaction's date; empty for a merchant nothing was with yet. */
  readonly lastDate: string;
}

/**
 * The merchants page: every merchant document, whether or not anything was
 * with it yet, and every name a transaction resolves to without one, most
 * used first. The total is spending -- outflows the reports count -- because
 * that is the question the page answers about a merchant; the count is every
 * transaction so a merge shows how many rows it will refile. Given a currency,
 * the total is over that currency alone, since nothing is converted.
 */
export function merchantRows(
  transactions: readonly Transaction[],
  merchants: readonly TaxonomyEntry[],
  currency?: string,
): readonly MerchantRow[] {
  const rows = new Map<
    string,
    { id: string | null; name: string; count: number; total: number; lastDate: string }
  >();
  for (const entry of merchants) {
    if (!isMerchant(entry)) continue;
    rows.set(`id:${entry.id}`, {
      id: entry.id,
      name: entry.name,
      count: 0,
      total: 0,
      lastDate: "",
    });
  }
  const resolve = merchantResolver(merchants);
  for (const transaction of transactions) {
    const merchant = resolve(transaction);
    const key = merchant.id === null ? `name:${merchant.name}` : `id:${merchant.id}`;
    const row = rows.get(key) ?? {
      id: merchant.id,
      name: merchant.name,
      count: 0,
      total: 0,
      lastDate: "",
    };
    row.count += 1;
    if (
      transaction.amount < 0 &&
      isCounted(transaction) &&
      (currency === undefined || transaction.currency === currency)
    ) {
      row.total += -transaction.amount;
    }
    if (transaction.date > row.lastDate) row.lastDate = transaction.date;
    rows.set(key, row);
  }
  return [...rows.values()].sort(
    (left, right) =>
      right.count - left.count ||
      right.total - left.total ||
      left.name.localeCompare(right.name) ||
      (left.id ?? "").localeCompare(right.id ?? ""),
  );
}

export const selectMerchantRows = memoizeLast(merchantRows);
