## Purpose

Turn a closed period's measured use into an invoice, collect payment for it through an external provider, and respond to non-payment without destroying a tenant's data.

## ADDED Requirements

### Requirement: An invoice is derived from retained evidence
Closing a billing period SHALL rate the period's ledger into invoice line items using the rate card version that applied during the period. A finalized invoice MUST re-derive to the same total from retained evidence, and every line item MUST identify the resource, quantity, rate, and period it came from.

#### Scenario: A finalized invoice is recomputed
- **WHEN** a finalized invoice is recomputed from the retained ledger and rate card version
- **THEN** the resulting total equals the finalized total

#### Scenario: Prices change after a period closes
- **WHEN** the rate card is revised after a period was invoiced
- **THEN** the closed invoice is unaffected and the new rates apply only to periods opened after the revision

### Requirement: Payment never exposes card data to this system
The platform SHALL collect payment through an external provider's hosted flow and MUST store only a provider token and non-sensitive descriptors. Card numbers, security codes, and full bank details MUST NOT be received, logged, or retained by any Mako Cloud service.

#### Scenario: A customer adds a payment method
- **WHEN** a customer completes the provider's hosted flow
- **THEN** the platform retains a token and non-sensitive descriptors, and no request, log, or audit record contains card data

### Requirement: Charges are idempotent and their outcome is verified
A payment attempt SHALL carry a provider-side idempotency key so a retry after an ambiguous failure cannot charge twice. Invoice state MUST advance only on a verified provider event or an explicit operator action, and MUST NOT advance because a request was sent successfully.

#### Scenario: A charge request times out and is retried
- **WHEN** a charge is retried after an ambiguous failure
- **THEN** the customer is charged at most once

#### Scenario: A provider event is unverified
- **WHEN** a webhook arrives whose signature does not verify, or whose identifier was already processed
- **THEN** it is rejected before its payload is interpreted and no invoice state changes

### Requirement: Non-payment suspends without destroying data
Non-payment SHALL progress through recorded grace periods and notices before any restriction takes effect. Suspension MUST use the existing project suspension lifecycle, MUST be reviewable by an operator before it takes effect, and MUST NOT delete tenant data. Payment MUST restore access.

#### Scenario: An invoice goes unpaid
- **WHEN** an invoice remains unpaid past its grace period
- **THEN** notices are recorded and a suspension is proposed for operator review rather than applied automatically

#### Scenario: A suspended tenant pays
- **WHEN** payment is received for a suspended organization
- **THEN** access is restored and no tenant data was destroyed by the suspension
