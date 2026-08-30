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
