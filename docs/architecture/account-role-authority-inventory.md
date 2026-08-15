# Account and role authority inventory

This inventory records which authority owns each existing `DeveloperAccount` field and epoch consumer. Every shared identity, developer-role, operator-role, organization/project, control audit, provisioning, idempotency, function-metadata, and mail-outbox record listed here is now control-plane SQLite state. Application users, project credentials/signing keys, documents, policies, indexes, and RxDB state remain tenant RocksDB state. Browser session storage holds only the existing short-lived developer token; browser SQLite, IndexedDB, Dexie, RxDB, and local storage are not durable authorities.

| Current data or consumer | Current use | Required authority after this change |
| --- | --- | --- |
| `DeveloperAccount.normalized_email`, password hash, email verification, authentication timestamp | Registration, verification, password login/recovery, operator password verification | Shared authentication identity |
| `DeveloperAccount.status`, review summary, status index | Developer wait-list and developer access lifecycle | Developer role only |
| `DeveloperAccount.authorization_epoch` in developer access/refresh claims | Reject stale developer role sessions after lifecycle or password changes | Developer epoch plus shared credential epoch |
| `DeveloperAccount.authorization_epoch` in operator session records | Reject operator sessions after any developer or credential change | Shared credential epoch only; operator permissions use operator epoch |
| `OperatorEntitlementRecord.operator_epoch` | Grant replacement/revocation and permission snapshot invalidation | Operator role only |
| `DeveloperRegistrationStore::replace_account_with` | Atomically writes credentials and developer lifecycle in one legacy record | Compare/write shared identity and developer role independently in one transaction |
| `DeveloperRegistrationStore::migrate_legacy_developers` | Converts pre-registration identities into version-1 developer accounts | Backfill versioned authentication-identity and developer-role records, retain legacy keys, and commit a completion marker last |
| `DeveloperRegistrationStore::review_page` and developer status indexes | Lists wait-listed accounts | Developer role/status index joined to bounded identity presentation |
| `DeveloperRegistrationService::{verify_email,sign_in,refresh,recover}` | Mixes credential and developer lifecycle checks through an aggregate | Credential operations use identity security/epoch; developer access additionally uses role state/epoch |
| `ControlPlaneAuthenticator` and `DeveloperIdentityProvider` | Validate active developer claims against the single epoch | Validate credential and developer epochs for developer authority |
| `OperatorAuthenticationService::{sign_in,authenticate}` | Requires developer `Active` and compares developer epoch | Require verified security-active identity plus entitlement; compare credential and operator epochs only |
| `OperatorEntitlementService` and `OperatorBootstrapService` | Require active developer; bootstrap may activate wait-listed developer | Mutate entitlement only and report developer state as unchanged |
| Password recovery/change | Replaces password and advances the single epoch | Advance credential epoch and invalidate both developer and operator sessions without altering roles |
| Developer approval/rejection/disable | Advances the single epoch | Advance developer epoch and invalidate only developer sessions |
| Operator replacement/revocation | Advances operator epoch and revokes operator sessions | Operator role only; developer role/sessions unchanged |
| SQLite backup, restore, and compatible rollback | Captures every control-plane keyspace | Include identity/role keyspaces, sessions, audit, idempotency, outbox, project metadata, migration receipt, and durable high water; never roll back to a RocksDB-only binary after SQLite writes |
| Audit events and admin plan/apply results | Sometimes describe combined bootstrap lifecycle | Name the affected role and present developer/operator before-and-after state separately |

Repository hotspots checked:

- `crates/mako-control-plane/src/developer_registration.rs`
- `crates/mako-control-plane/src/developer_workflow.rs`
- `crates/mako-control-plane/src/developer_identity.rs`
- `crates/mako-control-plane/src/operator_authentication.rs`
- `crates/mako-control-plane/src/keyspace.rs`
- `services/mako-control-plane/src/{developer_auth_http,operator_auth_http,internal_http}.rs`
- `services/mako-control-plane/src/bin/mako-operator-admin.rs`
- `apps/console/src/{operator-auth,operator-management,operator-waitlist}.tsx`
- release backup, restore, rollback, and public-preview qualification scripts under `scripts/`, `infra/`, and `.local/qualification/`
