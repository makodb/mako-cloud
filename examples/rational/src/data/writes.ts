import type { MangoQuerySelector, RxCollection, RxDocument } from "rxdb";

import { defaultTaxonomyDocuments } from "../../scripts/default-taxonomy.mjs";
import { randomId } from "../model/ids.js";
import {
  type Account,
  type AccountType,
  type Alert,
  type AlertKind,
  type AlertSetting,
  ASSET_CLASSES,
  type BaseDocument,
  type Budget,
  type BudgetBucket,
  type BudgetKind,
  type Category,
  type CategoryGroup,
  type CategoryKind,
  type ConnectionDocument,
  type Goal,
  type GoalKind,
  type GoalProgressSource,
  HOLDING_TYPES,
  type Holding,
  type HouseholdCollectionId,
  type Merchant,
  type RationalDocuments,
  type Recurrence,
  type Rule,
  type RuleDirection,
  type Split,
  type Tag,
  type TaxonomyEntry,
  type TaxonomyKind,
  TRACKED_TYPES,
  type Transaction,
} from "../model/types.js";
import { isCurrencyCode } from "../selectors/money.js";
import { nextOccurrence } from "../selectors/recurrences.js";
import { validateSplits } from "../selectors/splits.js";
import { isIsoDate, normalizeDescription } from "../selectors/transactions.js";
import type { RationalCollections } from "./database.js";

/**
 * Every write the screens make goes through here so the rules are in one
 * place: validation before anything touches the database, `updated_at`
 * stamped on every change, and the scope told that a write is pending.
 */
export class ValidationError extends Error {
  override readonly name = "ValidationError";
  readonly difference: number | undefined;

  constructor(message: string, difference?: number) {
    super(message);
    this.difference = difference;
  }
}

/** `null` in a patch removes the field; `undefined` leaves it alone. */
export type Patch<T extends BaseDocument> = {
  readonly [Key in Exclude<keyof T, keyof BaseDocument>]?: T[Key] | null;
};

export interface WriteContext {
  readonly collections: RationalCollections<HouseholdCollectionId>;
  readonly householdId: string;
  readonly now: () => number;
  readonly noteLocalWrite: () => void;
}

/** A holding as the editor supplies it; a new one gets its id here. */
export type HoldingInput = Omit<Holding, "id"> & { readonly id?: string };

export interface AccountInput {
  readonly name: string;
  readonly type: AccountType;
  readonly currency: string;
  readonly opening_balance: number;
  readonly opening_date: string;
  readonly institution?: string;
  readonly hide_from_net_worth?: boolean;
  readonly owner_id?: string;
  readonly holdings?: readonly HoldingInput[];
}

/** One row an import is about to write. */
export interface ImportRow {
  readonly date: string;
  readonly description: string;
  readonly amount: number;
  readonly categoryId?: string;
  readonly merchantId?: string;
  readonly ruleId?: string;
  readonly tags?: readonly string[];
  readonly hidden?: boolean;
  /** An import leaves its rows unreviewed unless a rule looked at them. */
  readonly reviewed?: boolean;
}

export interface ImportInput {
  readonly accountId: string;
  readonly currency: string;
  readonly filename: string;
  readonly rows: readonly ImportRow[];
  readonly rowCount: number;
  readonly duplicateCount: number;
}

/** What an import did, for the screen that ran it. */
export interface ImportOutcome {
  readonly batchId: string;
  readonly created: number;
  readonly duplicates: number;
  readonly rowCount: number;
  readonly finishedAt: number;
}

export interface BudgetInput {
  /** The category or group; ignored for the `income` and `flex` kinds. */
  readonly category_id: string;
  readonly month: string;
  readonly amount: number;
  readonly currency: string;
  readonly rollover: boolean;
  /** Absent means a category budget, as every budget was before schema 3. */
  readonly kind?: BudgetKind;
}

export interface AlertSettingInput {
  readonly alert_kind: AlertKind;
  /** An amount for the money alerts, a number of days for `bill_due`. */
  readonly threshold: number;
  readonly enabled: boolean;
}

export interface ConnectionInput {
  readonly account_id: string;
  readonly institution: string;
  /** The institution's own id for the account; the sync asks it by this. */
  readonly external_id: string;
}

export interface RecurrenceInput {
  readonly account_id: string;
  readonly normalized_description: string;
  readonly interval: Recurrence["interval"];
  readonly expected_amount: number;
  readonly currency: string;
  readonly next_date: string;
  readonly last_date?: string;
  readonly status: Recurrence["status"];
  readonly matched_count: number;
  readonly source?: Recurrence["source"];
  readonly name?: string;
  readonly category_id?: string;
  readonly merchant_id?: string;
  readonly last_paid_transaction_id?: string;
}

/** The advance the bill engine computed once a recurrence's transaction arrived. */
export interface BillPayment {
  readonly next_date: string;
  readonly last_date: string;
  readonly last_paid_transaction_id: string;
  readonly matched_count: number;
}

export interface GoalInput {
  readonly name: string;
  readonly target_amount: number;
  readonly currency: string;
  readonly target_date?: string;
  readonly account_id?: string;
  readonly kind?: GoalKind;
  readonly account_ids?: readonly string[];
  readonly planned_monthly?: number;
  readonly priority?: number;
  readonly progress_source?: GoalProgressSource;
  readonly starting_balance?: number;
}

export interface RuleInput {
  readonly name: string;
  readonly match: {
    readonly description_contains?: string;
    readonly description_equals?: string;
    readonly merchant_id?: string;
    readonly category_id?: string;
    readonly direction?: RuleDirection;
    readonly amount_min?: number;
    readonly amount_max?: number;
    readonly account_id?: string;
  };
  readonly set_category_id?: string;
  readonly set_merchant_id?: string;
  readonly add_tags?: readonly string[];
  readonly hide?: boolean;
  readonly mark_reviewed?: boolean;
  readonly priority: number;
}

export interface TransactionInput {
  readonly account_id: string;
  readonly date: string;
  readonly amount: number;
  readonly currency: string;
  readonly description: string;
  readonly category_id?: string;
  readonly merchant_id?: string;
  readonly tags?: readonly string[];
  readonly notes?: string;
  readonly splits?: readonly Split[];
  /** Set by an import, so a transaction can say where it came from. */
  readonly import_batch_id?: string;
  /** Set when a rule categorized it, so the screen can say which rule. */
  readonly rule_id?: string;
  readonly recurrence_id?: string;
  readonly transfer_id?: string;
  /**
   * Absent means "needs review". `createTransaction` sets it unless the
   * caller passes `false`, which is how an import or a sync says the
   * household has not looked at this yet; `false` itself is never stored.
   */
  readonly reviewed?: boolean;
  readonly hidden?: boolean;
  readonly adjustment?: boolean;
}

/** What creating a category may say beyond its name and kind. */
export interface CategoryOptions {
  readonly groupId?: string;
  readonly icon?: string;
  readonly budgetBucket?: BudgetBucket;
  readonly targetAmount?: number;
  readonly targetMonths?: number;
}

/** What deleting a category touched besides the category itself. */
export interface CategoryDeletion {
  readonly transactions: number;
  readonly rules: number;
  readonly budgets: number;
}

/** The description every balance update carries, so screens can name it. */
export const BALANCE_UPDATE_DESCRIPTION = "Balance update";

/** The schema's caps on the embedded lists, refused here rather than by the store. */
const MAX_HOLDINGS = 256;
const MAX_PATTERNS = 64;
const MAX_GOAL_ACCOUNTS = 16;
const MAX_TARGET_MONTHS = 60;
const MAX_BILL_DUE_DAYS = 365;

export class HouseholdWrites {
  readonly #context: WriteContext;

  constructor(context: WriteContext) {
    this.#context = context;
  }

