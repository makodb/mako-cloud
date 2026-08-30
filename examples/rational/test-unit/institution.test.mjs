import assert from "node:assert/strict";
import { test } from "node:test";

import { statement, transactionId } from "../functions/institution-sync/institution.ts";

/**
 * The simulated institution is deterministic on purpose: what the sync has to
 * prove is that running twice over an overlapping window does not double a
 * household's transactions, and that is only testable if asking twice gives
 * the same answer.
 */
test("the same account and window always answer the same statement", () => {
  const first = statement("acct-1", "2026-08-10", 10);
  const second = statement("acct-1", "2026-08-10", 10);
  assert.deepEqual(first, second);
  assert.notDeepEqual(first, statement("acct-2", "2026-08-10", 10));
});

test("an overlapping window repeats the entries it shares, with the same ids", () => {
  const earlier = statement("acct-1", "2026-08-05", 10);
  const later = statement("acct-1", "2026-08-10", 10);
  const shared = new Set(earlier.map((entry) => entry.externalId));
  const overlap = later.filter((entry) => shared.has(entry.externalId));
  assert.ok(overlap.length > 0, "the windows overlap by five days");
  for (const entry of overlap) {
    const original = earlier.find((candidate) => candidate.externalId === entry.externalId);
    assert.deepEqual(entry, original, "a repeated entry is identical, not merely similar");
  }
});

test("entries are inside the window, dated, and mostly outgoing", () => {
  const entries = statement("acct-1", "2026-08-10", 10);
  for (const entry of entries) {
    assert.match(entry.date, /^\d{4}-\d{2}-\d{2}$/u);
    assert.ok(entry.date >= "2026-08-01" && entry.date <= "2026-08-10");
    assert.ok(Number.isSafeInteger(entry.amount) && entry.amount !== 0);
    assert.ok(entry.description.length > 0);
  }
  // The first of the month carries a salary, so a household has income too.
  assert.ok(entries.some((entry) => entry.amount > 0 && entry.description.startsWith("SALARY")));
});

test("a transaction id is one an edge function can actually write", () => {
  const id = transactionId("acc_everyday", "acct-1-2026-08-01-0");
  assert.equal(id, "txn_acc_everyday.acct-1-2026-08-01-0");
  // No character `encodeURIComponent` escapes: the data-plane route compares
  // the path segment with the body id, and an escaped id could not be written
  // from a function at all (findings log #12).
  assert.equal(encodeURIComponent(id), id);
  // Anything an institution puts in an external id is reduced to that set.
  assert.equal(encodeURIComponent(transactionId("acc_1", "a b/c:d")), transactionId("acc_1", "a b/c:d"));
});
