/**
 * Rational's `nightly` function: the work a household should not have to be
 * awake for.
 *
 * On a schedule at 02:00 UTC it walks every household and, for each:
 *
 *   - files transactions with the household's own rules -- a category where
 *     none was chosen, a merchant, tags, hiding, review -- recording which rule
 *     decided each one;
 *   - looks for synced transactions that duplicate a manual entry -- the
 *     institution and the person recorded the same purchase -- and marks the
 *     later one rather than deleting anything;
 *   - proposes repeating charges it has not proposed before;
 *   - advances a confirmed bill whose payment has landed, so a laptop left
 *     closed does not leave the rent "late" that the bank shows as paid;
 *   - decides the alerts the household asked for: a large charge, a budget
 *     over, an account low, a bill coming due, a goal reached;
 *   - writes the day's net-worth snapshot with every open account's balance,
 *     so a household -- and each of its accounts -- has a history rather than
 *     only a present.
 *
 * Everything is idempotent. A night that runs twice, or a run that fails
 * half-way and is retried, must leave the same household: snapshots are one
 * per household per day by id, a transaction already touched by a rule is left
 * alone, a bill advanced once no longer matches its payment, an alert has one
 * id per condition, and a duplicate already marked is not marked again.
 */
import {
  createServiceClient,
  type FunctionDocument,
  type JsonObject,
  type ServiceFunctionClient,
} from "@mako-cloud/edge-sdk";
import {
  type AlertAccount,
  type AlertBudget,
  type AlertSettingLike,
  type AlertTransaction,
  type FiredAlert,
  firedAlerts,
} from "../shared/alerts.ts";
import { type BudgetLike, type BudgetTransaction, budgetStatus } from "../shared/budgets.ts";
import {
  balancesOf,
  billSubjects,
  duplicates,
  filings,
  goalSubjects,
  netWorth,
  paidBills,
  snapshotBalances,
  snapshotId,
} from "../shared/nightly.ts";
import {
  type DetectedRecurrence,
  detectionId,
  detectRecurrences,
  type RecurrenceSubject,
} from "../shared/recurrences.ts";
import { RUN_KEY_HEADER, runKeyMatches } from "../shared/run-key.ts";
import { serviceCredential } from "./credential.ts";

declare const Deno: { readonly env: { get(name: string): string | undefined } };

const SCHEMA_VERSION = 3;
const TRANSACTIONS = "transactions";
const RULES = "rules";
const ACCOUNTS = "accounts";
const SNAPSHOTS = "net_worth_snapshots";
const RECURRENCES = "recurrences";
const BUDGETS = "budgets";
const GOALS = "goals";
const ALERTS = "alerts";
const HOUSEHOLDS = "households";

export default {
  async fetch(request: Request): Promise<Response> {
    if (request.method !== "POST") {
      return failure(405, "invalid_request", "nightly runs on POST");
    }
    try {
      authorizeRun(request);
      return await run(dayOf(request));
    } catch (error) {
      if (error instanceof RouteError) return failure(error.status, error.code, error.message);
      // The caller is told nothing but "it failed"; the operator is told
      // what -- including the message, which JSON serialization of an Error
      // drops (its own properties are not enumerable).
      console.error(error instanceof Error ? `${error.name}: ${error.message}` : error);
      return failure(500, "internal", "the nightly function failed");
    }
  },
};

interface HouseholdOutcome {
  readonly householdId: string;
  /** Transactions a rule touched tonight -- filed, tagged, hidden, or reviewed. */
  readonly categorized: number;
  readonly duplicatesMarked: number;
  readonly recurrencesDetected: number;
  readonly billsAdvanced: number;
  readonly alertsFired: number;
  readonly snapshot: string | null;
}

async function run(day: string): Promise<Response> {
  // Every household, as a range over the `updated` index. There is no way to
  // ask for a whole collection -- a query with no predicate names no index,
  // and the platform serves no unindexed scan -- so "everything" is spelled
  // "everything written since the epoch", which is the same set and is a
  // query the storage can actually plan.
  const households = await page("nightly maintenance", HOUSEHOLDS, [
    { field: "updated_at", operator: "gte", value: 0 },
  ]);
  const outcomes: HouseholdOutcome[] = [];
  for (const household of households) {
    if (household._deleted) continue;
    const householdId = String(household.body.id);
    try {
      outcomes.push(await runHousehold(householdId, String(household.body.currency), day));
    } catch (error) {
      // One household's failure is recorded and skipped: the rest still get
      // their night's work, which is the whole point of running unattended.
      const detail = error instanceof Error ? error.message.slice(0, 120) : "failed";
      outcomes.push({
        householdId,
        categorized: 0,
        duplicatesMarked: 0,
        recurrencesDetected: 0,
        billsAdvanced: 0,
        alertsFired: 0,
        snapshot: `error: ${detail}`,
      });
    }
  }
  return Response.json({ ok: true, day, households: outcomes.length, outcomes });
}