  async createAccount(input: AccountInput): Promise<Account> {
    validateAccount(input);
    const holdings = (input.holdings ?? []).map(validateHolding);
    const document = this.#stamp<Account>(randomId("acc"), {
      name: input.name.trim(),
      type: input.type,
      currency: input.currency,
      opening_balance: input.opening_balance,
      opening_date: input.opening_date,
      ...(input.institution === undefined || input.institution.trim() === ""
        ? {}
        : { institution: input.institution.trim() }),
      ...(input.hide_from_net_worth === true ? { hide_from_net_worth: true } : {}),
      ...(input.owner_id === undefined || input.owner_id.trim() === ""
        ? {}
        : { owner_id: input.owner_id.trim() }),
      ...(holdings.length === 0 ? {} : { holdings }),
    });
    return this.#insert("accounts", document);
  }

  async updateAccount(id: string, patch: Patch<Account>): Promise<Account> {
    if (patch.name !== undefined && patch.name !== null && patch.name.trim() === "") {
      throw new ValidationError("an account needs a name");
    }
    if (
      patch.currency !== undefined &&
      patch.currency !== null &&
      !isCurrencyCode(patch.currency)
    ) {
      throw new ValidationError("currency must be an ISO 4217 code such as USD");
    }
    if (
      patch.opening_balance !== undefined &&
      patch.opening_balance !== null &&
      !Number.isSafeInteger(patch.opening_balance)
    ) {
      throw new ValidationError("opening balance must be a whole number of minor units");
    }
    if (
      patch.opening_date !== undefined &&
      patch.opening_date !== null &&
      !isIsoDate(patch.opening_date)
    ) {
      throw new ValidationError("opening date must be YYYY-MM-DD");
    }
    if (patch.holdings === undefined || patch.holdings === null) {
      return this.#patch("accounts", id, patch);
    }
    const current = (await this.#require("accounts", id)).toJSON() as Account;
    requireHoldingType(typeof patch.type === "string" ? patch.type : current.type);
    if (patch.holdings.length > MAX_HOLDINGS) {
      throw new ValidationError(`an account holds at most ${MAX_HOLDINGS} positions`);
    }
    return this.#patch("accounts", id, { ...patch, holdings: patch.holdings.map(validateHolding) });
  }

  async closeAccount(id: string): Promise<Account> {
    return this.#patch("accounts", id, { closed_at: this.#context.now() });
  }

  async reopenAccount(id: string): Promise<Account> {
    return this.#patch("accounts", id, { closed_at: null });
  }

  /** Only `true` is stored; an account shown in net worth simply lacks the flag. */
  async setHideFromNetWorth(id: string, hidden: boolean): Promise<Account> {
    return this.#patch("accounts", id, { hide_from_net_worth: hidden ? true : null });
  }

  /**
   * Add a position or replace the one with the same id. Holdings live on the
   * account rather than in a collection of their own because the page-wide
   * collection cap is spent, and an account's positions are read together
   * anyway.
   */
  async upsertHolding(accountId: string, holding: HoldingInput): Promise<Account> {
    const account = (await this.#require("accounts", accountId)).toJSON() as Account;
    requireHoldingType(account.type);
    const next = validateHolding(holding);
    const holdings = account.holdings ?? [];
    const replaced = holdings.some((entry) => entry.id === next.id)
      ? holdings.map((entry) => (entry.id === next.id ? next : entry))
      : [...holdings, next];
    if (replaced.length > MAX_HOLDINGS) {
      throw new ValidationError(`an account holds at most ${MAX_HOLDINGS} positions`);
    }
    return this.#patch("accounts", accountId, { holdings: replaced });
  }

  async removeHolding(accountId: string, holdingId: string): Promise<Account> {
    const account = (await this.#require("accounts", accountId)).toJSON() as Account;
    const holdings = account.holdings ?? [];
    if (!holdings.some((entry) => entry.id === holdingId)) {
      throw new ValidationError(`holding ${holdingId} does not exist`);
    }
    return this.#patch("accounts", accountId, {
      holdings: holdings.filter((entry) => entry.id !== holdingId),
    });
  }

  /**
   * A price update is an edit of the account, not a transaction: the value of
   * a position moving is not money the household received or spent.
   */
  async updateHoldingPrice(
    accountId: string,
    holdingId: string,
    price: number,
    priceAsOf: string,
  ): Promise<Account> {
    if (!Number.isSafeInteger(price) || price < 0) {
      throw new ValidationError("a price is a whole, non-negative number of minor units");
    }
    if (!isIsoDate(priceAsOf)) throw new ValidationError("price date must be YYYY-MM-DD");
    const account = (await this.#require("accounts", accountId)).toJSON() as Account;
    const holdings = account.holdings ?? [];
    if (!holdings.some((entry) => entry.id === holdingId)) {
      throw new ValidationError(`holding ${holdingId} does not exist`);
    }
    return this.#patch("accounts", accountId, {
      holdings: holdings.map((entry) =>
        entry.id === holdingId ? { ...entry, price, price_as_of: priceAsOf } : entry,
      ),
    });
  }

  /**
   * Move a tracked account -- a house, a car -- to a new value. The change is
   * booked as a transaction flagged `adjustment` and `hidden`, so the balance
   * derivation, the snapshots, and the account's history stay one mechanism
   * while cash flow and budgets never see it as spending. An unchanged value
   * writes nothing: there is no difference to record.
   */
  async updateTrackedBalance(
    accountId: string,
    newBalance: number,
    date: string,
  ): Promise<Transaction | null> {
    if (!Number.isSafeInteger(newBalance)) {
      throw new ValidationError("a balance must be a whole number of minor units");
    }
    if (!isIsoDate(date)) throw new ValidationError("date must be YYYY-MM-DD");
    const account = (await this.#require("accounts", accountId)).toJSON() as Account;
    if (!TRACKED_TYPES.includes(account.type)) {
      throw new ValidationError(
        "only a tracked account's value is set in place; book a transaction",
      );
    }
    const booked = await this.#find("transactions", { account_id: accountId });
    const current = booked.reduce(
      (balance, document) => balance + document.amount,
      account.opening_balance,
    );
    const difference = newBalance - current;
    if (difference === 0) return null;
    return this.createTransaction({
      account_id: accountId,
      date,
      amount: difference,
      currency: account.currency,
      description: BALANCE_UPDATE_DESCRIPTION,
      tags: [],
      splits: [],
      adjustment: true,
      hidden: true,
      reviewed: true,
    });
  }

  /**
   * Something a member typed has been looked at by definition, so `reviewed`
   * is set unless the caller says otherwise -- an import or a sync passes
   * `false`, which is stored as the field's absence.
   */
  async createTransaction(input: TransactionInput): Promise<Transaction> {
    const fields = validateTransaction({ ...input, reviewed: input.reviewed ?? true });
    const document = this.#stamp<Transaction>(randomId("txn"), fields);
    return this.#insert("transactions", document);
  }

  /**
   * Import a batch of parsed rows, with the record of what was imported.
   *
   * The batch document is written first and every transaction names it, so a
   * person can see where a transaction came from and an import that fails
   * part-way leaves a batch whose count says how far it got rather than a
   * pile of unexplained rows. The rows arrive unreviewed: nobody in the
   * household has looked at them yet, and the review queue is where they go.
   */
  async importTransactions(input: ImportInput): Promise<ImportOutcome> {
    const batchId = randomId("imp");
    const batch = this.#stamp<ConnectionDocument>(batchId, {
      kind: "import",
      account_id: input.accountId,
      filename: input.filename.slice(0, 200),
      imported_at: this.#context.now(),
      row_count: input.rowCount,
      created_count: 0,
      duplicate_count: input.duplicateCount,
    });
    await this.#insert("connections", batch);
    let created = 0;
    for (const row of input.rows) {
      await this.createTransaction({
        account_id: input.accountId,
        date: row.date,
        amount: row.amount,
        currency: input.currency,
        description: row.description,
        tags: [...(row.tags ?? [])],
        splits: [],
        import_batch_id: batchId,
        reviewed: row.reviewed ?? false,
        ...(row.hidden === true ? { hidden: true } : {}),
        ...(row.categoryId === undefined ? {} : { category_id: row.categoryId }),
        ...(row.merchantId === undefined ? {} : { merchant_id: row.merchantId }),
        ...(row.ruleId === undefined ? {} : { rule_id: row.ruleId }),
      });
      created += 1;
    }
    const finished = await this.#patch("connections", batchId, {
      created_count: created,
    } as Patch<ConnectionDocument>);
    return {
      batchId,
      created,
      duplicates: input.duplicateCount,
      rowCount: input.rowCount,
      finishedAt: finished.updated_at,
    };
  }

  /**
   * `updatedAt` may be supplied by a test to stage a conflict deterministically;
   * the application always stamps the current time.
   */
  async updateTransaction(
    id: string,
    patch: Patch<Transaction>,
    updatedAt?: number,
  ): Promise<Transaction> {
    const current = await this.#require("transactions", id);
    return this.#reviseTransaction(current, patch, updatedAt);
  }

  async deleteTransaction(id: string): Promise<void> {
    await this.#remove("transactions", id);
  }

  /**
   * One patch over many transactions, each validated on its own. A stale
   * selection may name a transaction another device has since deleted; that
   * one is skipped rather than failing the rest, and the count says how many
   * were actually changed. `tags` in the patch replace; `addTagsToTransactions`
   * is the union.
   */
  async bulkPatchTransactions(ids: readonly string[], patch: Patch<Transaction>): Promise<number> {
    let patched = 0;
    for (const id of new Set(ids)) {
      const document = await this.#collection("transactions").findOne(id).exec();
      if (document === null) continue;
      await this.#reviseTransaction(document, patch);
      patched += 1;
    }
    return patched;
  }

  /** Add tags to each transaction, keeping the ones it has; returns how many gained one. */
  async addTagsToTransactions(ids: readonly string[], tagIds: readonly string[]): Promise<number> {
    const additions = [...new Set(tagIds)].filter((tag) => tag !== "");
    if (additions.length === 0) return 0;
    let changed = 0;
    for (const id of new Set(ids)) {
      const document = await this.#collection("transactions").findOne(id).exec();
      if (document === null) continue;
      const tags: readonly string[] = document.tags;
      if (additions.every((tag) => tags.includes(tag))) continue;
      await this.#modify(document, { tags: [...new Set([...tags, ...additions])] });
      changed += 1;
    }
    return changed;
  }

  /** `false` removes the flag: absence is what "needs review" means. */
  async markReviewed(ids: readonly string[], reviewed: boolean): Promise<number> {
    return this.bulkPatchTransactions(ids, { reviewed: reviewed ? true : null });
  }

  /** `false` removes the flag; a shown transaction simply lacks it. */
  async setHidden(ids: readonly string[], hidden: boolean): Promise<number> {
    return this.bulkPatchTransactions(ids, { hidden: hidden ? true : null });
  }

  async setMerchant(id: string, merchantId: string | null): Promise<Transaction> {
    const merchant = merchantId === null || merchantId === "" ? null : merchantId;
    if (merchant !== null) await this.#requireKind(merchant, "merchant");
    return this.updateTransaction(id, { merchant_id: merchant });
  }

  /**
   * Two legs become a transfer by sharing an id. Neither leg's category
   * changes -- the pairing, not a category, is what keeps them out of cash
   * flow -- and a leg already in a pair is refused rather than silently moved.
   */
  async pairTransfer(outflowId: string, inflowId: string): Promise<string> {
    if (outflowId === inflowId) throw new ValidationError("a transfer needs two transactions");
    const outflow = await this.#require("transactions", outflowId);
    const inflow = await this.#require("transactions", inflowId);
    if (outflow.account_id === inflow.account_id) {
      throw new ValidationError("a transfer moves money between two accounts");
    }
    if (outflow.transfer_id !== undefined || inflow.transfer_id !== undefined) {
      throw new ValidationError("one of them is already part of a transfer");
    }
    const transferId = randomId("trf");
    await this.#modify(outflow, { transfer_id: transferId });
    await this.#modify(inflow, { transfer_id: transferId });
    return transferId;
  }

  /** Undo a pairing on every leg that carries it; returns how many legs there were. */
  async unpairTransfer(transferId: string): Promise<number> {
    const legs = await this.#find("transactions", { transfer_id: transferId });
    for (const leg of legs) {
      await this.#modify(leg, { transfer_id: null });
    }
    return legs.length;
  }

  /**
   * Connect an account to the simulated institution.
   *
   * The connection is what the scheduled sync reads: it names the account to
   * write into and the institution's own id for it. Rational writes it from
   * the browser; from then on the function owns its `last_sync_at` and
   * outcome, which is why those are not set here.
   */
  async connectInstitution(input: ConnectionInput): Promise<ConnectionDocument> {
    if (input.account_id === "") throw new ValidationError("choose an account");
    if (input.institution.trim() === "") throw new ValidationError("name the institution");
    const externalId = input.external_id.trim();
    if (!/^[A-Za-z0-9_.-]{1,64}$/u.test(externalId)) {
      throw new ValidationError("the institution's account id may hold letters, digits, . _ and -");
    }
    return this.#insert(
      "connections",
      this.#stamp<ConnectionDocument>(randomId("con"), {
        kind: "institution",
        institution: input.institution.trim(),
        external_id: externalId,
        account_id: input.account_id,
        account_ids: [input.account_id],
        status: "connected",
      }),
    );
  }

  async setConnectionStatus(
    id: string,
    status: NonNullable<ConnectionDocument["status"]>,
  ): Promise<ConnectionDocument> {
    return this.#patch("connections", id, { status } as Patch<ConnectionDocument>);
  }

  /** Confirm a detected recurrence, or record that it was dismissed. */
  async saveRecurrence(input: RecurrenceInput): Promise<Recurrence> {
    if (input.account_id === "") throw new ValidationError("choose an account");
    if (input.normalized_description === "") {
      throw new ValidationError("a recurrence needs a description");
    }
    if (!isIsoDate(input.next_date)) throw new ValidationError("next date must be YYYY-MM-DD");
    if (input.last_date !== undefined && !isIsoDate(input.last_date)) {
      throw new ValidationError("last date must be YYYY-MM-DD");
    }
    const id = randomId("rec");
    return this.#insert(
      "recurrences",
      this.#stamp<Recurrence>(id, {
        account_id: input.account_id,
        normalized_description: input.normalized_description,
        interval: input.interval,
        expected_amount: input.expected_amount,
        currency: input.currency,
        next_date: input.next_date,
        ...(input.last_date === undefined ? {} : { last_date: input.last_date }),
        status: input.status,
        matched_count: input.matched_count,
        ...(input.source === undefined ? {} : { source: input.source }),
        ...(input.name === undefined || input.name.trim() === ""
          ? {}
          : { name: input.name.trim() }),
        ...(input.category_id === undefined || input.category_id === ""
          ? {}
          : { category_id: input.category_id }),
        ...(input.merchant_id === undefined || input.merchant_id === ""
          ? {}
          : { merchant_id: input.merchant_id }),
        ...(input.last_paid_transaction_id === undefined || input.last_paid_transaction_id === ""
          ? {}
          : { last_paid_transaction_id: input.last_paid_transaction_id }),
      }),
    );
  }

  async updateRecurrence(id: string, patch: Patch<Recurrence>): Promise<Recurrence> {
    return this.#patch("recurrences", id, patch);
  }

  /**
   * A member says a transaction repeats. Detection needs three occurrences to
   * be sure; a person needs none, so the recurrence starts confirmed, with
   * this transaction as its first and only occurrence and the next one an
   * interval on.
   */
  async markRecurring(
    transaction: Transaction,
    interval: Recurrence["interval"],
  ): Promise<Recurrence> {
    const normalized = normalizeDescription(transaction.description);
    if (normalized === "") {
      throw new ValidationError("the description has nothing a repeat could be matched on");
    }
    const recurrence = await this.saveRecurrence({
      account_id: transaction.account_id,
      normalized_description: normalized,
      interval,
      expected_amount: transaction.amount,
      currency: transaction.currency,
      next_date: nextOccurrence(transaction.date, interval),
      last_date: transaction.date,
      status: "confirmed",
      matched_count: 1,
      source: "manual",
      last_paid_transaction_id: transaction.id,
      ...(transaction.category_id === undefined ? {} : { category_id: transaction.category_id }),
      ...(transaction.merchant_id === undefined ? {} : { merchant_id: transaction.merchant_id }),
    });
    await this.#patch("transactions", transaction.id, { recurrence_id: recurrence.id });
    return recurrence;
  }

  /**
   * The bill engine decided a transaction paid a recurrence and computed the
   * advance; this applies it and stamps the transaction so the two point at
   * each other. The engine, not this method, owns the decision: the browser
   * and the nightly job must reach it the same way.
   */
  async recordBillPaid(
    recurrenceId: string,
    transaction: Pick<Transaction, "id">,
    payment: BillPayment,
  ): Promise<Recurrence> {
    if (!isIsoDate(payment.next_date)) throw new ValidationError("next date must be YYYY-MM-DD");
    if (!isIsoDate(payment.last_date)) throw new ValidationError("last date must be YYYY-MM-DD");
    if (!Number.isSafeInteger(payment.matched_count) || payment.matched_count < 0) {
      throw new ValidationError("matched count must be a whole, non-negative number");
    }
    const recurrence = await this.#patch("recurrences", recurrenceId, {
      next_date: payment.next_date,
      last_date: payment.last_date,
      last_paid_transaction_id: payment.last_paid_transaction_id,
      matched_count: payment.matched_count,
    });
    await this.#patch("transactions", transaction.id, { recurrence_id: recurrenceId });
    return recurrence;
  }

  /** A paused bill is neither due nor late; unpausing confirms it again. */
  async pauseRecurrence(id: string, paused: boolean): Promise<Recurrence> {
    return this.#patch("recurrences", id, { status: paused ? "paused" : "confirmed" });
  }

  /**
   * The household's standing instruction about one kind of alert. One setting
   * per kind, by id, because two thresholds for the same question is not a
   * thing a person means. The threshold means different things by kind -- an
   * amount, a number of days, nothing at all -- so it is checked by kind.
   */
  async saveAlertSetting(input: AlertSettingInput): Promise<AlertSetting> {
    const threshold = validateAlertThreshold(input.alert_kind, input.threshold);
    const id = alertSettingId(input.alert_kind);
    const existing = await this.#collection("alerts").findOne(id).exec();
    if (existing !== null) {
      return (await this.#patch("alerts", id, {
        threshold,
        enabled: input.enabled,
      } as Patch<AlertSetting>)) as AlertSetting;
    }
    return (await this.#insert(
      "alerts",
      this.#stamp<AlertSetting>(id, {
        kind: "setting",
        alert_kind: input.alert_kind,
        threshold,
        enabled: input.enabled,
      }),
    )) as AlertSetting;
  }

  /**
   * Marking an alert read is the only thing a person does to one. Nothing
   * deletes an alert: the history is the point, and a household that fired an
   * alert and then lost the record of it has been told nothing.
   */
  async markAlertRead(id: string, read = true): Promise<Alert> {
    return (await this.#patch("alerts", id, { read } as Patch<Alert>)) as Alert;
  }

  async createGoal(input: GoalInput): Promise<Goal> {
    if (input.name.trim() === "") throw new ValidationError("a goal needs a name");
    if (!Number.isSafeInteger(input.target_amount) || input.target_amount <= 0) {
      throw new ValidationError("a goal needs a target above zero");
    }
    if (!isCurrencyCode(input.currency)) {
      throw new ValidationError("currency must be an ISO 4217 code such as USD");
    }
    if (input.target_date !== undefined && !isIsoDate(input.target_date)) {
      throw new ValidationError("target date must be YYYY-MM-DD");
    }
    const accountIds = [
      ...new Set(
        [
          ...(input.account_ids ?? []),
          ...(input.account_id === undefined ? [] : [input.account_id]),
        ].filter((id) => id !== ""),
      ),
    ];
    if (accountIds.length > MAX_GOAL_ACCOUNTS) {
      throw new ValidationError(`a goal follows at most ${MAX_GOAL_ACCOUNTS} accounts`);
    }
    // Paying down a debt is paying down some account's debt; and progress
    // read from a balance needs a balance to read.
    if (input.kind === "pay_down" && accountIds.length === 0) {
      throw new ValidationError("a pay-down goal needs the account it pays down");
    }
    if (input.progress_source === "balance" && accountIds.length === 0) {
      throw new ValidationError("progress by balance needs a linked account");
    }
    if (
      input.planned_monthly !== undefined &&
      (!Number.isSafeInteger(input.planned_monthly) || input.planned_monthly < 0)
    ) {
      throw new ValidationError("the planned monthly amount is a whole, non-negative amount");
    }
    if (
      input.priority !== undefined &&
      (!Number.isSafeInteger(input.priority) || input.priority < 0)
    ) {
      throw new ValidationError("priority is a whole, non-negative number");
    }
    if (input.starting_balance !== undefined && !Number.isSafeInteger(input.starting_balance)) {
      throw new ValidationError("the starting balance must be a whole number of minor units");
    }
    return this.#insert(
      "goals",
      this.#stamp<Goal>(randomId("goa"), {
        name: input.name.trim(),
        target_amount: input.target_amount,
        currency: input.currency,
        ...(input.target_date === undefined ? {} : { target_date: input.target_date }),
        ...(input.account_id === undefined || input.account_id === ""
          ? {}
          : { account_id: input.account_id }),
        status: "active",
        contributions: [],
        ...(input.kind === undefined ? {} : { kind: input.kind }),
        ...(accountIds.length === 0 ? {} : { account_ids: accountIds }),
        ...(input.planned_monthly === undefined ? {} : { planned_monthly: input.planned_monthly }),
        ...(input.priority === undefined ? {} : { priority: input.priority }),
        ...(input.progress_source === undefined ? {} : { progress_source: input.progress_source }),
        ...(input.starting_balance === undefined
          ? {}
          : { starting_balance: input.starting_balance }),
      }),
    );
  }

  /**
   * A contribution is appended to the goal's own list rather than derived
   * from an account balance: one account holds several goals, and a goal may
   * be saved for across accounts.
   */
  async contributeToGoal(
    goalId: string,
    contribution: { date: string; amount: number; note?: string },
  ): Promise<Goal> {
    if (!isIsoDate(contribution.date)) throw new ValidationError("date must be YYYY-MM-DD");
    if (!Number.isSafeInteger(contribution.amount) || contribution.amount === 0) {
      throw new ValidationError("a contribution needs an amount");
    }
    const current = await this.#require("goals", goalId);
    const goal = current.toJSON() as Goal;
    const contributions = [
      ...goal.contributions,
      {
        id: randomId("gct"),
        date: contribution.date,
        amount: contribution.amount,
        ...(contribution.note === undefined || contribution.note.trim() === ""
          ? {}
          : { note: contribution.note.trim() }),
      },
    ];
    const saved = contributions.reduce((total, entry) => total + entry.amount, 0);
    return this.#patch("goals", goalId, {
      contributions,
      ...(saved >= goal.target_amount && goal.status === "active"
        ? { status: "completed" as const }
        : {}),
    } as Patch<Goal>);
  }

  async updateGoal(id: string, patch: Patch<Goal>): Promise<Goal> {
    return this.#patch("goals", id, patch);
  }

  /** Priorities become the list's positions; a goal already in place is left alone. */
  async reorderGoals(orderedIds: readonly string[]): Promise<void> {
    let position = 0;
    for (const id of orderedIds) {
      const document = await this.#collection("goals").findOne(id).exec();
      if (document === null) continue;
      if (document.priority !== position) {
        await this.#modify(document, { priority: position });
      }
      position += 1;
    }
  }

  /**
   * A budget is one subject in one month, so its id is derived from both:
   * two devices budgeting the same category in the same month write the same
   * document and the conflict handler settles it, rather than creating two
   * budgets nobody asked for. The subject is the category or group, or the
   * kind itself for the household-level income and flex numbers -- stored in
   * `category_id` too, so the field every budget has keeps holding something.
   */
  async setBudget(input: BudgetInput): Promise<Budget> {
    const kind = input.kind ?? "category";
    const subject = budgetSubject(kind, input.category_id);
    if (subject === "") throw new ValidationError("choose a category");
    if (!/^\d{4}-\d{2}$/u.test(input.month)) throw new ValidationError("month must be YYYY-MM");
    if (!Number.isSafeInteger(input.amount) || input.amount < 0) {
      throw new ValidationError("a budget is a whole, non-negative amount");
    }
    if (!isCurrencyCode(input.currency)) {
      throw new ValidationError("currency must be an ISO 4217 code such as USD");
    }
    const id = budgetId(subject, input.month);
    const existing = await this.#collection("budgets").findOne(id).exec();
    if (existing !== null) {
      return this.#patch("budgets", id, {
        amount: input.amount,
        currency: input.currency,
        rollover: input.rollover,
        ...(input.kind === undefined ? {} : { kind: input.kind }),
      } as Patch<Budget>);
    }
    return this.#insert(
      "budgets",
      this.#stamp<Budget>(id, {
        category_id: subject,
        month: input.month,
        amount: input.amount,
        currency: input.currency,
        rollover: input.rollover,
        ...(input.kind === undefined ? {} : { kind: input.kind }),
      }),
    );
  }

  async deleteBudget(categoryId: string, month: string): Promise<void> {
    await this.#remove("budgets", budgetId(categoryId, month));
  }

  /**
   * Carry one month's budgets into another. Only what the target month lacks
   * is written -- a month somebody already started budgeting keeps every
   * number they typed -- and the count says how many arrived.
   */
  async copyBudgets(fromMonth: string, toMonth: string): Promise<number> {
    if (!/^\d{4}-\d{2}$/u.test(fromMonth) || !/^\d{4}-\d{2}$/u.test(toMonth)) {
      throw new ValidationError("month must be YYYY-MM");
    }
    if (fromMonth === toMonth) throw new ValidationError("choose a different month to copy into");
    const sources = await this.#find("budgets", { month: fromMonth });
    const present = new Set(
      (await this.#find("budgets", { month: toMonth })).map((document) => document.id),
    );
    let copied = 0;
    for (const source of sources) {
      const budget = source.toJSON() as Budget;
      const id = budgetId(budget.category_id, toMonth);
      if (present.has(id)) continue;
      await this.#insert(
        "budgets",
        this.#stamp<Budget>(id, {
          category_id: budget.category_id,
          month: toMonth,
          amount: budget.amount,
          currency: budget.currency,
          rollover: budget.rollover,
          ...(budget.kind === undefined ? {} : { kind: budget.kind }),
        }),
      );
      copied += 1;
    }
    return copied;
  }

  async createRule(input: RuleInput): Promise<Rule> {
    if (input.name.trim() === "") throw new ValidationError("a rule needs a name");
    const stated = (value: string | undefined): value is string =>
      value !== undefined && value.trim() !== "";
    // The exact-text condition is stored normalized because that is what the
    // engine compares it with: a rule made from a statement line has to hold
    // once the bank's reference numbers on next month's charge differ.
    const match = {
      ...(stated(input.match.description_contains)
        ? { description_contains: input.match.description_contains.trim() }
        : {}),
      ...(stated(input.match.description_equals)
        ? { description_equals: normalizeDescription(input.match.description_equals) }
        : {}),
      ...(stated(input.match.merchant_id) ? { merchant_id: input.match.merchant_id } : {}),
      ...(stated(input.match.category_id) ? { category_id: input.match.category_id } : {}),
      ...(input.match.direction === undefined ? {} : { direction: input.match.direction }),
      ...(input.match.amount_min === undefined ? {} : { amount_min: input.match.amount_min }),
      ...(input.match.amount_max === undefined ? {} : { amount_max: input.match.amount_max }),
      ...(stated(input.match.account_id) ? { account_id: input.match.account_id } : {}),
    };
    if (Object.keys(match).length === 0) {
      throw new ValidationError("a rule needs at least one condition");
    }
    if (
      match.amount_min !== undefined &&
      match.amount_max !== undefined &&
      match.amount_min > match.amount_max
    ) {
      throw new ValidationError("the smallest amount must not be above the largest");
    }
    const document = this.#stamp<Rule>(randomId("rul"), {
      name: input.name.trim(),
      match,
      ...(stated(input.set_category_id) ? { set_category_id: input.set_category_id } : {}),
      ...(stated(input.set_merchant_id) ? { set_merchant_id: input.set_merchant_id } : {}),
      add_tags: [...(input.add_tags ?? [])],
      ...(input.hide === true ? { hide: true } : {}),
      ...(input.mark_reviewed === true ? { mark_reviewed: true } : {}),
      priority: Number.isSafeInteger(input.priority) ? input.priority : 10,
      match_count: 0,
      enabled: true,
    });
    return this.#insert("rules", document);
  }

  async updateRule(id: string, patch: Patch<Rule>): Promise<Rule> {
    return this.#patch("rules", id, patch);
  }

  async deleteRule(id: string): Promise<void> {
    await this.#remove("rules", id);
  }

  /**
   * Categories, tags, groups, and merchants are one collection with a `kind`
   * discriminator, so the helpers from here down are the only place that has
   * to know it. The third argument is what a category may say beyond its name
   * and kind; the two-argument form is what every earlier caller uses.
   */
  async createCategory(
    name: string,
    categoryKind: CategoryKind,
    options: CategoryOptions = {},
  ): Promise<Category> {
    if (name.trim() === "") throw new ValidationError("a category needs a name");
    const extras = validateCategoryOptions(options);
    if (extras.parent_id !== undefined) await this.#requireGroup(extras.parent_id, categoryKind);
    return (await this.#insert(
      "taxonomy",
      this.#stamp<TaxonomyEntry>(randomId("cat"), {
        kind: "category",
        name: name.trim(),
        category_kind: categoryKind,
        ...extras,
      }),
    )) as Category;
  }

  async updateCategory(id: string, patch: Patch<TaxonomyEntry>): Promise<Category> {
    if (patch.name !== undefined && patch.name !== null && patch.name.trim() === "") {
      throw new ValidationError("a category needs a name");
    }
    validateTaxonomyPatch(patch);
    // An empty group is no group; and a category may only move into a group
    // of its own kind, or an income category would sit under Housing.
    const cleaned: Patch<TaxonomyEntry> =
      patch.parent_id === "" ? { ...patch, parent_id: null } : patch;
    if (typeof cleaned.parent_id === "string") {
      const current = await this.#requireKind(id, "category");
      const kind =
        typeof cleaned.category_kind === "string" ? cleaned.category_kind : current.category_kind;
      await this.#requireGroup(cleaned.parent_id, kind);
    }
    return (await this.#patch("taxonomy", id, cleaned)) as Category;
  }

  async createGroup(
    name: string,
    categoryKind: CategoryKind,
    sortOrder?: number,
  ): Promise<CategoryGroup> {
    if (name.trim() === "") throw new ValidationError("a group needs a name");
    if (sortOrder !== undefined && (!Number.isSafeInteger(sortOrder) || sortOrder < 0)) {
      throw new ValidationError("a position is a whole, non-negative number");
    }
    return (await this.#insert(
      "taxonomy",
      this.#stamp<TaxonomyEntry>(randomId("grp"), {
        kind: "group",
        name: name.trim(),
        category_kind: categoryKind,
        ...(sortOrder === undefined ? {} : { sort_order: sortOrder }),
      }),
    )) as CategoryGroup;
  }

  async updateGroup(id: string, patch: Patch<TaxonomyEntry>): Promise<CategoryGroup> {
    if (patch.name !== undefined && patch.name !== null && patch.name.trim() === "") {
      throw new ValidationError("a group needs a name");
    }
    validateTaxonomyPatch(patch);
    if (typeof patch.category_kind === "string") {
      const current = await this.#requireKind(id, "group");
      if (current.category_kind !== patch.category_kind) {
        const members = await this.#find("taxonomy", { kind: "category", parent_id: id });
        if (members.length > 0) {
          throw new ValidationError("move the group's categories before changing its kind");
        }
      }
    }
    return (await this.#patch("taxonomy", id, patch)) as CategoryGroup;
  }

  /** A group goes only once nothing files under it; the categories are not orphaned quietly. */
  async deleteGroup(id: string): Promise<void> {
    await this.#requireKind(id, "group");
    const members = await this.#find("taxonomy", { kind: "category", parent_id: id });
    if (members.length > 0) {
      throw new ValidationError(
        `the group still holds ${members.length} ${members.length === 1 ? "category" : "categories"}; move or delete them first`,
      );
    }
    await this.#remove("taxonomy", id);
  }

  /**
   * Delete a category and say where its transactions go. Everything that
   * named it -- transactions and their splits, rules that file into it,
   * budgets for it in any month -- is rewritten first, so nothing is left
   * pointing at a category that no longer exists. `null` leaves the
   * transactions uncategorized rather than moving them.
   */
  async deleteCategory(id: string, reassignTo: string | null): Promise<CategoryDeletion> {
    await this.#requireKind(id, "category");
    const target = reassignTo === null || reassignTo === "" ? null : reassignTo;
    if (target === id)
      throw new ValidationError("a category cannot take over its own transactions");
    if (target !== null) await this.#requireKind(target, "category");
    // Splits are inside the transaction, so a query cannot pick out the ones
    // that name the category; one pass over the collection can.
    let transactions = 0;
    for (const document of await this.#collection("transactions").find().exec()) {
      const transaction = document.toJSON() as Transaction;
      const direct = transaction.category_id === id;
      const inSplits = transaction.splits.some((split) => split.category_id === id);
      if (!direct && !inSplits) continue;
      await this.#modify(document, {
        ...(direct ? { category_id: target } : {}),
        ...(inSplits
          ? {
              splits: transaction.splits.map((split) =>
                split.category_id === id ? recategorizedSplit(split, target) : split,
              ),
            }
          : {}),
      });
      transactions += 1;
    }
    let rules = 0;
    for (const rule of await this.#find("rules", { set_category_id: id })) {
      await this.#modify(rule, { set_category_id: target });
      rules += 1;
    }
    let budgets = 0;
    for (const budget of await this.#find("budgets", { category_id: id })) {
      await this.#removeDocument(budget);
      budgets += 1;
    }
    await this.#remove("taxonomy", id);
    return { transactions, rules, budgets };
  }

  /** Positions become the list's order; an entry already in place is left alone. */
  async reorderCategories(orderedIds: readonly string[]): Promise<void> {
    let position = 0;
    for (const id of orderedIds) {
      const document = await this.#collection("taxonomy").findOne(id).exec();
      if (document === null) continue;
      if (document.sort_order !== position) {
        await this.#modify(document, { sort_order: position });
      }
      position += 1;
    }
  }

  /**
   * A merchant is a display name and the normalized descriptions that mean
   * it. Patterns are stored normalized so that matching a transaction is an
   * equality test against its own normalized description, never a second
   * normalization at read time.
   */
  async createMerchant(name: string, patterns: readonly string[] = []): Promise<Merchant> {
    if (name.trim() === "") throw new ValidationError("a merchant needs a name");
    const normalized = normalizePatterns(patterns);
    if (normalized.length > MAX_PATTERNS) {
      throw new ValidationError(`a merchant has at most ${MAX_PATTERNS} patterns`);
    }
    return (await this.#insert(
      "taxonomy",
      this.#stamp<TaxonomyEntry>(randomId("mer"), {
        kind: "merchant",
        name: name.trim(),
        patterns: normalized,
      }),
    )) as Merchant;
  }

  async updateMerchant(id: string, patch: Patch<TaxonomyEntry>): Promise<Merchant> {
    if (patch.name !== undefined && patch.name !== null && patch.name.trim() === "") {
      throw new ValidationError("a merchant needs a name");
    }
    const cleaned: Patch<TaxonomyEntry> =
      patch.patterns === undefined || patch.patterns === null
        ? patch
        : { ...patch, patterns: normalizePatterns(patch.patterns) };
    if ((cleaned.patterns?.length ?? 0) > MAX_PATTERNS) {
      throw new ValidationError(`a merchant has at most ${MAX_PATTERNS} patterns`);
    }
    return (await this.#patch("taxonomy", id, cleaned)) as Merchant;
  }

  /**
   * Fold one merchant into another: the winner learns the loser's patterns,
   * everything that named the loser -- transactions, recurrences, rules --
   * names the winner, and the loser goes. Returns how many transactions moved.
   */
  async mergeMerchants(loserId: string, winnerId: string): Promise<number> {
    if (loserId === winnerId) throw new ValidationError("choose two different merchants");
    const loser = await this.#requireKind(loserId, "merchant");
    const winner = await this.#requireKind(winnerId, "merchant");
    const patterns = normalizePatterns([...(winner.patterns ?? []), ...(loser.patterns ?? [])]);
    if (patterns.length > MAX_PATTERNS) {
      throw new ValidationError(`together they would have more than ${MAX_PATTERNS} patterns`);
    }
    if (patterns.length !== (winner.patterns ?? []).length) {
      await this.#patch("taxonomy", winnerId, { patterns });
    }
    let moved = 0;
    for (const document of await this.#find("transactions", { merchant_id: loserId })) {
      await this.#modify(document, { merchant_id: winnerId });
      moved += 1;
    }
    for (const document of await this.#find("recurrences", { merchant_id: loserId })) {
      await this.#modify(document, { merchant_id: winnerId });
    }
    for (const document of await this.#collection("rules").find().exec()) {
      const rule = document.toJSON() as Rule;
      const patch: Patch<Rule> = {
        ...(rule.match.merchant_id === loserId
          ? { match: { ...rule.match, merchant_id: winnerId } }
          : {}),
        ...(rule.set_merchant_id === loserId ? { set_merchant_id: winnerId } : {}),
      };
      if (Object.keys(patch).length === 0) continue;
      await this.#modify(document, patch);
    }
    await this.#remove("taxonomy", loserId);
    return moved;
  }

  /**
   * Give a household that has no taxonomy at all the default groups and
   * categories. The ids are deterministic, so two devices seeding the same
   * empty household at once write the same documents: locally, whichever
   * arrives second is a conflict on an existing id, which `bulkInsert` reports
   * rather than throws, and that is the "already exists" this skips. Anything
   * else -- a document the schema refuses -- is a defect and is raised.
   */
  async seedDefaultTaxonomy(): Promise<number> {
    const collection = this.#collection("taxonomy");
    const existing = await collection.find({ limit: 1 }).exec();
    if (existing.length > 0) return 0;
    const documents = defaultTaxonomyDocuments(this.#context.householdId, this.#context.now());
    const result = await collection.bulkInsert([...documents]);
    const failed = result.error.find((error) => error.status !== 409);
    if (failed !== undefined) {
      throw new Error(`seeding the default categories failed with status ${failed.status}`);
    }
    for (let written = 0; written < result.success.length; written += 1) {
      this.#context.noteLocalWrite();
    }
    return result.success.length;
  }

  async createTag(name: string): Promise<Tag> {
    if (name.trim() === "") throw new ValidationError("a tag needs a name");
    return (await this.#insert(
      "taxonomy",
      this.#stamp<TaxonomyEntry>(randomId("tag"), { kind: "tag", name: name.trim() }),
    )) as Tag;
  }

  async updateTag(id: string, patch: Patch<TaxonomyEntry>): Promise<Tag> {
    if (patch.name !== undefined && patch.name !== null && patch.name.trim() === "") {
      throw new ValidationError("a tag needs a name");
    }
    return (await this.#patch("taxonomy", id, patch)) as Tag;
  }

  async deleteTag(id: string): Promise<void> {
    await this.#remove("taxonomy", id);
  }

  /** Validate the transaction as it would be after the patch, then store the patch. */
  async #reviseTransaction(
    document: RxDocument<Transaction>,
    patch: Patch<Transaction>,
    updatedAt?: number,
  ): Promise<Transaction> {
    const merged = applyPatch(document.toJSON() as Transaction, patch);
    const fields = validateTransaction(merged);
    return this.#modify(document, transactionPatch(patch, fields), updatedAt);
  }

  async #requireKind(id: string, kind: TaxonomyKind): Promise<TaxonomyEntry> {
    const entry = (await this.#require("taxonomy", id)).toJSON() as TaxonomyEntry;
    if (entry.kind !== kind) throw new ValidationError(`${id} is not a ${kind}`);
    return entry;
  }

  async #requireGroup(groupId: string, categoryKind: CategoryKind | undefined): Promise<void> {
    const group = await this.#requireKind(groupId, "group");
    if (group.category_kind !== categoryKind) {
      throw new ValidationError(
        `${group.name} is a group of ${group.category_kind ?? "unknown"} categories`,
      );
    }
  }

  #stamp<T extends BaseDocument>(id: string, fields: Omit<T, keyof BaseDocument>): T {
    const at = this.#context.now();
    return {
      id,
      household_id: this.#context.householdId,
      created_at: at,
      updated_at: at,
      ...fields,
    } as T;
  }

  #collection<Id extends HouseholdCollectionId>(id: Id): RxCollection<RationalDocuments[Id]> {
    return this.#context.collections[id] as RxCollection<RationalDocuments[Id]>;
  }

  async #find<Id extends HouseholdCollectionId>(
    collectionId: Id,
    selector: MangoQuerySelector<RationalDocuments[Id]>,
  ): Promise<RxDocument<RationalDocuments[Id]>[]> {
    return this.#collection(collectionId).find({ selector }).exec();
  }

  async #insert<Id extends HouseholdCollectionId>(
    collectionId: Id,
    document: RationalDocuments[Id],
  ): Promise<RationalDocuments[Id]> {
    this.#context.noteLocalWrite();
    const inserted = await this.#collection(collectionId).insert(document);
    return inserted.toJSON() as RationalDocuments[Id];
  }

  async #patch<Id extends HouseholdCollectionId>(
    collectionId: Id,
    id: string,
    patch: Patch<RationalDocuments[Id]>,
    updatedAt?: number,
  ): Promise<RationalDocuments[Id]> {
    const document = await this.#require(collectionId, id);
    return this.#modify(document, patch, updatedAt);
  }

  async #modify<T extends BaseDocument>(
    document: RxDocument<T>,
    patch: Patch<T>,
    updatedAt?: number,
  ): Promise<T> {
    this.#context.noteLocalWrite();
    const stamp = updatedAt ?? this.#context.now();
    const updated = await document.incrementalModify((current) => {
      const next = applyPatch(current as T, patch) as T & { updated_at: number };
      next.updated_at = stamp;
      return next;
    });
    return updated.toJSON() as T;
  }

  async #require<Id extends HouseholdCollectionId>(
    collectionId: Id,
    id: string,
  ): Promise<RxDocument<RationalDocuments[Id]>> {
    const document = await this.#collection(collectionId).findOne(id).exec();
    if (document === null) throw new ValidationError(`${collectionId} ${id} does not exist`);
    return document;
  }

  async #remove<Id extends HouseholdCollectionId>(collectionId: Id, id: string): Promise<void> {
    await this.#removeDocument(await this.#require(collectionId, id));
  }

  /**
   * A deletion is stamped before it is a deletion, so the tombstone carries
   * the time it happened and the conflict handler can order it against an
   * edit from another device.
   */
  async #removeDocument<T extends BaseDocument>(document: RxDocument<T>): Promise<void> {
    this.#context.noteLocalWrite();
    await document.incrementalPatch({ updated_at: this.#context.now() } as Partial<T>);
    await document.incrementalRemove();
  }
}

