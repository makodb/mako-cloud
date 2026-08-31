import type { RxCollection, RxDocument } from "rxdb";

import { randomId } from "../model/ids.js";
import type {
  Account,
  AccountType,
  Alert,
  AlertKind,
  AlertSetting,
  BaseDocument,
  Budget,
  Category,
  CategoryKind,
  ConnectionDocument,
  Goal,
  HouseholdCollectionId,
  RationalDocuments,
  Recurrence,
  Rule,
  Split,
  Tag,
  TaxonomyEntry,
  Transaction,
} from "../model/types.js";
import { isCurrencyCode } from "../selectors/money.js";
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

export interface AccountInput {
  readonly name: string;
  readonly type: AccountType;
  readonly currency: string;
  readonly opening_balance: number;
  readonly opening_date: string;
  readonly institution?: string;
}

/** One row an import is about to write. */
export interface ImportRow {
  readonly date: string;
  readonly description: string;
  readonly amount: number;
  readonly categoryId?: string;
  readonly ruleId?: string;
  readonly tags?: readonly string[];
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
  readonly category_id: string;
  readonly month: string;
  readonly amount: number;
  readonly currency: string;
  readonly rollover: boolean;
}

export interface AlertSettingInput {
  readonly alert_kind: AlertKind;
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
}

export interface GoalInput {
  readonly name: string;
  readonly target_amount: number;
  readonly currency: string;
  readonly target_date?: string;
  readonly account_id?: string;
}

export interface RuleInput {
  readonly name: string;
  readonly match: {
    readonly description_contains?: string;
    readonly amount_min?: number;
    readonly amount_max?: number;
    readonly account_id?: string;
  };
  readonly set_category_id?: string;
  readonly add_tags?: readonly string[];
  readonly priority: number;
}

export interface TransactionInput {
  readonly account_id: string;
  readonly date: string;
  readonly amount: number;
  readonly currency: string;
  readonly description: string;
  readonly category_id?: string;
  readonly tags?: readonly string[];
  readonly notes?: string;
  readonly splits?: readonly Split[];
  /** Set by an import, so a transaction can say where it came from. */
  readonly import_batch_id?: string;
  /** Set when a rule categorized it, so the screen can say which rule. */
  readonly rule_id?: string;
}

export class HouseholdWrites {
  readonly #context: WriteContext;

  constructor(context: WriteContext) {
    this.#context = context;
  }

  async createAccount(input: AccountInput): Promise<Account> {
    validateAccount(input);
    const document = this.#stamp<Account>(randomId("acc"), {
      name: input.name.trim(),
      type: input.type,
      currency: input.currency,
      opening_balance: input.opening_balance,
      opening_date: input.opening_date,
      ...(input.institution === undefined || input.institution.trim() === ""
        ? {}
        : { institution: input.institution.trim() }),
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
    return this.#patch("accounts", id, patch);
  }

  async closeAccount(id: string): Promise<Account> {
    return this.#patch("accounts", id, { closed_at: this.#context.now() });
  }

  async reopenAccount(id: string): Promise<Account> {
    return this.#patch("accounts", id, { closed_at: null });
  }

  async createTransaction(input: TransactionInput): Promise<Transaction> {
    const fields = validateTransaction(input);
    const document = this.#stamp<Transaction>(randomId("txn"), fields);
    return this.#insert("transactions", document);
  }

