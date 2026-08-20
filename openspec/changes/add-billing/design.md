## Context

Money is control-plane state: it belongs to an organization, it must survive, and it must be backed up and auditable. Usage is data-plane and edge-gateway observation: only they see the requests, bytes, invocations, and stored documents. Those two live in different databases by design, and everything that has ever needed to cross that boundary in this system has needed an explicit, idempotent transfer — collection metadata, policy activation, and index builds each turned out to be broken until one was added.

Billing has the same shape, with a harder constraint: an undercount is lost revenue and an overcount is a customer charged for work they did not do. It is the first thing in this system where a dropped or duplicated record is a correctness failure with a dollar attached.

## Goals / Non-Goals

- **Goal:** every billable unit is counted once, attributable to one organization and period, and survives restart of either plane.
- **Goal:** the amount a customer is charged can be re-derived from retained evidence, and an operator can explain any line item.
- **Goal:** a plan's limits are the limits the gateway actually enforces.
- **Non-Goal:** metering at a granularity finer than the billing period needs. Per-request rows for a year are a data-retention problem, not a billing requirement.
- **Non-Goal:** holding card data. The provider's hosted flow holds it; this system holds a token and never sees a PAN.
- **Non-Goal:** multi-currency, tax determination, and revenue recognition in the first phases. Each is real work and none blocks charging a single price in a single currency.

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

### Payment is an outbound integration with a webhook, both fail-closed

The provider is reached from a worker thread over TLS, the same way developer mail is, with a durable outbox and provider-side idempotency keys so a retry after an ambiguous failure cannot double-charge. Results that arrive asynchronously come back through a signed webhook on a dedicated public route that verifies the signature before parsing, rejects replays, and is the only public route in the system that a third party may call.

Invoice state advances only on a verified provider event or an explicit operator action — never on a request this system merely sent successfully. That distinction is the same one the wait-list approval already draws between a committed decision and a notification attempt.

## Risks / Trade-offs

- **Aggregating before transfer loses per-request detail.** A customer disputing a line item gets period totals and the sample series, not a request log. Keeping per-request rows for the dispute window is possible and expensive; the decision is deferred until someone actually disputes one.
- **Sampled storage is approximate by construction.** A tenant that writes and deletes between samples is undercharged. The sample interval is the tuning knob and the approximation is stated in the customer-facing description rather than hidden.
- **The webhook is new public attack surface** on a deployment whose public surface is otherwise deliberately small.
- **Dunning can suspend a paying customer through a billing bug.** Suspension for non-payment is therefore operator-reviewable before it takes effect in the first phase, and automatic only once the pipeline has a track record.

## Phases

Each phase is separately shippable and separately useful. Only Phase 1 is a prerequisite for the others.

1. **Metering.** Events, transfer, aggregation, retention, storage sampling, and reporting from the ledger. Ships value on its own: the usage API starts telling the truth, and quota divergence becomes visible.
2. **Plans and entitlements.** Catalog, subscription, a real policy source, and enforcement of both plan limits and the operator overrides that are currently inert. Ships value on its own: differentiated limits without charging anyone.
3. **Rating and invoices.** Periods, rate cards, line items, invoice lifecycle, credits. Produces an invoice a human can read and an operator can explain, with no money moving.
4. **Payments.** Provider integration, hosted checkout, payment methods, webhook receiver, retries.
5. **Dunning and enforcement.** Grace periods, notices, operator-reviewed suspension, restoration on payment.

Stopping after any phase leaves a coherent system. Stopping after 3 leaves one that can invoice and be paid out of band, which is a reasonable place for a public beta to sit.
