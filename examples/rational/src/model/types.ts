/**
 * Document types of the Rational model. The JSON schemas in
 * `mako/collections.json` are the source of truth for the wire; these types
 * mirror them for the application code. Amounts are integer minor units with
 * an ISO 4217 `currency`; timestamps are unix milliseconds; deletion is RxDB's
 * `_deleted` and never a field of the document.
 *
 * Three collections carry several document types each behind a `kind`
 * discriminator — `taxonomy` (categories, tags, groups, merchants), `alerts`
 * (settings and fired alerts), and `connections` (institutions, Plaid links,
 * and import batches). RxDB's open-source build opens at most thirteen
 * collections per page (`COL23`) and the model would otherwise need far more;
 * the policies of the merged kinds are identical, so nothing about sharing
 * changes.
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

/** How the budget page presents the household's budgets; the budgets are the same. */
export type BudgetMode = "category" | "flex";

export interface Household extends BaseDocument {
  readonly name: string;
  readonly currency: string;
  readonly owner_id: string;
  readonly budget_mode?: BudgetMode;
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
  "cash",
  "credit",
  "investment",
  "crypto",
  "loan",
  "real_estate",
  "vehicle",
  "other_asset",
  "other_liability",
] as const;
export type AccountType = (typeof ACCOUNT_TYPES)[number];

/** Liabilities carry a negative balance from the household's point of view. */
export const LIABILITY_TYPES: readonly AccountType[] = ["credit", "loan", "other_liability"];

/**
 * Accounts whose value is tracked rather than transacted: a house, a car, a
 * collection. Their balance moves by a balance update (a transaction flagged
 * `adjustment`), never by income or spending.
 */
export const TRACKED_TYPES: readonly AccountType[] = [
  "real_estate",
  "vehicle",
  "other_asset",
  "other_liability",
];

/** Accounts that may hold positions valued at a price. */
export const HOLDING_TYPES: readonly AccountType[] = ["investment", "crypto"];

/**
 * The classes accounts are grouped under, in the order they are shown. Every
 * account type belongs to exactly one.
 */
export const ACCOUNT_CLASSES = [
  { id: "cash", label: "Cash", types: ["checking", "savings", "cash"] },
  { id: "credit", label: "Credit cards", types: ["credit"] },
  { id: "investment", label: "Investments", types: ["investment"] },
  { id: "crypto", label: "Crypto", types: ["crypto"] },
  { id: "loan", label: "Loans", types: ["loan"] },
  { id: "real_estate", label: "Real estate", types: ["real_estate"] },
  { id: "vehicle", label: "Vehicles", types: ["vehicle"] },
  { id: "other_asset", label: "Other assets", types: ["other_asset"] },
  { id: "other_liability", label: "Other liabilities", types: ["other_liability"] },
] as const satisfies ReadonlyArray<{
  readonly id: string;
  readonly label: string;
  readonly types: readonly AccountType[];
}>;
export type AccountClassId = (typeof ACCOUNT_CLASSES)[number]["id"];

export const ACCOUNT_TYPE_LABELS: Readonly<Record<AccountType, string>> = {
  checking: "Checking",
  savings: "Savings",
  cash: "Cash",
  credit: "Credit card",
  investment: "Investment",
  crypto: "Crypto",
  loan: "Loan",
  real_estate: "Real estate",
  vehicle: "Vehicle",
  other_asset: "Other asset",
  other_liability: "Other liability",
};

export function accountClassOf(type: AccountType): AccountClassId {
  const found = ACCOUNT_CLASSES.find((entry) => (entry.types as readonly string[]).includes(type));
  return found?.id ?? "other_asset";
}

export const ASSET_CLASSES = ["stock", "etf", "fund", "bond", "cash", "crypto", "other"] as const;
export type AssetClass = (typeof ASSET_CLASSES)[number];

/** One position of an investment or crypto account, valued at `quantity × price`. */
export interface Holding {
  readonly id: string;
  readonly symbol: string;
  readonly name: string;
  /** Units held; fractional shares are ordinary, so this is a plain number. */
  readonly quantity: number;
  /** Minor units per unit. */
  readonly price: number;
  /** Minor units paid for the whole position, when known. */
  readonly cost_basis?: number;
  readonly asset_class: AssetClass;
  readonly price_as_of?: string;
}

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
  readonly hide_from_net_worth?: boolean;
  /** The member the account belongs to, for a household that keeps "mine" and "ours" apart. */
  readonly owner_id?: string;
  readonly holdings?: readonly Holding[];
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
  readonly merchant_id?: string;
  readonly tags: readonly string[];
  readonly notes?: string;
  readonly splits: readonly Split[];
  readonly pending?: boolean;
  /** Set on what a member typed and on what a member or a rule has looked at. */
  readonly reviewed?: boolean;
  /** Kept out of budgets, reports, and cash flow. */
  readonly hidden?: boolean;
  /** A balance update of a tracked account, never income or spending. */
  readonly adjustment?: boolean;
  /** Shared by the two legs of a transfer. */
  readonly transfer_id?: string;
  readonly rule_id?: string;
  readonly recurrence_id?: string;
  readonly external_id?: string;
  readonly import_batch_id?: string;
  readonly receipts?: readonly string[];
}

