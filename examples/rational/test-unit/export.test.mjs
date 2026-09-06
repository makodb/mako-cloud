import assert from "node:assert/strict";
import { test } from "node:test";

import { parseCsv } from "../dist/src/selectors/csv.js";
import {
  accountsCsv,
  csvDocument,
  csvEscape,
  transactionsCsv,
} from "../dist/src/selectors/export.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

test("fields are quoted only when they need to be, and quotes are doubled", () => {
  assert.equal(csvEscape("plain"), "plain");
  assert.equal(csvEscape("Food & Dining"), "Food & Dining");
  assert.equal(csvEscape(""), "");
  assert.equal(csvEscape("a, b"), '"a, b"');
  assert.equal(csvEscape('say "hi"'), '"say ""hi"""');
  assert.equal(csvEscape("two\nlines"), '"two\nlines"');
  assert.equal(csvEscape("carriage\rreturn"), '"carriage\rreturn"');
  assert.equal(csvDocument(["a", "b"], [["1", "2"]]), "a,b\r\n1,2\r\n");
});

test("transactions export with names for ids, decimal amounts, and every awkward character intact", () => {
  const accounts = [
    { ...base, id: "acc_a", name: "Everyday, checking", type: "checking", currency: "USD", opening_balance: 0, opening_date: "2026-01-01" },
  ];
  const taxonomy = [
    { ...base, id: "grp_food", kind: "group", name: "Food & Dining", category_kind: "expense" },
    { ...base, id: "cat_food", kind: "category", name: "Food", category_kind: "expense", parent_id: "grp_food" },
    { ...base, id: "tag_a", kind: "tag", name: "Trip" },
    { ...base, id: "tag_b", kind: "tag", name: "Shared" },
    { ...base, id: "mer_x", kind: "merchant", name: "Named Merchant", patterns: ["named"] },
  ];
  const transactions = [
    {
      ...base,
      id: "t1",
      account_id: "acc_a",
      date: "2026-08-01",
      amount: -1_234,
      currency: "USD",
      description: 'He said "hi", twice',
      category_id: "cat_food",
      tags: ["tag_a", "tag_b"],
      notes: "line1\nline2",
      hidden: true,
      splits: [],
    },
    {
      ...base,
      id: "t2",
      account_id: "acc_gone",
      date: "2026-08-02",
      amount: 1_500,
      currency: "JPY",
      description: "NAMED",
      tags: [],
      splits: [],
      transfer_id: "tr_1",
    },
  ];
  const csv = transactionsCsv(transactions, { accounts, taxonomy });
  assert.equal(
    csv,
    "date,account,description,merchant,category,group,tags,amount,currency,notes,hidden,transfer_id\r\n" +
      '2026-08-01,"Everyday, checking","He said ""hi"", twice",He Said Hi Twice,Food,Food & Dining,Trip;Shared,-12.34,USD,"line1\nline2",true,\r\n' +
      "2026-08-02,acc_gone,NAMED,Named Merchant,,,,1500,JPY,,false,tr_1\r\n",
  );
  // What went out comes back in through the importer's own reader.
  const table = parseCsv(csv);
  assert.equal(table.rows.length, 2);
  assert.equal(table.rows[0][2], 'He said "hi", twice');
  assert.equal(table.rows[0][9], "line1\nline2");
});

test("accounts export with their class, type, and balance as decimal text", () => {
  const accounts = [
    { ...base, id: "acc_a", name: "Main", type: "checking", currency: "USD", opening_balance: 1, opening_date: "2026-01-01" },
    { ...base, id: "acc_b", name: "Card", type: "credit", currency: "USD", opening_balance: -5_000, opening_date: "2026-01-01", institution: "Bank, Inc.", hide_from_net_worth: true, closed_at: 9 },
  ];
  const balances = new Map([["acc_a", 123_456]]);
  assert.equal(
    accountsCsv(accounts, balances),
    "name,class,type,currency,balance,institution,hidden,closed\r\n" +
      "Main,Cash,Checking,USD,1234.56,,false,false\r\n" +
      'Card,Credit cards,Credit card,USD,-50.00,"Bank, Inc.",true,true\r\n',
  );
});
