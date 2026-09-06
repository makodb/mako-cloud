# How close Rational is to Monarch

Rational is built to be a Monarch-style money manager, and this table is the honest account of
how close it is: every feature Monarch offers, whether Rational has it, and where. In the
platform repository it is read by `scripts/validate-rational-parity.js`
(`npm run validate:rational-parity`), which refuses a malformed row and a coverage under ninety
percent.

Status means:

- `yes` — the feature is there and a test exercises it.
- `partial` — some of it is there; the note says what is missing. Counts as half.
- `no` — not there. Counts as zero.
- `out of scope` — needs something Rational deliberately does not have (a third-party service,
  a native app, a model). Not counted; the note says why.

Coverage is the score over the rows that are not out of scope.

| Area | Feature | Status | Where in Rational | Note |
| ---- | ------- | ------ | ----------------- | ---- |
| Accounts | Connect a bank through an aggregator | partial | Settings › Connections, Plaid Link | Plaid Sandbox only; the beta holds sandbox keys |
| Accounts | Thousands of institutions, several aggregators | out of scope | — | one aggregator in sandbox; institutions are Plaid's business |
| Accounts | Manual accounts | yes | Accounts › New account | |
| Accounts | Account classes: cash, credit, investment, loan, real estate, vehicle, crypto, other | yes | Accounts, `ACCOUNT_CLASSES` | |
| Accounts | Accounts grouped by class with subtotals | yes | Accounts | |
| Accounts | Net worth summary | yes | Accounts, Dashboard | |
| Accounts | Net worth over time with a range picker | yes | Accounts | 1M, 3M, 6M, 1Y, all |
| Accounts | Account detail with balance history and transactions | yes | Accounts › account | history from nightly snapshots |
| Accounts | Hide an account from net worth | yes | Account detail | |
| Accounts | Close and reopen an account | yes | Accounts | |
| Accounts | Update the value of a tracked asset by hand | yes | Account detail › Update value | a balance update, never spending |
| Accounts | Home and vehicle values from Zillow and VIN lookups | out of scope | — | a valuation service Rational has no egress to |
| Accounts | CSV import with column mapping and duplicate detection | yes | Settings › Import | |
| Accounts | Import from Mint and other apps | partial | Settings › Import | any CSV with date, description, and amount columns; no Mint-specific mapping |
| Accounts | Connection status and last sync | yes | Settings › Connections | |
| Accounts | Account ownership within the household | partial | Account form › Owner | an owner per account; no per-transaction owner |
| Transactions | List grouped by date | yes | Transactions | |
| Transactions | Search | yes | Transactions | description, merchant, notes, amount |
| Transactions | Filters: account, category, tag, merchant, amount, date range | yes | Transactions | carried in the address |
| Transactions | Sort | yes | Transactions | by date or amount |
| Transactions | Edit category, merchant, date, notes, tags | yes | Transaction panel | |
| Transactions | Splits | yes | Transaction panel | must add up |
| Transactions | Tags | yes | Transaction panel, Settings › Tags | |
| Transactions | Notes | yes | Transaction panel | |
| Transactions | Attachments | yes | Transaction panel › Receipts | images and PDFs in the receipts bucket |
| Transactions | Receipt scanning | out of scope | — | needs an OCR service |
| Transactions | Merchants with display names | yes | Transactions, Settings › Merchants | |
| Transactions | Merchant logos | out of scope | — | a logo service; Rational shows initials |
| Transactions | Needs review and mark reviewed | yes | Transactions › Needs review | synced and imported transactions until a member looks |
| Transactions | Hide from budgets and cash flow | yes | Transaction panel, bulk edit | |
| Transactions | Transfers paired across accounts | yes | Transaction panel › Transfer | suggestions confirmed by an editor |
| Transactions | Bulk edit | yes | Transactions › select | category, tags, hide, reviewed, delete |
| Transactions | Pending transactions | partial | Transactions | shown when the source says pending; Plaid pending is replaced when it posts |
| Transactions | Duplicate detection | yes | Import, nightly job | |
| Transactions | Mark a transaction recurring | yes | Transaction panel | |
| Transactions | Create a rule from a transaction | yes | Transaction panel | |
| Transactions | Export transactions as CSV | yes | Transactions, Settings › Data | |
| Transactions | Activity log of edits | no | — | |
| Transactions | Delete transactions | yes | Transaction panel, bulk edit | deletes receipts with it |
| Transactions | Manual entry | yes | Transactions › New | |
| Categories | Category groups of income, expense, and transfer kind | yes | Settings › Categories | |
| Categories | A default set for a new household | yes | seeded on first open | deterministic ids, safe across devices |
| Categories | Custom categories with icons | yes | Settings › Categories | |
| Categories | Reorder categories and groups | yes | Settings › Categories | |
| Categories | Disable a category | yes | Settings › Categories › Archive | |
| Categories | Delete a category and move its transactions | yes | Settings › Categories | |
| Rules | Conditions: statement text, merchant, amount, account, category | yes | Settings › Rules | |
| Rules | Actions: category, tags, merchant, hide, mark reviewed | yes | Settings › Rules | |
| Rules | Rules run in order and can be reordered | yes | Settings › Rules | |
| Rules | Apply a rule to existing transactions | yes | Settings › Rules › Apply | |
| Rules | Preview how many transactions a rule touches | yes | Settings › Rules | |
| Rules | Split by rule | no | — | |
| Rules | Link a goal by rule | no | — | |
| Budget | Monthly budget per category | yes | Budget | |
| Budget | Rollover | yes | Budget | |
| Budget | Budget a whole group | yes | Budget | |
| Budget | Expected income | yes | Budget | |
| Budget | Flex budgeting: fixed, flexible, non-monthly | yes | Budget (flex mode) | |
| Budget | Non-monthly targets over a frequency | yes | Settings › Categories, Budget | |
| Budget | Left to budget and totals | yes | Budget | |
| Budget | Copy last month | yes | Budget | |
| Budget | Unbudgeted spending | yes | Budget | |
| Budget | Month navigation | yes | Budget | |
| Budget | Forecast of the month's end | no | — | |
| Budget | Exclude a category from the budget | partial | Transaction panel › Hide | transactions are hidden, not categories |
| Cash flow | Income, spending, savings, savings rate | yes | Cash Flow | |
| Cash flow | Date range: month, quarter, year, custom | yes | Cash Flow | |
| Cash flow | Sankey of income into spending | yes | Cash Flow | |
| Cash flow | Spending by category | yes | Cash Flow | |
| Cash flow | Spending by group | yes | Cash Flow | |
| Cash flow | Spending by merchant | yes | Cash Flow | |
| Cash flow | Spending by tag | yes | Cash Flow | |
| Cash flow | Spending by account | yes | Cash Flow | |
| Cash flow | A category's trend over time | yes | Cash Flow | |
| Cash flow | Treemap of spending | yes | Cash Flow | |
| Cash flow | Income and spending over time | yes | Cash Flow | monthly bars |
| Cash flow | Export a report as CSV | yes | Cash Flow | |
| Recurring | Detect recurring charges | yes | Recurring, nightly job | |
| Recurring | List with the monthly total | yes | Recurring | |
| Recurring | Calendar view | yes | Recurring › Calendar | |
| Recurring | Upcoming bills | yes | Recurring, Dashboard | |
| Recurring | Mark a transaction recurring by hand | yes | Transaction panel | |
| Recurring | Paid and late states | yes | Recurring | |
| Recurring | Edit, pause, and dismiss a recurrence | yes | Recurring | |
| Recurring | Reminder before a bill is due | yes | Settings › Notifications | decided overnight |
| Goals | Savings goals with a target and date | yes | Goals | |
| Goals | Progress from linked accounts | yes | Goals | |
| Goals | Planned monthly contribution and on-track status | yes | Goals | |
| Goals | Debt pay-down goals | yes | Goals | |
| Goals | Payoff projection | yes | Goals | at the planned payment |
| Goals | Goal priorities | yes | Goals | |
| Goals | Contributions history | yes | Goals | |
| Goals | Planned contributions inside the budget | yes | Budget (flex mode) | |
| Investments | Holdings per account | yes | Investments | |
| Investments | Allocation by asset class | yes | Investments | |
| Investments | Cost basis and gain | yes | Investments | |
| Investments | Live prices and performance against benchmarks | out of scope | — | a market-data service |
| Investments | Investment transactions | partial | Account detail | cash movements on the account; no trades ledger |
| Household | Invite members with roles | yes | Settings › Members | |
| Household | Several households per person | yes | space switcher | |
| Household | Share with a financial advisor | out of scope | — | an advisor product |
| Household | Personal versus shared money | partial | Account form › Owner | by account, not by transaction |
| Notifications | Large transaction | yes | Settings › Notifications | |
| Notifications | Budget exceeded | yes | Settings › Notifications | |
| Notifications | Low balance | yes | Settings › Notifications | |
| Notifications | Bill due soon | yes | Settings › Notifications | |
| Notifications | Goal reached | yes | Settings › Notifications | |
| Notifications | Connection failed to sync | yes | Settings › Notifications | |
| Notifications | In-app notification center | yes | bell in the top bar | |
| Notifications | Email and push delivery | partial | webhook per household | a signed webhook, not email or push |
| Dashboard | Overview of net worth, spending, budget, bills, goals | yes | Dashboard | |
| Dashboard | Rearrange and hide widgets | yes | Dashboard › Customize | kept per device |
| Dashboard | Net worth trend | yes | Dashboard | |
| Settings | Household name and currency | yes | Settings › Household | |
| Settings | Budget mode | yes | Settings › Household | |
| Settings | Export all data | yes | Settings › Data | accounts and transactions as CSV |
| Settings | Category, tag, merchant, and rule management | yes | Settings | |
| Settings | Sign in with password, a provider, or a magic link | yes | Sign in | |
| Settings | Two-factor authentication | out of scope | — | the platform's project auth has no second factor yet |
| Settings | Dark mode | yes | top bar › theme | |
| Settings | Native mobile apps | out of scope | — | a web app |
| Settings | AI assistant | out of scope | — | a model Rational does not call |
