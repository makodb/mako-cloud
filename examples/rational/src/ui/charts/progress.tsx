/**
 * A bar of how far along something is, and a meter that judges it.
 *
 * The bar says nothing about whether full is good -- a goal filling up is,
 * a budget filling up is not -- so its tone is the caller's. The meter is for
 * the second case: it turns to warn near the limit and to danger past it.
 */

export type ProgressTone = "accent" | "positive" | "danger" | "warn";

export interface ProgressBarProps {
  readonly value: number;
  readonly max: number;
  readonly label?: string;
  readonly tone?: ProgressTone;
}

function share(value: number, max: number): number {
  if (!(max > 0) || !Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value / max));
}

export function ProgressBar({ value, max, label, tone = "accent" }: ProgressBarProps) {
  const filled = share(value, max);
  const over = max > 0 && value > max;
  return (
    <div
      className={`progress${over ? " progress-over" : ""}`}
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={Math.max(0, max)}
      aria-valuenow={Math.max(0, Math.min(value, max))}
    >
      <div className={`progress-fill tone-${tone}`} style={{ width: `${filled * 100}%` }} />
    </div>
  );
}

export interface MeterProps {
  readonly value: number;
  readonly max: number;
  readonly label?: string;
  /** Of `max`, where the tone turns to warn; 0.85 by default. */
  readonly warnAt?: number;
}

/** A progress bar for a limit: calm under it, warning near it, danger past it. */
export function Meter({ value, max, label, warnAt = 0.85 }: MeterProps) {
  const ratio = max > 0 ? value / max : 0;
  const tone: ProgressTone = ratio > 1 ? "danger" : ratio >= warnAt ? "warn" : "accent";
  return (
    <div
      className={`progress meter${ratio > 1 ? " progress-over" : ""}`}
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={Math.max(0, max)}
      aria-valuenow={Math.max(0, value)}
    >
      <div
        className={`progress-fill tone-${tone}`}
        style={{ width: `${share(value, max) * 100}%` }}
      />
    </div>
  );
}