async function runHousehold(
  householdId: string,
  currency: string,
  day: string,
): Promise<HouseholdOutcome> {
  const scope = [{ field: "household_id", operator: "eq" as const, value: householdId }];
  const transactions = await page(`nightly categorization for ${householdId}`, TRANSACTIONS, scope);
  const rules = await page(`nightly categorization for ${householdId}`, RULES, scope);
  const accounts = await page(`nightly snapshot for ${householdId}`, ACCOUNTS, scope);
  // Read once, for the detection that must not propose a known bill again and
  // for the advance that moves a paid one on.
  const recurrences = await page(
    `nightly recurrence detection for ${householdId}`,
    RECURRENCES,
    scope,
  );

  const categorized = await categorize(householdId, transactions, rules);
  const duplicatesMarked = await markDuplicates(householdId, transactions);
  const recurrencesDetected = await detect(householdId, transactions, recurrences);
  const { advanced, count: billsAdvanced } = await advancePaidRecurrences(
    householdId,
    recurrences,
    transactions,
    day,
  );
  // Alerts are decided over the documents read at the top of this household's
  // pass. Tonight's filings are not in them, and need not be: no category
  // changes whether a charge was large, a budget over, or an account low. The
  // bills are the exception -- they are read after tonight's advance, or a
  // bill paid early would be announced as coming due.
  const alertsFired = await raiseAlerts(householdId, day, scope, accounts, transactions, advanced);
  const snapshot = await writeSnapshot(householdId, currency, day, accounts, transactions);
  return {
    householdId,
    categorized,
    duplicatesMarked,
    recurrencesDetected,
    billsAdvanced,
    alertsFired,
    snapshot,
  };
}

/**
 * Apply the rules, the same engine the application runs. A transaction that
 * already has a category is left with it even when a rule would choose
 * another: the person's own filing wins, and a nightly job that overruled it
 * would be unusable. The rule's other actions -- a merchant, tags, hiding,
 * marking reviewed -- are written whatever the engine decided, and a
 * transaction the same rule already touched is not touched again, so a member
 * who undid one of them is not fought every night.
 */
async function categorize(
  householdId: string,
  transactions: readonly FunctionDocument[],
  rules: readonly FunctionDocument[],
): Promise<number> {
  const decided = filings(transactions, rules, Date.now());
  for (const filing of decided) {
    await write(
      `nightly categorization for ${householdId}`,
      TRANSACTIONS,
      filing.documentId,
      filing.body,
      filing.revision,
    );
  }
  return decided.length;
}

/**
 * A synced transaction that repeats a manual one.
 *
 * The person wrote down the purchase and the institution then reported it, so
 * the household has it twice. The synced copy is the one marked -- it can be
 * reproduced from the institution, and the manual one carries whatever the
 * person typed. Nothing is deleted: a household decides that, and a nightly
 * job that removed a transaction it merely suspected would be the worst kind
 * of automation.
 */
async function markDuplicates(
  householdId: string,
  transactions: readonly FunctionDocument[],
): Promise<number> {
  const marked = duplicates(transactions, Date.now());
  for (const filing of marked) {
    await write(
      `nightly duplicate check for ${householdId}`,
      TRANSACTIONS,
      filing.documentId,
      filing.body,
      filing.revision,
    );
  }
  return marked.length;
}

/**
 * Repeating bills the household has not been told about.
 *
 * A detection is written down as a `detected` recurrence and nothing more --
 * the person confirms, adjusts, or dismisses it in Rational. Writing it down
 * is what lets a household that has not opened the app in a month still find
 * the subscription it forgot about; the id is derived from the account and
 * the normalized description, so two nights of the same detection are one
 * document, and a dismissal stays dismissed. Transfer legs, balance updates,
 * and hidden transactions travel with their flags so the engine can leave
 * them out: none of them is a charge.
 */
