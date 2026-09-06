import assert from "node:assert/strict";
import { test } from "node:test";

import {
  billSubjects,
  DUPLICATE_NOTE,
  duplicates,
  filings,
  fingerprint,
  goalSubjects,
  netWorth,
  paidBills,
  snapshotBalances,
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
    document({ id: "acct-2", type: "credit", currency: "USD", opening_balance: 0 }),
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

test("net worth values holdings and leaves out an account hidden from it", () => {
  const accounts = [
    document({ id: "acct-1", type: "checking", currency: "USD", opening_balance: 100_000 }),
    document({
      id: "acct-inv",
      type: "investment",
      currency: "USD",
      opening_balance: 0,
      holdings: [{ id: "h1", symbol: "VTI", name: "Total market", quantity: 2.5, price: 10_000, asset_class: "etf" }],
    }),
    document({
      id: "acct-hidden",
      type: "checking",
      currency: "USD",
      opening_balance: 50_000,
      hide_from_net_worth: true,
    }),
  ];
  assert.deepEqual(netWorth("USD", accounts, []), { assets: 125_000, liabilities: 0 });
});

test("the snapshot carries every open account's balance, hidden ones too, closed ones not", () => {
  const accounts = [
    document({ id: "acct-b", type: "checking", currency: "USD", opening_balance: 100_000 }),
    document({ id: "acct-a", type: "checking", currency: "USD", opening_balance: 5, hide_from_net_worth: true }),
    document({ id: "acct-closed", type: "checking", currency: "USD", opening_balance: 9, closed_at: NOW }),
    document({ id: "acct-gone", type: "checking", currency: "USD", opening_balance: 9 }, { deleted: true }),
  ];
  const transactions = [
    transaction({ id: "t1", account_id: "acct-b", amount: -25_000 }),
    transaction({ id: "t2", account_id: "acct-b", amount: -1_000, transfer_id: "tr-1" }),
  ];
  assert.deepEqual(snapshotBalances(accounts, transactions), [
    { account_id: "acct-a", balance: 5 },
    { account_id: "acct-b", balance: 74_000 },
  ]);
});

test("a rule's other actions apply to a categorized transaction, and the category is kept", () => {
  const filed = transaction({ category_id: "cat-groceries" });
  const hides = rule({ hide: true, set_merchant_id: "m-bb", add_tags: ["tag-coffee"] });
  const decided = filings([filed], [hides], NOW);
  assert.equal(decided.length, 1);
  assert.equal(decided[0].body.category_id, "cat-groceries", "the person's category stands");
  assert.equal(decided[0].body.hidden, true);
  assert.equal(decided[0].body.merchant_id, "m-bb");
  assert.deepEqual(decided[0].body.tags, ["tag-coffee"]);
  assert.equal(decided[0].body.rule_id, "rule-1");
  // The rule has touched it once; a member who un-hides it is not fought every night.
  const unhidden = document({ ...decided[0].body, hidden: false }, { revision: "rev-2" });
  assert.deepEqual(filings([unhidden], [hides], NOW), []);
});

test("a rule that marks reviewed does so overnight too, once", () => {
  const imported = transaction({ external_id: "inst-1", category_id: "cat-groceries" });
  const reviews = rule({ set_category_id: undefined, mark_reviewed: true });
  const decided = filings([imported], [reviews], NOW);
  assert.equal(decided.length, 1);
  assert.equal(decided[0].body.reviewed, true);
  assert.deepEqual(filings([document(decided[0].body, { revision: "rev-2" })], [reviews], NOW), []);
});

test("a transfer leg or a balance update is never filed; a hidden transaction still is", () => {
  assert.deepEqual(filings([transaction({ transfer_id: "tr-1" })], [rule({})], NOW), []);
  assert.deepEqual(filings([transaction({ adjustment: true })], [rule({})], NOW), []);
  const hidden = filings([transaction({ hidden: true })], [rule({})], NOW);
  assert.equal(hidden[0]?.body.category_id, "cat-coffee");
  assert.equal(hidden[0]?.body.hidden, true, "and stays hidden");
});

test("an exact-text rule matches the stored normalized description overnight", () => {
  const exact = rule({ match: { description_equals: "blue bottle" } });
  const stored = transaction({ normalized_description: "blue bottle" });
  assert.equal(filings([stored], [exact], NOW).length, 1);
  // Without one stored, the job normalizes the description itself.
  assert.equal(filings([transaction({})], [exact], NOW).length, 1);
});

function recurrence(overrides, options) {
  return document({
    id: "rec-rent",
    household_id: "hh-1",
    created_at: NOW,
    updated_at: NOW,
    account_id: "acct-1",
    normalized_description: "rent",
    interval: "monthly",
    expected_amount: -150_000,
    currency: "USD",
    next_date: "2026-09-01",
    status: "confirmed",
    matched_count: 3,
    ...overrides,
  }, options);
}

test("a paid bill is advanced from its payment; an unpaid or unconfirmed one is left", () => {
  const rent = transaction({ id: "t-rent", date: "2026-09-02", amount: -150_000, description: "RENT 0912" });
  const advanced = paidBills([recurrence({})], [rent], "2026-09-05", NOW);
  assert.equal(advanced.length, 1);
  assert.equal(advanced[0].documentId, "rec-rent");
  assert.equal(advanced[0].revision, "rev-1", "guarded by the revision it read");
  assert.equal(advanced[0].body.next_date, "2026-10-02");
  assert.equal(advanced[0].body.last_date, "2026-09-02");
  assert.equal(advanced[0].body.last_paid_transaction_id, "t-rent");
  assert.equal(advanced[0].body.matched_count, 4);
  assert.equal(advanced[0].body.updated_at, NOW);
  // Advanced once, the same payment is no longer near the next date.
  const next = document(advanced[0].body, { revision: "rev-2" });
  assert.deepEqual(paidBills([next], [rent], "2026-09-05", NOW), []);
  assert.deepEqual(paidBills([recurrence({})], [], "2026-09-05", NOW), []);
  assert.deepEqual(paidBills([recurrence({ status: "detected" })], [rent], "2026-09-05", NOW), []);
  assert.deepEqual(paidBills([recurrence({}, { deleted: true })], [rent], "2026-09-05", NOW), []);
});

test("bills and goals are shaped for the alert engine from the night's documents", () => {
  const bills = billSubjects(
    [
      recurrence({ name: "Rent", next_date: "2026-09-03" }),
      recurrence({ id: "rec-gym", normalized_description: "gym", next_date: "2026-09-02" }),
      recurrence({ id: "rec-maybe", status: "detected" }),
      recurrence({ id: "rec-paused", status: "paused" }),
    ],
    "2026-09-01",
  );
  assert.deepEqual(bills, [
    { recurrence_id: "rec-rent", name: "Rent", due_date: "2026-09-03", amount: -150_000, currency: "USD", days_away: 2 },
    { recurrence_id: "rec-gym", name: "gym", due_date: "2026-09-02", amount: -150_000, currency: "USD", days_away: 1 },
  ]);

  const goal = (overrides) =>
    document({
      id: "goal-1",
      household_id: "hh-1",
      created_at: NOW,
      updated_at: NOW,
      name: "Holiday",
      target_amount: 200_000,
      currency: "USD",
      status: "active",
      contributions: [],
      ...overrides,
    });
  const balances = new Map([["acct-loan", -50_000]]);
  const goals = goalSubjects(
    [
      goal({ contributions: [{ id: "c1", date: "2026-08-01", amount: 200_000 }] }),
      goal({ id: "goal-2", name: "Car loan", kind: "pay_down", target_amount: 0, account_ids: ["acct-loan"], starting_balance: -300_000 }),
      goal({ id: "goal-3", name: "Old", status: "archived" }),
    ],
    balances,
    "2026-09-01",
  );
  assert.deepEqual(goals, [
    { id: "goal-1", name: "Holiday", reached: true, target_amount: 200_000, currency: "USD" },
    { id: "goal-2", name: "Car loan", reached: false, target_amount: 0, currency: "USD" },
  ]);
  const cleared = goalSubjects(
    [goal({ id: "goal-2", kind: "pay_down", target_amount: 0, account_ids: ["acct-loan"] })],
    new Map([["acct-loan", 0]]),
    "2026-09-01",
  );
  assert.equal(cleared[0].reached, true, "a debt gone is a goal reached");
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
