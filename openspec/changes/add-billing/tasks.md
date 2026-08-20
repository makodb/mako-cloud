## 1. Metering pipeline

- [ ] 1.1 Define the billable resource set, its units, and which plane observes each one.
- [ ] 1.2 Record metered events locally in the plane that serves the work, aggregated per resource and period rather than per request.
- [ ] 1.3 Add an internal-RPC operation that accepts a usage batch with a content-derived idempotency key and applies it at most once.
- [ ] 1.4 Add the transfer worker, its lease, its retry policy, and its dead-letter handling.
- [ ] 1.5 Add the retained per-period ledger in the control database, with the retention the dispute window requires.
- [ ] 1.6 Sample storage at rest on a schedule and retain the sample series as line-item evidence.
- [ ] 1.7 Report usage from the ledger instead of the unpopulated observability backend.
- [ ] 1.8 Cross-check ledger totals against quota counters and alert on material divergence.
- [ ] 1.9 Prove the pipeline counts once across restart, retry, and duplicate delivery.

## 2. Plans and entitlements

- [ ] 2.1 Add the plan catalog with per-plan limits, expressed as data.
- [ ] 2.2 Add an organization's subscription to a plan, with effective-from semantics.
- [ ] 2.3 Implement a per-tenant `GatewayQuotaPolicySource` that resolves plan limits, applies operator overrides, caches with a bounded TTL, and falls back to free-tier limits rather than unlimited.
- [ ] 2.4 Replace the static policies constructed in the data plane and edge gateway with that source.
- [ ] 2.5 Prove an operator quota override changes what the gateway enforces.

## 3. Rating, invoices, and balance

- [ ] 3.1 Add billing periods per organization and their close semantics.
- [ ] 3.2 Add rate cards as versioned data, so prices change without a code change and a past invoice re-derives at the rate that applied. A rate card of all zeroes is valid.
- [ ] 3.3 Rate a closed period's ledger into invoice line items.
- [ ] 3.4 Add the invoice lifecycle, credits, and proration on plan change.
- [ ] 3.5 Add the running account balance as credits minus finalized charges, derived from those entries rather than stored as a total.
- [ ] 3.6 Prove a finalized invoice re-derives to the same total from retained evidence, and that a balance re-derives from its entries.

## 4. Not charging, enforced

- [ ] 4.1 Prove no code path contacts a payment provider or requests payment details, as a check that fails if one is ever added.
- [ ] 4.2 Prove no quota, suspension, or lifecycle decision reads the balance, and keep the balance out of the reach of those paths rather than relying on review.
- [ ] 4.3 State on every surface that shows a bill or balance that it is not payable and no charge will be made.
- [ ] 4.4 Prove a deeply negative balance leaves a tenant's traffic, quotas, and lifecycle unchanged.
- [ ] 4.5 Ensure no automatic process converts an accrued balance into a payable debt, including when the beta ends.

## 5. Surfaces

- [ ] 5.1 Add management API operations for plan, usage, invoices, and balance, and regenerate the checked API types.
- [ ] 5.2 Add the console surfaces for the same, including the not-payable statement.
- [ ] 5.3 Add operator surfaces for credits and comped plans.
- [ ] 5.4 Audit every action that changes a balance, and prove no audit record carries a rate card secret or personal financial data.
- [ ] 5.5 Record every new scenario in the requirements traceability matrix.

## Deferred to a later change

Collection is out of scope here and is listed so it is not mistaken for an
oversight: payment provider selection, hosted checkout, stored payment
methods, the signed webhook receiver, charge idempotency, dunning, and
suspension for non-payment.
