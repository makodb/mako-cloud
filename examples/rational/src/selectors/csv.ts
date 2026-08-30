import type { Transaction } from "../model/types.js";
import { parseAmount } from "./money.js";
import { isIsoDate, normalizeDescription } from "./transactions.js";

/**
 * Reading a bank's CSV export.
 *
 * Banks do not agree on anything: the column names, the date order, whether
 * an expense is negative or lives in its own column, whether amounts carry
 * thousands separators or a trailing `CR`. So the parser reads a shape the
 * person confirms rather than guessing once and being wrong quietly -- it
 * proposes a mapping, shows what the first rows become under it, and imports
 * only what the person accepted.
 */

export interface CsvTable {
  readonly headers: readonly string[];
  readonly rows: ReadonlyArray<readonly string[]>;
}

export interface ColumnMapping {
  readonly date: string;
  readonly description: string;
  /** One signed column, or a debit/credit pair. */
  readonly amount?: string;
  readonly debit?: string;
  readonly credit?: string;
  /** `DMY` when the export writes 03/04/2026 for the fourth of March. */
  readonly dateOrder: "ISO" | "MDY" | "DMY";
}

export interface ParsedRow {
  readonly line: number;
  readonly date: string;
  readonly description: string;
  readonly amount: number;
  readonly problem?: string;
}

/**
 * RFC 4180 with the concessions real exports need: a UTF-8 BOM, CRLF or LF,
 * quoted fields containing separators or doubled quotes, and a trailing
 * newline. A row with the wrong number of fields is kept and reported rather
 * than dropped -- an import that silently loses a line is worse than one that
 * says which line it could not read.
 */
export function parseCsv(text: string): CsvTable {
  const source = text.replace(/^\uFEFF/u, "");
  const rows: string[][] = [];
  let field = "";
  let row: string[] = [];
  let quoted = false;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    if (quoted) {
      if (character === '"') {
        if (source[index + 1] === '"') {
          field += '"';
          index += 1;
        } else {
          quoted = false;
        }
      } else {
        field += character;
      }
      continue;
    }
    if (character === '"') {
      quoted = true;
    } else if (character === ",") {
      row.push(field);
      field = "";
    } else if (character === "\n" || character === "\r") {
      if (character === "\r" && source[index + 1] === "\n") index += 1;
      row.push(field);
      field = "";
      if (row.some((value) => value.trim() !== "")) rows.push(row);
      row = [];
    } else {
      field += character;
    }
  }
  row.push(field);
  if (row.some((value) => value.trim() !== "")) rows.push(row);
  const [headers = [], ...body] = rows;
  return { headers: headers.map((header) => header.trim()), rows: body };
}

const DATE_HEADERS = ["date", "transaction date", "posted date", "posting date", "date posted"];
const DESCRIPTION_HEADERS = ["description", "payee", "name", "memo", "details", "narrative"];
const AMOUNT_HEADERS = ["amount", "value", "transaction amount"];
const DEBIT_HEADERS = ["debit", "withdrawal", "withdrawals", "money out", "paid out"];
const CREDIT_HEADERS = ["credit", "deposit", "deposits", "money in", "paid in"];

/** A first guess at the mapping, which the person then confirms or changes. */
export function proposeMapping(table: CsvTable): ColumnMapping | null {
  const find = (candidates: readonly string[]) =>
    table.headers.find((header) => candidates.includes(header.trim().toLowerCase()));
  const date = find(DATE_HEADERS);
  const description = find(DESCRIPTION_HEADERS);
  if (date === undefined || description === undefined) return null;
  const amount = find(AMOUNT_HEADERS);
  const debit = find(DEBIT_HEADERS);
  const credit = find(CREDIT_HEADERS);
  if (amount === undefined && debit === undefined && credit === undefined) return null;
  return {
    date,
    description,
    ...(amount === undefined ? {} : { amount }),
    ...(debit === undefined ? {} : { debit }),
    ...(credit === undefined ? {} : { credit }),
    dateOrder: guessDateOrder(table, date),
  };
}

/**
 * `03/04/2026` is ambiguous, and a wrong guess moves a transaction by months.
 * A column where some row has a first part above twelve can only be day-first;
 * one where some row has a second part above twelve can only be month-first.
 * With no evidence either way it stays month-first, which is what the exports
 * that use slashes overwhelmingly are, and the person can change it.
 */
export function guessDateOrder(table: CsvTable, dateHeader: string): "ISO" | "MDY" | "DMY" {
  const index = table.headers.indexOf(dateHeader);
  if (index < 0) return "ISO";
  let iso = 0;
  for (const row of table.rows) {
    const value = (row[index] ?? "").trim();
    if (isIsoDate(value)) {
      iso += 1;
      continue;
    }
    const parts = value.split(/[/.-]/u);
    const first = Number(parts[0]);
    const second = Number(parts[1]);
    if (Number.isInteger(first) && first > 12) return "DMY";
    if (Number.isInteger(second) && second > 12) return "MDY";
  }
  return iso > 0 ? "ISO" : "MDY";
}

