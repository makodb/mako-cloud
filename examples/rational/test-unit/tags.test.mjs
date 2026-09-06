import assert from "node:assert/strict";
import { test } from "node:test";

import { selectTagUsage, tagUsage } from "../dist/src/selectors/tags.js";

/**
 * The tags page says how many transactions carry each tag. The count is a
 * count of transactions, not of tag occurrences, and it counts everything --
 * a transfer leg or a hidden reimbursement tagged "shared" is still tagged.
 */

const base = { household_id: "hh_test", created_at: 1, updated_at: 1, currency: "USD", splits: [] };

function transaction(id, tags, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_a",
    date: "2026-08-10",
    amount: -1_000,
    description: id,
    tags,
    ...fields,
  };
}

test("each tag counts the transactions that carry it, hidden and transfer legs included", () => {
  const usage = tagUsage([
    transaction("t1", ["tag_shared"]),
    transaction("t2", ["tag_shared", "tag_trip"]),
    transaction("t3", ["tag_trip"], { hidden: true }),
    transaction("t4", ["tag_shared"], { transfer_id: "trf_1" }),
    transaction("t5", []),
  ]);
  assert.equal(usage.get("tag_shared"), 3);
  assert.equal(usage.get("tag_trip"), 2);
  assert.equal(usage.get("tag_unused"), undefined);
});

test("a tag repeated on one transaction counts that transaction once", () => {
  const usage = tagUsage([transaction("t1", ["tag_shared", "tag_shared"])]);
  assert.equal(usage.get("tag_shared"), 1);
});

test("the memoized selector returns the same map for the same array", () => {
  const transactions = [transaction("t1", ["tag_shared"])];
  const first = selectTagUsage(transactions);
  assert.equal(selectTagUsage(transactions), first);
  assert.notEqual(selectTagUsage([...transactions]), first);
});
