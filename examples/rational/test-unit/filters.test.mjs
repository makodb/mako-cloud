import assert from "node:assert/strict";
import { test } from "node:test";

import {
  activeFilterCount,
  applyTransactionQuery,
  groupByDate,
  parseTransactionQuery,
  serializeTransactionQuery,
} from "../dist/src/selectors/filters.js";

/**
 * The transactions page's query lives in the URL, so the codec has to be
 * exact both ways and forgiving of what it does not understand; the filters
 * have to leave out what the design says is left out by default (hidden) and
 * keep what it says is kept (transfers), and every filter has to be reachable
 * from the link.
 */

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function entry(id, kind, fields = {}) {
  return { ...base, id, kind, name: id, ...fields };
}

const taxonomy = [
  entry("grp_food", "group", { category_kind: "expense" }),
  entry("grp_fun", "group", { category_kind: "expense" }),
  entry("cat_food", "category", { category_kind: "expense", parent_id: "grp_food" }),
  entry("cat_fun", "category", { category_kind: "expense", parent_id: "grp_fun" }),
  entry("tag_x", "tag"),
  entry("mer_cafe", "merchant", { name: "Blue Bottle", patterns: ["blue bottle coffee"] }),
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
  transaction("t1", "2026-08-01", -425, {
    description: "Blue Bottle*Coffee 07/12",
    category_id: "cat_food",
    notes: "morning",
  }),
  transaction("t2", "2026-08-02", -1_200, {
    description: "Cinema",
    category_id: "cat_fun",
    tags: ["tag_x"],
  }),
  transaction("t3", "2026-08-03", -5_000, { description: "Unknown vendor" }),
  transaction("t4", "2026-08-04", 250_000, { description: "ACME payroll", category_id: "cat_pay" }),
  transaction("t5", "2026-08-05", -999, { description: "Secret", hidden: true }),
  transaction("t6", "2026-08-06", -10_000, { description: "To savings", transfer_id: "tr1" }),
  transaction("t7", "2026-08-06", 10_000, {
    account_id: "acc_b",
    description: "From checking",
    transfer_id: "tr1",
  }),
  transaction("t8", "2026-08-07", -3_000, {
    description: "Split shop",
    splits: [
      { id: "s1", category_id: "cat_food", amount: -1_000 },
      { id: "s2", amount: -2_000 },
    ],
  }),
  transaction("t9", "2026-07-31", -100, {
    account_id: "acc_b",
    description: "July",
    category_id: "cat_fun",
    reviewed: true,
    external_id: "ext1",
  }),
  transaction("t10", "2026-08-08", -200, {
    description: "Synced",
    category_id: "cat_fun",
    external_id: "ext2",
  }),
  transaction("t11", "2026-08-09", -300, {
    description: "Imported",
    category_id: "cat_fun",
    import_batch_id: "imp1",
    reviewed: false,
  }),
  transaction("t12", "2026-08-10", -400, { description: "Manual", category_id: "cat_fun" }),
  transaction("t13", "2026-08-11", -600, {
    description: "Filed",
    category_id: "cat_food",
    merchant_id: "mer_cafe",
  }),
];

const context = { categories: taxonomy, merchants: taxonomy };
const ids = (list) => list.map((item) => item.id);
const apply = (query) => ids(applyTransactionQuery(transactions, query, context));

test("the query codec round-trips every field with short, ordered keys", () => {
  const query = {
    text: "coffee",
    accountId: "acc_a",
    categoryId: "",
    groupId: "grp_food",
    tagId: "tag_x",
    merchantId: "mer_cafe",
    from: "2026-08-01",
    to: "2026-08-31",
    month: "2026-08",
    amountMin: 100,
    amountMax: 5_000,
    review: "needs",
    hidden: "only",
    transfers: "exclude",
    sort: "amount_desc",
  };
  const params = serializeTransactionQuery(query);
  assert.equal(
    params.toString(),
    "q=coffee&account=acc_a&category=&group=grp_food&tag=tag_x&merchant=mer_cafe" +
      "&from=2026-08-01&to=2026-08-31&month=2026-08&min=100&max=5000" +
      "&review=needs&hidden=only&transfers=exclude&sort=amount_desc",
  );
  assert.deepEqual(parseTransactionQuery(params), query);
  assert.deepEqual(parseTransactionQuery(new URLSearchParams()), {});
  assert.equal(serializeTransactionQuery({}).toString(), "");
});

