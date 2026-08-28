# Requirements-to-test traceability

This matrix is the release-facing index from every scenario in the eleven capability specs named in `scripts/validate-requirements-traceability.js` to its primary evidence. The remaining capability specs under `openspec/specs/` are not indexed here. `Automated` means the cited test executes in the repository test suites. Supporting tests may exercise more behavior than the scenario named here.

Evidence marked `(mocked backend)` runs against intercepted HTTP responses rather than a running service, so it proves client behavior and not that the server implements the scenario. Evidence marked `(end-to-end)` drives the real service binaries. A scenario whose only evidence is mocked has no automated proof that the server side works.

Run `npm run validate:traceability` whenever a scenario or row changes. The validator requires a one-to-one match with the scenario headings in those specs and verifies every cited file exists. Row IDs are positional — `CP-07` is the seventh scenario in the control-plane spec — so inserting a scenario mid-spec renumbers every row after it.

## Cloud / control plane

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| CP-01 | Owner invites a developer | `crates/mako-control-plane/src/organization.rs::owner_invites_developer_and_viewer_mutation_is_denied_and_audited` | Automated |
| CP-02 | Viewer attempts mutation | `crates/mako-control-plane/src/organization.rs::owner_invites_developer_and_viewer_mutation_is_denied_and_audited` | Automated |
| CP-03 | A developer creates an individual project | `crates/mako-smoke/tests/personal_space.rs::a_developer_creates_individual_projects_in_a_personal_space` (end-to-end) | Automated |
| CP-04 | A personal space refuses members and deletion | `crates/mako-control-plane/src/organization.rs::a_personal_space_is_created_once_and_refuses_members_and_deletion` and `crates/mako-smoke/tests/personal_space.rs::a_developer_creates_individual_projects_in_a_personal_space` (end-to-end) | Automated |
| CP-05 | Project provisioning succeeds | `crates/mako-provisioning/src/bootstrap.rs::activates_only_after_every_resource_is_ready_and_hides_suspended_endpoints` | Automated |
| CP-06 | Provisioning fails | `crates/mako-provisioning/src/workflow.rs::retryable_failure_is_compensated_and_can_resume_idempotently` | Automated |
| CP-07 | Created project converges without caller action | `crates/mako-smoke/tests/sample_app.rs::a_developer_builds_a_sample_app_and_an_application_user_replicates_through_it` (end-to-end) | Automated |
| CP-08 | Developer signs in during a tenant database outage | `crates/mako-smoke/tests/control_outage.rs::control_operations_continue_while_the_data_plane_is_unavailable` (end-to-end) | Automated |
| CP-09 | Operator diagnoses a data-plane outage | `services/mako-control-plane/src/graph.rs::operator_authenticates_and_inspects_control_state_while_the_data_plane_is_unavailable` | Automated |
| CP-10 | Control authority is unavailable | `crates/mako-smoke/tests/control_outage.rs::control_plane_refuses_to_serve_when_its_control_authority_is_unavailable` (end-to-end) and `scripts/validate-public-beta-caddy.js` | Automated |
| CP-11 | Resource is changed through API | `crates/mako-control-plane/src/management_access.rs::console_and_automation_paths_share_validation_and_rbac_outcomes` | Automated |
| CP-12 | Compatible schema version is published | `crates/mako-control-plane/src/collection.rs::compatible_publication_succeeds_and_incompatible_change_requires_migration` | Automated |
| CP-13 | Incompatible schema is submitted directly | `crates/mako-control-plane/src/collection.rs::compatible_publication_succeeds_and_incompatible_change_requires_migration` | Automated |
| CP-14 | Created collection accepts document traffic | `crates/mako-documents/src/collection.rs::installed_metadata_becomes_resolvable_and_replays_as_unchanged` and `crates/mako-smoke/tests/happy_path.rs::application_happy_path_succeeds_against_the_real_services` (end-to-end) | Automated |
| CP-15 | Data plane cannot record the collection | `crates/mako-control-plane/src/collection.rs::created_collection_stays_pending_until_activated_and_activation_is_idempotent` | Automated |
| CP-16 | Created index answers document queries | `crates/mako-smoke/tests/database_service.rs::an_application_user_authenticates_reads_writes_and_queries_their_own_data` (end-to-end) | Automated |
| CP-17 | Administrator tests a policy | `crates/mako-control-plane/src/policy.rs::draft_validation_testing_activation_and_rollback_report_epochs` | Automated |
| CP-18 | Activated policy governs document traffic | `crates/mako-smoke/tests/sample_app.rs::a_developer_builds_a_sample_app_and_an_application_user_replicates_through_it` (end-to-end) | Automated |
| CP-19 | Data plane cannot record the policy | `crates/mako-smoke/tests/sample_app.rs::policy_activation_fails_when_the_data_plane_cannot_record_it` (end-to-end) | Automated |
| CP-20 | Service credential is created | `crates/mako-control-plane/src/credentials.rs::credentials_keys_and_function_secrets_rotate_without_persistent_plaintext` | Automated |
| CP-21 | New environment obtains its first signing key | `crates/mako-smoke/tests/sample_app.rs::a_developer_builds_a_sample_app_and_an_application_user_replicates_through_it` (end-to-end) | Automated |
| CP-22 | Support operator inspects a user | `crates/mako-control-plane/src/application_user.rs::organization_roles_become_identity_permissions_and_core_service_audits_denials` | Automated |
| CP-23 | Developer promotes a function version | `crates/mako-control-plane/src/function.rs::deploy_promote_rollback_test_logs_and_delete_follow_safe_lifecycle` | Automated |
| CP-24 | Project reaches a hard quota | `crates/mako-gateway/src/quota.rs::hard_limits_rate_limits_and_retry_advice_are_stable` | Automated |
| CP-25 | A member inspects usage for a period | `crates/mako-smoke/tests/telemetry_pipeline.rs::observed_events_and_usage_reach_the_management_api` (end-to-end) | Automated |
| CP-26 | A function's printed line is retained | `crates/mako-smoke/tests/edge_function.rs::deployed_function_is_served_through_the_edge_gateway` (end-to-end) | Automated |
| CP-27 | A sensitive-looking value is masked before storage | `services/mako-telemetry-query/src/main.rs::a_project_log_is_scrubbed_before_it_is_stored` and `crates/mako-audit/src/redaction.rs::log_scrubbing_masks_emails_and_credentials_but_not_ordinary_text` | Automated |
| CP-28 | Policy version is activated | `crates/mako-control-plane/src/policy.rs::draft_validation_testing_activation_and_rollback_report_epochs` | Automated |
| CP-29 | Operator opens support access | `crates/mako-control-plane/src/operator.rs::operator_permissions_scope_repairs_abuse_and_expiring_support` | Automated |
| CP-30 | Project deletion is requested | `crates/mako-control-plane/src/deletion.rs::deletion_revokes_restores_and_destroys_in_durable_order` | Automated |
| CP-31 | Grace period expires | `crates/mako-control-plane/src/deletion.rs::deletion_revokes_restores_and_destroys_in_durable_order` | Automated |

