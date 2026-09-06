import assert from "node:assert/strict";
import { test } from "node:test";

import { accountBalances } from "../dist/src/selectors/balances.js";
import {
  allocationByClass,
  holdingGain,
  holdingValue,
  householdHoldings,
  investmentsTotal,
} from "../dist/src/selectors/holdings.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function account(id, type, opening, fields = {}) {
  return {
    ...base,
    id,
    name: id,
    type,
    currency: "USD",
    opening_balance: opening,
    opening_date: "2026-01-01",
    ...fields,
  };
}

const apple = { id: "h1", symbol: "AAPL", name: "Apple", quantity: 2, price: 10_000, cost_basis: 15_000, asset_class: "stock" };
const bonds = { id: "h2", symbol: "BND", name: "Bonds", quantity: 3.5, price: 1_000, asset_class: "bond" };
const bitcoin = { id: "h3", symbol: "BTC", name: "Bitcoin", quantity: 0.5, price: 6_000_000, cost_basis: 2_000_000, asset_class: "crypto" };

const accounts = [
  account("acc_inv", "investment", 1_000, { holdings: [apple, bonds] }),
  account("acc_crypto", "crypto", 0, { holdings: [bitcoin] }),
  account("acc_closed", "investment", 0, { holdings: [apple], closed_at: 5 }),
  account("acc_chk", "checking", 500_000),
];

test("a holding is worth quantity times price, and its gain needs a cost basis", () => {
  assert.equal(holdingValue(apple), 20_000);
  assert.equal(holdingGain(apple), 5_000);
  assert.equal(holdingValue(bonds), 3_500);
  assert.equal(holdingGain(bonds), null, "unknown is not zero");
});

test("the household's holdings are every open account's positions, most valuable first", () => {
  const rows = householdHoldings(accounts);
  assert.deepEqual(
    rows.map((row) => [row.account.id, row.holding.id, row.value, row.gain]),
    [
      ["acc_crypto", "h3", 3_000_000, 1_000_000],
      ["acc_inv", "h1", 20_000, 5_000],
      ["acc_inv", "h2", 3_500, null],
    ],
  );
  const allocation = allocationByClass(rows);
  assert.deepEqual(
    allocation.map((slice) => [slice.asset_class, slice.value]),
    [
      ["crypto", 3_000_000],
      ["stock", 20_000],
      ["bond", 3_500],
    ],
  );
  const shares = allocation.reduce((total, slice) => total + slice.share, 0);
  assert.ok(Math.abs(shares - 1) < 1e-9, "shares add up to one");
  assert.deepEqual(allocationByClass([]), []);
});

test("the investments total is the balance of every open investment and crypto account", () => {
  const balances = accountBalances(accounts, []);
  assert.deepEqual(investmentsTotal(accounts, balances), [{ currency: "USD", total: 3_024_500 }]);
});
