import assert from "node:assert/strict";
import { test } from "node:test";

import { resolveMerchant } from "../functions/shared/merchants.ts";
import { normalizeDescription } from "../functions/shared/recurrences.ts";
import {
  merchantDisplay,
  merchantResolver,
  merchantRows,
} from "../dist/src/selectors/merchants.js";

/**
 * The indexed resolver exists for speed; the test that matters is that it
 * never decides differently from the shared resolver it fronts.
 */

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function merchant(id, name, patterns) {
  return { ...base, id, kind: "merchant", name, patterns };
}

const merchants = [
  merchant("mer_grocer", "Corner Grocer", ["sq corner grocer"]),
  merchant("mer_coffee", "Blue Bottle", ["blue bottle coffee", "sq corner grocer"]),
  merchant("mer_empty", "Nobody Yet", []),
  { ...base, id: "cat_food", kind: "category", name: "Food" },
];

function transaction(id, description, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_a",
    date: "2026-08-01",
    amount: -1_000,
    currency: "USD",
    description,
    tags: [],
    splits: [],
    ...fields,
  };
}

const cases = [
  transaction("by-id", "whatever", { merchant_id: "mer_coffee" }),
  transaction("dangling-id-with-pattern", "SQ *CORNER GROCER 0412", { merchant_id: "mer_gone" }),
  transaction("by-pattern", "SQ *CORNER GROCER 0412"),
  transaction("by-stored-normalized", "ignored text", { normalized_description: "blue bottle coffee" }),
  transaction("cleaned", "TST* THE LOCAL BAKERY 0091"),
  transaction("processor-only", "PAYPAL"),
  transaction("empty", "   "),
  transaction("blank-id", "Blue Bottle*Coffee", { merchant_id: "" }),
];

test("the indexed resolver agrees with the shared resolver on every case", () => {
  const resolve = merchantResolver(merchants);
  const onlyMerchants = merchants.filter((entry) => entry.kind === "merchant");
  for (const subject of cases) {
    assert.deepEqual(
      resolve(subject),
      resolveMerchant(subject, onlyMerchants, normalizeDescription),
      subject.id,
    );
  }
  // And the decisions themselves are the design's.
  assert.deepEqual(resolve(cases[0]), { id: "mer_coffee", name: "Blue Bottle" });
  assert.deepEqual(resolve(cases[1]), { id: "mer_grocer", name: "Corner Grocer" });
  assert.deepEqual(resolve(cases[2]), { id: "mer_grocer", name: "Corner Grocer" }, "first claim wins");
  assert.deepEqual(resolve(cases[4]), { id: null, name: "The Local Bakery" });
  assert.equal(merchantDisplay(cases[5], merchants), "Paypal");
});

test("merchant rows count everything, total counted spending, and list merchants nobody used", () => {
  const transactions = [
    transaction("a", "SQ *CORNER GROCER 0412", { date: "2026-08-03", amount: -6_000 }),
    transaction("b", "SQ *CORNER GROCER 0412", { date: "2026-08-05", amount: 500 }),
    transaction("c", "SQ *CORNER GROCER 0412", { date: "2026-08-09", amount: -700, hidden: true }),
    transaction("d", "SQ *CORNER GROCER 0412", { date: "2026-08-01", amount: -900, currency: "EUR" }),
    transaction("e", "TST* THE LOCAL BAKERY 0091", { date: "2026-08-02", amount: -1_200 }),
    transaction("f", "The Local Bakery", { date: "2026-08-04", amount: -800 }),
  ];
  const rows = merchantRows(transactions, merchants, "USD");
  assert.deepEqual(rows, [
    { id: "mer_grocer", name: "Corner Grocer", count: 4, total: 6_000, lastDate: "2026-08-09" },
    { id: null, name: "The Local Bakery", count: 2, total: 2_000, lastDate: "2026-08-04" },
    { id: "mer_coffee", name: "Blue Bottle", count: 0, total: 0, lastDate: "" },
    { id: "mer_empty", name: "Nobody Yet", count: 0, total: 0, lastDate: "" },
  ]);
  assert.equal(merchantRows(transactions, merchants)[0].total, 6_900, "every currency when none is given");
});
