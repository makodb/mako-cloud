## Context

See proposal.md — Why. Rational today: twelve RxDB collections open per page (two directory,
ten household) of the thirteen the open-source build allows (`COL23`, a page-wide limit);
`mako/collections.json` at schema version 2, from which both the published platform schemas and
the local RxDB schemas derive; every write through `HouseholdWrites`; pure selectors under
`src/selectors`; engines shared with the functions under `functions/shared` (which import
nothing); a fake in-browser backend that is schema-agnostic; three suites (unit over built
selectors, Playwright against the fake, Playwright and a Rust smoke against the real stack).
The beta runs Rational as project `prj_43f3756d…` on `cloud-test.makodb.com`, with the site
published from `github.com/shuaimu/rational` by `scripts/export-rational-app.mjs`.

Schema publication is compatible when every existing field is unchanged or widened (a superset
enum), no new field is required, and every stored document satisfies the proposed schema. The
functions name the model's schema version on every write.

## Goals / Non-Goals

**Goals:**
- Reach the parity matrix's ninety percent on the platform surface Rational already uses, with
  no new collection and no new runtime dependency in the published app.
- Keep every number consistent across screens: the dashboard, budget, cash flow, and account
  pages derive from the same selectors with the same exclusions.
- Keep the nightly job and the browser deciding the same things with the same shared engines.

**Non-Goals:**
- Monarch features that need a third party Rational does not have: the AI assistant, 13 000
  institutions (Plaid Sandbox stands in), Zillow and VIN valuations, market prices and
  benchmarks, receipt OCR, advisor sharing, native mobile apps. The matrix lists them as out of
  scope with the reason.
- Multi-currency conversion; each account keeps its currency and nothing is summed across.
- A platform change. If one turns out to be needed it is a finding, recorded and fixed in the
  platform under its own change.

## Decisions

**1. No new collections; new document types ride existing ones behind `kind`.**
`taxonomy` gains `kind: "group"` and `kind: "merchant"` beside categories and tags; `budgets`
gains an optional `kind` (`category`, `group`, `income`, `flex`) with `category_id` naming the
category or group and `bud_income.<month>` / `bud_flex.<month>` as the household-level ids;
`net_worth_snapshots` gains `balances: [{account_id, balance}]`; holdings are embedded in the
investment account (`holdings: [...]`, at most 256); household preferences (`budget_mode`) live
on the `households` document. *Alternative rejected:* a thirteenth `misc` collection — it spends
the last slot on the first feature and leaves nothing for the next.

**2. Schema version 3, additive only.** Every new field is optional; every enum only widens
(`accounts.type`, `taxonomy.kind`, `alerts.alert_kind`, `budgets.kind`). The bootstrap's existing
`schema publish` path upgrades a deployment and fails loudly on `migration_required`; the
functions' `SCHEMA_VERSION` moves to 3 with the model; a device holding version 2 locally rebuilds
its replica on open, as `openDatabase` already does. Deploy order on the beta: publish the schema,
redeploy the functions, publish the site — a stale site against the new schema shows the existing
"needs an update" recovery notice rather than corrupting anything.

**3. Transfers are a pairing, not a category.** Two legs share a `transfer_id`; a leg's
`category_id` may be any category or none. Cash flow, budgets, spending reports, and recurrence
detection exclude any transaction with a `transfer_id`, any with `hidden: true`, and any with
`adjustment: true`. Pairing is suggested (opposite amounts on two accounts within three days) and
confirmed by an editor; the seed pairs its own transfers. *Alternative rejected:* excluding the
`transfer` category kind alone — it cannot tell a real transfer from a miscategorized purchase, and
Monarch's model is the pair.

**4. Tracked values are balance updates.** Updating a real-estate or vehicle value writes a
transaction with `adjustment: true`, `hidden: true`, description "Balance update", and the
difference as its amount, so the account's balance derivation, snapshots, and history stay one
mechanism. Investment balances add the holdings' value (`round(quantity × price)` in minor units)
to the account's cash; a price update is an account edit, not a transaction.

**5. Merchants are taxonomy entries with patterns.** A merchant document carries a display
`name` and `patterns` (normalized descriptions). A transaction's merchant is `merchant_id` when
set, else the merchant whose pattern equals the transaction's normalized description, else a
cleaned form of the description (title-cased normalized text). Renaming edits the document;
merging rewrites `merchant_id` on the loser's transactions, appends its patterns to the winner,
and deletes the loser. Rules can set `merchant_id`.

**6. Budgets: one document shape, three levels, two presentations.** A budget names a category,
a group, expected income, or the flex number; spent for a group is the sum over its categories;
a category under a budgeted group shows no budget of its own. Categories carry
`budget_bucket` (`fixed`, `flexible`, `non_monthly`) and, for non-monthly, `target_amount` and
`target_months`, whose monthly share is `target_amount / target_months`. Flex mode is a
presentation: flexible = income − Σ fixed budgets − Σ non-monthly shares − Σ planned goal
contributions, and flexible spending is Σ spending in flexible-bucket categories. Copying a month
inserts what the target month lacks and never overwrites.

**7. Goals: two kinds, one document.** `kind: "save" | "pay_down"`, `account_ids`,
`planned_monthly`, `priority`, `progress_source: "contributions" | "balance"`. Balance-based
progress for a save goal is Σ linked balances − `starting_balance` (recorded at creation, so the
goal measures what was saved since); a pay-down goal's progress is `starting_balance − |owed|`.
On-track compares progress with `planned_monthly × months since creation`; payoff month is
`ceil(|owed| / planned_monthly)` months on. *Alternative rejected:* amortization with an interest
rate — Rational has no rate it trusts, and Monarch's projection at the planned payment is what
people read.

