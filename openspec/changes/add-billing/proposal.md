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
- Add payment collection through an external provider, using the durable-outbox and worker shape the developer mail path already uses, plus a signed webhook receiver for asynchronous results.
- Add dunning and its consequences, reusing the existing project suspension lifecycle rather than inventing a second one.
- Add management, console, and operator surfaces for plans, usage, invoices, payment methods, credits, and refunds, with audit for every action that moves money.

## Capabilities

### New Capabilities

- `billing/metering`: Measurement of billable resources, their transfer from the planes that observe them, and the retained per-period ledger they aggregate into.
- `billing/plans-and-entitlements`: The plan catalog, an organization's subscription to a plan, and the effective limits that follow from it.
- `billing/invoicing-and-payments`: Billing periods, rating, invoice lifecycle, payment collection through an external provider, and dunning.

### Modified Capabilities

- `cloud/control-plane`: Report usage from the retained ledger rather than an unpopulated telemetry store, and enforce per-tenant quota limits that follow from a plan and from operator overrides.

## Impact

- New crates for the billing domain; new control-plane routes and console surfaces; a new outbound integration and a new public webhook route.
- `crates/mako-gateway` gains a real per-tenant policy source; `services/mako-data-plane` and `services/mako-edge-gateway` stop constructing static policies.
- **This is a multi-phase change and should not be implemented as one.** The phases below are separable and each is independently useful; only the first is a prerequisite for the rest.
- **Not decided here:** which payment provider, which resources are billable, what the prices are, and whether the public beta charges at all. Those are product decisions this proposal deliberately leaves open, and the rate card is designed as data so they can be made later.
