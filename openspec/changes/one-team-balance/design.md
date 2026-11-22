## Context

`handle_organization_bill` in `services/mako-control-plane/src/management_http.rs`
closes ended months, sums the closed invoices, reads the credit total, and
then branches. The live branch derives the current month so far and subtracts
it from the balance. The period branch reads one stored invoice and subtracts
closed invoices only.

## Decisions

**One balance, computed once, before the branch.** The live month's rated
total is needed by both branches, so it is derived before the branch and the
balance is computed in one place. The period branch still shows the stored
invoice's lines and total; only the balance beside them changes.

**A closed-period read fails when the live month cannot be derived.** The
alternative is to answer with a balance that leaves the live month out and
say nothing, which is the defect this change removes. The requirement
"Billing handles large retained usage histories" already says a failed page
returns an error instead of a partial bill; the same rule now covers the
balance on a closed-period read.

**The period checks stay first.** A request for a period that has not
started, for the current month, or for a month with no closed invoice is
refused before any usage is read, as today.

## Risks

- A closed-period read becomes as expensive as a live read. Both are bounded
  by the same paging rules.
