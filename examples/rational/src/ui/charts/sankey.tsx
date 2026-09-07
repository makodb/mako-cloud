import { seriesColor } from "@mako-cloud/ui";

import {
  SANKEY_DEFICIT,
  SANKEY_HUB,
  SANKEY_SAVINGS,
  type SankeyGraph,
} from "../../selectors/cashflow.js";
import { formatMinorUnits } from "../../selectors/money.js";
import { type SankeyLaidNode, sankeyLayout } from "./layout.js";

/**
 * Income into spending, drawn from the layout module's columns.
 *
 * The two outer columns keep a margin for their labels, so a category's name
 * never lies across a ribbon; the inner columns label to the right, over the
 * ribbons leaving them, as a Sankey usually does. Labels yield to their
 * neighbours: a node with no room for two lines shows only its name, one
 * with no room for a line keeps only its tooltip, and the hidden table has
 * every figure regardless. Color follows the spending group: a group, its
 * categories, and the ribbons into and out of it share one of the design
 * system's series colours, income is the primary colour, savings the positive
 * colour, and a deficit the destructive one. Every colour is a token, so the
 * drawing follows the theme like the rest of the page.
 */

export interface SankeyProps {
  readonly graph: SankeyGraph;
  readonly currency: string;
  readonly formatValue?: (value: number) => string;
  /** In viewBox units, of a 720-unit width. */
  readonly height?: number;
  readonly ariaLabel: string;
  readonly emptyMessage?: string;
}

const WIDTH = 720;
const LABEL_MARGIN = 140;
const NODE_WIDTH = 14;
const MAX_LABEL = 22;
/** Two lines of text need this much between a node's centre and its neighbours'. */
const VALUE_ROOM = 26;
/** One line needs this much: its own size, at which neighbouring lines still clear each other. */
const LABEL_ROOM = 11;

function clip(text: string): string {
  return text.length > MAX_LABEL ? `${text.slice(0, MAX_LABEL - 1)}…` : text;
}

/** The token a node -- and the ribbons that take its colour -- is drawn in. */
function colorOf(node: SankeyLaidNode): string {
  if (node.id === SANKEY_SAVINGS || node.kind === "savings") return "var(--positive)";
  if (node.id === SANKEY_DEFICIT) return "var(--destructive)";
  if (node.groupIndex === null) return "var(--primary)";
  return seriesColor(node.groupIndex);
}

/**
 * How much vertical room each node's label has: the distance from its centre
 * to the nearer neighbour's centre in the same column, in viewBox units.
 */
function labelRoom(nodes: readonly SankeyLaidNode[]): ReadonlyMap<string, number> {
  const columns = new Map<number, SankeyLaidNode[]>();
  for (const node of nodes) {
    if (node.height <= 0) continue;
    const column = columns.get(node.column) ?? [];
    column.push(node);
    columns.set(node.column, column);
  }
  const room = new Map<string, number>();
  for (const column of columns.values()) {
    column.sort((left, right) => left.y - right.y);
    column.forEach((node, index) => {
      const middle = node.y + node.height / 2;
      const above = column[index - 1];
      const below = column[index + 1];
      const gapAbove =
        above === undefined ? Number.POSITIVE_INFINITY : middle - (above.y + above.height / 2);
      const gapBelow =
        below === undefined ? Number.POSITIVE_INFINITY : below.y + below.height / 2 - middle;
      room.set(node.id, Math.min(gapAbove, gapBelow));
    });
  }
  return room;
}