export function applyPatch<T extends BaseDocument>(current: T, patch: Patch<T>): T {
  const next = { ...(current as object) } as Record<string, unknown>;
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) continue;
    if (value === null) {
      delete next[key];
    } else {
      next[key] = value;
    }
  }
  return next as T;
}

/**
 * The optional fields of a transaction that validation decides about: each
 * is stored when it says something and absent otherwise, so a patch that sets
 * one to its empty value -- a blank category, a flag turned off -- removes it.
 */
const OPTIONAL_TRANSACTION_FIELDS = [
  "category_id",
  "merchant_id",
  "notes",
  "import_batch_id",
  "rule_id",
  "recurrence_id",
  "transfer_id",
  "reviewed",
  "hidden",
  "adjustment",
] as const satisfies ReadonlyArray<Exclude<keyof Transaction, keyof BaseDocument>>;

/**
 * The patch to store once the merged transaction has been validated. The
 * validated fields win over what the patch said; an optional field the patch
 * mentioned that validation left out is removed rather than stored as its
 * empty value, so absence keeps its one meaning on every device.
 */
export function transactionPatch(
  patch: Patch<Transaction>,
  fields: Omit<Transaction, keyof BaseDocument>,
): Patch<Transaction> {
  const next: Record<string, unknown> = { ...patch, ...fields };
  for (const key of OPTIONAL_TRANSACTION_FIELDS) {
    if (patch[key] !== undefined && !(key in fields)) next[key] = null;
  }
  return next as Patch<Transaction>;
}

