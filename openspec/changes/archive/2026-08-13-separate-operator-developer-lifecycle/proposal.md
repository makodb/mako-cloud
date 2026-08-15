## Why

Mako Cloud currently treats developer admission as a prerequisite and side effect of operator authorization, so granting operator access can silently approve the same person's developer account. Developer admission and operator privilege are separate decisions and must be reviewable, reversible, and auditable independently even when they share one login identity.

## What Changes

- Model developer lifecycle state and operator entitlement as independent role state attached to one authentication identity.
- Allow an email-verified identity with an active operator entitlement to sign in to the operator console regardless of whether its developer state is wait-listed, active, rejected, or absent.
- Change operator bootstrap and entitlement administration to grant, replace, or revoke only operator authority; it must never approve, reject, disable, or otherwise transition developer state.
- Preserve normal wait-list review for an operator's own developer application, including the same permission, recent-password verification, confirmation, idempotency, concurrency, and audit requirements used for any other application; a private review reason is optional but remains validated when supplied.
- Add a bounded, page-scoped operator-console action that batch approves selected wait-listed developers with one confirmation while preserving an independent idempotent decision, audit event, notification, and outcome for every target.
- Ensure developer approval, rejection, or developer-only disablement does not grant, alter, or revoke operator authority, and operator entitlement changes do not alter developer status or developer sessions.
- Distinguish shared authentication-identity security events from role lifecycle events: credential compromise, identity deletion, or an account-wide security disable may revoke both roles, while role-specific actions affect only that role.
- Expose developer status and operator entitlement separately in private administration results and operator-console review UI so neither state is inferred from the other.
- Add a protected, idempotent migration operation that can return an operator's developer state to the wait list without changing operator access, enabling the initial operator to approve their own developer application manually.
- **BREAKING**: Operator eligibility no longer requires an active developer lifecycle, and operator bootstrap no longer activates a wait-listed developer automatically.

## Capabilities

### New Capabilities

- `identity/account-role-lifecycle`: Independent developer admission and operator entitlement lifecycles over one verified authentication identity, including self-review and role-isolated administration.

### Modified Capabilities

None. The related developer-registration and operator-authentication capabilities currently exist only in unarchived changes; this change supplies the authoritative cross-role contract and records the superseded assumptions for later reconciliation.

## Impact

- Control-plane identity, developer-registration, operator-authentication, bootstrap, session-revocation, audit, and RocksDB migration logic.
- Private operator administration APIs and `mako-operator-admin` plan/apply behavior.
- Operator sign-in eligibility and operator-console wait-list presentation.
- OpenAPI schemas, management SDK, operator-console selection and decision UX, unit/integration/browser qualification, backup/restore and rollback evidence, and public-beta deployment/runbooks.
- The existing `add-operator-password-signin` assumptions that require an active developer and combine bootstrap with developer activation must be reconciled before those changes are archived.
