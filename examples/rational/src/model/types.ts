/**
 * Document types of the Rational model. The JSON schemas in
 * `mako/collections.json` are the source of truth for the wire; these types
 * mirror them for the application code. Amounts are integer minor units with
 * an ISO 4217 `currency`; timestamps are unix milliseconds; deletion is RxDB's
 * `_deleted` and never a field of the document.
 *
 * Three collections carry two document types each behind a `kind`
 * discriminator — `taxonomy` (categories and tags), `alerts` (settings and
 * fired alerts), and `connections` (institutions and import batches). RxDB's
 * open-source build opens at most thirteen collections per page (`COL23`) and
 * the model would otherwise need fifteen; the policies of the merged kinds
 * were identical, so nothing about sharing changes.
 */
export interface BaseDocument {
  readonly id: string;
  readonly household_id: string;
  readonly created_at: number;
  readonly updated_at: number;
}

export type HouseholdRole = "owner" | "editor" | "viewer";

export const HOUSEHOLD_ROLES = [
  "owner",
  "editor",
  "viewer",
] as const satisfies readonly HouseholdRole[];

export interface Household extends BaseDocument {
  readonly name: string;
  readonly currency: string;
  readonly owner_id: string;
}

export interface Membership extends BaseDocument {
  /** Empty while an invitation is pending: the invitee may not exist yet. */
  readonly user_id: string;
  readonly email?: string;
  readonly role: HouseholdRole;
  readonly status: "active" | "invited" | "accepted" | "removed";
  readonly invited_by?: string;
}

export const ACCOUNT_TYPES = [
  "checking",
  "savings",
  "credit",
  "investment",
  "loan",
  "cash",
] as const;
export type AccountType = (typeof ACCOUNT_TYPES)[number];

/** Liabilities carry a negative balance from the household's point of view. */
export const LIABILITY_TYPES: readonly AccountType[] = ["credit", "loan"];

export interface Account extends BaseDocument {
  readonly name: string;
  readonly type: AccountType;
  readonly currency: string;
  readonly opening_balance: number;
  readonly opening_date: string;
  readonly institution?: string;
  readonly external_id?: string;
  readonly connection_id?: string;
  readonly closed_at?: number;
}

export interface Split {
  readonly id: string;
  readonly category_id?: string;
  readonly amount: number;
  readonly note?: string;
}

export interface Transaction extends BaseDocument {
  readonly account_id: string;
  readonly date: string;
  readonly amount: number;
  readonly currency: string;
  readonly description: string;
  readonly normalized_description?: string;
  readonly category_id?: string;
  readonly tags: readonly string[];
  readonly notes?: string;
  readonly splits: readonly Split[];
  readonly pending?: boolean;
  readonly rule_id?: string;
  readonly recurrence_id?: string;
  readonly external_id?: string;
  readonly import_batch_id?: string;
  readonly receipts?: readonly string[];
}

export type CategoryKind = "income" | "expense" | "transfer";
export type TaxonomyKind = "category" | "tag";

/** A category or a tag; `category_kind` is set on categories only. */
export interface TaxonomyEntry extends BaseDocument {
  readonly kind: TaxonomyKind;
  readonly name: string;
  readonly category_kind?: CategoryKind;
  readonly parent_id?: string;
  readonly color?: string;
  readonly archived?: boolean;
}

export type Category = TaxonomyEntry & { readonly kind: "category" };
export type Tag = TaxonomyEntry & { readonly kind: "tag" };

export function isCategory(entry: TaxonomyEntry): entry is Category {
  return entry.kind === "category";
}

export function isTag(entry: TaxonomyEntry): entry is Tag {
  return entry.kind === "tag";
}

export interface Rule extends BaseDocument {
  readonly name: string;
  readonly match: {
    readonly description_contains?: string;
    readonly amount_min?: number;
    readonly amount_max?: number;
    readonly account_id?: string;
  };
  readonly set_category_id?: string;
  readonly add_tags: readonly string[];
  readonly priority: number;
  readonly match_count: number;
  readonly enabled: boolean;
}

export interface Budget extends BaseDocument {
  readonly category_id: string;
  readonly month: string;
  readonly amount: number;
  readonly currency: string;
  readonly rollover: boolean;
}

