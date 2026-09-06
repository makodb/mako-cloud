/**
 * What the nightly job decides, separated from how it reads and writes.
 *
 * Every decision here is a pure function of the documents the job read, so it
 * can be tested without a runtime, a service credential, or a network -- and
 * so the published copy of this application can run those tests too, where the
 * edge SDK the function itself imports does not exist.
 *
 * Unlike the other modules of this directory, this one is used by the
 * functions alone and never by the browser, so it may import its neighbours
 * with the `.ts` specifiers Deno wants; the rest must import nothing (see
 * `rules.ts`).
 */

import type { AlertBill, AlertGoal } from "./alerts.ts";
import {
  accountBalances,
  type BalanceAccount,
  type NetWorthAccount,
  netWorth as netWorthOf,
  type SnapshotBalance,
  snapshotBalances as snapshotBalancesOf,
} from "./balances.ts";
import { type GoalLike, goalProgress } from "./goals.ts";
import {
  advanceAfterPayment,
  type BillRecurrence,
  type BillTransaction,
  daysBetween,
  normalizeDescription,
  resolveBills,
} from "./recurrences.ts";
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

export interface Filing {
  readonly documentId: string;
  readonly revision: string;
  readonly body: DocumentBody;
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function optionalText(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" ? value : undefined;
}

function live(documents: readonly DocumentLike[]): DocumentBody[] {
  return documents.filter((document) => !document._deleted).map((document) => document.body);
}

/**
 * What the rules would file, decided without writing anything.
 *
 * The first matching rule by priority is applied with every action it
 * states, the same engine the application runs. Two guards keep the job from
 * overruling a person: a category already on the transaction, or carried by
 * its splits, is never replaced -- the category is the one thing a member
 * files by hand most -- and a transaction the same rule has already touched
 * (its `rule_id`) is left alone, so a member who un-hides or re-tags what a
 * rule did once does not fight the job every night. The other actions of a
 * rule are still applied to a categorized transaction it matches: a rule that
 * hides a fee or marks a merchant's charges reviewed is about those charges
 * whatever category they carry, and applying only the part that is missing
 * is what makes a second night find nothing left to do.
 *
 * Transfer legs and balance updates are not filed: neither is a purchase.
 * Hidden transactions are, because hiding is about reports, not about what
 * the transaction is.
 */
export function filings(
  transactions: readonly DocumentLike[],
  rules: readonly DocumentLike[],
  now: number,
): readonly Filing[] {
  const enabled = live(rules) as unknown as RuleLike[];
  if (enabled.length === 0) return [];
  const decided: Filing[] = [];
  for (const document of transactions) {
    if (document._deleted) continue;
    const body = document.body;
    if (body.adjustment === true || optionalText(body.transfer_id) !== undefined) continue;
    const description = text(body.description);
    const categoryId = optionalText(body.category_id);
    const merchantId = optionalText(body.merchant_id);
    const outcome = applyRules(enabled, {
      description,
      normalized_description:
        optionalText(body.normalized_description) ?? normalizeDescription(description),
      amount: Number(body.amount ?? 0),
      account_id: text(body.account_id),
      ...(categoryId === undefined ? {} : { category_id: categoryId }),
      ...(merchantId === undefined ? {} : { merchant_id: merchantId }),
      hidden: body.hidden === true,
      reviewed: body.reviewed === true,
    });
    if (outcome === null || body.rule_id === outcome.rule.id) continue;
    const hasSplits = Array.isArray(body.splits) && body.splits.length > 0;
    const patch: DocumentBody = {};
    if (outcome.categoryId !== undefined && categoryId === undefined && !hasSplits) {
      patch.category_id = outcome.categoryId;
    }
    if (outcome.merchantId !== undefined && merchantId !== outcome.merchantId) {
      patch.merchant_id = outcome.merchantId;
    }
    if (outcome.hide === true && body.hidden !== true) patch.hidden = true;
    if (outcome.markReviewed === true && body.reviewed !== true) patch.reviewed = true;
    const existingTags = Array.isArray(body.tags) ? (body.tags as string[]) : [];
    const tags = [...new Set([...existingTags, ...outcome.tags])];
    if (tags.length !== existingTags.length) patch.tags = tags;
    if (Object.keys(patch).length === 0) continue;
    decided.push({
      documentId: text(body.id),
      revision: document.revision,
      body: { ...body, ...patch, updated_at: now, rule_id: outcome.rule.id },
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

/** An account document as the balance math reads it. */
function balanceAccount(body: DocumentBody): NetWorthAccount {
  const holdings = Array.isArray(body.holdings)
    ? (body.holdings as ReadonlyArray<Record<string, unknown>>).map((holding) => ({
        quantity: Number(holding.quantity ?? 0),
        price: Number(holding.price ?? 0),
      }))
    : undefined;
  return {
    id: text(body.id),
    opening_balance: Number(body.opening_balance ?? 0),
    type: text(body.type),
    currency: text(body.currency),
    ...(typeof body.closed_at === "number" ? { closed_at: body.closed_at } : {}),
    ...(body.hide_from_net_worth === true ? { hide_from_net_worth: true } : {}),
    ...(holdings === undefined ? {} : { holdings }),
  };
}

/**
 * Balance per account, derived the one way `balances.ts` derives them: the
 * opening balance, everything booked to the account, and the holdings' value.
 */
export function balancesOf(
  accounts: readonly DocumentLike[],
  transactions: readonly DocumentLike[],
): Map<string, number> {
  const opened: BalanceAccount[] = live(accounts).map(balanceAccount);
  return accountBalances(
    opened,
    live(transactions).map((body) => ({
      account_id: text(body.account_id),
      amount: Number(body.amount ?? 0),
    })),
  );
}

/**
 * Assets and liabilities in one currency over the open accounts not hidden
 * from net worth, from balances derived the same way the application derives
 * them.
 */
export function netWorth(
  currency: string,
  accounts: readonly DocumentLike[],
  transactions: readonly DocumentLike[],
): { readonly assets: number; readonly liabilities: number } {
  return netWorthOf(
    currency,
    live(accounts).map(balanceAccount),
    balancesOf(accounts, transactions),
  );
}

/** Each open account's balance, for the night's snapshot. */
export function snapshotBalances(
  accounts: readonly DocumentLike[],
  transactions: readonly DocumentLike[],
): SnapshotBalance[] {
  return snapshotBalancesOf(live(accounts).map(balanceAccount), balancesOf(accounts, transactions));
}

/** `.` rather than `:`, as everything a function writes must be (#12). */
export function snapshotId(householdId: string, day: string): string {
  return `nws_${householdId}.${day}`;
}

/** A recurrence document as the bill life cycle reads it. */
function billRecurrence(body: DocumentBody): BillRecurrence & { readonly body: DocumentBody } {
  const name = optionalText(body.name);
  return {
    body,
    id: text(body.id),
    account_id: text(body.account_id),
    normalized_description: text(body.normalized_description),
    interval: text(body.interval) as BillRecurrence["interval"],
    expected_amount: Number(body.expected_amount ?? 0),
    currency: text(body.currency),
    next_date: text(body.next_date),
    status: text(body.status),
    matched_count: Number(body.matched_count ?? 0),
    ...(name === undefined ? {} : { name }),
  };
}

/** A transaction document as the bill life cycle reads it. */
function billTransaction(body: DocumentBody): BillTransaction & { readonly date: string } {
  const normalized = optionalText(body.normalized_description);
  const transferId = optionalText(body.transfer_id);
  return {
    id: text(body.id),
    account_id: text(body.account_id),
    description: text(body.description),
    amount: Number(body.amount ?? 0),
    currency: text(body.currency),
    date: text(body.date),
    ...(normalized === undefined ? {} : { normalized_description: normalized }),
    hidden: body.hidden === true,
    adjustment: body.adjustment === true,
    ...(transferId === undefined ? {} : { transfer_id: transferId }),
  };
}

/**
 * The confirmed recurrences whose bill has been paid, advanced to their next
 * date, decided without writing. The job does this as well as the app so a
 * household that has not opened Rational since the rent went out does not
 * find the rent "late" on a screen the bank contradicts. Idempotent by
 * construction: once advanced, the recurrence's next date is a month on and
 * the same transaction no longer lands within its slack.
 */
export function paidBills(
  recurrences: readonly DocumentLike[],
  transactions: readonly DocumentLike[],
  today: string,
  now: number = Date.now(),
): readonly Filing[] {
  const payments = new Map<string, { readonly id: string; readonly date: string }>();
  const subjects = live(transactions).map(billTransaction);
  for (const subject of subjects) payments.set(subject.id, subject);
  const filings: Filing[] = [];
  for (const document of recurrences) {
    if (document._deleted) continue;
    const recurrence = billRecurrence(document.body);
    const [bill] = resolveBills([recurrence], subjects, today);
    if (bill === undefined || bill.state !== "paid" || bill.paidBy === undefined) continue;
    const payment = payments.get(bill.paidBy);
    if (payment === undefined) continue;
    filings.push({
      documentId: recurrence.id,
      revision: document.revision,
      body: {
        ...document.body,
        ...advanceAfterPayment(recurrence, payment, recurrence.interval),
        updated_at: now,
      },
    });
  }
  return filings;
}

/**
 * The confirmed bills as the alert engine reads them. Pass the recurrences
 * after `paidBills` has advanced them, or a bill paid early is still
 * announced as coming due.
 */
export function billSubjects(recurrences: readonly DocumentLike[], today: string): AlertBill[] {
  return live(recurrences)
    .filter((body) => body.status === "confirmed")
    .map((body) => {
      const recurrence = billRecurrence(body);
      return {
        recurrence_id: recurrence.id,
        name: recurrence.name ?? recurrence.normalized_description,
        due_date: recurrence.next_date,
        amount: recurrence.expected_amount,
        currency: recurrence.currency,
        days_away: daysBetween(today, recurrence.next_date),
      };
    });
}

/**
 * The active goals as the alert engine reads them, with progress computed
 * the one way `goals.ts` computes it over the night's balances. A goal is
 * reached when nothing remains: the target saved, or the debt gone.
 */
export function goalSubjects(
  goals: readonly DocumentLike[],
  balances: ReadonlyMap<string, number>,
  today: string,
): AlertGoal[] {
  return live(goals)
    .filter((body) => body.status === "active")
    .map((body) => {
      const goal = body as unknown as GoalLike;
      const progress = goalProgress(
        { ...goal, contributions: Array.isArray(goal.contributions) ? goal.contributions : [] },
        { today, balances },
      );
      return {
        id: text(body.id),
        name: text(body.name),
        reached: progress.remaining <= 0,
        target_amount: Number(body.target_amount ?? 0),
        currency: text(body.currency),
      };
    });
}
