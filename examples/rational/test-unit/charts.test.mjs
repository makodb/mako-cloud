import assert from "node:assert/strict";
import { test } from "node:test";

import {
  annulusPath,
  areaPathFor,
  axisLabel,
  bandScale,
  compactMoney,
  linearScale,
  monthTicks,
  niceTicks,
  pathFor,
  readoutLabel,
  sankeyLayout,
  shortMonthLabel,
  sparseTicks,
} from "../dist/src/ui/charts/layout.js";

const close = (actual, expected, message) =>
  assert.ok(Math.abs(actual - expected) < 1e-6, `${message}: ${actual} vs ${expected}`);

test("a linear scale maps the domain onto the range, and a flat domain onto the middle", () => {
  const scale = linearScale([0, 100], [200, 0]);
  assert.equal(scale(0), 200);
  assert.equal(scale(100), 0);
  assert.equal(scale(25), 150);
  assert.equal(scale(150), -100, "values outside the domain extrapolate rather than clamp");
  const flat = linearScale([7, 7], [10, 30]);
  assert.equal(flat(7), 20);
  assert.equal(flat(-1_000), 20);
});

test("nice ticks are on 1, 2, or 5 times a power of ten and cover the data", () => {
  assert.deepEqual(niceTicks(0, 100), [0, 20, 40, 60, 80, 100]);
  assert.deepEqual(niceTicks(0, 123_456), [0, 50_000, 100_000, 150_000]);
  assert.deepEqual(niceTicks(240_000, 260_000), [240_000, 245_000, 250_000, 255_000, 260_000]);
  assert.deepEqual(niceTicks(3, 17, 3), [0, 5, 10, 15, 20]);
  for (const [low, high] of [
    [0.001, 0.037],
    [7, 9],
    [-3, 4_000],
  ]) {
    const ticks = niceTicks(low, high);
    assert.ok(ticks.length >= 2, `${low}..${high} has at least two ticks`);
    assert.ok(ticks[0] <= low && ticks[ticks.length - 1] >= high, `${low}..${high} is covered`);
    const step = ticks[1] - ticks[0];
    const mantissa = step / 10 ** Math.floor(Math.log10(step));
    assert.ok(
      [1, 2, 5].some((nice) => Math.abs(mantissa - nice) < 1e-9),
      `${low}..${high} steps by ${step}`,
    );
  }
});

test("zero is a tick whenever the data straddles it, and a flat series still gets an axis", () => {
  assert.ok(niceTicks(-1_234, 5_678).includes(0));
  assert.deepEqual(niceTicks(-50, 50), [-60, -40, -20, 0, 20, 40, 60]);
  assert.deepEqual(niceTicks(250, 250), [0, 50, 100, 150, 200, 250], "one value spans to zero");
  assert.deepEqual(niceTicks(-80, -80), [-80, -60, -40, -20, 0]);
  assert.deepEqual(niceTicks(0, 0), [0]);
  assert.deepEqual(niceTicks(Number.NaN, 5), [0]);
  assert.deepEqual(niceTicks(0.5, 0.5), [0, 0.1, 0.2, 0.3, 0.4, 0.5], "no float garbage");
});

test("paths are M then L through the points, and an area closes to the baseline", () => {
  const points = [
    { x: 0, y: 10 },
    { x: 50, y: 2.345 },
    { x: 100, y: 7 },
  ];
  assert.equal(pathFor(points), "M 0 10 L 50 2.35 L 100 7");
  assert.equal(areaPathFor(points, 20), "M 0 20 L 0 10 L 50 2.35 L 100 7 L 100 20 Z");
  assert.equal(pathFor([]), "");
  assert.equal(areaPathFor([], 20), "");
  assert.equal(pathFor([{ x: -0.001, y: 0 }]), "M 0 0", "never a negative zero");
});

test("a band scale shares the range equally with padding between neighbours", () => {
  const bands = bandScale(["a", "b", "c", "d"], [100, 500], 0.25);
  assert.equal(bands.step, 100);
  assert.equal(bands.bandwidth, 75);
  assert.equal(bands.position("a"), 112.5);
  assert.equal(bands.position("d"), 412.5);
  assert.ok(Number.isNaN(bands.position("nope")), "an unknown key has no position");
  const none = bandScale([], [0, 100]);
  assert.equal(none.bandwidth, 0);
  const snug = bandScale(["only"], [0, 100], 0);
  assert.equal(snug.bandwidth, 100);
  assert.equal(snug.position("only"), 0);
});

test("month ticks thin out as the series lengthens", () => {
  const months = (count) =>
    Array.from({ length: count }, (unused, index) => {
      const year = 2020 + Math.floor(index / 12);
      const month = String((index % 12) + 1).padStart(2, "0");
      return `${year}-${month}`;
    });
  assert.equal(monthTicks(months(12)).length, 12);
  assert.equal(monthTicks(months(24)).length, 8);
  assert.deepEqual(monthTicks(months(24)).slice(0, 3), ["2020-01", "2020-04", "2020-07"]);
  assert.equal(monthTicks(months(36)).length, 12);
  assert.equal(monthTicks(months(48)).length, 8);
  assert.deepEqual(monthTicks([]), []);
});

