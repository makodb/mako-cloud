import type { KeyboardEvent } from "react";

import { formatMinorUnits } from "../../selectors/money.js";
import { squarify } from "../../selectors/treemap.js";
import { paletteClass } from "./bars.jsx";

/**
 * Spending as area: the squarified layout from the selectors, drawn.
 *
 * Text is the one thing a treemap cannot lay out exactly without measuring,
 * so each rectangle decides from its own size, in viewBox units, whether it
 * has room for a label and a value, a label, or nothing; a rectangle that
 * says nothing still has a title, and every rectangle is in the hidden table.
 */

export interface TreemapEntry {
  readonly key: string;
  readonly label: string;
  /** Minor units; nothing at or below zero is drawn. */
  readonly value: number;
}

export interface TreemapProps {
  readonly items: readonly TreemapEntry[];
  readonly currency: string;
  readonly formatValue?: (value: number) => string;
  /** ViewBox units; the drawing scales to its container. */
  readonly width?: number;
  readonly height?: number;
  /** Makes each rectangle a button. */
  readonly onSelect?: (key: string) => void;
  readonly ariaLabel?: string;
  readonly emptyMessage?: string;
}

/** Roughly how wide a run of text is at the label size, in viewBox units. */
function textWidth(text: string, fontSize: number): number {
  return text.length * fontSize * 0.58;
}

const LABEL_SIZE = 12;
const VALUE_SIZE = 11;
const PAD = 6;

export function Treemap({
  items,
  currency,
  formatValue,
  width = 640,
  height = 320,
  onSelect,
  ariaLabel,
  emptyMessage = "Nothing to chart yet.",
}: TreemapProps) {
  const format = formatValue ?? ((value: number) => formatMinorUnits(value, currency));
  const rects = squarify(items, width, height);
  if (rects.length === 0) return <p className="muted chart-empty">{emptyMessage}</p>;
  const byKey = new Map(items.map((item) => [item.key, item]));
  const total = items.reduce((sum, item) => sum + Math.max(0, item.value), 0);
  const label =
    ariaLabel ??
    `Treemap of ${rects.length} items: ${rects
      .map((rect) => byKey.get(rect.key))
      .filter((item) => item !== undefined)
      .map((item) => `${item.label} ${format(item.value)}`)
      .join(", ")}`;

  return (
    <figure className="chart treemap">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label={label}
      >
        {rects.map((rect, index) => {
          const item = byKey.get(rect.key);
          if (item === undefined || rect.width <= 0 || rect.height <= 0) return null;
          const value = format(item.value);
          const share = total > 0 ? ` (${Math.round((item.value / total) * 100)}%)` : "";
          const fitsLabel =
            rect.height >= LABEL_SIZE + PAD * 2 &&
            rect.width >= textWidth(item.label, LABEL_SIZE) + PAD * 2;
          const fitsValue =
            fitsLabel &&
            rect.height >= LABEL_SIZE + VALUE_SIZE + PAD * 3 &&
            rect.width >= textWidth(value, VALUE_SIZE) + PAD * 2;
          const title = `${item.label}: ${value}${share}`;
          const className = `treemap-cell ${paletteClass("chart-fill", index)}`;
          const content = (
            <>
              <title>{title}</title>
              <rect x={rect.x} y={rect.y} width={rect.width} height={rect.height} />
              {fitsLabel ? (
                <text
                  className="treemap-label"
                  x={rect.x + PAD}
                  y={rect.y + PAD + LABEL_SIZE * 0.85}
                >
                  {item.label}
                </text>
              ) : null}
              {fitsValue ? (
                <text
                  className="treemap-value"
                  x={rect.x + PAD}
                  y={rect.y + PAD * 2 + LABEL_SIZE + VALUE_SIZE * 0.85}
                >
                  {value}
                </text>
              ) : null}
            </>
          );
          if (onSelect === undefined) {
            return (
              <g key={rect.key} className={className} data-key={rect.key} aria-label={title}>
                {content}
              </g>
            );
          }
          const activate = (event: KeyboardEvent<SVGGElement>): void => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              onSelect(item.key);
            }
          };
          return (
            // biome-ignore lint/a11y/useSemanticElements: SVG has no <button>; a focusable <g> with the role is how a drawn rectangle becomes one.
            <g
              key={rect.key}
              className={className}
              data-key={rect.key}
              role="button"
              tabIndex={0}
              aria-label={title}
              onClick={() => onSelect(item.key)}
              onKeyDown={activate}
            >
              {content}
            </g>
          );
        })}
      </svg>
      <table className="visually-hidden">
        <caption>{label}</caption>
        <tbody>
          {rects.map((rect) => {
            const item = byKey.get(rect.key);
            if (item === undefined) return null;
            return (
              <tr key={rect.key}>
                <th scope="row">{item.label}</th>
                <td>{format(item.value)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </figure>
  );
}