async function detect(
  householdId: string,
  transactions: readonly FunctionDocument[],
  known: readonly FunctionDocument[],
): Promise<number> {
  const reason = `nightly recurrence detection for ${householdId}`;
  const detections = detectRecurrences(
    transactions
      .filter((document) => !document._deleted)
      .map(
        (document): RecurrenceSubject => ({
          account_id: String(document.body.account_id ?? ""),
          description: String(document.body.description ?? ""),
          amount: Number(document.body.amount ?? 0),
          currency: String(document.body.currency ?? "USD"),
          date: String(document.body.date ?? ""),
          ...exclusionFlags(document.body),
        }),
      ),
    known
      .filter((document) => !document._deleted)
      .map((document) => ({
        account_id: String(document.body.account_id ?? ""),
        normalized_description: String(document.body.normalized_description ?? ""),
      })),
  );
  let written = 0;
  for (const detection of detections) {
    const id = detectionId(householdId, detection);
    const existing = await read(reason, RECURRENCES, id);
    if (existing !== null && !existing._deleted) continue;
    const now = Date.now();
    await write(reason, RECURRENCES, id, recurrenceBody(id, householdId, detection, now), null);
    written += 1;
  }
  return written;
}

function recurrenceBody(
  id: string,
  householdId: string,
  detection: DetectedRecurrence,
  now: number,
): JsonObject {
  return {
    id,
    household_id: householdId,
    created_at: now,
    updated_at: now,
    account_id: detection.accountId,
    normalized_description: detection.normalizedDescription,
    interval: detection.interval,
    expected_amount: detection.expectedAmount,
    currency: detection.currency,
    next_date: detection.nextDate,
    last_date: detection.lastDate,
    status: "detected",
    matched_count: detection.occurrences,
    source: "detected",
  };
}

/**
 * Confirmed bills whose payment has landed, moved on to their next date.
 *
 * The application does this too, when it is open; the job does it so a
 * household that has not opened Rational since the rent went out does not
 * find the rent "late" on a screen the bank contradicts. Each write names the
 * revision it read, and a bill advanced once no longer matches the same
 * payment, so a second night finds nothing to advance. The recurrences are
 * returned as they stand afterwards, for the alerts that read them next.
 */
async function advancePaidRecurrences(
  householdId: string,
  recurrences: readonly FunctionDocument[],
  transactions: readonly FunctionDocument[],
  day: string,
): Promise<{ readonly advanced: readonly FunctionDocument[]; readonly count: number }> {
  const reason = `nightly bill advance for ${householdId}`;
  const decided = paidBills(recurrences, transactions, day, Date.now());
  for (const filing of decided) {
    await write(reason, RECURRENCES, filing.documentId, filing.body, filing.revision);
  }
  const bodies = new Map(decided.map((filing) => [filing.documentId, filing.body]));
  const advanced = recurrences.map((document) => {
    const body = bodies.get(String(document.body.id));
    return body === undefined ? document : { ...document, body };
  });
  return { advanced, count: decided.length };
}

/**
 * The day's net worth: assets less liabilities, over open accounts not hidden
 * from it, from balances derived the same way the application derives them --
 * holdings valued at their last price included. One snapshot per household
 * per day, by id, so a night that runs twice writes one.
 *
 * Only accounts held in the household's own currency are counted. Rational
 * does not convert -- it has no rate anybody agreed to -- so a household with
 * a euro account gets a snapshot of its dollars rather than a sum of two
 * currencies pretending to be one number. The per-account balances are
 * recorded whatever the currency, and for hidden accounts too: an account
 * page wants its own history even when the household's figure leaves it out.
 */
async function writeSnapshot(
  householdId: string,
  currency: string,
  day: string,
  accounts: readonly FunctionDocument[],
  transactions: readonly FunctionDocument[],
): Promise<string> {
  const { assets, liabilities } = netWorth(currency, accounts, transactions);
  const id = snapshotId(householdId, day);
  const reason = `nightly snapshot for ${householdId}`;
  const existing = await read(reason, SNAPSHOTS, id);
  const now = Date.now();
  const body: JsonObject = {
    id,
    household_id: householdId,
    created_at: (existing?.body.created_at as number | undefined) ?? now,
    updated_at: now,
    date: day,
    assets,
    liabilities,
    net_worth: assets - liabilities,
    currency,
    balances: snapshotBalances(accounts, transactions),
  };
  await write(
    reason,
    SNAPSHOTS,
    id,
    body,
    existing === null || existing._deleted ? null : existing.revision,
  );
  return id;
}

