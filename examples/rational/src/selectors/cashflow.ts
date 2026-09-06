import { isCounted } from "../../functions/shared/exclusions.js";
import { addMonths } from "../../functions/shared/goals.js";
import {
  type Account,
  type CategoryKind,
  isCategory,
  isGroup,
  isTag,
  type TaxonomyEntry,
  type Transaction,
} from "../model/types.js";
import { memoizeLast } from "./memo.js";
import { merchantResolver } from "./merchants.js";
import { isIsoDate, monthKey } from "./transactions.js";

/**
 * Cash flow: what came in, what went out, and where it went, over a range.
 *
 * Every figure here counts the same transactions the budget page counts --
 * the shared exclusion test leaves out transfer legs, balance updates, and
 * hidden transactions -- and files them by the same rule the budget engine
 * uses: an amount under an income category is income whatever its sign, an
 * amount under an expense category is spending whatever its sign (a refund
 * reduces spending rather than posing as income), and an amount under no
 * category, or a category of another kind, counts by its sign. That is what
 * lets the dashboard, the cash-flow page, and the budget page show one
 * number for "spent this month".
 *
 * Totals are minor units of one currency; a household with accounts in two
 * currencies gets two rows, never a conversion.
 */

/** Inclusive ISO dates. */
export interface DateRange {
  readonly start: string;
  readonly end: string;
}

/**
 * A range as it appears in a link: `2026-08`, `2026-Q3`, `2026`, `all`, or
 * `custom:2026-08-01:2026-09-06`.
 */
export type RangeKey = string;

type ParsedRange =
  | { readonly kind: "month"; readonly month: string }
  | { readonly kind: "quarter"; readonly year: number; readonly quarter: number }
  | { readonly kind: "year"; readonly year: number }
  | { readonly kind: "all" }
  | { readonly kind: "custom"; readonly start: string; readonly end: string };

const MONTH_NAMES = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

function parseRangeKey(key: string): ParsedRange | null {
  if (key === "all") return { kind: "all" };
  const month = /^(\d{4})-(0[1-9]|1[0-2])$/u.exec(key);
  if (month !== null) return { kind: "month", month: key };
  const quarter = /^(\d{4})-Q([1-4])$/u.exec(key);
  if (quarter !== null) {
    return { kind: "quarter", year: Number(quarter[1]), quarter: Number(quarter[2]) };
  }
  const year = /^(\d{4})$/u.exec(key);
  if (year !== null) return { kind: "year", year: Number(year[1]) };
  const custom = /^custom:(\d{4}-\d{2}-\d{2}):(\d{4}-\d{2}-\d{2})$/u.exec(key);
  if (custom !== null) {
    const [, start = "", end = ""] = custom;
    if (isIsoDate(start) && isIsoDate(end)) {
      return start <= end
        ? { kind: "custom", start, end }
        : { kind: "custom", start: end, end: start };
    }
  }
  return null;
}

/** `2026-08` → `2026-08-31`. */
export function lastDayOfMonth(month: string): string {
  const [year = 0, index = 1] = month.split("-").map(Number);
  const day = new Date(Date.UTC(year, index, 0)).getUTCDate();
  return `${month}-${String(day).padStart(2, "0")}`;
}

export function monthRange(month: string): DateRange {
  return { start: `${month}-01`, end: lastDayOfMonth(month) };
}

/** `2026-08` plus 3 → `2026-11`; the goal engine's, so month arithmetic has one home. */
export { addMonths } from "../../functions/shared/goals.js";

/** Every month from the first to the last, inclusive. */
export function monthsBetween(first: string, last: string): string[] {
  const months: string[] = [];
  for (let month = first; month <= last && months.length < 1_200; month = addMonths(month, 1)) {
    months.push(month);
  }
  return months;
}

export function monthLabel(month: string): string {
  const [year = "", index = "1"] = month.split("-");
  return `${MONTH_NAMES[Number(index) - 1] ?? index} ${year}`;
}

function shortDate(date: string): string {
  const [year = "", month = "1", day = "1"] = date.split("-");
  return `${(MONTH_NAMES[Number(month) - 1] ?? month).slice(0, 3)} ${Number(day)}, ${year}`;
}

export function quarterKey(date: string): RangeKey {
  const [year = "", month = "1"] = date.split("-");
  return `${year}-Q${Math.floor((Number(month) - 1) / 3) + 1}`;
}

export function yearKey(date: string): RangeKey {
  return date.slice(0, 4);
}

export function customRangeKey(range: DateRange): RangeKey {
  return `custom:${range.start}:${range.end}`;
}