## Billing / metering

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| BM-01 | A batch is delivered twice | `services/mako-telemetry-query/src/main.rs::durable_source_offsets_are_idempotent_and_conflict_on_changed_replay` | Automated |
| BM-02 | The ledger store is unavailable | `services/mako-data-plane/src/telemetry.rs::stored_size_and_user_count_are_marked_and_sampled_independently` (records against an unreachable endpoint) and `crates/mako-smoke/tests/telemetry_pipeline.rs::observed_events_and_usage_reach_the_management_api` (end-to-end) | Automated |
| BM-03 | Use is lost under pressure | `services/mako-telemetry-query/src/main.rs::a_checkpoint_is_compared_against_the_ledger_and_material_divergence_alerts` | Automated |
| BM-04 | Storage is billed for a period | `crates/mako-billing/src/rating.rs::samples_of_a_level_average_instead_of_multiplying_the_charge` | Automated |
| BM-05 | An idle tenant is not resampled | `services/mako-data-plane/src/telemetry.rs::a_marked_tenant_is_sampled_once_and_not_again_until_the_interval_passes` | Automated |
| BM-06 | A developer reads their usage | `crates/mako-smoke/tests/telemetry_pipeline.rs::observed_events_and_usage_reach_the_management_api` (end-to-end) | Automated |
| BM-07 | The two disagree | `services/mako-telemetry-query/src/main.rs::a_checkpoint_is_compared_against_the_ledger_and_material_divergence_alerts` and `services/mako-telemetry-query/src/main.rs::a_replayed_checkpoint_batch_does_not_alert_twice` | Automated |

