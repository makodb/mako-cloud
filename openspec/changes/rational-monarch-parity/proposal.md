## Why

Rational was built as a Monarch-style money manager, but feature for feature it is still a
long way from Monarch: no dashboard, a flat transaction list with two filters, budgets that are
one number per category, reports that are tables, recurring charges with no calendar, savings
goals with no debt side, no investments at all, no merchants, no review queue, no transfers,
no settings. The platform proof it exists for is only as strong as the product on top of it, and
a household comparing the two today would not stay. This change takes Rational to at least
ninety percent of Monarch's feature set, measured against a written parity matrix, on the
same platform surface it uses today — so that what it proves is that a real product fits.

## What Changes

- **Dashboard.** A landing page with net worth and its trend, this month's spending against
  last month's, the budget's headroom, the next bills, goal progress, the review queue, unread
  alerts, recent transactions, and the investment total.
- **Accounts.** Account types grow to Monarch's classes (real estate, vehicles, crypto, other
  assets and liabilities); accounts are grouped by class with subtotals; a net-worth chart over a
  chosen range; each account has a detail page with its balance history and transactions; an
  account can be hidden from net worth; a tracked asset's value is updated in place without
  becoming spending.
- **Transactions.** Search and filters (account, category, tag, merchant, date range, amount
  range, review state, hidden, transfers); date-grouped list with a detail panel; bulk edit;
  merchants with display names; a review queue for what arrived by sync or import; transfer
  pairing that keeps both legs out of cash flow; hide from budgets and reports; export to CSV;
  a rule created from a transaction.
- **Categories and merchants.** Category groups with a kind, an icon per category, ordering,
  a default set for a new household, delete-and-reassign, and merchants that can be renamed and
  merged; each category carries a budget bucket (fixed, flexible, non-monthly).
- **Budgets.** A month-by-month budget page with expected income, group and category budgets,
  rollover, copy from last month, non-monthly targets, unbudgeted spending, and a flex mode
  where the flexible number is what is left after income, fixed costs, and goals.
- **Cash flow and reports.** A date range (month, quarter, year, custom); income, expenses,
  savings, and savings rate; monthly bars; a Sankey of income into spending; spending by
  group, category, merchant, tag, and account with a treemap; a category's trend over time;
  transfers, hidden transactions, and balance updates excluded everywhere; CSV export.
- **Recurring.** A calendar and a list of recurring charges with their monthly total; a charge
  marked recurring by hand; a bill shown paid when its transaction arrives and late when it does
  not; an alert before a bill is due.
- **Goals.** Save-up goals whose progress can follow linked accounts or recorded contributions,
  with a planned monthly amount and on-track status; pay-down goals on a liability with a
  projected payoff; priorities; an alert when a goal is reached.
- **Investments.** Holdings per investment account (symbol, quantity, price, cost basis, asset
  class) valued into the account balance, with allocation and gain shown.
- **Notifications and settings.** A notification bell; alert kinds for a bill due soon, a goal
  reached, and a connection failing to sync; a settings hub for the household (name, currency,
  budget mode), members, categories, tags, merchants, rules, connections, import, notifications,
  and data export.
- **Rules.** Conditions on statement text (contains or exact), merchant, amount (over, under,
  between, by direction), account, and category; actions that set category, tags, merchant,
  hide, and mark reviewed; rules run in their listed order and can be reordered.
- **Automation.** The nightly job records each account's balance in the day's snapshot, applies
  the extended rules, and decides the new alert kinds; the sync marks what it imports as needing
  review and reports a failing connection.
- **Parity is measured.** `examples/rational/MONARCH-PARITY.md` lists Monarch's features with
  Rational's status for each, and a validator refuses a matrix under ninety percent.
- No new collections: RxDB's open-source build opens thirteen at once and Rational holds
  twelve. New document types share existing collections behind their `kind`, new fields are
  optional, and the model moves to schema version 3 compatibly.

## Capabilities

### New Capabilities

_None._

### Modified Capabilities

- `samples/rational-money-app`: "Accounts and balances" (classes, hiding, tracked values,
  detail pages), "Transactions with categories, splits, and receipts" (search, filters, bulk
  edit, merchants, review, transfers, hiding, export), "Categorization rules" (conditions,
  actions, order), "Monthly budgets with rollover" (income, groups, flex mode, copy,
  non-monthly), "Recurring transactions and upcoming bills" (calendar, manual, paid and late),
  "Goals with contributions" (linked progress, pay-down, priorities), "Net worth and reports"
  (ranges, Sankey, treemap, trends, exclusions, per-account history, export), "Alerts to a
  household endpoint" (bill due, goal reached, sync failing); new requirements for the
  dashboard, category groups and merchants, investment holdings, household settings and data
  export, and measured parity.

## Impact

- `examples/rational`: `mako/collections.json` (schema version 3, additive), `src/model`,
  `src/data/writes.ts`, new and rewritten screens under `src/ui`, new selectors and SVG chart
  components, `functions/shared` engines, `functions/nightly` and `functions/institution-sync`,
  `scripts/demo-data.mjs`, the fake backend, all three test suites, `README.md`, and a new
  `MONARCH-PARITY.md`.
- Root: `scripts/validate-rational-parity.js` and its npm script; `docs/rational.md`;
  `docs/requirements-traceability.md` rows for every new scenario.
- The beta: the Rational environment's collections republished at schema version 3, its three
  functions redeployed, and the published site regenerated. No platform change is planned; a
  platform defect the work exposes is recorded in the findings log and fixed in the platform.
- No breaking changes for the platform. For Rational, a device holding the version-2 local
  schema rebuilds its replica on first open, as it already does on a model change.
