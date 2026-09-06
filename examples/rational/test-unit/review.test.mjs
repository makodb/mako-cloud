import assert from "node:assert/strict";
import { test } from "node:test";

import { needsReview, reviewCount, reviewQueue } from "../dist/src/selectors/review.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function transaction(id, date, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_a",
    date,
    amount: -100,
    currency: "USD",
    description: id,
    tags: [],
    splits: [],
    ...fields,
  };
}

test("only what arrived from outside and was not looked at needs review", () => {
  assert.equal(needsReview(transaction("typed", "2026-08-01")), false, "typed by hand");
  assert.equal(needsReview(transaction("synced", "2026-08-01", { external_id: "e1" })), true);
  assert.equal(
    needsReview(transaction("imported", "2026-08-01", { import_batch_id: "b1" })),
    true,
  );
  assert.equal(
    needsReview(transaction("seen", "2026-08-01", { external_id: "e1", reviewed: true })),
    false,
  );
  assert.equal(needsReview(transaction("blank", "2026-08-01", { external_id: "" })), false);
});

test("the queue is newest first and the count agrees with it", () => {
  const transactions = [
    transaction("a", "2026-08-01", { external_id: "e1" }),
    transaction("b", "2026-08-03", { import_batch_id: "b1" }),
    transaction("c", "2026-08-02", { external_id: "e2", reviewed: true }),
    transaction("d", "2026-08-04"),
  ];
  assert.deepEqual(
    reviewQueue(transactions).map((entry) => entry.id),
    ["b", "a"],
  );
  assert.equal(reviewCount(transactions), 2);
});