/** Merchant patterns as stored: normalized, non-empty, each once, in first-seen order. */
export function normalizePatterns(patterns: readonly string[]): string[] {
  return [...new Set(patterns.map(normalizeDescription).filter((pattern) => pattern !== ""))];
}

/**
 * What a budget's id names: the category or group for those kinds, the kind
 * itself for the household-level income and flex numbers.
 */
export function budgetSubject(kind: BudgetKind, categoryId: string): string {
  return kind === "income" || kind === "flex" ? kind : categoryId;
}

function requireHoldingType(type: AccountType): void {
  if (!HOLDING_TYPES.includes(type)) {
    throw new ValidationError("only investment and crypto accounts hold positions");
  }
}

function validateAccount(input: AccountInput): void {
  if (input.name.trim() === "") throw new ValidationError("an account needs a name");
  if (!isCurrencyCode(input.currency)) {
    throw new ValidationError("currency must be an ISO 4217 code such as USD");
  }
  if (!Number.isSafeInteger(input.opening_balance)) {
    throw new ValidationError("opening balance must be a whole number of minor units");
  }
  if (!isIsoDate(input.opening_date)) throw new ValidationError("opening date must be YYYY-MM-DD");
  if (input.holdings !== undefined && input.holdings.length > 0) {
    requireHoldingType(input.type);
    if (input.holdings.length > MAX_HOLDINGS) {
      throw new ValidationError(`an account holds at most ${MAX_HOLDINGS} positions`);
    }
  }
}

