# Invoicing and Balance Specification

## Purpose

Turn a closed period's measured use into a bill a team can read, and keep a running account balance that shows what that use would have cost — without collecting it.

## Requirements

### Requirement: A bill is derived from retained evidence
Closing a billing period SHALL rate the period's ledger into invoice line items, each identifying the resource, quantity, included allowance, overage, and amount, with the plan and rate-card versions the period was rated under. Rating MUST be deterministic -- the same evidence and versions always produce the same total -- and a finalized invoice MUST be stored exactly once and never rewritten. A period that spanned a plan change MUST rate each plan's stretch under its own terms, prorated by time.

#### Scenario: A finalized invoice is recomputed
- **WHEN** a closed period's rating is re-derived from the same evidence and versions
- **THEN** the resulting total equals the finalized total, and the stored invoice is untouched

#### Scenario: Prices change after a period closes
- **WHEN** the rate card is revised after a period was invoiced
- **THEN** the closed invoice is unaffected, because it is stored rather than re-rated, and new rates apply only to later derivations

#### Scenario: The plan changed mid-period
- **WHEN** a period that spanned a plan change is rated
- **THEN** each plan's stretch is rated under its own terms with base fees and allowances prorated by time, and an unchanged plan rates identically to the unsegmented arithmetic

### Requirement: Project costs are allocated under the owner's shared plan
An authorized member SHALL be able to read a project's current-period usage and resource cost breakdown across all its environments. The personal account or owning team SHALL retain one shared plan and shared allowances. Each resource's rated usage charge SHALL be allocated among that owner's projects in proportion to their measured quantity for that resource. Allocation MUST be deterministic, preserve the rated resource charge through rounding, and assign no cost to a project with zero measured use of that resource. The plan's base fee, credits, and balance MUST remain at the owner level and MUST NOT be included in project cost totals.

The response SHALL identify the project, owner, shared plan, period, earliest retained evidence, observation time, allocation method, resource quantities, allocated amounts, and project total. It MUST contain only the requested project's breakdown and the informational non-payable notice. Reading the project breakdown MUST NOT finalize an invoice or change a plan, credit, or payment state. Unavailable evidence MUST produce an explicit failure, and partial retained coverage MUST be identified.

#### Scenario: Two projects share a metered resource charge
- **WHEN** two projects account for one quarter and three quarters of a resource's measured use and the owner's rated usage charge for that resource is one dollar
- **THEN** their allocated resource costs are 25 cents and 75 cents, while the owner's base fee and credits remain outside both project totals

#### Scenario: Allocated costs require rounding
- **WHEN** a resource charge cannot be divided exactly in micro-dollars according to project quantities
- **THEN** the allocation is deterministic, the project shares sum to the owner's resource charge, and zero-use projects receive no rounding remainder

#### Scenario: Project billing is read without mutation
- **WHEN** an authorized member requests a project's current bill
- **THEN** the result reports that project's retained usage and allocated costs without returning another project's breakdown or changing invoice, credit, or plan state

#### Scenario: Caller lacks developer access
- **WHEN** an anonymous caller or an application credential requests a project bill
- **THEN** the request is denied without disclosing project usage, costs, or the owner's billing information

### Requirement: A team has a running balance that may be negative
Each team SHALL have an account balance equal to its credits minus its finalized charges. A balance MUST be permitted to go negative, meaning the team has accrued more use than credit, and MUST be derivable from the retained invoices and credit entries that produced it.

#### Scenario: Use accrues beyond any credit
- **WHEN** finalized charges exceed the credits a team holds
- **THEN** the balance is reported as negative and the invoices and credits that produced it remain available

#### Scenario: A balance is explained
- **WHEN** an authorized member inspects the balance
- **THEN** each closed period behind it is retrievable as a finalized invoice and the live month derives on read

#### Scenario: An operator grants credit
- **WHEN** an operator applies a credit with a reason and an unused idempotency key
- **THEN** the credit is applied exactly once, the balance moves by that amount, and the action is audited

### Requirement: Nothing is collected and the display says so
The platform MUST NOT attempt to collect a balance, request payment details, or contact a payment provider. Any surface that shows a bill or a balance MUST state that it is not payable and that no charge will be made. A negative balance MUST NOT restrict, throttle, suspend, or degrade any tenant's service.

#### Scenario: A bill is displayed
- **WHEN** a team views its bill or balance
- **THEN** the surface states that it is not payable and no payment method is requested

#### Scenario: A tenant's balance is deeply negative
- **WHEN** a team's balance is negative by any amount
- **THEN** its projects continue to serve traffic unchanged, and no quota, suspension, or lifecycle decision reads the balance

#### Scenario: A component attempts collection
- **WHEN** any code path would contact a payment provider or request payment details
- **THEN** it does not exist in this system, and its absence is verified rather than assumed

### Requirement: Turning a balance into a receivable is an explicit decision
An accrued balance SHALL NOT become a payable debt by the passage of time, by the beta ending, or by any automatic process. Converting accrued balances into collectible charges MUST require an explicit, audited operator action taken with knowledge of the amounts involved.

#### Scenario: The beta ends
- **WHEN** the beta period ends
- **THEN** accrued balances remain informational until an operator explicitly converts them, and no team is billed for beta use without that decision

### Requirement: Billing handles large retained usage histories
Overall and project billing SHALL aggregate all retained usage in the requested billing period without a fixed record-count ceiling. Reads SHALL use a fixed upper time bound and process pages with memory bounded by the page size and resource counts per plan segment. Flows SHALL sum and levels SHALL average per environment and plan segment before combining environments. Retention gaps SHALL remain explicit. A failed page MUST fail the derivation rather than return or finalize a partial bill.

#### Scenario: Usage exceeds sixteen thousand records
- **WHEN** an environment has more than 16,000 retained usage records in the billing period
- **THEN** overall and project billing succeed and include every in-period record with the existing sum, average, and plan-segment rules

#### Scenario: A later usage page fails
- **WHEN** a usage page fails after earlier pages were aggregated
- **THEN** the derivation fails without returning or finalizing partial totals

#### Scenario: A billing period has expired from retention
- **WHEN** the entire billing period predates retained telemetry
- **THEN** billing reports the empty evidence window without querying an invalid telemetry range or inventing usage
