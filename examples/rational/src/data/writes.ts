import type { RxCollection, RxDocument } from "rxdb";

import { randomId } from "../model/ids.js";
import type {
  Account,
  AccountType,
  BaseDocument,
  Category,
  CategoryKind,
  HouseholdCollectionId,
  RationalDocuments,
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
  const optional: { category_id?: string; notes?: string } = {};
  if (input.category_id !== undefined && input.category_id !== "") {
    optional.category_id = input.category_id;
  }
  if (input.notes !== undefined && input.notes.trim() !== "") optional.notes = input.notes.trim();
  return { ...fields, ...optional };
}
