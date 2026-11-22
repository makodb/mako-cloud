## 1. Specifications

- [x] 1.1 `cloud/developer-cli`: reword "Destructive commands are confirmed" (identifier typed back; policy activation always; rollback, backup restore, rotation, data-job cancel and document mutation confirmed).
- [x] 1.2 `identity/operator-authentication`: state `Path=/v1` in "Operator sessions are isolated, cookie-backed, and revocable".
- [x] 1.3 `storage/control-plane-sqlite`: an incomplete temporary target is refused and a completed migration is answered from its receipt, in "RocksDB-to-SQLite migration is verified and restart safe".
- [x] 1.4 `cloud/developer-database-workspace`: "API and RxDB connection guidance" shows the public key id and a placeholder, never the value.

## 2. Evidence that the code already meets them

- [x] 2.1 `packages/cli/src/cli/context.ts` asks for the resource to be typed back, and the `destructive` markers in `packages/cli/src/commands/` cover policy activation and rollback, deployment promotion and rollback, restore requests, rotations, data-job cancel and explorer mutations.
- [x] 2.2 `crates/mako-control-plane/src/operator_authentication.rs` sets `Path=/v1; Secure; HttpOnly; SameSite=Strict`.
- [x] 2.3 `crates/mako-storage/src/control_migration.rs` returns the completed migration's receipt when the final target exists and refuses when the temporary target exists; `crates/mako-storage/tests/control_sqlite_recovery.rs::migration_refuses_non_control_keys_incomplete_targets_and_receipt_drift` covers the refusal.
- [x] 2.4 `services/mako-control-plane/src/workspace_http.rs` returns the active public key id and an empty public key value.

## 3. Validation

- [x] 3.1 Run `openspec validate align-four-requirements-with-the-books --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`.
