# Billing

The beta charges nobody. It measures what every tenant uses, shows each
team the bill that use would imply, and keeps a balance that may go
negative. Nothing is collected: every bill response carries
`collectable: false` and says in words that no charge will be made, and
`npm run validate:no-collection` proves in CI that no payment-provider
integration exists and that nothing which enforces limits can read the
balance.

## Meters

Usage is observed in the plane that serves the work and shipped to the
telemetry service over its ingest contract, buffered and bounded so a
telemetry outage costs reporting rather than availability.

| Resource | Where it is observed | How it aggregates |
| --- | --- | --- |
| `storage_bytes` | Sampled by the data plane when writes mark a tenant due | Average of samples |
| `application_users` | Sampled on signup and administrative lifecycle | Average of samples |
| `replication_requests_per_minute`, `replication_bytes_per_month` | Emitted where the gateway charges the request | Sum of records |
| `edge_invocations_per_month` | Emitted by the edge gateway's audit sink, admitted invocations only | Sum of records |
| `object_storage_bytes` | Bytes held in an environment's buckets, sampled by the data plane the way `storage_bytes` is | Average of samples |
| `object_egress_bytes_per_month` | Bytes served by object downloads, emitted by the data plane per download request | Sum of records |

Flows sum; levels average. Twelve samples of the same nine stored gigabytes
are one overage, not twelve, and a mid-period delete halves the storage
charge. Production telemetry retains ninety days, so any period inside that
window re-derives.

### The ledger is cross-checked against enforcement

The usage ledger and the gateway's quota counters watch the same admitted
requests through different mechanisms: the counter is written transactionally
with admission, while the ledger travels a bounded buffer that sheds under
pressure. The data plane therefore summarizes its replication counters for
every settled minute a tenant was active in as `quota` telemetry records, and
the telemetry store compares each summary against the sum of the usage
records it kept for the same minute. A material difference -- past an
absolute floor and five percent of the larger side -- means one of billing or
enforcement is wrong, and is reported as a degraded health record for the
tenant plus an operator-visible log line. The comparison samples; it does not
gate ingest, and a cross-check that cannot run is a missed sample rather than
an error.

## Plans

`mako-billing` defines the catalog as data: a free tier that caps and a pro
tier that keeps serving and bills the excess. An included amount is what the
price covers -- exceeding it on a paid plan is normal and billed, never
blocked, because capping there would stop a customer at the moment they
started paying more. Platform rate limits are neither: every plan carries
them and no plan removes them.

Application file storage is two entitlements on each plan. Free includes
1 GiB of stored object bytes and 5 GiB of object egress per month and caps
at both; Pro includes 50 GiB stored and 250 GiB of egress and bills the
excess at $0.02 per GiB-month stored and $0.09 per GiB served. The egress
cap is an ordinary hard window on the gateway's `egress_bytes` resource. The
stored-bytes cap is a level rather than a flow, so it travels on the
installed quota policy as a hard limit on `object_storage_bytes` whose
window is nominal: the data plane compares the limit against its running
total of object bytes before admitting an upload, and nothing is charged
against a windowed counter for it.

A team records its plan (`free` by default, including teams
stored before plans existed). The only way onto another plan in the beta is
an audited operator action, which also reinstalls the limits every one of
the team's environments is held to. Operator plan exceptions
("ignore the plan for this resource") replace as a set, expire, and apply
everywhere entitlements are read -- enforcement and the bill move together.

## The bill

`GET /v1/teams/{teamId}/bill` rates the current calendar
month so far against the team's effective plan and the rate card,
whose prices were verified against supabase.com/pricing on 2026-08-25. Money
is integer micro-dollars end to end; only the display divides. Credits are
operator-granted, exactly-once per credit id, and the balance is credits
minus all charges -- every closed period plus the live month -- unclamped.
The console renders the bill on the team page with the non-payable
notice ahead of any number.

### Proration

A period that spanned a plan change is rated stretch by stretch. Each
stretch is rated under the plan that held during it: the base fee and a
flow's included allowance take the stretch's share of the period, while a
level -- a height like stored bytes -- compares its stretch average against
the full included level and prorates the resulting charge by time held. Use
under a free stretch stays uncharged, because a plan that cannot bill
overage cannot start billing it retroactively. One plan for the whole period
rates identically to the unsegmented arithmetic. Time before the
team existed is covered by the free plan's zero-priced terms, which
is what prorates a mid-month signup's base fee by construction.

### Period close

Ended months are closed on any bill read while their evidence is still
inside the ninety-day telemetry retention: the period is derived, stored as
an invoice under a conditional create -- exactly once, first derivation
wins -- and never rewritten. `?period=YYYY-MM` serves a closed month's
invoice, marked `finalized` with the instant it closed; the response's
period start reports where retained evidence actually began when it was
derived, so a month whose usage evidence had partly aged out says so instead
of pretending. A past-period response's balance counts credits against
closed periods only; the live bill adds the current month on top.

## What is deliberately absent

Payment collection, stored payment methods, provider webhooks, dunning, and
suspension for non-payment do not exist, and converting an accrued balance
into a payable debt has no mechanism.
