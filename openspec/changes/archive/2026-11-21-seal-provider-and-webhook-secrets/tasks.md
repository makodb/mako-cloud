## 1. Specifications

- [x] 1.1 Rewrite the requirement "External sign-in providers" in `identity/auth-providers` so that a client secret is a sealed value and not a reference, and add the scenarios "A client secret is saved and never returned" and "A provider is saved without its client secret".
- [x] 1.2 Rewrite the requirement "Webhook endpoints on collection changes" in `sync/database-webhooks` so that the signing secret is platform-generated, sealed, shown once and replaced only by rotation, and deliveries never carry document fields; add the scenario "The signing secret is rotated".

## 2. Evidence that the code already meets them

- [x] 2.1 Confirm that `services/mako-control-plane/src/auth_settings_http.rs` tests cover the two provider scenarios: sealing bound to tenant and provider name, the read view without secrets, the kept secret, and the refusal when none is installed.
- [x] 2.2 Confirm that `crates/mako-control-plane/src/webhook.rs` tests cover rotation (a new secret, the advanced version, later deliveries signed with the new secret only) and that a stored endpoint record and debug output never contain the secret.

## 3. Traceability and validation

- [x] 3.1 Add traceability rows for the three new scenarios and renumber the rows after them in `docs/requirements-traceability.md`.
- [x] 3.2 Run `openspec validate seal-provider-and-webhook-secrets --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`, and the Rust tests the new rows cite.