/**
 * The dates a key covers; null for `all`, which is unbounded. A key nobody
 * could have written falls back to the month of today, so a broken link shows
 * this month rather than nothing.
 */
export function rangeFor(key: RangeKey, today: string): DateRange | null {
  const parsed = parseRangeKey(key) ?? { kind: "month", month: monthKey(today) };
  switch (parsed.kind) {
    case "all":
      return null;
    case "month":
      return monthRange(parsed.month);
    case "quarter": {
      const first = `${parsed.year}-${String((parsed.quarter - 1) * 3 + 1).padStart(2, "0")}`;
      return { start: `${first}-01`, end: lastDayOfMonth(addMonths(first, 2)) };
    }
    case "year":
      return { start: `${parsed.year}-01-01`, end: `${parsed.year}-12-31` };
    case "custom":
      return { start: parsed.start, end: parsed.end };
  }
}

export function rangeLabel(key: RangeKey): string {
  const parsed = parseRangeKey(key);
  if (parsed === null) return key;
  switch (parsed.kind) {
    case "all":
      return "All time";
    case "month":
      return monthLabel(parsed.month);
    case "quarter":
      return `Q${parsed.quarter} ${parsed.year}`;
    case "year":
      return String(parsed.year);
    case "custom":
      return `${shortDate(parsed.start)} – ${shortDate(parsed.end)}`;
  }
}

/** The previous or next period of the same length; `all` and a custom range have no neighbour. */
export function shiftRange(key: RangeKey, direction: 1 | -1): RangeKey {
  const parsed = parseRangeKey(key);
  if (parsed === null) return key;
  switch (parsed.kind) {
    case "month":
      return addMonths(parsed.month, direction);
    case "quarter": {
      const index = parsed.year * 4 + (parsed.quarter - 1) + direction;
      return `${Math.floor(index / 4)}-Q${(index % 4) + 1}`;
    }
    case "year":
      return String(parsed.year + direction);
    case "all":
    case "custom":
      return key;
  }
}

export function inRange(date: string, range: DateRange | null): boolean {
  return range === null || (date >= range.start && date <= range.end);
}

export interface RangePreset {
  readonly key: RangeKey;
  readonly label: string;
}

/** The ranges a picker offers, relative to today. */
export function rangePresets(today: string): readonly RangePreset[] {
  const month = monthKey(today);
  const lastMonth = addMonths(month, -1);
  const year = yearKey(today);
  return [
    { key: month, label: "This month" },
    { key: lastMonth, label: "Last month" },
    { key: quarterKey(today), label: "This quarter" },
    { key: year, label: "This year" },
    { key: String(Number(year) - 1), label: "Last year" },
    { key: "all", label: "All time" },
  ];
}

interface Effect {
  readonly categoryId: string;
  readonly amount: number;
}

/**
 * A transaction's effect, split by split: one without splits counts once
 * against its own category, one with splits counts each against its own.
 */
function* effects(transaction: Transaction): Generator<Effect> {
  if (transaction.splits.length === 0) {
    yield { categoryId: transaction.category_id ?? "", amount: transaction.amount };
    return;
  }
  for (const split of transaction.splits) {
    yield { categoryId: split.category_id ?? "", amount: split.amount };
  }
}

type CategoryKinds = ReadonlyMap<string, CategoryKind | undefined>;

function categoryKinds(entries: readonly TaxonomyEntry[]): CategoryKinds {
  const kinds = new Map<string, CategoryKind | undefined>();
  for (const entry of entries) if (isCategory(entry)) kinds.set(entry.id, entry.category_kind);
  return kinds;
}

type Side = "income" | "spending";

/** Which side of the cash flow an effect lands on, and how much, signed for that side. */
function classify(effect: Effect, kinds: CategoryKinds): { side: Side; amount: number } {
  const kind = kinds.get(effect.categoryId);
  if (kind === "income") return { side: "income", amount: effect.amount };
  if (kind === "expense") return { side: "spending", amount: -effect.amount };
  return effect.amount >= 0
    ? { side: "income", amount: effect.amount }
    : { side: "spending", amount: -effect.amount };
}

function* countedEffects(
  transactions: readonly Transaction[],
  range: DateRange | null,
  kinds: CategoryKinds,
  currency?: string,
): Generator<{ transaction: Transaction; effect: Effect; side: Side; amount: number }> {
  for (const transaction of transactions) {
    if (!isCounted(transaction) || !inRange(transaction.date, range)) continue;
    if (currency !== undefined && transaction.currency !== currency) continue;
    for (const effect of effects(transaction)) {
      const { side, amount } = classify(effect, kinds);
      yield { transaction, effect, side, amount };
    }
  }
}

