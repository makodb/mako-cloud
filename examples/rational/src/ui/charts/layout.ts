import {
  SANKEY_HUB,
  type SankeyGraph,
  type SankeyNode,
  type SankeyNodeKind,
} from "../../selectors/cashflow.js";
import { minorUnitDigits } from "../../selectors/money.js";

/**
 * The geometry under every chart, with no DOM in it.
 *
 * A chart component is two things: a layout, which turns numbers into
 * coordinates, and a drawing, which turns coordinates into SVG. Only the first
 * can be asserted from a test without a browser, so it lives here, alone, and
 * the components in this directory do nothing but draw what these functions
 * return. Everything is deterministic -- there is no clock and no randomness
 * -- so a chart of the same numbers is the same picture, which is what lets
 * the browser suite snapshot one.
 *
 * Coordinates are in a chart's viewBox units. Numbers written into path
 * strings are rounded to two decimals so the strings stay short and stable.
 */

export interface Point {
  readonly x: number;
  readonly y: number;
}

/** A path number: short, stable, and never `-0`. */
function fixed(value: number): string {
  const rounded = Math.round(value * 100) / 100;
  return String(rounded === 0 ? 0 : rounded);
}

/**
 * A linear map from `domain` onto `range`. A domain with no span -- one point,
 * or every value the same -- maps everything to the middle of the range, so a
 * flat series draws as a flat line through the chart rather than as nothing.
 */
export function linearScale(
  domain: readonly [number, number],
  range: readonly [number, number],
): (value: number) => number {
  const [d0, d1] = domain;
  const [r0, r1] = range;
  const span = d1 - d0;
  if (span === 0 || !Number.isFinite(span)) {
    const middle = (r0 + r1) / 2;
    return () => middle;
  }
  const ratio = (r1 - r0) / span;
  return (value) => r0 + (value - d0) * ratio;
}

/**
 * Round tick values covering `[min, max]`, about `count` of them, on steps of
 * 1, 2, or 5 times a power of ten. The first tick is at or below the minimum
 * and the last at or above the maximum, so the ticks are also the axis
 * domain; when the data straddles zero, zero is a tick, because every tick is
 * a multiple of the step. A span of nothing widens to include zero, so a
 * single value still gets an axis to sit on.
 */
export function niceTicks(min: number, max: number, count = 5): number[] {
  if (!Number.isFinite(min) || !Number.isFinite(max)) return [0];
  let low = Math.min(min, max);
  let high = Math.max(min, max);
  if (low === high) {
    if (low === 0) return [0];
    low = Math.min(0, low);
    high = Math.max(0, high);
  }
  const raw = (high - low) / Math.max(1, count);
  const exponent = Math.floor(Math.log10(raw));
  const magnitude = 10 ** exponent;
  const residual = raw / magnitude;
  const factor = residual <= 1 ? 1 : residual <= 2 ? 2 : residual <= 5 ? 5 : 10;
  const step = factor * magnitude;
  const decimals = Math.max(0, -exponent + (factor === 10 ? -1 : 0));
  const first = Math.floor(low / step);
  const last = Math.ceil(high / step);
  const ticks: number[] = [];
  for (let index = first; index <= last && ticks.length < 1_000; index += 1) {
    ticks.push(Number((index * step).toFixed(decimals)));
  }
  return ticks;
}

/** `M x y L x y …` through the points in order; empty for no points. */
export function pathFor(points: readonly Point[]): string {
  return points
    .map((point, index) => `${index === 0 ? "M" : "L"} ${fixed(point.x)} ${fixed(point.y)}`)
    .join(" ");
}

/** The line's path closed down to a baseline and back, for a soft fill under it. */
export function areaPathFor(points: readonly Point[], baselineY: number): string {
  const first = points[0];
  const last = points[points.length - 1];
  if (first === undefined || last === undefined) return "";
  return `M ${fixed(first.x)} ${fixed(baselineY)} ${points
    .map((point) => `L ${fixed(point.x)} ${fixed(point.y)}`)
    .join(" ")} L ${fixed(last.x)} ${fixed(baselineY)} Z`;
}

export interface BandScale {
  /** The width each key gets to draw in. */
  readonly bandwidth: number;
  /** The distance from one key's start to the next's. */
  readonly step: number;
  /** Where a key's band starts; NaN for a key that was not in the list. */
  position(key: string): number;
}

/**
 * Equal bands across a range, one per key, with `paddingRatio` of each step
 * left empty between neighbours. This is the bar chart's x axis.
 */
