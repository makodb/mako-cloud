import { formatMinorUnits } from "../../selectors/money.js";
import { paletteClass } from "./bars.jsx";
import { annulusPath } from "./layout.js";

/**
 * A ring of shares with the legend that makes it readable.
 *
 * The ring shows proportion at a glance and nothing else; the legend carries
 * every number, since a slice's angle is not a figure anyone can read off.
 * Given a `total` larger than the slices, the difference is drawn as an
 * unfilled remainder -- spent of a budget, saved of a goal.
 */

export interface DonutSlice {
  readonly key: string;
  readonly label: string;
  /** Minor units. A slice at or below zero is listed but takes no arc. */
  readonly value: number;
}

export interface DonutProps {
  readonly slices: readonly DonutSlice[];
  readonly currency: string;
  readonly formatValue?: (value: number) => string;
  /** The whole the slices are shares of; their sum by default. */
  readonly total?: number;
  /** Two lines in the middle: what the ring is of. */
  readonly centerLabel?: string;
  readonly ariaLabel?: string;
  readonly emptyMessage?: string;
}

const SIZE = 160;
const OUTER = 76;
const INNER = 54;

function percent(share: number): string {
  return `${Math.round(share * 100)}%`;
}

export function Donut({
  slices,
  currency,
  formatValue,
  total,
  centerLabel,
  ariaLabel,
  emptyMessage = "Nothing to chart yet.",
}: DonutProps) {
  const format = formatValue ?? ((value: number) => formatMinorUnits(value, currency));
  const drawn = slices.filter((slice) => Number.isFinite(slice.value) && slice.value > 0);
  const sum = drawn.reduce((acc, slice) => acc + slice.value, 0);
  const whole = total !== undefined && total > sum ? total : sum;
  if (slices.length === 0 || whole <= 0) return <p className="muted chart-empty">{emptyMessage}</p>;

  const center = SIZE / 2;
  let angle = 0;
  const arcs = drawn.map((slice) => {
    const start = angle;
    angle += (slice.value / whole) * Math.PI * 2;
    return { slice, start, end: angle };
  });
  const remainder = whole - sum;
  const label =
    ariaLabel ??
    `${centerLabel ?? "Breakdown"}: ${slices
      .map((slice) => `${slice.label} ${percent(Math.max(0, slice.value) / whole)}`)
      .join(", ")}`;

  return (
    <figure className="chart donut">
      <svg
        viewBox={`0 0 ${SIZE} ${SIZE}`}
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label={label}
      >
        {remainder > 0 ? (
          <path
            className="donut-remainder"
            d={annulusPath(center, center, OUTER, INNER, angle, Math.PI * 2)}
          />
        ) : null}
        {arcs.map(({ slice, start, end }) => (
          <path
            key={slice.key}
            className={`donut-arc ${paletteClass("chart-fill", slices.indexOf(slice))}`}
            d={annulusPath(center, center, OUTER, INNER, start, end)}
          >
            <title>{`${slice.label}: ${format(slice.value)} (${percent(slice.value / whole)})`}</title>
          </path>
        ))}
        <text className="donut-center-value" x={center} y={center - 4} textAnchor="middle">
          {format(total ?? sum)}
        </text>
        {centerLabel !== undefined ? (
          <text className="donut-center-label" x={center} y={center + 14} textAnchor="middle">
            {centerLabel}
          </text>
        ) : null}
      </svg>
      <ol className="chart-legend donut-legend">
        {slices.map((slice, index) => (
          <li key={slice.key} data-key={slice.key}>
            <span className={`chart-swatch ${paletteClass("chart-swatch", index)}`} />
            <span className="donut-legend-label">{slice.label}</span>
            <span className="donut-legend-share">{percent(Math.max(0, slice.value) / whole)}</span>
            <span className="donut-legend-value">{format(slice.value)}</span>
          </li>
        ))}
        {remainder > 0 ? (
          <li className="donut-legend-remainder">
            <span className="chart-swatch chart-swatch-muted" />
            <span className="donut-legend-label">Remaining</span>
            <span className="donut-legend-share">{percent(remainder / whole)}</span>
            <span className="donut-legend-value">{format(remainder)}</span>
          </li>
        ) : null}
      </ol>
    </figure>
  );
}
