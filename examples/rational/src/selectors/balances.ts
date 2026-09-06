import {
  accountBalances as sharedAccountBalances,
  netWorth as sharedNetWorth,
  snapshotBalances as sharedSnapshotBalances,
} from "../../functions/shared/balances.js";
import {
  ACCOUNT_CLASSES,
  type Account,
  accountClassOf,
  type SnapshotBalance,
  type Transaction,
} from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * Balances as the screens show them.
 *
 * The arithmetic is the shared engine's (`functions/shared/balances.ts`): the
 * nightly snapshot is written from the same functions, and an account page
 * whose number disagreed with last night's snapshot would be a bug with two
 * homes. What lives here is the application's own types, and the groupings
 * only the screens need.
 */
export { holdingsValue } from "../../functions/shared/balances.js";

/**
 * Balance per account id: the opening balance, every transaction of the
 * account -- transfers, hidden ones, and balance updates included, because
 * the bank counts them -- and the value of its holdings.
 */
export function accountBalances(
  accounts: readonly Account[],
  transactions: readonly Transaction[],
): ReadonlyMap<string, number> {
  return sharedAccountBalances(accounts, transactions);
}

export const selectAccountBalances = memoizeLast(accountBalances);

export function accountBalance(account: Account, transactions: readonly Transaction[]): number {
  return sharedAccountBalances([account], transactions).get(account.id) ?? account.opening_balance;
}

export interface NetWorth {
  readonly currency: string;
  readonly assets: number;
  readonly liabilities: number;
  readonly netWorth: number;
}

/** Net worth in one currency; zeros when no account is in it. */
export function netWorthIn(
  currency: string,
  accounts: readonly Account[],
  balances: ReadonlyMap<string, number>,
): NetWorth {
  const { assets, liabilities } = sharedNetWorth(currency, accounts, balances);
  return { currency, assets, liabilities, netWorth: assets - liabilities };
}

/**
 * Net worth per currency over the accounts that are open and not hidden from
 * it: assets are the balances of asset accounts, liabilities the owed
 * magnitude of the rest. No conversion happens between currencies.
 */
export function netWorthByCurrency(
  accounts: readonly Account[],
  balances: ReadonlyMap<string, number>,
): readonly NetWorth[] {
  const currencies = new Set<string>();
  for (const account of accounts) {
    if (account.closed_at !== undefined || account.hide_from_net_worth === true) continue;
    currencies.add(account.currency);
  }
  return [...currencies].sort().map((currency) => netWorthIn(currency, accounts, balances));
}

export const selectNetWorth = memoizeLast(netWorthByCurrency);

/** Each open account's balance, in the shape the nightly snapshot records. */
export function snapshotBalances(
  accounts: readonly Account[],
  balances: ReadonlyMap<string, number>,
): readonly SnapshotBalance[] {
  return sharedSnapshotBalances(accounts, balances);
}

export interface CurrencyTotal {
  readonly currency: string;
  readonly total: number;
}

/** Balances summed per currency, signed as they are: a class of credit cards totals negative. */
export function totalsByCurrency(
  accounts: readonly Account[],
  balances: ReadonlyMap<string, number>,
): readonly CurrencyTotal[] {
  const totals = new Map<string, number>();
  for (const account of accounts) {
    const balance = balances.get(account.id) ?? account.opening_balance;
    totals.set(account.currency, (totals.get(account.currency) ?? 0) + balance);
  }
  return [...totals.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([currency, total]) => ({ currency, total }));
}

export type AccountClass = (typeof ACCOUNT_CLASSES)[number];

export interface AccountClassGroup {
  readonly class: AccountClass;
  readonly accounts: readonly Account[];
  readonly subtotals: readonly CurrencyTotal[];
}

/**
 * The accounts page's shape: every class that has an account, in the order
 * the classes are declared, each with its accounts by name and a subtotal per
 * currency. Nothing is filtered here -- a page that wants to keep closed
 * accounts apart passes the open ones -- and an empty class is left out
 * rather than shown as a heading over nothing.
 */
export function accountsByClass(
  accounts: readonly Account[],
  balances: ReadonlyMap<string, number>,
): readonly AccountClassGroup[] {
  const groups: AccountClassGroup[] = [];
  for (const entry of ACCOUNT_CLASSES) {
    const members = accounts
      .filter((account) => accountClassOf(account.type) === entry.id)
      .sort(
        (left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id),
      );
    if (members.length === 0) continue;
    groups.push({
      class: entry,
      accounts: members,
      subtotals: totalsByCurrency(members, balances),
    });
  }
  return groups;
}

export const selectAccountsByClass = memoizeLast(accountsByClass);
