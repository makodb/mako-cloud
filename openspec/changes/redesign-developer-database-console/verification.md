# Verification

Implemented locally on September 22, 2026. This change has not been deployed. Existing homepage, CLI, deployment, and sample-application work in the shared worktree was preserved.

## Delivered

- Searchable project home with direct data, schema, and connection actions, project creation, team and billing navigation, and documentation links.
- Environment navigation grouped by database, application, observation, and configuration tasks. Mobile screens use a destination selector. Project pages expose database shortcuts.
- Database overview with collection inventory, lifecycle and resource counts, timestamped usage samples, bounded audit activity, and explicit stale, partial, empty, and unavailable states. Collection loading is independent of summary loading.
- Collection policy inventory linking to each collection's existing editor.
- Data workbench with collection search, document field columns, JSON inspection and conditional editing, query and bulk-job tabs, and retained policy-preview and administrative workflows.
- Management summaries add bounded observations and truncation metadata using the existing authorized telemetry queries. Free-form audit details and cursor tokens are omitted. API keys receive an authorized navigation entry.
- Environment and collection changes discard prior data, drafts, and temporary grants. Late reads are ignored, and late grant issuance is revoked.

## Checks

| Check | Result |
| --- | --- |
| Full console Playwright suite | 88 passed |
| Final database-console and developer-data-workspace suites, including the added late-grant test | 15 passed |
| Final home-dashboard suite, including the added failed-owner-listing test | 6 passed |
| Console unit tests | 10 passed |
| Control-plane service library tests | 58 passed |
| File-storage service smoke test with workspace-summary and navigation assertions | 1 passed over real local HTTP |
| Console production build | Passed |
| Console source and browser-test typechecks | Passed |
| Biome checks on changed console files | Passed |
| Rust formatting on changed service and smoke-test files | Passed |
| Documentation validation | Passed, 195 links checked |
| OpenSpec strict validation | Passed |
| Whitespace check | Passed |

The final focused suites cover the last changes after the full browser run. There are 90 distinct browser tests across these runs. Browser requests use intercepted test APIs. The service smoke test ran against freshly built control-plane, data-plane, and bootstrap binaries with disposable local data and an object-store test server. It verified bounded audit observations, unavailable usage when telemetry is absent, working inventory, an authorized API-key destination, and refusal of unauthenticated summary requests. Usage-sample selection and truncation are covered by Rust unit tests.

The initial smoke assertion assumed telemetry was present. The fixture intentionally starts no telemetry service, so the assertion was corrected to require an unavailable usage section with no payload, while verifying that audit and inventory still work.

The production build reports dependency `use client` directives ignored by Vite and an application chunk over 500 kB. These warnings do not prevent a build.

## Visual review

Inspected desktop home, desktop document browser, and overview at desktop, dark, and 390-pixel mobile widths. Browser coverage also checks mobile page overflow and the destination selector.

Screenshots are local test artifacts under `.local/console-redesign/`:

- `home-desktop.png`
- `overview-desktop.png`
- `overview-dark.png`
- `overview-mobile.png`
- `data-desktop.png`

## Limits

Usage values are the latest samples within the returned bounded observation page, not lifetime totals or billable usage. Partial responses are labelled and link to detailed views. The overview does not claim latency, CPU, or connection metrics that the current summary API does not provide. No data migration or storage-engine change is included.
