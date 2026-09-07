## Context

See proposal.md — Why. Both web surfaces are React 19 on Vite 7 with hand-written CSS: Rational has twelve stylesheets under `src/ui/styles` and eight hand-drawn SVG chart components; the console has eleven stylesheets imported from `main.tsx`. Neither uses a component library, an icon set, or a chosen typeface. Rational already keeps a per-device theme in `localStorage` and applies it as `data-theme` on the document root; the console has no theme.

Constraints that shape the approach:

- Rational is exported to `github.com/shuaimu/rational` by `scripts/export-rational-app.mjs`, which rewrites `package.json`, `vite.config.ts`, and the tsconfigs so the checkout builds with `npm ci` and no workspace. The only workspace dependency today, `@mako-cloud/rxdb`, is rewritten to a published git specifier. Whatever the kit becomes, the export must keep building alone.
- Rational's browser suites select by role and text almost exclusively (893 role queries, four class selectors); the console's e2e suite has 73 class selectors on a handful of status classes (`.status-badge`, `.schedule-state`, `.run-outcome`, …). Those classes are stable hooks and survive the re-skin.
- Biome formats JS/TS/JSON only; CSS is not linted. TypeScript is strict with `verbatimModuleSyntax` and `exactOptionalPropertyTypes`; workspace packages build to `dist` with `tsc -b` and are referenced by project references.
- The console ships inside the public-beta release, so its re-skin reaches the beta host on the next upgrade.

## Goals / Non-Goals

**Goals:**
- One kit, one set of tokens, one typeface, one icon set, one chart library, consumed by both apps through a single import.
- Delete application CSS as screens migrate; what remains is layout specific to a screen, written with the kit's utilities.
- Keep every existing browser and unit test passing at every step; the re-skin never changes behaviour.
- The standalone Rational export keeps working without a publish step for the kit.

**Non-Goals:**
- No framework change: React and Vite stay; no Svelte, no Next.
- No redesign of information architecture or flows — the same screens, routes, and actions.
- No runtime theming service or per-household branding.
- No new spec behaviour for the sample beyond presentation and the theme preference.

## Decisions

1. **Tailwind v4, CSS-first, with the Vite plugin.** Tokens are declared once in the kit's stylesheet with `@theme` and semantic CSS variables (`--background`, `--foreground`, `--primary`, `--muted`, `--destructive`, `--chart-1..5`, radius, and a type scale) under `:root`, redefined under `:root[data-theme="dark"]` and under `prefers-color-scheme: dark` for a root with no explicit theme. Utilities are compiled at build time, so nothing new runs in the browser. *Alternative:* CSS Modules per screen — keeps the status quo of twelve stylesheets and gives no shared vocabulary. *Alternative:* a CSS-in-JS library — runtime cost and a second styling system.

2. **shadcn/ui-style components vendored into `packages/ui`, built on the `radix-ui` package.** The components are source files in the repository (button, input, label, textarea, select, checkbox, switch, dialog, sheet, dropdown-menu, popover, tooltip, tabs, table, card, badge, separator, progress, toast, skeleton, command-less), composed with `class-variance-authority` and a `cn()` helper over `clsx` + `tailwind-merge`. Vendoring means the code is ours to read and change and the kit has no dependency on a generator at build time; Radix gives keyboard operation, focus management, and ARIA for free, which is what the accessibility requirement asks for. *Alternative:* Mantine or Chakra — batteries included but their own theming systems fight Tailwind and pull a runtime. *Alternative:* Radix Themes — its own token system, no Tailwind interop.