/**
 * What the household asked to be told about.
 *
 * The evaluation happens here, and after each institution sync, because a
 * device that is closed would never fire an alert -- and the charge worth
 * telling somebody about is usually the one that arrived while nobody was
 * looking. Each alert has a derived id, so a condition that is still true
 * tomorrow night is the same alert rather than a second one.
 *
 * The bills and goals are shaped from tonight's recurrences and goals; the
 * connections are not this job's to judge -- the sync writes `sync_error`
 * itself, the moment a pass fails, with the connection as its subject.
 */
async function raiseAlerts(
  householdId: string,
  day: string,
  scope: ReadonlyArray<{ field: string; operator: "eq"; value: string }>,
  accounts: readonly FunctionDocument[],
  transactions: readonly FunctionDocument[],
  recurrences: readonly FunctionDocument[],
): Promise<number> {
  const reason = `nightly alerts for ${householdId}`;
  const stored = await page(reason, ALERTS, scope);
  const settings = stored
    .filter((document) => !document._deleted && document.body.kind === "setting")
    .map((document) => document.body as unknown as AlertSettingLike);
  if (settings.length === 0) return 0;
  const budgets = await page(reason, BUDGETS, scope);
  const goals = await page(reason, GOALS, scope);
  const fired = firedAlerts(settings, {
    householdId,
    day,
    transactions: alertTransactions(transactions),
    accounts: alertAccounts(accounts, transactions),
    budgets: alertBudgets(budgets, transactions),
    bills: billSubjects(recurrences, day),
    goals: goalSubjects(goals, balancesOf(accounts, transactions), day),
    connections: [],
    existingIds: new Set(
      stored.filter((document) => !document._deleted).map((document) => String(document.body.id)),
    ),
  });
  for (const alert of fired) {
    await writeAlert(reason, householdId, alert);
  }
  return fired.length;
}

async function writeAlert(reason: string, householdId: string, alert: FiredAlert): Promise<void> {
  const existing = await read(reason, ALERTS, alert.id);
  if (existing !== null && !existing._deleted) return;
  const now = Date.now();
  await write(
    reason,
    ALERTS,
    alert.id,
    {
      id: alert.id,
      household_id: householdId,
      created_at: now,
      updated_at: now,
      kind: "alert",
      alert_kind: alert.alert_kind,
      fired_at: now,
      message: alert.message,
      amount: alert.amount,
      currency: alert.currency,
      read: false,
      ...(alert.transaction_id === undefined ? {} : { transaction_id: alert.transaction_id }),
      ...(alert.account_id === undefined ? {} : { account_id: alert.account_id }),
      ...(alert.category_id === undefined ? {} : { category_id: alert.category_id }),
      ...(alert.budget_id === undefined ? {} : { budget_id: alert.budget_id }),
      ...(alert.recurrence_id === undefined ? {} : { recurrence_id: alert.recurrence_id }),
      ...(alert.goal_id === undefined ? {} : { goal_id: alert.goal_id }),
      ...(alert.connection_id === undefined ? {} : { connection_id: alert.connection_id }),
    },
    null,
  );
}

/**
 * The three flags that keep a transaction out of spending, present only when
 * the document states them. A transfer leg, a balance update, or a hidden
 * charge is not spending, and every engine that counts spending leaves them
 * out -- but only if it is told.
 */
function exclusionFlags(body: JsonObject): {
  readonly hidden?: boolean;
  readonly adjustment?: boolean;
  readonly transfer_id?: string;
} {
  return {
    ...(body.hidden === true ? { hidden: true } : {}),
    ...(body.adjustment === true ? { adjustment: true } : {}),
    ...(typeof body.transfer_id === "string" && body.transfer_id !== ""
      ? { transfer_id: body.transfer_id }
      : {}),
  };
}

export function alertTransactions(
  transactions: readonly FunctionDocument[],
): readonly AlertTransaction[] {
  return transactions
    .filter((document) => !document._deleted)
    .map((document) => ({
      id: String(document.body.id),
      account_id: String(document.body.account_id ?? ""),
      date: String(document.body.date ?? ""),
      amount: Number(document.body.amount ?? 0),
      currency: String(document.body.currency ?? "USD"),
      description: String(document.body.description ?? ""),
      ...exclusionFlags(document.body),
    }));
}

/**
 * Each account with its balance derived the one way the application derives
 * it -- opening balance, every transaction, and the holdings' value -- so a
 * "low balance" is judged on the number the account page shows.
 */
