# Design system

Every Mako web surface — the developer console and the Rational sample — is dressed by one kit,
`@mako-cloud/ui` in `packages/ui`. It holds the tokens, the components, the icons, the typeface,
and the charts, so a button in the console and a button in Rational are the same button, and a
dark theme is the same dark theme.

## What the kit is

- **Tokens.** Colour, radius, and type are named CSS variables declared once in
  `packages/ui/src/styles.css` with `light-dark()`, so each has a light and a dark value and the
  whole palette follows the document's colour scheme. Tailwind v4 reads them through
  `@theme inline`, so `bg-background`, `text-muted-foreground`, and `border-border` are the same
  colours the components use. The names are exported from `@mako-cloud/ui/tokens` for tests and
  charts.
- **Components.** shadcn/ui-style components built on Radix primitives (`radix-ui`): button, input,
  textarea, label and field, badge, card, separator, skeleton, dialog, sheet, dropdown menu,
  popover, tooltip, tabs, select (native and Radix), checkbox, switch, progress, table, alert,
  empty state, avatar, toast. Radix gives keyboard operation, focus management, and ARIA: a dialog
  takes focus, closes on Escape, and returns focus to what opened it; a menu is arrow-key
  navigable; every control has a visible focus ring in both themes.
- **Icons.** Lucide, imported by the application from `lucide-react`; one weight everywhere.
- **Type.** Inter, self-hosted through `@fontsource-variable/inter` and imported by the stylesheet,
  so no page depends on a font service and an offline-first app keeps its type offline. Money is
  set in tabular figures with the `money` utility.
- **Charts.** Wrappers over Recharts — `LineChart`, `AreaChart`, `BarChart`, `DonutChart`,
  `Sparkline` — that use the palette's series colours, format axes, show a tooltip in the kit's
  style, carry an accessible name (`role="img"` with the chart's title), and never animate when the
  device asks for reduced motion or a browser suite marks the root with `data-testing`.

## How an application adopts it

1. Depend on `@mako-cloud/ui` and add the Tailwind plugin to `vite.config.ts`:

   ```ts
   import tailwindcss from "@tailwindcss/vite";
   export default defineConfig({ plugins: [tailwindcss()] });
   ```

2. Import the stylesheet once, in the entry module: `import "@mako-cloud/ui/styles.css";`. That
   brings Tailwind, the typeface, the tokens, the base styles, and — through `@source "./"` inside
   the stylesheet — the kit's own component classes, wherever the kit's sources live.
3. Use the components and utilities. Layout that is specific to a screen is written with Tailwind
   utilities in the screen; there is no application stylesheet to keep.
4. Keep the theme with `useTheme(storageKey)`, which applies `data-theme` to the document root and
   remembers the choice per device; no stored choice means no attribute, and the device's own
   preference shows through. `ThemeToggle` is the button that flips it.

Adoption is checked, not hoped for: `npm run validate:ui-kit` (`scripts/validate-ui-kit.js`)
refuses a raw `<button>`, `<input>`, `<select>`, `<textarea>`, or `<dialog>` in
`examples/rational/src` or `apps/console/src` and names the file. A file picker (`type="file"`)
and a hidden input are the deliberate exceptions. An application still being moved over can be
allow-listed by path in the script, with the reason beside it.

## How a standalone export carries it

Rational is exported to its own repository by `scripts/export-rational-app.mjs`. The export copies
`packages/ui/src` to `src/kit` in the checkout, points `@mako-cloud/ui` at it with a `paths` entry
in the rewritten tsconfigs and a `resolve.alias` in the generated `vite.config.ts`, and writes the
kit's dependencies into the generated `package.json` with the exact versions the workspace lock
resolved. The kit's stylesheet uses `@source "./"` relative to itself, so it compiles its own
classes from `src/kit` there and from `packages/ui/src` here with no change.

## Serving it

The typeface is a file the application ships, so whatever serves the built site has to hand it
over. On the public beta the console is served by Caddy from the release directory, and its asset
route names the kinds of file it will serve; when the kit arrived the route still listed only
`css` and `js`, so every `woff2` answered 404 and the console quietly fell back to a system face —
nothing failed, it just stopped being Inter. The route now serves `css`, `js`, and `woff2`, and
`npm run validate:public-beta-caddy` reads the extensions out of the template and asserts that
all three are there. It cannot know what a future build will emit, so a new asset kind still has
to be added to both the route and that assertion; what the check buys is that nobody quietly
removes one of the three.

## Tests

- `packages/ui/test/tokens.test.mjs` reads every `light-dark()` pair from the stylesheet, converts
  both sides from oklch to sRGB, and checks each text-on-surface pair the kit promises
  (`READABLE_PAIRS` in `tokens.ts`) against the AA contrast ratio in both themes; it also checks
  that every token is mapped into the Tailwind theme and that the stylesheet imports the framework,
  the font, and its own sources.
- `packages/ui/test/components.test.mjs` checks the public surface: every promised component is
  exported, `cn` merges utilities, `initials` reduces a name, and motion is refused outside a
  browser.
- `scripts/test/validate-ui-kit.test.js` drives the validator over fixtures.
- The applications' browser suites are the integration gate: a re-skinned screen ships only with
  its Playwright scenarios green.

## Bundle size

Rational, production build, gzipped. Before the screens moved onto the kit the app shipped one
script of 266 KB with 17 KB of hand-written CSS. On the kit it ships 444 KB of script in six
cacheable chunks and 11 KB of CSS; the growth is Recharts (117 KB, its own chunk) and the Radix
primitives (30 KB), and it buys tooltips, formatted axes, keyboard-operable menus and dialogs,
and one palette. The chunks:

| Chunk | What is in it | Gzipped |
| --- | --- | --- |
| `index` | Rational's own screens, selectors, and data layer | 128 KB |
| `charts` | Recharts and the d3 modules it draws with | 95 KB |
| `database` | RxDB, Dexie, RxJS, and the `@mako-cloud/rxdb` client | 73 KB |
| `react` | React and the scheduler | 59 KB |
| `vendor` | Lucide icons in use, the state and utility libraries Recharts shares, everything else | 57 KB |
| `primitives` | Radix primitives | 30 KB |

The Inter variable font adds 48 KB for Latin, with the other scripts loaded only when a page
uses them.

The console, same build, same units. Before the re-skin it shipped one script of 168 KB with 7 KB
of hand-written CSS; on the kit it ships 223 KB in four chunks and 12 KB of CSS. It draws no
charts, so it pays for the primitives and not for Recharts:

| Chunk | What is in it | Gzipped |
| --- | --- | --- |
| `index` | The console's own screens and its management SDK | 128 KB |
| `react` | React and the scheduler | 59 KB |
| `primitives` | Radix primitives | 19 KB |
| `vendor` | Lucide icons in use, Tailwind runtime helpers, everything else | 18 KB |
