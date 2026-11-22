## 1. Specification

- [x] 1.1 Add the requirement "Proof of an address activates only a pending user" with its three scenarios to `identity/project-auth`.

## 2. Evidence that the code already meets it

- [x] 2.1 `crates/mako-identity/src/store.rs::magic_links_are_single_use_and_activate_pending_users` and the pending user in `::a_verification_link_does_not_reactivate_a_disabled_or_deleted_user` cover a pending user proving the address.
- [x] 2.2 `crates/mako-smoke/tests/auth_providers.rs::applications_sign_users_in_through_providers_and_magic_links` covers the provider sign-in for a pending user end to end, and `crates/mako-identity/src/store.rs::a_pending_user_reached_through_a_provider_is_activated` the store rule.
- [x] 2.3 `crates/mako-identity/src/store.rs::a_verification_link_does_not_reactivate_a_disabled_or_deleted_user` covers the verification link, and `::a_pending_user_reached_through_a_provider_is_activated` the refusal of a disabled user on the provider path.

## 3. Traceability and validation

- [x] 3.1 Add rows PA-21 to PA-23 to `docs/requirements-traceability.md`.
- [x] 3.2 Run `openspec validate state-proof-of-address-rule --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`.
