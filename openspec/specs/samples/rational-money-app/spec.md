# Rational Money App Specification

## Purpose

Rational is the platform's reference money-management application: a Monarch-style product that households use to track accounts, transactions, budgets, and goals, built only on what Mako Cloud offers applications, and kept running as the standing proof that those capabilities hold up together.

## Requirements

### Requirement: Sign-in by every application method
Rational SHALL let a person sign up and sign in with email and password, with a registered external provider, and by magic link, and SHALL keep the session across browser restarts until it is signed out or revoked. A sign-in method the environment has not enabled MUST be shown as unavailable rather than failing on use.

#### Scenario: A person signs in with a provider
- **WHEN** a person chooses a provider the environment enables and completes its flow
- **THEN** Rational exchanges the returned code for a session, shows the person's household data, and reloading the page keeps them signed in

#### Scenario: A person signs in by magic link
- **WHEN** a person requests a magic link and opens it in the browser
- **THEN** Rational redeems the link once for a session, and opening it again does not sign anyone in

### Requirement: Households shared under roles
A household SHALL group accounts and money data shared by its members. The person who creates a household is its owner; the owner SHALL invite members by email and assign the roles owner, editor, or viewer; membership and role SHALL govern every read and write of household data through document policies, so that a member sees and changes only the households they belong to at the role they hold, and a removed member loses access without waiting for their session to expire.

#### Scenario: An invited member joins
- **WHEN** an owner invites a person and that person signs in with the invited address
- **THEN** the household's accounts and transactions replicate to the new member, and a viewer's edits are refused while an editor's are accepted

#### Scenario: A member is removed
- **WHEN** an owner removes a member who has the app open
- **THEN** the removed member's local copy of the household's data is cleared, further reads and writes are refused, and the other members are unaffected

### Requirement: Accounts and balances
Rational SHALL track accounts of the classes cash (checking, savings, cash), credit, investment, loan, real estate, vehicle, crypto, other asset, and other liability, each with a currency and a current balance derived from transactions and an opening balance — and, for an investment account, its holdings' value — and SHALL let a household add accounts manually, import a CSV statement into an account, or connect a simulated institution or Plaid that provides accounts and transactions. Accounts SHALL be shown grouped by class with a subtotal per class; an account MAY be hidden from net worth; an account whose value is tracked rather than transacted (real estate, vehicle, other asset, other liability) SHALL have its value updated in place by a balance update that never counts as income or spending; and every account SHALL have a detail page with its balance history and its transactions.

#### Scenario: A CSV statement is imported
- **WHEN** an editor imports a CSV statement with a mapping of its columns
- **THEN** the transactions are created under the account, rows that match existing transactions by date, amount, and normalized description are flagged as duplicates and not created, and the account balance reflects the import

#### Scenario: A simulated institution syncs
- **WHEN** an editor connects the simulated institution and the connection's schedule runs
- **THEN** new transactions from the institution appear under the connected accounts without duplicating ones already synced, and the connection shows its last sync time and outcome

#### Scenario: A tracked asset's value is updated
- **WHEN** an editor updates the value of a real-estate account from one amount to another
- **THEN** the account's balance and the household's net worth move by the difference, the update appears in the account's history as a balance update, and the month's income, spending, and cash flow are unchanged

#### Scenario: An account is hidden from net worth
- **WHEN** an editor hides an account from net worth
- **THEN** net worth and the class subtotals exclude its balance while the account, its transactions, and its detail page remain available

### Requirement: Transactions with categories, splits, and receipts
A transaction SHALL carry a date, amount, currency, description, account, category, merchant, tags, notes, optional splits whose amounts sum to the transaction, optional receipt files, a review state, whether it is hidden from budgets and reports, and, when it is one leg of a transfer, the transfer that pairs it with its other leg. Editors SHALL create, edit, split, categorize, hide, mark reviewed, pair as transfers, and delete transactions singly or several at once; every member SHALL see them; the list SHALL be searchable by text and filterable by account, category, tag, merchant, date range, amount range, review state, hidden state, and transfers, and exportable as CSV; receipts SHALL be readable only by the household's members. A transaction that arrived by sync or import SHALL need review until a member marks it reviewed; a transaction a member typed SHALL not.

#### Scenario: A receipt is attached
- **WHEN** an editor attaches an image to a transaction
- **THEN** the file is stored under the household, members can open it from the transaction, a person outside the household cannot read it, and deleting the transaction deletes its receipts

