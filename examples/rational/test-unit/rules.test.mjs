import assert from "node:assert/strict";
import { test } from "node:test";

import {
  applyRules,
  countMatches,
  pendingRecategorization,
  ruleMatches,
  ruleStatesSomething,
  sortRules,
} from "../dist/src/selectors/rules.js";
import { moveRule, reprioritize, wouldChange } from "../functions/shared/rules.ts";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function rule(fields) {
  return {
    ...base,
    id: "rule_1",
    name: "A rule",
    match: {},
    add_tags: [],
    priority: 10,
    match_count: 0,
    enabled: true,
    ...fields,
  };
}

function transaction(fields) {
  return {
    ...base,
    id: "txn_1",
    account_id: "acc_1",
    date: "2026-08-02",
    currency: "USD",
    tags: [],
    splits: [],
    ...fields,
  };
}

test("a rule that states nothing matches nothing", () => {
  const empty = rule({});
  assert.equal(ruleStatesSomething(empty), false);
  assert.equal(
    ruleMatches(empty, { description: "anything", amount: -1, account_id: "acc_1" }),
    false,
  );
  // Otherwise an empty form would recategorize the whole household.
});

test("every stated condition has to hold, and a disabled rule holds none", () => {
  const groceries = rule({
    match: { description_contains: "market", amount_min: -50_00, amount_max: -1_00 },
  });
  assert.ok(ruleMatches(groceries, { description: "CORNER MARKET", amount: -12_34, account_id: "a" }));
  assert.equal(
    ruleMatches(groceries, { description: "CORNER MARKET", amount: -80_00, account_id: "a" }),
    false,
    "outside the range",
  );
  assert.equal(
    ruleMatches(groceries, { description: "PHARMACY", amount: -12_34, account_id: "a" }),
    false,
    "the description does not contain it",
  );
  assert.equal(
    ruleMatches({ ...groceries, enabled: false }, {
      description: "CORNER MARKET",
      amount: -12_34,
      account_id: "a",
    }),
    false,
  );
});

test("an account condition scopes a rule to one account", () => {
  const card = rule({ match: { description_contains: "fee", account_id: "acc_card" } });
  assert.ok(ruleMatches(card, { description: "ANNUAL FEE", amount: -95_00, account_id: "acc_card" }));
  assert.equal(
    ruleMatches(card, { description: "ANNUAL FEE", amount: -95_00, account_id: "acc_other" }),
    false,
  );
});

test("the first rule by priority wins, and ties are broken the same way everywhere", () => {
  const rules = [
    rule({ id: "rule_b", priority: 5, match: { description_contains: "market" }, set_category_id: "cat_general" }),
    rule({ id: "rule_a", priority: 5, match: { description_contains: "market" }, set_category_id: "cat_food" }),
    rule({ id: "rule_c", priority: 1, match: { description_contains: "corner" }, set_category_id: "cat_corner", add_tags: ["tag_local"] }),
  ];
  assert.deepEqual(
    sortRules(rules).map((entry) => entry.id),
    ["rule_c", "rule_a", "rule_b"],
  );
  const outcome = applyRules(rules, {
    description: "CORNER MARKET",
    amount: -12_34,
    account_id: "acc_1",
  });
  assert.equal(outcome?.rule.id, "rule_c");
  assert.equal(outcome?.categoryId, "cat_corner");
  assert.deepEqual(outcome?.tags, ["tag_local"]);
});

test("a rule reports what it would touch before it touches anything", () => {
  const groceries = rule({
    match: { description_contains: "market" },
    set_category_id: "cat_food",
  });
  const transactions = [
    transaction({ id: "t1", description: "CORNER MARKET", amount: -12_34 }),
    transaction({ id: "t2", description: "CORNER MARKET", amount: -5_00, category_id: "cat_food" }),
    transaction({ id: "t3", description: "PHARMACY", amount: -7_00 }),
  ];
  assert.equal(countMatches(groceries, transactions), 2);
  // The one already filed there is not rewritten: it would push a document
  // for no change.
  assert.deepEqual(
    pendingRecategorization(groceries, transactions).map((entry) => entry.id),
    ["t1"],
  );
});

test("exact text is compared against the normalized description", () => {
  const netflix = rule({ match: { description_equals: " Netflix Com " } });
  assert.ok(
    ruleMatches(netflix, {
      description: "NETFLIX.COM 4471",
      normalized_description: "netflix com",
      amount: -1_299,
      account_id: "a",
    }),
  );
  assert.equal(
    ruleMatches(netflix, {
      description: "NETFLIX.COM 4471",
      normalized_description: "netflix com premium",
      amount: -1_299,
      account_id: "a",
    }),
    false,
    "equals is not contains",
  );
  // Without a stored normalized description the raw text is lower-cased and
  // compared as it is: this module cannot normalize on its own.
  assert.ok(ruleMatches(netflix, { description: "netflix com", amount: -1_299, account_id: "a" }));
  assert.equal(
    ruleMatches(netflix, { description: "NETFLIX.COM 4471", amount: -1_299, account_id: "a" }),
    false,
  );
});

