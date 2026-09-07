// The stylesheet's tokens follow the theme and stay readable in both of them.
//
// Every colour token is declared once with `light-dark(light, dark)`; this
// reads those pairs straight from the stylesheet, converts each side from
// oklch to sRGB, and checks the text-on-surface pairs the kit promises against
// the AA contrast ratio. The token list itself comes from the built module, so
// the stylesheet and the code cannot drift apart unnoticed.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  CHART_SERIES,
  COLOR_TOKENS,
  FOCUS_PAIRS,
  READABLE_PAIRS,
  seriesColor,
} from "../dist/tokens.js";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const stylesheet = readFileSync(resolve(root, "src/styles.css"), "utf8");

/** `--name: light-dark(a, b);` for every token on the document root. */
function tokenPairs() {
  const pairs = new Map();
  const pattern = /--([a-z0-9-]+):\s*light-dark\(\s*(oklch\([^)]*\))\s*,\s*(oklch\([^)]*\))\s*\);/g;
  for (const match of stylesheet.matchAll(pattern)) {
    pairs.set(match[1], { light: match[2], dark: match[3] });
  }
  return pairs;
}

/** oklch(L C H [/ alpha]) → { r, g, b, alpha } in sRGB 0..1. */
function oklchToSrgb(text) {
  const match = /oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+)(%?))?\s*\)/.exec(text);
  assert.ok(match, `not an oklch colour: ${text}`);
  const L = Number(match[1]);
  const C = Number(match[2]);
  const H = (Number(match[3]) * Math.PI) / 180;
  const alpha = match[4] === undefined ? 1 : Number(match[4]) / (match[5] === "%" ? 100 : 1);
  const a = C * Math.cos(H);
  const b = C * Math.sin(H);
  const l_ = L + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = L - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = L - 0.0894841775 * a - 1.291485548 * b;
  const l = l_ ** 3;
  const m = m_ ** 3;
  const s = s_ ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ].map((channel) => Math.min(1, Math.max(0, channel)));
  return { linear, alpha };
}

function relativeLuminance({ linear }) {
  const [r, g, b] = linear;
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(text, surface) {
  const a = relativeLuminance(text);
  const b = relativeLuminance(surface);
  const [lighter, darker] = a > b ? [a, b] : [b, a];
  return (lighter + 0.05) / (darker + 0.05);
}

test("every colour token is declared once with a light and a dark value", () => {
  const pairs = tokenPairs();
  for (const token of COLOR_TOKENS) {
    assert.ok(pairs.has(token), `token --${token} is missing from styles.css`);
  }
  for (const token of pairs.keys()) {
    assert.ok(COLOR_TOKENS.includes(token), `stylesheet declares --${token}, which tokens.ts does not name`);
  }
  // Each side is a colour the converter understands.
  for (const [token, { light, dark }] of pairs) {
    oklchToSrgb(light);
    oklchToSrgb(dark);
    assert.notEqual(light, dark, `--${token} has the same value in both themes`);
  }
});

test("text stays readable on its surface in both themes (AA, 4.5:1)", () => {
  const pairs = tokenPairs();
  for (const [text, surface] of READABLE_PAIRS) {
    for (const theme of ["light", "dark"]) {
      const foreground = oklchToSrgb(pairs.get(text)[theme]);
      const background = oklchToSrgb(pairs.get(surface)[theme]);
      assert.equal(foreground.alpha, 1, `--${text} must be opaque`);
      assert.equal(background.alpha, 1, `--${surface} must be opaque`);
      const ratio = contrast(foreground, background);
      assert.ok(
        ratio >= 4.5,
        `${theme}: --${text} on --${surface} is ${ratio.toFixed(2)}:1, under 4.5:1`,
      );
    }
  }
});

test("the focus ring stands out from every ground it is drawn on (3:1)", () => {
  const pairs = tokenPairs();
  for (const [ring, surface] of FOCUS_PAIRS) {
    for (const theme of ["light", "dark"]) {
      const ratio = contrast(oklchToSrgb(pairs.get(ring)[theme]), oklchToSrgb(pairs.get(surface)[theme]));
      assert.ok(
        ratio >= 3,
        `${theme}: --${ring} on --${surface} is ${ratio.toFixed(2)}:1, under 3:1`,
      );
    }
  }
});

test("the theme follows the root attribute and, without one, the device", () => {
  assert.match(stylesheet, /:root\s*\{[^}]*color-scheme:\s*light dark/);
  assert.match(stylesheet, /:root\[data-theme="dark"\]\s*\{\s*color-scheme:\s*dark;?\s*\}/);
  assert.match(stylesheet, /:root\[data-theme="light"\]\s*\{\s*color-scheme:\s*light;?\s*\}/);
});

test("one import brings the framework, the typeface, the kit's own classes, and the base", () => {
  assert.match(stylesheet, /@import "tailwindcss";/);
  assert.match(stylesheet, /@import "@fontsource-variable\/inter";/);
  assert.match(stylesheet, /@source "\.\/";/);
  assert.match(stylesheet, /--font-sans:\s*"Inter Variable"/);
  assert.match(stylesheet, /@layer base/);
  assert.match(stylesheet, /prefers-reduced-motion: reduce/);
  for (const token of COLOR_TOKENS) {
    assert.match(
      stylesheet,
      new RegExp(`--color-${token}:\\s*var\\(--${token}\\);`),
      `--${token} is not mapped into the Tailwind theme`,
    );
  }
});

test("series colours are handed out in order and wrap", () => {
  assert.equal(CHART_SERIES.length, 8);
  assert.equal(seriesColor(0), "var(--chart-1)");
  assert.equal(seriesColor(7), "var(--chart-8)");
  assert.equal(seriesColor(8), "var(--chart-1)");
  assert.equal(seriesColor(-1), "var(--chart-8)");
});
