## 1. Specification

- [x] 1.1 Extend "Conditional document mutations" in `cloud/developer-data-explorer` and add the two scenarios.

## 2. Evidence that the code already meets it

- [x] 2.1 `services/mako-data-plane/src/explorer_http.rs::a_delete_without_content_is_judged_on_the_stored_document` and `::a_delete_with_content_is_judged_on_the_supplied_document`.
- [x] 2.2 `services/mako-data-plane/src/explorer_http.rs::content_stays_required_to_create_or_update`.
- [x] 2.3 `services/mako-data-plane/src/explorer_http.rs::a_stored_document_the_schema_refuses_is_reported_without_its_fields`.

## 3. Validation

- [x] 3.1 Run `openspec validate state-explorer-delete-content-rule --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`.
