## 1. Model, writes, and shared engines (schema version 3)

- [x] 1.1 `mako/collections.json` to schema version 3, additive: account types widened (`real_estate`, `vehicle`, `crypto`, `other_asset`, `other_liability`) with `hide_from_net_worth`, `owner_id`, `holdings[]`; transactions gain `merchant_id`, `reviewed`, `hidden`, `adjustment`, `transfer_id`; taxonomy `kind` widened with `group` and `merchant` plus `icon`, `sort_order`, `budget_bucket`, `target_amount`, `target_months`, `patterns[]`; budgets gain `kind`; recurrences gain `source`, `paused` status, `category_id`, `merchant_id`, `last_paid_transaction_id`; goals gain `kind`, `account_ids`, `planned_monthly`, `priority`, `progress_source`, `starting_balance`; snapshots gain `balances[]`; alerts `alert_kind` widened with `bill_due`, `goal_reached`, `sync_error` and gain `recurrence_id`, `goal_id`, `connection_id`; households gain `budget_mode`. `src/model/types.ts` mirrors it; the functions' `SCHEMA_VERSION` moves to 3.
- [x] 1.2 `HouseholdWrites`: account holdings and price updates, balance updates for tracked accounts, hide from net worth; transaction merchant, reviewed (set on manual creation), hidden, transfer pairing and unpairing, bulk patch; groups, merchants (create, rename, merge), category icon/bucket/order, delete-and-reassign; budget kinds, copy month; manual recurrence, paused, paid advance; goal kinds and fields; household settings; default taxonomy seeding with deterministic ids.
- [x] 1.3 Shared engines under `functions/shared/`: rules with the new conditions and actions and ordered application; transfers (suggest pairs); exclusions (`isSpending`-style predicate for transfers, hidden, adjustments) used by budgets, reports, and alerts; recurrence paid/late resolution; goal progress and payoff projection; snapshot balances; `bill_due`, `goal_reached`, `sync_error` alert rules; merchant resolution. Unit tests for each.
- [x] 1.4 Selectors: transaction search and filters (text, account, category, tag, merchant, date range, amount range, review, hidden, transfer) with a query-string codec; cash flow over a range with savings rate; spending by group, merchant, tag; Sankey flows; treemap layout; category trend; per-account balance history; budget month model in category and flex modes with group budgets, income, unbudgeted, copy; recurring calendar and paid/late; goals by priority with on-track; holdings valuation and allocation; dashboard summary; CSV export. Unit tests for each.
- [x] 1.5 Demo data: default groups and icons, merchants, paired transfers, a real-estate and an investment account with holdings, a hidden transaction, a balance update, goals of both kinds, confirmed recurrences, budgets across months, snapshots with balances, synced transactions awaiting review.
- [x] 1.6 Functions: `nightly` writes `balances` into the snapshot, advances paid recurrences, applies the extended rules, and decides `bill_due` and `goal_reached`; `institution-sync` leaves imported transactions unreviewed and writes `sync_error` on a failing connection; both name schema version 3. Unit tests over the shared decisions.

## 2. Shell, navigation, and dashboard

- [x] 2.1 Router with parameters (`#/accounts/<id>`, `#/transactions?…`, `#/cash-flow?range=…`, `#/budget?month=…`, `#/recurring?month=…&view=…`, `#/goals`, `#/investments`, `#/settings/<page>`) and redirects for the old routes; a sidebar shell with the notification bell and space switcher; per-screen stylesheets under `src/ui/styles/`.
- [x] 2.2 Dashboard screen: net worth and trend, this month against last, budget headroom, next bills, goals, review count, unread alerts, recent transactions, investments total, each linking to its screen; Playwright spec against the fake.

## 3. Accounts, net worth, and investments

- [x] 3.1 Accounts screen: classes with subtotals, net-worth chart with range picker, hidden accounts, closed accounts, account form with the new types and owner; Playwright spec.
- [x] 3.2 Account detail page: balance history from snapshots, transactions of the account, edit, hide, close, connection status, balance update for tracked accounts, holdings for investment accounts; Playwright spec.
- [x] 3.3 Investments screen: holdings across accounts with value and gain, allocation by asset class, add/update/remove holdings and prices; Playwright spec.

