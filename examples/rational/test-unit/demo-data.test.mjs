import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { generateDemoHousehold } from "../scripts/demo-data.mjs";

/**
 * The demo household is what every screen shows first, and the fake backend
 * serves it without a schema in the way -- so nothing but this test would
 * notice a stray field or a broken pairing until the seed script pushed it to
 * a real environment and the platform refused it.
 */
const model = JSON.parse(
  readFileSync(new URL("../mako/collections.json", import.meta.url), "utf8"),
);
const schemas = new Map(model.collections.map((entry) => [entry.id, entry.jsonSchema]));

/**
 * The subset of JSON Schema the model uses -- required, closed objects, enums,
 * integers, bounds, lengths, patterns, arrays -- checked by hand so the test
 * needs no validator dependency.
 */
function conformance(schema, value, path) {
  const problems = [];
  if (schema.type === "object") {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      return [`${path}: expected an object`];
    }
    for (const key of schema.required ?? []) {
      if (!(key in value)) problems.push(`${path}: missing ${key}`);
    }
    for (const [key, field] of Object.entries(value)) {
      const fieldSchema = schema.properties?.[key];
      if (fieldSchema === undefined) {
        if (schema.additionalProperties === false) problems.push(`${path}: stray field ${key}`);
        continue;
      }
      if (field === undefined) {
        problems.push(`${path}.${key}: undefined value`);
        continue;
      }
      problems.push(...conformance(fieldSchema, field, `${path}.${key}`));
    }
    return problems;
  }
  if (schema.type === "array") {
    if (!Array.isArray(value)) return [`${path}: expected an array`];
    if (schema.maxItems !== undefined && value.length > schema.maxItems) {
      problems.push(`${path}: more than ${schema.maxItems} items`);
    }
    value.forEach((item, index) => {
      problems.push(...conformance(schema.items, item, `${path}[${index}]`));
    });
    return problems;
  }
  if (schema.type === "integer" || schema.type === "number") {
    if (typeof value !== "number" || Number.isNaN(value)) return [`${path}: expected a number`];
    if (schema.type === "integer" && !Number.isInteger(value)) {
      problems.push(`${path}: expected an integer`);
    }
    if (schema.minimum !== undefined && value < schema.minimum) problems.push(`${path}: too small`);
    if (schema.maximum !== undefined && value > schema.maximum) problems.push(`${path}: too large`);
    return problems;
  }
  if (schema.type === "string") {
    if (typeof value !== "string") return [`${path}: expected a string`];
    if (schema.minLength !== undefined && value.length < schema.minLength) {
      problems.push(`${path}: too short`);
    }
    if (schema.maxLength !== undefined && value.length > schema.maxLength) {
      problems.push(`${path}: too long`);
    }
    if (schema.enum !== undefined && !schema.enum.includes(value)) {
      problems.push(`${path}: ${value} is not one of ${schema.enum.join(", ")}`);
    }
    if (schema.pattern !== undefined && !new RegExp(schema.pattern, "u").test(value)) {
      problems.push(`${path}: ${value} does not match ${schema.pattern}`);
    }
    return problems;
  }
  if (schema.type === "boolean") {
    return typeof value === "boolean" ? [] : [`${path}: expected a boolean`];
  }
  return [`${path}: unhandled schema type ${String(schema.type)}`];
}

const demo = generateDemoHousehold({ householdId: "hh_demo", transactionCount: 60 });
const collections = {
  households: [demo.household],
  accounts: demo.accounts,
  taxonomy: demo.taxonomy,
  transactions: demo.transactions,
  rules: demo.rules,
  budgets: demo.budgets,
  recurrences: demo.recurrences,
  goals: demo.goals,
  net_worth_snapshots: demo.net_worth_snapshots,
};

test("every demo document satisfies its collection's schema and ids are unique", () => {
  for (const [collectionId, documents] of Object.entries(collections)) {
    const schema = schemas.get(collectionId);
    assert.ok(schema, `${collectionId} is a collection of the model`);
    const ids = new Set();
    for (const document of documents) {
      assert.ok(!ids.has(document.id), `${collectionId}/${document.id} appears twice`);
      ids.add(document.id);
      assert.deepEqual(conformance(schema, document, `${collectionId}/${document.id}`), []);
    }
  }
});

test("the demo is deterministic and honours the transaction count", () => {
  const again = generateDemoHousehold({ householdId: "hh_demo", transactionCount: 60 });
  assert.deepEqual(again, demo);
  assert.equal(demo.transactions.length, 60);
  assert.equal(generateDemoHousehold({ householdId: "hh_demo" }).transactions.length, 200);
  assert.equal(demo.accounts.length, 7);
  assert.equal(demo.household.budget_mode, "category");
});