## Billing / plans and entitlements

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| BP-01 | A plan's terms change | `crates/mako-control-plane/src/organization.rs::an_invoice_closes_exactly_once_and_reads_back_unchanged` | Automated |
| BP-02 | A team changes plan mid-period | `crates/mako-control-plane/src/model.rs::plan_changes_leave_stretches_a_billing_period_can_be_split_by` and `crates/mako-billing/src/rating.rs::an_upgrade_mid_month_prorates_the_base_and_the_flow_allowances` | Automated |
| BP-03 | Two tenants on different plans | `crates/mako-gateway/src/quota.rs::an_installed_policy_applies_to_one_tenant_and_the_default_to_the_rest` | Automated |
| BP-04 | An operator raises a tenant's limit | `crates/mako-control-plane/src/operator.rs::plan_exceptions_replace_as_a_set_and_expired_ones_are_not_in_force` | Automated |
| BP-05 | Plan resolution fails | `crates/mako-gateway/src/quota.rs::an_installed_policy_applies_to_one_tenant_and_the_default_to_the_rest` | Automated |

## Billing / invoicing and balance

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| BI-01 | A finalized invoice is recomputed | `crates/mako-billing/src/rating.rs::one_segment_covering_the_period_rates_exactly_like_the_unsegmented_period` and `crates/mako-control-plane/src/organization.rs::an_invoice_closes_exactly_once_and_reads_back_unchanged` | Automated |
| BI-02 | Prices change after a period closes | `crates/mako-control-plane/src/organization.rs::an_invoice_closes_exactly_once_and_reads_back_unchanged` | Automated |
| BI-03 | The plan changed mid-period | `crates/mako-billing/src/rating.rs::an_upgrade_mid_month_prorates_the_base_and_the_flow_allowances` and `crates/mako-billing/src/rating.rs::a_level_prorates_its_charge_by_the_time_it_was_held` | Automated |
| BI-04 | Use accrues beyond any credit | `apps/console/test-e2e/management-workflows.spec.ts` (mocked backend) -- asserts the negative balance is shown and marked, never clamped | Automated |
| BI-05 | A balance is explained | `crates/mako-smoke/tests/telemetry_pipeline.rs::observed_events_and_usage_reach_the_management_api` (end-to-end; live bill and period queries) | Automated |
| BI-06 | An operator grants credit | `crates/mako-control-plane/src/operator.rs::a_credit_is_granted_exactly_once_and_totals_are_the_sum_of_grants` | Automated |
| BI-07 | A bill is displayed | `crates/mako-smoke/tests/telemetry_pipeline.rs::observed_events_and_usage_reach_the_management_api` (end-to-end) and `apps/console/test-e2e/management-workflows.spec.ts` (mocked backend) | Automated |
| BI-08 | A tenant's balance is deeply negative | `scripts/validate-no-collection.js` (proves no enforcement path reads the balance) | Automated |
| BI-09 | A component attempts collection | `scripts/validate-no-collection.js` | Automated |
| BI-10 | The beta ends | `scripts/validate-no-collection.js` (no conversion mechanism exists to find) | Automated |

