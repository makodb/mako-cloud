import {
  SANKEY_DEFICIT,
  SANKEY_HUB,
  SANKEY_SAVINGS,
  type SankeyGraph,
} from "../../selectors/cashflow.js";
import { formatMinorUnits } from "../../selectors/money.js";
import { paletteClass } from "./bars.jsx";
import { type SankeyLaidNode, sankeyLayout } from "./layout.js";

/**
 * Income into spending, drawn from the layout module's columns.
 *
 * The two outer columns keep a margin for their labels, so a category's name
 * never lies across a ribbon; the inner columns label to the right, over the
 * ribbons leaving them, as a Sankey usually does. Color follows the spending
 * group: a group, its categories, and the ribbons into and out of it share
 * one palette slot, income is the accent, savings is the positive color, and
 * a deficit the danger color.
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

function clip(text: string): string {
  return text.length > MAX_LABEL ? `${text.slice(0, MAX_LABEL - 1)}…` : text;
}

function fillClass(node: SankeyLaidNode): string {
  if (node.id === SANKEY_SAVINGS || node.kind === "savings") return "chart-fill-positive";
  if (node.id === SANKEY_DEFICIT) return "chart-fill-danger";
  if (node.groupIndex === null) return "chart-fill-accent";
  return paletteClass("chart-fill", node.groupIndex);
}

function strokeClass(node: SankeyLaidNode): string {
  return fillClass(node).replace("chart-fill", "chart-stroke");
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
  if (layout.links.length === 0) return <p className="muted chart-empty">{emptyMessage}</p>;
  const byId = new Map(layout.nodes.map((node) => [node.id, node]));
  const last = layout.columns - 1;

  return (
    <figure className="chart sankey">
      <svg
        viewBox={`0 0 ${WIDTH} ${height}`}
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label={ariaLabel}
      >
        <g transform={`translate(${LABEL_MARGIN} 0)`}>
          <g className="sankey-links">
            {layout.links.map((link) => {
              const source = byId.get(link.source);
              const target = byId.get(link.target);
              if (source === undefined || target === undefined) return null;
              // The ribbon takes the color of whichever end is not the hub.
              const colored = source.id === SANKEY_HUB ? target : source;
              return (
                <path
                  key={`${link.source}→${link.target}`}
                  className={`sankey-link ${strokeClass(colored)}`}
                  d={link.path}
                  strokeWidth={Math.max(1, link.thickness)}
                  data-source={link.source}
                  data-target={link.target}
                >
                  <title>{`${source.label} → ${target.label}: ${format(link.value)}`}</title>
                </path>
              );
            })}
          </g>
          <g className="sankey-nodes">
            {layout.nodes.map((node) => {
              if (node.height <= 0) return null;
              const labelLeft = node.column === 0 && layout.columns > 1;
              const labelX = labelLeft ? node.x - 8 : node.x + node.width + 8;
              const anchor = labelLeft ? "end" : "start";
              const roomForValue = node.height >= 30 || node.column === last || node.column === 0;
              const middle = node.y + node.height / 2;
              return (
                <g key={node.id} className="sankey-node" data-node={node.id}>
                  <rect
                    className={fillClass(node)}
                    x={node.x}
                    y={node.y}
                    width={node.width}
                    height={Math.max(1, node.height)}
                  >
                    <title>{`${node.label}: ${format(node.value)}`}</title>
                  </rect>
                  <text
                    className="sankey-label"
                    x={labelX}
                    y={roomForValue ? middle - 2 : middle}
                    textAnchor={anchor}
                    dominantBaseline={roomForValue ? "auto" : "middle"}
                  >
                    {clip(node.label)}
                  </text>
                  {roomForValue ? (
                    <text className="sankey-value" x={labelX} y={middle + 12} textAnchor={anchor}>
                      {format(node.value)}
                    </text>
                  ) : null}
                </g>
              );
            })}
          </g>
        </g>
      </svg>
      <table className="visually-hidden">
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