/** A holding as stored: trimmed, checked against the schema's ranges, with an id. */
export function validateHolding(input: HoldingInput): Holding {
  const symbol = input.symbol.trim();
  if (symbol === "" || symbol.length > 32) {
    throw new ValidationError("a symbol is 1 to 32 characters");
  }
  const name = input.name.trim();
  if (name === "") throw new ValidationError("a holding needs a name");
  if (!Number.isFinite(input.quantity) || input.quantity < 0) {
    throw new ValidationError("quantity must be a non-negative number");
  }
  if (!Number.isSafeInteger(input.price) || input.price < 0) {
    throw new ValidationError("a price is a whole, non-negative number of minor units");
  }
  if (
    input.cost_basis !== undefined &&
    (!Number.isSafeInteger(input.cost_basis) || input.cost_basis < 0)
  ) {
    throw new ValidationError("cost basis is a whole, non-negative number of minor units");
  }
  if (!ASSET_CLASSES.includes(input.asset_class)) {
    throw new ValidationError(`asset class must be one of ${ASSET_CLASSES.join(", ")}`);
  }
  if (input.price_as_of !== undefined && !isIsoDate(input.price_as_of)) {
    throw new ValidationError("price date must be YYYY-MM-DD");
  }
  return {
    id: input.id === undefined || input.id === "" ? randomId("hld") : input.id,
    symbol,
    name,
    quantity: input.quantity,
    price: input.price,
    asset_class: input.asset_class,
    ...(input.cost_basis === undefined ? {} : { cost_basis: input.cost_basis }),
    ...(input.price_as_of === undefined ? {} : { price_as_of: input.price_as_of }),
  };
}

