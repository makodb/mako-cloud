import { type Account, LIABILITY_TYPES, type Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/** Balance per account id: opening balance plus every transaction of the account. */
export function accountBalances(
  accounts: readonly Account[],
  transactions: readonly Transaction[],
): ReadonlyMap<string, number> {
  const balances = new Map<string, number>();
  for (const account of accounts) {
    balances.set(account.id, account.opening_balance);
  }
  for (const transaction of transactions) {
    const current = balances.get(transaction.account_id);
    if (current !== undefined) {
      balances.set(transaction.account_id, current + transaction.amount);
    }
  }
  return balances;
}

export const selectAccountBalances = memoizeLast(accountBalances);

export function accountBalance(account: Account, transactions: readonly Transaction[]): number {
  return transactions.reduce(
    (balance, transaction) =>
      transaction.account_id === account.id ? balance + transaction.amount : balance,
    account.opening_balance,
  );
}

export interface NetWorth {
  readonly currency: string;
  readonly assets: number;
  readonly liabilities: number;
  readonly netWorth: number;
}

/**
 * Net worth per currency over open accounts: assets are the balances of asset
 * accounts, liabilities the owed magnitude of credit and loan accounts. No
 * conversion happens between currencies.
 */
export function netWorthByCurrency(
  accounts: readonly Account[],
  balances: ReadonlyMap<string, number>,
): readonly NetWorth[] {
  const totals = new Map<string, { assets: number; liabilities: number }>();
  for (const account of accounts) {
    if (account.closed_at !== undefined) continue;
    const balance = balances.get(account.id) ?? account.opening_balance;
    const total = totals.get(account.currency) ?? { assets: 0, liabilities: 0 };
    if (LIABILITY_TYPES.includes(account.type)) {
      total.liabilities += -balance;
    } else {
      total.assets += balance;
    }
    totals.set(account.currency, total);
  }
  return [...totals.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([currency, total]) => ({
      currency,
      assets: total.assets,
      liabilities: total.liabilities,
      netWorth: total.assets - total.liabilities,
    }));
}

export const selectNetWorth = memoizeLast(netWorthByCurrency);
