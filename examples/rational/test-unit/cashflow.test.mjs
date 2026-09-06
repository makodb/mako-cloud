import assert from "node:assert/strict";
import { test } from "node:test";

import {
  cashFlowSummary,
  categoryTrend,
  inRange,
  monthlySeries,
  monthsBetween,
  rangeFor,
  rangeLabel,
  rangePresets,
  sankeyFlows,
  shiftRange,
  spendingBy,
  summaryFor,
} from "../dist/src/selectors/cashflow.js";

/**
 * Cash flow is the page most likely to disagree with the budget page, so the
 * tests pin the rule they share: transfers, hidden transactions, and balance
 * updates are not money moving; an amount under an income category is income;
 * an amount under an expense category is spending, refunds included; and
 * everything else counts by its sign.
 */

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function entry(id, kind, fields = {}) {
  return { ...base, id, kind, name: id, ...fields };
}

const taxonomy = [
  entry("grp_income", "group", { name: "Income", category_kind: "income" }),
  entry("grp_food", "group", { name: "Food", category_kind: "expense" }),
  entry("grp_home", "group", { name: "Home", category_kind: "expense" }),
  entry("cat_pay", "category", {
    name: "Paychecks",
    category_kind: "income",
    parent_id: "grp_income",
  }),
  entry("cat_groceries", "category", {
    name: "Groceries",
    category_kind: "expense",
    parent_id: "grp_food",
  }),
  entry("cat_restaurants", "category", {
    name: "Restaurants",
    category_kind: "expense",
    parent_id: "grp_food",
  }),
  entry("cat_rent", "category", { name: "Rent", category_kind: "expense", parent_id: "grp_home" }),
  entry("cat_orphan", "category", { name: "Orphan", category_kind: "expense" }),
  entry("tag_travel", "tag", { name: "Travel" }),
  entry("tag_shared", "tag", { name: "Shared" }),
  entry("mer_grocer", "merchant", { name: "Corner Grocer", patterns: ["sq corner grocer"] }),
];

function transaction(id, date, amount, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_a",
    date,
    amount,
    currency: "USD",
    description: id,
    tags: [],
    splits: [],
    ...fields,
  };
}

const transactions = [
  transaction("t1", "2026-08-01", 500_000, { category_id: "cat_pay" }),
  transaction("t2", "2026-08-03", -60_000, {
    category_id: "cat_groceries",
    description: "SQ *CORNER GROCER 0412",
  }),
  transaction("t3", "2026-08-05", -40_000, {
    category_id: "cat_restaurants",
    description: "Blue Bottle Coffee",
    tags: ["tag_travel", "tag_shared"],
  }),
  transaction("t4", "2026-08-01", -150_000, { category_id: "cat_rent" }),
  // A refund into an expense category reduces that category, not income.
  transaction("t5", "2026-08-06", 5_000, {
    category_id: "cat_groceries",
    description: "SQ *CORNER GROCER 0412",
  }),
  transaction("t6", "2026-08-07", -20_000),
  transaction("t7", "2026-08-08", 10_000),
  transaction("t8", "2026-08-09", -999_999, { hidden: true }),
  transaction("t9", "2026-08-10", -100_000, { transfer_id: "tr1" }),
  transaction("t10", "2026-08-10", 100_000, { account_id: "acc_b", transfer_id: "tr1" }),
  transaction("t11", "2026-08-11", 50_000, { adjustment: true }),
  transaction("t12", "2026-08-12", -7_000, { category_id: "cat_orphan" }),
  transaction("t13", "2026-07-15", -3_000, { category_id: "cat_groceries" }),
  transaction("t14", "2026-08-13", -1_000, { category_id: "cat_groceries", currency: "EUR" }),
];

const august = rangeFor("2026-08", "2026-09-06");
const context = { categories: taxonomy, groups: taxonomy, merchants: taxonomy, tags: taxonomy };

