import assert from "node:assert/strict";
import { test } from "node:test";

import { suggestTransferPairs } from "../functions/shared/transfers.ts";

/**
 * A transfer suggestion hides money from the reports once it is confirmed,
 * so the tests are mostly about what is *not* suggested, and about every
 * transaction being in at most one pair.
 */

function transaction(id, accountId, amount, date, overrides = {}) {
  return { id, account_id: accountId, amount, currency: "USD", date, ...overrides };
}

test("opposite amounts on two accounts within three days are a pair", () => {
  const suggestions = suggestTransferPairs([
    transaction("o1", "acc_1", -50_000, "2026-08-01"),
    transaction("i1", "acc_2", 50_000, "2026-08-03"),
  ]);
  assert.deepEqual(suggestions, [{ outflowId: "o1", inflowId: "i1", amount: 50_000, days: 2 }]);
});

test("the same account, another currency, another amount, or too many days apart is not", () => {
  const out = transaction("o1", "acc_1", -50_000, "2026-08-01");
  assert.deepEqual(suggestTransferPairs([out, transaction("i1", "acc_1", 50_000, "2026-08-02")]), []);
  assert.deepEqual(
    suggestTransferPairs([out, transaction("i1", "acc_2", 50_000, "2026-08-02", { currency: "EUR" })]),
    [],
  );
  assert.deepEqual(suggestTransferPairs([out, transaction("i1", "acc_2", 49_999, "2026-08-02")]), []);
  const far = transaction("i1", "acc_2", 50_000, "2026-08-05");
  assert.deepEqual(suggestTransferPairs([out, far]), []);
  // The window is the caller's to widen.
  assert.equal(suggestTransferPairs([out, far], { maxDays: 5 }).length, 1);
});

test("each transaction is in at most one suggestion, the closest pair taken first", () => {
  const suggestions = suggestTransferPairs([
    transaction("o1", "acc_1", -50_000, "2026-08-01"),
    transaction("o2", "acc_1", -50_000, "2026-08-03"),
    transaction("i1", "acc_2", 50_000, "2026-08-03"),
    transaction("i2", "acc_2", 50_000, "2026-08-04"),
  ]);
  // o2 and i1 are on the same day, which leaves o1 with i2 three days off.
  assert.deepEqual(suggestions, [
    { outflowId: "o2", inflowId: "i1", amount: 50_000, days: 0 },
    { outflowId: "o1", inflowId: "i2", amount: 50_000, days: 3 },
  ]);
});

test("a paired, hidden, or balance-update transaction is not a candidate", () => {
  const inflow = transaction("i1", "acc_2", 50_000, "2026-08-02");
  assert.deepEqual(
    suggestTransferPairs([transaction("o1", "acc_1", -50_000, "2026-08-01", { transfer_id: "tr_1" }), inflow]),
    [],
  );
  assert.deepEqual(
    suggestTransferPairs([transaction("o1", "acc_1", -50_000, "2026-08-01", { hidden: true }), inflow]),
    [],
  );
  assert.deepEqual(
    suggestTransferPairs([transaction("o1", "acc_1", -50_000, "2026-08-01", { adjustment: true }), inflow]),
    [],
  );
});