test("sparse ticks keep the ends and spread the rest", () => {
  const keys = Array.from({ length: 31 }, (unused, index) => `d${index}`);
  const chosen = sparseTicks(keys, 4);
  assert.deepEqual(chosen, ["d0", "d10", "d20", "d30"]);
  assert.deepEqual(sparseTicks(["a", "b"], 6), ["a", "b"]);
  assert.deepEqual(sparseTicks(["a"], 6), ["a"]);
});

test("month labels say the year once, when it changes", () => {
  assert.equal(shortMonthLabel("2026-08"), "Aug");
  assert.equal(shortMonthLabel("2026-08", "2026-05"), "Aug");
  assert.equal(shortMonthLabel("2026-01", "2025-10"), "Jan 26");
  assert.equal(shortMonthLabel("not a month"), "not a month");
  assert.equal(axisLabel("2026-08-15"), "Aug 15");
  assert.equal(axisLabel("2026-01-03", "2025-12-27"), "Jan 3, 2026");
  assert.equal(axisLabel("2026-03", "2026-02"), "Mar");
  assert.equal(axisLabel("Q3"), "Q3");
  assert.equal(readoutLabel("2026-08"), "August 2026");
  assert.equal(readoutLabel("2026-08-05"), "Aug 5, 2026");
  assert.equal(readoutLabel("total"), "total");
});

test("axis money is compact and the full amount is left to the readout", () => {
  assert.equal(compactMoney(0, "USD"), "$0");
  assert.equal(compactMoney(1_234_500, "USD"), "$12.3K");
  assert.equal(compactMoney(-250_000_00, "USD"), "-$250K");
  assert.equal(compactMoney(150_000_000_00, "USD"), "$150M");
  assert.equal(compactMoney(123_456, "JPY"), "¥123.5K", "a zero-decimal currency is not divided");
  assert.equal(compactMoney(500, "USD"), "$5");
});

test("a ring segment is two arcs, and a whole ring is two segments", () => {
  const quarter = annulusPath(50, 50, 40, 30, 0, Math.PI / 2);
  assert.match(quarter, /^M 50 10 A 40 40 0 0 1 90 50 L 80 50 A 30 30 0 0 0 50 20 Z$/u);
  const big = annulusPath(50, 50, 40, 30, 0, Math.PI * 1.5);
  assert.match(big, /A 40 40 0 1 1 /u, "more than a half turn takes the large arc");
  const whole = annulusPath(50, 50, 40, 30, 0, Math.PI * 2);
  assert.equal(whole.split("M ").length - 1, 2, "a full turn is two subpaths");
  assert.equal(annulusPath(50, 50, 40, 30, 1, 1), "");
});

/** The graph `sankeyFlows` would build for one paycheck, two bills, and something left. */
const graph = {
  nodes: [
    { id: "income:salary", label: "Salary", kind: "income" },
    { id: "income", label: "Income", kind: "income" },
    { id: "group:home", label: "Home", kind: "group" },
    { id: "category:rent", label: "Rent", kind: "category" },
    { id: "category:power", label: "Power", kind: "category" },
    { id: "savings", label: "Savings", kind: "savings" },
  ],
  links: [
    { source: "income:salary", target: "income", value: 5_000 },
    { source: "income", target: "group:home", value: 3_000 },
    { source: "group:home", target: "category:rent", value: 2_000 },
    { source: "group:home", target: "category:power", value: 1_000 },
    { source: "income", target: "savings", value: 2_000 },
  ],
};

test("sankey columns run income, hub, groups, then categories and savings", () => {
  const layout = sankeyLayout(graph, 400, 300, { nodeWidth: 10, nodePadding: 10 });
  const node = (id) => layout.nodes.find((entry) => entry.id === id);
  assert.equal(layout.columns, 4);
  assert.equal(node("income:salary").column, 0);
  assert.equal(node("income").column, 1);
  assert.equal(node("group:home").column, 2);
  assert.equal(node("category:rent").column, 3);
  assert.equal(node("savings").column, 3);
  assert.equal(node("income:salary").x, 0);
  assert.equal(node("income").x, 130);
  assert.equal(node("savings").x, 390, "the last column ends at the right edge");
  assert.equal(node("group:home").groupIndex, 0);
  assert.equal(node("category:power").groupIndex, 0, "a category takes its group's color");
  assert.equal(node("income:salary").groupIndex, null);
});

