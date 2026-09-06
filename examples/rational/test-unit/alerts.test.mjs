import assert from "node:assert/strict";
import { test } from "node:test";

import { alertId, firedAlerts } from "../functions/shared/alerts.ts";

/**
 * An alert is a promise: tell me about this, and do not tell me about
 * anything else. So the tests are mostly about restraint — what does *not*
 * fire — and about the derived ids that keep a condition still true tomorrow
 * from becoming a second alert.
 */

const HOUSEHOLD = "hh-1";

function subject(overrides = {}) {
  return {
    householdId: HOUSEHOLD,
    day: "2026-08-30",
    transactions: [],
    accounts: [],
    budgets: [],
    existingIds: new Set(),
    ...overrides,
  };
}

function spend(overrides = {}) {
  return {
    id: "txn-1",
    account_id: "acct-1",
    date: "2026-08-29",
    amount: -50_000,
    currency: "USD",
    description: "ROOF REPAIR",
    ...overrides,
  };
}

function account(overrides = {}) {
  return {
    id: "acct-1",
    name: "Everyday",
    type: "checking",
    currency: "USD",
    balance: 1_000,
    closed: false,
    ...overrides,
  };
}

function budget(overrides = {}) {
  return {
    id: "bud-1",
    category_id: "cat-1",
    month: "2026-08",
    amount: 20_000,
    spent: 25_000,
    currency: "USD",
    ...overrides,
  };
}

const LARGE = { alert_kind: "large_transaction", threshold: 40_000, enabled: true };
const OVER = { alert_kind: "budget_exceeded", threshold: 0, enabled: true };
const LOW = { alert_kind: "low_balance", threshold: 5_000, enabled: true };

test("a setting with no threshold fires nothing: a threshold nobody chose is not one", () => {
  const settings = [{ alert_kind: "large_transaction", enabled: true }];
  assert.deepEqual(firedAlerts(settings, subject({ transactions: [spend()] })), []);
});

test("a setting that is turned off fires nothing", () => {
  const off = [{ ...LARGE, enabled: false }];
  assert.deepEqual(firedAlerts(off, subject({ transactions: [spend()] })), []);
});

test("spending at or above the threshold fires; below it does not", () => {
  const at = firedAlerts([LARGE], subject({ transactions: [spend({ amount: -40_000 })] }));
  assert.equal(at.length, 1);
  assert.equal(at[0].alert_kind, "large_transaction");
  assert.equal(at[0].transaction_id, "txn-1");
  const below = firedAlerts([LARGE], subject({ transactions: [spend({ amount: -39_999 })] }));
  assert.deepEqual(below, []);
});

test("money arriving is never a large-transaction alert, however large", () => {
  const income = firedAlerts([LARGE], subject({ transactions: [spend({ amount: 900_000 })] }));
  assert.deepEqual(income, []);
});

test("an alert already fired is not fired again", () => {
  const [first] = firedAlerts([LARGE], subject({ transactions: [spend()] }));
  const again = firedAlerts(
    [LARGE],
    subject({ transactions: [spend()], existingIds: new Set([first.id]) }),
  );
  assert.deepEqual(again, []);
});

test("a budget over its allowance fires, and one within tolerance does not", () => {
  const over = firedAlerts([OVER], subject({ budgets: [budget()] }));
  assert.equal(over.length, 1);
  assert.equal(over[0].budget_id, "bud-1");
  assert.equal(over[0].amount, 5_000);
  const tolerant = firedAlerts(
    [{ ...OVER, threshold: 10_000 }],
    subject({ budgets: [budget()] }),
  );
  assert.deepEqual(tolerant, []);
});

test("a budget exactly on its allowance is not over it", () => {
  assert.deepEqual(firedAlerts([OVER], subject({ budgets: [budget({ spent: 20_000 })] })), []);
});

test("an account below the threshold fires once per day", () => {
  const fired = firedAlerts([LOW], subject({ accounts: [account()] }));
  assert.equal(fired.length, 1);
  assert.equal(fired[0].account_id, "acct-1");
  assert.equal(fired[0].id, alertId(HOUSEHOLD, "low", "acct-1.2026-08-30"));
  const tomorrow = firedAlerts(
    [LOW],
    subject({ accounts: [account()], day: "2026-08-31", existingIds: new Set([fired[0].id]) }),
  );
  assert.equal(tomorrow.length, 1, "a balance still low the next day is that day's alert");
});

test("a credit card is never low on money, and a closed account is not watched", () => {
  const card = firedAlerts(
    [LOW],
    subject({ accounts: [account({ type: "credit", balance: -90_000 })] }),
  );
  assert.deepEqual(card, []);
  const closed = firedAlerts([LOW], subject({ accounts: [account({ closed: true })] }));
  assert.deepEqual(closed, []);
});

