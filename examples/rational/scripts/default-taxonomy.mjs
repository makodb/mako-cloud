/**
 * The groups and categories a new household starts with — Monarch's shape,
 * trimmed to what a household actually files under. Shared by the app (which
 * seeds them into an empty household), the demo data (which files its
 * merchants under them), and the tests.
 *
 * Ids are deterministic per household (`grp_<household>.<slug>`,
 * `cat_<household>.<slug>`), so two devices seeding the same empty household
 * at the same moment write the same documents and the conflict handler
 * settles them rather than doubling the set — while two households never
 * collide, because a collection's ids are one namespace for the whole
 * environment, not one per household. Plain JavaScript because `scripts/`
 * runs under Node and the browser build both, and neither may import the
 * other's language.
 */

/**
 * @typedef {"income" | "expense" | "transfer"} CategoryKind
 * @typedef {"fixed" | "flexible" | "non_monthly"} BudgetBucket
 * @typedef {{ readonly slug: string; readonly name: string; readonly icon: string; readonly bucket?: BudgetBucket }} DefaultCategory
 * @typedef {{ readonly slug: string; readonly name: string; readonly kind: CategoryKind; readonly categories: readonly DefaultCategory[] }} DefaultGroup
 */

/** `Gas & electric` → `gas_electric`. */
export function taxonomySlug(name) {
  return name
    .toLowerCase()
    .replaceAll("&", " and ")
    .replaceAll(/[^a-z0-9]+/gu, "_")
    .replaceAll(/^_+|_+$/gu, "");
}

/** The id of a household's default group, from its name or its slug. */
export function defaultGroupId(householdId, name) {
  return `grp_${householdId}.${taxonomySlug(name)}`;
}

/** The id of a household's default category, from its name or its slug. */
export function defaultCategoryId(householdId, name) {
  return `cat_${householdId}.${taxonomySlug(name)}`;
}

/**
 * @param {string} name
 * @param {string} icon
 * @param {BudgetBucket} [bucket]
 * @returns {DefaultCategory}
 */
function category(name, icon, bucket) {
  return { slug: taxonomySlug(name), name, icon, ...(bucket === undefined ? {} : { bucket }) };
}

/**
 * @param {string} name
 * @param {CategoryKind} kind
 * @param {readonly DefaultCategory[]} categories
 * @returns {DefaultGroup}
 */
function group(name, kind, categories) {
  return { slug: taxonomySlug(name), name, kind, categories };
}

/** @type {readonly DefaultGroup[]} */
export const DEFAULT_TAXONOMY = [
  group("Income", "income", [
    category("Paychecks", "💵"),
    category("Interest", "🏦"),
    category("Business income", "💼"),
    category("Other income", "💰"),
  ]),
  group("Gifts & Donations", "expense", [
    category("Charity", "❤️", "flexible"),
    category("Gifts", "🎁", "non_monthly"),
  ]),
  group("Auto & Transport", "expense", [
    category("Auto payment", "🚗", "fixed"),
    category("Public transit", "🚌", "flexible"),
    category("Gas", "⛽", "flexible"),
    category("Auto maintenance", "🔧", "non_monthly"),
    category("Parking & tolls", "🅿️", "flexible"),
    category("Taxi & ride shares", "🚕", "flexible"),
  ]),
  group("Housing", "expense", [
    category("Mortgage", "🏠", "fixed"),
    category("Rent", "🏘️", "fixed"),
    category("Home improvement", "🛠️", "non_monthly"),
  ]),
  group("Bills & Utilities", "expense", [
    category("Garbage", "🗑️", "fixed"),
    category("Water", "💧", "fixed"),
    category("Gas & electric", "⚡", "fixed"),
    category("Internet & cable", "📡", "fixed"),
    category("Phone", "📱", "fixed"),
  ]),
  group("Food & Dining", "expense", [
    category("Groceries", "🛒", "flexible"),
    category("Restaurants & bars", "🍽️", "flexible"),
    category("Coffee shops", "☕", "flexible"),
  ]),
  group("Travel & Lifestyle", "expense", [
    category("Travel & vacation", "✈️", "non_monthly"),
    category("Entertainment & recreation", "🎟️", "flexible"),
    category("Personal", "🧴", "flexible"),
    category("Pets", "🐾", "flexible"),
    category("Fun money", "🎉", "flexible"),
  ]),
  group("Shopping", "expense", [
    category("Shopping", "🛍️", "flexible"),
    category("Clothing", "👕", "flexible"),
    category("Furniture & housewares", "🛋️", "flexible"),
    category("Electronics", "💻", "flexible"),
  ]),
  group("Children", "expense", [
    category("Child care", "👶", "fixed"),
    category("Child activities", "🧸", "flexible"),
  ]),
  group("Education", "expense", [
    category("Student loans", "🎓", "fixed"),
    category("Education", "📚", "non_monthly"),
  ]),
  group("Health & Wellness", "expense", [
    category("Medical", "🩺", "flexible"),
    category("Dentist", "🦷", "non_monthly"),
    category("Fitness", "🏋️", "fixed"),
  ]),
  group("Financial", "expense", [
    category("Loan repayment", "💳", "fixed"),
    category("Financial & legal services", "⚖️", "flexible"),
    category("Financial fees", "🧾", "flexible"),
    category("Cash & ATM", "🏧", "flexible"),
    category("Insurance", "🛡️", "fixed"),
    category("Taxes", "🧮", "non_monthly"),
  ]),
  group("Other", "expense", [
    category("Uncategorized", "❔", "flexible"),
    category("Check", "✅", "flexible"),
    category("Miscellaneous", "📦", "flexible"),
  ]),
  group("Business", "expense", [
    category("Advertising & promotion", "📣", "flexible"),
    category("Office supplies & expenses", "📎", "flexible"),
    category("Business travel & meals", "🧳", "flexible"),
    category("Postage & shipping", "📮", "flexible"),
  ]),
  group("Transfers", "transfer", [
    category("Transfer", "🔁"),
    category("Credit card payment", "💳"),
    category("Balance adjustment", "🧭"),
  ]),
];

/**
 * The taxonomy documents a household is seeded with, stamped for one
 * household at one moment: groups first, then their categories, each with
 * its position so the screens can show them in this order.
 *
 * @param {string} householdId
 * @param {number} at
 * @returns {ReadonlyArray<Record<string, unknown>>}
 */
export function defaultTaxonomyDocuments(householdId, at) {
  const documents = [];
  DEFAULT_TAXONOMY.forEach((entry, groupIndex) => {
    const groupId = defaultGroupId(householdId, entry.slug);
    documents.push({
      id: groupId,
      household_id: householdId,
      created_at: at,
      updated_at: at,
      kind: "group",
      name: entry.name,
      category_kind: entry.kind,
      sort_order: groupIndex,
    });
    entry.categories.forEach((item, categoryIndex) => {
      documents.push({
        id: defaultCategoryId(householdId, item.slug),
        household_id: householdId,
        created_at: at,
        updated_at: at,
        kind: "category",
        name: item.name,
        category_kind: entry.kind,
        parent_id: groupId,
        icon: item.icon,
        sort_order: categoryIndex,
        ...(item.bucket === undefined ? {} : { budget_bucket: item.bucket }),
      });
    });
  });
  return documents;
}
