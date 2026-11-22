## 1. Specification

- [x] 1.1 Extend "Public developer registration is bounded and enumeration-safe" and add the scenario "Password does not satisfy the policy".
- [x] 1.2 Extend "Developer credentials and sessions follow hosted security controls" and add the scenario "Recovery password does not satisfy the policy".

## 2. Evidence that the code already meets it

- [x] 2.1 `crates/mako-control-plane/src/developer_workflow.rs::a_weak_password_is_refused_alike_for_known_and_unknown_addresses`, and the route-level check in `services/mako-control-plane/src/graph.rs::hosted_registration_reaches_the_product_through_the_public_and_operator_routes`.
- [x] 2.2 `crates/mako-control-plane/src/developer_workflow.rs::recovery_refuses_a_weak_password_without_spending_the_token`.

## 3. Traceability and validation

- [x] 3.1 Add rows DR-04 and DR-14 to `docs/requirements-traceability.md` and renumber the rows after them.
- [x] 3.2 Run `openspec validate state-weak-password-refusals --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`.
