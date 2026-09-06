/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * When a household should be told something.
 *
 * Alerts are decided on the server, in `nightly` and after each
 * `institution-sync` pass, because a device that is closed would never fire
 * them -- and the transaction worth telling somebody about is usually the one
 * that arrived while nobody was looking.
 *
 * Every alert has a derived id, so the same condition seen on two nights is
 * one alert rather than a pile of them. That is the whole idempotence story:
 * a job that fires an alert twice is a job people turn off.
 */

export type AlertKind =
  | "large_transaction"
  | "budget_exceeded"
  | "low_balance"
  | "bill_due"
  | "goal_reached"
  | "sync_error";

/** What the engine reads of an alert setting. */
export interface AlertSettingLike {
  readonly alert_kind: AlertKind;
  /** An amount for the money alerts; a number of days for `bill_due`; unused by the rest. */
  readonly threshold?: number;
  readonly enabled?: boolean;
}

/**
 * What the engine reads of a transaction. The three exclusion fields are the
 * test `exclusions.ts` states, repeated here because this file imports
 * nothing: a transfer leg, a balance update, or a hidden charge is not spending.
 */
export interface AlertTransaction {
  readonly id: string;
  readonly account_id: string;
  readonly date: string;
  readonly amount: number;
  readonly currency: string;
  readonly description: string;
  readonly hidden?: boolean;
  readonly adjustment?: boolean;
  readonly transfer_id?: string;
}

export interface AlertAccount {
  readonly id: string;
  readonly name: string;
  readonly type: string;
  readonly currency: string;
  /** Derived the way the application derives it: opening balance plus activity. */
  readonly balance: number;
  readonly closed: boolean;
}

export interface AlertBudget {
  readonly id: string;
  readonly category_id: string;
  readonly month: string;
  readonly amount: number;
  readonly spent: number;
  readonly currency: string;
}

/** What the engine reads of a confirmed bill. */
export interface AlertBill {
  readonly recurrence_id: string;
  readonly name: string;
  readonly due_date: string;
  readonly amount: number;
  readonly currency: string;
  /** Days from the evaluation day to the due date; negative once it has passed. */
  readonly days_away: number;
}

/** What the engine reads of a goal. */
export interface AlertGoal {
  readonly id: string;
  readonly name: string;
  readonly reached: boolean;
  readonly target_amount: number;
  readonly currency: string;
}

/** What the engine reads of an institution connection. */
export interface AlertConnection {
  readonly id: string;
  readonly institution: string;
  readonly failing: boolean;
  /** What the last sync said, when it said anything. */
  readonly outcome?: string;
}

/** One alert, decided but not yet written. */
export interface FiredAlert {
  readonly id: string;
  readonly alert_kind: AlertKind;
  readonly message: string;
  readonly amount: number;
  readonly currency: string;
  readonly transaction_id?: string;
  readonly account_id?: string;
  readonly category_id?: string;
  readonly budget_id?: string;
  readonly recurrence_id?: string;
  readonly goal_id?: string;
  readonly connection_id?: string;
}

export interface AlertSubject {
  readonly householdId: string;
  /** The day the evaluation is for; a low balance is one alert per day. */
  readonly day: string;
  readonly transactions: readonly AlertTransaction[];
  readonly accounts: readonly AlertAccount[];
  readonly budgets: readonly AlertBudget[];
  readonly bills?: readonly AlertBill[];
  readonly goals?: readonly AlertGoal[];
  readonly connections?: readonly AlertConnection[];
  /**
   * The household's currency, stamped on an alert about nothing priced (a
   * connection failing). Without it such an alert carries ISO 4217's own
   * code for "no currency".
   */
  readonly currency?: string;
  /** Alerts fired before now, so a condition already reported stays reported. */
  readonly existingIds: ReadonlySet<string>;
}

/** Accounts whose balance is money owed rather than money held. */
const LIABILITIES = ["credit", "loan", "other_liability"];

