/**
 * The pure half of Rational's Plaid integration: shapes in, decisions out,
 * no network and no secrets.
 *
 * Everything here is a translation between two honest vocabularies. Plaid
 * speaks floats in major units with positive-means-money-left; Rational
 * speaks integers in minor units with negative-means-money-left. Plaid's
 * `/transactions/sync` speaks added/modified/removed pages under a cursor;
 * the sync pass speaks the same statement entries the simulated institution
 * has always produced, plus a list of entries the institution has withdrawn
 * (a pending charge that posted comes back as one removal and one addition).
 *
 * The one rule with teeth: nothing this module returns for the `connections`
 * collection may carry a token. The access token's only projection is
 * `plaidItemBody`, which the function writes to a collection no application
 * user can read.
 */

export interface PlaidTransaction {
  readonly transaction_id: string;
  readonly account_id: string;
  /** Major units; positive means money left the account. */
  readonly amount: number;
  readonly iso_currency_code: string | null;
  readonly date: string;
  readonly name: string;
  readonly merchant_name?: string | null;
  readonly pending: boolean;
}

export interface PlaidSyncPage {
  readonly added: readonly PlaidTransaction[];
  readonly modified: readonly PlaidTransaction[];
  readonly removed: readonly { readonly transaction_id: string }[];
  readonly next_cursor: string;
  readonly has_more: boolean;
}

/**
 * An entry the sync pass writes: the simulated institution's statement shape
 * (`externalId`, `date`, `amount`, `description`) plus the currency Plaid
 * names. Shared modules import nothing, so the shape is stated here rather
 * than imported from the function that also uses it.
 */
export interface PlaidEntry {
  readonly externalId: string;
  readonly date: string;
  readonly amount: number;
  readonly currency: string;
  readonly description: string;
}

/**
 * Plaid's major-unit float, as Rational's minor-unit integer. The sign flips:
 * Plaid's positive is an outflow, and in Rational spending is negative. The
 * rounding guards against float representation (`12.34 * 100 === 1233.9999…`),
 * not against sub-cent amounts, which Plaid does not produce for card feeds.
 */