/** The stored form of a category's options; the group's kind is checked by the caller. */
function validateCategoryOptions(options: CategoryOptions): {
  parent_id?: string;
  icon?: string;
  budget_bucket?: BudgetBucket;
  target_amount?: number;
  target_months?: number;
} {
  validateTaxonomyPatch({
    ...(options.icon === undefined ? {} : { icon: options.icon }),
    ...(options.targetAmount === undefined ? {} : { target_amount: options.targetAmount }),
    ...(options.targetMonths === undefined ? {} : { target_months: options.targetMonths }),
  });
  return {
    ...(options.groupId === undefined || options.groupId === ""
      ? {}
      : { parent_id: options.groupId }),
    ...(options.icon === undefined || options.icon.trim() === ""
      ? {}
      : { icon: options.icon.trim() }),
    ...(options.budgetBucket === undefined ? {} : { budget_bucket: options.budgetBucket }),
    ...(options.targetAmount === undefined ? {} : { target_amount: options.targetAmount }),
    ...(options.targetMonths === undefined ? {} : { target_months: options.targetMonths }),
  };
}

/** The schema's ranges for the fields a category, group, or merchant edit may carry. */
function validateTaxonomyPatch(patch: Patch<TaxonomyEntry>): void {
  if (patch.icon !== undefined && patch.icon !== null && patch.icon.trim().length > 16) {
    throw new ValidationError("an icon is at most 16 characters");
  }
  if (
    patch.target_amount !== undefined &&
    patch.target_amount !== null &&
    (!Number.isSafeInteger(patch.target_amount) || patch.target_amount < 0)
  ) {
    throw new ValidationError("a target is a whole, non-negative amount");
  }
  if (
    patch.target_months !== undefined &&
    patch.target_months !== null &&
    (!Number.isSafeInteger(patch.target_months) ||
      patch.target_months < 1 ||
      patch.target_months > MAX_TARGET_MONTHS)
  ) {
    throw new ValidationError(`a target spans 1 to ${MAX_TARGET_MONTHS} months`);
  }
  if (
    patch.sort_order !== undefined &&
    patch.sort_order !== null &&
    (!Number.isSafeInteger(patch.sort_order) || patch.sort_order < 0)
  ) {
    throw new ValidationError("a position is a whole, non-negative number");
  }
}

