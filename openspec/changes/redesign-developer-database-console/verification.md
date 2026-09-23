# Verification

Deployed to https://cloud-test.makodb.com on September 23, 2026 from commit `5d26be0c921cc23824dea241a1f5e1fdf7da75e6`. The release includes the homepage, documentation navigation, and `mako-cloud` CLI changes, plus the current upstream fixes for project provisioning, authentication, explorer access, storage, and replication.

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
| Full console Playwright suite after merging current main | 90 passed |
| Console unit tests | 10 passed |
| CLI unit tests | 130 passed |
| Management SDK unit tests | 16 passed |
| Control-plane, control-plane service, storage, and sync library tests after merging current main | 270 passed |
| File-storage service smoke test with workspace-summary and navigation assertions | 1 passed over real local HTTP |
| Console production build | Passed |
| Console source and browser-test typechecks | Passed |
| Biome checks on changed console files | Passed |
| Rust formatting on changed service and smoke-test files | Passed |
| Documentation validation | Passed, 195 links checked |
| OpenSpec strict validation | Passed |
| Public beta local deployment preflight | Passed |
| Immutable production release build | Passed |
| Whitespace check | Passed |

The final full browser run covers the merged code, including late-grant cleanup and failed owner listing. Browser requests use intercepted test APIs. The service smoke test ran against freshly built control-plane, data-plane, and bootstrap binaries with disposable local data and an object-store test server. It verified bounded audit observations, unavailable usage when telemetry is absent, working inventory, an authorized API-key destination, and refusal of unauthenticated summary requests. Usage-sample selection and truncation are covered by Rust unit tests.

The initial smoke assertion assumed telemetry was present. The fixture intentionally starts no telemetry service, so the assertion was corrected to require an unavailable usage section with no payload, while verifying that audit and inventory still work.

The production build reports dependency `use client` directives ignored by Vite and an application chunk over 500 kB. These warnings do not prevent a build.

## Deployment

Release `d7bec53355decb88145469f70a9fd6e97aa53d9631449d936f9c8ca461a15d86` is selected on the existing cloud-test host. The upgrade verified checkpoints and storage compatibility before selecting the release. All four services and storage roots are active, all four readiness endpoints return 200, and backup and preview-admission timers are active.

The public homepage and user book return 200 over trusted HTTPS with HSTS. HTTP redirects to HTTPS with 308. Five served asset hashes match the release manifest. An unauthenticated workspace summary request returns 401, and internal and unknown route probes return 404. Developer registration and operator password login remain enabled; operator break-glass bearer login remains disabled.

A browser loaded the deployed homepage, docs, database overview, and document browser without JavaScript errors. Signed-in management requests were intercepted with synthetic fixtures. This check used the real deployed assets without reading tenant documents or changing live accounts. Full hosted qualification, rollback drills, mail delivery, and account lifecycle suites were not rerun for this release. Existing risk-accepted preview admission remains in effect.

Measurements are recorded in [console deployment evidence](../../../docs/evidence/public-beta-console-deployment.json) and the [current deployment record](../../../docs/evidence/public-beta-current-deployment.json). Earlier qualification evidence retains its original release bindings.

## Visual review

Inspected desktop home, desktop document browser, and overview at desktop, dark, and 390-pixel mobile widths. Browser coverage also checks mobile page overflow and the destination selector.

Screenshots are local test artifacts under `.local/console-redesign/`:

- `home-desktop.png`
- `overview-desktop.png`
- `overview-dark.png`
- `overview-mobile.png`
- `data-desktop.png`
- `live-overview.png`
- `live-data.png`

## Limits

Usage values are the latest samples within the returned bounded observation page, not lifetime totals or billable usage. Partial responses are labelled and link to detailed views. The overview does not claim latency, CPU, or connection metrics that the current summary API does not provide. No data migration or storage-engine change is included.