#### Scenario: A split does not add up
- **WHEN** an editor saves splits whose amounts differ from the transaction amount
- **THEN** the save is refused with the difference shown, and nothing is written

#### Scenario: Transactions are found by search and filters
- **WHEN** a member types text into the search and narrows by a category, a tag, a date range, and an amount range
- **THEN** only the transactions matching every condition are listed, the count and net total describe that list, and the filters are carried in the address so a reload shows the same list

#### Scenario: Several transactions are edited at once
- **WHEN** an editor selects several transactions and sets a category, adds a tag, and marks them reviewed in one action
- **THEN** every selected transaction carries the new category, the added tag, and the reviewed state, and no unselected transaction changed

#### Scenario: A transfer is paired and leaves cash flow
- **WHEN** an editor pairs an outflow on one account with the matching inflow on another as a transfer
- **THEN** both legs name the same transfer, both are excluded from income, spending, budgets, and cash flow, and the two account balances are unchanged

#### Scenario: A synced transaction needs review until someone looks
- **WHEN** a transaction arrives by sync or import and a member later marks it reviewed
- **THEN** it is counted in the review queue until it is marked, is not counted after, and a transaction a member typed by hand is never counted

### Requirement: Categorization rules
A household SHALL define rules that match transactions by statement text (containing or exactly equal to a phrase), merchant, amount (over, under, equal to, or between, for spending or income), account, or category, and that set a category, add tags, set the merchant, hide the transaction, or mark it reviewed. Rules SHALL apply in the order the household lists them, which the household MAY change; they SHALL apply in the app as transactions are created or imported, MAY be applied to existing transactions on request, and a nightly job SHALL apply them to transactions that arrived without the app open, recording which rule categorized each transaction. A rule MAY be started from a transaction.

#### Scenario: A rule categorizes an import
- **WHEN** an editor adds a rule and then imports a statement whose rows match it
- **THEN** the matching transactions are created with the rule's category and tags, and the rule reports how many transactions it matched

#### Scenario: The nightly job categorizes synced transactions
- **WHEN** the simulated institution syncs transactions while no member has the app open and the nightly job runs
- **THEN** the synced transactions match the household's rules the next time a member opens the app, each naming the rule that categorized it

#### Scenario: A rule sets the merchant and marks the transaction reviewed
- **WHEN** an editor adds a rule matching statement text that sets a merchant and marks matches reviewed, and a matching transaction arrives
- **THEN** the transaction shows the merchant's display name, is not in the review queue, and still records the rule that acted on it

#### Scenario: Rules apply in their listed order
- **WHEN** two enabled rules match the same transaction and the household moves the second above the first
- **THEN** the category the transaction receives is the one the rule now listed first sets

### Requirement: Monthly budgets with rollover
A household SHALL budget month by month: an expected income, and a budget per category or per category group, with spending against each computed from that month's transactions excluding transfers and hidden transactions. An unspent or overspent amount SHALL roll over into the next month when the category is marked to roll over; a non-monthly category SHALL budget its target spread over its frequency; a month's budget MAY be copied from the previous month; spending in categories without a budget SHALL be shown as unbudgeted; and progress SHALL be visible per category, per group, and in total, with what is left to spend. In flex mode the household SHALL see its fixed, non-monthly, and flexible spending as three buckets, where the flexible amount is what remains of expected income after fixed costs, non-monthly targets, and planned goal contributions.

#### Scenario: Spending is tracked against a budget
- **WHEN** an editor sets a monthly budget for a category and transactions in that category are added for the month
- **THEN** the category shows spent, remaining, and percent used, updating as transactions change, and the household total sums every budgeted category

#### Scenario: A budget rolls over
- **WHEN** a category marked to roll over ends a month under budget
- **THEN** the next month's remaining amount for that category includes the unspent amount

#### Scenario: A group budget sums its categories
- **WHEN** an editor budgets a category group rather than its categories
- **THEN** the group's spent amount is the sum of spending in every category under it, its remaining amount is the group budget less that sum, and its categories show no budget of their own

#### Scenario: A flex budget is what income leaves
- **WHEN** a household in flex mode has expected income, budgets on fixed and non-monthly categories, and a planned goal contribution
- **THEN** the flexible amount shown is income less fixed budgets, less the monthly share of non-monthly targets, less the planned contribution, and flexible spending is measured against it