/** ISO 4217's code for a transaction involving no currency. */
const NO_CURRENCY = "XXX";

/**
 * Everything the household has asked to be told about and has not been told
 * yet, in the order the settings name.
 *
 * A money alert with no threshold, or any alert turned off, fires nothing: a
 * threshold nobody chose is not a threshold, and guessing one would make the
 * first night after enabling alerts a flood. A goal reached and a connection
 * failing have no threshold to choose, so being enabled is enough.
 */
export function firedAlerts(
  settings: readonly AlertSettingLike[],
  subject: AlertSubject,
): readonly FiredAlert[] {
  const fired: FiredAlert[] = [];
  for (const setting of settings) {
    if (setting.enabled === false) continue;
    if (setting.alert_kind === "goal_reached") {
      fired.push(...reachedGoals(subject));
      continue;
    }
    if (setting.alert_kind === "sync_error") {
      fired.push(...failingConnections(subject));
      continue;
    }
    if (typeof setting.threshold !== "number" || !Number.isFinite(setting.threshold)) continue;
    if (setting.alert_kind === "large_transaction") {
      fired.push(...largeTransactions(setting.threshold, subject));
    } else if (setting.alert_kind === "budget_exceeded") {
      fired.push(...exceededBudgets(setting.threshold, subject));
    } else if (setting.alert_kind === "low_balance") {
      fired.push(...lowBalances(setting.threshold, subject));
    } else if (setting.alert_kind === "bill_due") {
      fired.push(...dueBills(setting.threshold, subject));
    }
  }
  return fired.filter((alert) => !subject.existingIds.has(alert.id));
}

/** The same test `exclusions.ts` states; repeated because this file imports nothing. */
function isCounted(transaction: AlertTransaction): boolean {
  return (
    transaction.hidden !== true &&
    transaction.adjustment !== true &&
    (transaction.transfer_id === undefined || transaction.transfer_id === "")
  );
}

/**
 * Spending at or above the threshold. Money arriving is not alarming, so only
 * negative amounts count, and the threshold is read as a magnitude -- a
 * household that types 500.00 means "tell me about five hundred dollars",
 * whichever sign it thinks in. A transfer to savings or a revaluation of the
 * house is not spending, however large.
 */
function largeTransactions(threshold: number, subject: AlertSubject): readonly FiredAlert[] {
  const limit = Math.abs(threshold);
  const fired: FiredAlert[] = [];
  for (const transaction of subject.transactions) {
    if (transaction.amount >= 0 || !isCounted(transaction)) continue;
    if (Math.abs(transaction.amount) < limit) continue;
    fired.push({
      id: alertId(subject.householdId, "large", transaction.id),
      alert_kind: "large_transaction",
      message: `${transaction.description} on ${transaction.date}`,
      amount: transaction.amount,
      currency: transaction.currency,
      transaction_id: transaction.id,
      account_id: transaction.account_id,
    });
  }
  return fired;
}

/**
 * A budget spent past its limit, by at least the threshold's tolerance. The
 * threshold is how far over is worth a word: a household that set it to zero
 * hears about every overrun, one that set it to 20.00 does not hear about
 * being twelve cents over.
 */
function exceededBudgets(threshold: number, subject: AlertSubject): readonly FiredAlert[] {
  const tolerance = Math.abs(threshold);
  const fired: FiredAlert[] = [];
  for (const budget of subject.budgets) {
    const over = budget.spent - budget.amount;
    if (over < tolerance || over <= 0) continue;
    fired.push({
      id: alertId(subject.householdId, "budget", budget.id),
      alert_kind: "budget_exceeded",
      message: `${budget.month} is over its budget by ${minorUnits(over)}`,
      amount: over,
      currency: budget.currency,
      category_id: budget.category_id,
      budget_id: budget.id,
    });
  }
  return fired;
}

