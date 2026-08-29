import assert from "node:assert/strict";
import { test } from "node:test";

import {
  accountBalance,
  accountBalances,
  netWorthByCurrency,
} from "../dist/src/selectors/balances.js";
import { amountToText, formatMinorUnits, parseAmount } from "../dist/src/selectors/money.js";
import { validateSplits } from "../dist/src/selectors/splits.js";
import {
  availableMonths,
  filterTransactions,
  isIsoDate,
  monthKey,
  normalizeDescription,
  sortTransactions,
} from "../dist/src/selectors/transactions.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };
const account = (id, type, opening, currency = "USD") => ({
  ...base,
  id,
  name: id,
  type,
  currency,
  opening_balance: opening,
  opening_date: "2026-01-01",
});
const transaction = (id, accountId, date, amount, extra = {}) => ({
  ...base,
  id,
  account_id: accountId,
  date,
  amount,
  currency: "USD",
  description: id,
  tags: [],
  splits: [],
  ...extra,
});

test("an account balance is the opening balance plus its transactions", () => {
  const checking = account("acc_checking", "checking", 10_000);
  const transactions = [
    transaction("t1", "acc_checking", "2026-08-01", -2_500),
    transaction("t2", "acc_checking", "2026-08-02", 50_000),
    transaction("t3", "acc_other", "2026-08-02", -99_999),
  ];
  assert.equal(accountBalance(checking, transactions), 57_500);
  const balances = accountBalances([checking], transactions);
  assert.equal(balances.get("acc_checking"), 57_500);
  assert.equal(balances.has("acc_other"), false, "unknown accounts are not invented");
});

test("net worth subtracts liabilities per currency and ignores closed accounts", () => {
  const accounts = [
    account("acc_checking", "checking", 100_000),
    account("acc_card", "credit", -25_000),
    account("acc_loan", "loan", -300_000),
    { ...account("acc_closed", "savings", 999_999), closed_at: 5 },
    account("acc_eur", "savings", 4_000, "EUR"),
  ];
  const balances = accountBalances(accounts, [transaction("t1", "acc_card", "2026-08-01", -5_000)]);
  const totals = netWorthByCurrency(accounts, balances);
  assert.deepEqual(totals, [
    { currency: "EUR", assets: 4_000, liabilities: 0, netWorth: 4_000 },
    { currency: "USD", assets: 100_000, liabilities: 330_000, netWorth: -230_000 },
  ]);
});

test("splits must add up exactly and the difference is reported", () => {
  assert.deepEqual(validateSplits(-1_000, []), { ok: true, sum: 0 });
  assert.deepEqual(
    validateSplits(-1_000, [
      { id: "a", amount: -600 },
      { id: "b", amount: -400 },
    ]),
    { ok: true, sum: -1_000 },
  );
  const short = validateSplits(-1_000, [
    { id: "a", amount: -600 },
    { id: "b", amount: -300 },
  ]);
  assert.equal(short.ok, false);
  assert.equal(short.difference, -100, "the splits are 100 short of the outflow");
  assert.equal(short.reason, "sum_mismatch");
  const invalid = validateSplits(-1_000, [{ id: "a", amount: Number.NaN }]);
  assert.equal(invalid.ok, false);
  assert.equal(invalid.reason, "invalid_amount");
});

test("transactions filter by month and account and sort newest first deterministically", () => {
  const transactions = [
    transaction("t1", "acc_a", "2026-07-31", -1),
    transaction("t2", "acc_a", "2026-08-01", -2),
    transaction("t3", "acc_b", "2026-08-15", -3),
    transaction("t4", "acc_a", "2026-08-15", -4, { updated_at: 9 }),
    transaction("t5", "acc_a", "2026-08-15", -5, { updated_at: 9 }),
  ];
  assert.deepEqual(availableMonths(transactions), ["2026-08", "2026-07"]);
  assert.equal(monthKey("2026-08-15"), "2026-08");
  assert.deepEqual(
    filterTransactions(transactions, { month: "2026-08" }).map((entry) => entry.id),
    ["t5", "t4", "t3", "t2"],
  );
  assert.deepEqual(
    filterTransactions(transactions, { month: "2026-08", accountId: "acc_a" }).map((entry) => entry.id),
    ["t5", "t4", "t2"],
  );
  assert.deepEqual(
    sortTransactions(transactions).map((entry) => entry.id),
    ["t5", "t4", "t3", "t2", "t1"],
  );
  assert.equal(isIsoDate("2026-02-30"), false);
  assert.equal(isIsoDate("2026-02-28"), true);
});

test("amounts round-trip through minor units", () => {
  assert.equal(parseAmount("12.34", "USD"), 1_234);
  assert.equal(parseAmount("-0.5", "USD"), -50);
  assert.equal(parseAmount("1,234", "USD"), 123_400);
  assert.equal(parseAmount("1500", "JPY"), 1_500);
  assert.equal(parseAmount("1.234", "KWD"), 1_234);
  assert.throws(() => parseAmount("1.234", "USD"), RangeError);
  assert.throws(() => parseAmount("abc", "USD"), RangeError);
  assert.throws(() => parseAmount("", "USD"), RangeError);
  assert.equal(amountToText(-1_234, "USD"), "-12.34");
  assert.equal(amountToText(5, "USD"), "0.05");
  assert.equal(amountToText(1_500, "JPY"), "1500");
  assert.equal(formatMinorUnits(-1_234, "USD"), "-$12.34");
  assert.equal(formatMinorUnits(1_500, "JPY"), "¥1,500");
});

test("descriptions normalize for matching", () => {
  assert.equal(normalizeDescription("ACME Corp payroll #4412"), "acme corp payroll");
  assert.equal(normalizeDescription("  Blue   Bottle*Coffee 07/12 "), "blue bottle coffee");
});