export function bandScale(
  keys: readonly string[],
  range: readonly [number, number],
  paddingRatio = 0.2,
): BandScale {
  const [r0, r1] = range;
  const step = keys.length === 0 ? 0 : (r1 - r0) / keys.length;
  const padding = Math.min(Math.max(paddingRatio, 0), 0.95);
  const bandwidth = step * (1 - padding);
  const inset = (step - bandwidth) / 2;
  const index = new Map(keys.map((key, position) => [key, position]));
  return {
    bandwidth,
    step,
    position(key) {
      const at = index.get(key);
      return at === undefined ? Number.NaN : r0 + at * step + inset;
    },
  };
}

/**
 * Which keys deserve a label when there are many: at most `max`, spread
 * evenly by position, the first and the last always among them.
 */
export function sparseTicks(keys: readonly string[], max = 6): string[] {
  if (keys.length <= max) return [...keys];
  const slots = Math.max(2, max);
  const chosen: string[] = [];
  for (let slot = 0; slot < slots; slot += 1) {
    const key = keys[Math.round((slot * (keys.length - 1)) / (slots - 1))];
    if (key !== undefined && chosen[chosen.length - 1] !== key) chosen.push(key);
  }
  return chosen;
}

/**
 * Which months of a series get an axis label: every one up to a year, every
 * third up to three years, every sixth beyond that, counted from the first.
 */
export function monthTicks(months: readonly string[]): string[] {
  const every = months.length <= 12 ? 1 : months.length <= 36 ? 3 : 6;
  return months.filter((_month, index) => index % every === 0);
}

const SHORT_MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

const LONG_MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

const MONTH_KEY = /^(\d{4})-(0[1-9]|1[0-2])$/u;
const DATE_KEY = /^(\d{4})-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$/u;

export function isMonthKey(key: string): boolean {
  return MONTH_KEY.test(key);
}

export function isDateKey(key: string): boolean {
  return DATE_KEY.test(key);
}

/**
 * `2026-08` as an axis label: `Aug`, or `Aug 26` when the previous label was
 * in another year, so a series crossing a year boundary says so once rather
 * than on every tick. Anything that is not a month is returned as it came.
 */
export function shortMonthLabel(month: string, previous?: string): string {
  const match = MONTH_KEY.exec(month);
  if (match === null) return month;
  const [, year = "", index = "01"] = match;
  const name = SHORT_MONTHS[Number(index) - 1] ?? index;
  const yearChanged = previous !== undefined && previous.slice(0, 4) !== year;
  return yearChanged ? `${name} ${year.slice(2)}` : name;
}

/**
 * An axis label for whatever kind of key a series uses: months as above, ISO
 * dates as `Aug 15` (with the year once when it changes), anything else as
 * itself.
 */
export function axisLabel(key: string, previous?: string): string {
  if (MONTH_KEY.test(key)) return shortMonthLabel(key, previous);
  const match = DATE_KEY.exec(key);
  if (match === null) return key;
  const [, year = "", month = "01", day = "01"] = match;
  const name = SHORT_MONTHS[Number(month) - 1] ?? month;
  const yearChanged = previous !== undefined && previous.slice(0, 4) !== year;
  return yearChanged ? `${name} ${Number(day)}, ${year}` : `${name} ${Number(day)}`;
}

/** The unabbreviated form of a key for a hover readout: `August 2026`, `Aug 15, 2026`. */
export function readoutLabel(key: string): string {
  const month = MONTH_KEY.exec(key);
  if (month !== null) {
    const [, year = "", index = "01"] = month;
    return `${LONG_MONTHS[Number(index) - 1] ?? index} ${year}`;
  }
  const date = DATE_KEY.exec(key);
  if (date !== null) {
    const [, year = "", index = "01", day = "01"] = date;
    return `${SHORT_MONTHS[Number(index) - 1] ?? index} ${Number(day)}, ${year}`;
  }
  return key;
}

/**
 * Minor units as an axis tick: `$12.3K`, `-$1.2M`, `$0`. Ticks have to be
 * short, and a tick's precision is the step's, so the compact notation loses
 * nothing a tick needs to say. The full amount is on the readout.
 */
export function compactMoney(minor: number, currency: string): string {
  const major = minor / 10 ** minorUnitDigits(currency);
  try {
    return new Intl.NumberFormat("en-US", {
      style: "currency",
      currency,
      notation: "compact",
      maximumFractionDigits: 1,
    }).format(major);
  } catch {
    return `${Math.round(major)} ${currency}`;
  }
}

function polar(cx: number, cy: number, radius: number, angle: number): Point {
  return { x: cx + radius * Math.sin(angle), y: cy - radius * Math.cos(angle) };
}

