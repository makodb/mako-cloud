import assert from "node:assert/strict";
import { test } from "node:test";

import {
  addDays,
  daysBetween,
  detectRecurrences,
  intervalOf,
  nextOccurrence,
  upcomingBills,
} from "../dist/src/selectors/recurrences.js";
import {
  advanceAfterPayment,
  monthlyEquivalent,
  monthlyTotal,
  occurrencesInMonth,
  paymentFor,
  previousOccurrence,
  resolveBills,
  shiftOccurrence,
  slackDays,
} from "../functions/shared/recurrences.ts";

const base = { household_id: "hh_test", created_at: 1, updated_at: 1 };

function transaction(id, date, amount, description, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_1",
    date,
    amount,
    currency: "USD",
    description,
    tags: [],
    splits: [],
    ...fields,
  };
}

test("dates move by days and by months, keeping the day where a month is shorter", () => {
  assert.equal(daysBetween("2026-08-01", "2026-08-15"), 14);
  assert.equal(addDays("2026-08-30", 7), "2026-09-06");
  assert.equal(nextOccurrence("2026-01-31", "monthly"), "2026-02-28");
  assert.equal(nextOccurrence("2026-02-28", "monthly"), "2026-03-28");
  assert.equal(nextOccurrence("2026-08-15", "quarterly"), "2026-11-15");
  assert.equal(nextOccurrence("2026-08-15", "yearly"), "2027-08-15");
  assert.equal(nextOccurrence("2026-08-15", "biweekly"), "2026-08-29");
});

test("an interval is the one every gap is close to, or none", () => {
  assert.equal(intervalOf([30, 31, 28]), "monthly");
  assert.equal(intervalOf([7, 7, 8]), "weekly");
  assert.equal(intervalOf([90, 92]), "quarterly");
  // A monthly bill and a one-off in the middle: the gaps disagree.
  assert.equal(intervalOf([30, 5, 25]), null);
});

test("three occurrences at a steady interval are a recurrence; two are a coincidence", () => {
  const monthly = [
    transaction("t1", "2026-06-01", -1_299, "NETFLIX.COM 4471"),
    transaction("t2", "2026-07-01", -1_299, "NETFLIX.COM 8802"),
    transaction("t3", "2026-08-01", -1_499, "NETFLIX COM 9911"),
  ];
  const [detected, ...rest] = detectRecurrences(monthly);
  assert.equal(rest.length, 0);
  assert.equal(detected?.interval, "monthly");
  assert.equal(detected?.occurrences, 3);
  // The median, so one month's price rise does not become the expectation.
  assert.equal(detected?.expectedAmount, -1_299);
  assert.equal(detected?.lastDate, "2026-08-01");
  assert.equal(detected?.nextDate, "2026-09-01");
  assert.equal(detected?.description, "NETFLIX COM 9911");

  assert.deepEqual(detectRecurrences(monthly.slice(0, 2)), []);
});

test("income, one-offs, and other accounts are not recurrences", () => {
  const mixed = [
    transaction("i1", "2026-06-01", 250_000, "SALARY"),
    transaction("i2", "2026-07-01", 250_000, "SALARY"),
    transaction("i3", "2026-08-01", 250_000, "SALARY"),
    transaction("o1", "2026-06-02", -1_299, "GYM", { account_id: "acc_1" }),
    transaction("o2", "2026-07-02", -1_299, "GYM", { account_id: "acc_2" }),
    transaction("o3", "2026-08-02", -1_299, "GYM", { account_id: "acc_1" }),
  ];
  // Income is not a bill, and the gym charges are split across two accounts,
  // so neither account has three.
  assert.deepEqual(detectRecurrences(mixed), []);
});

test("a dismissed or confirmed recurrence is not detected again", () => {
  const monthly = [
    transaction("t1", "2026-06-01", -1_299, "NETFLIX"),
    transaction("t2", "2026-07-01", -1_299, "NETFLIX"),
    transaction("t3", "2026-08-01", -1_299, "NETFLIX"),
  ];
  const known = [
    {
      ...base,
      id: "rec_1",
      account_id: "acc_1",
      normalized_description: "netflix",
      interval: "monthly",
      expected_amount: -1_299,
      currency: "USD",
      next_date: "2026-09-01",
      status: "dismissed",
      matched_count: 3,
    },
  ];
  assert.deepEqual(detectRecurrences(monthly, known), []);
});