test("a value the codec does not understand is dropped, not refused", () => {
  const parsed = parseTransactionQuery(
    new URLSearchParams(
      "q=+&account=&sort=sideways&review=maybe&hidden=never&transfers=some" +
        "&from=2026-02-30&to=yesterday&min=abc&max=-500&month=2026-13",
    ),
  );
  // A negative bound is read as a magnitude: amounts are compared on theirs.
  assert.deepEqual(parsed, { amountMax: 500 });
  // "category=" is the one empty value that means something: uncategorized.
  assert.deepEqual(parseTransactionQuery(new URLSearchParams("category=")), { categoryId: "" });
  assert.equal(activeFilterCount({ text: "a", categoryId: "", sort: "date_asc" }), 2);
});

test("hidden transactions are left out unless asked for; transfers are kept unless asked otherwise", () => {
  assert.deepEqual(apply({}), [
    "t13",
    "t12",
    "t11",
    "t10",
    "t8",
    "t7",
    "t6",
    "t4",
    "t3",
    "t2",
    "t1",
    "t9",
  ]);
  assert.deepEqual(apply({ hidden: "only" }), ["t5"]);
  assert.equal(apply({ hidden: "include" }).length, 13);
  assert.deepEqual(apply({ transfers: "only" }), ["t7", "t6"]);
  assert.equal(apply({ transfers: "exclude" }).length, 10);
});

test("every filter narrows the list as its link says", () => {
  assert.deepEqual(apply({ accountId: "acc_b" }), ["t7", "t9"]);
  // Uncategorized means filed under nothing, by the transaction or by a split.
  assert.deepEqual(apply({ categoryId: "" }), ["t8", "t7", "t6", "t3"]);
  assert.deepEqual(apply({ categoryId: "cat_food" }), ["t13", "t8", "t1"]);
  assert.deepEqual(apply({ groupId: "grp_food" }), ["t13", "t8", "t1"]);
  assert.deepEqual(apply({ tagId: "tag_x" }), ["t2"]);
  // Filed by hand, and resolved by pattern, are both the merchant.
  assert.deepEqual(apply({ merchantId: "mer_cafe" }), ["t13", "t1"]);
  assert.deepEqual(apply({ from: "2026-08-08", to: "2026-08-10" }), ["t12", "t11", "t10"]);
  assert.deepEqual(apply({ month: "2026-07" }), ["t9"]);
  assert.deepEqual(apply({ amountMin: 1_000, amountMax: 5_000 }), ["t8", "t3", "t2"]);
  assert.deepEqual(apply({ review: "needs" }), ["t11", "t10"]);
  assert.deepEqual(apply({ review: "reviewed" }), ["t9"]);
});

test("text search reads the description, the merchant's name, the notes, and the amount", () => {
  assert.deepEqual(apply({ text: "BLUE" }), ["t13", "t1"], "the merchant name matches t13");
  assert.deepEqual(apply({ text: "morning" }), ["t1"]);
  assert.deepEqual(apply({ text: "4.25" }), ["t1"]);
  assert.deepEqual(apply({ text: "nothing like this" }), []);
});

test("sorting by amount uses the magnitude, so the biggest purchase leads", () => {
  const largest = apply({ sort: "amount_desc" });
  assert.deepEqual(largest.slice(0, 3), ["t4", "t7", "t6"]);
  assert.equal(apply({ sort: "amount_asc" })[0], "t9");
  assert.equal(apply({ sort: "date_asc" })[0], "t9");
});

test("grouping by day keeps the list's order within a day and totals what is shown", () => {
  const groups = groupByDate(applyTransactionQuery(transactions, {}, context));
  assert.equal(groups[0].date, "2026-08-11");
  assert.deepEqual(ids(groups[0].transactions), ["t13"]);
  assert.equal(groups[0].total, -600);
  const transferDay = groups.find((group) => group.date === "2026-08-06");
  assert.deepEqual(ids(transferDay.transactions), ["t7", "t6"]);
  assert.equal(transferDay.total, 0);
  assert.equal(groups.at(-1).date, "2026-07-31");
});