test("range keys cover months, quarters, years, custom spans, and all time", () => {
  assert.deepEqual(august, { start: "2026-08-01", end: "2026-08-31" });
  assert.deepEqual(rangeFor("2026-Q3", "2026-09-06"), { start: "2026-07-01", end: "2026-09-30" });
  assert.deepEqual(rangeFor("2026", "2026-09-06"), { start: "2026-01-01", end: "2026-12-31" });
  assert.deepEqual(rangeFor("custom:2026-08-15:2026-08-01", "2026-09-06"), {
    start: "2026-08-01",
    end: "2026-08-15",
  });
  assert.equal(rangeFor("all", "2026-09-06"), null);
  assert.deepEqual(rangeFor("garbage", "2026-09-06"), { start: "2026-09-01", end: "2026-09-30" });
  assert.deepEqual(rangeFor("2024-02", "2026-09-06"), { start: "2024-02-01", end: "2024-02-29" });

  assert.equal(inRange("2026-08-31", august), true);
  assert.equal(inRange("2026-09-01", august), false);
  assert.equal(inRange("1999-01-01", null), true);
});

test("ranges step to their neighbours and read as people say them", () => {
  assert.equal(shiftRange("2026-12", 1), "2027-01");
  assert.equal(shiftRange("2026-01", -1), "2025-12");
  assert.equal(shiftRange("2026-Q4", 1), "2027-Q1");
  assert.equal(shiftRange("2026-Q1", -1), "2025-Q4");
  assert.equal(shiftRange("2026", -1), "2025");
  assert.equal(shiftRange("all", 1), "all");
  assert.equal(shiftRange("custom:2026-08-01:2026-08-15", 1), "custom:2026-08-01:2026-08-15");

  assert.equal(rangeLabel("2026-08"), "August 2026");
  assert.equal(rangeLabel("2026-Q3"), "Q3 2026");
  assert.equal(rangeLabel("2026"), "2026");
  assert.equal(rangeLabel("all"), "All time");
  assert.equal(rangeLabel("custom:2026-08-01:2026-09-06"), "Aug 1, 2026 – Sep 6, 2026");
  assert.deepEqual(
    rangePresets("2026-09-06").map((preset) => preset.key),
    ["2026-09", "2026-08", "2026-Q3", "2026", "2025", "all"],
  );
  assert.deepEqual(monthsBetween("2026-11", "2027-02"), ["2026-11", "2026-12", "2027-01", "2027-02"]);
});

test("cash flow excludes hidden transactions, transfer legs, and balance updates", () => {
  const summaries = cashFlowSummary(transactions, taxonomy, august);
  assert.deepEqual(summaries, [
    { currency: "EUR", income: 0, spending: 1_000, savings: -1_000, savingsRate: 0 },
    {
      currency: "USD",
      income: 510_000,
      spending: 272_000,
      savings: 238_000,
      savingsRate: 238_000 / 510_000,
    },
  ]);
  assert.deepEqual(summaryFor(summaries, "GBP"), {
    currency: "GBP",
    income: 0,
    spending: 0,
    savings: 0,
    savingsRate: 0,
  });
});

test("the savings rate is of income, clamped to what a rate can be", () => {
  const overspent = cashFlowSummary(
    [transaction("a", "2026-08-01", 1_000), transaction("b", "2026-08-02", -3_000)],
    taxonomy,
    august,
  );
  assert.equal(overspent[0].savings, -2_000);
  assert.equal(overspent[0].savingsRate, 0);
  const half = cashFlowSummary(
    [transaction("a", "2026-08-01", 1_000), transaction("b", "2026-08-02", -500)],
    taxonomy,
    august,
  );
  assert.equal(half[0].savingsRate, 0.5);
});

test("the monthly series has a row for every month of the range, quiet ones included", () => {
  const rows = monthlySeries(transactions, taxonomy, rangeFor("2026-Q3", "2026-09-06"), "USD");
  assert.deepEqual(rows, [
    { month: "2026-07", currency: "USD", income: 0, spending: 3_000, net: -3_000 },
    { month: "2026-08", currency: "USD", income: 510_000, spending: 272_000, net: 238_000 },
    { month: "2026-09", currency: "USD", income: 0, spending: 0, net: 0 },
  ]);
  // Unbounded: only the months that occur, every currency seen.
  const all = monthlySeries(transactions, taxonomy, null);
  assert.deepEqual(
    all.map((row) => [row.month, row.currency]),
    [
      ["2026-07", "EUR"],
      ["2026-07", "USD"],
      ["2026-08", "EUR"],
      ["2026-08", "USD"],
    ],
  );
});

