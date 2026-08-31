## ADDED Requirements

### Requirement: Plaid-connected accounts
Rational SHALL let an editor connect a household account to a real institution through Plaid's Link flow in sandbox mode, using the platform's declared-egress capability rather than any special treatment: a function issues the Link token, exchanges the public token, and keeps the resulting access token where no application user can read it. The scheduled sync SHALL then import that connection's transactions with Plaid's cursor protocol — new entries appear once, corrected entries update the same transaction, and an entry the institution replaced (a pending charge that posted) does not survive as a duplicate. Aggregator credentials and tokens MUST never reach a browser, a replicated collection an application user can read, or a function response. When no Plaid credentials are configured, the connection option is absent and everything else — the simulated institution, CSV import, all tests that gate CI — SHALL work unchanged.

#### Scenario: An editor links an account through Plaid Sandbox
- **WHEN** an editor completes the Link flow against Plaid Sandbox and the connection's schedule runs
- **THEN** the linked institution's transactions appear under the household's account without duplicating ones already synced, and the connection shows its last sync time and outcome like any other

#### Scenario: A pending charge posts between syncs
- **WHEN** a synced pending transaction is replaced by its posted form in a later sync
- **THEN** the household sees one transaction — updated, not doubled — and any alert it fired is not fired again

#### Scenario: No application user can reach the Plaid token
- **WHEN** any signed-in member, on any device, queries every collection they can open and calls every function route they can call
- **THEN** no Plaid access token, client identifier, or secret is present in any response, replicated document, or error

#### Scenario: Rational without Plaid credentials is whole
- **WHEN** the app and its test suites run with no Plaid credentials configured
- **THEN** the Plaid connection option is not offered, the simulated institution and CSV import work unchanged, and every CI-gating suite passes without external network access