/**
 * A ring segment from `startAngle` to `endAngle`, radians clockwise from
 * twelve o'clock, between two radii. A whole ring cannot be one arc -- an arc
 * from a point back to itself has no length -- so a full turn is two halves.
 */
export function annulusPath(
  cx: number,
  cy: number,
  outer: number,
  inner: number,
  startAngle: number,
  endAngle: number,
): string {
  const sweep = endAngle - startAngle;
  if (sweep <= 0) return "";
  if (sweep >= Math.PI * 2 - 1e-9) {
    const middle = startAngle + Math.PI;
    return `${annulusPath(cx, cy, outer, inner, startAngle, middle)} ${annulusPath(
      cx,
      cy,
      outer,
      inner,
      middle,
      startAngle + Math.PI * 2,
    )}`;
  }
  const large = sweep > Math.PI ? 1 : 0;
  const o0 = polar(cx, cy, outer, startAngle);
  const o1 = polar(cx, cy, outer, endAngle);
  const i0 = polar(cx, cy, inner, endAngle);
  const i1 = polar(cx, cy, inner, startAngle);
  return [
    `M ${fixed(o0.x)} ${fixed(o0.y)}`,
    `A ${fixed(outer)} ${fixed(outer)} 0 ${large} 1 ${fixed(o1.x)} ${fixed(o1.y)}`,
    `L ${fixed(i0.x)} ${fixed(i0.y)}`,
    `A ${fixed(inner)} ${fixed(inner)} 0 ${large} 0 ${fixed(i1.x)} ${fixed(i1.y)}`,
    "Z",
  ].join(" ");
}

/* --- Sankey ------------------------------------------------------------ */

export interface SankeyLayoutOptions {
  readonly nodeWidth?: number;
  readonly nodePadding?: number;
}

export interface SankeyLaidNode {
  readonly id: string;
  readonly label: string;
  readonly kind: SankeyNodeKind;
  /** 0 at the left; columns nothing occupies are closed up. */
  readonly column: number;
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
  /** The larger of what flows in and what flows out. */
  readonly value: number;
  /**
   * Which spending group colors the node: a group's own position among the
   * groups, a category's parent's, null on the income side.
   */
  readonly groupIndex: number | null;
}

export interface SankeyLaidLink {
  readonly source: string;
  readonly target: string;
  readonly value: number;
  /** A cubic ribbon between the node edges; draw it with `stroke-width` of `thickness`. */
  readonly path: string;
  readonly thickness: number;
  /** The ribbon's centre where it leaves the source. */
  readonly y0: number;
  /** The ribbon's centre where it reaches the target. */
  readonly y1: number;
}

export interface SankeyLayout {
  readonly nodes: readonly SankeyLaidNode[];
  readonly links: readonly SankeyLaidLink[];
  /** How many columns the picture has after empty ones close up. */
  readonly columns: number;
}

/**
 * The column a node belongs to before empty columns close up: what supplies
 * the hub on the left, the hub, the groups, and what the groups feed on the
 * right. A deficit supplies the hub as an income category does, so it stands
 * with them; savings is fed by the hub but stands with the categories so the
 * right edge is everything the income became.
 */
function sankeyColumn(node: SankeyNode): number {
  if (node.id === SANKEY_HUB) return 1;
  switch (node.kind) {
    case "income":
      return 0;
    case "group":
      return 2;
    case "category":
    case "savings":
      return 3;
  }
}

/**
 * The classic Sankey: nodes in columns, each as tall as the flow through it,
 * stacked in the graph's order and centred; ribbons leaving a node in the
 * order of their targets, arriving in the order of their sources, so they
 * cross as little as the graph allows. One scale serves every column, set by
 * the fullest, so a unit of money is the same thickness everywhere.
 */