export type CategoryKind = "income" | "expense" | "transfer";
export type TaxonomyKind = "category" | "tag" | "group" | "merchant";
export type BudgetBucket = "fixed" | "flexible" | "non_monthly";

/**
 * A category, a tag, a category group, or a merchant. `category_kind` is set
 * on categories and groups; `parent_id` on a category names its group;
 * `patterns` on a merchant are the normalized descriptions that mean it.
 */
export interface TaxonomyEntry extends BaseDocument {
  readonly kind: TaxonomyKind;
  readonly name: string;
  readonly category_kind?: CategoryKind;
  readonly parent_id?: string;
  readonly color?: string;
  readonly icon?: string;
  readonly sort_order?: number;
  readonly archived?: boolean;
  readonly budget_bucket?: BudgetBucket;
  /** For a non-monthly category: this much every `target_months`. */
  readonly target_amount?: number;
  readonly target_months?: number;
  readonly patterns?: readonly string[];
}

export type Category = TaxonomyEntry & { readonly kind: "category" };
export type Tag = TaxonomyEntry & { readonly kind: "tag" };
export type CategoryGroup = TaxonomyEntry & { readonly kind: "group" };
export type Merchant = TaxonomyEntry & { readonly kind: "merchant" };

export function isCategory(entry: TaxonomyEntry): entry is Category {
  return entry.kind === "category";
}

export function isTag(entry: TaxonomyEntry): entry is Tag {
  return entry.kind === "tag";
}

export function isGroup(entry: TaxonomyEntry): entry is CategoryGroup {
  return entry.kind === "group";
}

export function isMerchant(entry: TaxonomyEntry): entry is Merchant {
  return entry.kind === "merchant";
}

export type RuleDirection = "expense" | "income";

export interface Rule extends BaseDocument {
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
  readonly add_tags: readonly string[];
  readonly hide?: boolean;
  readonly mark_reviewed?: boolean;
  readonly priority: number;
  readonly match_count: number;
  readonly enabled: boolean;
}

export type BudgetKind = "category" | "group" | "income" | "flex";

/**
 * One budget for one month. `category_id` names a category or a group; for
 * the household-level kinds it is the kind itself (`income`, `flex`).
 */
export interface Budget extends BaseDocument {
  readonly category_id: string;
  readonly month: string;
  readonly amount: number;
  readonly currency: string;
  readonly rollover: boolean;
  readonly kind?: BudgetKind;
}

export type RecurrenceInterval = "weekly" | "biweekly" | "monthly" | "quarterly" | "yearly";
export type RecurrenceStatus = "detected" | "confirmed" | "dismissed" | "paused";

export interface Recurrence extends BaseDocument {
  readonly account_id: string;
  readonly normalized_description: string;
  readonly interval: RecurrenceInterval;
  readonly expected_amount: number;
  readonly currency: string;
  readonly next_date: string;
  readonly last_date?: string;
  readonly status: RecurrenceStatus;
  readonly matched_count: number;
  readonly source?: "detected" | "manual";
  /** What the household calls it, when the normalized description will not do. */
  readonly name?: string;
  readonly category_id?: string;
  readonly merchant_id?: string;
  readonly last_paid_transaction_id?: string;
}

export interface GoalContribution {
  readonly id: string;
  readonly date: string;
  readonly amount: number;
  readonly note?: string;
}

export type GoalKind = "save" | "pay_down";
export type GoalProgressSource = "contributions" | "balance";

export interface Goal extends BaseDocument {
  readonly name: string;
  readonly target_amount: number;
  readonly currency: string;
  readonly target_date?: string;
  /** Kept for the original single link; `account_ids` is what new goals use. */
  readonly account_id?: string;
  readonly status: "active" | "completed" | "archived";
  readonly contributions: readonly GoalContribution[];
  readonly kind?: GoalKind;
  readonly account_ids?: readonly string[];
  readonly planned_monthly?: number;
  readonly priority?: number;
  readonly progress_source?: GoalProgressSource;
  /** The linked balance (or the amount owed) when the goal began, so progress measures what changed since. */
  readonly starting_balance?: number;
}

export interface SnapshotBalance {
  readonly account_id: string;
  readonly balance: number;
}

export interface NetWorthSnapshot extends BaseDocument {
  readonly date: string;
  readonly assets: number;
  readonly liabilities: number;
  readonly net_worth: number;
  readonly currency: string;
  readonly balances?: readonly SnapshotBalance[];
}

export type AlertKind =
  | "large_transaction"
  | "budget_exceeded"
  | "low_balance"
  | "bill_due"
  | "goal_reached"
  | "sync_error";
export type AlertDocumentKind = "setting" | "alert";

/** An alert setting or an alert it fired; `kind` says which. */
export interface AlertDocument extends BaseDocument {
  readonly kind: AlertDocumentKind;
  readonly alert_kind: AlertKind;
  /** An amount for the money alerts; a number of days for `bill_due`. */
  readonly threshold?: number;
  readonly enabled?: boolean;
  readonly fired_at?: number;
  readonly message?: string;
  readonly transaction_id?: string;
  readonly account_id?: string;
  readonly category_id?: string;
  readonly budget_id?: string;
  readonly recurrence_id?: string;
  readonly goal_id?: string;
  readonly connection_id?: string;
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
