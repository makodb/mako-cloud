#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

node scripts/exercise-service-rollback.js
cargo test --locked -p mako-provisioning \
  workflow::tests::retryable_failure_is_compensated_and_can_resume_idempotently -- --exact
cargo test --locked -p mako-control-plane \
  policy::tests::draft_validation_testing_activation_and_rollback_report_epochs -- --exact
cargo test --locked -p mako-control-plane \
  function::tests::deploy_promote_rollback_test_logs_and_delete_follow_safe_lifecycle -- --exact
cargo test --locked -p mako-control-plane \
  collection::tests::compatible_publication_succeeds_and_incompatible_change_requires_migration -- --exact
cargo test --locked -p mako-control-plane \
  operator_authentication::tests::rocksdb_restart_preserves_entitlement_and_never_resurrects_revoked_session -- --exact
cargo test --locked -p mako-control-plane \
  operator_authentication::tests::bootstrap_requires_exact_confirmation_and_is_atomic_and_replay_safe -- --exact
cargo test --locked -p mako-identity \
  signing_keys::tests::encrypted_keys_rotate_with_jwks_overlap_and_retirement -- --exact
cargo test --locked -p mako-storage --test production_startup \
  format_compatible_previous_binary_rollback_uses_the_same_nonempty_volume -- --exact
node scripts/validate-rollback-qualification.js
