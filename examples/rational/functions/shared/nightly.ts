/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * What the nightly job decides, separated from how it reads and writes.
 *
 * Every decision here is a pure function of the documents the job read, so it
 * can be tested without a runtime, a service credential, or a network -- and
 * so the published copy of this application can run those tests too, where the
 * edge SDK the function itself imports does not exist.
 */

import { normalizeDescription } from "./recurrences.ts";
import { applyRules, type RuleLike } from "./rules.ts";

/** A document as the platform hands one to a function. */
export interface DocumentLike {
  readonly body: Record<string, unknown>;
  readonly revision: string;
  readonly _deleted?: boolean;
}

/** What a rule reads of a transaction, and what a duplicate check compares. */
export type DocumentBody = Record<string, unknown>;

/** What a marked duplicate says, and how a second night knows it said it. */
export const DUPLICATE_NOTE = "[duplicate]";

/** Accounts whose balance is money owed rather than money held. */
const LIABILITIES = ["credit_card", "loan"];

export interface Filing {
  readonly documentId: string;
  readonly revision: string;
  readonly body: DocumentBody;
}

/** What the rules would file, decided without writing anything. */
export function filings(
  transactions: readonly DocumentLike[],
  rules: readonly DocumentLike[],
  now: number,
): readonly Filing[] {
  const enabled = rules
    .filter((document) => !document._deleted)
    .map((document) => document.body as unknown as RuleLike);
  if (enabled.length === 0) return [];
  const decided: Filing[] = [];
  for (const document of transactions) {
    if (document._deleted) continue;
    const body = document.body;
    if (typeof body.category_id === "string" && body.category_id !== "") continue;
    if (Array.isArray(body.splits) && body.splits.length > 0) continue;
    const outcome = applyRules(enabled, {
      description: String(body.description ?? ""),
      amount: Number(body.amount ?? 0),
      account_id: String(body.account_id ?? ""),
    });
    if (outcome === null || outcome.categoryId === undefined) continue;
    const tags = [...new Set([...((body.tags as string[] | undefined) ?? []), ...outcome.tags])];
    decided.push({
      documentId: String(body.id),
      revision: document.revision,
      body: {
        ...body,
        updated_at: now,
        category_id: outcome.categoryId,
        rule_id: outcome.rule.id,
        ...(outcome.tags.length === 0 ? {} : { tags }),
      },
    });
  }
  return decided;
}

/** Which synced transactions repeat a manual one, decided without writing. */
export function duplicates(transactions: readonly DocumentLike[], now: number): readonly Filing[] {
  const manual = new Map<string, DocumentBody>();
  for (const document of transactions) {
    if (document._deleted) continue;
    const body = document.body;
    if (typeof body.external_id === "string" && body.external_id !== "") continue;
    manual.set(fingerprint(body), body);
  }
  const marked: Filing[] = [];
  for (const document of transactions) {
    if (document._deleted) continue;
    const body = document.body;
    if (typeof body.external_id !== "string" || body.external_id === "") continue;
    if (body.notes !== undefined && String(body.notes).includes(DUPLICATE_NOTE)) continue;
    const original = manual.get(fingerprint(body));
    if (original === undefined) continue;
    marked.push({
      documentId: String(body.id),
      revision: document.revision,
      body: {
        ...body,
        updated_at: now,
        notes: `${DUPLICATE_NOTE} also entered by hand as ${String(original.id)}`,
      },
    });
  }
  return marked;
}

/** `(account, date, amount, normalized description)`, as the import uses. */
export function fingerprint(body: DocumentBody): string {
  const description = normalizeDescription(String(body.description ?? ""));
  return `${String(body.account_id ?? "")}|${String(body.date ?? "")}|${Number(body.amount ?? 0)}|${description}`;
}

/**
 * Assets and liabilities in one currency, from balances derived the same way
 * the application derives them: an account's opening balance plus everything
 * booked to it.
 */
export function netWorth(
  currency: string,
  accounts: readonly DocumentLike[],
  transactions: readonly DocumentLike[],
): { readonly assets: number; readonly liabilities: number } {
  const balances = new Map<string, number>();
  for (const document of accounts) {
    if (document._deleted) continue;
    balances.set(String(document.body.id), Number(document.body.opening_balance ?? 0));
  }
  for (const document of transactions) {
    if (document._deleted) continue;
    const accountId = String(document.body.account_id ?? "");
    if (!balances.has(accountId)) continue;
    balances.set(accountId, (balances.get(accountId) ?? 0) + Number(document.body.amount ?? 0));
  }
  let assets = 0;
  let liabilities = 0;
  for (const document of accounts) {
    if (document._deleted || document.body.closed_at !== undefined) continue;
    if (String(document.body.currency) !== currency) continue;
    const balance = balances.get(String(document.body.id)) ?? 0;
    if (LIABILITIES.includes(String(document.body.type))) liabilities += -balance;
    else assets += balance;
  }
  return { assets, liabilities };
}

/** `.` rather than `:`, as everything a function writes must be (#12). */
export function snapshotId(householdId: string, day: string): string {
  return `nws_${householdId}.${day}`;
}