export interface CashFlowSummary {
  readonly currency: string;
  readonly income: number;
  readonly spending: number;
  /** Income less spending; negative when the range spent more than it earned. */
  readonly savings: number;
  /** Of income, between 0 and 1; 0 when there was no income. */
  readonly savingsRate: number;
}

function summarize(currency: string, income: number, spending: number): CashFlowSummary {
  const savings = income - spending;
  const savingsRate = income <= 0 ? 0 : Math.min(1, Math.max(0, savings / income));
  return { currency, income, spending, savings, savingsRate };
}

export function cashFlowSummary(
  transactions: readonly Transaction[],
  categories: readonly TaxonomyEntry[],
  range: DateRange | null,
): readonly CashFlowSummary[] {
  const kinds = categoryKinds(categories);
  const totals = new Map<string, { income: number; spending: number }>();
  for (const { transaction, side, amount } of countedEffects(transactions, range, kinds)) {
    const total = totals.get(transaction.currency) ?? { income: 0, spending: 0 };
    total[side] += amount;
    totals.set(transaction.currency, total);
  }
  return [...totals.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([currency, total]) => summarize(currency, total.income, total.spending));
}

export const selectCashFlowSummary = memoizeLast(cashFlowSummary);

/** The row for one currency, or zeros when the range had nothing in it. */
export function summaryFor(
  summaries: readonly CashFlowSummary[],
  currency: string,
): CashFlowSummary {
  return summaries.find((summary) => summary.currency === currency) ?? summarize(currency, 0, 0);
}

export interface MonthFlow {
  readonly month: string;
  readonly currency: string;
  readonly income: number;
  readonly spending: number;
  readonly net: number;
}

/**
 * Month by month over the range, oldest first, with a row for every month
 * even when nothing happened in it -- a bar chart with a gap where a quiet
 * month should be misreads as a chart with a bug. An unbounded range has
 * only the months that occur. Given a currency, only that currency's rows;
 * otherwise a row per month per currency seen.
 */
export function monthlySeries(
  transactions: readonly Transaction[],
  categories: readonly TaxonomyEntry[],
  range: DateRange | null,
  currency?: string,
): readonly MonthFlow[] {
  const kinds = categoryKinds(categories);
  const totals = new Map<string, { income: number; spending: number }>();
  const currencies = new Set<string>(currency === undefined ? [] : [currency]);
  const seen = new Set<string>();
  for (const { transaction, side, amount } of countedEffects(
    transactions,
    range,
    kinds,
    currency,
  )) {
    const month = monthKey(transaction.date);
    const key = `${month}\u0000${transaction.currency}`;
    const total = totals.get(key) ?? { income: 0, spending: 0 };
    total[side] += amount;
    totals.set(key, total);
    currencies.add(transaction.currency);
    seen.add(month);
  }
  const months =
    range === null ? [...seen].sort() : monthsBetween(monthKey(range.start), monthKey(range.end));
  const rows: MonthFlow[] = [];
  for (const month of months) {
    for (const code of [...currencies].sort()) {
      const total = totals.get(`${month}\u0000${code}`) ?? { income: 0, spending: 0 };
      rows.push({
        month,
        currency: code,
        income: total.income,
        spending: total.spending,
        net: total.income - total.spending,
      });
    }
  }
  return rows;
}

export const selectMonthlySeries = memoizeLast(monthlySeries);

export type SpendingKey = "category" | "group" | "merchant" | "tag" | "account";

export interface SpendingContext {
  /** The taxonomy, or the relevant entries; each is read by its kind. */
  readonly categories: readonly TaxonomyEntry[];
  readonly groups: readonly TaxonomyEntry[];
  readonly merchants: readonly TaxonomyEntry[];
  readonly tags?: readonly TaxonomyEntry[];
  readonly accounts?: readonly Account[];
  readonly normalize?: (text: string) => string;
}

export interface SpendingSlice {
  /**
   * The id of what was spent on: a category, group, tag, or account id, a
   * merchant id, or -- for a merchant that is only a cleaned description --
   * its display name. The empty string is "Uncategorized".
   */
  readonly key: string;
  readonly name: string;
  readonly currency: string;
  /** Net spending: refunds reduce it, and a slice can go negative. */
  readonly amount: number;
}

export const UNCATEGORIZED_NAME = "Uncategorized";

function nameIndex(entries: readonly TaxonomyEntry[]): ReadonlyMap<string, string> {
  return new Map(entries.map((entry) => [entry.id, entry.name]));
}

