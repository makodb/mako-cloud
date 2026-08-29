/**
 * A deterministic demo household: the same accounts, taxonomy (categories and
 * tags), and
 * about two hundred transactions across the last three months every time it
 * is generated for the same household and anchor date. The seed script pushes
 * it to a real environment; the in-browser fake backend serves it directly.
 */

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

const CATEGORY_SPECS = [
  ["cat_demo_salary", "Salary", "income"],
  ["cat_demo_interest", "Interest", "income"],
  ["cat_demo_groceries", "Groceries", "expense"],
  ["cat_demo_dining", "Dining out", "expense"],
  ["cat_demo_rent", "Rent", "expense"],
  ["cat_demo_utilities", "Utilities", "expense"],
  ["cat_demo_transport", "Transport", "expense"],
  ["cat_demo_health", "Health", "expense"],
  ["cat_demo_entertainment", "Entertainment", "expense"],
  ["cat_demo_shopping", "Shopping", "expense"],
  ["cat_demo_travel", "Travel", "expense"],
  ["cat_demo_transfer", "Transfer", "transfer"],
];

const TAG_SPECS = [
  ["tag_demo_reimbursable", "reimbursable"],
  ["tag_demo_shared", "shared"],
  ["tag_demo_subscription", "subscription"],
  ["tag_demo_vacation", "vacation"],
];

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
 * @param {{ householdId: string; currency?: string; anchor?: string; transactionCount?: number; ownerId?: string; ownerEmail?: string }} options
 */
export function generateDemoHousehold(options) {
  const householdId = options.householdId;
  const currency = options.currency ?? "USD";
  const anchor = new Date(options.anchor ?? "2026-08-15T12:00:00.000Z");
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

  const household = document(householdId, {
    name: options.name ?? "Demo household",
    currency,
    owner_id: options.ownerId ?? "usr_demo_owner",
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
  ];

  // Categories and tags share the `taxonomy` collection behind a `kind`.
  const taxonomy = [
    ...CATEGORY_SPECS.map(([id, name, categoryKind]) =>
      document(id, { kind: "category", name, category_kind: categoryKind }),
    ),
    ...TAG_SPECS.map(([id, name]) => document(id, { kind: "tag", name })),
  ];

  const transactions = [];
  const spendingAccounts = ["acc_demo_checking", "acc_demo_credit", "acc_demo_cash"];
  let index = 0;
  const transaction = (fields) => {
    index += 1;
    transactions.push(
      document(`txn_demo_${String(index).padStart(4, "0")}`, {
        currency,
        tags: [],
        splits: [],
        ...fields,
      }),
    );
  };
  // Three months of fixed items: salary, rent, utilities, loan payment, interest.
  for (let month = 0; month < 3; month += 1) {
    const monthStart = new Date(Date.UTC(anchor.getUTCFullYear(), anchor.getUTCMonth() - month, 1));
    const on = (day) => isoDate(new Date(Date.UTC(monthStart.getUTCFullYear(), monthStart.getUTCMonth(), day)));
    transaction({
      account_id: "acc_demo_checking",
      date: on(1),
      amount: 520_000,
      description: "ACME Corp payroll",
      category_id: "cat_demo_salary",
    });
    transaction({
      account_id: "acc_demo_checking",
      date: on(2),
      amount: -185_000,
      description: "Rent - Maple Street",
      category_id: "cat_demo_rent",
      tags: ["tag_demo_shared"],
    });
    transaction({
      account_id: "acc_demo_checking",
      date: on(5),
      amount: -12_450,
      description: "City Power & Light",
      category_id: "cat_demo_utilities",
      tags: ["tag_demo_shared"],
    });
    transaction({
      account_id: "acc_demo_checking",
      date: on(6),
      amount: -6_999,
      description: "Fiber internet",
      category_id: "cat_demo_utilities",
      tags: ["tag_demo_subscription"],
    });
    transaction({
      account_id: "acc_demo_checking",
      date: on(10),
      amount: -38_500,
      description: "Rational Auto Finance payment",
      category_id: "cat_demo_transfer",
    });
    transaction({
      account_id: "acc_demo_loan",
      date: on(10),
      amount: 38_500,
      description: "Payment received",
      category_id: "cat_demo_transfer",
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
    });
    transaction({
      account_id: "acc_demo_savings",
      date: on(15),
      amount: 50_000,
      description: "Transfer from checking",
      category_id: "cat_demo_transfer",
    });
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
      { id: "split_demo_2", category_id: "cat_demo_shopping", amount: -3_000, note: "kitchen towels" },
    ],
  });
  // Random everyday spending fills the rest, spread over the last 90 days.
  while (transactions.length < transactionCount) {
    const merchant = MERCHANTS[Math.floor(next() * MERCHANTS.length)];
    const [description, categoryId, minimum, maximum, merchantTags] = merchant;
    const amount = maximum === minimum ? minimum : Math.round(minimum + next() * (maximum - minimum));
    transaction({
      account_id: spendingAccounts[Math.floor(next() * spendingAccounts.length)],
      date: daysAgo(anchor, Math.floor(next() * 90)),
      amount,
      description,
      category_id: categoryId,
      tags: merchantTags,
    });
  }
  transactions.sort((left, right) => left.date.localeCompare(right.date) || left.id.localeCompare(right.id));

  return { household, accounts, taxonomy, transactions };
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
