import { holdingsValue } from "../../functions/shared/balances.js";
import { type Account, type AssetClass, HOLDING_TYPES, type Holding } from "../model/types.js";
import { type CurrencyTotal, totalsByCurrency } from "./balances.js";
import { memoizeLast } from "./memo.js";

/**
 * Positions held in investment and crypto accounts, and what they are worth.
 *
 * A holding is valued at its last price, `round(quantity × price)`, by the
 * shared balance engine -- the same valuation that goes into the account's
 * balance, the net-worth figure, and the nightly snapshot -- so the
 * investments page and the accounts page cannot disagree about a position.
 * Gain is against the cost basis when one was recorded, and null rather than
 * zero when it was not: "no gain" and "unknown gain" are different answers.
 */

export function holdingValue(holding: Holding): number {
  return holdingsValue([holding]);
}

export function holdingGain(holding: Holding): number | null {
  return holding.cost_basis === undefined ? null : holdingValue(holding) - holding.cost_basis;
}

export interface HoldingRow {
  readonly account: Account;
  readonly holding: Holding;
  readonly value: number;
  readonly gain: number | null;
}

/** Every position of every open account, most valuable first. */
export function householdHoldings(accounts: readonly Account[]): readonly HoldingRow[] {
  const rows: HoldingRow[] = [];
  for (const account of accounts) {
    if (account.closed_at !== undefined) continue;
    for (const holding of account.holdings ?? []) {
      rows.push({ account, holding, value: holdingValue(holding), gain: holdingGain(holding) });
    }
  }
  return rows.sort(
    (left, right) =>
      right.value - left.value ||
      left.holding.symbol.localeCompare(right.holding.symbol) ||
      left.holding.id.localeCompare(right.holding.id),
  );
}

export const selectHouseholdHoldings = memoizeLast(householdHoldings);

export interface AllocationSlice {
  readonly asset_class: AssetClass;
  readonly value: number;
  /** Of the total, between 0 and 1. */
  readonly share: number;
}

/**
 * How the positions divide by asset class, largest first. Given a currency,
 * only the rows in it -- an allocation across currencies would be a sum of
 * unlike things.
 */
export function allocationByClass(
  rows: readonly HoldingRow[],
  currency?: string,
): readonly AllocationSlice[] {
  const totals = new Map<AssetClass, number>();
  let total = 0;
  for (const row of rows) {
    if (currency !== undefined && row.account.currency !== currency) continue;
    totals.set(row.holding.asset_class, (totals.get(row.holding.asset_class) ?? 0) + row.value);
    total += row.value;
  }
  return [...totals.entries()]
    .map(([asset_class, value]) => ({
      asset_class,
      value,
      share: total > 0 ? value / total : 0,
    }))
    .sort(
      (left, right) =>
        right.value - left.value || left.asset_class.localeCompare(right.asset_class),
    );
}

/** The balance of every open investment and crypto account, per currency: positions and cash alike. */
export function investmentsTotal(
  accounts: readonly Account[],
  balances: ReadonlyMap<string, number>,
): readonly CurrencyTotal[] {
  return totalsByCurrency(
    accounts.filter(
      (account) => account.closed_at === undefined && HOLDING_TYPES.includes(account.type),
    ),
    balances,
  );
}

export const selectInvestmentsTotal = memoizeLast(investmentsTotal);