test("a merchant, a category, and a direction are each a condition on their own", () => {
  const byMerchant = rule({ match: { merchant_id: "m_1" } });
  assert.ok(ruleStatesSomething(byMerchant));
  assert.ok(ruleMatches(byMerchant, { description: "x", amount: -1, account_id: "a", merchant_id: "m_1" }));
  assert.equal(ruleMatches(byMerchant, { description: "x", amount: -1, account_id: "a", merchant_id: "m_2" }), false);
  assert.equal(ruleMatches(byMerchant, { description: "x", amount: -1, account_id: "a" }), false);

  const byCategory = rule({ match: { category_id: "cat_food" } });
  assert.ok(ruleMatches(byCategory, { description: "x", amount: -1, account_id: "a", category_id: "cat_food" }));
  assert.equal(ruleMatches(byCategory, { description: "x", amount: -1, account_id: "a" }), false);

  const expense = rule({ match: { direction: "expense" } });
  const income = rule({ match: { direction: "income" } });
  assert.ok(ruleStatesSomething(expense));
  assert.ok(ruleMatches(expense, { description: "x", amount: -1, account_id: "a" }));
  assert.equal(ruleMatches(expense, { description: "x", amount: 1, account_id: "a" }), false);
  assert.ok(ruleMatches(income, { description: "x", amount: 1, account_id: "a" }));
  assert.equal(ruleMatches(income, { description: "x", amount: -1, account_id: "a" }), false);
  // Zero moved nothing in either direction.
  assert.equal(ruleMatches(expense, { description: "x", amount: 0, account_id: "a" }), false);
  assert.equal(ruleMatches(income, { description: "x", amount: 0, account_id: "a" }), false);
});

test("every action a rule states is in its outcome, and none it does not", () => {
  const fee = rule({
    match: { description_contains: "fee" },
    set_merchant_id: "m_bank",
    hide: true,
    mark_reviewed: true,
    add_tags: ["tag_fees"],
  });
  const outcome = applyRules([fee], { description: "ANNUAL FEE", amount: -95_00, account_id: "a" });
  assert.equal(outcome?.categoryId, undefined);
  assert.equal(outcome?.merchantId, "m_bank");
  assert.equal(outcome?.hide, true);
  assert.equal(outcome?.markReviewed, true);
  assert.deepEqual(outcome?.tags, ["tag_fees"]);

  const plain = applyRules([rule({ match: { description_contains: "fee" }, set_category_id: "cat_fees" })], {
    description: "ANNUAL FEE",
    amount: -95_00,
    account_id: "a",
  });
  assert.equal("hide" in plain, false, "an action not stated is absent, not false");
  assert.equal("markReviewed" in plain, false);
  assert.equal("merchantId" in plain, false);
});

test("a rule is pending for a transaction until every one of its actions has been applied", () => {
  const fee = rule({
    match: { description_contains: "fee" },
    set_merchant_id: "m_bank",
    hide: true,
    mark_reviewed: true,
  });
  const transactions = [
    transaction({ id: "t1", description: "ANNUAL FEE", amount: -95_00 }),
    transaction({ id: "t2", description: "ANNUAL FEE", amount: -95_00, merchant_id: "m_bank", hidden: true, reviewed: true }),
    transaction({ id: "t3", description: "ANNUAL FEE", amount: -95_00, merchant_id: "m_bank", hidden: true }),
    transaction({ id: "t4", description: "PHARMACY", amount: -95_00 }),
  ];
  assert.equal(wouldChange(fee, transactions[1]), false);
  assert.equal(wouldChange(fee, transactions[2]), true, "still unreviewed");
  assert.deepEqual(
    pendingRecategorization(fee, transactions).map((entry) => entry.id),
    ["t1", "t3"],
  );
});

test("moving a rule swaps it with its neighbour in the sorted order, and reordering rewrites priorities as tens", () => {
  const rules = [
    rule({ id: "rule_b", priority: 5 }),
    rule({ id: "rule_a", priority: 5 }),
    rule({ id: "rule_c", priority: 1 }),
  ];
  assert.deepEqual(
    moveRule(rules, "rule_b", "up").map((entry) => entry.id),
    ["rule_c", "rule_b", "rule_a"],
  );
  assert.deepEqual(reprioritize(moveRule(rules, "rule_b", "up")), [
    { id: "rule_c", priority: 10 },
    { id: "rule_b", priority: 20 },
    { id: "rule_a", priority: 30 },
  ]);
  // At an edge, or not in the list, nothing moves -- but the order is still the sorted one.
  assert.deepEqual(moveRule(rules, "rule_c", "up").map((entry) => entry.id), ["rule_c", "rule_a", "rule_b"]);
  assert.deepEqual(moveRule(rules, "rule_b", "down").map((entry) => entry.id), ["rule_c", "rule_a", "rule_b"]);
  assert.deepEqual(moveRule(rules, "rule_x", "down").map((entry) => entry.id), ["rule_c", "rule_a", "rule_b"]);
  assert.deepEqual(reprioritize([]), []);
});