test("spending by category adds up to the summary's spending, and other keys name what they can", () => {
  const byCategory = spendingBy(transactions, august, "category", context);
  assert.deepEqual(
    byCategory.map((slice) => [slice.key, slice.name, slice.currency, slice.amount]),
    [
      ["cat_rent", "Rent", "USD", 150_000],
      ["cat_groceries", "Groceries", "USD", 55_000],
      ["cat_restaurants", "Restaurants", "USD", 40_000],
      ["", "Uncategorized", "USD", 20_000],
      ["cat_orphan", "Orphan", "USD", 7_000],
      ["cat_groceries", "Groceries", "EUR", 1_000],
    ],
  );
  const usd = byCategory
    .filter((slice) => slice.currency === "USD")
    .reduce((total, slice) => total + slice.amount, 0);
  assert.equal(usd, summaryFor(cashFlowSummary(transactions, taxonomy, august), "USD").spending);

  assert.deepEqual(
    spendingBy(transactions, august, "group", context)
      .filter((slice) => slice.currency === "USD")
      .map((slice) => [slice.key, slice.name, slice.amount]),
    [
      ["grp_home", "Home", 150_000],
      ["grp_food", "Food", 95_000],
      ["", "Uncategorized", 27_000],
    ],
  );

  const byMerchant = spendingBy(transactions, august, "merchant", context);
  const grocer = byMerchant.find((slice) => slice.key === "mer_grocer");
  assert.deepEqual(grocer, {
    key: "mer_grocer",
    name: "Corner Grocer",
    currency: "USD",
    amount: 55_000,
  });
  // A merchant that is only a cleaned description is keyed by its name.
  assert.ok(byMerchant.some((slice) => slice.key === "Blue Bottle Coffee" && slice.amount === 40_000));

  assert.deepEqual(
    spendingBy(transactions, august, "tag", context).map((slice) => [slice.name, slice.amount]),
    [
      ["Shared", 40_000],
      ["Travel", 40_000],
    ],
  );
  assert.deepEqual(
    spendingBy(transactions, august, "account", {
      ...context,
      accounts: [{ ...base, id: "acc_a", name: "Everyday" }],
    }).map((slice) => [slice.key, slice.name, slice.currency, slice.amount]),
    [
      ["acc_a", "Everyday", "USD", 272_000],
      ["acc_a", "Everyday", "EUR", 1_000],
    ],
  );
});

test("a category's trend has a point for every month asked about", () => {
  assert.deepEqual(
    categoryTrend(transactions, "cat_groceries", ["2026-07", "2026-08", "2026-09"], "USD"),
    [
      { month: "2026-07", amount: 3_000 },
      { month: "2026-08", amount: 55_000 },
      { month: "2026-09", amount: 0 },
    ],
  );
});

function conserved(graph) {
  const inflow = new Map();
  const outflow = new Map();
  for (const link of graph.links) {
    assert.ok(link.value > 0, `link ${link.source} -> ${link.target} is positive`);
    inflow.set(link.target, (inflow.get(link.target) ?? 0) + link.value);
    outflow.set(link.source, (outflow.get(link.source) ?? 0) + link.value);
  }
  for (const node of graph.nodes) {
    if (!inflow.has(node.id) || !outflow.has(node.id)) continue;
    assert.equal(inflow.get(node.id), outflow.get(node.id), `${node.id} conserves`);
  }
  const ids = new Set(graph.nodes.map((node) => node.id));
  for (const link of graph.links) {
    assert.ok(ids.has(link.source) && ids.has(link.target), "every link joins two nodes");
  }
}

test("the Sankey conserves: income categories into the hub, the hub into groups and savings", () => {
  const graph = sankeyFlows(transactions, taxonomy, taxonomy, august, "USD");
  conserved(graph);
  const link = (source, target) =>
    graph.links.find((entry) => entry.source === source && entry.target === target)?.value;
  assert.equal(link("income:cat_pay", "income"), 500_000);
  assert.equal(link("income:", "income"), 10_000);
  assert.equal(link("income", "group:grp_home"), 150_000);
  assert.equal(link("income", "group:grp_food"), 95_000);
  assert.equal(link("income", "group:"), 27_000);
  assert.equal(link("income", "savings"), 238_000);
  assert.equal(link("group:grp_food", "category:cat_groceries"), 55_000);
  assert.equal(link("group:", "category:"), 20_000);
  assert.equal(link("group:", "category:cat_orphan"), 7_000);
  assert.ok(!graph.nodes.some((node) => node.id === "deficit"));
  assert.deepEqual(
    graph.nodes.filter((node) => node.kind === "savings").map((node) => node.label),
    ["Savings"],
  );
});

