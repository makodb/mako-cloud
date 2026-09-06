import { useState } from "react";

import { formatMinorUnits } from "../../selectors/money.js";
import {
  axisLabel,
  bandScale,
  compactMoney,
  isMonthKey,
  linearScale,
  monthTicks,
  niceTicks,
  sparseTicks,
} from "./layout.js";

/**
 * Vertical bars, grouped or stacked, one color per series from the palette.
 *
 * Negative values hang below a zero line rather than being clipped or made
 * positive: a month that netted a loss should look like one. In a stack the
 * positive segments pile up from zero and the negative ones pile down, so a
 * stack's height is its total either way.
 */

export interface BarValue {
  readonly series: string;
  /** Minor units; negative is drawn below the zero line. */
  readonly value: number;
}

export interface BarGroup {
  readonly key: string;
  readonly label: string;
  readonly values: readonly BarValue[];
}

export interface BarSeries {
  readonly key: string;
  readonly label: string;
  /** Which palette slot, counted from zero; the series' position by default. */
  readonly colorIndex?: number;
}

export interface BarChartProps {
  readonly groups: readonly BarGroup[];
  readonly series: readonly BarSeries[];
  readonly currency: string;
  readonly formatValue?: (value: number) => string;
  readonly stacked?: boolean;
  /** In viewBox units, of a 640-unit width. */
  readonly height?: number;
  readonly ariaLabel: string;
  readonly emptyMessage?: string;
}

const WIDTH = 640;
const MARGIN = { top: 22, right: 16, bottom: 26, left: 60 } as const;

/** The palette class for a slot; the eight colors repeat past the eighth series. */
export function paletteClass(
  prefix: "chart-fill" | "chart-stroke" | "chart-swatch",
  index: number,
): string {
  return `${prefix}-${(((index % 8) + 8) % 8) + 1}`;
}

interface Segment {
  readonly group: string;
  readonly series: string;
  readonly value: number;
  readonly x: number;
  readonly width: number;
  readonly from: number;
  readonly to: number;
}

