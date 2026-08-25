# Invoicing and Balance Specification

## Purpose

Turn a closed period's measured use into a bill an organization can read, and keep a running account balance that shows what that use would have cost — without collecting it.

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

### Requirement: An organization has a running balance that may be negative
Each organization SHALL have an account balance equal to its credits minus its finalized charges. A balance MUST be permitted to go negative, meaning the organization has accrued more use than credit, and MUST be derivable from the retained invoices and credit entries that produced it.

#### Scenario: Use accrues beyond any credit
- **WHEN** finalized charges exceed the credits an organization holds
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
- **WHEN** an organization views its bill or balance
- **THEN** the surface states that it is not payable and no payment method is requested

#### Scenario: A tenant's balance is deeply negative
- **WHEN** an organization's balance is negative by any amount
- **THEN** its projects continue to serve traffic unchanged, and no quota, suspension, or lifecycle decision reads the balance

#### Scenario: A component attempts collection
- **WHEN** any code path would contact a payment provider or request payment details
- **THEN** it does not exist in this system, and its absence is verified rather than assumed

### Requirement: Turning a balance into a receivable is an explicit decision
An accrued balance SHALL NOT become a payable debt by the passage of time, by the beta ending, or by any automatic process. Converting accrued balances into collectible charges MUST require an explicit, audited operator action taken with knowledge of the amounts involved.

#### Scenario: The beta ends
- **WHEN** the beta period ends
- **THEN** accrued balances remain informational until an operator explicitly converts them, and no organization is billed for beta use without that decision
