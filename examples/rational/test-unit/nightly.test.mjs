import assert from "node:assert/strict";
import { test } from "node:test";

import {
  DUPLICATE_NOTE,
  duplicates,
  filings,
  fingerprint,
  netWorth,
  snapshotId,
} from "../functions/shared/nightly.ts";
import {
  detectionId,
  detectRecurrences,
} from "../functions/shared/recurrences.ts";

/**
 * The nightly job runs while nobody is watching, so what it must never do
 * matters more than what it does: it must not overrule a filing the person
 * made, must not delete anything, and must leave the same household after two
 * runs as after one. Every test here is about one of those three.
 *
 * The decisions are pure functions over the documents the job read, so they
 * are tested without a runtime, a service credential, or a network.
 */

const NOW = 1_760_000_000_000;

function document(body, { deleted = false, revision = "rev-1" } = {}) {
  return { body, revision, _deleted: deleted, schemaVersion: 1 };
}

function transaction(overrides, options) {
  return document({
    id: "txn-1",
    household_id: "hh-1",
    created_at: NOW,
    updated_at: NOW,
    account_id: "acct-1",
    date: "2026-08-01",
    amount: -1250,
    currency: "USD",
    description: "BLUE BOTTLE #221",
    tags: [],
    splits: [],
    ...overrides,
  }, options);
}

function rule(overrides, options) {
  return document({
    id: "rule-1",
    household_id: "hh-1",
    created_at: NOW,
    updated_at: NOW,
    name: "Coffee",
    match: { description_contains: "blue bottle" },
    set_category_id: "cat-coffee",
    add_tags: [],
    priority: 10,
    enabled: true,
    ...overrides,
  }, options);
}

test("an uncategorized transaction is filed, and the rule that filed it is recorded", () => {
  const decided = filings([transaction({})], [rule({})], NOW);
  assert.equal(decided.length, 1);
  assert.equal(decided[0].body.category_id, "cat-coffee");
  assert.equal(decided[0].body.rule_id, "rule-1");
  assert.equal(decided[0].revision, "rev-1", "the write is guarded by the revision it read");
});

test("a category the person chose is never overruled", () => {
  const filed = transaction({ category_id: "cat-groceries" });
  assert.deepEqual(filings([filed], [rule({})], NOW), []);
});

test("a split transaction is left alone: its categories are on the splits", () => {
  const split = transaction({ splits: [{ id: "s1", category_id: "cat-a", amount: -1250 }] });
  assert.deepEqual(filings([split], [rule({})], NOW), []);
});

test("a deleted transaction and a deleted rule are both ignored", () => {
  assert.deepEqual(filings([transaction({}, { deleted: true })], [rule({})], NOW), []);
  assert.deepEqual(filings([transaction({})], [rule({}, { deleted: true })], NOW), []);
});

test("a rule's tags are added to the transaction's own, without duplicating one", () => {
  const tagged = transaction({ tags: ["tag-cafe"] });
  const withTags = rule({ add_tags: ["tag-cafe", "tag-treat"] });
  const decided = filings([tagged], [withTags], NOW);
  assert.deepEqual(decided[0].body.tags, ["tag-cafe", "tag-treat"]);
});

test("filing is idempotent: a second night finds nothing left to file", () => {
  const first = filings([transaction({})], [rule({})], NOW);
  const after = document(first[0].body, { revision: "rev-2" });
  assert.deepEqual(filings([after], [rule({})], NOW), []);
});

test("a synced transaction that repeats a manual one is marked, and the manual one is not", () => {
  const manual = transaction({ id: "txn-manual" });
  const synced = transaction({ id: "txn-synced", external_id: "inst-99" });
  const marked = duplicates([manual, synced], NOW);
  assert.equal(marked.length, 1);
  assert.equal(marked[0].documentId, "txn-synced");
  assert.match(String(marked[0].body.notes), /txn-manual/u);
  assert.ok(String(marked[0].body.notes).startsWith(DUPLICATE_NOTE));
});