test("every kind is decided in one pass, and each carries what it is about", () => {
  const fired = firedAlerts(
    [LARGE, OVER, LOW],
    subject({ transactions: [spend()], budgets: [budget()], accounts: [account()] }),
  );
  assert.deepEqual(
    fired.map((alert) => alert.alert_kind),
    ["large_transaction", "budget_exceeded", "low_balance"],
  );
  assert.equal(new Set(fired.map((alert) => alert.id)).size, 3);
  for (const alert of fired) assert.ok(!alert.id.includes(":"));
});

test("a transfer leg, a hidden charge, or a balance update is never a large transaction", () => {
  assert.deepEqual(firedAlerts([LARGE], subject({ transactions: [spend({ transfer_id: "tr_1" })] })), []);
  assert.deepEqual(firedAlerts([LARGE], subject({ transactions: [spend({ hidden: true })] })), []);
  assert.deepEqual(firedAlerts([LARGE], subject({ transactions: [spend({ adjustment: true })] })), []);
});

const BILL = { alert_kind: "bill_due", threshold: 3, enabled: true };
const GOAL = { alert_kind: "goal_reached", enabled: true };
const SYNC = { alert_kind: "sync_error", enabled: true };

function bill(overrides = {}) {
  return {
    recurrence_id: "rec-rent",
    name: "Rent",
    due_date: "2026-09-02",
    amount: -150_000,
    currency: "USD",
    days_away: 3,
    ...overrides,
  };
}

test("a bill due within the threshold's days fires once per occurrence; a late one is not coming due", () => {
  const fired = firedAlerts(
    [BILL],
    subject({
      bills: [
        bill(),
        bill({ recurrence_id: "rec-netflix", name: "Netflix", due_date: "2026-09-03", days_away: 4 }),
        bill({ recurrence_id: "rec-gym", name: "Gym", due_date: "2026-08-29", days_away: -1 }),
        bill({ recurrence_id: "rec-phone", name: "Phone", due_date: "2026-08-30", days_away: 0 }),
      ],
    }),
  );
  assert.deepEqual(
    fired.map((alert) => alert.recurrence_id),
    ["rec-rent", "rec-phone"],
  );
  assert.equal(fired[0].id, alertId(HOUSEHOLD, "bill", "rec-rent.2026-09-02"));
  assert.equal(fired[0].message, "Rent for 1500.00 is due on 2026-09-02");
  assert.equal(fired[0].amount, -150_000);
  // Tomorrow the same occurrence is the same alert; next month's is a new one.
  const tomorrow = firedAlerts(
    [BILL],
    subject({ bills: [bill({ days_away: 2 })], existingIds: new Set([fired[0].id]) }),
  );
  assert.deepEqual(tomorrow, []);
  assert.equal(firedAlerts([BILL], subject({ bills: [bill({ due_date: "2026-10-02" })] })).length, 1);
  // The threshold is days here, and a setting without one still fires nothing.
  assert.deepEqual(firedAlerts([{ alert_kind: "bill_due", enabled: true }], subject({ bills: [bill()] })), []);
});

test("a goal reached fires with no threshold to choose, once, and never while it is short", () => {
  const goals = [
    { id: "g-1", name: "Holiday", reached: true, target_amount: 200_000, currency: "USD" },
    { id: "g-2", name: "Roof", reached: false, target_amount: 900_000, currency: "USD" },
  ];
  const fired = firedAlerts([GOAL], subject({ goals }));
  assert.equal(fired.length, 1);
  assert.equal(fired[0].id, alertId(HOUSEHOLD, "goal", "g-1"));
  assert.equal(fired[0].goal_id, "g-1");
  assert.equal(fired[0].amount, 200_000);
  assert.match(fired[0].message, /Holiday/u);
  assert.deepEqual(firedAlerts([{ ...GOAL, enabled: false }], subject({ goals })), []);
  assert.deepEqual(firedAlerts([GOAL], subject({ goals, existingIds: new Set([fired[0].id]) })), []);
});

test("a failing connection is one alert, named after the institution and what went wrong", () => {
  const connections = [
    { id: "con-1", institution: "First Bank", failing: true, outcome: "ITEM_LOGIN_REQUIRED" },
    { id: "con-2", institution: "Credit Union", failing: false },
  ];
  const fired = firedAlerts([SYNC], subject({ connections }));
  assert.equal(fired.length, 1);
  assert.equal(fired[0].id, alertId(HOUSEHOLD, "sync", "con-1"));
  assert.equal(fired[0].connection_id, "con-1");
  assert.match(fired[0].message, /First Bank/u);
  assert.match(fired[0].message, /ITEM_LOGIN_REQUIRED/u);
  // Nothing is priced: the household's currency when known, ISO's "none" otherwise.
  assert.equal(fired[0].amount, 0);
  assert.equal(fired[0].currency, "XXX");
  assert.equal(firedAlerts([SYNC], subject({ connections, currency: "USD" }))[0].currency, "USD");
  const silent = firedAlerts([SYNC], subject({ connections: [{ id: "con-3", institution: "Bank", failing: true }] }));
  assert.match(silent[0].message, /failed/u);
});

test("a subject without bills, goals, or connections fires none of those alerts", () => {
  assert.deepEqual(firedAlerts([BILL, GOAL, SYNC], subject()), []);
});