export function BarChart({
  groups,
  series,
  currency,
  formatValue,
  stacked = false,
  height = 200,
  ariaLabel,
  emptyMessage = "Nothing to chart yet.",
}: BarChartProps) {
  const [hovered, setHovered] = useState<string | null>(null);
  const format = formatValue ?? ((value: number) => formatMinorUnits(value, currency));
  if (groups.length === 0 || series.length === 0) {
    return <p className="muted chart-empty">{emptyMessage}</p>;
  }

  const plot = {
    left: MARGIN.left,
    right: WIDTH - MARGIN.right,
    top: MARGIN.top,
    bottom: height - MARGIN.bottom,
  };
  const valueFor = (group: BarGroup, key: string): number =>
    group.values.find((value) => value.series === key)?.value ?? 0;

  let low = 0;
  let high = 0;
  for (const group of groups) {
    if (stacked) {
      let up = 0;
      let down = 0;
      for (const item of series) {
        const value = valueFor(group, item.key);
        if (value >= 0) up += value;
        else down += value;
      }
      high = Math.max(high, up);
      low = Math.min(low, down);
    } else {
      for (const item of series) {
        const value = valueFor(group, item.key);
        high = Math.max(high, value);
        low = Math.min(low, value);
      }
    }
  }
  const ticks = niceTicks(low, high);
  const first = ticks[0] ?? 0;
  const last = ticks[ticks.length - 1] ?? first;
  const y = linearScale(first === last ? [first - 1, first + 1] : [first, last], [
    plot.bottom,
    plot.top,
  ]);
  const zero = y(0);

  const keys = groups.map((group) => group.key);
  const bands = bandScale(keys, [plot.left, plot.right], groups.length > 24 ? 0.15 : 0.3);
  const inner = bandScale(
    series.map((item) => item.key),
    [0, bands.bandwidth],
    series.length === 1 ? 0 : 0.1,
  );

  const segments: Segment[] = [];
  for (const group of groups) {
    const x0 = bands.position(group.key);
    let up = 0;
    let down = 0;
    for (const item of series) {
      const value = valueFor(group, item.key);
      if (stacked) {
        const from = value >= 0 ? up : down;
        const to = from + value;
        if (value >= 0) up = to;
        else down = to;
        segments.push({
          group: group.key,
          series: item.key,
          value,
          x: x0,
          width: bands.bandwidth,
          from,
          to,
        });
      } else {
        segments.push({
          group: group.key,
          series: item.key,
          value,
          x: x0 + inner.position(item.key),
          width: inner.bandwidth,
          from: 0,
          to: value,
        });
      }
    }
  }

  const monthKeys = keys.every(isMonthKey);
  const labelled = new Set(monthKeys ? monthTicks(keys) : sparseTicks(keys, 8));
  const labelFor = (group: BarGroup, previous: string | undefined): string =>
    monthKeys ? axisLabel(group.key, previous) : group.label;

  const hoveredGroup = groups.find((group) => group.key === hovered);
  const hoveredX = hoveredGroup === undefined ? 0 : bands.position(hoveredGroup.key);
  const readoutOnLeft = hoveredX + bands.bandwidth / 2 > WIDTH / 2;
  const colorOf = (item: BarSeries, index: number): string =>
    paletteClass("chart-fill", item.colorIndex ?? index);

  return (
    <figure className="chart bar-chart">
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
          {groups.map((group, index) => {
            if (!labelled.has(group.key)) return null;
            const previous = keys.slice(0, index).findLast((key) => labelled.has(key));
            return (
              <text
                key={group.key}
                x={bands.position(group.key) + bands.bandwidth / 2}
                y={plot.bottom + 18}
                textAnchor="middle"
              >
                {labelFor(group, previous)}
              </text>
            );
          })}
        </g>
        <g className="chart-bars">
          {segments.map((segment) => {
            const item = series.findIndex((candidate) => candidate.key === segment.series);
            const top = Math.min(y(segment.from), y(segment.to));
            const bottom = Math.max(y(segment.from), y(segment.to));
            if (segment.value === 0) return null;
            return (
              <rect
                key={`${segment.group}:${segment.series}`}
                className={`chart-bar ${colorOf(series[item] ?? { key: "", label: "" }, item)}${
                  hovered !== null && hovered !== segment.group ? " chart-bar-dim" : ""
                }`}
                x={segment.x}
                y={top}
                width={segment.width}
                height={Math.max(0, bottom - top)}
                data-group={segment.group}
                data-series={segment.series}
              />
            );
          })}
        </g>
        {first < 0 && last > 0 ? (
          <line className="chart-zero" x1={plot.left} x2={plot.right} y1={zero} y2={zero} />
        ) : null}
        {hoveredGroup !== undefined ? (
          <g className="chart-readout" data-testid="chart-readout">
            <text
              x={hoveredX + (readoutOnLeft ? bands.bandwidth : 0)}
              y={12}
              textAnchor={readoutOnLeft ? "end" : "start"}
            >
              <tspan className="chart-readout-key">{hoveredGroup.label}</tspan>
              {series.map((item) => (
                <tspan key={item.key} className="chart-readout-value" dx="8">
                  {series.length > 1 ? `${item.label} ` : ""}
                  {format(valueFor(hoveredGroup, item.key))}
                </tspan>
              ))}
            </text>
          </g>
        ) : null}
        <g className="chart-hits">
          {groups.map((group) => (
            <rect
              key={group.key}
              className="chart-hit"
              x={bands.position(group.key) - (bands.step - bands.bandwidth) / 2}
              y={plot.top}
              width={bands.step}
              height={plot.bottom - plot.top}
              onPointerEnter={() => setHovered(group.key)}
              onPointerDown={() => setHovered(group.key)}
            >
              <title>
                {`${group.label}: ${series
                  .map((item) => `${item.label} ${format(valueFor(group, item.key))}`)
                  .join(", ")}`}
              </title>
            </rect>
          ))}
        </g>
      </svg>
      {series.length > 1 ? (
        <ul className="chart-legend">
          {series.map((item, index) => (
            <li key={item.key}>
              <span
                className={`chart-swatch ${paletteClass("chart-swatch", item.colorIndex ?? index)}`}
              />
              {item.label}
            </li>
          ))}
        </ul>
      ) : null}
      <table className="visually-hidden">
        <caption>{ariaLabel}</caption>
        <thead>
          <tr>
            <th scope="col">Group</th>
            {series.map((item) => (
              <th key={item.key} scope="col">
                {item.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {groups.map((group) => (
            <tr key={group.key}>
              <th scope="row">{group.label}</th>
              {series.map((item) => (
                <td key={item.key}>{format(valueFor(group, item.key))}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </figure>
  );
}
