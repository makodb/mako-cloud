## Context

Money is control-plane state: it belongs to an organization, it must survive, and it must be backed up and auditable. Usage is data-plane and edge-gateway observation: only they see the requests, bytes, invocations, and stored documents. Those two live in different databases by design, and everything that has ever needed to cross that boundary in this system has needed an explicit, idempotent transfer — collection metadata, policy activation, and index builds each turned out to be broken until one was added.

Billing has the same shape, with a harder constraint: an undercount is lost revenue and an overcount is a customer charged for work they did not do. It is the first thing in this system where a dropped or duplicated record is a correctness failure with a dollar attached.

## Goals / Non-Goals

- **Goal:** every billable unit is counted once, attributable to one organization and period, and survives restart of either plane.
- **Goal:** the amount a customer is charged can be re-derived from retained evidence, and an operator can explain any line item.
- **Goal:** a plan's limits are the limits the gateway actually enforces.
- **Non-Goal:** metering at a granularity finer than the billing period needs. Per-request rows for a year are a data-retention problem, not a billing requirement.
- **Goal:** an organization can see what its use costs, and that number is explained by evidence rather than asserted.
- **Non-Goal:** collecting anything. The beta charges nobody, so there is no provider, no card data, no webhook, and no dunning in this change.
- **Non-Goal:** multi-currency, tax determination, and revenue recognition. None of them blocks showing a single price in a single currency, and all of them are cheaper to add once there is a real rate card to attach them to.

## Decisions

### The meter is a ledger, not the quota counters

Reusing the quota engine's windowed counters as the meter is tempting and wrong. They exist to answer "may this request proceed", so they are keyed by enforcement window, they are reset by design, and their retention is whatever enforcement needs. A billing meter needs the opposite properties: append-only, retained for the dispute window, attributable to a period, and reconcilable after the fact.

The two stay separate and are cross-checked. The quota engine keeps enforcing; the meter records what was served. A material divergence between them is an alert, because it means one of the two is wrong.

### Usage crosses planes through a durable outbox, not a synchronous call

The data plane records metered events locally and a worker transfers batches to the control plane over internal RPC, each batch carrying an idempotency key derived from its content and range. The control plane applies a batch at most once.

A synchronous call on the request path was rejected: it makes every document write depend on the control plane being up, and it either drops usage when that fails or fails the customer's request over an accounting concern. Neither is acceptable. The mail outbox already establishes this shape in this codebase, including its worker, its lease, and its dead-letter handling.

Batches are also how the pipeline stays affordable: an event per request is aggregated locally before transfer, so the control plane stores period totals per resource, not per request.

### Storage at rest is sampled, not integrated

Bytes stored is a level, not a count, so it has no natural event. It is sampled on a fixed schedule and billed as the average of samples over the period, with the sample series retained as the evidence for the line item. Integrating exactly on every write would put the meter on the write path for a number that changes slowly.

### Plans drive the gateway policy through a real policy source

`GatewayQuotaPolicySource` exists as a trait with exactly one implementation: the static map itself. Giving it an implementation that resolves an organization's plan, applies operator overrides, and caches with a bounded TTL is what makes a plan mean anything — and it repairs the existing defect that operator quota overrides are recorded and never enforced.

The cache must fail closed on a lookup failure by falling back to the free-tier limits, never to unlimited.

### Not charging is a property to enforce, not merely a feature left out

The beta shows a bill and a balance and collects neither. The easy version of that is to not write the payment code, which is what this change does — but "we did not build it" degrades quietly. A later change that adds collection, or a well-meaning change that makes a quota decision read the balance, turns an informational number into a restriction on a customer who was told they would not be charged.

So the posture is written as requirements rather than left as an absence: no path may contact a payment provider or ask for payment details, no surface may show a bill without saying it is not payable, and no quota, suspension, or lifecycle decision may read the balance. The last one is the load-bearing constraint, because it is the one a future change is most likely to violate by accident. The balance is deliberately not made available to the code paths that make those decisions.

### A balance is credits minus charges, and may be negative

The sign convention is stated so it cannot be inferred inconsistently by two surfaces: balance is credits minus finalized charges, so an organization that has accrued use beyond its credit reads negative. Nothing clamps it at zero, because clamping would hide exactly the number this feature exists to show.

The balance is derived from retained invoices and credit entries rather than stored as a running total that could drift from them. An operator explaining a balance walks the same entries the customer sees.

### An accrued balance is not a debt

Showing someone a number that looks like a bill, then later collecting it, is a trap. Accrual during the beta creates no payable obligation: it does not become one when the beta ends, when a plan changes, or when any timer expires. Converting accrued balances into collectible charges takes an explicit operator action with the amounts in front of them, and that action does not exist yet.

This is why the rate card is versioned from the start even though nothing is charged. A beta invoice records the rates that produced it, so if that conversation ever happens, both sides are looking at the same arithmetic.

## Risks / Trade-offs

- **Aggregating before transfer loses per-request detail.** A customer disputing a line item gets period totals and the sample series, not a request log. Keeping per-request rows for the dispute window is possible and expensive; the decision is deferred until someone actually disputes one.
- **Sampled storage is approximate by construction.** A tenant that writes and deletes between samples is undercharged. The sample interval is the tuning knob and the approximation is stated in the customer-facing description rather than hidden.
- **A shown bill invites reliance.** Someone plans around a number produced by a pipeline that has never been reconciled against money. The mitigation is that the number is explained by retained evidence and labelled as not payable, not that it is assumed correct.
- **Metering bugs are cheap now and expensive later.** Running the pipeline through a beta where nobody is charged is the only chance to find them without a customer paying for the mistake. That is an argument for building metering early, not for deferring it until collection matters.

## Phases

Each phase is separately shippable and separately useful. Only Phase 1 is a prerequisite for the others.

1. **Metering.** Events, transfer, aggregation, retention, storage sampling, and reporting from the ledger. Ships value on its own: the usage API starts telling the truth, and quota divergence becomes visible.
2. **Plans and entitlements.** Catalog, subscription, a real policy source, and enforcement of both plan limits and the operator overrides that are currently inert. Ships value on its own: differentiated limits without charging anyone.
3. **Rating, invoices, and balance.** Periods, rate cards, line items, credits, and the running balance — shown, explained, never collected.

Three phases, and the beta needs all three to show a bill. Stopping after 1 leaves honest usage reporting; stopping after 2 adds limits that differ by plan. Neither is wasted if the prices are never chosen, because a rate card of all zeroes is a valid rate card and the pipeline runs the same.

Collection — provider, payment methods, webhook, dunning, suspension for non-payment — is a separate later change, and this one is deliberately shaped so that change is additive rather than a rewrite.