3. **`@mako-cloud/ui` is a workspace package built like the others.** `tsc -b` emits `dist` with declarations; `exports` names `.` (components, `cn`, chart wrappers), `./styles.css` (tokens, base, font import, `@source` for its own sources so Tailwind sees the kit's classes), and `./tokens` (the token names as a typed constant, for tests and charts). Apps add a project reference and import `@mako-cloud/ui/styles.css` once in their entry. *Alternative:* ship TS source and let Vite compile it — simpler, but breaks `tsc -b` project references and the strict typecheck the workspace relies on.

4. **The export vendors the kit.** `scripts/export-rational-app.mjs` copies `packages/ui/src` to `src/kit` in the exported checkout, adds a `paths` entry for `@mako-cloud/ui` and `@mako-cloud/ui/*` to the rewritten tsconfigs and a matching `resolve.alias` to the generated `vite.config.ts`, and adds the kit's runtime and build dependencies to the generated `package.json` (`radix-ui`, `class-variance-authority`, `clsx`, `tailwind-merge`, `lucide-react`, `recharts`, `@fontsource-variable/inter`, `tailwindcss`, `@tailwindcss/vite`) with exact versions read from the workspace lock. The kit's stylesheet uses `@source "./"` relative to itself, so it scans `src/kit` in the export and `packages/ui/src` in the workspace with no change. *Alternative:* publish the kit like the RxDB client — a release step for every visual tweak, and a second repository to keep in sync.

5. **Inter, self-hosted through `@fontsource-variable/inter`.** Imported from the kit's stylesheet; Vite bundles the woff2 files with the app, so the published site loads no third-party font and the offline-first app keeps its type offline. *Alternative:* Google Fonts — a third-party request on every page load and nothing offline.

6. **Lucide for icons.** Tree-shaken React components, one visual weight, covers every icon both apps need (navigation, actions, statuses, account classes). Rational's merchant initials stay as they are (the parity matrix already records logos as out of scope).

7. **Recharts, wrapped by the kit.** `Chart` wrappers set the palette (`--chart-n`), the tooltip and legend styling, formatted axes, an accessible name (`role="img"` with `aria-label`), and disable animation under `prefers-reduced-motion` and in the browser suites. Line, area, bar, donut, and sparkline move to Recharts; Sankey, treemap, and the month calendar stay hand-drawn SVG restyled with tokens, because Recharts' equivalents are weaker than what Rational already has. *Alternative:* keep every chart hand-drawn — no tooltips, no consistent axes, and every new chart is a project.

8. **Adoption is enforced, not hoped for.** `scripts/validate-ui-kit.js` (`npm run validate:ui-kit`, wired into CI) refuses a raw `<button`, `<input`, `<select`, `<textarea`, or `<dialog` element in `examples/rational/src` or `apps/console/src` outside an allow-list of the kit's own wrappers, and names the file. It is the test for the "raw control slips in" scenario.

9. **The theme is a root attribute in both apps.** `data-theme="light|dark"` on the document root, kept in `localStorage` per device (Rational's existing key stays; the console gets `mako.console.theme`), with no attribute meaning "follow the device". The kit exposes `useTheme()` so both shells use the same hook and the same toggle.

10. **Migration order: kit → Rational shell → Rational screens → console shell → console destinations.** Each step deletes the CSS it replaces and ends with the app's browser suite green. Rational first because it is the showcase and its suites are the most thorough; the console follows with the same components and lessons.

## Risks / Trade-offs

- [Existing assertions against chart SVG text] → run the Rational suites after the chart swap and adapt assertions to the Recharts DOM only where they targeted drawing details, never behaviour; keep hand-drawn charts where Recharts has no equivalent.
- [Console e2e class selectors] → keep every class the suite selects on the re-skinned element; the validator does not touch class names.
- [Bundle growth: Recharts and Radix add to a console bundle already over 500 KB] → split vendor chunks (`react`, `recharts`, `radix-ui`) with `manualChunks`; measure before and after and record it in the docs page.
- [Tailwind scanning across the workspace symlink] → the kit's stylesheet carries `@source "./"` for its own sources; each app's sources are scanned from its root by the Vite plugin. A missing class shows up immediately as an unstyled control in the browser suite.
- [Export lock file drift] → the exporter already refuses a lock file out of date with the generated manifest; after the kit lands, `npm install` in the export checkout regenerates it once, and the exported build in GitHub Actions confirms it.
- [Two theming attributes] → both apps use `data-theme`; the kit owns the definition, so there is exactly one place the palette is declared.
- [Visual regressions the suites cannot see] → a screenshot pass per screen during migration (Playwright screenshots into the scratchpad, reviewed by eye), not committed as golden images, which would rot.

## Migration Plan

1. Land the kit with unit tests and the validator, apps untouched (validator not yet wired to CI so it does not fail on the old raw controls).
2. Rational: shell and sign-in, then each screen; delete each stylesheet as its screen moves; suites after each.
3. Wire `validate:ui-kit` into CI once Rational is clean; console still allow-listed by path until it is done.
4. Console: shell with theme, then destinations; remove the console allow-list; suites.
5. Export Rational (`scripts/export-rational-app.mjs`), regenerate the lock in the checkout, confirm the Pages build.
6. Build and deploy a public-beta release so the console re-skin is qualified on the host by the existing browser and hosted suites.

Rollback is the previous commit; no data, schema, or API changes are involved.

## Open Questions

- Whether the console's operator control centre (a separate operator identity and screen set) adopts the kit in this change or the next. It is included in the task list as the last console step and can be deferred without affecting the specs.
