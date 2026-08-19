## MODIFIED Requirements

### Requirement: Operator authentication survives qualified restart and recovery
Operator entitlements, authorization epochs, revocations, password-verification state, and protected session records SHALL use the control plane's SQLite ownership, backup, restore, and migration controls. Restart, exact-release upgrade, SQLite-compatible rollback, checkpoint restore, and empty-target recovery MUST preserve committed entitlements and MUST NOT resurrect expired or revoked sessions. Operator authentication and step-up MUST NOT require tenant data-plane RocksDB readiness.

#### Scenario: Restart preserves current authority
- **WHEN** the control plane restarts after committing an operator entitlement and a current session
- **THEN** the entitlement remains authoritative and the session is accepted only if it is still unexpired, unrevoked, credential-current, and operator-epoch-current

#### Scenario: Restore does not resurrect revoked access
- **WHEN** a qualified SQLite backup is restored after an operator session or entitlement has been revoked according to the recovery contract
- **THEN** post-restore reconciliation preserves the revocation boundary and no stale credential gains operator authority

#### Scenario: Operator signs in while tenant storage is down
- **WHEN** the control database is healthy and one or more tenant RocksDB services are unavailable
- **THEN** an eligible operator can establish and step up a cookie-backed operator session without bypassing normal password, entitlement, throttling, or audit controls

