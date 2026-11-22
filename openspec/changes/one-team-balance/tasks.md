## 1. Specification

- [x] 1.1 Write the delta for `billing/invoicing-and-balance`: one balance that includes the live month, reported on every bill response, and the scenario "A closed period is read during a month with use".

## 2. Control plane

- [ ] 2.1 In `handle_organization_bill` (`services/mako-control-plane/src/management_http.rs`), compute the balance once from the credits, the closed invoices and the live month's rated total, and use it in both the live and the closed-period response. Keep the period checks (not started, current month, no closed invoice) ahead of the usage read.
- [ ] 2.2 Add a test that reads the live bill and a closed period for a team with closed invoices, credits and live-month use, and asserts the two responses carry the same `balanceMicroDollars` while the closed-period response keeps its own total. The test must fail against the current code.
- [ ] 2.3 Check the `balanceMicroDollars` description in `api/openapi/mako-cloud-v1.yaml` and correct it if it says the balance excludes the live month; regenerate the API types if the document changes.

## 3. Traceability and validation

- [ ] 3.1 Add the traceability row for the new scenario in `docs/requirements-traceability.md` and renumber the rows after it.
- [ ] 3.2 Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, the control-plane service tests, `npm run generate:api:check`, `npm run validate:traceability`, `npm run validate:docs` and `openspec validate one-team-balance --strict --no-interactive`.
