## 1. Metering pipeline

- [x] 1.1 Define the billable resource set, its units, and which plane observes each one.
- [x] 1.2 Record billable use in the plane that serves the work. (Implemented over the telemetry pipeline -- bounded buffer, batch delivery with source and offset -- rather than the separate control-plane outbox the proposal sketched; one transfer mechanism instead of two.)
- [x] 1.3 Deliver batches with an identifying source and offset so a redelivery is recognisable.
- [x] 1.4 Sample storage at rest and application users on a schedule, marked by the writes that change them, measured off the request path.
- [x] 1.5 Report usage from real records instead of an unpopulated store.
- [x] 1.6 Dispute-window retention: production telemetry retains ninety days, so any period inside it re-derives. (The seven-day window earlier noted here was the smoke environment's test config, not production's.) What remains open is only the immutable close snapshot, tracked at 3.7.
- [x] 1.7 Cross-check ledger totals against quota counters, alerting on material divergence: the data plane checkpoints each settled minute's replication counters as `quota` records, and the telemetry store compares them against its usage ledger, degrading tenant health and logging when they part.

## 2. Plans and entitlements

- [x] 2.1 Plan catalog as data, free and pro, with included amounts and overage-billed flags.
- [x] 2.2 An organization's subscription, defaulted to free for records stored before plans existed.
- [x] 2.3 Per-tenant quota policy resolution with a bounded cache, falling back to the deployment default and never to no limits.
- [x] 2.4 Plan-derived limits installed at environment creation and reinstalled on plan change.
- [x] 2.5 Operator plan change, audited as its own action, refusing plans the catalog does not name.
- [x] 2.6 Operator plan exceptions: replace-the-set, expiring, applied everywhere entitlements are read.

## 3. Rating, invoices, and balance

- [x] 3.1 Rate card as versioned data in integer micro-dollars; a card of zeroes is valid. Prices verified against supabase.com/pricing on 2026-08-25.
- [x] 3.2 Aggregation semantics: flows sum, levels average, so samples do not multiply a charge and a mid-period delete halves one.
- [x] 3.3 Rating into line items carrying quantity, included, overage, and amount, re-deriving deterministically.
- [x] 3.4 The live bill: GET /v1/organizations/{id}/bill, current month clamped to retained evidence, reporting the window it covers.
- [x] 3.5 Credits, granted exactly once per id, audited, never negative.
- [x] 3.6 Balance as credits minus charges, unclamped.
- [x] 3.7 Period close: ended months close on any bill read while evidence is retained, stored exactly-once under a conditional create, never rewritten, served via `?period=YYYY-MM` marked finalized; the balance counts every closed period.
- [x] 3.8 Proration on plan change: recorded plan history splits a period into stretches, each rated under its own plan's terms -- base and flow allowances by time share, level charges by time held -- reducing exactly to the unsegmented arithmetic when nothing changed.

## 4. Not charging, enforced

- [x] 4.1 validate:no-collection proves no payment-provider dependency, endpoint, or card-data field exists, in CI.
- [x] 4.2 The same guard proves the balance is read only by the surface that shows it, never by enforcement.
- [x] 4.3 Every bill response carries collectable: false and says in words that nothing will be charged.
- [x] 4.4 A negative balance changes no limit and no lifecycle: nothing that enforces can read it.
- [x] 4.5 No automatic process converts a balance into a debt; no conversion mechanism exists at all.

## 5. Surfaces

- [x] 5.1 Management API: the bill. Operator API: plan change, plan exceptions, credits. All in the OpenAPI contract with regenerated types.
- [x] 5.2 Console surfaces. The organization page renders the bill: notice first, line items, credits, balance (negative allowed). Mocked e2e asserts notice text, metered quantity, and balance.
- [x] 5.3 Operator actions audited as distinct actions: plan change, plan exception, credit grant.
- [ ] 5.4 Traceability rows for billing scenarios, when the change's spec deltas are merged at archive.