#### Scenario: Last month's budget is copied
- **WHEN** an editor copies the previous month's budget into an empty month
- **THEN** every category and group budget of the previous month exists for the new month with the same amounts and rollover flags, and a budget the new month already had is left alone

### Requirement: Recurring transactions and upcoming bills
Rational SHALL detect recurring transactions from a household's history by matching description and interval, SHALL let an editor mark a transaction recurring by hand, SHALL list upcoming bills with their expected date and amount and show them on a calendar with the month's total, SHALL show a bill paid once a matching transaction arrives and late once its date passes without one, and SHALL let an editor confirm, adjust, pause, or dismiss a recurrence.

#### Scenario: A recurrence is detected
- **WHEN** an account holds at least three transactions with the same normalized description at a regular interval
- **THEN** Rational lists the recurrence with its interval, next expected date, and expected amount, and marks the matching transactions as part of it

#### Scenario: A bill is paid by its matching transaction
- **WHEN** a transaction with the recurrence's description arrives within a few days of its expected date
- **THEN** the bill shows as paid with that transaction, the recurrence's next date advances by its interval, and a bill whose date passed with no such transaction shows as late

#### Scenario: A transaction is marked recurring by hand
- **WHEN** an editor marks a transaction as a monthly recurrence
- **THEN** a confirmed recurrence exists with the transaction's account, description, amount, and a next date one interval on, and it appears on the calendar and in the upcoming list

### Requirement: Goals with contributions
A household SHALL create save-up goals with a target amount, an optional date, a planned monthly contribution, and linked accounts, whose progress follows either the linked accounts' balances or recorded contributions, and pay-down goals on a liability account with a planned monthly payment whose progress follows the balance owed. Goals SHALL be ordered by priority, SHALL report whether they are on track for their date, and a pay-down goal SHALL project when it is paid off at the planned payment.

#### Scenario: A contribution advances a goal
- **WHEN** an editor records a contribution to a goal
- **THEN** the goal's progress, remaining amount, and required monthly contribution update for every member

#### Scenario: A goal's progress follows its linked account
- **WHEN** a save-up goal follows a linked savings account and a deposit lands in that account
- **THEN** the goal's saved amount rises by the deposit, and its on-track status compares that amount with what the planned contributions would have reached by now

#### Scenario: A pay-down goal projects its payoff
- **WHEN** an editor creates a pay-down goal on a loan with a planned monthly payment
- **THEN** the goal shows the balance owed, the payoff month at that payment, and the payment needed to finish by a target date when one is set

### Requirement: Net worth and reports
Rational SHALL compute net worth as assets minus liabilities from the balances of accounts not hidden from it, SHALL keep a history of net worth and of each account's balance from nightly snapshots and chart it over a chosen range, and SHALL show cash-flow and spending reports over a chosen date range — income, spending, savings, and savings rate; monthly income and spending; the flow of income into spending groups and categories; spending by group, category, merchant, tag, and account; and a category's trend over time — all computed from the household's transactions in the app with transfers, hidden transactions, and balance updates excluded, and exportable as CSV.

#### Scenario: Net worth history accrues
- **WHEN** the nightly snapshot has run on several days
- **THEN** the net-worth chart shows one point per day with assets, liabilities, and net worth as of that day

#### Scenario: A report is computed offline
- **WHEN** a member opens the spending report while the device is offline
- **THEN** the report is computed from local data and marks itself as of the last sync

#### Scenario: Cash flow over a range excludes what is not spending
- **WHEN** a member chooses a quarter and the quarter holds income, spending, a paired transfer, a hidden transaction, and a balance update
- **THEN** income and spending sum only the ordinary transactions, savings is income less spending, the savings rate is savings over income, and the transfer, the hidden transaction, and the balance update appear in none of them

#### Scenario: Income flows into spending
- **WHEN** a member opens the cash-flow flow for a month
- **THEN** every unit of income shown entering is accounted for by a spending group, a category under it, or savings, and each group's flow equals the sum of its categories

#### Scenario: An account's balance history is drawn from its snapshots
- **WHEN** the nightly snapshot has recorded several days and a member opens an account's detail page
- **THEN** the account's balance chart shows one point per recorded day, from what the snapshot recorded rather than recomputed from today's transactions