/**
 * Where the money went, largest first, keyed by what the screen can link to.
 * A transaction with several tags counts once per tag, so tag slices do not
 * add up to the total -- they are labels, not a partition. Only the spending
 * side counts: income under an income category is not "spent on Paychecks".
 */
export function spendingBy(
  transactions: readonly Transaction[],
  range: DateRange | null,
  key: SpendingKey,
  context: SpendingContext,
): readonly SpendingSlice[] {
  const kinds = categoryKinds(context.categories);
  const categoryNames = nameIndex(context.categories.filter(isCategory));
  const groupNames = nameIndex(context.groups.filter(isGroup));
  const parents = new Map<string, string>();
  for (const entry of context.categories) {
    if (isCategory(entry) && entry.parent_id !== undefined && groupNames.has(entry.parent_id)) {
      parents.set(entry.id, entry.parent_id);
    }
  }
  const tagNames = nameIndex((context.tags ?? []).filter(isTag));
  const accountNames = new Map(
    (context.accounts ?? []).map((account) => [account.id, account.name]),
  );
  const resolve =
    key === "merchant" ? merchantResolver(context.merchants, context.normalize) : null;

  const totals = new Map<string, { key: string; name: string; currency: string; amount: number }>();
  const add = (slice: string, name: string, currency: string, amount: number): void => {
    const id = `${slice}\u0000${currency}`;
    const total = totals.get(id) ?? { key: slice, name, currency, amount: 0 };
    total.amount += amount;
    totals.set(id, total);
  };
  for (const { transaction, effect, side, amount } of countedEffects(transactions, range, kinds)) {
    if (side !== "spending") continue;
    const currency = transaction.currency;
    switch (key) {
      case "category":
        add(
          effect.categoryId,
          categoryNames.get(effect.categoryId) ?? UNCATEGORIZED_NAME,
          currency,
          amount,
        );
        break;
      case "group": {
        const group = parents.get(effect.categoryId) ?? "";
        add(group, groupNames.get(group) ?? UNCATEGORIZED_NAME, currency, amount);
        break;
      }
      case "merchant": {
        const merchant = resolve?.(transaction) ?? { id: null, name: transaction.description };
        add(merchant.id ?? merchant.name, merchant.name, currency, amount);
        break;
      }
      case "tag":
        for (const tag of new Set(transaction.tags)) {
          add(tag, tagNames.get(tag) ?? tag, currency, amount);
        }
        break;
      case "account":
        add(
          transaction.account_id,
          accountNames.get(transaction.account_id) ?? transaction.account_id,
          currency,
          amount,
        );
        break;
    }
  }
  return [...totals.values()].sort(
    (left, right) =>
      right.amount - left.amount ||
      left.name.localeCompare(right.name) ||
      left.key.localeCompare(right.key),
  );
}

export const selectSpendingBy = memoizeLast(spendingBy);

export interface TrendPoint {
  readonly month: string;
  /** Net spending in the category that month: positive for money out. */
  readonly amount: number;
}

/**
 * One category over the months asked for, a point per month whether or not
 * anything was spent. The figure is the category's net outflow, so an income
 * category trends negative and the screen flips the sign it shows.
 */
export function categoryTrend(
  transactions: readonly Transaction[],
  categoryId: string,
  months: readonly string[],
  currency?: string,
): readonly TrendPoint[] {
  const wanted = new Set(months);
  const totals = new Map<string, number>();
  for (const transaction of transactions) {
    if (!isCounted(transaction)) continue;
    if (currency !== undefined && transaction.currency !== currency) continue;
    const month = monthKey(transaction.date);
    if (!wanted.has(month)) continue;
    for (const effect of effects(transaction)) {
      if (effect.categoryId !== categoryId) continue;
      totals.set(month, (totals.get(month) ?? 0) - effect.amount);
    }
  }
  return months.map((month) => ({ month, amount: totals.get(month) ?? 0 }));
}

export const selectCategoryTrend = memoizeLast(categoryTrend);

export type SankeyNodeKind = "income" | "group" | "category" | "savings";

export interface SankeyNode {
  readonly id: string;
  readonly label: string;
  readonly kind: SankeyNodeKind;
}

export interface SankeyLink {
  readonly source: string;
  readonly target: string;
  readonly value: number;
}

export interface SankeyGraph {
  readonly nodes: readonly SankeyNode[];
  readonly links: readonly SankeyLink[];
}

export const SANKEY_HUB = "income";
export const SANKEY_SAVINGS = "savings";
export const SANKEY_DEFICIT = "deficit";

