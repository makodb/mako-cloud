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

export type AlertKind = "large_transaction" | "budget_exceeded" | "low_balance";

/** What the engine reads of an alert setting. */
export interface AlertSettingLike {
  readonly alert_kind: AlertKind;
  readonly threshold?: number;
  readonly enabled?: boolean;
}

export interface AlertTransaction {
  readonly id: string;
  readonly account_id: string;
  readonly date: string;
  readonly amount: number;
  readonly currency: string;
  readonly description: string;
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
}

export interface AlertSubject {
  readonly householdId: string;
  /** The day the evaluation is for; a low balance is one alert per day. */
  readonly day: string;
  readonly transactions: readonly AlertTransaction[];
  readonly accounts: readonly AlertAccount[];
  readonly budgets: readonly AlertBudget[];
  /** Alerts fired before now, so a condition already reported stays reported. */
  readonly existingIds: ReadonlySet<string>;
}

/** Accounts whose balance is money owed rather than money held. */
const LIABILITIES = ["credit_card", "loan"];

/**
 * Everything the household has asked to be told about and has not been told
 * yet, in the order the settings name.
 *
 * A setting with no threshold, or one turned off, fires nothing: a threshold
 * nobody chose is not a threshold, and guessing one would make the first
 * night after enabling alerts a flood.
 */
export function firedAlerts(
  settings: readonly AlertSettingLike[],
  subject: AlertSubject,
): readonly FiredAlert[] {
  const fired: FiredAlert[] = [];
  for (const setting of settings) {
    if (setting.enabled === false) continue;
    if (typeof setting.threshold !== "number" || !Number.isFinite(setting.threshold)) continue;
    if (setting.alert_kind === "large_transaction") {
      fired.push(...largeTransactions(setting.threshold, subject));
    } else if (setting.alert_kind === "budget_exceeded") {
      fired.push(...exceededBudgets(setting.threshold, subject));
    } else if (setting.alert_kind === "low_balance") {
      fired.push(...lowBalances(setting.threshold, subject));
    }
  }
  return fired.filter((alert) => !subject.existingIds.has(alert.id));
}

/**
 * Spending at or above the threshold. Money arriving is not alarming, so only
 * negative amounts count, and the threshold is read as a magnitude -- a
 * household that types 500.00 means "tell me about five hundred dollars",
 * whichever sign it thinks in.
 */
function largeTransactions(threshold: number, subject: AlertSubject): readonly FiredAlert[] {
  const limit = Math.abs(threshold);
  const fired: FiredAlert[] = [];
  for (const transaction of subject.transactions) {
    if (transaction.amount >= 0) continue;
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
 * alerts.
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