export function Sankey({
  graph,
  currency,
  formatValue,
  height = 360,
  ariaLabel,
  emptyMessage = "Nothing to chart yet.",
}: SankeyProps) {
  const format = formatValue ?? ((value: number) => formatMinorUnits(value, currency));
  const layout = sankeyLayout(graph, WIDTH - LABEL_MARGIN * 2, height, {
    nodeWidth: NODE_WIDTH,
    nodePadding: 10,
  });
  if (layout.links.length === 0) {
    return <p className="text-sm text-muted-foreground">{emptyMessage}</p>;
  }
  const byId = new Map(layout.nodes.map((node) => [node.id, node]));
  const room = labelRoom(layout.nodes);
  const last = layout.columns - 1;

  return (
    <figure className="m-0 w-full" data-slot="sankey">
      <svg
        className="block h-auto w-full overflow-visible"
        viewBox={`0 0 ${WIDTH} ${height}`}
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label={ariaLabel}
      >
        <g transform={`translate(${LABEL_MARGIN} 0)`}>
          <g>
            {layout.links.map((link) => {
              const source = byId.get(link.source);
              const target = byId.get(link.target);
              if (source === undefined || target === undefined) return null;
              // The ribbon takes the color of whichever end is not the hub.
              const colored = source.id === SANKEY_HUB ? target : source;
              return (
                <path
                  key={`${link.source}→${link.target}`}
                  className="fill-none transition-[stroke-opacity] [stroke-opacity:0.32] hover:[stroke-opacity:0.6]"
                  d={link.path}
                  stroke={colorOf(colored)}
                  strokeWidth={Math.max(1, link.thickness)}
                  data-source={link.source}
                  data-target={link.target}
                >
                  <title>{`${source.label} → ${target.label}: ${format(link.value)}`}</title>
                </path>
              );
            })}
          </g>
          <g>
            {layout.nodes.map((node) => {
              if (node.height <= 0) return null;
              const labelLeft = node.column === 0 && layout.columns > 1;
              const labelX = labelLeft ? node.x - 8 : node.x + node.width + 8;
              const anchor = labelLeft ? "end" : "start";
              const gap = room.get(node.id) ?? Number.POSITIVE_INFINITY;
              const outer = node.column === last || node.column === 0;
              // The outer columns say their amounts when their neighbours leave
              // room for a second line; a node too close to the next keeps its
              // name, and one crammed tighter still keeps only its tooltip.
              const roomForValue = node.height >= 30 || (outer && gap >= VALUE_ROOM);
              const roomForLabel = gap >= LABEL_ROOM;
              const middle = node.y + node.height / 2;
              return (
                <g key={node.id} data-node={node.id}>
                  <rect
                    fill={colorOf(node)}
                    stroke="var(--border)"
                    strokeWidth={1}
                    x={node.x}
                    y={node.y}
                    width={node.width}
                    height={Math.max(1, node.height)}
                  >
                    <title>{`${node.label}: ${format(node.value)}`}</title>
                  </rect>
                  {roomForLabel ? (
                    <text
                      className="text-[11px] font-medium tabular-nums"
                      fill="var(--foreground)"
                      x={labelX}
                      y={roomForValue ? middle - 2 : middle}
                      textAnchor={anchor}
                      dominantBaseline={roomForValue ? "auto" : "middle"}
                    >
                      {clip(node.label)}
                    </text>
                  ) : null}
                  {roomForValue ? (
                    <text
                      className="text-[10px] tabular-nums"
                      fill="var(--muted-foreground)"
                      x={labelX}
                      y={middle + 12}
                      textAnchor={anchor}
                    >
                      {format(node.value)}
                    </text>
                  ) : null}
                </g>
              );
            })}
          </g>
        </g>
      </svg>
      <table className="sr-only">
        <caption>{ariaLabel}</caption>
        <thead>
          <tr>
            <th scope="col">From</th>
            <th scope="col">To</th>
            <th scope="col">Amount</th>
          </tr>
        </thead>
        <tbody>
          {layout.links.map((link) => (
            <tr key={`${link.source}→${link.target}`}>
              <td>{byId.get(link.source)?.label ?? link.source}</td>
              <td>{byId.get(link.target)?.label ?? link.target}</td>
              <td>{format(link.value)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </figure>
  );
}