test("sankey node heights share one scale, set by the fullest column, and stack centred", () => {
  const height = 300;
  const padding = 10;
  const layout = sankeyLayout(graph, 400, height, { nodeWidth: 10, nodePadding: padding });
  const node = (id) => layout.nodes.find((entry) => entry.id === id);
  // The right column holds 5 000 with two gaps, so it sets the scale.
  const scale = (height - 2 * padding) / 5_000;
  close(node("income").height, 5_000 * scale, "the hub is the whole income");
  close(node("income").y, padding, "the hub is centred");
  close(node("group:home").height, 3_000 * scale, "the group is what it spent");
  close(node("group:home").y, (height - 3_000 * scale) / 2, "a lone node is centred");
  close(node("category:rent").y, 0, "the full column starts at the top");
  close(node("category:power").y, 2_000 * scale + padding, "stacked with padding");
  close(node("savings").y + node("savings").height, height, "and ends at the bottom");
  for (const entry of layout.nodes) {
    assert.ok(Number.isFinite(entry.y) && Number.isFinite(entry.height), `${entry.id} is placed`);
    assert.equal(entry.width, 10);
  }
});

test("sankey ribbons accumulate within a node in the order of the other end", () => {
  const height = 300;
  const padding = 10;
  const layout = sankeyLayout(graph, 400, height, { nodeWidth: 10, nodePadding: padding });
  const scale = (height - 2 * padding) / 5_000;
  const link = (source, target) =>
    layout.links.find((entry) => entry.source === source && entry.target === target);
  const toGroup = link("income", "group:home");
  const toSavings = link("income", "savings");
  close(toGroup.thickness, 3_000 * scale, "thickness is the value at scale");
  close(toGroup.y0, padding + 1_500 * scale, "the first ribbon leaves the top of the hub");
  close(toSavings.y0, padding + 3_000 * scale + 1_000 * scale, "the next leaves below it");
  close(toSavings.y1, 3_000 * scale + 2 * padding + 1_000 * scale, "and lands on savings");
  const rent = link("group:home", "category:rent");
  const power = link("group:home", "category:power");
  close(rent.y1, 1_000 * scale, "rent arrives at the top of its node");
  close(power.y1, 2_000 * scale + padding + 500 * scale, "power at the middle of its own");
  // The hub's right edge is at 140 and the group column starts at 260; the bend is halfway.
  assert.match(toGroup.path, /^M 140 [\d.]+ C 200 [\d.]+ 200 [\d.]+ 260 [\d.]+$/u);
  for (const entry of layout.links) {
    assert.ok(!entry.path.includes("NaN"), `${entry.source}→${entry.target} has a path`);
  }
});

test("a sankey with a deficit stands it beside the income categories, and empty columns close", () => {
  const layout = sankeyLayout(
    {
      nodes: [
        { id: "income:salary", label: "Salary", kind: "income" },
        { id: "deficit", label: "Deficit", kind: "income" },
        { id: "income", label: "Income", kind: "income" },
        { id: "group:home", label: "Home", kind: "group" },
        { id: "category:rent", label: "Rent", kind: "category" },
      ],
      links: [
        { source: "income:salary", target: "income", value: 1_000 },
        { source: "deficit", target: "income", value: 500 },
        { source: "income", target: "group:home", value: 1_500 },
        { source: "group:home", target: "category:rent", value: 1_500 },
      ],
    },
    300,
    100,
  );
  const node = (id) => layout.nodes.find((entry) => entry.id === id);
  assert.equal(node("deficit").column, 0);
  assert.ok(node("deficit").y > node("income:salary").y, "after the income categories");
  assert.equal(layout.columns, 4);

  const noSpending = sankeyLayout(
    {
      nodes: [
        { id: "income:salary", label: "Salary", kind: "income" },
        { id: "income", label: "Income", kind: "income" },
        { id: "savings", label: "Savings", kind: "savings" },
      ],
      links: [
        { source: "income:salary", target: "income", value: 1_000 },
        { source: "income", target: "savings", value: 1_000 },
      ],
    },
    300,
    100,
    { nodeWidth: 10 },
  );
  assert.equal(noSpending.columns, 3, "the groups' and categories' column is not left empty");
  assert.equal(noSpending.nodes.find((entry) => entry.id === "savings").x, 290);
  assert.equal(noSpending.nodes.find((entry) => entry.id === "savings").height, 100);
});

test("a sankey ignores links to nothing and draws nothing for no flow", () => {
  const layout = sankeyLayout(
    {
      nodes: [{ id: "income", label: "Income", kind: "income" }],
      links: [
        { source: "income", target: "ghost", value: 10 },
        { source: "income", target: "income", value: 0 },
      ],
    },
    100,
    100,
  );
  assert.deepEqual(layout.links, []);
  assert.equal(layout.nodes[0].height, 0);
  assert.equal(layout.columns, 1);
  assert.equal(layout.nodes[0].x, 44, "a lone column sits in the middle");
});
