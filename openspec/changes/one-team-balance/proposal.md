## Why

The User Book defines a team's balance as "credits minus all charges — every
closed period plus the live month — unclamped".

`billing/invoicing-and-balance` defines it as "credits minus its finalized
charges", which leaves the live month out.

The code computes both. `GET /v1/teams/{teamId}/bill` without a period
subtracts every closed period and the live month, as the book says. The same
route with `?period=YYYY-MM` subtracts closed periods only. A team that has
used anything this month therefore sees two different numbers called
`balanceMicroDollars`, depending on which bill it opened.

## What Changes

- The requirement "A team has a running balance that may be negative" defines
  one balance: credits minus the finalized charges of every closed period and
  the charges the live month has accrued so far. Every bill response for a
  team reports that balance, whether it shows the live month or a closed
  period.
- A scenario is added for reading a closed period while the live month has
  use.
- The closed-period read of the team bill derives the live month's charges and
  reports the same balance as the live read.

## Capabilities

### New Capabilities

### Modified Capabilities

- `billing/invoicing-and-balance`: one balance that includes the live month,
  reported identically on every bill response.

## Impact

- `openspec/specs/billing/invoicing-and-balance/spec.md`
- `services/mako-control-plane/src/management_http.rs`: the `?period=` branch
  of the team bill. It gains one derivation of the live month, the same one
  the live read already performs, so a telemetry outage now fails a
  closed-period read as it fails the live read instead of answering with a
  balance that silently omits the live month.
- `docs/requirements-traceability.md`: a new row for the new scenario; the
  rows after it move down.
- No change to the wire shape: `balanceMicroDollars` keeps its name and type.
