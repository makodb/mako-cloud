import { isTransferLeg } from "../../functions/shared/exclusions.js";
import { normalizeDescription } from "../../functions/shared/recurrences.js";
import { isCategory, type TaxonomyEntry, type Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";
import { merchantResolver } from "./merchants.js";
import { amountToText } from "./money.js";
import { needsReview } from "./review.js";
import { isIsoDate, monthKey, sortTransactions } from "./transactions.js";

/**
 * The transactions page's query: every filter it offers, as one value that
 * lives in the URL.
 *
 * Putting the whole query in the hash is what makes a filtered list a thing
 * that can be linked to, shared with the other member, and returned to with
 * the back button. The keys are short and never renamed, because a link
 * someone kept last year should still open the same list; a value the codec
 * does not understand is dropped rather than refused, so a stale link opens a
 * wider list rather than an error.
 */

export type ReviewFilter = "needs" | "reviewed";
export type HiddenFilter = "include" | "only";
export type TransferFilter = "include" | "only" | "exclude";
export type TransactionSort = "date_desc" | "date_asc" | "amount_desc" | "amount_asc";

export interface TransactionQuery {
  /** Free text over description, merchant, notes, and the amount as written. */
  readonly text?: string;
  readonly accountId?: string;
  /** A category id; the empty string means "uncategorized". */
  readonly categoryId?: string;
  readonly groupId?: string;
  readonly tagId?: string;
  readonly merchantId?: string;
  /** Inclusive ISO dates. */
  readonly from?: string;
  readonly to?: string;
  /** Kept so links written before ranges existed still open. */
  readonly month?: string;
  /** Minor units, compared against the magnitude of the amount. */
  readonly amountMin?: number;
  readonly amountMax?: number;
  readonly review?: ReviewFilter;
  /** Hidden transactions are left out unless asked for. */
  readonly hidden?: HiddenFilter;
  /** Transfer legs are shown unless asked otherwise. */
  readonly transfers?: TransferFilter;
  readonly sort?: TransactionSort;
}

const REVIEW_FILTERS = ["needs", "reviewed"] as const satisfies readonly ReviewFilter[];
const HIDDEN_FILTERS = ["include", "only"] as const satisfies readonly HiddenFilter[];
const TRANSFER_FILTERS = [
  "include",
  "only",
  "exclude",
] as const satisfies readonly TransferFilter[];
export const TRANSACTION_SORTS = [
  "date_desc",
  "date_asc",
  "amount_desc",
  "amount_asc",
] as const satisfies readonly TransactionSort[];

const MONTH_PATTERN = /^\d{4}-(0[1-9]|1[0-2])$/u;

function oneOf<T extends string>(values: readonly T[], value: string | null): T | undefined {
  return values.find((candidate) => candidate === value);
}

function nonEmpty(value: string | null): string | undefined {
  return value === null || value === "" ? undefined : value;
}

function isoDate(value: string | null): string | undefined {
  return value !== null && isIsoDate(value) ? value : undefined;
}

function minorUnits(value: string | null): number | undefined {
  if (value === null || !/^-?\d+$/u.test(value)) return undefined;
  const parsed = Number.parseInt(value, 10);
  return Number.isSafeInteger(parsed) ? Math.abs(parsed) : undefined;
}

export function parseTransactionQuery(params: URLSearchParams): TransactionQuery {
  const text = params.get("q")?.trim();
  const account = nonEmpty(params.get("account"));
  const category = params.get("category");
  const group = nonEmpty(params.get("group"));
  const tag = nonEmpty(params.get("tag"));
  const merchant = nonEmpty(params.get("merchant"));
  const from = isoDate(params.get("from"));
  const to = isoDate(params.get("to"));
  const month = params.get("month");
  const min = minorUnits(params.get("min"));
  const max = minorUnits(params.get("max"));
  const review = oneOf(REVIEW_FILTERS, params.get("review"));
  const hidden = oneOf(HIDDEN_FILTERS, params.get("hidden"));
  const transfers = oneOf(TRANSFER_FILTERS, params.get("transfers"));
  const sort = oneOf(TRANSACTION_SORTS, params.get("sort"));
  return {
    ...(text === undefined || text === "" ? {} : { text }),
    ...(account === undefined ? {} : { accountId: account }),
    ...(category === null ? {} : { categoryId: category }),
    ...(group === undefined ? {} : { groupId: group }),
    ...(tag === undefined ? {} : { tagId: tag }),
    ...(merchant === undefined ? {} : { merchantId: merchant }),
    ...(from === undefined ? {} : { from }),
    ...(to === undefined ? {} : { to }),
    ...(month === null || !MONTH_PATTERN.test(month) ? {} : { month }),
    ...(min === undefined ? {} : { amountMin: min }),
    ...(max === undefined ? {} : { amountMax: max }),
    ...(review === undefined ? {} : { review }),
    ...(hidden === undefined ? {} : { hidden }),
    ...(transfers === undefined ? {} : { transfers }),
    ...(sort === undefined ? {} : { sort }),
  };
}

/** The inverse of `parseTransactionQuery`, in a fixed key order so equal queries make equal links. */
export function serializeTransactionQuery(query: TransactionQuery): URLSearchParams {
  const params = new URLSearchParams();
  if (query.text !== undefined && query.text.trim() !== "") params.set("q", query.text.trim());
  if (query.accountId !== undefined) params.set("account", query.accountId);
  if (query.categoryId !== undefined) params.set("category", query.categoryId);
  if (query.groupId !== undefined) params.set("group", query.groupId);
  if (query.tagId !== undefined) params.set("tag", query.tagId);
  if (query.merchantId !== undefined) params.set("merchant", query.merchantId);
  if (query.from !== undefined) params.set("from", query.from);
  if (query.to !== undefined) params.set("to", query.to);
  if (query.month !== undefined) params.set("month", query.month);
  if (query.amountMin !== undefined) params.set("min", String(query.amountMin));
  if (query.amountMax !== undefined) params.set("max", String(query.amountMax));
  if (query.review !== undefined) params.set("review", query.review);
  if (query.hidden !== undefined) params.set("hidden", query.hidden);
  if (query.transfers !== undefined) params.set("transfers", query.transfers);
  if (query.sort !== undefined) params.set("sort", query.sort);
  return params;
}

/** How many filters narrow the list; the sort is an order, not a filter. */
export function activeFilterCount(query: TransactionQuery): number {
  return Object.entries(query).filter(([key, value]) => key !== "sort" && value !== undefined)
    .length;
}

export interface TransactionQueryContext {
  /** The taxonomy, or just its categories; only categories are read. */
  readonly categories: readonly TaxonomyEntry[];
  /** The taxonomy, or just its merchants; only merchants are read. */
  readonly merchants: readonly TaxonomyEntry[];
  readonly normalize?: (text: string) => string;
}

/** The categories a transaction's money is filed under: its own, or its splits'. */
function categoryIdsOf(transaction: Transaction): string[] {
  if (transaction.splits.length === 0) return [transaction.category_id ?? ""];
  return transaction.splits.map((split) => split.category_id ?? "");
}

function queryPredicate(
  query: TransactionQuery,
  context: TransactionQueryContext,
): (transaction: Transaction) => boolean {
  const normalize = context.normalize ?? normalizeDescription;
  const groupCategories =
    query.groupId === undefined
      ? null
      : new Set(
          context.categories
            .filter((entry) => isCategory(entry) && entry.parent_id === query.groupId)
            .map((entry) => entry.id),
        );
  const needle = query.text?.trim().toLowerCase() ?? "";
  const resolve =
    query.merchantId !== undefined || needle !== ""
      ? merchantResolver(context.merchants, normalize)
      : null;
  return (transaction) => {
    if (query.hidden === undefined && transaction.hidden === true) return false;
    if (query.hidden === "only" && transaction.hidden !== true) return false;
    if (query.transfers === "only" && !isTransferLeg(transaction)) return false;
    if (query.transfers === "exclude" && isTransferLeg(transaction)) return false;
    if (query.accountId !== undefined && transaction.account_id !== query.accountId) return false;
    if (query.from !== undefined && transaction.date < query.from) return false;
    if (query.to !== undefined && transaction.date > query.to) return false;
    if (query.month !== undefined && monthKey(transaction.date) !== query.month) return false;
    const magnitude = Math.abs(transaction.amount);
    if (query.amountMin !== undefined && magnitude < query.amountMin) return false;
    if (query.amountMax !== undefined && magnitude > query.amountMax) return false;
    if (query.review === "needs" && !needsReview(transaction)) return false;
    if (query.review === "reviewed" && transaction.reviewed !== true) return false;
    if (query.tagId !== undefined && !transaction.tags.includes(query.tagId)) return false;
    if (query.categoryId !== undefined || groupCategories !== null) {
      const filed = categoryIdsOf(transaction);
      if (query.categoryId !== undefined && !filed.includes(query.categoryId)) return false;
      if (groupCategories !== null && !filed.some((id) => groupCategories.has(id))) return false;
    }
    if (resolve !== null) {
      const merchant = resolve(transaction);
      if (query.merchantId !== undefined && merchant.id !== query.merchantId) return false;
      if (needle !== "") {
        const haystack = [
          transaction.description,
          merchant.name,
          transaction.notes ?? "",
          amountToText(transaction.amount, transaction.currency),
        ]
          .join("\n")
          .toLowerCase();
        if (!haystack.includes(needle)) return false;
      }
    }
    return true;
  };
}

/**
 * Amounts sort by magnitude: a list of spending sorted "largest first" should
 * lead with the biggest purchase, and signed order would put the smallest
 * payment at the top.
 */
function sortBy(sort: TransactionSort, transactions: readonly Transaction[]): Transaction[] {
  switch (sort) {
    case "date_desc":
      return sortTransactions(transactions);
    case "date_asc":
      return [...transactions].sort(
        (left, right) =>
          left.date.localeCompare(right.date) ||
          left.updated_at - right.updated_at ||
          left.id.localeCompare(right.id),
      );
    case "amount_desc":
      return [...transactions].sort(
        (left, right) =>
          Math.abs(right.amount) - Math.abs(left.amount) ||
          right.date.localeCompare(left.date) ||
          right.id.localeCompare(left.id),
      );
    case "amount_asc":
      return [...transactions].sort(
        (left, right) =>
          Math.abs(left.amount) - Math.abs(right.amount) ||
          right.date.localeCompare(left.date) ||
          right.id.localeCompare(left.id),
      );
  }
}

/** The list the query describes, filtered and in its order. */
export function applyTransactionQuery(
  transactions: readonly Transaction[],
  query: TransactionQuery,
  context: TransactionQueryContext,
): Transaction[] {
  return sortBy(query.sort ?? "date_desc", transactions.filter(queryPredicate(query, context)));
}

export const selectQueriedTransactions = memoizeLast(applyTransactionQuery);

export interface DayGroup {
  readonly date: string;
  readonly transactions: readonly Transaction[];
  /** The day's signed sum, over the transactions shown. */
  readonly total: number;
}

/** The list by day, newest day first; within a day the given order holds. */
export function groupByDate(transactions: readonly Transaction[]): readonly DayGroup[] {
  const days = new Map<string, { transactions: Transaction[]; total: number }>();
  for (const transaction of transactions) {
    const day = days.get(transaction.date) ?? { transactions: [], total: 0 };
    day.transactions.push(transaction);
    day.total += transaction.amount;
    days.set(transaction.date, day);
  }
  return [...days.entries()]
    .sort(([left], [right]) => right.localeCompare(left))
    .map(([date, day]) => ({ date, transactions: day.transactions, total: day.total }));
}

export const selectGroupedByDate = memoizeLast(groupByDate);
