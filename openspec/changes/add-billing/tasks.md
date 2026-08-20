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

## 3. Rating and invoices

- [ ] 3.1 Add billing periods per organization and their close semantics.
- [ ] 3.2 Add rate cards as versioned data, so prices change without a code change and a past invoice re-derives at the rate that applied.
- [ ] 3.3 Rate a closed period's ledger into invoice line items.
- [ ] 3.4 Add the invoice lifecycle, credits, and proration on plan change.
- [ ] 3.5 Prove a finalized invoice re-derives to the same total from retained evidence.

## 4. Payments

- [ ] 4.1 Choose the provider and record the decision, including what it is trusted with.
- [ ] 4.2 Add hosted checkout and payment-method storage that never exposes card data to this system.
- [ ] 4.3 Add the payment outbox, worker, and provider-side idempotency keys.
- [ ] 4.4 Add the signed webhook route: verify before parsing, reject replays, fail closed.
- [ ] 4.5 Advance invoice state only on a verified provider event or an explicit operator action.

## 5. Dunning and enforcement

- [ ] 5.1 Add grace periods, notices, and their schedule.
- [ ] 5.2 Suspend for non-payment through the existing project suspension lifecycle, operator-reviewed before taking effect.
- [ ] 5.3 Restore on payment, and prove no data is destroyed by a suspension.

## 6. Surfaces

- [ ] 6.1 Add management API operations for plan, usage, invoices, and payment methods, and regenerate the checked API types.
- [ ] 6.2 Add the console surfaces for the same.
- [ ] 6.3 Add operator surfaces for credits, refunds, and comped plans.
- [ ] 6.4 Audit every action that moves money, and prove no audit record carries a card, token, or provider secret.
- [ ] 6.5 Record every new scenario in the requirements traceability matrix.
