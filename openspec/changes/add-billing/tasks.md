## 1. Metering pipeline

- [x] 1.1 Define the billable resource set, its units, and which plane observes each one.
- [x] 1.2 Record billable use in the plane that serves the work. (Implemented over the telemetry pipeline -- bounded buffer, batch delivery with source and offset -- rather than the separate control-plane outbox the proposal sketched; one transfer mechanism instead of two.)
- [x] 1.3 Deliver batches with an identifying source and offset so a redelivery is recognisable.
- [x] 1.4 Sample storage at rest and application users on a schedule, marked by the writes that change them, measured off the request path.
- [x] 1.5 Report usage from real records instead of an unpopulated store.
- [ ] 1.6 A durable per-period ledger with dispute-window retention. Telemetry retains seven days; the bill reports the window it actually covers, and a closed period cannot yet be re-derived after retention passes.
- [ ] 1.7 Cross-check ledger totals against quota counters and alert on material divergence.

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
- [ ] 3.7 Period close: a finalized invoice re-derivable after the period ends. Blocked on 1.6.
- [ ] 3.8 Proration on plan change. The live bill rates the whole window at the current plan.

## 4. Not charging, enforced

- [x] 4.1 validate:no-collection proves no payment-provider dependency, endpoint, or card-data field exists, in CI.
- [x] 4.2 The same guard proves the balance is read only by the surface that shows it, never by enforcement.
- [x] 4.3 Every bill response carries collectable: false and says in words that nothing will be charged.
- [x] 4.4 A negative balance changes no limit and no lifecycle: nothing that enforces can read it.
- [x] 4.5 No automatic process converts a balance into a debt; no conversion mechanism exists at all.

## 5. Surfaces

- [x] 5.1 Management API: the bill. Operator API: plan change, plan exceptions, credits. All in the OpenAPI contract with regenerated types.
- [ ] 5.2 Console surfaces. The API serves everything; no page renders it yet.
- [x] 5.3 Operator actions audited as distinct actions: plan change, plan exception, credit grant.
- [ ] 5.4 Traceability rows for billing scenarios, when the change's spec deltas are merged at archive.