**8. Recurrences gain a life cycle.** `source: "detected" | "manual"`, `status` widened with
`paused`, `category_id`, `merchant_id`, `last_paid_transaction_id`. A bill is paid when a
transaction on its account with its normalized description lands within the interval's slack of
its expected date, which advances `next_date`; late when today is past `next_date` with no such
transaction. The nightly job advances paid bills too, so a closed laptop does not leave a bill
"late". The calendar is a month grid over confirmed recurrences' next dates.

**9. Charts are hand-rolled SVG.** A small `src/ui/charts` set: line with axes and a hover
readout, bars, stacked bars, donut, treemap (squarified), Sankey (three columns: income
categories → groups → categories and savings), and a month calendar. No charting dependency:
the published app's whole point is what it does not need, and every chart's numbers come from
a selector the tests can assert. *Alternative rejected:* Recharts or Chart.js — a large
dependency for six charts, and untestable without a browser.

**10. Navigation becomes Monarch's shape.** A left sidebar: Dashboard, Accounts,
Transactions, Cash Flow, Budget, Recurring, Goals, Investments, Settings; a notification bell
and the space switcher in the top bar. Settings is a hub with sub-pages (Household, Members,
Categories, Tags, Merchants, Rules, Connections, Import, Notifications, Data). Hash routes gain
parameters: `#/accounts/<id>`, `#/transactions?…` (every filter in the query),
`#/cash-flow?range=…`, `#/budget?month=…`, `#/recurring?month=…&view=calendar`,
`#/settings/<page>`. Existing routes (`#/reports`, `#/plan`, `#/household`, …) redirect.

**11. Default taxonomy is seeded on first open, idempotently.** When a household opens with no
taxonomy documents, the app writes the default groups and categories with ids deterministic per
household (`grp_<household>.<slug>`, `cat_<household>.<slug>`), so two devices seeding at once
write the same documents and the conflict handler settles them, while two households never
collide — a collection's ids are one namespace for the environment, not one per household. The demo household keeps its own richer seed.

**12. Review state is a flag with a definition.** `reviewed: true` is set on everything a member
creates by hand; sync and import leave it unset; the review queue is transactions without
`reviewed` that carry an `external_id` or `import_batch_id`. Marking reviewed is a patch, singly
or in bulk, and a rule action.

**13. Rules gain conditions and actions without breaking the shared engine.** `match` gains
`description_equals`, `merchant_id`, `category_id`, and `direction` (`expense` | `income`);
`amount_min`/`amount_max` stay as the range. Actions gain `set_merchant_id`, `hide`, and
`mark_reviewed`. `priority` remains the order; reordering rewrites priorities as consecutive
integers. The nightly job's `filings` apply the same engine, so a rule that hides or marks
reviewed does so overnight too.

**14. The nightly job's snapshot carries every account.** `balances` records each open account's
balance (holdings included) so account pages have a history; `bill_due` and `goal_reached` are
decided from recurrences and goals read in the same pass; `sync_error` is written by the sync
when a connection's pass fails, with the connection as subject, so a still-failing connection is
one alert.

**15. Parity is a file and a validator.** `examples/rational/MONARCH-PARITY.md` is a table with
columns feature, area, status (`yes` | `partial` | `no` | `out of scope`), where, and note;
`scripts/validate-rational-parity.js` parses it, requires every row complete, counts
`yes` as 1 and `partial` as 0.5 over rows not out of scope, and fails under 90%. It runs with the
other documentation validators.

**16. Screens are built in parallel on distinct files.** Each screen owns its module, its
selectors file, its stylesheet (`src/ui/styles/<screen>.css`, imported by the screen), and its
Playwright spec; the model, writes, router, shell, shared engines, and demo data land first so
the screens build on stable foundations.

## Risks / Trade-offs

- [Thirteen-collection cap leaves no room for the next feature] → the one spare slot stays
  unused by this change; the `kind` pattern absorbs new small document types.
- [A published site at version 2 against an environment at version 3] → the recovery notice
  already handles it; the deploy publishes the schema and the site within minutes of each other.
- [Client-side reports grow heavier with merchants, Sankey, treemap, trends] → every derivation
  is a memoized selector over the transaction array; the existing 50 000-transaction measurement
  is repeated for the new selectors and recorded in the findings log if it degrades.
- [Balance-based goal progress double counts when two goals link one account] → allowed, as
  Monarch allows it; the goal page says so where it happens.
- [Transfer suggestions pair the wrong legs] → a suggestion is confirmed by an editor and undone
  by unpairing; nothing is paired automatically except the seed's own transfers.
- [Default seeding races across devices] → deterministic ids make the race a benign conflict.
- [Bulk edits push many documents] → bulk actions patch in one loop under one pending-write
  count per document; the offline queue already handles hundreds.

## Migration Plan

1. Land the model at version 3 with every suite green locally; the fake backend needs no
   change beyond demo data.
2. On the beta: `node scripts/bootstrap.mjs --functions …` against the Rational project
   publishes version 3 of every collection (failing loudly on `migration_required`), creates any
   new index, and redeploys the three functions; then `node scripts/export-rational-app.mjs`
   publishes the site.
3. Rollback: the site is a static publish (revert the commit in the application repository); a
   schema version cannot be lowered, but version 3 accepts every version-2 document, so an older
   site would only see the "needs an update" notice, never a data loss.

## Open Questions

- Whether the beta's hosted qualification should be re-run for an application-only change. The
  Rational smoke publishes the model from the same files and passes locally; the plan re-runs it
  locally and leaves a platform requalification for a platform change.
