import assert from "node:assert/strict";
import { test } from "node:test";

import {
  accountBalanceHistory,
  historyCutoff,
  historyRange,
  netWorthChange,
  netWorthHistoryInRange,
} from "../dist/src/selectors/history.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function snapshot(id, date, netWorth, fields = {}) {
  return {
    ...base,
    id,
    date,
    assets: netWorth,
    liabilities: 0,
    net_worth: netWorth,
    currency: "USD",
    ...fields,
  };
}

const snapshots = [
  snapshot("s4", "2026-08-15", 150, {
    balances: [
      { account_id: "acc_a", balance: 20 },
      { account_id: "acc_b", balance: 6 },
    ],
  }),
  snapshot("s1", "2026-07-01", 100, { balances: [{ account_id: "acc_a", balance: 10 }] }),
  // Written before the nightly job recorded balances.
  snapshot("s2", "2026-07-15", 120),
  snapshot("s3", "2026-08-01", 130, { balances: [{ account_id: "acc_b", balance: 5 }] }),
  snapshot("s5", "2026-08-15", 999, { currency: "EUR" }),
];

test("an account's history skips the nights that recorded nothing about it", () => {
  assert.deepEqual(accountBalanceHistory(snapshots, "acc_a"), [
    { date: "2026-07-01", balance: 10 },
    { date: "2026-08-15", balance: 20 },
  ]);
  assert.deepEqual(
    accountBalanceHistory(snapshots, "acc_a", { start: "2026-08-01", end: "2026-08-31" }),
    [{ date: "2026-08-15", balance: 20 }],
  );
  assert.deepEqual(accountBalanceHistory(snapshots, "acc_missing"), []);
});

test("net worth over a range is scaled to the range shown", () => {
  const august = netWorthHistoryInRange(snapshots, "USD", {
    start: "2026-08-01",
    end: "2026-08-31",
  });
  assert.deepEqual(
    august.points.map((point) => [point.date, point.netWorth, point.position]),
    [
      ["2026-08-01", 130, 0],
      ["2026-08-15", 150, 1],
    ],
  );
  assert.equal(august.change, 20);
  const all = netWorthHistoryInRange(snapshots, "USD", null);
  assert.equal(all.points.length, 4);
  assert.equal(all.change, 50);
  assert.equal(netWorthHistoryInRange(snapshots, "GBP", null), null);
});

test("history ranges are calendar months back from today", () => {
  assert.equal(historyCutoff("1M", "2026-08-31"), "2026-07-31");
  assert.equal(historyCutoff("3M", "2026-08-31"), "2026-05-31");
  assert.equal(historyCutoff("6M", "2026-08-31"), "2026-02-28");
  assert.equal(historyCutoff("1Y", "2026-08-31"), "2025-08-31");
  assert.equal(historyCutoff("ALL", "2026-08-31"), null);
  assert.deepEqual(historyRange("1M", "2026-08-31"), { start: "2026-07-31", end: "2026-08-31" });
  assert.equal(historyRange("ALL", "2026-08-31"), null);
});

test("the change over so many days compares the latest snapshot with the one at the cutoff", () => {
  assert.equal(netWorthChange(snapshots, "USD", 30, "2026-08-20"), 30, "s4 against s2");
  assert.equal(netWorthChange(snapshots, "USD", 7, "2026-08-20"), 20, "s4 against s3");
  // A household younger than the window is measured from its first night.
  assert.equal(netWorthChange(snapshots, "USD", 365, "2026-08-20"), 50);
  assert.equal(netWorthChange(snapshots, "USD", 30, "2026-06-01"), null, "nothing yet");
  assert.equal(netWorthChange([], "USD", 30, "2026-08-20"), null);
});