  /**
   * Import a batch of parsed rows, with the record of what was imported.
   *
   * The batch document is written first and every transaction names it, so a
   * person can see where a transaction came from and an import that fails
   * part-way leaves a batch whose count says how far it got rather than a
   * pile of unexplained rows.
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
        ...(row.categoryId === undefined ? {} : { category_id: row.categoryId }),
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
    const merged = applyPatch(current.toJSON(), patch);
    const fields = validateTransaction(merged);
    return this.#patch(
      "transactions",
      id,
      { ...patch, ...fields } as Patch<Transaction>,
      updatedAt,
    );
  }

  async deleteTransaction(id: string): Promise<void> {
    const document = await this.#require("transactions", id);
    this.#context.noteLocalWrite();
    await document.incrementalPatch({ updated_at: this.#context.now() });
    await document.incrementalRemove();
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
      }),
    );
  }

  async updateRecurrence(id: string, patch: Patch<Recurrence>): Promise<Recurrence> {
    return this.#patch("recurrences", id, patch);
  }

  /**
   * The household's standing instruction about one kind of alert. One setting
   * per kind, by id, because two thresholds for the same question is not a
   * thing a person means.
   */
  async saveAlertSetting(input: AlertSettingInput): Promise<AlertSetting> {
    if (!Number.isSafeInteger(input.threshold) || input.threshold < 0) {
      throw new ValidationError("a threshold is a whole, non-negative amount");
    }
    const id = alertSettingId(input.alert_kind);
    const existing = await this.#collection("alerts").findOne(id).exec();
    if (existing !== null) {
      return (await this.#patch("alerts", id, {
        threshold: input.threshold,
        enabled: input.enabled,
      } as Patch<AlertSetting>)) as AlertSetting;
    }
    return (await this.#insert(
      "alerts",
      this.#stamp<AlertSetting>(id, {
        kind: "setting",
        alert_kind: input.alert_kind,
        threshold: input.threshold,
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

  /**
   * A budget is one category in one month, so its id is derived from both:
   * two devices budgeting the same category in the same month write the same
   * document and the conflict handler settles it, rather than creating two
   * budgets nobody asked for.
   */
  async setBudget(input: BudgetInput): Promise<Budget> {
    if (input.category_id === "") throw new ValidationError("choose a category");
    if (!/^\d{4}-\d{2}$/u.test(input.month)) throw new ValidationError("month must be YYYY-MM");
    if (!Number.isSafeInteger(input.amount) || input.amount < 0) {
      throw new ValidationError("a budget is a whole, non-negative amount");
    }
    if (!isCurrencyCode(input.currency)) {
      throw new ValidationError("currency must be an ISO 4217 code such as USD");
    }
    const id = budgetId(input.category_id, input.month);
    const existing = await this.#collection("budgets").findOne(id).exec();
    if (existing !== null) {
      return this.#patch("budgets", id, {
        amount: input.amount,
        currency: input.currency,
        rollover: input.rollover,
      } as Patch<Budget>);
    }
    return this.#insert(
      "budgets",
      this.#stamp<Budget>(id, {
        category_id: input.category_id,
        month: input.month,
        amount: input.amount,
        currency: input.currency,
        rollover: input.rollover,
      }),
    );
  }

  async deleteBudget(categoryId: string, month: string): Promise<void> {
    const document = await this.#require("budgets", budgetId(categoryId, month));
    this.#context.noteLocalWrite();
    await document.incrementalPatch({ updated_at: this.#context.now() });
    await document.incrementalRemove();
  }

  async createRule(input: RuleInput): Promise<Rule> {
    if (input.name.trim() === "") throw new ValidationError("a rule needs a name");
    const match = {
      ...(input.match.description_contains === undefined ||
      input.match.description_contains.trim() === ""
        ? {}
        : { description_contains: input.match.description_contains.trim() }),
      ...(input.match.amount_min === undefined ? {} : { amount_min: input.match.amount_min }),
      ...(input.match.amount_max === undefined ? {} : { amount_max: input.match.amount_max }),
      ...(input.match.account_id === undefined || input.match.account_id === ""
        ? {}
        : { account_id: input.match.account_id }),
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
      ...(input.set_category_id === undefined || input.set_category_id === ""
        ? {}
        : { set_category_id: input.set_category_id }),
      add_tags: [...(input.add_tags ?? [])],
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
    const document = await this.#require("rules", id);
    this.#context.noteLocalWrite();
    await document.incrementalPatch({ updated_at: this.#context.now() });
    await document.incrementalRemove();
  }

  /**
   * Categories and tags are one collection with a `kind` discriminator, so
   * these four helpers are the only place that has to know it.
   */
  async createCategory(name: string, categoryKind: CategoryKind): Promise<Category> {
    if (name.trim() === "") throw new ValidationError("a category needs a name");
    return (await this.#insert(
      "taxonomy",
      this.#stamp<TaxonomyEntry>(randomId("cat"), {
        kind: "category",
        name: name.trim(),
        category_kind: categoryKind,
      }),
    )) as Category;
  }

  async updateCategory(id: string, patch: Patch<TaxonomyEntry>): Promise<Category> {
    if (patch.name !== undefined && patch.name !== null && patch.name.trim() === "") {
      throw new ValidationError("a category needs a name");
    }
    return (await this.#patch("taxonomy", id, patch)) as Category;
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
    const document = await this.#require("taxonomy", id);
    this.#context.noteLocalWrite();
    await document.incrementalPatch({ updated_at: this.#context.now() });
    await document.incrementalRemove();
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
    this.#context.noteLocalWrite();
    const stamp = updatedAt ?? this.#context.now();
    const updated = await document.incrementalModify((current) => {
      const next = applyPatch(current as RationalDocuments[Id], patch) as RationalDocuments[Id] & {
        updated_at: number;
      };
      next.updated_at = stamp;
      return next;
    });
    return updated.toJSON() as RationalDocuments[Id];
  }

  async #require<Id extends HouseholdCollectionId>(
    collectionId: Id,
    id: string,
  ): Promise<RxDocument<RationalDocuments[Id]>> {
    const document = await this.#collection(collectionId).findOne(id).exec();
    if (document === null) throw new ValidationError(`${collectionId} ${id} does not exist`);
    return document;
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

function validateAccount(input: AccountInput): void {
  if (input.name.trim() === "") throw new ValidationError("an account needs a name");
  if (!isCurrencyCode(input.currency)) {
    throw new ValidationError("currency must be an ISO 4217 code such as USD");
  }
  if (!Number.isSafeInteger(input.opening_balance)) {
    throw new ValidationError("opening balance must be a whole number of minor units");
  }
  if (!isIsoDate(input.opening_date)) throw new ValidationError("opening date must be YYYY-MM-DD");
}

/** The fields of a transaction, validated, with derived fields filled in. */
/**
 * `.` rather than `:` on purpose, as with membership ids: a document id
 * containing a character `encodeURIComponent` escapes cannot be written from
 * an edge function (findings log #7c and #12), and the nightly job of Phase 3
 * writes budgets.
 */
export function budgetId(categoryId: string, month: string): string {
  return `bud_${categoryId}.${month}`;
}

/** One setting per kind: the id is the question, not an occurrence of it. */
export function alertSettingId(kind: AlertKind): string {
  return `als_${kind}`;
}

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
    notes?: string;
    import_batch_id?: string;
    rule_id?: string;
  } = {};
  if (input.category_id !== undefined && input.category_id !== "") {
    optional.category_id = input.category_id;
  }
  if (input.notes !== undefined && input.notes.trim() !== "") optional.notes = input.notes.trim();
  // Where the transaction came from and what filed it. A transaction that
  // says neither is one somebody typed, which is also worth being able to
  // tell apart.
  if (input.import_batch_id !== undefined && input.import_batch_id !== "") {
    optional.import_batch_id = input.import_batch_id;
  }
  if (input.rule_id !== undefined && input.rule_id !== "") optional.rule_id = input.rule_id;
  return { ...fields, ...optional };
}
