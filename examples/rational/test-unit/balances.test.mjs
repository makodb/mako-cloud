import assert from "node:assert/strict";
import { test } from "node:test";

import {
  accountBalances,
  holdingsValue,
  netWorth,
  snapshotBalances,
} from "../functions/shared/balances.ts";

/**
 * A balance is what the bank says, so the tests here are about the things
 * every other total leaves out -- transfers, balance updates, hidden
 * transactions -- being counted, and about holdings being worth what their
 * price says.
 */

function account(overrides = {}) {
  return {
    id: "acct-1",
    type: "checking",
    currency: "USD",
    opening_balance: 100_000,
    ...overrides,
  };
}

test("holdings are valued position by position, rounded to the cent", () => {
  assert.equal(holdingsValue(undefined), 0);
  assert.equal(holdingsValue([]), 0);
  // 2.5 × 100.00 and 0.3333 × 1.00: the fraction of a cent rounds away.
  assert.equal(
    holdingsValue([
      { quantity: 2.5, price: 10_000 },
      { quantity: 0.3333, price: 100 },
    ]),
    25_033,
  );
});

test("a balance is the opening balance, every transaction, and the holdings' value", () => {
  const accounts = [
    account(),
    account({ id: "acct-inv", type: "investment", opening_balance: 1_000, holdings: [{ quantity: 2.5, price: 10_000 }] }),
    account({ id: "acct-house", type: "real_estate", opening_balance: 30_000_000 }),
  ];
  const transactions = [
    { account_id: "acct-1", amount: -25_000 },
    { account_id: "acct-1", amount: 5_000 },
    // A transfer leg and a hidden transaction moved the account, whatever
    // the reports say about them.
    { account_id: "acct-1", amount: -10_000, transfer_id: "tr_1" },
    { account_id: "acct-1", amount: -2_000, hidden: true },
    // A balance update is how a tracked account moves at all.
    { account_id: "acct-house", amount: 1_000_000, adjustment: true },
    { account_id: "acct-unknown", amount: 999 },
  ];
  const balances = accountBalances(accounts, transactions);
  assert.equal(balances.get("acct-1"), 68_000);
  assert.equal(balances.get("acct-inv"), 26_000);
  assert.equal(balances.get("acct-house"), 31_000_000);
  assert.equal(balances.has("acct-unknown"), false);
});

test("net worth counts open, shown accounts of one currency, liabilities as owed", () => {
  const accounts = [
    account({ id: "cash", opening_balance: 80_000 }),
    account({ id: "card", type: "credit", opening_balance: -40_000 }),
    account({ id: "loan", type: "loan", opening_balance: -200_000 }),
    account({ id: "iou", type: "other_liability", opening_balance: -10_000 }),
    account({ id: "hidden", opening_balance: 999_999, hide_from_net_worth: true }),
    account({ id: "closed", opening_balance: 555_555, closed_at: 1 }),
    account({ id: "euro", currency: "EUR", opening_balance: 777_777 }),
  ];
  const balances = accountBalances(accounts, []);
  assert.deepEqual(netWorth("USD", accounts, balances), { assets: 80_000, liabilities: 250_000 });
  assert.deepEqual(netWorth("EUR", accounts, balances), { assets: 777_777, liabilities: 0 });
});

test("an account missing from the balances falls back to what it opened with, holdings included", () => {
  const accounts = [
    account({ id: "inv", type: "investment", opening_balance: 500, holdings: [{ quantity: 1, price: 1_000 }] }),
  ];
  assert.deepEqual(netWorth("USD", accounts, new Map()), { assets: 1_500, liabilities: 0 });
});

test("a snapshot records every open account, hidden ones included, by id", () => {
  const accounts = [
    account({ id: "zeta", opening_balance: 1 }),
    account({ id: "alpha", opening_balance: 2, hide_from_net_worth: true }),
    account({ id: "closed", opening_balance: 3, closed_at: 1 }),
  ];
  const balances = accountBalances(accounts, [{ account_id: "zeta", amount: 10 }]);
  assert.deepEqual(snapshotBalances(accounts, balances), [
    { account_id: "alpha", balance: 2 },
    { account_id: "zeta", balance: 11 },
  ]);
});