export function minorUnitsFromPlaid(amount: number): number {
  const minor = -Math.round(amount * 100);
  // A zero amount negates to -0, which is equal to 0 almost everywhere --
  // "almost" being exactly the kind of word a money field cannot carry.
  return minor === 0 ? 0 : minor;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asTransaction(value: unknown): PlaidTransaction {
  if (
    !isRecord(value) ||
    typeof value.transaction_id !== "string" ||
    value.transaction_id === "" ||
    typeof value.account_id !== "string" ||
    typeof value.amount !== "number" ||
    !Number.isFinite(value.amount) ||
    typeof value.date !== "string" ||
    typeof value.name !== "string"
  ) {
    throw new Error("the institution's answer does not look like a transaction");
  }
  return {
    transaction_id: value.transaction_id,
    account_id: value.account_id,
    amount: value.amount,
    iso_currency_code: typeof value.iso_currency_code === "string" ? value.iso_currency_code : null,
    date: value.date,
    name: value.name,
    merchant_name: typeof value.merchant_name === "string" ? value.merchant_name : null,
    pending: value.pending === true,
  };
}

/** One `/transactions/sync` page, checked rather than trusted. */
export function parseSyncPage(value: unknown): PlaidSyncPage {
  if (
    !isRecord(value) ||
    !Array.isArray(value.added) ||
    !Array.isArray(value.modified) ||
    !Array.isArray(value.removed) ||
    typeof value.next_cursor !== "string" ||
    typeof value.has_more !== "boolean"
  ) {
    throw new Error("the institution's answer does not look like a sync page");
  }
  const removed = value.removed.map((entry) => {
    if (
      !isRecord(entry) ||
      typeof entry.transaction_id !== "string" ||
      entry.transaction_id === ""
    ) {
      throw new Error("the institution's answer does not look like a removal");
    }
    return { transaction_id: entry.transaction_id };
  });
  return {
    added: value.added.map(asTransaction),
    modified: value.modified.map(asTransaction),
    removed,
    next_cursor: value.next_cursor,
    has_more: value.has_more,
  };
}

/**
 * What one page asks to be written, in the statement vocabulary the sync pass
 * already speaks: added and modified entries both become upserts, because the
 * pass writes by `(account, external_id)` and an update of the same id is the
 * same document. A transaction modified and re-modified within one page keeps
 * the last word.
 */
export function syncEntries(page: PlaidSyncPage, fallbackCurrency: string): readonly PlaidEntry[] {
  const byId = new Map<string, PlaidEntry>();
  for (const transaction of [...page.added, ...page.modified]) {
    byId.set(transaction.transaction_id, {
      externalId: transaction.transaction_id,
      date: transaction.date,
      amount: minorUnitsFromPlaid(transaction.amount),
      currency: transaction.iso_currency_code ?? fallbackCurrency,
      description: transaction.merchant_name ?? transaction.name,
    });
  }
  return [...byId.values()];
}

/**
 * Entries the institution has withdrawn. A removal beats an addition of the
 * same id in the same page: Plaid documents removed as authoritative, and a
 * deleted-then-kept transaction would double the money.
 */
export function removedIds(page: PlaidSyncPage): readonly string[] {
  return page.removed.map((entry) => entry.transaction_id);
}

/**
 * The connection's document id, derived from Plaid's item id so relinking the
 * same institution finds its own connection instead of growing a second one.
 */
export function plaidConnectionId(itemId: string): string {
  return `con_plaid-${itemId}`.replaceAll(/[^A-Za-z0-9_.-]/gu, "-").slice(0, 128);
}

/** The Link token out of `/link/token/create`'s answer. */
export function parseLinkToken(value: unknown): string {
  if (!isRecord(value) || typeof value.link_token !== "string" || value.link_token === "") {
    throw new Error("the institution did not issue a link token");
  }
  return value.link_token;
}

/** Item id and access token out of `/item/public_token/exchange`'s answer. */
export function parseExchange(value: unknown): { itemId: string; accessToken: string } {
  if (
    !isRecord(value) ||
    typeof value.item_id !== "string" ||
    value.item_id === "" ||
    typeof value.access_token !== "string" ||
    value.access_token === ""
  ) {
    throw new Error("the institution did not exchange the public token");
  }
  return { itemId: value.item_id, accessToken: value.access_token };
}

/**
 * The connection document a household sees: everything a member needs to
 * recognize and trust the link, and no credential of any kind. The item id is
 * Plaid's public identifier for the link, not a secret; the access token never
 * passes through here, by construction -- this function does not accept one.
 */
export function plaidConnectionBody(input: {
  readonly connectionId: string;
  readonly householdId: string;
  readonly accountId: string;
  readonly itemId: string;
  readonly institutionName: string;
  readonly now: number;
}): Record<string, unknown> {
  return {
    id: input.connectionId,
    household_id: input.householdId,
    created_at: input.now,
    updated_at: input.now,
    kind: "plaid",
    status: "connected",
    account_id: input.accountId,
    external_id: input.itemId,
    institution: input.institutionName,
  };
}

/**
 * The server-only record: the access token and the sync cursor, keyed by the
 * connection it serves. Lives in `plaid_items`, whose policy allows no
 * application user anything, so the only reader is the service credential
 * this function holds.
 */
export function plaidItemBody(input: {
  readonly connectionId: string;
  readonly householdId: string;
  readonly itemId: string;
  readonly accessToken: string;
  readonly cursor: string;
  readonly now: number;
}): Record<string, unknown> {
  return {
    id: input.connectionId,
    household_id: input.householdId,
    created_at: input.now,
    updated_at: input.now,
    item_id: input.itemId,
    access_token: input.accessToken,
    cursor: input.cursor,
  };
}
