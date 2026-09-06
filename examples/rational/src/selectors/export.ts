import {
  ACCOUNT_CLASSES,
  ACCOUNT_TYPE_LABELS,
  type Account,
  accountClassOf,
  isCategory,
  isGroup,
  isTag,
  type TaxonomyEntry,
  type Transaction,
} from "../model/types.js";
import { merchantResolver } from "./merchants.js";
import { amountToText } from "./money.js";

/**
 * The household's data as a spreadsheet would like it.
 *
 * RFC 4180: fields are quoted when they hold a comma, a quote, or a line
 * break, quotes inside are doubled, and rows end in CRLF. Amounts are decimal
 * text rather than minor units because the file is for people and their
 * spreadsheets, and names stand in for ids for the same reason -- an export
 * is a way out, and a way out should not need the model to read.
 */

export function csvEscape(value: string): string {
  return /[",\r\n]/u.test(value) ? `"${value.replaceAll('"', '""')}"` : value;
}

export function csvRow(fields: readonly string[]): string {
  return fields.map(csvEscape).join(",");
}

export function csvDocument(
  header: readonly string[],
  rows: ReadonlyArray<readonly string[]>,
): string {
  return `${[header, ...rows].map(csvRow).join("\r\n")}\r\n`;
}

function flag(value: boolean | undefined): string {
  return value === true ? "true" : "false";
}

export const TRANSACTION_CSV_COLUMNS = [
  "date",
  "account",
  "description",
  "merchant",
  "category",
  "group",
  "tags",
  "amount",
  "currency",
  "notes",
  "hidden",
  "transfer_id",
] as const;

export interface TransactionExportContext {
  readonly accounts: readonly Account[];
  /** Categories, groups, and tags; merchants too unless given apart. */
  readonly taxonomy: readonly TaxonomyEntry[];
  readonly merchants?: readonly TaxonomyEntry[];
}

/**
 * One row per transaction, in the order given -- the screen exports the list
 * it shows. A split transaction is one row under its own category; the
 * splits are the model's detail, and a spreadsheet wants the amount once.
 */
export function transactionsCsv(
  transactions: readonly Transaction[],
  context: TransactionExportContext,
): string {
  const accountNames = new Map(context.accounts.map((account) => [account.id, account.name]));
  const categories = new Map(
    context.taxonomy.filter(isCategory).map((entry) => [entry.id, entry] as const),
  );
  const groupNames = new Map(
    context.taxonomy.filter(isGroup).map((entry) => [entry.id, entry.name]),
  );
  const tagNames = new Map(context.taxonomy.filter(isTag).map((entry) => [entry.id, entry.name]));
  const resolve = merchantResolver(context.merchants ?? context.taxonomy);
  const rows = transactions.map((transaction) => {
    const category =
      transaction.category_id === undefined ? undefined : categories.get(transaction.category_id);
    const group =
      category?.parent_id === undefined ? "" : (groupNames.get(category.parent_id) ?? "");
    return [
      transaction.date,
      accountNames.get(transaction.account_id) ?? transaction.account_id,
      transaction.description,
      resolve(transaction).name,
      category?.name ?? "",
      group,
      transaction.tags.map((tag) => tagNames.get(tag) ?? tag).join(";"),
      amountToText(transaction.amount, transaction.currency),
      transaction.currency,
      transaction.notes ?? "",
      flag(transaction.hidden),
      transaction.transfer_id ?? "",
    ];
  });
  return csvDocument(TRANSACTION_CSV_COLUMNS, rows);
}

export const ACCOUNT_CSV_COLUMNS = [
  "name",
  "class",
  "type",
  "currency",
  "balance",
  "institution",
  "hidden",
  "closed",
] as const;

export function accountsCsv(
  accounts: readonly Account[],
  balances: ReadonlyMap<string, number>,
): string {
  const classLabels = new Map<string, string>(
    ACCOUNT_CLASSES.map((entry) => [entry.id, entry.label]),
  );
  const rows = accounts.map((account) => [
    account.name,
    classLabels.get(accountClassOf(account.type)) ?? account.type,
    ACCOUNT_TYPE_LABELS[account.type] ?? account.type,
    account.currency,
    amountToText(balances.get(account.id) ?? account.opening_balance, account.currency),
    account.institution ?? "",
    flag(account.hide_from_net_worth),
    flag(account.closed_at !== undefined),
  ]);
  return csvDocument(ACCOUNT_CSV_COLUMNS, rows);
}