/**
 * An open asset account below the threshold, once per account per day. A
 * credit card is never "low": what its balance means is the opposite, and a
 * household told its card is low on money would rightly stop trusting the
 * alerts. The balance arrives derived, and a balance counts everything --
 * the exclusions are about spending, not about what the bank holds.
 */
function lowBalances(threshold: number, subject: AlertSubject): readonly FiredAlert[] {
  const fired: FiredAlert[] = [];
  for (const account of subject.accounts) {
    if (account.closed || LIABILITIES.includes(account.type)) continue;
    if (account.balance >= threshold) continue;
    fired.push({
      id: alertId(subject.householdId, "low", `${account.id}.${subject.day}`),
      alert_kind: "low_balance",
      message: `${account.name} is down to ${minorUnits(account.balance)}`,
      amount: account.balance,
      currency: account.currency,
      account_id: account.id,
    });
  }
  return fired;
}

/**
 * A bill due within the threshold's number of days, today included. The id
 * names the occurrence, so a bill is announced once per due date and again
 * when the next one comes round; a bill already past its date is not
 * "coming due" and is the recurring page's business as late.
 */
function dueBills(threshold: number, subject: AlertSubject): readonly FiredAlert[] {
  const days = Math.abs(threshold);
  const fired: FiredAlert[] = [];
  for (const bill of subject.bills ?? []) {
    if (bill.days_away < 0 || bill.days_away > days) continue;
    fired.push({
      id: alertId(subject.householdId, "bill", `${bill.recurrence_id}.${bill.due_date}`),
      alert_kind: "bill_due",
      message: `${bill.name} for ${minorUnits(Math.abs(bill.amount))} is due on ${bill.due_date}`,
      amount: bill.amount,
      currency: bill.currency,
      recurrence_id: bill.recurrence_id,
    });
  }
  return fired;
}

/** A goal that has arrived, once: the id is the goal's, and a goal is reached one time. */
function reachedGoals(subject: AlertSubject): readonly FiredAlert[] {
  const fired: FiredAlert[] = [];
  for (const goal of subject.goals ?? []) {
    if (!goal.reached) continue;
    fired.push({
      id: alertId(subject.householdId, "goal", goal.id),
      alert_kind: "goal_reached",
      message: `${goal.name} reached its target of ${minorUnits(goal.target_amount)}`,
      amount: goal.target_amount,
      currency: goal.currency,
      goal_id: goal.id,
    });
  }
  return fired;
}

/**
 * A connection whose last sync failed. The id is the connection's, so a
 * connection that keeps failing is one alert until somebody fixes it, and a
 * connection fixed and broken again is the same alert reopened by whoever
 * clears it. The sync function raises this; the rule lives here so the
 * nightly job and the sync agree on what a failing connection is.
 */
function failingConnections(subject: AlertSubject): readonly FiredAlert[] {
  const fired: FiredAlert[] = [];
  for (const connection of subject.connections ?? []) {
    if (!connection.failing) continue;
    const outcome =
      connection.outcome === undefined || connection.outcome === ""
        ? "the last sync failed"
        : connection.outcome;
    fired.push({
      id: alertId(subject.householdId, "sync", connection.id),
      alert_kind: "sync_error",
      message: `${connection.institution} is not syncing: ${outcome}`,
      amount: 0,
      currency: subject.currency ?? NO_CURRENCY,
      connection_id: connection.id,
    });
  }
  return fired;
}

/** `.` rather than `:`, as everything a function writes must be. */
export function alertId(householdId: string, kind: string, subject: string): string {
  return `alr_${householdId}.${kind}.${subject}`;
}

/** Minor units as a plain decimal, for a message that carries no locale. */
function minorUnits(amount: number): string {
  const sign = amount < 0 ? "-" : "";
  const absolute = Math.abs(amount);
  return `${sign}${Math.floor(absolute / 100)}.${String(absolute % 100).padStart(2, "0")}`;
}
