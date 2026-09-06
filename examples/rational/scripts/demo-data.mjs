/**
 * A deterministic demo household, rich enough to show every screen: the
 * default category groups with the demo's own categories filed under them,
 * tags, merchants with the patterns that mean them, seven accounts (a house
 * and a brokerage among them), about two hundred transactions across the last
 * three months with paired transfers, a hidden reimbursement, a balance update,
 * and three synced arrivals awaiting review; then a rule, three months of
 * budgets, four confirmed bills, three goals, and ninety nightly net-worth
 * snapshots computed from the same transactions. The same household id and
 * anchor date always produce the same documents. The seed script pushes them
 * to a real environment; the in-browser fake backend serves them directly.
 */

import { DEFAULT_TAXONOMY, defaultGroupId, taxonomySlug } from "./default-taxonomy.mjs";

/** A tiny seeded PRNG (mulberry32) so generated data is reproducible. */
function random(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = Math.imul(state ^ (state >>> 15), 1 | state);
    value = (value + Math.imul(value ^ (value >>> 7), 61 | value)) ^ value;
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

function isoDate(date) {
  return date.toISOString().slice(0, 10);
}

function daysAgo(anchor, days) {
  const date = new Date(anchor);
  date.setUTCDate(date.getUTCDate() - days);
  return isoDate(date);
}

function monthsAfter(anchor, months) {
  const date = new Date(anchor);
  date.setUTCMonth(date.getUTCMonth() + months);
  return isoDate(date);
}

/**
 * Mirrors `normalizeDescription` in `functions/shared/recurrences.ts`, which
 * this script cannot import: the merchants' patterns and the recurrences'
 * descriptions must be what the engines derive from a statement line, or a
 * seeded bill would never be matched to the transaction that pays it.
 */
function normalizeDescription(description) {
  return description
    .toLowerCase()
    .replaceAll(/[0-9#*]+/gu, " ")
    .replaceAll(/[^a-z ]+/gu, " ")
    .replaceAll(/\s+/gu, " ")
    .trim();
}

/** Mirrors `LIABILITY_TYPES` in `src/model/types.ts`: balances that are money owed. */
const LIABILITY_TYPES = ["credit", "loan", "other_liability"];

/**
 * The demo's categories keep their original ids and names -- the browser
 * suite selects them by label -- and are filed under the default groups so
 * the group-level screens have something to roll up.
 */
const CATEGORY_SPECS = [
  { id: "cat_demo_salary", name: "Salary", kind: "income", group: "Income", icon: "💵" },
  { id: "cat_demo_interest", name: "Interest", kind: "income", group: "Income", icon: "🏦" },
  {
    id: "cat_demo_groceries",
    name: "Groceries",
    kind: "expense",
    group: "Food & Dining",
    icon: "🛒",
    bucket: "flexible",
  },
  {
    id: "cat_demo_dining",
    name: "Dining out",
    kind: "expense",
    group: "Food & Dining",
    icon: "🍽️",
    bucket: "flexible",
  },
  {
    id: "cat_demo_rent",
    name: "Rent",
    kind: "expense",
    group: "Housing",
    icon: "🏘️",
    bucket: "fixed",
  },
  {
    id: "cat_demo_utilities",
    name: "Utilities",
    kind: "expense",
    group: "Bills & Utilities",
    icon: "⚡",
    bucket: "fixed",
  },
  {
    id: "cat_demo_transport",
    name: "Transport",
    kind: "expense",
    group: "Auto & Transport",
    icon: "🚌",
    bucket: "flexible",
  },
  {
    id: "cat_demo_health",
    name: "Health",
    kind: "expense",
    group: "Health & Wellness",
    icon: "🩺",
    bucket: "flexible",
  },
  {
    id: "cat_demo_entertainment",
    name: "Entertainment",
    kind: "expense",
    group: "Travel & Lifestyle",
    icon: "🎟️",
    bucket: "flexible",
  },
  {
    id: "cat_demo_shopping",
    name: "Shopping",
    kind: "expense",
    group: "Shopping",
    icon: "🛍️",
    bucket: "flexible",
  },
  {
    id: "cat_demo_travel",
    name: "Travel",
    kind: "expense",
    group: "Travel & Lifestyle",
    icon: "✈️",
    bucket: "non_monthly",
    target: { amount: 240_000, months: 12 },
  },
  { id: "cat_demo_transfer", name: "Transfer", kind: "transfer", group: "Transfers", icon: "🔁" },
];

const TAG_SPECS = [
  ["tag_demo_reimbursable", "reimbursable"],
  ["tag_demo_shared", "shared"],
  ["tag_demo_subscription", "subscription"],
  ["tag_demo_vacation", "vacation"],
];

/** Everyday merchants: name, category, amount range, tags. The name is the statement line. */
const MERCHANTS = [
  ["Whole Harvest Market", "cat_demo_groceries", -3200, -14500, []],
  ["Corner Grocer", "cat_demo_groceries", -800, -6200, []],
  ["Noodle House", "cat_demo_dining", -1400, -5600, ["tag_demo_shared"]],
  ["Blue Bottle Coffee", "cat_demo_dining", -450, -1250, []],
  ["Metro Transit", "cat_demo_transport", -275, -275, []],
  ["Ride Share", "cat_demo_transport", -900, -3400, ["tag_demo_reimbursable"]],
  ["Pharmacy", "cat_demo_health", -600, -4800, []],
  ["Streaming Service", "cat_demo_entertainment", -1599, -1599, ["tag_demo_subscription"]],
  ["Cinema", "cat_demo_entertainment", -1800, -4200, ["tag_demo_shared"]],
  ["Department Store", "cat_demo_shopping", -2500, -18000, []],
  ["Hardware Store", "cat_demo_shopping", -1200, -9000, []],
];

/**
 * The streaming charge is a monthly bill with a confirmed recurrence, so it is
 * booked on the first of each month rather than drawn at random; everything
 * else in MERCHANTS is everyday spending.
 */
const EVERYDAY_MERCHANTS = MERCHANTS.filter(([name]) => name !== "Streaming Service");

/** The fixed payees: a display name and the statement line their transactions carry. */
const PAYEES = [
  { name: "ACME Corp", description: "ACME Corp payroll" },
  { name: "Maple Street Rentals", description: "Rent - Maple Street" },
  { name: "City Power & Light", description: "City Power & Light" },
  { name: "Fiber Internet", description: "Fiber internet" },
  { name: "Rational Auto Finance", description: "Rational Auto Finance payment" },
];

function merchantId(name) {
  return `mer_demo_${taxonomySlug(name)}`;
}

/** The value of an account's positions, as the balance derivation adds it. */
function holdingsValue(account) {
  return (account.holdings ?? []).reduce(
    (total, holding) => total + Math.round(holding.quantity * holding.price),
    0,
  );
}

/**
 * @param {{ householdId: string; currency?: string; anchor?: string; transactionCount?: number; ownerId?: string; name?: string }} options
 */
export function generateDemoHousehold(options) {
  const householdId = options.householdId;
  const currency = options.currency ?? "USD";
  const anchor = new Date(options.anchor ?? "2026-08-15T12:00:00.000Z");
  const anchorDate = isoDate(anchor);
  const transactionCount = options.transactionCount ?? 200;
  const next = random(hash(householdId));
  // Timestamps are fixed relative to the anchor so re-generation is identical.
  const base = anchor.getTime() - 90 * 24 * 60 * 60 * 1000;
  let sequence = 0;
  const stamp = () => {
    sequence += 1;
    return base + sequence;
  };
  const document = (id, fields) => {
    const at = stamp();
    return { id, household_id: householdId, created_at: at, updated_at: at, ...fields };
  };
  // Month 0 is the anchor's month, 1 the month before, -1 the month after.
  const monthStart = (offset) =>
    new Date(Date.UTC(anchor.getUTCFullYear(), anchor.getUTCMonth() - offset, 1));
  const dayOf = (offset, day) => {
    const start = monthStart(offset);
    return isoDate(new Date(Date.UTC(start.getUTCFullYear(), start.getUTCMonth(), day)));
  };
  const monthKey = (offset) => isoDate(monthStart(offset)).slice(0, 7);

  const household = document(householdId, {
    name: options.name ?? "Demo household",
    currency,
    owner_id: options.ownerId ?? "usr_demo_owner",
    budget_mode: "category",
  });

  const accounts = [
    document("acc_demo_checking", {
      name: "Everyday checking",
      type: "checking",
      currency,
      opening_balance: 245_000,
      opening_date: daysAgo(anchor, 100),
      institution: "First Rational Bank",
    }),
    document("acc_demo_savings", {
      name: "Rainy-day savings",
      type: "savings",
      currency,
      opening_balance: 1_200_000,
      opening_date: daysAgo(anchor, 100),
      institution: "First Rational Bank",
    }),
    document("acc_demo_credit", {
      name: "Rewards card",
      type: "credit",
      currency,
      opening_balance: -42_000,
      opening_date: daysAgo(anchor, 100),
      institution: "Rational Cards",
    }),
    document("acc_demo_cash", {
      name: "Wallet",
      type: "cash",
      currency,
      opening_balance: 8_000,
      opening_date: daysAgo(anchor, 100),
    }),
    document("acc_demo_loan", {
      name: "Car loan",
      type: "loan",
      currency,
      opening_balance: -1_450_000,
      opening_date: daysAgo(anchor, 100),
      institution: "Rational Auto Finance",
    }),
    // A tracked asset: its value moves by balance updates, never by spending.
    document("acc_demo_home", {
      name: "Maple Street house",
      type: "real_estate",
      currency,
      opening_balance: 45_000_000,
      opening_date: daysAgo(anchor, 100),
    }),
    // The opening balance is the cash; the positions are valued on top of it.
    document("acc_demo_brokerage", {
      name: "Brokerage",
      type: "investment",
      currency,
      opening_balance: 120_000,
      opening_date: daysAgo(anchor, 100),
      institution: "Rational Securities",
      holdings: [
        {
          id: "hld_demo_vti",
          symbol: "VTI",
          name: "Vanguard Total Stock Market ETF",
          quantity: 42.5,
          price: 26_500,
          cost_basis: 1_020_000,
          asset_class: "etf",
          price_as_of: anchorDate,
        },
        {
          id: "hld_demo_bnd",
          symbol: "BND",
          name: "Vanguard Total Bond Market ETF",
          quantity: 100,
          price: 7_250,
          cost_basis: 740_000,
          asset_class: "bond",
          price_as_of: anchorDate,
        },
      ],
    }),
  ];

  // Groups, categories, tags, and merchants share the `taxonomy` collection
  // behind a `kind`. The groups are the defaults every household gets, under
  // their default ids, so the demo files its categories where a fresh
  // household would.
  const groupPosition = new Map();
  const taxonomy = [
    ...DEFAULT_TAXONOMY.map((group, index) =>
      document(defaultGroupId(householdId, group.slug), {
        kind: "group",
        name: group.name,
        category_kind: group.kind,
        sort_order: index,
      }),
    ),
    ...CATEGORY_SPECS.map((spec) => {
      const parentId = defaultGroupId(householdId, spec.group);
      const position = groupPosition.get(parentId) ?? 0;
      groupPosition.set(parentId, position + 1);
      return document(spec.id, {
        kind: "category",
        name: spec.name,
        category_kind: spec.kind,
        parent_id: parentId,
        icon: spec.icon,
        sort_order: position,
        ...(spec.bucket === undefined ? {} : { budget_bucket: spec.bucket }),
        ...(spec.target === undefined
          ? {}
          : { target_amount: spec.target.amount, target_months: spec.target.months }),
      });
    }),
    ...TAG_SPECS.map(([id, name]) => document(id, { kind: "tag", name })),
    ...MERCHANTS.map(([name]) =>
      document(merchantId(name), {
        kind: "merchant",
        name,
        patterns: [normalizeDescription(name)],
      }),
    ),
    ...PAYEES.map((payee) =>
      document(merchantId(payee.name), {
        kind: "merchant",
        name: payee.name,
        patterns: [normalizeDescription(payee.description)],
      }),
    ),
  ];

  const transactions = [];
  const spendingAccounts = ["acc_demo_checking", "acc_demo_credit", "acc_demo_cash"];
  let index = 0;
  // Everything a member entered by hand is reviewed; what arrived by sync is
  // not, and that absence is what the review queue shows.
  const transaction = (fields, { synced = false } = {}) => {
    index += 1;
    const entry = document(`txn_demo_${String(index).padStart(4, "0")}`, {
      currency,
      tags: [],
      splits: [],
      ...(synced ? {} : { reviewed: true }),
      ...fields,
    });
    transactions.push(entry);
    return entry;
  };
  // Three months of fixed items: salary, rent, utilities, loan payment,
  // interest, the savings transfer, and the streaming subscription. The first
  // is txn_demo_0001, the payroll of the anchor month, which the browser suite
  // edits from two devices at once.
  const lastPaid = { rent: "", power: "", fiber: "", streaming: "" };
  for (let month = 0; month < 3; month += 1) {
    const on = (day) => dayOf(month, day);
    transaction({
      account_id: "acc_demo_checking",
      date: on(1),
      amount: 520_000,
      description: "ACME Corp payroll",
      category_id: "cat_demo_salary",
      merchant_id: merchantId("ACME Corp"),
    });
    const rent = transaction({
      account_id: "acc_demo_checking",
      date: on(2),
      amount: -185_000,
      description: "Rent - Maple Street",
      category_id: "cat_demo_rent",
      merchant_id: merchantId("Maple Street Rentals"),
      tags: ["tag_demo_shared"],
    });
    const power = transaction({
      account_id: "acc_demo_checking",
      date: on(5),
      amount: -12_450,
      description: "City Power & Light",
      category_id: "cat_demo_utilities",
      merchant_id: merchantId("City Power & Light"),
      tags: ["tag_demo_shared"],
    });
    const fiber = transaction({
      account_id: "acc_demo_checking",
      date: on(6),
      amount: -6_999,
      description: "Fiber internet",
      category_id: "cat_demo_utilities",
      merchant_id: merchantId("Fiber Internet"),
      tags: ["tag_demo_subscription"],
    });
    // The two legs of a transfer share a transfer_id, which is what keeps them
    // out of cash flow; their category is incidental.
    transaction({
      account_id: "acc_demo_checking",
      date: on(10),
      amount: -38_500,
      description: "Rational Auto Finance payment",
      category_id: "cat_demo_transfer",
      merchant_id: merchantId("Rational Auto Finance"),
      transfer_id: `trf_demo_loan_${month}`,
    });
    transaction({
      account_id: "acc_demo_loan",
      date: on(10),
      amount: 38_500,
      description: "Payment received",
      category_id: "cat_demo_transfer",
      transfer_id: `trf_demo_loan_${month}`,
    });
    transaction({
      account_id: "acc_demo_savings",
      date: on(28),
      amount: 2_150,
      description: "Interest credit",
      category_id: "cat_demo_interest",
    });
    transaction({
      account_id: "acc_demo_checking",
      date: on(15),
      amount: -50_000,
      description: "Transfer to savings",
      category_id: "cat_demo_transfer",
      transfer_id: `trf_demo_savings_${month}`,
    });
    transaction({
      account_id: "acc_demo_savings",
      date: on(15),
      amount: 50_000,
      description: "Transfer from checking",
      category_id: "cat_demo_transfer",
      transfer_id: `trf_demo_savings_${month}`,
    });
    const streaming = transaction({
      account_id: "acc_demo_checking",
      date: on(1),
      amount: -1_599,
      description: "Streaming Service",
      category_id: "cat_demo_entertainment",
      merchant_id: merchantId("Streaming Service"),
      tags: ["tag_demo_subscription"],
    });
    if (month === 0) {
      lastPaid.rent = rent.id;
      lastPaid.power = power.id;
      lastPaid.fiber = fiber.id;
      lastPaid.streaming = streaming.id;
    }
  }
  // One split transaction so the editor has something to show.
  transaction({
    account_id: "acc_demo_credit",
    date: daysAgo(anchor, 12),
    amount: -9_850,
    description: "Superstore run",
    tags: ["tag_demo_shared"],
    splits: [
      { id: "split_demo_1", category_id: "cat_demo_groceries", amount: -6_850 },
      {
        id: "split_demo_2",
        category_id: "cat_demo_shopping",
        amount: -3_000,
        note: "kitchen towels",
      },
    ],
  });
  // Money that was never income: hidden, so it stays out of cash flow.
  transaction({
    account_id: "acc_demo_checking",
    date: daysAgo(anchor, 9),
    amount: 4_500,
    description: "Venmo reimbursement",
    hidden: true,
    notes: "paid back by a friend",
  });
  // The house was revalued: a balance update moves a tracked account without
  // becoming spending.
  transaction({
    account_id: "acc_demo_home",
    date: daysAgo(anchor, 20),
    amount: 500_000,
    description: "Balance update",
    adjustment: true,
    hidden: true,
    reviewed: true,
  });
  // What the sync brought in this week: raw statement lines, no category, not
  // yet looked at -- the review queue.
  const arrivals = [
    ["sim-demo-1", "SQ *CORNER GROCER 0412", -4_210, 4],
    ["sim-demo-2", "UBER *TRIP 4X2", -1_875, 3],
    ["sim-demo-3", "AMZN MKTP US*2K1", -3_299, 1],
  ];
  for (const [externalId, description, amount, age] of arrivals) {
    transaction(
      {
        account_id: "acc_demo_checking",
        date: daysAgo(anchor, age),
        amount,
        description,
        external_id: externalId,
      },
      { synced: true },
    );
  }
  // Random everyday spending fills the rest, spread over the last 90 days.
  while (transactions.length < transactionCount) {
    const merchant = EVERYDAY_MERCHANTS[Math.floor(next() * EVERYDAY_MERCHANTS.length)];
    const [description, categoryId, minimum, maximum, merchantTags] = merchant;
    const amount =
      maximum === minimum ? minimum : Math.round(minimum + next() * (maximum - minimum));
    transaction({
      account_id: spendingAccounts[Math.floor(next() * spendingAccounts.length)],
      date: daysAgo(anchor, Math.floor(next() * 90)),
      amount,
      description,
      category_id: categoryId,
      merchant_id: merchantId(description),
      tags: merchantTags,
    });
  }
  transactions.sort(
    (left, right) => left.date.localeCompare(right.date) || left.id.localeCompare(right.id),
  );

  const rules = [
    document("rul_demo_coffee", {
      name: "Coffee shops",
      match: { description_contains: "blue bottle" },
      set_category_id: "cat_demo_dining",
      add_tags: [],
      priority: 10,
      match_count: 0,
      enabled: true,
    }),
  ];

  // The same budgets for each of the three months, so month-to-month
  // comparisons and rollover have something to work with.
  const BUDGET_SPECS = [
    ["cat_demo_groceries", 40_000, true],
    ["cat_demo_dining", 20_000, false],
    ["cat_demo_entertainment", 8_000, true],
    ["cat_demo_shopping", 15_000, false],
    ["cat_demo_transport", 6_000, false],
    ["cat_demo_utilities", 20_000, false],
  ];
  const budgets = [];
  for (let month = 2; month >= 0; month -= 1) {
    const key = monthKey(month);
    for (const [categoryId, amount, rollover] of BUDGET_SPECS) {
      budgets.push(
        document(`bud_${categoryId}.${key}`, {
          category_id: categoryId,
          month: key,
          amount,
          currency,
          rollover,
          kind: "category",
        }),
      );
    }
    budgets.push(
      document(`bud_income.${key}`, {
        category_id: "income",
        month: key,
        amount: 520_000,
        currency,
        rollover: false,
        kind: "income",
      }),
    );
  }

  // Four bills the nightly job noticed and the household confirmed; each was
  // paid this month and falls due again next month, on the day its
  // transactions land.
  const bill = (id, fields) =>
    document(id, {
      account_id: "acc_demo_checking",
      interval: "monthly",
      currency,
      status: "confirmed",
      source: "detected",
      matched_count: 3,
      ...fields,
    });
  const recurrences = [
    bill("rec_demo_streaming", {
      name: "Streaming Service",
      normalized_description: "streaming service",
      expected_amount: -1_599,
      next_date: dayOf(-1, 1),
      last_date: dayOf(0, 1),
      category_id: "cat_demo_entertainment",
      merchant_id: merchantId("Streaming Service"),
      last_paid_transaction_id: lastPaid.streaming,
    }),
    bill("rec_demo_rent", {
      name: "Rent",
      normalized_description: "rent maple street",
      expected_amount: -185_000,
      next_date: dayOf(-1, 2),
      last_date: dayOf(0, 2),
      category_id: "cat_demo_rent",
      merchant_id: merchantId("Maple Street Rentals"),
      last_paid_transaction_id: lastPaid.rent,
    }),
    bill("rec_demo_power", {
      name: "City Power & Light",
      normalized_description: "city power light",
      expected_amount: -12_450,
      next_date: dayOf(-1, 5),
      last_date: dayOf(0, 5),
      category_id: "cat_demo_utilities",
      merchant_id: merchantId("City Power & Light"),
      last_paid_transaction_id: lastPaid.power,
    }),
    bill("rec_demo_fiber", {
      name: "Fiber internet",
      normalized_description: "fiber internet",
      expected_amount: -6_999,
      next_date: dayOf(-1, 6),
      last_date: dayOf(0, 6),
      category_id: "cat_demo_utilities",
      merchant_id: merchantId("Fiber Internet"),
      last_paid_transaction_id: lastPaid.fiber,
    }),
  ];

  // A save goal read from a balance, a pay-down goal on the loan, and a save
  // goal read from its recorded contributions.
  const goals = [
    document("goa_demo_emergency", {
      name: "Emergency fund",
      kind: "save",
      target_amount: 2_000_000,
      currency,
      target_date: monthsAfter(anchor, 12),
      account_ids: ["acc_demo_savings"],
      planned_monthly: 50_000,
      priority: 0,
      progress_source: "balance",
      starting_balance: 1_200_000,
      status: "active",
      contributions: [],
    }),
    document("goa_demo_car", {
      name: "Pay off the car",
      kind: "pay_down",
      target_amount: 1_450_000,
      currency,
      account_ids: ["acc_demo_loan"],
      planned_monthly: 38_500,
      priority: 1,
      progress_source: "balance",
      starting_balance: 1_450_000,
      status: "active",
      contributions: [],
    }),
    document("goa_demo_trip", {
      name: "Japan trip",
      kind: "save",
      target_amount: 600_000,
      currency,
      target_date: monthsAfter(anchor, 8),
      planned_monthly: 60_000,
      priority: 2,
      progress_source: "contributions",
      status: "active",
      contributions: [2, 1, 0].map((month) => ({
        id: `con_demo_trip_${month}`,
        date: dayOf(month, 15),
        amount: 60_000,
      })),
    }),
  ];

  const net_worth_snapshots = snapshots({
    householdId,
    currency,
    anchor,
    accounts,
    transactions,
    document,
  });

  return {
    household,
    accounts,
    taxonomy,
    transactions,
    rules,
    budgets,
    recurrences,
    goals,
    net_worth_snapshots,
  };
}

/**
 * What the nightly job would have recorded on each of the ninety days up to
 * the anchor, derived as the job derives it: every open account's opening
 * balance plus its transactions through that day plus the value of its
 * holdings, with liabilities counted as money owed and only the household's
 * currency summed. One pass over the date-sorted transactions serves every
 * day.
 */
function snapshots({ householdId, currency, anchor, accounts, transactions, document }) {
  const sorted = [...transactions].sort((left, right) => left.date.localeCompare(right.date));
  const balances = new Map(
    accounts.map((account) => [account.id, account.opening_balance + holdingsValue(account)]),
  );
  const documents = [];
  let pointer = 0;
  for (let age = 89; age >= 0; age -= 1) {
    const date = daysAgo(anchor, age);
    while (pointer < sorted.length && sorted[pointer].date <= date) {
      const entry = sorted[pointer];
      balances.set(entry.account_id, (balances.get(entry.account_id) ?? 0) + entry.amount);
      pointer += 1;
    }
    let assets = 0;
    let liabilities = 0;
    for (const account of accounts) {
      if (account.closed_at !== undefined || account.currency !== currency) continue;
      const balance = balances.get(account.id) ?? 0;
      if (LIABILITY_TYPES.includes(account.type)) liabilities += -balance;
      else assets += balance;
    }
    documents.push(
      document(`nws_${householdId}.${date}`, {
        date,
        assets,
        liabilities,
        net_worth: assets - liabilities,
        currency,
        balances: accounts.map((account) => ({
          account_id: account.id,
          balance: balances.get(account.id) ?? 0,
        })),
      }),
    );
  }
  return documents;
}

/** A stable 32-bit hash so the PRNG seed follows the household id. */
function hash(text) {
  let value = 2166136261;
  for (const character of text) {
    value ^= character.codePointAt(0);
    value = Math.imul(value, 16777619);
  }
  return value >>> 0;
}