test("marking a duplicate never deletes it", () => {
  const marked = duplicates(
    [transaction({ id: "txn-manual" }), transaction({ id: "txn-synced", external_id: "i-1" })],
    NOW,
  );
  assert.equal(marked[0].body._deleted, undefined);
  assert.equal(marked[0].body.id, "txn-synced", "the document is rewritten, not removed");
});

test("a duplicate already marked is not marked again", () => {
  const manual = transaction({ id: "txn-manual" });
  const synced = transaction({ id: "txn-synced", external_id: "i-1" });
  const [first] = duplicates([manual, synced], NOW);
  const again = duplicates([manual, document(first.body, { revision: "rev-2" })], NOW);
  assert.deepEqual(again, []);
});

test("two synced transactions that merely look alike are not duplicates of each other", () => {
  const one = transaction({ id: "txn-a", external_id: "i-1" });
  const two = transaction({ id: "txn-b", external_id: "i-2" });
  assert.deepEqual(duplicates([one, two], NOW), []);
});

test("the fingerprint ignores the digits a bank writes into a description", () => {
  const left = fingerprint({
    account_id: "acct-1",
    date: "2026-08-01",
    amount: -1250,
    description: "BLUE BOTTLE #221",
  });
  const right = fingerprint({
    account_id: "acct-1",
    date: "2026-08-01",
    amount: -1250,
    description: "Blue Bottle 4471",
  });
  assert.equal(left, right);
});

test("net worth counts open accounts of the household's currency, liabilities as owed", () => {
  const accounts = [
    document({ id: "acct-1", type: "checking", currency: "USD", opening_balance: 100_000 }),
    document({ id: "acct-2", type: "credit_card", currency: "USD", opening_balance: 0 }),
    document({ id: "acct-3", type: "checking", currency: "EUR", opening_balance: 500_000 }),
    document({
      id: "acct-4",
      type: "checking",
      currency: "USD",
      opening_balance: 900_000,
      closed_at: NOW,
    }),
  ];
  const transactions = [
    transaction({ id: "t1", account_id: "acct-1", amount: -25_000 }),
    transaction({ id: "t2", account_id: "acct-2", amount: -40_000 }),
    transaction({ id: "t3", account_id: "acct-3", amount: -1_000 }),
  ];
  assert.deepEqual(netWorth("USD", accounts, transactions), {
    assets: 75_000,
    liabilities: 40_000,
  });
});

test("a closed account leaves net worth, and a foreign-currency one is never summed in", () => {
  const accounts = [
    document({ id: "acct-1", type: "checking", currency: "USD", opening_balance: 10_000 }),
    document({ id: "acct-2", type: "savings", currency: "EUR", opening_balance: 99_000 }),
  ];
  assert.deepEqual(netWorth("USD", accounts, []), { assets: 10_000, liabilities: 0 });
});

test("a snapshot id is one per household per day, and carries no colon", () => {
  const id = snapshotId("hh-1", "2026-08-30");
  assert.equal(id, snapshotId("hh-1", "2026-08-30"));
  assert.notEqual(id, snapshotId("hh-1", "2026-08-31"));
  assert.ok(!id.includes(":"), "ids a function writes avoid characters the path escapes");
});

test("a detection has one id whatever night finds it, and a known one is not proposed again", () => {
  const monthly = ["2026-05-02", "2026-06-01", "2026-07-02", "2026-08-01"].map((date) => ({
    account_id: "acct-1",
    description: `NETFLIX ${date.replaceAll("-", "")}`,
    amount: -1599,
    currency: "USD",
    date,
  }));
  const [detected] = detectRecurrences(monthly);
  assert.equal(detected.interval, "monthly");
  assert.equal(detectionId("hh-1", detected), detectionId("hh-1", detected));
  assert.notEqual(detectionId("hh-2", detected), detectionId("hh-1", detected));
  assert.ok(!detectionId("hh-1", detected).includes(":"));
  const known = [
    { account_id: "acct-1", normalized_description: detected.normalizedDescription },
  ];
  assert.deepEqual(detectRecurrences(monthly, known), [], "a dismissal stays dismissed");
});