/** One CSV row as a transaction would be, or with the reason it cannot be. */
export function parseRows(
  table: CsvTable,
  mapping: ColumnMapping,
  currency: string,
): readonly ParsedRow[] {
  const column = (header: string | undefined) =>
    header === undefined ? -1 : table.headers.indexOf(header);
  const dateAt = column(mapping.date);
  const descriptionAt = column(mapping.description);
  const amountAt = column(mapping.amount);
  const debitAt = column(mapping.debit);
  const creditAt = column(mapping.credit);
  return table.rows.map((row, index) => {
    const line = index + 2;
    const date = normalizeDate((row[dateAt] ?? "").trim(), mapping.dateOrder);
    const description = (row[descriptionAt] ?? "").trim();
    const amount = readAmount(row, { amountAt, debitAt, creditAt }, currency);
    const problem =
      date === null
        ? "the date could not be read"
        : description === ""
          ? "the description is empty"
          : amount === null
            ? "the amount could not be read"
            : undefined;
    return {
      line,
      date: date ?? "",
      description,
      amount: amount ?? 0,
      ...(problem === undefined ? {} : { problem }),
    };
  });
}

function readAmount(
  row: readonly string[],
  columns: { amountAt: number; debitAt: number; creditAt: number },
  currency: string,
): number | null {
  if (columns.amountAt >= 0) {
    return readSigned(row[columns.amountAt] ?? "", currency);
  }
  const debit = columns.debitAt < 0 ? null : readSigned(row[columns.debitAt] ?? "", currency);
  const credit = columns.creditAt < 0 ? null : readSigned(row[columns.creditAt] ?? "", currency);
  // A debit column holds a positive number for money leaving the account.
  if (debit !== null && debit !== 0) return -Math.abs(debit);
  if (credit !== null && credit !== 0) return Math.abs(credit);
  return debit === null && credit === null ? null : 0;
}

/** `1,234.56`, `(12.34)`, `12.34 CR`, `-12.34`, and an empty cell. */
function readSigned(value: string, currency: string): number | null {
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const credit = /\bCR\b/iu.test(trimmed);
  const debit = /\bDR\b/iu.test(trimmed);
  const parenthesised = trimmed.startsWith("(") && trimmed.endsWith(")");
  const cleaned = trimmed
    .replace(/\b(CR|DR)\b/giu, "")
    .replaceAll(/[()]/gu, "")
    .replaceAll(",", "")
    .trim();
  if (cleaned === "") return null;
  try {
    const amount = parseAmount(cleaned, currency);
    if (parenthesised || debit) return -Math.abs(amount);
    if (credit) return Math.abs(amount);
    return amount;
  } catch {
    return null;
  }
}

/** `03/04/2026` under an order, or an ISO date, or null. */
export function normalizeDate(value: string, order: "ISO" | "MDY" | "DMY"): string | null {
  if (isIsoDate(value)) return value;
  const parts = value.split(/[/.-]/u).map((part) => part.trim());
  if (parts.length !== 3) return null;
  const [first = "", second = "", third = ""] = parts;
  const year = third.length === 2 ? `20${third}` : third;
  const [month, day] = order === "DMY" ? [second, first] : [first, second];
  const iso = `${year.padStart(4, "0")}-${month.padStart(2, "0")}-${day.padStart(2, "0")}`;
  return isIsoDate(iso) ? iso : null;
}

export interface ImportPlan {
  readonly rows: readonly ParsedRow[];
  readonly duplicates: ReadonlySet<number>;
  readonly unreadable: readonly ParsedRow[];
  readonly importable: readonly ParsedRow[];
}

/**
 * What an import would do, decided before anything is written.
 *
 * A duplicate is a row whose account, date, amount, and normalized
 * description already exist locally -- the same test the nightly job will
 * make server-side for synced transactions.
 *
 * Matching is by count, not by presence. A person can buy the same coffee
 * twice in a day, and the bank exports both rows; if only one is stored, the
 * second row is a transaction that has not been imported yet. Treating every
 * matching row as a duplicate would silently drop it, and treating none as
 * one would double the coffee on every re-import.
 */
export function planImport(
  rows: readonly ParsedRow[],
  existing: readonly Transaction[],
  accountId: string,
): ImportPlan {
  const unmatched = new Map<string, number>();
  for (const transaction of existing) {
    if (transaction.account_id !== accountId) continue;
    const key = fingerprint(transaction.date, transaction.amount, transaction.description);
    unmatched.set(key, (unmatched.get(key) ?? 0) + 1);
  }
  const duplicates = new Set<number>();
  const unreadable: ParsedRow[] = [];
  const importable: ParsedRow[] = [];
  for (const row of rows) {
    if (row.problem !== undefined) {
      unreadable.push(row);
      continue;
    }
    const key = fingerprint(row.date, row.amount, row.description);
    const remaining = unmatched.get(key) ?? 0;
    if (remaining > 0) {
      unmatched.set(key, remaining - 1);
      duplicates.add(row.line);
      continue;
    }
    importable.push(row);
  }
  return { rows, duplicates, unreadable, importable };
}

export function fingerprint(date: string, amount: number, description: string): string {
  return `${date}|${amount}|${normalizeDescription(description)}`;
}
