import assert from "node:assert/strict";
import { test } from "node:test";

import { compareStates, resolveConflict, sameState } from "../dist/src/data/conflict.js";
import { generateDemoHousehold } from "../scripts/demo-data.mjs";

test("the newer updated_at wins and ties resolve identically from either side", () => {
  const older = { id: "t", updated_at: 100, description: "a", _deleted: false };
  const newer = { id: "t", updated_at: 200, description: "b", _deleted: false };
  assert.equal(resolveConflict(older, newer), newer);
  assert.equal(resolveConflict(newer, older), newer);

  const left = { id: "t", updated_at: 300, description: "left", _deleted: false };
  const right = { id: "t", updated_at: 300, description: "right", _deleted: false };
  const fromLeft = resolveConflict(left, right);
  const fromRight = resolveConflict(right, left);
  assert.deepEqual(fromLeft, fromRight, "both devices pick the same state");
  assert.equal(Math.sign(compareStates(left, right)), -Math.sign(compareStates(right, left)));
});

test("equality ignores replication metadata", () => {
  const a = { id: "t", updated_at: 1, x: 1, _deleted: false, _rev: "1-a", _meta: { lwt: 1 } };
  const b = { id: "t", updated_at: 1, x: 1, _deleted: false, _rev: "2-b" };
  assert.equal(sameState(a, b), true);
  assert.equal(sameState(a, { ...b, x: 2 }), false);
});

test("the demo household is deterministic", () => {
  const first = generateDemoHousehold({ householdId: "hh_demo" });
  const second = generateDemoHousehold({ householdId: "hh_demo" });
  assert.deepEqual(first, second);
  assert.equal(first.transactions.length, 200);
  assert.equal(first.accounts.length, 5);
  const split = first.transactions.find((transaction) => transaction.splits.length > 0);
  assert.ok(split);
  assert.equal(
    split.splits.reduce((sum, entry) => sum + entry.amount, 0),
    split.amount,
    "seeded splits add up",
  );
  for (const transaction of first.transactions) {
    assert.ok(Number.isSafeInteger(transaction.amount));
    assert.ok(/^\d{4}-\d{2}-\d{2}$/u.test(transaction.date));
    assert.ok(first.accounts.some((account) => account.id === transaction.account_id));
  }
});
