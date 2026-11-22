## 1. Specification

- [x] 1.1 Write the delta for `billing/invoicing-and-balance`: one balance that includes the live month, reported on every bill response, and the scenario "A closed period is read during a month with use".

## 2. Control plane

- [x] 2.1 In `handle_organization_bill` (`services/mako-control-plane/src/management_http.rs`), gather the credits, the closed invoices' total and the live month's rated total into one `TeamLedger`, and take the balance of both the live and the closed-period response from it. Keep the period checks (not started, current month, no closed invoice) ahead of the live month's derivation.
- [x] 2.2 Add tests at the seam every team bill response is built through (`bill_response` with a `TeamLedger`): a live bill and a closed period for a team with closed invoices, credits and live-month use carry the same `balanceMicroDollars` while each keeps its own total, and the balance is credits minus closed and live charges, unclamped. The route itself stays covered end to end by `crates/mako-smoke/tests/telemetry_pipeline.rs` (live read, and the refusals of a closed-period read).
- [x] 2.3 Check the `balanceMicroDollars` description in `api/openapi/mako-cloud-v1.yaml` and correct it if it says the balance excludes the live month; regenerate the API types if the document changes. (Checked: the property has no description and the operation says only "the balance it implies", so the document is unchanged.)

## 3. Traceability and validation

- [x] 3.1 Add the traceability row for the new scenario in `docs/requirements-traceability.md` and renumber the rows after it.
- [x] 3.2 Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, the control-plane service tests, `npm run generate:api:check`, `npm run validate:traceability`, `npm run validate:docs` and `openspec validate one-team-balance --strict --no-interactive`.