/**
 * What a threshold means depends on the alert: an amount for the money
 * alerts, a number of days ahead for a bill, and nothing for the alerts that
 * fire on an event -- those store zero so the field reads the same everywhere.
 */
export function validateAlertThreshold(kind: AlertKind, threshold: number): number {
  if (kind === "goal_reached" || kind === "sync_error") return 0;
  if (kind === "bill_due") {
    if (!Number.isSafeInteger(threshold) || threshold < 0 || threshold > MAX_BILL_DUE_DAYS) {
      throw new ValidationError(`days ahead is a whole number from 0 to ${MAX_BILL_DUE_DAYS}`);
    }
    return threshold;
  }
  if (!Number.isSafeInteger(threshold) || threshold < 0) {
    throw new ValidationError("a threshold is a whole, non-negative amount");
  }
  return threshold;
}

/** A split moved off a deleted category: to the replacement, or to none. */
function recategorizedSplit(split: Split, target: string | null): Split {
  return {
    id: split.id,
    amount: split.amount,
    ...(split.note === undefined ? {} : { note: split.note }),
    ...(target === null ? {} : { category_id: target }),
  };
}

/**
 * `.` rather than `:` on purpose, as with membership ids: a document id
 * containing a character `encodeURIComponent` escapes cannot be written from
 * an edge function (findings log #7c and #12), and the nightly job of Phase 3
 * writes budgets. `subject` is a category or group id, or `income` / `flex`.
 */