/**
 * Income into spending, in three columns: each income category into one
 * "Income" hub, the hub into each expense group by what the group spent and
 * into "Savings" by what was left, and each group into its categories.
 *
 * A Sankey has to conserve -- what flows into a node flows out of it -- and
 * money does not always oblige: a range can spend more than it earned, and a
 * category can end the range with net refunds. So only positive flows are
 * drawn (a category refunded on balance is left out, and its group's value is
 * the sum of what is drawn), and when spending exceeds income a "Deficit"
 * node on the income side supplies the difference. The picture is then
 * consistent with itself; the exact totals are the summary's.
 */
export function sankeyFlows(
  transactions: readonly Transaction[],
  categories: readonly TaxonomyEntry[],
  groups: readonly TaxonomyEntry[],
  range: DateRange | null,
  currency?: string,
): SankeyGraph {
  const kinds = categoryKinds(categories);
  const categoryNames = nameIndex(categories.filter(isCategory));
  const groupNames = nameIndex(groups.filter(isGroup));
  const parents = new Map<string, string>();
  for (const entry of categories) {
    if (isCategory(entry) && entry.parent_id !== undefined && groupNames.has(entry.parent_id)) {
      parents.set(entry.id, entry.parent_id);
    }
  }
  const incomeByCategory = new Map<string, number>();
  const spendingByCategory = new Map<string, number>();
  for (const { effect, side, amount } of countedEffects(transactions, range, kinds, currency)) {
    const totals = side === "income" ? incomeByCategory : spendingByCategory;
    totals.set(effect.categoryId, (totals.get(effect.categoryId) ?? 0) + amount);
  }

  const nodes: SankeyNode[] = [];
  const links: SankeyLink[] = [];
  const largestFirst = (left: [string, number], right: [string, number]): number =>
    right[1] - left[1] || left[0].localeCompare(right[0]);

  let incomeTotal = 0;
  for (const [categoryId, amount] of [...incomeByCategory.entries()].sort(largestFirst)) {
    if (amount <= 0) continue;
    const id = `income:${categoryId}`;
    nodes.push({
      id,
      label: categoryNames.get(categoryId) ?? (categoryId === "" ? "Other income" : categoryId),
      kind: "income",
    });
    links.push({ source: id, target: SANKEY_HUB, value: amount });
    incomeTotal += amount;
  }

  const byGroup = new Map<string, Array<[string, number]>>();
  for (const [categoryId, amount] of spendingByCategory) {
    if (amount <= 0) continue;
    const group = parents.get(categoryId) ?? "";
    const members = byGroup.get(group) ?? [];
    members.push([categoryId, amount]);
    byGroup.set(group, members);
  }
  const groupTotals = [...byGroup.entries()]
    .map(([group, members]): [string, number] => [
      group,
      members.reduce((total, [, amount]) => total + amount, 0),
    ])
    .sort(largestFirst);
  const spendingTotal = groupTotals.reduce((total, [, amount]) => total + amount, 0);

  if (spendingTotal > incomeTotal) {
    nodes.push({ id: SANKEY_DEFICIT, label: "Deficit", kind: "income" });
    links.push({ source: SANKEY_DEFICIT, target: SANKEY_HUB, value: spendingTotal - incomeTotal });
  }
  nodes.push({ id: SANKEY_HUB, label: "Income", kind: "income" });

  const categoryNodes: SankeyNode[] = [];
  for (const [group, total] of groupTotals) {
    const groupNode = `group:${group}`;
    nodes.push({
      id: groupNode,
      label: groupNames.get(group) ?? UNCATEGORIZED_NAME,
      kind: "group",
    });
    links.push({ source: SANKEY_HUB, target: groupNode, value: total });
    for (const [categoryId, amount] of (byGroup.get(group) ?? []).sort(largestFirst)) {
      const categoryNode = `category:${categoryId}`;
      categoryNodes.push({
        id: categoryNode,
        label: categoryNames.get(categoryId) ?? UNCATEGORIZED_NAME,
        kind: "category",
      });
      links.push({ source: groupNode, target: categoryNode, value: amount });
    }
  }
  nodes.push(...categoryNodes);

  if (incomeTotal > spendingTotal) {
    nodes.push({ id: SANKEY_SAVINGS, label: "Savings", kind: "savings" });
    links.push({ source: SANKEY_HUB, target: SANKEY_SAVINGS, value: incomeTotal - spendingTotal });
  }
  return { nodes, links };
}

export const selectSankeyFlows = memoizeLast(sankeyFlows);
