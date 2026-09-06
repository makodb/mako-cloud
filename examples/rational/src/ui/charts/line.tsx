import { type PointerEvent, useState } from "react";

import { formatMinorUnits } from "../../selectors/money.js";
import {
  areaPathFor,
  axisLabel,
  compactMoney,
  isMonthKey,
  linearScale,
  monthTicks,
  niceTicks,
  pathFor,
  readoutLabel,
  sparseTicks,
} from "./layout.js";

/**
 * A line over time with axes, and the same line bare for a tile.
 *
 * The chart is drawn in a fixed viewBox and scaled to its container by the
 * browser, so the geometry is decided once here and never measured. What
 * cannot be decided without measuring -- how wide a label is -- is avoided:
 * labels are sparse enough to never touch, and the hover readout wears a halo
 * of the surface color rather than a box sized to fit it.
 */

export interface LinePoint {
  /** An ISO date or a month; the label is derived from its shape. */
  readonly x: string;
  /** Minor units. */
  readonly y: number;
}

export interface LineChartProps {
  readonly points: readonly LinePoint[];
  readonly currency: string;
  /** How a value reads on the readout; the currency's full form by default. */
  readonly formatValue?: (value: number) => string;
  /** In viewBox units, of a 640-unit width. */
  readonly height?: number;
  readonly showArea?: boolean;
  readonly ariaLabel: string;
  /** Names the series in the readout and the hidden table. */
  readonly label?: string;
  readonly emptyMessage?: string;
}

const WIDTH = 640;
const MARGIN = { top: 22, right: 16, bottom: 26, left: 60 } as const;

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}

