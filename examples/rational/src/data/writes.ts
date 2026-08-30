import type { RxCollection, RxDocument } from "rxdb";

import { randomId } from "../model/ids.js";
import type {
  Account,
  AccountType,
  BaseDocument,
  Budget,
  Category,
  CategoryKind,
  ConnectionDocument,
  HouseholdCollectionId,
  RationalDocuments,
  Rule,
  Split,
  Tag,
  TaxonomyEntry,
  Transaction,
} from "../model/types.js";
import { isCurrencyCode } from "../selectors/money.js";
import { isIsoDate, normalizeDescription } from "../selectors/transactions.js";
import { validateSplits } from "../selectors/splits.js";
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