export function budgetId(subject: string, month: string): string {
  return `bud_${subject}.${month}`;
}

/** One setting per kind: the id is the question, not an occurrence of it. */
export function alertSettingId(kind: AlertKind): string {
  return `als_${kind}`;
}

/** The fields of a transaction, validated, with derived fields filled in. */
function validateTransaction(
  input: TransactionInput | Transaction,
): Omit<Transaction, keyof BaseDocument> {
  if (input.account_id === "") throw new ValidationError("choose an account");
  if (!isIsoDate(input.date)) throw new ValidationError("date must be YYYY-MM-DD");
  if (!Number.isSafeInteger(input.amount)) {
    throw new ValidationError("amount must be a whole number of minor units");
  }
  if (!isCurrencyCode(input.currency)) {
    throw new ValidationError("currency must be an ISO 4217 code such as USD");
  }
  const description = input.description.trim();
  if (description === "") throw new ValidationError("a transaction needs a description");
  const splits = (input.splits ?? []).map((split) => ({
    ...split,
    id: split.id === "" ? randomId("split") : split.id,
  }));
  const validation = validateSplits(input.amount, splits);
  if (!validation.ok) {
    throw new ValidationError(
      validation.reason === "invalid_amount"
        ? "every split needs an amount"
        : "splits must add up to the transaction amount",
      validation.difference,
    );
  }
  const fields: Omit<Transaction, keyof BaseDocument> = {
    account_id: input.account_id,
    date: input.date,
    amount: input.amount,
    currency: input.currency,
    description,
    normalized_description: normalizeDescription(description),
    tags: [...new Set(input.tags ?? [])],
    splits,
  };
  const optional: {
    category_id?: string;
    merchant_id?: string;
    notes?: string;
    import_batch_id?: string;
    rule_id?: string;
    recurrence_id?: string;
    transfer_id?: string;
    reviewed?: true;
    hidden?: true;
    adjustment?: true;
  } = {};
  if (input.category_id !== undefined && input.category_id !== "") {
    optional.category_id = input.category_id;
  }
  if (input.merchant_id !== undefined && input.merchant_id !== "") {
    optional.merchant_id = input.merchant_id;
  }
  if (input.notes !== undefined && input.notes.trim() !== "") optional.notes = input.notes.trim();
  // Where the transaction came from and what filed it. A transaction that
  // says neither is one somebody typed, which is also worth being able to
  // tell apart.
  if (input.import_batch_id !== undefined && input.import_batch_id !== "") {
    optional.import_batch_id = input.import_batch_id;
  }
  if (input.rule_id !== undefined && input.rule_id !== "") optional.rule_id = input.rule_id;
  if (input.recurrence_id !== undefined && input.recurrence_id !== "") {
    optional.recurrence_id = input.recurrence_id;
  }
  if (input.transfer_id !== undefined && input.transfer_id !== "") {
    optional.transfer_id = input.transfer_id;
  }
  // The flags are stored only when set: `reviewed: false` would be a second
  // way of saying "needs review", and two spellings of one state is how two
  // devices come to disagree about it.
  if (input.reviewed === true) optional.reviewed = true;
  if (input.hidden === true) optional.hidden = true;
  if (input.adjustment === true) optional.adjustment = true;
  return { ...fields, ...optional };
}
