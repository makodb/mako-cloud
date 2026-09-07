/**
 * The design system's tokens by name: what `styles.css` declares, so tests
 * and charts can name them without a second copy of their values. Each token
 * has a light and a dark value in the stylesheet; the pairs below are the
 * text-on-surface combinations that must stay readable in both.
 */

/** Every colour token the stylesheet declares on the document root. */
export const COLOR_TOKENS = [
  "background",
  "foreground",
  "card",
  "card-foreground",
  "popover",
  "popover-foreground",
  "primary",
  "primary-foreground",
  "secondary",
  "secondary-foreground",
  "muted",
  "muted-foreground",
  "accent",
  "accent-foreground",
  "destructive",
  "destructive-foreground",
  "positive",
  "positive-foreground",
  "warning",
  "warning-foreground",
  "border",
  "input",
  "ring",
  "chart-1",
  "chart-2",
  "chart-3",
  "chart-4",
  "chart-5",
  "chart-6",
  "chart-7",
  "chart-8",
  "sidebar",
  "sidebar-foreground",
  "sidebar-primary",
  "sidebar-primary-foreground",
  "sidebar-accent",
  "sidebar-accent-foreground",
  "sidebar-border",
  "sidebar-ring",
] as const;

export type ColorToken = (typeof COLOR_TOKENS)[number];

/**
 * Text tokens and the surface each is read against.
 *
 * The role colours are here twice over: once as a fill with their own
 * foreground on top, and once as the text they are also used for -- a
 * `text-destructive` message on a card, a `text-warning` count beside a rule.
 * Leaving out the second use is how a palette passes its own contrast test
 * while a screen fails it.
 */
export const READABLE_PAIRS: ReadonlyArray<readonly [text: ColorToken, surface: ColorToken]> = [
  ["foreground", "background"],
  ["card-foreground", "card"],
  ["popover-foreground", "popover"],
  ["primary-foreground", "primary"],
  ["secondary-foreground", "secondary"],
  ["muted-foreground", "muted"],
  ["muted-foreground", "background"],
  ["muted-foreground", "card"],
  ["accent-foreground", "accent"],
  ["destructive-foreground", "destructive"],
  ["positive-foreground", "positive"],
  ["warning-foreground", "warning"],
  ["sidebar-foreground", "sidebar"],
  ["sidebar-primary-foreground", "sidebar-primary"],
  ["sidebar-accent-foreground", "sidebar-accent"],
  // The same colours used as text rather than as a ground.
  ["primary", "background"],
  ["primary", "card"],
  ["primary", "muted"],
  ["destructive", "background"],
  ["destructive", "card"],
  ["destructive", "muted"],
  ["positive", "background"],
  ["positive", "card"],
  ["positive", "muted"],
  ["warning", "background"],
  ["warning", "card"],
  ["warning", "muted"],
  ["foreground", "muted"],
  ["foreground", "accent"],
  ["muted-foreground", "accent"],
];

/**
 * The focus ring against the grounds it is drawn on. A focus indicator is
 * held to 3:1, not 4.5:1, and it is the only thing marking a borderless
 * control, so it is drawn at full strength rather than tinted.
 */
export const FOCUS_PAIRS: ReadonlyArray<readonly [ring: ColorToken, surface: ColorToken]> = [
  ["ring", "background"],
  ["ring", "card"],
  ["ring", "muted"],
  ["ring", "sidebar"],
];

/** The series colours a chart hands out, in order. */
export const CHART_SERIES = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
  "var(--chart-6)",
  "var(--chart-7)",
  "var(--chart-8)",
] as const;

/** The colour for the `index`th series, wrapping around when there are more. */
export function seriesColor(index: number): string {
  return CHART_SERIES[
    ((index % CHART_SERIES.length) + CHART_SERIES.length) % CHART_SERIES.length
  ] as string;
}

/** The document-root attribute that pins a theme; absent, the device decides. */
export const THEME_ATTRIBUTE = "data-theme";
