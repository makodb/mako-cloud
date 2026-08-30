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
Rational SHALL track accounts of type checking, savings, credit, investment, loan, and cash with a currency and a current balance derived from transactions and an opening balance, and SHALL let a household add accounts manually, import a CSV statement into an account, or connect a simulated institution that provides accounts and transactions.

#### Scenario: A CSV statement is imported
- **WHEN** an editor imports a CSV statement with a mapping of its columns
- **THEN** the transactions are created under the account, rows that match existing transactions by date, amount, and normalized description are flagged as duplicates and not created, and the account balance reflects the import

#### Scenario: A simulated institution syncs
- **WHEN** an editor connects the simulated institution and the connection's schedule runs
- **THEN** new transactions from the institution appear under the connected accounts without duplicating ones already synced, and the connection shows its last sync time and outcome

### Requirement: Transactions with categories, splits, and receipts
A transaction SHALL carry a date, amount, currency, description, account, category, tags, notes, optional splits whose amounts sum to the transaction, and optional receipt files. Editors SHALL create, edit, split, categorize, and delete transactions; every member SHALL see them; receipts SHALL be readable only by the household's members.

#### Scenario: A receipt is attached
- **WHEN** an editor attaches an image to a transaction
- **THEN** the file is stored under the household, members can open it from the transaction, a person outside the household cannot read it, and deleting the transaction deletes its receipts

#### Scenario: A split does not add up
- **WHEN** an editor saves splits whose amounts differ from the transaction amount
- **THEN** the save is refused with the difference shown, and nothing is written

### Requirement: Categorization rules
A household SHALL define rules that match transactions by description, amount range, or account and set a category and tags. Rules SHALL apply in the app as transactions are created or imported, and a nightly job SHALL apply them to transactions that arrived without the app open, recording which rule categorized each transaction.

#### Scenario: A rule categorizes an import
- **WHEN** an editor adds a rule and then imports a statement whose rows match it
- **THEN** the matching transactions are created with the rule's category and tags, and the rule reports how many transactions it matched

#### Scenario: The nightly job categorizes synced transactions
- **WHEN** the simulated institution syncs transactions while no member has the app open and the nightly job runs
- **THEN** the synced transactions match the household's rules the next time a member opens the app, each naming the rule that categorized it

### Requirement: Monthly budgets with rollover
A household SHALL set a budget per category per month; spending against a budget SHALL be computed from that month's transactions; an unspent or overspent amount SHALL roll over into the next month when the category is marked to roll over; and progress SHALL be visible per category and in total.

#### Scenario: Spending is tracked against a budget
- **WHEN** an editor sets a monthly budget for a category and transactions in that category are added for the month
- **THEN** the category shows spent, remaining, and percent used, updating as transactions change, and the household total sums every budgeted category

#### Scenario: A budget rolls over
- **WHEN** a category marked to roll over ends a month under budget
- **THEN** the next month's remaining amount for that category includes the unspent amount

### Requirement: Recurring transactions and upcoming bills
Rational SHALL detect recurring transactions from a household's history by matching description and interval, SHALL list upcoming bills with their expected date and amount, and SHALL let an editor confirm, adjust, or dismiss a detected recurrence.

#### Scenario: A recurrence is detected
- **WHEN** an account holds at least three transactions with the same normalized description at a regular interval
- **THEN** Rational lists the recurrence with its interval, next expected date, and expected amount, and marks the matching transactions as part of it

### Requirement: Goals with contributions
A household SHALL create savings goals with a target amount and optional date, link a goal to an account, record contributions, and see progress and the monthly contribution needed to finish on time.

#### Scenario: A contribution advances a goal
- **WHEN** an editor records a contribution to a goal
- **THEN** the goal's progress, remaining amount, and required monthly contribution update for every member

### Requirement: Net worth and reports
Rational SHALL compute net worth as assets minus liabilities from account balances, SHALL keep a history of net worth from nightly snapshots, and SHALL show cash-flow and spending reports by category, account, and month computed from the household's transactions in the app.

#### Scenario: Net worth history accrues
- **WHEN** the nightly snapshot has run on several days
- **THEN** the net-worth chart shows one point per day with assets, liabilities, and net worth as of that day

#### Scenario: A report is computed offline
- **WHEN** a member opens the spending report while the device is offline
- **THEN** the report is computed from local data and marks itself as of the last sync

### Requirement: Alerts to a household endpoint
A household SHALL configure alerts for a transaction above an amount, a budget exceeded, and an account balance below a threshold; alerts SHALL be decided on the server rather than on a device, since a device that is closed would never fire one; each fired alert SHALL be written as a document the household reads and SHALL be delivered once as a signed webhook to the household's registered endpoint. A delivery names what changed and never carries a document's fields, so nothing a member could not read leaves on that channel.

#### Scenario: A large transaction fires an alert
- **WHEN** a scheduled run finds a transaction above the household's threshold
- **THEN** the alert appears in the household's alert history and the household's endpoint receives exactly one delivery for it, signed with the secret its registration handed over, carrying the alert's identity and none of its content

#### Scenario: A condition still true tomorrow is the same alert
- **WHEN** the same scheduled run happens again over an unchanged household
- **THEN** no second alert is written and no second delivery is made

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