### Requirement: Alerts to a household endpoint
A household SHALL configure alerts for a transaction above an amount, a budget exceeded, an account balance below a threshold, a recurring bill due within a number of days, a goal reached, and a connection that failed to sync; alerts SHALL be decided on the server rather than on a device, since a device that is closed would never fire one; each fired alert SHALL be written as a document the household reads, counted unread in the app until a member reads it, and SHALL be delivered once as a signed webhook to the household's registered endpoint. A delivery names what changed and never carries a document's fields, so nothing a member could not read leaves on that channel.

#### Scenario: A large transaction fires an alert
- **WHEN** a scheduled run finds a transaction above the household's threshold
- **THEN** the alert appears in the household's alert history and the household's endpoint receives exactly one delivery for it, signed with the secret its registration handed over, carrying the alert's identity and none of its content

#### Scenario: A condition still true tomorrow is the same alert
- **WHEN** the same scheduled run happens again over an unchanged household
- **THEN** no second alert is written and no second delivery is made

#### Scenario: A bill due soon fires an alert
- **WHEN** the nightly job finds a confirmed recurrence due within the household's chosen number of days
- **THEN** one alert names the bill, its amount, and its date, and a later night before the date does not fire it again

#### Scenario: A goal reached fires once
- **WHEN** a goal's progress reaches its target
- **THEN** one alert names the goal, and neither a later night nor a further contribution fires a second

### Requirement: Offline-first operation
Rational SHALL work without a network: reads come from local storage, writes are queued and pushed when the network returns, concurrent edits to the same transaction resolve deterministically, local data SHALL survive a browser restart and resume replication from where it stopped, and a security reset SHALL clear the household's local data and resynchronize.

#### Scenario: Edits made offline reach the household
- **WHEN** a member edits transactions while offline and later reconnects
- **THEN** the edits push, other members receive them, and a conflicting edit by another member resolves the same way on every device

#### Scenario: The app restarts offline
- **WHEN** a member closes the browser and reopens Rational without a network
- **THEN** the household's data is shown from local storage, and when the network returns replication resumes from the persisted checkpoint rather than pulling everything again

### Requirement: Rational proves the platform
Rational SHALL run against the real platform in an automated suite covering sign-in, sharing, import, rules, receipts, alerts, and offline behavior; every platform defect the suite or the app exposes SHALL be recorded in the sample's findings log with the platform fix and the regression test that guards it; and Rational SHALL be deployed on the beta as an ordinary project — the same API URL any developer's project gets, with no privileged route of its own — so that what it proves is what a customer would get.

#### Scenario: A finding becomes a platform fix
- **WHEN** Rational's suite exposes a platform defect
- **THEN** the findings log records the symptom, the platform change that fixes it, and the regression test, and the suite passes against the fixed platform

#### Scenario: Rational runs on the beta
- **WHEN** the beta is requalified
- **THEN** Rational's smoke publishes the sample's own model and walks a household's life against the deployed release, and the release gates carry its outcome

### Requirement: Plaid-connected accounts
Rational SHALL let an editor connect a household account to a real institution through Plaid's Link flow in sandbox mode, using the platform's declared-egress capability rather than any special treatment: a function issues the Link token, exchanges the public token, and keeps the resulting access token where no application user can read it. The scheduled sync SHALL then import that connection's transactions with Plaid's cursor protocol — new entries appear once, corrected entries update the same transaction, and an entry the institution replaced (a pending charge that posted) does not survive as a duplicate. Aggregator credentials and tokens MUST never reach a browser, a replicated collection an application user can read, or a function response. When no Plaid credentials are configured, the connection option is absent and everything else — the simulated institution, CSV import, all tests that gate CI — SHALL work unchanged.

#### Scenario: An editor links an account through Plaid Sandbox
- **WHEN** an editor completes the Link flow against Plaid Sandbox and the connection's schedule runs
- **THEN** the linked institution's transactions appear under the household's account without duplicating ones already synced, and the connection shows its last sync time and outcome like any other

#### Scenario: A pending charge posts between syncs
- **WHEN** a synced pending transaction is replaced by its posted form in a later sync
- **THEN** the household sees one transaction — updated, not doubled — and any alert it fired is not fired again

#### Scenario: No application user can reach the Plaid token
- **WHEN** any signed-in member, on any device, queries every collection they can open and calls every function route they can call
- **THEN** no Plaid access token, client identifier, or secret is present in any response, replicated document, or error