test("the demo's references resolve: groups, merchants, accounts, tags, and paid bills", () => {
  const taxonomyIds = new Set(demo.taxonomy.map((entry) => entry.id));
  const accountIds = new Set(demo.accounts.map((entry) => entry.id));
  const transactionIds = new Map(demo.transactions.map((entry) => [entry.id, entry]));
  for (const entry of demo.taxonomy) {
    if (entry.kind === "category") {
      assert.ok(taxonomyIds.has(entry.parent_id), `${entry.id} is filed under a group`);
      assert.equal(demo.taxonomy.find((group) => group.id === entry.parent_id)?.kind, "group");
    }
    if (entry.kind === "merchant") assert.ok(entry.patterns.length > 0, `${entry.id} has patterns`);
  }
  for (const transaction of demo.transactions) {
    assert.ok(accountIds.has(transaction.account_id), `${transaction.id} names an account`);
    for (const field of ["merchant_id", "category_id"]) {
      if (transaction[field] !== undefined) {
        assert.ok(taxonomyIds.has(transaction[field]), `${transaction.id}.${field} resolves`);
      }
    }
    for (const tag of transaction.tags) assert.ok(taxonomyIds.has(tag), `${transaction.id} tag`);
  }
  for (const recurrence of demo.recurrences) {
    assert.equal(recurrence.status, "confirmed");
    assert.ok(taxonomyIds.has(recurrence.category_id));
    assert.ok(taxonomyIds.has(recurrence.merchant_id));
    const paid = transactionIds.get(recurrence.last_paid_transaction_id);
    assert.ok(paid, `${recurrence.id} names the transaction that paid it`);
    assert.equal(paid.date, recurrence.last_date);
    assert.equal(paid.account_id, recurrence.account_id);
    assert.equal(paid.amount, recurrence.expected_amount);
  }
  for (const goal of demo.goals) {
    for (const accountId of goal.account_ids ?? []) assert.ok(accountIds.has(accountId));
  }
  for (const budget of demo.budgets) {
    if (budget.kind === "category") assert.ok(taxonomyIds.has(budget.category_id));
    assert.equal(budget.id, `bud_${budget.category_id}.${budget.month}`);
  }
});

test("transfers come in pairs, manual entries are reviewed, and synced ones await review", () => {
  assert.equal(transactionById("txn_demo_0001").description, "ACME Corp payroll");
  const legs = new Map();
  for (const transaction of demo.transactions) {
    if (transaction.transfer_id === undefined) continue;
    legs.set(transaction.transfer_id, [...(legs.get(transaction.transfer_id) ?? []), transaction]);
  }
  assert.equal(legs.size, 6);
  for (const [transferId, pair] of legs) {
    assert.equal(pair.length, 2, `${transferId} has two legs`);
    assert.equal(pair[0].amount + pair[1].amount, 0, `${transferId} legs cancel`);
    assert.notEqual(pair[0].account_id, pair[1].account_id, `${transferId} spans two accounts`);
  }
  const synced = demo.transactions.filter((transaction) => transaction.external_id !== undefined);
  assert.deepEqual(
    synced.map((transaction) => transaction.external_id),
    ["sim-demo-1", "sim-demo-2", "sim-demo-3"],
  );
  for (const transaction of synced) {
    assert.equal(transaction.reviewed, undefined);
    assert.equal(transaction.category_id, undefined);
  }
  for (const transaction of demo.transactions) {
    if (transaction.external_id === undefined) assert.equal(transaction.reviewed, true);
  }
  const hidden = demo.transactions.filter((transaction) => transaction.hidden === true);
  assert.deepEqual(
    hidden.map((transaction) => transaction.description).sort(),
    ["Balance update", "Venmo reimbursement"],
  );
  assert.equal(hidden.find((transaction) => transaction.adjustment)?.account_id, "acc_demo_home");
});

test("net-worth snapshots cover ninety days and agree with the derived balances", () => {
  assert.equal(demo.net_worth_snapshots.length, 90);
  const last = demo.net_worth_snapshots.at(-1);
  assert.equal(last.date, "2026-08-15");
  assert.equal(demo.net_worth_snapshots[0].date, "2026-05-18");
  const expected = new Map();
  for (const account of demo.accounts) {
    const holdings = (account.holdings ?? []).reduce(
      (total, holding) => total + Math.round(holding.quantity * holding.price),
      0,
    );
    expected.set(account.id, account.opening_balance + holdings);
  }
  for (const transaction of demo.transactions) {
    if (transaction.date > last.date) continue;
    expected.set(transaction.account_id, expected.get(transaction.account_id) + transaction.amount);
  }
  assert.deepEqual(
    Object.fromEntries(last.balances.map((entry) => [entry.account_id, entry.balance])),
    Object.fromEntries(expected),
  );
  const owed = ["credit", "loan", "other_liability"];
  let assets = 0;
  let liabilities = 0;
  for (const account of demo.accounts) {
    if (owed.includes(account.type)) liabilities -= expected.get(account.id);
    else assets += expected.get(account.id);
  }
  assert.equal(last.assets, assets);
  assert.equal(last.liabilities, liabilities);
  assert.equal(last.net_worth, assets - liabilities);
  // The house's revaluation shows up in its history on the day it was booked.
  const home = (snapshot) =>
    snapshot.balances.find((entry) => entry.account_id === "acc_demo_home");
  assert.equal(home(demo.net_worth_snapshots[0]).balance, 45_000_000);
  assert.equal(home(last).balance, 45_500_000);
});

function transactionById(id) {
  const found = demo.transactions.find((transaction) => transaction.id === id);
  assert.ok(found, `${id} exists`);
  return found;
}