export function alertAccounts(
  accounts: readonly FunctionDocument[],
  transactions: readonly FunctionDocument[],
): readonly AlertAccount[] {
  const balances = balancesOf(accounts, transactions);
  return accounts
    .filter((document) => !document._deleted)
    .map((document) => ({
      id: String(document.body.id),
      name: String(document.body.name ?? ""),
      type: String(document.body.type ?? ""),
      currency: String(document.body.currency ?? "USD"),
      balance: balances.get(String(document.body.id)) ?? 0,
      closed: document.body.closed_at !== undefined,
    }));
}

/** Each budget with what its month spent, allowance and rollover included. */
export function alertBudgets(
  budgets: readonly FunctionDocument[],
  transactions: readonly FunctionDocument[],
): readonly AlertBudget[] {
  const documents = budgets
    .filter((document) => !document._deleted)
    .map((document) => document.body as unknown as BudgetLike);
  const spending = transactions
    .filter((document) => !document._deleted)
    .map(
      (document): BudgetTransaction => ({
        date: String(document.body.date ?? ""),
        amount: Number(document.body.amount ?? 0),
        currency: String(document.body.currency ?? "USD"),
        ...(typeof document.body.category_id === "string"
          ? { category_id: document.body.category_id }
          : {}),
        splits: Array.isArray(document.body.splits)
          ? (document.body.splits as Array<{ category_id?: string; amount: number }>)
          : [],
        ...exclusionFlags(document.body),
      }),
    );
  return documents.map((budget) => {
    const status = budgetStatus(budget, documents, spending);
    return {
      id: budget.id,
      category_id: budget.category_id,
      month: budget.month,
      amount: status.allowance,
      spent: status.spent,
      currency: budget.currency,
    };
  });
}

/** Every document of a collection in this scope, following the cursor. */
async function page(
  reason: string,
  collectionId: string,
  predicates: ReadonlyArray<{ field: string; operator: "eq" | "gte"; value: string | number }>,
): Promise<readonly FunctionDocument[]> {
  const documents: FunctionDocument[] = [];
  let cursor: string | null = null;
  for (let request = 0; request < 50; request += 1) {
    const result = await service(reason)
      .documents(collectionId)
      .query({
        predicates: [...predicates],
        sort: [],
        cursor,
        limit: 200,
      });
    documents.push(...result.documents);
    cursor = result.nextCursor ?? null;
    if (cursor === null) break;
  }
  return documents;
}

function service(reason: string): ServiceFunctionClient {
  return createServiceClient({
    endpoint: environment("MAKO_API_URL"),
    projectId: environment("MAKO_PROJECT_ID"),
    environmentId: environment("MAKO_ENVIRONMENT_ID"),
    serviceCredential: serviceCredential(),
    reason,
    requestId: `req_${randomSuffix()}${randomSuffix()}`,
  });
}

async function read(
  reason: string,
  collectionId: string,
  documentId: string,
): Promise<FunctionDocument | null> {
  return service(reason).documents(collectionId).get(documentId);
}

async function write(
  reason: string,
  collectionId: string,
  documentId: string,
  body: JsonObject,
  expectedRevision: string | null,
): Promise<void> {
  await service(reason)
    .documents(collectionId)
    .mutate(documentId, {
      mutationId: `rational-nightly-${randomSuffix()}${randomSuffix()}`,
      operation: expectedRevision === null ? "create" : "update",
      expectedRevision,
      schemaVersion: SCHEMA_VERSION,
      body,
    });
}

/** The scheduler may name the day, so a test can ask for a fixed one. */
function dayOf(request: Request): string {
  const header = request.headers.get("x-rational-today");
  if (header !== null && /^\d{4}-\d{2}-\d{2}$/u.test(header)) return header;
  return new Date().toISOString().slice(0, 10);
}

function randomSuffix(): string {
  return crypto.randomUUID().replaceAll("-", "");
}

class RouteError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

/**
 * The run key this deployment was given, or "" when it has none -- in which
 * case nothing may run it, because a function that falls open when its secret
 * is missing has no gate at all.
 */
function runKey(): string {
  return Deno.env.get("RATIONAL_RUN_KEY") ?? "";
}

function authorizeRun(request: Request): void {
  if (!runKeyMatches(request.headers.get(RUN_KEY_HEADER), runKey())) {
    throw new RouteError(401, "unauthenticated", "this function runs on a schedule");
  }
}

function environment(name: string): string {
  const value = Deno.env.get(name);
  if (value === undefined || value === "") {
    throw new RouteError(500, "internal", `${name} is not configured for this function`);
  }
  return value;
}

function failure(status: number, code: string, message: string): Response {
  return Response.json({ error: { code, message } }, { status });
}