test("upcoming bills are soonest first, and a missed one stays as late", () => {
  const recurrence = (id, nextDate, status = "confirmed") => ({
    ...base,
    id,
    account_id: "acc_1",
    normalized_description: id,
    interval: "monthly",
    expected_amount: -1_299,
    currency: "USD",
    next_date: nextDate,
    status,
    matched_count: 3,
  });
  const bills = upcomingBills(
    [
      recurrence("late", "2026-08-25"),
      recurrence("soon", "2026-09-02"),
      recurrence("far", "2026-12-01"),
      recurrence("suggested", "2026-09-03", "suggested"),
    ],
    "2026-09-01",
  );
  assert.deepEqual(
    bills.map((bill) => [bill.recurrence.id, bill.daysAway]),
    [
      ["late", -7],
      ["soon", 1],
    ],
  );
});

test("a transfer leg, a hidden charge, or a balance update is not a bill", () => {
  const withTransfer = [
    transaction("t1", "2026-06-01", -1_299, "NETFLIX"),
    transaction("t2", "2026-07-01", -1_299, "NETFLIX", { transfer_id: "tr_1" }),
    transaction("t3", "2026-08-01", -1_299, "NETFLIX"),
  ];
  assert.deepEqual(detectRecurrences(withTransfer), [], "two counted occurrences are a coincidence");
  const withHidden = [
    transaction("t1", "2026-06-01", -1_299, "NETFLIX", { hidden: true }),
    transaction("t2", "2026-07-01", -1_299, "NETFLIX"),
    transaction("t3", "2026-08-01", -1_299, "NETFLIX", { adjustment: true }),
  ];
  assert.deepEqual(detectRecurrences(withHidden), []);
});

function bill(id, nextDate, fields = {}) {
  return {
    ...base,
    id,
    account_id: "acc_1",
    normalized_description: "rent",
    interval: "monthly",
    expected_amount: -150_000,
    currency: "USD",
    next_date: nextDate,
    status: "confirmed",
    matched_count: 3,
    ...fields,
  };
}

test("a bill is paid by a counted charge with its description near its date, on its account, with its sign", () => {
  const rent = bill("rec_rent", "2026-09-01");
  const paid = transaction("p1", "2026-09-03", -150_000, "RENT 0912");
  assert.equal(paymentFor(rent, [paid])?.id, "p1");
  assert.equal(paymentFor(rent, [transaction("p2", "2026-09-03", -150_000, "RENT", { account_id: "acc_2" })]), null);
  assert.equal(paymentFor(rent, [transaction("p3", "2026-09-03", 150_000, "RENT")]), null, "a refund is not a payment");
  assert.equal(paymentFor(rent, [transaction("p4", "2026-09-03", -150_000, "RENT", { hidden: true })]), null);
  assert.equal(paymentFor(rent, [transaction("p5", "2026-09-03", -150_000, "RENT", { transfer_id: "tr" })]), null);
  assert.equal(paymentFor(rent, [transaction("p6", "2026-09-07", -150_000, "RENT")]), null, "past the slack");
  assert.equal(slackDays("monthly"), 5);
  // A stored normalized description is trusted over the raw text.
  assert.equal(
    paymentFor(rent, [transaction("p7", "2026-09-01", -150_000, "anything", { normalized_description: "rent" })])?.id,
    "p7",
  );
  // The closest one wins; equally close, the lower id.
  const two = [transaction("p9", "2026-09-02", -150_000, "RENT"), transaction("p8", "2026-09-02", -150_000, "RENT")];
  assert.equal(paymentFor(rent, two)?.id, "p8");
});