/**
 * The same measurement `reports.test.mjs` makes, repeated for the selectors
 * this change adds, as the design asks. The bound is loose on purpose: it is
 * there to catch an accidental quadratic -- a merchant resolved by scanning
 * every pattern for every row, say -- not to police milliseconds.
 */
test("every new selector over 50 000 transactions stays interactive", async () => {
  const { applyTransactionQuery } = await import("../dist/src/selectors/filters.js");
  // Normalization drops digits, so a shop's token has to be letters.
  const token = (number) => String(number).replaceAll(/\d/gu, (digit) => "abcdefghij"[Number(digit)]);
  const merchants = Array.from({ length: 300 }, (unused, index) =>
    entry(`mer_${index}`, "merchant", {
      name: `Merchant ${index}`,
      patterns: [`shop ${token(index)}`],
    }),
  );
  const many = [...taxonomy, ...merchants];
  const big = Array.from({ length: 50_000 }, (unused, index) => {
    const month = 1 + (index % 12);
    const day = 1 + (index % 28);
    return transaction(`txn_${index}`, `2026-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`, index % 11 === 0 ? 12_345 : -((index % 97) + 1) * 13, {
      description: `SHOP ${token(index % 400).toUpperCase()} #${index}`,
      category_id: index % 11 === 0 ? "cat_pay" : index % 3 === 0 ? "cat_groceries" : index % 3 === 1 ? "cat_rent" : "cat_restaurants",
      tags: index % 5 === 0 ? ["tag_travel"] : [],
      ...(index % 13 === 0 ? { transfer_id: `tr_${index}` } : {}),
    });
  });
  const year = rangeFor("2026", "2026-12-31");
  const measured = {};
  const measure = (name, run) => {
    const started = process.hrtime.bigint();
    const result = run();
    measured[name] = Number(process.hrtime.bigint() - started) / 1e6;
    return result;
  };
  const summary = measure("cashFlowSummary", () => cashFlowSummary(big, many, year));
  measure("monthlySeries", () => monthlySeries(big, many, year, "USD"));
  measure("spendingBy(merchant)", () =>
    spendingBy(big, year, "merchant", { categories: many, groups: many, merchants: many }),
  );
  measure("sankeyFlows", () => sankeyFlows(big, many, many, year, "USD"));
  const searched = measure("applyTransactionQuery(text)", () =>
    applyTransactionQuery(big, { text: "merchant 7" }, { categories: many, merchants: many }),
  );
  console.log(
    `new selectors over 50000 transactions (ms): ${Object.entries(measured)
      .map(([name, milliseconds]) => `${name}=${milliseconds.toFixed(1)}`)
      .join(" ")}`,
  );
  assert.equal(summary.length, 1);
  assert.ok(searched.length > 0, "the text search found the merchants named seven-something");
  const total = Object.values(measured).reduce((sum, value) => sum + value, 0);
  assert.ok(total < 5_000, `the new selectors together took ${total.toFixed(0)}ms`);
});

test("when spending exceeds income a deficit node supplies the difference", () => {
  const graph = sankeyFlows(
    [
      transaction("a", "2026-08-01", 10_000, { category_id: "cat_pay" }),
      transaction("b", "2026-08-02", -15_000, { category_id: "cat_rent" }),
      // A category refunded on balance is left out of the picture.
      transaction("c", "2026-08-03", 2_000, { category_id: "cat_groceries" }),
    ],
    taxonomy,
    taxonomy,
    august,
    "USD",
  );
  conserved(graph);
  const deficit = graph.links.find((link) => link.source === "deficit");
  assert.deepEqual(deficit, { source: "deficit", target: "income", value: 5_000 });
  assert.ok(!graph.nodes.some((node) => node.id === "savings"));
  assert.ok(!graph.nodes.some((node) => node.id === "category:cat_groceries"));
});
