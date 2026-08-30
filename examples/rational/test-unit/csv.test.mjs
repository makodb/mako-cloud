import assert from "node:assert/strict";
import { test } from "node:test";

import {
  fingerprint,
  guessDateOrder,
  normalizeDate,
  parseCsv,
  parseRows,
  planImport,
  proposeMapping,
} from "../dist/src/selectors/csv.js";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

test("quoted fields, doubled quotes, CRLF, and a BOM all survive", () => {
  const table = parseCsv(
    '\uFEFFDate,Description,Amount\r\n' +
      '2026-08-02,"COFFEE, LARGE",-4.25\r\n' +
      '2026-08-03,"SHE SAID ""HI""",-1.00\r\n' +
      "\r\n",
  );
  assert.deepEqual(table.headers, ["Date", "Description", "Amount"]);
  assert.deepEqual(table.rows, [
    ["2026-08-02", "COFFEE, LARGE", "-4.25"],
    ["2026-08-03", 'SHE SAID "HI"', "-1.00"],
  ]);
});

test("a mapping is proposed from the headers a bank actually writes", () => {
  const signed = parseCsv("Posted Date,Payee,Amount\n2026-08-02,COFFEE,-4.25\n");
  assert.deepEqual(proposeMapping(signed), {
    date: "Posted Date",
    description: "Payee",
    amount: "Amount",
    dateOrder: "ISO",
  });

  const paired = parseCsv("Date,Narrative,Debit,Credit\n02/08/2026,RENT,850.00,\n");
  const mapping = proposeMapping(paired);
  assert.equal(mapping?.debit, "Debit");
  assert.equal(mapping?.credit, "Credit");
  assert.equal(mapping?.amount, undefined);

  // Nothing recognisable is not a wrong guess; it is no guess.
  assert.equal(proposeMapping(parseCsv("a,b\n1,2\n")), null);
});

test("an ambiguous date order is decided by evidence, not by hope", () => {
  // 13 can only be a day.
  const dayFirst = parseCsv("Date,Description,Amount\n03/04/2026,A,-1\n13/04/2026,B,-1\n");
  assert.equal(guessDateOrder(dayFirst, "Date"), "DMY");
  // 13 in the second position can only be a day, so the first is a month.
  const monthFirst = parseCsv("Date,Description,Amount\n04/03/2026,A,-1\n04/13/2026,B,-1\n");
  assert.equal(guessDateOrder(monthFirst, "Date"), "MDY");
  // No evidence: the common case for slashes, and the person can change it.
  const ambiguous = parseCsv("Date,Description,Amount\n03/04/2026,A,-1\n");
  assert.equal(guessDateOrder(ambiguous, "Date"), "MDY");

  assert.equal(normalizeDate("03/04/2026", "DMY"), "2026-04-03");
  assert.equal(normalizeDate("03/04/2026", "MDY"), "2026-03-04");
  assert.equal(normalizeDate("3/4/26", "MDY"), "2026-03-04");
  assert.equal(normalizeDate("2026-02-30", "ISO"), null);
  assert.equal(normalizeDate("not a date", "MDY"), null);
});

test("amounts arrive in every shape a bank writes them", () => {
  const table = parseCsv(
    "Date,Description,Amount\n" +
      "2026-08-02,SIGNED,-4.25\n" +
      "2026-08-03,THOUSANDS,\"1,234.56\"\n" +
      "2026-08-04,PARENTHESISED,(12.34)\n" +
      "2026-08-05,CREDIT MARKER,12.34 CR\n" +
      "2026-08-06,EMPTY,\n",
  );
  const rows = parseRows(table, proposeMapping(table), "USD");
  assert.deepEqual(
    rows.map((row) => [row.description, row.amount, row.problem]),
    [
      ["SIGNED", -425, undefined],
      ["THOUSANDS", 123_456, undefined],
      ["PARENTHESISED", -1_234, undefined],
      ["CREDIT MARKER", 1_234, undefined],
      ["EMPTY", 0, "the amount could not be read"],
    ],
  );
});

test("a debit column is money leaving, a credit column money arriving", () => {
  const table = parseCsv(
    "Date,Narrative,Debit,Credit\n2026-08-02,RENT,850.00,\n2026-08-03,SALARY,,2500.00\n",
  );
  const rows = parseRows(table, proposeMapping(table), "USD");
  assert.deepEqual(
    rows.map((row) => row.amount),
    [-85_000, 250_000],
  );
});

test("an unreadable row is reported by its line, never dropped", () => {
  const table = parseCsv("Date,Description,Amount\n2026-08-02,COFFEE,-4.25\nnope,,x\n");
  const rows = parseRows(table, proposeMapping(table), "USD");
  const plan = planImport(rows, [], "acc_1");
  assert.deepEqual(
    plan.unreadable.map((row) => [row.line, row.problem]),
    [[3, "the date could not be read"]],
  );
  assert.equal(plan.importable.length, 1);
});

test("a duplicate is one already stored, and two identical rows in a file are not", () => {
  const stored = [
    {
      ...base,
      id: "txn_1",
      account_id: "acc_1",
      date: "2026-08-02",
      amount: -425,
      currency: "USD",
      description: "Coffee #1234",
      tags: [],
      splits: [],
    },
    {
      ...base,
      id: "txn_2",
      account_id: "acc_2",
      date: "2026-08-02",
      amount: -425,
      currency: "USD",
      description: "Coffee",
      tags: [],
      splits: [],
    },
  ];
  const table = parseCsv(
    "Date,Description,Amount\n" +
      "2026-08-02,COFFEE 5678,-4.25\n" +
      "2026-08-02,COFFEE 5678,-4.25\n" +
      "2026-08-03,COFFEE 5678,-4.25\n",
  );
  const rows = parseRows(table, proposeMapping(table), "USD");
  const plan = planImport(rows, stored, "acc_1");
  // The first matches what is stored -- the digits differ, the normalized
  // description does not. The second is the same row again in one file, which
  // is a second real transaction. The third is another day.
  assert.deepEqual([...plan.duplicates], [2]);
  assert.deepEqual(
    plan.importable.map((row) => row.line),
    [3, 4],
  );
  // The other account's identical transaction is not this account's duplicate.
  assert.equal(
    fingerprint("2026-08-02", -425, "Coffee #1234"),
    fingerprint("2026-08-02", -425, "COFFEE 5678"),
  );
});
