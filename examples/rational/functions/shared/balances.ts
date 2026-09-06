/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * One definition of an account's balance, for the screens and the nightly
 * snapshot alike.
 *
 * A balance is what the bank would say: the opening balance plus every
 * transaction booked to the account -- transfers, hidden ones, and balance
 * updates included, because they moved the account even where they moved no
 * money of the household's -- plus, for an investment or crypto account, what
 * its holdings are worth at their last price. A snapshot written from this
 * and an account page drawn from this show the same number, which is the
 * point of having one place to compute it.
 */

/** What valuation reads of a holding: units held and minor units per unit. */
export interface HoldingLike {
  readonly quantity: number;
  readonly price: number;
}

/** What the balance math reads of an account. */
export interface BalanceAccount {
  readonly id: string;
  readonly opening_balance: number;
  readonly holdings?: readonly HoldingLike[];
}

/** What the balance math reads of a transaction. */
export interface BalanceTransaction {
  readonly account_id: string;
  readonly amount: number;
}

/** What net worth reads of an account, over what a balance needs. */
export interface NetWorthAccount extends BalanceAccount {
  readonly type: string;
  readonly currency: string;
  readonly closed_at?: number;
  readonly hide_from_net_worth?: boolean;
}

export interface SnapshotBalance {
  readonly account_id: string;
  readonly balance: number;
}

/** Accounts whose balance is money owed rather than money held. */
export const LIABILITY_TYPES: readonly string[] = ["credit", "loan", "other_liability"];

/**
 * The value of a set of positions, each rounded on its own: fractional
 * shares at a price in minor units leave fractions of a cent, and rounding
 * per position is what a brokerage statement does.
 */
export function holdingsValue(holdings: ReadonlyArray<HoldingLike> | undefined): number {
  if (holdings === undefined) return 0;
  let total = 0;
  for (const holding of holdings) {
    total += Math.round(holding.quantity * holding.price);
  }
  return total;
}

/** Balance per account id: opening balance, every transaction, and the holdings' value. */
export function accountBalances(
  accounts: readonly BalanceAccount[],
  transactions: readonly BalanceTransaction[],
): Map<string, number> {
  const balances = new Map<string, number>();
  for (const account of accounts) {
    balances.set(account.id, account.opening_balance + holdingsValue(account.holdings));
  }
  for (const transaction of transactions) {
    const current = balances.get(transaction.account_id);
    if (current !== undefined) balances.set(transaction.account_id, current + transaction.amount);
  }
  return balances;
}

/**
 * Assets and liabilities in one currency over the accounts that are open and
 * not hidden from net worth. A liability's balance is negative from the
 * household's side; it is reported as the magnitude owed. Nothing is
 * converted, so a foreign-currency account is simply not in this sum.
 */
export function netWorth(
  currency: string,
  accounts: readonly NetWorthAccount[],
  balances: ReadonlyMap<string, number>,
): { readonly assets: number; readonly liabilities: number } {
  let assets = 0;
  let liabilities = 0;
  for (const account of accounts) {
    if (account.closed_at !== undefined || account.hide_from_net_worth === true) continue;
    if (account.currency !== currency) continue;
    const balance =
      balances.get(account.id) ?? account.opening_balance + holdingsValue(account.holdings);
    if (LIABILITY_TYPES.includes(account.type)) liabilities += -balance;
    else assets += balance;
  }
  return { assets, liabilities };
}

/**
 * Each open account's balance, for the night's snapshot. Hidden accounts are
 * recorded too: hiding is about the net-worth figure, and an account page
 * still wants its own history. Sorted by id so two nights write the same
 * array in the same order.
 */
export function snapshotBalances(
  accounts: ReadonlyArray<BalanceAccount & { readonly closed_at?: number }>,
  balances: ReadonlyMap<string, number>,
): SnapshotBalance[] {
  return accounts
    .filter((account) => account.closed_at === undefined)
    .map((account) => ({
      account_id: account.id,
      balance:
        balances.get(account.id) ?? account.opening_balance + holdingsValue(account.holdings),
    }))
    .sort((left, right) => left.account_id.localeCompare(right.account_id));
}
