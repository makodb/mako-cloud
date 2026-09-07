## 1. The kit

- [x] 1.1 Create `packages/ui` (`@mako-cloud/ui`): manifest with `exports` for `.`, `./styles.css`, `./tokens`; `tsconfig.json` composite build to `dist`; add to the root `tsc -b` references and to Biome's scope
- [x] 1.2 Add dependencies (`tailwindcss`, `@tailwindcss/vite`, `radix-ui`, `class-variance-authority`, `clsx`, `tailwind-merge`, `lucide-react`, `recharts`, `@fontsource-variable/inter`) and refresh `package-lock.json`
- [x] 1.3 Write `src/styles.css`: Tailwind import, `@source "./"`, Inter import, `@theme` tokens, light palette on `:root`, dark palette under `:root[data-theme="dark"]` and under `prefers-color-scheme: dark` for an unthemed root, base styles and focus ring
- [x] 1.4 Export the token names and palette pairs from `src/tokens.ts`; unit test that every foreground/background pair meets AA contrast in both themes
- [x] 1.5 Write `cn()` and the components: button, input, label, textarea, select, checkbox, switch, dialog, sheet, dropdown-menu, popover, tooltip, tabs, table, card, badge, separator, progress, toast, skeleton, empty-state
- [x] 1.6 Write `useTheme()` (root attribute + per-device storage key parameter) and a `ThemeToggle`
- [x] 1.7 Write the chart wrappers on Recharts: `LineChart`, `AreaChart`, `BarChart`, `DonutChart`, `Sparkline` with palette series colours, formatted axes, tooltip, accessible name, and no animation under reduced motion or when `data-testing` is set on the root
- [x] 1.8 Unit tests over `dist`: `cn()` merges, every component module exports a component, the stylesheet declares the font, tokens, both palettes, and `@source`
- [x] 1.9 Write `scripts/validate-ui-kit.js` and `npm run validate:ui-kit` with a unit test: refuses raw `<button|input|select|textarea|dialog` in `examples/rational/src` and `apps/console/src` outside the allow-list, names the file; allow-list the console by path until task 4 is done
- [x] 1.10 Write `docs/design-system.md` (what the kit is, how an app adopts it, how the export carries it, tokens and theme, charts) and index it in `docs/README.md`

## 2. Rational on the kit

- [x] 2.1 Add the project reference and the Vite plugin; import `@mako-cloud/ui/styles.css` from `main.tsx`; replace the theme hook in `shell.tsx` with the kit's `useTheme()` keeping the `rational.theme` key
- [x] 2.2 Re-skin the shell: sidebar navigation with Lucide icons, top bar with space switcher, notification bell (popover), theme toggle; delete `shell.css`
- [x] 2.3 Re-skin sign-in (card, inputs, provider buttons, magic-link state)
- [x] 2.4 Re-skin the dashboard: widget cards, net worth trend on the kit's `AreaChart`, upcoming bills, budget summary, customise sheet; delete `dashboard.css`
- [x] 2.5 Re-skin accounts, account detail (balance history on `LineChart`, update-value dialog), account form, investments and holdings (allocation on `DonutChart`); delete `accounts.css`
- [x] 2.6 Re-skin transactions: filter bar (selects, date range popover), grouped table with tabular figures and signed colours, bulk-edit toolbar, needs-review tab; transaction panel as a sheet with tabs (details, splits, receipts, transfer, rule); delete `transactions.css`
- [x] 2.7 Re-skin budget (both modes, progress bars, month navigation, copy-last-month) and cash flow (`BarChart` for income vs spending, `DonutChart` breakdowns, restyled Sankey and treemap, category trend on `LineChart`); delete `budget.css` and `cash-flow.css`
- [x] 2.8 Re-skin recurring (list, restyled month calendar, paid and late badges) and goals (progress, projections on `LineChart`, contribution history); delete `recurring.css` and `goals.css`
- [x] 2.9 Re-skin settings hub and pages (household, members, categories with drag order, merchants, rules with preview, tags, connections, notifications, import wizard, data export); delete `settings.css`, `settings-pages.css`, `taxonomy.css`
- [x] 2.10 Remove `src/ui/charts/{line,bars,donut,progress}.tsx` and `charts.css`; keep `sankey`, `treemap`, `calendar` restyled with tokens; delete what is left of `styles.css` beyond screen layout
- [x] 2.11 Browser suites (`test/` and `test-live/`), unit tests, typecheck, lint, and `validate:rational-parity` green; screenshot pass per screen in both themes
- [ ] 2.12 Add traceability rows for the Rational scenarios (theme dresses every screen, money table, parity unchanged) and the design-system scenarios they cover

## 3. The export carries the kit

- [x] 3.1 `scripts/export-rational-app.mjs`: copy `packages/ui/src` to `src/kit`; add `paths` for `@mako-cloud/ui` and `@mako-cloud/ui/*` to the rewritten tsconfigs; add a matching `resolve.alias` and the Tailwind plugin to the generated `vite.config.ts`; add the kit's dependencies with exact versions from the workspace lock to the generated `package.json`
- [x] 3.2 Unit test the export's manifest, tsconfig, and Vite config generation for the kit
- [ ] 3.3 Export, run `npm install` in the checkout to refresh its lock file, build and run its browser suite there, push, confirm the Pages workflow
- [ ] 3.4 Traceability row for "the exported application builds alone"

## 4. The console on the kit

- [x] 4.1 Add the project reference, Vite plugin, stylesheet import; add `useTheme()` with `mako.console.theme` and a toggle in the shell header
- [ ] 4.2 Re-skin the navigation shell (destinations with icons, unavailable state, context header, deep links); delete `styles.css` shell rules
- [ ] 4.3 Re-skin the home dashboard, onboarding, project home, and settings; delete `home.css`, `project-home.css`
- [ ] 4.4 Re-skin Database (collections, schemas, indexes), Explorer, Auth (users, providers), Storage; delete `storage.css`, `auth.css`
- [ ] 4.5 Re-skin Functions (deployments, secrets, schedules), Sync (replication, webhooks), Logs, Observability; keep the classes the e2e suite selects; delete `function-schedules.css`, `webhooks.css`, `usage-activity.css`, `surfaced.css`
- [ ] 4.6 Re-skin API & Keys, API docs, Backups, custom domains, allowed origins, billing; delete `api-docs.css`, `custom-domains.css`, `allowed-origins.css`
- [ ] 4.7 Re-skin the operator control centre and operator auth screens
- [ ] 4.8 Remove the console allow-list from `validate:ui-kit`; wire `validate:ui-kit` into CI
- [ ] 4.9 Console e2e, unit tests, typecheck, lint green; split vendor chunks and record bundle sizes before and after in `docs/design-system.md`
- [ ] 4.10 Traceability rows for "the console follows the developer's theme" and the remaining design-system scenarios

## 5. Ship

- [ ] 5.1 Update `docs/rational.md` and `examples/rational/README.md` for the new look and the kit
- [ ] 5.2 Build a public-beta release, deploy, run the hosted and operator browser qualification, refresh evidence, commit