## Developer registration and wait list

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| DR-01 | New visitor registers | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-02 | Existing email is submitted again | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-03 | Registration is abusive or over limit | `crates/mako-control-plane/src/developer_workflow.rs::registration_is_disabled_without_mail_and_password_work_is_bounded` | Automated |
| DR-04 | Applicant verifies within the token lifetime | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-05 | Verification token is invalid, expired, or replayed | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-06 | Applicant requests another verification message | `crates/mako-control-plane/src/developer_registration.rs::cleanup_is_bounded_and_removes_only_expired_records` | Automated |
| DR-07 | Wait-listed applicant signs in | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-08 | Wait-listed session calls a product route | `crates/mako-control-plane/src/developer_identity.rs::persistent_authority_rejects_stale_active_claims_and_epochs` and `services/mako-control-plane/src/graph.rs::hosted_registration_reaches_the_product_through_the_public_and_operator_routes` | Automated |
| DR-09 | Applicant inspects wait-list status | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-10 | Active developer signs in after approval | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-11 | Stale token claims active status | `crates/mako-control-plane/src/developer_identity.rs::persistent_authority_rejects_stale_active_claims_and_epochs` | Automated |
| DR-12 | Password recovery completes | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-13 | Operator reviews pending applicants | `services/mako-control-plane/src/graph.rs::hosted_registration_reaches_the_product_through_the_public_and_operator_routes` and `apps/console/test-e2e/developer-registration-waitlist.spec.ts` (mocked backend) | Automated |
| DR-14 | Operator approves an applicant | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` and `apps/console/test-e2e/developer-registration-waitlist.spec.ts` (mocked backend) | Automated |
| DR-15 | Operator rejects an applicant | `services/mako-control-plane/src/graph.rs::hosted_registration_reaches_the_product_through_the_public_and_operator_routes` and `apps/console/test-e2e/developer-registration-waitlist.spec.ts` (mocked backend) | Automated |
| DR-16 | Two operators decide concurrently | `crates/mako-control-plane/src/developer_registration.rs::concurrent_decisions_have_one_winner_and_stable_indexes` | Automated |
| DR-17 | Unauthorized actor calls a review route | `services/mako-control-plane/src/graph.rs::local_graph_composes_all_dependencies_and_probes_data_plane` | Automated |
| DR-18 | Approved identity has an old wait-list session | `crates/mako-control-plane/src/developer_workflow.rs::hosted_registration_waitlist_approval_and_recovery_are_isolated` | Automated |
| DR-19 | Active identity is disabled | `crates/mako-control-plane/src/developer_registration.rs::account_lifecycle_is_explicit_and_advances_authority` | Automated |
| DR-20 | Newly active developer enters the product | `services/mako-control-plane/src/graph.rs::hosted_registration_reaches_the_product_through_the_public_and_operator_routes` and `apps/console/test-e2e/developer-registration-waitlist.spec.ts` (mocked backend) | Automated |
| DR-21 | Hosted mail is not configured | `crates/mako-config/src/lib.rs::developer_registration_is_deny_by_default_and_requires_protected_mail` | Automated |
| DR-22 | Approval notice delivery fails temporarily | `crates/mako-control-plane/src/developer_workflow.rs::durable_worker_claims_and_delivers_each_outbox_record_once` | Automated |
| DR-23 | Control plane restarts with pending applicants | `crates/mako-control-plane/src/developer_registration.rs::registration_is_atomic_unique_and_recovers_from_restart` | Automated |
| DR-24 | Existing deployment is migrated | `crates/mako-control-plane/src/developer_registration.rs::legacy_migration_is_resumable_and_refuses_email_collisions` | Automated |
| DR-25 | Tenant RocksDB is unavailable | `crates/mako-smoke/tests/control_outage.rs::control_operations_continue_while_the_data_plane_is_unavailable` (end-to-end) | Automated |
| DR-26 | Visitor opens the hosted sign-in page | `apps/console/test-e2e/developer-registration-waitlist.spec.ts` (mocked backend) | Automated |
| DR-27 | Operator approves from the console | `services/mako-control-plane/src/graph.rs::hosted_registration_reaches_the_product_through_the_public_and_operator_routes` and `apps/console/test-e2e/developer-registration-waitlist.spec.ts` (mocked backend) | Automated |
| DR-28 | Public caller probes a private identity route | `services/mako-control-plane/src/graph.rs::local_graph_composes_all_dependencies_and_probes_data_plane` and `scripts/validate-public-beta-caddy.js` | Automated |
| DR-29 | Operator monitors wait-list health | `services/mako-control-plane/src/developer_metrics.rs::rendered_metrics_have_only_bounded_aggregate_labels` and `scripts/validate-observability-assets.js` | Automated |
| DR-30 | Accepted preview remains open over time | `scripts/test/public-beta-preview-admission-guard.test.js` | Automated |
| DR-31 | Operator manually pauses public preview | `scripts/test/public-beta-preview-admission-guard.test.js` | Automated |
| DR-32 | Persistent preview safety binding drifts | `scripts/test/public-beta-preview-admission-guard.test.js` | Automated |

## Functions / edge runtime

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| ER-01 | TypeScript function handles a request | `packages/cli/test/compatibility.integration.mjs::the pinned local runtime supports the qualified edge-function API surface` | Automated |
| ER-02 | Unsupported runtime feature is used | `crates/mako-control-plane/src/function_bundle.rs::unresolved_imports_and_oversized_sources_return_sanitized_diagnostics` | Automated |
| ER-03 | New version passes validation | `crates/mako-control-plane/src/function_bundle.rs::source_archives_are_deterministic_across_input_order` | Automated |
| ER-04 | Rollback is requested | `crates/mako-control-plane/src/function.rs::deploy_promote_rollback_test_logs_and_delete_follow_safe_lifecycle` | Automated |
| ER-05 | Protected function is called without a token | `crates/mako-edge-gateway/src/lib.rs::jwt_and_request_limit_fail_before_user_code` | Automated |
| ER-06 | Public webhook function is called | `crates/mako-edge-gateway/src/lib.rs::explicit_public_route_keeps_admission_and_audit_context` | Automated |
| ER-07 | Function reads application data | `packages/edge-sdk/test/client.test.mjs::auth and document clients automatically propagate the verified caller` | Automated |
| ER-08 | Function performs privileged maintenance | `packages/edge-sdk/test/client.test.mjs::service access is explicit, uses a separate route, and carries mandatory audit context` | Automated |
| ER-09 | Secret is created | `crates/mako-control-plane/src/credentials.rs::credentials_keys_and_function_secrets_rotate_without_persistent_plaintext` | Automated |
| ER-10 | Function logs a known secret | `packages/cli/test/serve.test.mjs::runtime output redaction covers every exact active secret` | Automated |
| ER-11 | Function exceeds CPU limit | `crates/mako-edge-runtime/src/lib.rs::every_resource_limit_returns_a_stable_correlation_aware_code` | Automated |
| ER-12 | Function attempts undeclared secret access | `crates/mako-edge-runtime/src/lib.rs::only_exact_declared_secret_versions_reach_the_worker_factory` | Automated |
| ER-13 | Nearest region is unavailable | `crates/mako-edge-gateway/src/lib.rs::nearest_healthy_selected_region_fails_over_without_using_unauthorized_health` | Automated |
| ER-14 | Invocation fails | `crates/mako-edge-runtime/src/lib.rs::crash_recovery_and_clean_recycling_create_new_generations` | Automated |
| ER-15 | Developer serves a function locally | `packages/cli/test/serve.test.mjs::local serve maps only declared environment and secret values into the pinned runtime` | Automated |
| ER-16 | Function attempts to run past its wall limit | `packages/cli/test/adversarial.integration.mjs::the pinned runtime contains adversarial failures within one project worker` | Automated |

## Identity / project auth

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| PA-01 | Same email is used in two projects | `crates/mako-identity/src/store.rs::normalized_email_is_unique_per_tenant_and_independent_across_projects` | Automated |
| PA-02 | User signs up successfully | `crates/mako-identity/src/signup.rs::signup_is_enumeration_safe_and_verification_is_single_use` | Automated |
| PA-03 | Password recovery is requested | `crates/mako-identity/src/recovery.rs::recovery_is_enumeration_safe_single_use_and_revokes_sessions` | Automated |
| PA-04 | Valid access token is presented | `crates/mako-identity/src/access_token.rs::issued_token_contains_every_required_scoped_claim_and_valid_signature` | Automated |
| PA-05 | Token targets another project | `crates/mako-gateway/src/token_verifier.rs::verifies_every_boundary_and_rejects_cross_project_tokens` | Automated |
| PA-06 | Session refresh succeeds | `crates/mako-identity/src/refresh.rs::rotates_hashes_allows_bounded_concurrency_and_revokes_on_replay` | Automated |
| PA-07 | Rotated token is replayed | `crates/mako-identity/src/refresh.rs::rotates_hashes_allows_bounded_concurrency_and_revokes_on_replay` | Automated |
| PA-08 | Administrator disables a user | `crates/mako-identity/src/lifecycle.rs::lifecycle_changes_revoke_sessions_and_publish_one_ordered_stream` | Automated |
| PA-09 | User edits profile metadata | `crates/mako-identity/src/records.rs::records_retain_project_environment_scope_and_separate_metadata` | Automated |
| PA-10 | Public credential is used without a user session | `crates/mako-gateway/src/replication.rs::authorizes_public_user_context_and_returns_retryable_quota_errors` | Automated |
| PA-11 | Service credential is rotated | `crates/mako-identity/src/project_credentials.rs::credentials_are_one_time_scoped_rotatable_and_public_keys_never_bypass` | Automated |
| PA-12 | Developer without user-admin permission attempts deletion | `crates/mako-identity/src/admin.rs::admin_surface_checks_permissions_audits_and_exposes_no_credentials` | Automated |
| PA-13 | Repeated failed sign-ins occur | `crates/mako-identity/src/rate_limit.rs::limiter_reserves_admission_fails_closed_and_reports_retry_window` | Automated |
| PA-14 | Authentication fails | `crates/mako-identity/src/signin.rs::wrong_absent_and_unverified_accounts_share_one_failure_response` | Automated |

## Security / document policies

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| DP-01 | Collection has no active policies | `crates/mako-policy/src/evaluator.rs::absent_policy_and_scope_mismatch_fail_closed` | Automated |
| DP-02 | Update policy checks ownership | `crates/mako-policy/src/document_hook.rs::write_hook_uses_proposed_create_and_both_update_states_without_partial_writes` | Automated |
| DP-03 | Policy references untrusted metadata | `crates/mako-policy/src/compiler.rs::unknown_fields_calls_and_cost_excess_return_source_diagnostics` | Automated |
| DP-04 | Allow and deny both match | `crates/mako-policy/src/evaluator.rs::matching_allow_is_required_and_matching_deny_overrides` | Automated |
| DP-05 | Ownership is changed by update | `crates/mako-policy/src/document_hook.rs::write_hook_uses_proposed_create_and_both_update_states_without_partial_writes` | Automated |
| DP-06 | Document changes between check and commit | `crates/mako-documents/tests/document_invariants.rs::concurrent_revision_change_and_acknowledgement_invariants_survive_restart` | Automated |
| DP-07 | Conflict exists on unreadable document | `crates/mako-sync/src/push.rs::bounded_push_commits_rows_and_returns_only_readable_conflicts` | Automated |
| DP-08 | Edge function uses caller context | `packages/edge-sdk/test/client.test.mjs::auth and document clients automatically propagate the verified caller` | Automated |
| DP-09 | Team field changes | `crates/mako-policy/src/visibility.rs::classifies_all_visibility_transitions_without_protected_new_content` | Automated |
| DP-10 | Policy becomes more restrictive | `crates/mako-policy/src/authorization_epoch.rs::trusted_claim_changes_advance_only_the_subject_and_publish_in_order` | Automated |
| DP-11 | Invalid policy is submitted | `crates/mako-policy/src/store.rs::validation_activation_failure_and_rollback_preserve_atomic_history` | Automated |
| DP-12 | New policy set activates | `crates/mako-policy/src/store.rs::validation_activation_failure_and_rollback_preserve_atomic_history` | Automated |
| DP-13 | Edge function uses default data client | `packages/edge-sdk/test/client.test.mjs::the runtime request helper consumes only trusted caller and correlation headers` | Automated |
| DP-14 | Service credential performs maintenance | `crates/mako-policy/src/privileged.rs::service_bypass_is_scoped_and_requires_a_successful_audit_append` | Automated |
| DP-15 | Policy evaluator is unavailable | `crates/mako-policy/src/evaluator.rs::timeout_unavailability_invalid_context_and_internal_failure_deny` | Automated |
| DP-16 | Write is denied | `crates/mako-policy/src/document_hook.rs::write_hook_uses_proposed_create_and_both_update_states_without_partial_writes` | Automated |

## Storage / document engine

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| DE-01 | Unsupported adapter is rejected | `crates/mako-storage/src/readiness.rs::memory_adapter_cannot_make_the_data_plane_ready` | Automated |
| DE-02 | Production writes use synchronous RocksDB durability | `crates/mako-storage/src/rocks.rs::sync_acknowledged_write_survives_reopen` | Automated |
| DE-03 | Production storage is not ready | `crates/mako-storage/tests/production_startup.rs::missing_unprovisioned_wrong_owner_and_wrong_format_fail_closed` | Automated |
| DE-04 | Production restarts on its owned volume | `crates/mako-documents/tests/document_invariants.rs::concurrent_revision_change_and_acknowledgement_invariants_survive_restart` | Automated |
| DE-05 | Production storage cannot open | `crates/mako-storage/tests/production_startup.rs::lock_contention_and_corrupt_database_fail_closed` | Automated |
| DE-06 | Cross-project identifier is supplied | `crates/mako-documents/src/engine.rs::rejects_cross_project_and_cross_environment_requests` | Automated |
| DE-07 | Valid document is written | `crates/mako-documents/src/mutation.rs::commits_document_change_receipt_and_position_atomically` | Automated |
| DE-08 | Invalid document is rejected | `crates/mako-documents/src/validation.rs::reports_only_schema_paths_not_protected_values` | Automated |
| DE-09 | Concurrent conditional updates | `crates/mako-documents/src/mutation.rs::conditional_create_update_and_delete_return_current_revision_conflicts` | Automated |
| DE-10 | Failed commit leaves no partial state | `crates/mako-storage/tests/fault_injection.rs::every_precommit_io_failure_leaves_no_candidate_state` | Automated |
| DE-11 | Equal-time writes remain deterministic | `crates/mako-storage/tests/key_codec_properties.rs::change_key_order_matches_position_and_document_tuple` | Automated |
| DE-12 | Iteration uses a fixed high water | `crates/mako-documents/src/change_log.rs::captured_high_water_excludes_later_writes_and_pages_in_commit_order` | Automated |
| DE-13 | Document is deleted | `crates/mako-documents/src/mutation.rs::conditional_create_update_and_delete_return_current_revision_conflicts` | Automated |
| DE-14 | Tombstone is compacted safely | `crates/mako-documents/src/retention.rs::expires_checkpoints_before_compacting_history_and_tombstones` | Automated |
| DE-15 | Indexed query is executed | `crates/mako-documents/src/query.rs::primary_key_and_compound_range_queries_page_deterministically` | Automated |
| DE-16 | Query lacks an eligible index | `crates/mako-documents/src/query.rs::unbounded_unsupported_and_cursor_mismatch_queries_fail_closed` | Automated |
| DE-17 | Unique index finds duplicate data | `crates/mako-documents/src/index_build.rs::duplicate_unique_build_fails_with_non_sensitive_diagnostics` | Automated |
| DE-18 | Writes occur during index build | `crates/mako-documents/src/index_build.rs::change_log_catch_up_precedes_atomic_activation` | Automated |
| DE-19 | Process restarts after acknowledgement | `crates/mako-documents/tests/document_invariants.rs::concurrent_revision_change_and_acknowledgement_invariants_survive_restart` | Automated |
| DE-20 | Concurrent write occurs during a page read | `crates/mako-documents/src/read.rs::multi_key_reads_remain_on_one_snapshot_during_concurrent_writes` | Automated |

## Sync / RxDB replication

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| RR-01 | Application starts replication | `packages/rxdb-client/test/config.test.mjs::normalizes a bounded browser or Node configuration` | Automated |
| RR-02 | Initial pull | `crates/mako-sync/src/pull.rs::null_checkpoint_pull_captures_high_water_and_returns_commit_order` | Automated |
| RR-03 | Changes are not visible to the caller | `crates/mako-sync/tests/multi_client.rs::clients_handle_hidden_visibility_delete_reconnect_and_expired_checkpoint` | Automated |
| RR-04 | Assumed master matches | `crates/mako-sync/src/push.rs::bounded_push_commits_rows_and_returns_only_readable_conflicts` | Automated |
| RR-05 | Assumed master is stale | `crates/mako-sync/tests/multi_client.rs::concurrent_offline_clients_conflict_and_winner_retry_is_idempotent` | Automated |
| RR-06 | Push is not authorized | `packages/rxdb-client/test/push.test.mjs::surfaces denied rows as non-retryable policy errors` | Automated |
| RR-07 | Push response is lost | `crates/mako-sync/tests/multi_client.rs::concurrent_offline_clients_conflict_and_winner_retry_is_idempotent` | Automated |
| RR-08 | Visible document changes | `crates/mako-sync/src/live.rs::live_delivery_filters_frames_heartbeats_and_overflow_resyncs` | Automated |
| RR-09 | Client reconnects | `crates/mako-sync/tests/multi_client.rs::clients_handle_hidden_visibility_delete_reconnect_and_expired_checkpoint` | Automated |
| RR-10 | Remote document is deleted | `crates/mako-sync/src/pull.rs::pull_delivers_stored_and_visibility_transition_tombstones` | Automated |
| RR-11 | Document update revokes its own visibility | `crates/mako-policy/src/visibility.rs::classifies_all_visibility_transitions_without_protected_new_content` | Automated |
| RR-12 | User membership is revoked | `packages/rxdb-client/test/security-reset.test.mjs::pauses, securely clears, notifies, and restarts under a new identifier` | Automated |
| RR-13 | Client schema is outdated | `packages/rxdb-client/test/recovery.test.mjs::surfaces migration and full-resync states through application hooks` | Automated |
| RR-14 | Access token expires during live sync | `packages/rxdb-client/test/refresh.test.mjs::automatically rotates an expiring access token once for concurrent callers` | Automated |
| RR-15 | Refresh session is revoked | `packages/rxdb-client/test/refresh.test.mjs::clears revoked or unavailable refresh state and requires authentication` | Automated |
| RR-16 | Client exceeds a transient rate limit | `crates/mako-gateway/src/replication.rs::authorizes_public_user_context_and_returns_retryable_quota_errors` | Automated |

## Operations / local bootstrap and smoke verification

| ID | Scenario | Primary automated evidence | Status |
| --- | --- | --- | --- |
| LS-01 | Operator follows the quickstart verbatim | `crates/mako-config/src/lib.rs::local_control_storage_resolves_relative_defaults_and_production_stays_strict` and `crates/mako-smoke/tests/happy_path.rs::application_happy_path_succeeds_against_the_real_services` (end-to-end) | Automated |
| LS-02 | Required configuration is absent | `crates/mako-config/src/lib.rs::control_sqlite_rejects_unsafe_paths_identity_limits_and_secret_fields` | Automated |
| LS-03 | Bootstrap produces a usable tenant | `crates/mako-smoke/tests/happy_path.rs::application_happy_path_succeeds_against_the_real_services` (end-to-end) | Automated |
| LS-04 | Bootstrap runs a second time | `crates/mako-smoke/tests/happy_path.rs::bootstrap_converges_on_rerun_and_refuses_outside_a_local_environment` (end-to-end) | Automated |
| LS-05 | Bootstrap is invoked against a non-local environment | `crates/mako-smoke/tests/happy_path.rs::bootstrap_converges_on_rerun_and_refuses_outside_a_local_environment` (end-to-end) and `crates/mako-local-bootstrap/src/main.rs::bootstrap_is_refused_outside_a_local_environment` | Automated |
| LS-06 | Happy path succeeds | `crates/mako-smoke/tests/happy_path.rs::application_happy_path_succeeds_against_the_real_services` (end-to-end) | Automated |
| LS-07 | A step of the happy path regresses | `crates/mako-smoke/tests/happy_path.rs::application_happy_path_succeeds_against_the_real_services` (end-to-end) | Automated |
| LS-08 | Credentials are required | `crates/mako-smoke/tests/happy_path.rs::application_happy_path_succeeds_against_the_real_services` (end-to-end) | Automated |
| LS-09 | Happy path breaks on a branch | `.github/workflows/ci.yml` and `crates/mako-smoke/tests/happy_path.rs::application_happy_path_succeeds_against_the_real_services` (end-to-end) | Automated |
| LS-10 | Coverage is claimed for a mocked test | `scripts/validate-requirements-traceability.js` | Automated |