## 4. Transactions

- [x] 4.1 Transactions screen: search box, filter bar carried in the address, date-grouped list with merchant, category with group icon, tags, review and hidden markers, transfer marker; count and net total; CSV export of the list; Playwright spec covering search, filters, and the address.
- [x] 4.2 Transaction detail panel: edit fields, splits, receipts, notes, tags, hide, reviewed, mark recurring, pair as transfer (with suggestions), create a rule from it, delete; Playwright spec.
- [x] 4.3 Bulk selection and actions (category, tags, hide, reviewed, delete) and the review queue filter; Playwright spec.

## 5. Categories, tags, merchants, and rules

- [x] 5.1 Categories settings: groups with ordering, categories with icon, bucket, and non-monthly target, archive, delete-and-reassign; default set seeded for a new household; Playwright spec covering seeding and delete-and-reassign.
- [x] 5.2 Merchants settings: list with counts and totals, rename, merge; Playwright spec.
- [x] 5.3 Rules settings: editor with the new conditions and actions, ordering by move up/down, preview count, apply to existing, rule from a transaction; Playwright spec covering order and the merchant/reviewed actions.

## 6. Budget

- [x] 6.1 Budget screen in category mode: month navigation, expected income, groups with subtotals and optional group budgets, category rows with inline amounts, rollover, non-monthly shares, unbudgeted spending, totals and left to spend, copy last month; Playwright spec.
- [x] 6.2 Budget screen in flex mode: fixed, non-monthly, and flexible buckets with the flexible number derived from income, fixed, non-monthly, and planned goal contributions; the mode switch in household settings; Playwright spec.

## 7. Cash flow and reports

- [x] 7.1 SVG chart components: line with axes and hover readout, bars and stacked bars, donut, treemap, Sankey, month calendar; unit tests over their layout functions.
- [x] 7.2 Cash-flow screen: range picker (month, quarter, year, custom), income/spending/savings/rate tiles, monthly bars, Sankey, spending by group, category (treemap), merchant, tag, and account, category trend, exclusions applied, CSV export, as-of stamp; Playwright spec.

## 8. Recurring, goals, alerts

- [x] 8.1 Recurring screen: list with monthly total, calendar view, upcoming with paid and late states, confirm/adjust/pause/dismiss, mark recurring from a transaction; Playwright spec.
- [x] 8.2 Goals screen: save-up and pay-down goals, linked accounts, planned monthly, progress source, priority ordering, on-track, payoff projection, contributions; Playwright spec.
- [x] 8.3 Notifications: bell with unread count, notification settings with the new kinds (`bill_due` days, `goal_reached`, `sync_error`), history with mark read; Playwright spec covering the bell and the new settings; unit tests for the new alert rules.

## 9. Settings hub and data

- [x] 9.1 Settings hub with sub-pages: household (name, currency, budget mode), members (existing screen), categories, tags, merchants, rules, connections, import, notifications, data export (accounts and transactions CSV); Playwright spec covering the export and the mode switch.

## 10. Parity, docs, and traceability

- [x] 10.1 `examples/rational/MONARCH-PARITY.md` with every Monarch feature, its status, and where it lives; `scripts/validate-rational-parity.js` and `npm run validate:rational-parity`, run with the documentation validators; a unit test of the validator over a fixture.
- [x] 10.2 `examples/rational/README.md` (model, screens, functions), `docs/rational.md`, `docs/requirements-traceability.md` rows for every new scenario; spec sync into `openspec/specs/samples/rational-money-app/spec.md`.

## 11. Verification and the beta

- [x] 11.1 Every local gate green: `npm run format:check`, `lint`, `typecheck`, `test:unit`, the Rational browser suite, `validate:docs`, `validate:traceability`, `validate:rational-parity`; the Rational smoke (`npm run test:rational-smoke`); the live suite where the stack is available; the 50 000-transaction report measurement repeated for the new selectors.
- [x] 11.2 The beta: publish schema version 3 and redeploy the functions with the bootstrap against the Rational project, verify a version-3 behavior live, regenerate the published site, and record anything the work exposed in `PLATFORM-FINDINGS.md`.
