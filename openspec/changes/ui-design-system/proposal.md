## Why

Rational reaches 92.9% of Monarch's features but still looks like an engineering prototype: two apps, 6,000 lines of hand-written CSS between them, no shared components, no icon set, no chosen typeface, and charts drawn by hand. The console has the same problem. Feature breadth was the goal of the last change; this one makes both surfaces look like products people would trust with money and with their production data, without changing the framework either is built on.

## What Changes

- A new workspace package, `@mako-cloud/ui`, holds one design system for every Mako web surface: Tailwind v4 design tokens (colour, spacing, radius, type scale) with light and dark palettes, shadcn/ui-style components built on Radix primitives (button, input, select, dialog, sheet, dropdown, tabs, table, card, badge, tooltip, toast, switch, progress), Lucide icons, the self-hosted Inter variable font, and chart components on Recharts.
- Rational is re-skinned screen by screen on the kit: shell and navigation, sign-in, dashboard, accounts and account detail, transactions and the transaction panel, budget, cash flow, recurring, goals, investments, settings and its pages. Its existing theme toggle drives the kit's dark palette. Every Playwright scenario keeps passing; the parity matrix does not change.
- The standalone Rational export stays self-contained: the kit's sources are vendored into the exported repository and its dependencies spelled out, so `github.com/shuaimu/rational` builds with `npm ci` as before.
- The console is re-skinned on the same kit, navigation shell first, then each destination. Its e2e suite keeps passing; the shell's destinations, context, and unavailable states are unchanged in behaviour.
- Hand-rolled SVG charts that Recharts covers (line, bar, donut, sparkline) are replaced; Sankey, treemap, and the month calendar stay hand-drawn and are restyled with the tokens.
- No framework change: React 19 and Vite 7 stay. No RxDB, model, function, or platform API change.

## Capabilities

### New Capabilities
- `cloud/design-system`: the shared kit — tokens with light and dark palettes, accessible components, icons, typography, charts — how an app adopts it, how a standalone export carries it, and the accessibility and consistency guarantees it makes.

### Modified Capabilities
- `samples/rational-money-app`: a new requirement that Rational is presented through the design system on every screen, that its theme preference drives the kit's palette, and that the exported standalone application builds without the workspace.
- `cloud/developer-console`: the navigation shell requirement gains the design system and a theme preference: every destination renders through the kit, and the shell honours the developer's light or dark choice.

## Impact

- New: `packages/ui` (source, tokens CSS, tests), `docs/design-system.md`, traceability rows for the new scenarios.
- Changed: `examples/rational/src/ui/*` and `src/ui/styles/*` (CSS largely deleted), `examples/rational/vite.config.ts`, `apps/console/src/*.tsx` and its CSS files, `apps/console/vite.config.ts`, `scripts/export-rational-app.mjs` (vendors the kit into the exported repository), root `package.json`/`package-lock.json`, `tsconfig` project references, `docs/README.md` index, `docs/rational.md`.
- Dependencies added: `tailwindcss` 4, `@tailwindcss/vite`, `radix-ui`, `class-variance-authority`, `clsx`, `tailwind-merge`, `lucide-react`, `recharts`, `@fontsource-variable/inter`. All are build-time or client bundle dependencies; nothing new runs on a server or on the beta host.
- Risk: the console ships inside the public-beta release, so its re-skin reaches the beta on the next upgrade and is qualified by the existing hosted and browser qualification suites. Rational's re-skin reaches the published site through the existing export.