export function LineChart({
  points,
  currency,
  formatValue,
  height = 160,
  showArea = false,
  ariaLabel,
  label,
  emptyMessage = "Nothing to chart yet.",
}: LineChartProps) {
  const [hovered, setHovered] = useState<number | null>(null);
  const format = formatValue ?? ((value: number) => formatMinorUnits(value, currency));
  if (points.length === 0) return <p className="muted chart-empty">{emptyMessage}</p>;

  const plot = {
    left: MARGIN.left,
    right: WIDTH - MARGIN.right,
    top: MARGIN.top,
    bottom: height - MARGIN.bottom,
  };
  const values = points.map((point) => point.y);
  const ticks = niceTicks(Math.min(...values), Math.max(...values));
  const first = ticks[0] ?? 0;
  const last = ticks[ticks.length - 1] ?? first;
  const yDomain: [number, number] = first === last ? [first - 1, first + 1] : [first, last];
  const y = linearScale(yDomain, [plot.bottom, plot.top]);
  const x = linearScale([0, points.length - 1], [plot.left, plot.right]);
  const positioned =
    points.length === 1
      ? [
          { x: plot.left, y: y(points[0]?.y ?? 0) },
          { x: plot.right, y: y(points[0]?.y ?? 0) },
        ]
      : points.map((point, index) => ({ x: x(index), y: y(point.y) }));

  const keys = points.map((point) => point.x);
  const labelled = new Set(
    keys.every(isMonthKey) && keys.length <= 36 ? monthTicks(keys) : sparseTicks(keys, 6),
  );
  const baseline = y(clamp(0, yDomain[0], yDomain[1]));

  const hoveredPoint = hovered === null ? undefined : points[hovered];
  const hoveredX = hovered === null ? 0 : points.length === 1 ? WIDTH / 2 : x(hovered);
  const readoutOnLeft = hoveredX > WIDTH / 2;

  const locate = (event: PointerEvent<SVGRectElement>): void => {
    const svg = event.currentTarget.ownerSVGElement;
    if (svg === null) return;
    const bounds = svg.getBoundingClientRect();
    if (bounds.width === 0) return;
    const unit = ((event.clientX - bounds.left) / bounds.width) * WIDTH;
    const ratio = (unit - plot.left) / (plot.right - plot.left);
    setHovered(clamp(Math.round(ratio * (points.length - 1)), 0, points.length - 1));
  };

  return (
    <figure className="chart line-chart">
      <svg
        viewBox={`0 0 ${WIDTH} ${height}`}
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label={ariaLabel}
        onPointerLeave={() => setHovered(null)}
      >
        <g className="chart-axis chart-axis-y">
          {ticks.map((tick) => (
            <g key={tick}>
              <line
                className={tick === 0 ? "chart-zero" : "chart-grid"}
                x1={plot.left}
                x2={plot.right}
                y1={y(tick)}
                y2={y(tick)}
              />
              <text x={plot.left - 8} y={y(tick)} textAnchor="end" dominantBaseline="middle">
                {compactMoney(tick, currency)}
              </text>
            </g>
          ))}
        </g>
        <g className="chart-axis chart-axis-x">
          {keys.map((key, index) => {
            if (!labelled.has(key)) return null;
            const previousLabelled = keys.slice(0, index).findLast((k) => labelled.has(k));
            return (
              <text
                key={key}
                x={points.length === 1 ? WIDTH / 2 : x(index)}
                y={plot.bottom + 18}
                textAnchor="middle"
              >
                {axisLabel(key, previousLabelled)}
              </text>
            );
          })}
        </g>
        {showArea ? <path className="chart-area" d={areaPathFor(positioned, baseline)} /> : null}
        <path className="chart-line" d={pathFor(positioned)} />
        {hoveredPoint !== undefined ? (
          <g className="chart-readout" data-testid="chart-readout">
            <line
              className="chart-guide"
              x1={hoveredX}
              x2={hoveredX}
              y1={plot.top}
              y2={plot.bottom}
            />
            <circle className="chart-marker" cx={hoveredX} cy={y(hoveredPoint.y)} r={4} />
            <text
              x={hoveredX + (readoutOnLeft ? -8 : 8)}
              y={12}
              textAnchor={readoutOnLeft ? "end" : "start"}
            >
              <tspan className="chart-readout-key">{readoutLabel(hoveredPoint.x)}</tspan>
              <tspan className="chart-readout-value" dx="8">
                {format(hoveredPoint.y)}
              </tspan>
            </text>
          </g>
        ) : null}
        <rect
          className="chart-hit"
          x={plot.left}
          y={plot.top}
          width={plot.right - plot.left}
          height={plot.bottom - plot.top}
          onPointerMove={locate}
          onPointerDown={locate}
        />
      </svg>
      <table className="visually-hidden">
        <caption>{label ?? ariaLabel}</caption>
        <tbody>
          {points.map((point) => (
            <tr key={point.x}>
              <th scope="row">{readoutLabel(point.x)}</th>
              <td>{format(point.y)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </figure>
  );
}

export interface SparklineProps {
  readonly points: readonly LinePoint[];
  /** In viewBox units, of a 100-unit width. */
  readonly height?: number;
  /** What the line is of and where it ends up; the tile's number alone does not say. */
  readonly ariaLabel: string;
}

/**
 * The line alone, stretched to fill whatever box it is given; the stroke
 * keeps its width under the stretch. Nothing at all for no points, since a
 * tile with no history should not show an empty frame.
 */
export function Sparkline({ points, height = 32, ariaLabel }: SparklineProps) {
  if (points.length === 0) return null;
  const values = points.map((point) => point.y);
  const low = Math.min(...values);
  const high = Math.max(...values);
  const pad = 3;
  const y = linearScale([low, high], [height - pad, pad]);
  const x = linearScale([0, points.length - 1], [0, 100]);
  const positioned =
    points.length === 1
      ? [
          { x: 0, y: height / 2 },
          { x: 100, y: height / 2 },
        ]
      : points.map((point, index) => ({ x: x(index), y: y(point.y) }));
  return (
    <svg
      className="chart-sparkline"
      viewBox={`0 0 100 ${height}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={ariaLabel}
    >
      <path className="chart-line" d={pathFor(positioned)} />
    </svg>
  );
}
