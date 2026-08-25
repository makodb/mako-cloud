## Why

Mako Cloud cannot charge anyone. There is no plan, subscription, price, rate card, invoice, credit, or payment integration anywhere in the workspace — not in Rust, not in TypeScript, not in the OpenAPI contract. A deployment can serve tenants indefinitely and has no way to know what they owe or to collect it.

Three things that look like billing foundations do not currently serve as one:

- **The quota engine is the only real measurement.** It durably counts ten request and throughput resources per tenant per window in the data plane, under compare-and-write. Those are enforcement windows, not a ledger: nothing reads them for reporting, nothing aggregates them into a period, and nothing prunes them.
- **The usage API reports nothing.** `queryProjectUsage` and `queryProjectQuotas` read an observability backend whose ingest request type has no caller anywhere in the workspace. The endpoints answer from an empty store.
- **Quota limits are identical for every tenant.** Each service constructs one hard-coded `GatewayQuotaPolicy` at startup, and the only implementation of `GatewayQuotaPolicySource` is that static map. Operator quota overrides are recorded in the control plane and never reach the gateway that would enforce them.

Nothing measures storage at rest, which is the resource a database is normally billed for.

Billing therefore cannot be added as a surface on top of existing metering. The metering has to exist first.

## What Changes

- Add a usage pipeline: metered events emitted where work happens, transferred to the control plane, and aggregated into per-period totals that survive restart and are idempotent under retry. This also gives the existing usage API something to report.
- Measure storage at rest per collection and environment, which no current resource covers.
- Add a plan catalog, per-organization subscription, and effective entitlements, and make the gateway's quota policy per-tenant so a plan's limits — and the operator overrides that already exist — are actually enforced.
- Add billing periods, rate cards, rating of usage into line items, and an invoice lifecycle with credits and proration.
- Add a running account balance per organization, permitted to go negative, derivable from the invoices and credits that produced it.
- Show the bill and the balance without collecting either. The beta charges nobody: no payment provider, no payment details, no dunning, and no lifecycle decision that reads the balance.

**Deliberately excluded**, because the beta does not charge: payment collection, stored payment methods, provider webhooks, dunning, and suspension for non-payment. Each is a later change, and none of them is a prerequisite for showing an organization what its use costs.

## Capabilities

### New Capabilities

- `billing/metering`: Measurement of billable resources, their transfer from the planes that observe them, and the retained per-period ledger they aggregate into.
- `billing/plans-and-entitlements`: The plan catalog, an organization's subscription to a plan, and the effective limits that follow from it.
- `billing/invoicing-and-balance`: Billing periods, rating, invoice lifecycle, credits, and a running balance that is shown but never collected.

### Modified Capabilities

- `cloud/control-plane`: Report usage from the retained ledger rather than an unpopulated telemetry store, and enforce per-tenant quota limits that follow from a plan and from operator overrides.

## Impact

- New crates for the billing domain; new control-plane routes and console surfaces. No new outbound integration and no new public route, because nothing is collected.
- `crates/mako-gateway` gains a real per-tenant policy source; `services/mako-data-plane` and `services/mako-edge-gateway` stop constructing static policies.
- **This is a multi-phase change and should not be implemented as one.** The phases below are separable and each is independently useful; only the first is a prerequisite for the rest.
- **Decided:** the beta does not charge. It shows the bill and a balance that may go negative.
- **Not decided here:** which resources are billable and what the prices are. The rate card is versioned data so those can be set, and changed, without a code change — and a rate card of all zeroes is a valid one, so the pipeline can run before any price is chosen.
- **Deferred to a later change:** the payment provider, stored payment methods, the webhook receiver, dunning, and whether accrued beta balances are ever converted into collectible charges.