export interface Recurrence extends BaseDocument {
  readonly account_id: string;
  readonly normalized_description: string;
  readonly interval: "weekly" | "biweekly" | "monthly" | "quarterly" | "yearly";
  readonly expected_amount: number;
  readonly currency: string;
  readonly next_date: string;
  readonly last_date?: string;
  readonly status: "detected" | "confirmed" | "dismissed";
  readonly matched_count: number;
}

export interface GoalContribution {
  readonly id: string;
  readonly date: string;
  readonly amount: number;
  readonly note?: string;
}

export interface Goal extends BaseDocument {
  readonly name: string;
  readonly target_amount: number;
  readonly currency: string;
  readonly target_date?: string;
  readonly account_id?: string;
  readonly status: "active" | "completed" | "archived";
  readonly contributions: readonly GoalContribution[];
}

export interface NetWorthSnapshot extends BaseDocument {
  readonly date: string;
  readonly assets: number;
  readonly liabilities: number;
  readonly net_worth: number;
  readonly currency: string;
}

export type AlertKind = "large_transaction" | "budget_exceeded" | "low_balance";
export type AlertDocumentKind = "setting" | "alert";

/** An alert setting or an alert it fired; `kind` says which. */
export interface AlertDocument extends BaseDocument {
  readonly kind: AlertDocumentKind;
  readonly alert_kind: AlertKind;
  readonly threshold?: number;
  readonly enabled?: boolean;
  readonly fired_at?: number;
  readonly message?: string;
  readonly transaction_id?: string;
  readonly account_id?: string;
  readonly category_id?: string;
  readonly budget_id?: string;
  readonly amount?: number;
  readonly currency?: string;
  readonly read?: boolean;
}

export type AlertSetting = AlertDocument & { readonly kind: "setting" };
export type Alert = AlertDocument & { readonly kind: "alert" };

export type ConnectionKind = "institution" | "plaid" | "import";

/** An institution connection, a Plaid link, or a CSV import batch; `kind` says which. */
export interface ConnectionDocument extends BaseDocument {
  readonly kind: ConnectionKind;
  readonly institution?: string;
  readonly external_id?: string;
  readonly status?: "connected" | "error" | "disconnected";
  readonly account_ids?: readonly string[];
  readonly last_sync_at?: number;
  readonly last_sync_outcome?: string;
  readonly account_id?: string;
  readonly filename?: string;
  readonly imported_at?: number;
  readonly row_count?: number;
  readonly created_count?: number;
  readonly duplicate_count?: number;
  readonly mapping?: {
    readonly date: string;
    readonly amount: string;
    readonly description: string;
    readonly date_format?: string;
  };
}

export type InstitutionConnection = ConnectionDocument & { readonly kind: "institution" };
export type ImportBatch = ConnectionDocument & { readonly kind: "import" };

/** Every collection keyed by its id, with its document type. */
export interface RationalDocuments {
  households: Household;
  memberships: Membership;
  accounts: Account;
  transactions: Transaction;
  taxonomy: TaxonomyEntry;
  rules: Rule;
  budgets: Budget;
  recurrences: Recurrence;
  goals: Goal;
  net_worth_snapshots: NetWorthSnapshot;
  alerts: AlertDocument;
  connections: ConnectionDocument;
}

export type CollectionId = keyof RationalDocuments;

/** Collections that describe which households a user belongs to. */
export const DIRECTORY_COLLECTIONS = [
  "households",
  "memberships",
] as const satisfies readonly CollectionId[];
export type DirectoryCollectionId = (typeof DIRECTORY_COLLECTIONS)[number];

/**
 * Collections replicated into one database per household. Ten of them, plus
 * the two directory collections, is twelve open at once — one under RxDB's
 * open-source limit, so every collection of the model is open from the start
 * rather than opened screen by screen.
 */
export const HOUSEHOLD_COLLECTIONS = [
  "accounts",
  "transactions",
  "taxonomy",
  "rules",
  "budgets",
  "recurrences",
  "goals",
  "net_worth_snapshots",
  "alerts",
  "connections",
] as const satisfies readonly CollectionId[];
export type HouseholdCollectionId = (typeof HOUSEHOLD_COLLECTIONS)[number];