#### Scenario: Rational without Plaid credentials is whole
- **WHEN** the app and its test suites run with no Plaid credentials configured
- **THEN** the Plaid connection option is not offered, the simulated institution and CSV import work unchanged, and every CI-gating suite passes without external network access

### Requirement: Dashboard
Rational SHALL open on a dashboard that summarizes the household: net worth with its recent trend, this month's spending against last month's, what the budget has left, the next bills due, goal progress, how many transactions await review, unread alerts, the most recent transactions, and the value of investments — every number the same one its own screen shows, and each leading to that screen.

#### Scenario: The dashboard summarizes the household
- **WHEN** a member opens Rational with accounts, transactions, a budget, a confirmed recurrence, a goal, and unreviewed synced transactions
- **THEN** the dashboard shows net worth equal to the accounts page's, this month's spending equal to the cash-flow page's, the budget's remaining equal to the budget page's total, the soonest bill, the goal's percent, and the review count, and following any of them opens the screen that owns it

### Requirement: Category groups and merchants
Categories SHALL belong to groups, each group being income, expense, or transfer in kind; a category SHALL carry an icon and a position within its group; a new household SHALL start with a default set of groups and categories; deleting a category SHALL move its transactions to a category the editor chooses. A merchant SHALL be the name a household gives to the statement descriptions that mean the same payee: a transaction shows its merchant where it has one and a cleaned description where it does not, merchants MAY be renamed, and two merchants MAY be merged so every transaction of one carries the other.

#### Scenario: A new household starts with a default set of categories
- **WHEN** a person's space is created and opened for the first time
- **THEN** it holds the default groups and categories, and opening it again on a second device adds none

#### Scenario: A category is deleted and its transactions move
- **WHEN** an editor deletes a category holding transactions and chooses another to receive them
- **THEN** every transaction and split of the deleted category names the chosen one, budgets of the deleted category are removed, and the category is gone from every list

#### Scenario: Merchants are renamed and merged
- **WHEN** an editor renames a merchant and merges a second merchant into it
- **THEN** every transaction of either shows the new name, the second merchant no longer exists, and reports by merchant total the two together

### Requirement: Investment holdings
An investment account SHALL hold zero or more holdings, each with a symbol, a name, a quantity, a price, an optional cost basis, and an asset class; the account's balance SHALL be its cash plus the value of its holdings; a member SHALL see holdings' value, gain against cost basis, and allocation by asset class across the household; and an editor SHALL add, update, and remove holdings and their prices.

#### Scenario: Holdings value an investment account
- **WHEN** an editor adds a holding of ten units at a price of one hundred to an investment account with cash of fifty
- **THEN** the account's balance is one thousand and fifty, net worth includes it, and updating the price to one hundred and ten raises the balance and net worth by one hundred with no transaction written

#### Scenario: Allocation is reported by asset class
- **WHEN** the household's investment accounts hold assets of two classes
- **THEN** the investments page shows each class's value and share of the total, and the gain of every holding with a cost basis

### Requirement: Household settings and data export
A household SHALL have settings a member can reach from one place: its name and base currency, its budget mode (category or flex), its members, its categories, tags, merchants, and rules, its connections and imports, its notification settings, and an export of its accounts and transactions as CSV files. Changing the budget mode SHALL change how the budget page presents the same budgets, never the budgets themselves.

#### Scenario: The household's transactions are exported
- **WHEN** a member exports the household's data
- **THEN** the transactions file holds one row per transaction with its date, account, description, merchant, category, group, tags, amount, currency, and notes, the accounts file one row per account with its class and balance, and no row of either belongs to another household

#### Scenario: The household switches budget mode
- **WHEN** an owner switches the household from category to flex mode and back
- **THEN** the budget page shows the three buckets in flex mode and the groups in category mode, and every budget amount is the same in both

### Requirement: Feature parity with Monarch is measured
Rational SHALL keep a parity matrix naming Monarch's features, each with Rational's status — present, partial, absent, or out of scope with a reason — and where in Rational it lives; a validator SHALL refuse a matrix that is malformed or whose coverage of in-scope features, counting a partial feature as half, is below ninety percent.

#### Scenario: Parity is at least ninety percent
- **WHEN** the parity validator runs over the matrix
- **THEN** it passes only when every row names a feature, a status, and a place, and the in-scope coverage is at least ninety percent, and it fails naming the coverage otherwise