export function sankeyLayout(
  graph: SankeyGraph,
  width: number,
  height: number,
  options: SankeyLayoutOptions = {},
): SankeyLayout {
  const nodeWidth = options.nodeWidth ?? 12;
  const nodePadding = options.nodePadding ?? 8;
  const byId = new Map(graph.nodes.map((node) => [node.id, node]));
  const links = graph.links.filter(
    (link) =>
      Number.isFinite(link.value) &&
      link.value > 0 &&
      byId.has(link.source) &&
      byId.has(link.target),
  );

  const inflow = new Map<string, number>();
  const outflow = new Map<string, number>();
  for (const link of links) {
    outflow.set(link.source, (outflow.get(link.source) ?? 0) + link.value);
    inflow.set(link.target, (inflow.get(link.target) ?? 0) + link.value);
  }
  const flowOf = (id: string): number => Math.max(inflow.get(id) ?? 0, outflow.get(id) ?? 0);

  const occupied = [...new Set(graph.nodes.map(sankeyColumn))].sort((a, b) => a - b);
  const columnIndex = new Map(occupied.map((column, index) => [column, index]));
  const columns = occupied.length;
  const columnGap = columns <= 1 ? 0 : (width - nodeWidth) / (columns - 1);

  const perColumn = new Map<number, SankeyNode[]>();
  for (const node of graph.nodes) {
    const column = columnIndex.get(sankeyColumn(node)) ?? 0;
    const members = perColumn.get(column) ?? [];
    members.push(node);
    perColumn.set(column, members);
  }

  // The padding a column can afford: never more than half its height in gaps.
  const paddingFor = (count: number): number =>
    count <= 1 ? 0 : Math.min(nodePadding, height / (2 * (count - 1)));
  let scale = Number.POSITIVE_INFINITY;
  for (const members of perColumn.values()) {
    const total = members.reduce((sum, node) => sum + flowOf(node.id), 0);
    if (total <= 0) continue;
    const available = height - paddingFor(members.length) * (members.length - 1);
    scale = Math.min(scale, available / total);
  }
  if (!Number.isFinite(scale)) scale = 0;

  const groupOrder = graph.nodes.filter((node) => node.kind === "group").map((node) => node.id);
  const groupIndexOf = (node: SankeyNode): number | null => {
    if (node.kind === "group") return groupOrder.indexOf(node.id);
    if (node.kind === "category") {
      const parent = links.find((link) => link.target === node.id)?.source;
      return parent === undefined ? null : Math.max(0, groupOrder.indexOf(parent));
    }
    return null;
  };

  const laid = new Map<string, SankeyLaidNode>();
  const order = new Map<string, number>();
  for (const [column, members] of perColumn) {
    const padding = paddingFor(members.length);
    const heights = members.map((node) => flowOf(node.id) * scale);
    const stack = heights.reduce((sum, h) => sum + h, 0) + padding * (members.length - 1);
    let y = (height - stack) / 2;
    members.forEach((node, index) => {
      const nodeHeight = heights[index] ?? 0;
      laid.set(node.id, {
        id: node.id,
        label: node.label,
        kind: node.kind,
        column,
        x: columns <= 1 ? (width - nodeWidth) / 2 : column * columnGap,
        y,
        width: nodeWidth,
        height: nodeHeight,
        value: flowOf(node.id),
        groupIndex: groupIndexOf(node),
      });
      order.set(node.id, column * 1_000_000 + y);
      y += nodeHeight + padding;
    });
  }

  // Ribbons leave in the order of their targets and arrive in the order of
  // their sources; a link's offset within a node is the sum of the ones before.
  const position = (id: string): number => order.get(id) ?? 0;
  const outOffset = new Map<string, number>();
  const inOffset = new Map<string, number>();
  const sourceOrder = [...links].sort(
    (a, b) => position(a.source) - position(b.source) || position(a.target) - position(b.target),
  );
  const startAt = new Map<SankeyGraph["links"][number], number>();
  for (const link of sourceOrder) {
    const offset = outOffset.get(link.source) ?? 0;
    startAt.set(link, offset);
    outOffset.set(link.source, offset + link.value * scale);
  }
  const targetOrder = [...links].sort(
    (a, b) => position(a.target) - position(b.target) || position(a.source) - position(b.source),
  );
  const endAt = new Map<SankeyGraph["links"][number], number>();
  for (const link of targetOrder) {
    const offset = inOffset.get(link.target) ?? 0;
    endAt.set(link, offset);
    inOffset.set(link.target, offset + link.value * scale);
  }

  const laidLinks: SankeyLaidLink[] = [];
  for (const link of links) {
    const source = laid.get(link.source);
    const target = laid.get(link.target);
    if (source === undefined || target === undefined) continue;
    const thickness = link.value * scale;
    const y0 = source.y + (startAt.get(link) ?? 0) + thickness / 2;
    const y1 = target.y + (endAt.get(link) ?? 0) + thickness / 2;
    const x0 = source.x + source.width;
    const x1 = target.x;
    const bend = (x0 + x1) / 2;
    laidLinks.push({
      source: link.source,
      target: link.target,
      value: link.value,
      thickness,
      y0,
      y1,
      path: `M ${fixed(x0)} ${fixed(y0)} C ${fixed(bend)} ${fixed(y0)} ${fixed(bend)} ${fixed(
        y1,
      )} ${fixed(x1)} ${fixed(y1)}`,
    });
  }

  return {
    nodes: graph.nodes.map((node) => laid.get(node.id)).filter((node) => node !== undefined),
    links: laidLinks,
    columns,
  };
}