test("bills resolve to upcoming, due, paid, or late, soonest first", () => {
  const recurrences = [
    bill("rec_far", "2026-12-01", { normalized_description: "insurance" }),
    bill("rec_rent", "2026-09-01"),
    bill("rec_gym", "2026-08-20", { normalized_description: "gym", expected_amount: -3_000 }),
    bill("rec_today", "2026-09-05", { normalized_description: "phone" }),
    bill("rec_paused", "2026-09-05", { status: "paused" }),
    bill("rec_detected", "2026-09-05", { status: "detected" }),
    bill("rec_dismissed", "2026-09-05", { status: "dismissed" }),
  ];
  const transactions = [transaction("p1", "2026-09-03", -150_000, "RENT 0912")];
  const resolved = resolveBills(recurrences, transactions, "2026-09-05");
  assert.deepEqual(
    resolved.map((entry) => [entry.recurrence.id, entry.state, entry.daysAway, entry.paidBy]),
    [
      ["rec_gym", "late", -16, undefined],
      ["rec_rent", "paid", -4, "p1"],
      ["rec_today", "due", 0, undefined],
      ["rec_far", "upcoming", 87, undefined],
    ],
  );
  // Within the slack after its date a bill is due, not late: the bank may still post it.
  assert.equal(resolveBills([bill("r", "2026-09-01")], [], "2026-09-06")[0].state, "due");
  assert.equal(resolveBills([bill("r", "2026-09-01")], [], "2026-09-07")[0].state, "late");
  assert.equal(resolveBills([bill("r", "2026-09-01")], [], "2026-08-20")[0].state, "upcoming");
  // A horizon drops what is far off, never what is late, due, or paid.
  assert.deepEqual(
    resolveBills(recurrences, transactions, "2026-09-05", { withinDays: 30 }).map((entry) => entry.recurrence.id),
    ["rec_gym", "rec_rent", "rec_today"],
  );
});

test("advancing after a payment moves the date on from the payment itself", () => {
  assert.deepEqual(advanceAfterPayment({ matched_count: 3 }, { id: "p1", date: "2026-09-03" }, "monthly"), {
    next_date: "2026-10-03",
    last_date: "2026-09-03",
    last_paid_transaction_id: "p1",
    matched_count: 4,
  });
});

test("an amount at an interval comes to a monthly figure, and confirmed bills total per currency", () => {
  assert.equal(monthlyEquivalent(-1_000, "weekly"), -4_333);
  assert.equal(monthlyEquivalent(-1_000, "biweekly"), -2_167);
  assert.equal(monthlyEquivalent(-1_000, "monthly"), -1_000);
  assert.equal(monthlyEquivalent(-1_000, "quarterly"), -333);
  assert.equal(monthlyEquivalent(-1_000, "yearly"), -83);
  assert.deepEqual(
    monthlyTotal([
      bill("rent", "2026-09-01"),
      bill("netflix", "2026-09-01", { expected_amount: -1_299 }),
      bill("gym", "2026-09-01", { expected_amount: -3_000, interval: "weekly", currency: "EUR" }),
      bill("maybe", "2026-09-01", { expected_amount: -99_999, status: "detected" }),
    ]),
    [
      { currency: "EUR", total: -13_000 },
      { currency: "USD", total: -151_299 },
    ],
  );
});

test("occurrences in a month are measured from the anchor in whole intervals, so the day holds", () => {
  const endOfMonth = { next_date: "2026-03-31", interval: "monthly" };
  assert.deepEqual(occurrencesInMonth(endOfMonth, "2026-02"), ["2026-02-28"]);
  assert.deepEqual(occurrencesInMonth(endOfMonth, "2026-04"), ["2026-04-30"]);
  assert.deepEqual(occurrencesInMonth(endOfMonth, "2026-05"), ["2026-05-31"], "not drifted to the 28th");
  const weekly = { next_date: "2026-09-01", interval: "weekly" };
  assert.deepEqual(occurrencesInMonth(weekly, "2026-08"), ["2026-08-04", "2026-08-11", "2026-08-18", "2026-08-25"]);
  assert.deepEqual(occurrencesInMonth(weekly, "2026-09"), ["2026-09-01", "2026-09-08", "2026-09-15", "2026-09-22", "2026-09-29"]);
  const yearly = { next_date: "2026-09-15", interval: "yearly" };
  assert.deepEqual(occurrencesInMonth(yearly, "2026-03"), []);
  assert.deepEqual(occurrencesInMonth(yearly, "2028-09"), ["2028-09-15"]);
  assert.equal(previousOccurrence("2026-03-31", "monthly"), "2026-02-28");
  assert.equal(shiftOccurrence("2026-01-31", "monthly", 2), "2026-03-31");
  assert.equal(shiftOccurrence("2026-09-01", "biweekly", -1), "2026-08-18");
});
