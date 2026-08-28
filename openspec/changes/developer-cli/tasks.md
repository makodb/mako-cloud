## 1. Foundation

- [ ] 1.1 Command registry and dispatcher over `node:util` `parseArgs`: noun–verb tree, generated help, global flags (`--endpoint`, `--profile`, `--json`, `--all`, `--yes`, `--wait`, `--timeout`), `mako functions serve` registered unchanged.
- [ ] 1.2 Credential store: profiles file under the config directory with 0700/0600 creation and unsafe-mode refusal, `MAKO_TOKEN` / `MAKO_ENDPOINT` precedence without persistence, session refresh before expiry, wait-listed session reporting.
- [ ] 1.3 Output and errors: human tables and key/value rendering, JSON verbatim, pagination following, addressed-error rendering with retry advice, secret redaction, exit-code map.
- [ ] 1.4 Parity manifest and test: `operationId` → command table, OpenAPI loader, path-prefix exclusions printed by the test.
- [ ] 1.5 Loopback mock of the management API for unit tests, with fixtures typed from the generated schema, header assertions (bearer, confirmation, idempotency), and stdout/stderr/file capture for secret tests.

## 2. Identity and ownership

- [ ] 2.1 `mako auth`: login, logout (server-side revoke), status, whoami, register, verify-email, resend-verification, recover-password; step-up prompt helper with `MAKO_STEP_UP_PASSWORD_FILE`.
- [ ] 2.2 `mako auth token`: create (permissions and optional project/environment scope, secret shown once), list, revoke, rotate.
- [ ] 2.3 `mako teams`: list, create, get, rename, delete, restore, bill (`--period`); `members list|update|remove`; `invitations create|accept`.
- [ ] 2.4 `mako projects` and `mako envs`: list, create (personal by default, `--team`), get, suspend, restore, delete, with `--wait`.

## 3. Database

- [ ] 3.1 `mako collections`: list, create, get, `schema publish`, `migrations create|get|update`.
- [ ] 3.2 `mako indexes`: list, create, get, delete; `mako observability index-state`.
- [ ] 3.3 `mako policies`: get, draft, validate, test, activate (confirmed), rollback.
- [ ] 3.4 `mako users`: search, create, invite, get, update-metadata, disable, restore, delete, revoke-sessions, revoke-session.
- [ ] 3.5 `mako keys`: `public create|get|retire|rotate`, `service create|get|retire|rotate`, `signing init|list|rotate`.

## 4. Functions

- [ ] 4.1 `mako functions`: list, create, get, update, delete, `deployments list|get|delete|promote|rollback|health`, invoke, test, logs, `secrets create|get|retire|rotate`.
- [ ] 4.2 `mako functions deploy`: bundle with the serve bundler, upload, create deployment, health check, promote unless `--no-promote`, identifiers printed per step.

## 5. Observation

- [ ] 5.1 `mako logs` (project logs with level, source, time window, cursor), `mako observability usage|quotas|health|replication-errors|auth-events|function-metrics|audit`, `mako activity`, `mako usage`, `mako bill`.
- [ ] 5.2 `mako workspace summary|nav|connect|check`, `mako sync summary`, `mako backups list`, `mako backups restore-requests list|create`.

## 6. Data

- [ ] 6.1 `mako explorer`: grant issue/revoke handled internally with step-up; get, browse, plan, query, history, simulate, mutate; capabilities kept in memory only.
- [ ] 6.2 `mako data jobs list|get|cancel`, `mako data export` (job, wait, download grant, stream to file), `mako data import` (upload grant, stream, dry run, confirm with `--yes` or after review).

## 7. Verification and release

- [ ] 7.1 Unit tests per command group over the loopback mock; secret-handling test over captured output and the credential file; exit-code test.
- [ ] 7.2 `packages/cli/test/cli.integration.mjs` against the local stack in `npm run test:integration`: login, project create with wait, schema publish, policy activate, function deploy, log read, export.
- [ ] 7.3 Verify the beta's Caddy admission allows every route the CLI uses without a browser origin; record the check.
- [ ] 7.4 `docs/cli.md` with the command reference generated from the registry, `docs/README.md` row, traceability rows, CI wiring; build the package and hand publishing to the user.
